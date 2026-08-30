//! A shell held open inside a world, so a command costs a write rather than a
//! process.
//!
//! Everything this app does inside a WSL distro or on an SSH host has, until
//! now, been its own `wsl.exe` or `ssh` — and on Windows a process is the
//! expensive part. Phase 1 cut the *number* of questions by asking several at
//! once; this cuts the price of asking at all. One `sh` is started per world
//! and kept, and every later command is a line written to its stdin.
//!
//! **It is an optimisation, never a dependency.** Every path here falls back
//! to spawning the command the old way: a world where the channel will not
//! open, a shell that died, a command too large to push through a pipe, or a
//! moment when every channel in the pool is busy. That is what makes it safe
//! to put in front of every command — the worst case is the behaviour this
//! app already had.
//!
//! ## The frame
//!
//! A command is written as one line that redirects its own streams to files
//! inside the world, then announces what happened:
//!
//! ```text
//! MAROL <exit status> <stdout bytes> <stderr bytes>\n
//! <stdout bytes><stderr bytes>
//! ```
//!
//! Byte counts rather than a terminator, because output is bytes: a
//! transcript read comes back as whatever the file holds, and any marker
//! chosen to end it would eventually appear inside one. Counting is the only
//! framing that cannot be spoofed by content.
//!
//! The command's own stdin is `/dev/null`. Without that, a program that reads
//! stdin would eat the command stream itself and the channel would never
//! recover.

use anyhow::{anyhow, Result};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;

use crate::host::{sh_quote, Host};
use crate::shell_env::ShellEnv;

/// How many shells one world keeps open.
///
/// More than one because commands run concurrently now — the Tauri commands
/// were moved onto a blocking pool, and the hook path has always had threads
/// of its own — and a single shell would serialise what used to overlap.
/// Small because each is an idle process in somebody's distro, and a caller
/// that finds them all busy is not blocked: it spawns the command the old
/// way, which is exactly what it would have done anyway.
const POOL: usize = 4;

/// The longest command line pushed through a pipe.
///
/// `write_file` carries a file's whole contents as an argument, and a big
/// enough one is better handed to its own process than pushed through a pipe
/// a shell is reading a line at a time.
const MAX_LINE: usize = 128 * 1024;

/// What a command did, in the shape `std::process::Output` has, because every
/// caller here already reads that.
pub struct Answer {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// The preamble every channel runs before its first command: somewhere to put
/// each command's streams, inside the world, named for this shell alone.
const PREAMBLE: &str = concat!(
    "__d=${TMPDIR:-/tmp}; __o=$__d/.marol-ch-$$-o; __e=$__d/.marol-ch-$$-e; ",
    "trap 'rm -f \"$__o\" \"$__e\"' EXIT; ",
    "echo MAROL-READY\n"
);

struct Channel {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Channel {
    /// Start one shell inside `host` and wait for it to say it is there.
    fn open(host: &Host, local: &ShellEnv, env: &ShellEnv) -> Result<Self> {
        let (program, args) = host.wrap_channel("sh", &crate::host::carry_env_pub(env));
        let exe = local
            .which(&program)
            .unwrap_or_else(|| std::path::PathBuf::from(&program));
        let mut child = Command::new(exe)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?);
        let mut ch = Channel {
            child,
            stdin,
            stdout,
        };
        ch.stdin.write_all(PREAMBLE.as_bytes())?;
        ch.stdin.flush()?;
        // A world that cannot start a shell says so now, once, rather than
        // once per command for the life of the app.
        let mut hello = String::new();
        ch.stdout.read_line(&mut hello)?;
        if hello.trim() != "MAROL-READY" {
            let _ = ch.child.kill();
            return Err(anyhow!("the shell did not answer: {hello:?}"));
        }
        Ok(ch)
    }

    fn run(&mut self, line: &str) -> Result<Answer> {
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;

        let mut head = String::new();
        loop {
            head.clear();
            if self.stdout.read_line(&mut head)? == 0 {
                return Err(anyhow!("the shell closed mid-command"));
            }
            if head.starts_with("MAROL ") {
                break;
            }
            // Nothing else should reach this pipe — the command's own streams
            // go to files. A line that is not the frame means the protocol is
            // out of step, and guessing past it would answer one command with
            // another's output.
            return Err(anyhow!("unframed output: {head:?}"));
        }
        let mut parts = head.trim().split(' ').skip(1);
        let mut next = || -> Result<usize> {
            parts
                .next()
                .and_then(|n| n.parse::<usize>().ok())
                .ok_or_else(|| anyhow!("malformed frame: {head:?}"))
        };
        let status = next()? as i32;
        let out_len = next()?;
        let err_len = next()?;

        let mut stdout = vec![0u8; out_len];
        self.stdout.read_exact(&mut stdout)?;
        let mut stderr = vec![0u8; err_len];
        self.stdout.read_exact(&mut stderr)?;
        Ok(Answer {
            status,
            stdout,
            stderr,
        })
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        // Closing stdin is how a shell reading a script is told to stop; the
        // kill is for one that is wedged inside a command.
        let _ = self.stdin.write_all(b"exit\n");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One world's shells.
#[derive(Default)]
pub struct Channels {
    slots: Vec<Mutex<Option<Channel>>>,
}

impl Channels {
    pub fn new() -> Self {
        Channels {
            slots: (0..POOL).map(|_| Mutex::new(None)).collect(),
        }
    }

    /// Run one command through a held shell, or `None` to say the caller
    /// should spawn it itself.
    ///
    /// `None` is not a failure — it is this layer declining, and every caller
    /// has the old path to fall back on. A channel that answers wrongly is
    /// dropped rather than reused: a shell whose framing has slipped would
    /// answer the *next* command with this one's output, which is worse than
    /// any latency it was saving.
    pub fn run(
        &self,
        host: &Host,
        local: &ShellEnv,
        env: &ShellEnv,
        program: &str,
        args: &[&str],
        cwd: Option<&str>,
        extra: &[(String, String)],
    ) -> Option<Answer> {
        if self.slots.is_empty() {
            return None;
        }
        let line = compose(program, args, cwd, extra);
        if line.len() > MAX_LINE {
            return None;
        }
        for slot in &self.slots {
            // `try_lock`, never `lock`: waiting for a busy shell would make
            // this slower than the spawn it replaces under exactly the
            // concurrency it was added for.
            let Ok(mut guard) = slot.try_lock() else {
                continue;
            };
            if guard.is_none() {
                match Channel::open(host, local, env) {
                    Ok(ch) => *guard = Some(ch),
                    Err(e) => {
                        eprintln!("[channel] {host:?} would not hold a shell: {e:#}");
                        return None;
                    }
                }
            }
            let answer = guard.as_mut().and_then(|ch| match ch.run(&line) {
                Ok(a) => Some(a),
                Err(e) => {
                    eprintln!("[channel] {host:?} dropped a shell: {e:#}");
                    None
                }
            });
            if answer.is_none() {
                *guard = None;
            }
            return answer;
        }
        None
    }

    /// Let go of every shell. Called on the way out, so a world is not left
    /// holding an idle process for an app that has gone.
    pub fn close(&self) {
        for slot in &self.slots {
            if let Ok(mut guard) = slot.lock() {
                *guard = None;
            }
        }
    }
}

/// The one line a command becomes.
///
/// Every word is quoted exactly once — the same armouring the SSH path has
/// always applied, now applied to the WSL path too, which used to hand argv
/// across untouched. `</dev/null` is load-bearing: a program that read stdin
/// would otherwise read the command stream.
fn compose(program: &str, args: &[&str], cwd: Option<&str>, extra: &[(String, String)]) -> String {
    let mut cmd = String::from("{ ");
    if let Some(dir) = cwd {
        cmd.push_str(&format!("cd {} && ", sh_quote(dir)));
    }
    cmd.push_str("env");
    for (k, v) in extra {
        cmd.push(' ');
        cmd.push_str(&sh_quote(&format!("{k}={v}")));
    }
    cmd.push(' ');
    cmd.push_str(&sh_quote(program));
    for a in args {
        cmd.push(' ');
        cmd.push_str(&sh_quote(a));
    }
    cmd.push_str(" ; } >\"$__o\" 2>\"$__e\" </dev/null; __s=$?; ");
    cmd.push_str("__ol=$(wc -c <\"$__o\"); __el=$(wc -c <\"$__e\"); ");
    cmd.push_str("printf 'MAROL %s %s %s\\n' \"$__s\" $__ol $__el; cat \"$__o\"; cat \"$__e\"");
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frame is the contract, so the line that produces it is pinned.
    #[test]
    fn a_command_carries_its_directory_its_env_and_nothing_of_the_shells() {
        let line = compose(
            "git",
            &["status", "--porcelain"],
            Some("/home/me/wt"),
            &[("GIT_INDEX_FILE".into(), "/tmp/idx".into())],
        );
        assert!(line.starts_with("{ cd '/home/me/wt' && env 'GIT_INDEX_FILE=/tmp/idx' 'git'"));
        // Its own stdin is closed off, or a program that reads one would eat
        // the command stream and the channel would never recover.
        assert!(line.contains("</dev/null"), "{line}");
        // Both streams are captured, then measured, then handed over.
        assert!(line.contains(">\"$__o\" 2>\"$__e\""), "{line}");
        assert!(line.contains("printf 'MAROL %s %s %s"), "{line}");
    }

    /// The quoting is the whole safety of pushing argv through a shell, and
    /// the WSL path never needed it before — `wsl.exe -e` handed argv over
    /// intact. Anything a prompt or a filename can contain has to survive.
    #[test]
    fn every_word_is_armoured_exactly_once() {
        let line = compose("sh", &["-c", "echo $HOME `id`", "it's"], None, &[]);
        assert!(line.contains(r"'echo $HOME `id`'"), "{line}");
        assert!(line.contains(r"'it'\''s'"), "{line}");
    }

    /// One command per line, always: a newline inside an argument must ride
    /// inside its quotes rather than ending the line the shell is reading.
    #[test]
    fn a_newline_in_an_argument_does_not_end_the_command() {
        let line = compose("claude", &["a\nb"], None, &[]);
        let head = line.split('\n').next().unwrap_or_default();
        assert!(head.contains("'a"), "the argument was cut in half: {head}");
        assert_eq!(line.matches('\n').count(), 1, "more than one line: {line:?}");
    }
}
