//! A shell held open inside a world, so a command costs a write rather than a
//! process.
//!
//! Everything this app does inside a WSL distro or on an SSH host has, until
//! now, been its own `wsl.exe` or `ssh` — and on Windows a process is the
//! expensive part. Phase 1 cut the *number* of questions by asking several at
//! once; this cuts the price of asking at all. One `sh` is started per world
//! and kept, and every later command is a line written to its stdin.
//!
//! **It is an optimisation, never a dependency.** A world where the channel
//! will not open, a command too large to push through a pipe, or a moment
//! when every shell in the pool is busy all fall back to spawning the command
//! the old way, which is exactly what this app did before channels existed.
//!
//! There is one failure that must *not* fall back, and telling it apart from
//! the others is what `Outcome` is for. Once a command has been written to a
//! shell, that shell going quiet does not mean the command did not run — `git
//! commit` writes its commit and then the pipe breaks just the same. Spawning
//! it again would be a second commit. So a failure after the line was sent is
//! reported as lost, not declined, and the caller raises it instead of
//! retrying: a slow answer is worth waiting for, a doubled mutation is not
//! worth anything.
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

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
#[derive(Debug)]
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

/// How long a shell may go without answering before it is given up on.
///
/// Not a latency budget. Nothing here had one before — a spawned `wsl.exe`
/// blocked its thread for as long as the command took, and still does — and a
/// budget tight enough to be one would fire on a clone over a slow link and
/// call a working command lost. This is the other thing entirely: the point
/// past which a shell is not slow but stuck, so that the slot it holds and
/// the thread waiting on it come back instead of being gone for the life of
/// the app. Set far above anything this app legitimately runs, so expiring
/// always means something is actually wrong.
const SILENCE: Duration = Duration::from_secs(300);

/// What came of handing one command to a held shell.
pub enum Outcome {
    /// It ran, and this is what it said.
    Ran(Answer),
    /// Nothing was sent. Spawn it the old way — that is the whole point of
    /// the fallback, and it costs only the process it was going to cost.
    Declined,
    /// It was sent and no answer came back, so whether it ran is unknown.
    /// Must not be retried; the caller raises this instead.
    Lost(String),
}

/// A command that produced no answer, and whether the shell had already been
/// given it — the one fact that decides between spawning again and refusing.
struct Failed {
    launched: bool,
    why: anyhow::Error,
}

/// The deadline one shell is working against, shared with its watchdog.
struct Watch {
    /// `Some` while a command is in flight, holding when to give up on it.
    until: Mutex<Option<Instant>>,
    wake: Condvar,
    /// Set by the watchdog when it killed the shell, so `run` can say why
    /// the pipe went quiet rather than blaming the shell for closing.
    expired: AtomicBool,
    /// Cleared when the channel is dropped, so the watchdog goes home rather
    /// than outliving the shell it watches.
    alive: AtomicBool,
}

struct Channel {
    /// Shared because the watchdog kills it, and killing needs `&mut`. The
    /// lock is held only for that kill and for the one in `Drop`, never
    /// across a read, so the reading thread is never waiting on it.
    child: Arc<Mutex<Child>>,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    watch: Arc<Watch>,
    silence: Duration,
}

/// Wait out one shell's deadline, and kill it if it passes.
///
/// Killing is what makes the wait end: there is no timeout on reading a pipe,
/// so the only way to release a thread blocked on one is to close the other
/// end. The shell dies; whatever it had started does not, and may well finish
/// — which is exactly why the read failure this produces is `Lost` rather
/// than a reason to run the command again.
fn watchdog(child: Arc<Mutex<Child>>, watch: Arc<Watch>) {
    let mut until = watch.until.lock().unwrap();
    loop {
        if !watch.alive.load(Ordering::SeqCst) {
            return;
        }
        match *until {
            // Parked between commands, costing nothing until one arms it.
            None => until = watch.wake.wait(until).unwrap(),
            Some(at) => {
                let now = Instant::now();
                if now < at {
                    until = watch.wake.wait_timeout(until, at - now).unwrap().0;
                    continue;
                }
                watch.expired.store(true, Ordering::SeqCst);
                *until = None;
                drop(until);
                let _ = child.lock().unwrap().kill();
                return;
            }
        }
    }
}

impl Channel {
    /// Start one shell inside `host` and wait for it to say it is there.
    fn open(host: &Host, local: &ShellEnv, env: &ShellEnv, silence: Duration) -> Result<Self> {
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
            child: Arc::new(Mutex::new(child)),
            stdin,
            stdout,
            watch: Arc::new(Watch {
                until: Mutex::new(None),
                wake: Condvar::new(),
                expired: AtomicBool::new(false),
                alive: AtomicBool::new(true),
            }),
            silence,
        };
        ch.stdin.write_all(PREAMBLE.as_bytes())?;
        ch.stdin.flush()?;
        // A world that cannot start a shell says so now, once, rather than
        // once per command for the life of the app.
        //
        // Unwatched on purpose: this read is the one that has no command
        // behind it, so a shell that never says hello is a shell nothing was
        // entrusted to. `Channel::open` failing is `Declined`, and declining
        // is free.
        let mut hello = String::new();
        ch.stdout.read_line(&mut hello)?;
        if hello.trim() != "MAROL-READY" {
            let _ = ch.child.lock().unwrap().kill();
            return Err(anyhow!("the shell did not answer: {hello:?}"));
        }
        // One watchdog per shell, not per command: a thread apiece for the
        // four a world may hold is a cost worth paying once, and a thread per
        // command would reintroduce the per-command price this file exists to
        // remove.
        let (child, watch) = (Arc::clone(&ch.child), Arc::clone(&ch.watch));
        std::thread::spawn(move || watchdog(child, watch));
        Ok(ch)
    }

    /// Arm the deadline, run the exchange, disarm it.
    fn run(&mut self, line: &str) -> std::result::Result<Answer, Failed> {
        *self.watch.until.lock().unwrap() = Some(Instant::now() + self.silence);
        self.watch.wake.notify_all();
        let out = self.exchange(line);
        *self.watch.until.lock().unwrap() = None;
        self.watch.wake.notify_all();
        match out {
            Ok(a) => Ok(a),
            // The watchdog got there first, so the pipe closing is our own
            // doing and saying "the shell closed" would blame the shell for
            // what this did to it.
            Err(f) if self.watch.expired.load(Ordering::SeqCst) => Err(Failed {
                launched: f.launched,
                why: anyhow!(
                    "no answer in {:?}, so the shell was given up on",
                    self.silence
                ),
            }),
            Err(f) => Err(f),
        }
    }

    /// One command down the pipe and its answer back.
    ///
    /// Every failure says whether the shell had been given the command,
    /// because that is what the caller needs and nothing further up can work
    /// it out afterwards. The newline is the line the fact turns on: `sh`
    /// does not begin a line it has not been given the end of, so a write
    /// that reported failure delivered nothing, and everything after it is a
    /// command whose fate this process can no longer see.
    fn exchange(&mut self, line: &str) -> std::result::Result<Answer, Failed> {
        let unsent = |e: std::io::Error| Failed {
            launched: false,
            why: e.into(),
        };
        let lost = |e: anyhow::Error| Failed {
            launched: true,
            why: e,
        };
        self.stdin.write_all(line.as_bytes()).map_err(unsent)?;
        self.stdin.write_all(b"\n").map_err(unsent)?;
        self.stdin.flush().map_err(unsent)?;

        let mut head = String::new();
        loop {
            head.clear();
            if self
                .stdout
                .read_line(&mut head)
                .map_err(|e| lost(e.into()))?
                == 0
            {
                return Err(lost(anyhow!("the shell closed mid-command")));
            }
            if head.starts_with("MAROL ") {
                break;
            }
            // Nothing else should reach this pipe — the command's own streams
            // go to files. A line that is not the frame means the protocol is
            // out of step, and guessing past it would answer one command with
            // another's output.
            return Err(lost(anyhow!("unframed output: {head:?}")));
        }
        let mut parts = head.trim().split(' ').skip(1);
        let mut next = || -> Result<usize> {
            parts
                .next()
                .and_then(|n| n.parse::<usize>().ok())
                .ok_or_else(|| anyhow!("malformed frame: {head:?}"))
        };
        let status = next().map_err(lost)? as i32;
        let out_len = next().map_err(lost)?;
        let err_len = next().map_err(lost)?;

        let mut stdout = vec![0u8; out_len];
        self.stdout.read_exact(&mut stdout).map_err(|e| lost(e.into()))?;
        let mut stderr = vec![0u8; err_len];
        self.stdout.read_exact(&mut stderr).map_err(|e| lost(e.into()))?;
        Ok(Answer {
            status,
            stdout,
            stderr,
        })
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        // The watchdog first, so it cannot outlive the shell it watches and
        // wake up to kill a pid the OS has since given to somebody else.
        self.watch.alive.store(false, Ordering::SeqCst);
        *self.watch.until.lock().unwrap() = None;
        self.watch.wake.notify_all();
        // Closing stdin is how a shell reading a script is told to stop; the
        // kill is for one that is wedged inside a command.
        let _ = self.stdin.write_all(b"exit\n");
        let mut child = self.child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// One world's shells.
#[derive(Default)]
pub struct Channels {
    slots: Vec<Mutex<Option<Channel>>>,
    /// How long a shell here may go quiet. A field rather than the constant
    /// read directly, so a test can prove the watchdog fires without waiting
    /// out a timeout scaled for real work.
    silence: Duration,
    counters: Counters,
}

/// What this world's channel has actually done, since the app started.
///
/// Kept because the win is otherwise invisible. Declining is designed to be
/// silent and correct, which means a world where the channel never opens at
/// all — no `sh` on the far side, every command over `MAX_LINE`, a pool that
/// is always contended — behaves exactly like a world where it is working
/// perfectly, only slower. The tests pin the crossing count at zero on a
/// bench; this is the same question asked of the machine somebody is using.
#[derive(Default, Debug)]
pub struct Tally {
    /// Answered by a shell that was already open. No process was started.
    pub held: u64,
    /// Handed back for the caller to spawn the old way.
    pub spawned: u64,
    /// Sent, and never answered. Raised rather than retried.
    pub lost: u64,
}

#[derive(Default)]
struct Counters {
    held: AtomicU64,
    spawned: AtomicU64,
    lost: AtomicU64,
}


impl Channels {
    pub fn new() -> Self {
        Channels {
            slots: (0..POOL).map(|_| Mutex::new(None)).collect(),
            silence: SILENCE,
            counters: Counters::default(),
        }
    }

    #[cfg(test)]
    fn with_silence(silence: Duration) -> Self {
        Channels {
            slots: (0..POOL).map(|_| Mutex::new(None)).collect(),
            silence,
            counters: Counters::default(),
        }
    }

    /// Run one command through a held shell.
    ///
    /// `Declined` is not a failure — it is this layer standing aside, and
    /// every caller has the old path to fall back on. A shell that answers
    /// wrongly is dropped rather than reused: one whose framing has slipped
    /// would answer the *next* command with this one's output, which is worse
    /// than any latency it was saving.
    ///
    /// `Lost` is the case that must not be confused with declining. Both are
    /// "no answer", and the difference is only whether the command was
    /// already sent — but on that hangs whether spawning it again is free or
    /// is a second `git commit`.
    pub fn run(
        &self,
        host: &Host,
        local: &ShellEnv,
        env: &ShellEnv,
        program: &str,
        args: &[&str],
        cwd: Option<&str>,
        extra: &[(String, String)],
    ) -> Outcome {
        if self.slots.is_empty() {
            return self.declined();
        }
        let line = compose(program, args, cwd, extra);
        if line.len() > MAX_LINE {
            return self.declined();
        }
        for slot in &self.slots {
            // `try_lock`, never `lock`: waiting for a busy shell would make
            // this slower than the spawn it replaces under exactly the
            // concurrency it was added for.
            let Ok(mut guard) = slot.try_lock() else {
                continue;
            };
            if guard.is_none() {
                match Channel::open(host, local, env, self.silence) {
                    Ok(ch) => *guard = Some(ch),
                    Err(e) => {
                        eprintln!("[channel] {host:?} would not hold a shell: {e:#}");
                        return self.declined();
                    }
                }
            }
            let Some(ch) = guard.as_mut() else {
                return self.declined();
            };
            return match ch.run(&line) {
                Ok(a) => {
                    self.counters.held.fetch_add(1, Ordering::Relaxed);
                    Outcome::Ran(a)
                }
                Err(f) => {
                    // Dropped either way: whatever state this shell is in, it
                    // is not one to hand the next command to.
                    eprintln!("[channel] {host:?} dropped a shell: {:#}", f.why);
                    *guard = None;
                    if f.launched {
                        self.counters.lost.fetch_add(1, Ordering::Relaxed);
                        Outcome::Lost(format!("{:#}", f.why))
                    } else {
                        self.declined()
                    }
                }
            };
        }
        // Every shell busy. The commonest reason to decline, and the one
        // worth telling apart from the rest by the count alone: a pool that
        // is always contended is a pool that wants to be bigger.
        self.declined()
    }

    fn declined(&self) -> Outcome {
        self.counters.spawned.fetch_add(1, Ordering::Relaxed);
        Outcome::Declined
    }

    /// What this world's channel has done so far.
    pub fn tally(&self) -> Tally {
        Tally {
            held: self.counters.held.load(Ordering::Relaxed),
            spawned: self.counters.spawned.load(Ordering::Relaxed),
            lost: self.counters.lost.load(Ordering::Relaxed),
        }
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

    /// A local `sh`, which is what `Host::Local` wraps a channel into. Unix
    /// only, because the thing being tested is only ever reached through a
    /// doorway — a WSL distro or an SSH host — and both have a `sh` on the
    /// far side by construction. Windows has no local one to stand in.
    #[cfg(unix)]
    fn bare_env() -> ShellEnv {
        ShellEnv {
            vars: std::collections::HashMap::new(),
            shell: "sh".to_string(),
            resolved: true,
        }
    }

    /// The distinction the whole `Outcome` split exists for.
    ///
    /// A shell that dies *after* being handed a command has not declined it —
    /// the command may have run to completion and taken its effects with it.
    /// If that came back as "declined", the layer above would spawn the same
    /// `git commit` a second time, so the flag is checked here at its source.
    #[cfg(unix)]
    #[test]
    fn a_shell_that_dies_holding_a_command_reports_it_lost_not_declined() {
        let env = bare_env();
        let mut ch = Channel::open(&Host::Local, &env, &env, Duration::from_secs(30))
            .expect("a local sh");
        // It runs, and then there is no shell left to say so — exactly the
        // shape of a distro going away mid-command.
        let failed = ch.run("kill -9 $$").expect_err("the shell answered its own death");
        assert!(
            failed.launched,
            "a command the shell had already been given was called unsent: {:#}",
            failed.why
        );
    }

    /// Nothing written, nothing to be unsure about. A shell that is already
    /// gone before the line reaches it declines, and declining is free.
    #[cfg(unix)]
    #[test]
    fn a_command_that_never_reached_the_shell_is_safe_to_spawn() {
        let env = bare_env();
        let mut ch = Channel::open(&Host::Local, &env, &env, Duration::from_secs(30))
            .expect("a local sh");
        ch.child.lock().unwrap().kill().expect("kill");
        let _ = ch.child.lock().unwrap().wait();
        // A pipe whose reader is gone does not necessarily refuse the first
        // write — the buffer may still take it, and then the *read* is what
        // fails, which is the other case and is reported as such. Keep going
        // until the pipe itself says no: that is the case under test, and the
        // one where nothing can have run.
        let mut unsent = None;
        for _ in 0..64 {
            match ch.run("echo hi") {
                Ok(_) => panic!("a killed shell answered a command"),
                Err(f) if !f.launched => {
                    unsent = Some(f);
                    break;
                }
                Err(_) => continue,
            }
        }
        assert!(
            unsent.is_some(),
            "a write to a dead pipe never reported itself as unsent"
        );
    }

    /// The watchdog's whole job: a shell that goes quiet is given up on, the
    /// slot it was holding comes back, and the next command gets a fresh one.
    /// Without the kill there is no way to end a blocking read on a pipe, so
    /// this is also the test that the thread waiting on it is released.
    #[cfg(unix)]
    #[test]
    fn a_silent_shell_is_given_up_on_and_its_slot_returns() {
        let env = bare_env();
        let chs = Channels::with_silence(Duration::from_millis(200));

        let started = Instant::now();
        let out = chs.run(&Host::Local, &env, &env, "sleep", &["30"], None, &[]);
        // Lost, not declined: `sleep` was handed over before the silence.
        let why = match out {
            Outcome::Lost(why) => why,
            Outcome::Ran(_) => panic!("a 30-second sleep answered inside 200ms"),
            Outcome::Declined => panic!("the command was sent, so declining would invite a re-run"),
        };
        assert!(why.contains("given up on"), "the reason blames the wrong thing: {why}");
        assert!(
            started.elapsed() < Duration::from_secs(25),
            "the read outlived the deadline by {:?}",
            started.elapsed()
        );

        // And the world is not poisoned: the slot was dropped, so the next
        // command opens a new shell and is answered normally.
        match chs.run(&Host::Local, &env, &env, "echo", &["awake"], None, &[]) {
            Outcome::Ran(a) => assert_eq!(String::from_utf8_lossy(&a.stdout).trim(), "awake"),
            Outcome::Lost(why) => panic!("the replacement shell was lost too: {why}"),
            Outcome::Declined => panic!("every slot stayed wedged after one bad command"),
        }
    }
}
