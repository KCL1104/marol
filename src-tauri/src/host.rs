//! Where a repository — and everything that runs against it — actually lives.
//!
//! The app runs *here*; a repository can live *somewhere else that can run
//! commands*: a WSL distro today, an SSH host next. Everything Marol does
//! with an environment reduces to three acts — spawn a PTY, run a command and
//! read its output, receive a hook callback — so a host is exactly the thing
//! that wraps the first two. (The third degrades gracefully by design: status
//! reporting is a nicety, sessions are not.)
//!
//! A host rides inside the paths the app already stores, so nothing about the
//! database changes shape:
//!
//! ```text
//! /Users/me/code/app              the machine the app runs on
//! wsl://Ubuntu/home/me/code/app   the Ubuntu distro under WSL
//! ssh://devbox/home/me/app        the `devbox` alias in ~/.ssh/config
//! ```
//!
//! Inside a non-local host every path is the host's own (POSIX), and is
//! handled as a string: `PathBuf` on Windows joins with backslashes, which
//! would quietly corrupt a WSL path.
//!
//! Two environments are always in play, and `HostRef` carries both: the app
//! machine's login environment finds the *doorway* (`wsl.exe`), and the
//! host's own resolved environment finds what runs *inside* (`claude`, `git`,
//! `gh`). Environment variables do not cross the WSL boundary on their own
//! (that is WSLENV's job, and it needs per-variable annotations), so commands
//! are wrapped as `wsl.exe -d <distro> --cd <dir> -e env K=V… <program>
//! <args…>`: `-e` skips the shell so argv — including a multi-line prompt —
//! arrives exactly as sent, and POSIX `env` carries the variables and
//! resolves the program against the PATH it was handed.

use anyhow::{anyhow, Result};

use crate::shell_env::ShellEnv;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Host {
    Local,
    Wsl { distro: String },
    /// A host from the user's own `~/.ssh/config`, named by its alias there.
    /// The alias is the whole configuration: user, port, key, jump hosts —
    /// Marol invents no connection settings of its own.
    Ssh { host: String },
}

/// A stored path, split into who runs it and what it is called there.
#[derive(Debug, Clone, PartialEq)]
pub struct Located {
    pub host: Host,
    /// The path as the host itself sees it.
    pub path: String,
}

/// Read a stored path. Plain paths are local, `wsl://distro/...` is that
/// distro, `ssh://alias/...` is that host from the user's own ssh config.
pub fn locate(raw: &str) -> Result<Located> {
    if let Some(rest) = raw.strip_prefix("wsl://") {
        let (distro, path) = rest
            .split_once('/')
            .ok_or_else(|| anyhow!("`{raw}` names a distro but no path — wsl://<distro>/<path>"))?;
        if distro.is_empty() {
            return Err(anyhow!("`{raw}` names no distro — wsl://<distro>/<path>"));
        }
        return Ok(Located {
            host: Host::Wsl {
                distro: distro.to_string(),
            },
            path: format!("/{path}"),
        });
    }
    if let Some(rest) = raw.strip_prefix("ssh://") {
        let (alias, path) = rest
            .split_once('/')
            .ok_or_else(|| anyhow!("`{raw}` names a host but no path — ssh://<host>/<path>"))?;
        if alias.is_empty() {
            return Err(anyhow!("`{raw}` names no host — ssh://<host>/<path>"));
        }
        return Ok(Located {
            host: Host::Ssh {
                host: alias.to_string(),
            },
            path: format!("/{path}"),
        });
    }
    Ok(Located {
        host: Host::Local,
        path: raw.to_string(),
    })
}

/// Put a host-side path back into the form the app stores.
pub fn stored(host: &Host, path: &str) -> String {
    match host {
        Host::Local => path.to_string(),
        Host::Wsl { distro } => format!("wsl://{distro}{path}"),
        Host::Ssh { host } => format!("ssh://{host}{path}"),
    }
}

/// The short badge a card wears for a non-local host: `wsl:Ubuntu`.
pub fn label(host: &Host) -> Option<String> {
    match host {
        Host::Local => None,
        Host::Wsl { distro } => Some(format!("wsl:{distro}")),
        Host::Ssh { host } => Some(format!("ssh:{host}")),
    }
}

/// `C:\Users\me\x` → `/mnt/c/Users/me/x`, for handing an app-side file (the
/// hooks plugin) to a program inside WSL, which sees Windows drives mounted
/// under `/mnt`. A path that is not drive-lettered passes through — in tests
/// and on non-Windows hosts the two sides share one filesystem.
pub fn win_path_for_wsl(path: &str) -> String {
    let bytes = path.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest: String = path[3..].replace('\\', "/");
        return format!("/mnt/{drive}/{rest}");
    }
    path.to_string()
}

impl Host {
    /// Join a path the way the host spells paths. Non-local hosts are POSIX;
    /// using `PathBuf` for them on Windows would join with backslashes.
    pub fn join(&self, base: &str, leaf: &str) -> String {
        match self {
            Host::Local => std::path::Path::new(base)
                .join(leaf)
                .to_string_lossy()
                .to_string(),
            _ => format!("{}/{leaf}", base.trim_end_matches('/')),
        }
    }

    /// Wrap `program args…` so it runs inside this host, in `cwd`, with
    /// `envs` set. Returns what to actually spawn *here*, plus the working
    /// directory the outer process needs — `None` when the real cwd only
    /// exists inside the host.
    pub fn wrap<'a>(
        &self,
        program: &str,
        args: &[String],
        cwd: Option<&'a str>,
        envs: &[(String, String)],
    ) -> (String, Vec<String>, Option<&'a str>) {
        self.wrap_inner(program, args, cwd, envs, true)
    }

    /// `wrap`, for a command with nobody watching it.
    ///
    /// The difference is only ever SSH's, and it is two flags: no `-t`,
    /// because there is no terminal on this side to give one — ssh would warn
    /// on stderr and carry on — and `BatchMode`, so a host that wants a
    /// password fails in a moment instead of blocking for ever on a prompt
    /// that has no keyboard in front of it. The interactive session is the one
    /// place a prompt *can* be answered, so it keeps the tty; a `kill-server`
    /// fired from a close button is not.
    pub fn wrap_quiet(&self, program: &str, args: &[String]) -> (String, Vec<String>) {
        let (p, a, _) = self.wrap_inner(program, args, None, &[], false);
        (p, a)
    }

    /// `wrap`, for a long-lived shell on pipes rather than a terminal.
    ///
    /// No `-t`, because there is no terminal on this side to give one and the
    /// thing on the far end is reading a script; the environment still
    /// crosses, because it is what the commands inside will resolve against.
    pub fn wrap_channel(&self, program: &str, envs: &[(String, String)]) -> (String, Vec<String>) {
        let (p, a, _) = self.wrap_inner(program, &[], None, envs, false);
        (p, a)
    }

    fn wrap_inner<'a>(
        &self,
        program: &str,
        args: &[String],
        cwd: Option<&'a str>,
        envs: &[(String, String)],
        tty: bool,
    ) -> (String, Vec<String>, Option<&'a str>) {
        match self {
            // Locally the caller applies cwd and env natively, as it always
            // has; wrapping would only add a process to every spawn.
            Host::Local => (program.to_string(), args.to_vec(), cwd),
            Host::Wsl { distro } => {
                let mut wrapped = vec!["-d".to_string(), distro.clone()];
                if let Some(dir) = cwd {
                    wrapped.push("--cd".to_string());
                    wrapped.push(dir.to_string());
                }
                wrapped.push("-e".to_string());
                // `env` even when there is nothing to set: the wrapping has
                // one shape, and `env prog` is exactly `prog`.
                wrapped.push("env".to_string());
                for (k, v) in envs {
                    wrapped.push(format!("{k}={v}"));
                }
                wrapped.push(program.to_string());
                wrapped.extend(args.iter().cloned());
                ("wsl.exe".to_string(), wrapped, None)
            }
            Host::Ssh { host } => {
                let mut a = ssh_base_args(tty);
                a.push("--".to_string());
                a.push(host.clone());
                a.push(remote_command(program, args, cwd, envs));
                ("ssh".to_string(), a, None)
            }
        }
    }

    /// Resolve the host's own login environment — the same question
    /// `shell_env::resolve` answers locally: what PATH, and therefore which
    /// `claude`, `git` and `gh`, does a person's terminal in this host get.
    ///
    /// `--shell-type login` runs the probe through the user's default shell
    /// as a login shell, so version-manager PATH entries are present. `-0`
    /// keeps values containing newlines whole, exactly as the local probe
    /// does.
    pub fn probe_env(&self, local: &ShellEnv) -> Result<ShellEnv> {
        match self {
            Host::Local => Ok(local.clone()),
            Host::Wsl { distro } => {
                let out = std::process::Command::new(wsl_exe(local))
                    .args(["-d", distro, "--shell-type", "login", "--", "env", "-0"])
                    .output()
                    .map_err(|e| anyhow!("running wsl.exe for `{distro}`: {e}"))?;
                if !out.status.success() {
                    return Err(anyhow!(
                        "could not read `{distro}`'s environment: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    ));
                }
                let vars = crate::shell_env::parse_env0(&String::from_utf8_lossy(&out.stdout));
                if vars.get("PATH").is_none() {
                    return Err(anyhow!("`{distro}`'s environment came back without a PATH"));
                }
                Ok(ShellEnv {
                    vars,
                    shell: format!("wsl:{distro}"),
                    resolved: true,
                })
            }
            Host::Ssh { host } => {
                // The remote's own login shell answers, whatever it is:
                // sshd runs the command through `$SHELL -c`, and `-l` inside
                // sources the profile that version managers install into.
                // The marker discards whatever the rc files echo on the way,
                // exactly as the local probe does.
                let probe = format!(
                    "$SHELL -lc {}",
                    sh_quote(&format!(
                        "printf {}; env -0",
                        crate::shell_env::MARKER
                    ))
                );
                let mut a = ssh_base_args(false);
                a.push("--".to_string());
                a.push(host.clone());
                a.push(probe);
                let out = std::process::Command::new(ssh_exe(local))
                    .args(a)
                    .output()
                    .map_err(|e| anyhow!("running ssh for `{host}`: {e}"))?;
                if !out.status.success() {
                    return Err(anyhow!(
                        "could not read `{host}`'s environment: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    ));
                }
                let stdout = String::from_utf8_lossy(&out.stdout);
                let dump = stdout
                    .split_once(crate::shell_env::MARKER)
                    .map(|(_, after)| after)
                    .ok_or_else(|| {
                        anyhow!("`{host}`'s shell answered without the environment marker")
                    })?;
                let vars = crate::shell_env::parse_env0(dump);
                if vars.get("PATH").is_none() {
                    return Err(anyhow!("`{host}`'s environment came back without a PATH"));
                }
                Ok(ShellEnv {
                    vars,
                    shell: format!("ssh:{host}"),
                    resolved: true,
                })
            }
        }
    }
}

/// A channel's answer in the shape every caller here already reads.
///
/// `ExitStatus` cannot be built from a number in stable Rust, so the exit
/// code rides through the platform's own encoding — the one place this file
/// has to know that a status is a wait(2) word on unix and a plain code on
/// Windows.
fn as_output(a: crate::channel::Answer) -> std::process::Output {
    #[cfg(unix)]
    let status = {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(a.status << 8)
    };
    #[cfg(windows)]
    let status = {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(a.status as u32)
    };
    std::process::Output {
        status,
        stdout: a.stdout,
        stderr: a.stderr,
    }
}

/// The doorway binary, found on the app machine's own login PATH. On real
/// Windows that is System32's `wsl.exe`; in tests it is whatever stands in
/// for it.
fn wsl_exe(local: &ShellEnv) -> std::path::PathBuf {
    local
        .which("wsl.exe")
        .unwrap_or_else(|| std::path::PathBuf::from("wsl.exe"))
}

/// The user's own ssh — their config, their keys, their agent.
fn ssh_exe(local: &ShellEnv) -> std::path::PathBuf {
    local
        .which("ssh")
        .unwrap_or_else(|| std::path::PathBuf::from("ssh"))
}

/// Quote one word for a POSIX shell. Single quotes swallow everything except
/// a single quote, which is closed around: `it's` → `'it'\''s'`. This is the
/// whole difference between WSL and SSH delivery: `wsl.exe -e` hands argv
/// over intact, while ssh gives the remote *shell* a string — so every word,
/// multi-line prompt included, is armoured here and unwrapped exactly once
/// there.
pub fn sh_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// The one command line an SSH invocation carries: enter the directory, set
/// the environment, become the program. `exec` so the remote shell does not
/// linger between the PTY and the agent.
fn remote_command(
    program: &str,
    args: &[String],
    cwd: Option<&str>,
    envs: &[(String, String)],
) -> String {
    let mut cmd = String::new();
    if let Some(dir) = cwd {
        cmd.push_str(&format!("cd {} && ", sh_quote(dir)));
    }
    cmd.push_str("exec env");
    for (k, v) in envs {
        cmd.push(' ');
        cmd.push_str(&sh_quote(&format!("{k}={v}")));
    }
    cmd.push(' ');
    cmd.push_str(&sh_quote(program));
    for a in args {
        cmd.push(' ');
        cmd.push_str(&sh_quote(a));
    }
    cmd
}

/// Where this machine keeps its SSH control sockets: `~/.marol/ssh/%C`,
/// `%C` being ssh's own short hash of the connection — short, because a unix
/// socket path has ~100 bytes to live in.
pub fn ssh_control_path() -> Option<String> {
    let dir = dirs::home_dir()?.join(".marol").join("ssh");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("%C").to_string_lossy().to_string())
}

/// Open — or confirm — the standing connection to an SSH host, carrying the
/// hook tunnel: remote `127.0.0.1:remote_port` forwards back to the app's
/// listener. Returns whether a master with the tunnel is up.
///
/// Best effort end to end, by the hooks rule: a host whose tunnel could not
/// be raised runs sessions that simply show no status. `-f -N` backgrounds
/// the master after auth; `ControlPersist=yes` keeps it for every later
/// command, and `Core::shutdown` closes it on the way out.
/// Tries each candidate remote port in turn and returns the one that took, or
/// `None` if none did. The first candidate is the one this desk used last
/// time, so an agent the host held through a restart is still reporting to an
/// address that exists.
///
/// `ExitOnForwardFailure` is what makes the answer mean anything. `-f` forks
/// after authentication, so without it ssh exits 0 having printed "remote port
/// forwarding failed" to a stderr nobody reads — the connection is up, the
/// tunnel is not, and every session on that host shows no status for a reason
/// that never surfaces.
pub fn open_ssh_master(
    local: &ShellEnv,
    host: &str,
    candidates: &[u16],
    local_port: u16,
) -> Option<u16> {
    // Already up from an earlier contact this run? Then its tunnel is too,
    // and it is on whichever port that contact settled.
    let check = std::process::Command::new(ssh_exe(local))
        .args(ssh_base_args(false))
        .args(["-O", "check", "--", host])
        .output();
    if matches!(&check, Ok(o) if o.status.success()) {
        return candidates.first().copied();
    }

    let mut last = String::new();
    for port in candidates {
        let out = std::process::Command::new(ssh_exe(local))
            .args(ssh_base_args(false))
            .args([
                "-o",
                "ExitOnForwardFailure=yes",
                "-f",
                "-N",
                "-R",
                &format!("127.0.0.1:{port}:127.0.0.1:{local_port}"),
                "--",
                host,
            ])
            .output();
        match out {
            Ok(o) if o.status.success() => return Some(*port),
            Ok(o) => last = String::from_utf8_lossy(&o.stderr).trim().to_string(),
            Err(e) => last = e.to_string(),
        }
    }
    eprintln!("[host] ssh master for `{host}` failed: {last}");
    None
}

/// Close the standing connection, tunnel and all. Best effort — the host may
/// already be gone, and so may the socket.
pub fn close_ssh_master(local: &ShellEnv, host: &str) {
    let _ = std::process::Command::new(ssh_exe(local))
        .args(ssh_base_args(false))
        .args(["-O", "exit", "--", host])
        .output();
}

/// The standing options every ssh invocation carries. Multiplexing makes the
/// second and every later command ride the first connection instead of
/// re-authenticating — a `git status` should cost a round trip, not a
/// handshake. `BatchMode` on non-interactive calls fails fast instead of
/// hanging on a password prompt nothing can answer; the interactive PTY is
/// exactly where a prompt *can* be answered, so it does not get the flag.
fn ssh_base_args(tty: bool) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(path) = ssh_control_path() {
        args.extend([
            "-o".to_string(),
            "ControlMaster=auto".to_string(),
            "-o".to_string(),
            format!("ControlPath={path}"),
            "-o".to_string(),
            "ControlPersist=yes".to_string(),
        ]);
    }
    if tty {
        // A command is given, so ssh will not allocate a remote tty on its
        // own — and the whole point of the session is the TUI on one.
        args.push("-t".to_string());
    } else {
        args.extend(["-o".to_string(), "BatchMode=yes".to_string()]);
    }
    args
}

/// The exit status these scripts use for "the file is not there".
///
/// A number of our own because the shells' own are taken: `cat` and `tail`
/// both exit 1 for a missing file *and* for one they were not allowed to
/// read, and this app has always insisted those are different answers — one
/// is `Ok(None)`, the other is an error worth showing.
const ABSENT: i32 = 3;

/// `cat` the file, or say plainly that it is not there.
const READ_IF_PRESENT: &str = r#"if [ -e "$1" ]; then cat -- "$1"; else exit 3; fi"#;

/// The same, from a byte offset — the append-only transcript read.
const TAIL_IF_PRESENT: &str = r#"if [ -e "$1" ]; then tail -c "$2" -- "$1"; else exit 3; fi"#;

/// `1` or `0` per path, in the order given.
const EXIST_ALL: &str = r#"for p in "$@"; do
  if [ -e "$p" ]; then echo 1; else echo 0; fi
done"#;

/// The skill names under each root, one U+001E-separated section per root.
///
/// `$root/*/SKILL.md` rather than a listing plus a test apiece: the glob is
/// the filter, and an unmatched glob stays literal in `sh`, which is why the
/// existence of the file is checked before the name is printed.
const SKILLS_IN: &str = r#"first=1
for root in "$@"; do
  [ $first -eq 1 ] || printf '\036'
  first=0
  for d in "$root"/*/; do
    [ -f "$d/SKILL.md" ] || continue
    b=${d%/}
    printf '%s\n' "${b##*/}"
  done
done"#;

/// A host together with both environments commands need: the app machine's
/// (`local`, which finds `wsl.exe`) and the host's own (`env`, whose PATH
/// finds what runs inside). Built by the core from its per-host cache and
/// handed to everything that executes.
#[derive(Clone, Copy)]
pub struct HostRef<'a> {
    pub host: &'a Host,
    pub local: &'a ShellEnv,
    pub env: &'a ShellEnv,
    /// This world's held shells, when it has any. `None` is a world that
    /// keeps none — every test that builds a `HostRef` by hand, and the local
    /// host, which has no doorway to amortise.
    pub channels: Option<&'a crate::channel::Channels>,
}

impl HostRef<'_> {
    pub fn join(&self, base: &str, leaf: &str) -> String {
        self.host.join(base, leaf)
    }

    /// Run a command inside the host to completion.
    pub fn run(&self, program: &str, args: &[&str], cwd: Option<&str>) -> Result<std::process::Output> {
        self.run_with_env(program, args, cwd, &[])
    }

    /// `run`, with extra variables set for this one call. The checkpoint
    /// snapshot is why this exists: `GIT_INDEX_FILE` pointed at a temp index
    /// keeps its `add -A` out of the index the agent sees. The variables ride
    /// the pipelines each doorway already trusts — native envs locally, the
    /// POSIX `env K=V` prefix through wsl.exe and ssh — and land *after* the
    /// host's own, so for one call the extra wins.
    pub fn run_with_env(
        &self,
        program: &str,
        args: &[&str],
        cwd: Option<&str>,
        extra: &[(String, String)],
    ) -> Result<std::process::Output> {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        match self.host {
            Host::Local => {
                let exe = self
                    .env
                    .which(program)
                    .ok_or_else(|| anyhow!("`{program}` not found on the login-shell PATH"))?;
                let mut cmd = std::process::Command::new(exe);
                cmd.args(&owned).envs(&self.env.vars);
                cmd.envs(extra.iter().map(|(k, v)| (k.as_str(), v.as_str())));
                if let Some(dir) = cwd {
                    cmd.current_dir(dir);
                }
                Ok(cmd.output()?)
            }
            // Behind a doorway, a held shell answers without a process if
            // one is free. Declining is normal and costs nothing: the spawn
            // below is what this app did before there were channels at all.
            _ => {
                match self
                    .channels
                    .map(|c| c.run(self.host, self.local, self.env, program, args, cwd, extra))
                {
                    Some(crate::channel::Outcome::Ran(answer)) => return Ok(as_output(answer)),
                    // Sent, and then silence. Whether it ran is not knowable
                    // from here, and the two ways of being wrong are not
                    // equal: waiting again for a read costs a moment, while
                    // repeating a commit or a merge costs the repository. So
                    // this is raised rather than retried, and says which of
                    // the two it is refusing to guess at.
                    Some(crate::channel::Outcome::Lost(why)) => {
                        return Err(anyhow!(
                            "`{program}` was handed to a held shell in this world and no answer \
                             came back ({why}). Whether it ran is unknown, so it has not been \
                             run again — check the world before repeating it."
                        ))
                    }
                    _ => {}
                }
                match self.host {
                    Host::Local => unreachable!("handled above"),
                    Host::Wsl { .. } => {
                        let carried = carried_with(self.env, extra);
                        let (_, wrapped, _) = self.host.wrap(program, &owned, cwd, &carried);
                        Ok(std::process::Command::new(wsl_exe(self.local))
                            .args(wrapped)
                            .output()?)
                    }
                    Host::Ssh { host } => {
                        let carried = carried_with(self.env, extra);
                        let mut a = ssh_base_args(false);
                        a.push("--".to_string());
                        a.push(host.clone());
                        a.push(remote_command(program, &owned, cwd, &carried));
                        Ok(std::process::Command::new(ssh_exe(self.local))
                            .args(a)
                            .output()?)
                    }
                }
            }
        }
    }

    /// `run`, then insist it worked, then hand back trimmed stdout.
    pub fn run_ok(&self, program: &str, args: &[&str], cwd: Option<&str>) -> Result<String> {
        self.run_ok_with_env(program, args, cwd, &[])
    }

    /// `run_with_env` with `run_ok`'s insistence and trimmed stdout.
    pub fn run_ok_with_env(
        &self,
        program: &str,
        args: &[&str],
        cwd: Option<&str>,
        extra: &[(String, String)],
    ) -> Result<String> {
        let out = self.run_with_env(program, args, cwd, extra)?;
        if !out.status.success() {
            return Err(anyhow!(
                "{program} {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Whether `path` is a directory inside the host.
    pub fn is_dir(&self, path: &str) -> bool {
        match self.host {
            Host::Local => std::path::Path::new(path).is_dir(),
            _ => self
                .run("test", &["-d", path], None)
                .map(|o| o.status.success())
                .unwrap_or(false),
        }
    }

    /// Whether `path` exists at all inside the host.
    pub fn exists(&self, path: &str) -> bool {
        match self.host {
            Host::Local => std::path::Path::new(path).exists(),
            _ => self
                .run("test", &["-e", path], None)
                .map(|o| o.status.success())
                .unwrap_or(false),
        }
    }

    /// The names directly inside `path`; empty when it is not a directory or
    /// cannot be read.
    ///
    /// Names, not paths: joining is `join`'s job, because that is the one
    /// place that knows what separator a world speaks. Failure is emptiness
    /// rather than an error — every caller is asking "what is here", and a
    /// directory that does not exist answers that question with "nothing".
    pub fn list_dir(&self, path: &str) -> Vec<String> {
        match self.host {
            Host::Local => std::fs::read_dir(path)
                .map(|it| {
                    it.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default(),
            _ => self
                .run_ok("ls", &["-1", path], None)
                .map(|s| s.lines().map(str::to_string).filter(|l| !l.is_empty()).collect())
                .unwrap_or_default(),
        }
    }

    /// Which of these exist, in the order asked.
    ///
    /// One crossing for the whole list. Locally it stays a syscall apiece,
    /// because locally that is what one crossing costs anyway — and because
    /// `sh` is not on a Windows login-shell PATH, which is the same reason
    /// `Core::list_dir` keeps a native branch.
    ///
    /// A failure answers "none of them", which is what every caller means by
    /// a path it cannot reach: `agent_docs` lists a rules file that is not
    /// there just as deliberately as one that is, and "absent" is the honest
    /// reading of a world that would not answer.
    pub fn exist_all(&self, paths: &[&str]) -> Vec<bool> {
        if paths.is_empty() {
            return Vec::new();
        }
        match self.host {
            Host::Local => paths
                .iter()
                .map(|p| std::path::Path::new(p).exists())
                .collect(),
            _ => {
                let mut argv: Vec<&str> = vec!["-c", EXIST_ALL, "_"];
                argv.extend(paths.iter().copied());
                let answered = self
                    .run_ok("sh", &argv, None)
                    .map(|out| out.lines().map(|l| l == "1").collect::<Vec<bool>>())
                    .unwrap_or_default();
                // A short answer is a broken one; padding with `false` beats
                // zipping a mismatched list onto the paths it describes.
                paths
                    .iter()
                    .enumerate()
                    .map(|(i, _)| answered.get(i).copied().unwrap_or(false))
                    .collect()
            }
        }
    }

    /// The skill directories under each root — the ones that actually hold a
    /// `SKILL.md`, since a directory without one is somebody's notes.
    ///
    /// One crossing for every root together, where this used to be a listing
    /// per root plus a `test -e` per entry.
    pub fn skills_in(&self, roots: &[&str]) -> Vec<Vec<String>> {
        if roots.is_empty() {
            return Vec::new();
        }
        match self.host {
            Host::Local => roots
                .iter()
                .map(|root| {
                    let mut names: Vec<String> = std::fs::read_dir(root)
                        .map(|it| {
                            it.flatten()
                                .filter(|e| e.path().join("SKILL.md").exists())
                                .map(|e| e.file_name().to_string_lossy().into_owned())
                                .collect()
                        })
                        .unwrap_or_default();
                    names.sort();
                    names
                })
                .collect(),
            _ => {
                let mut argv: Vec<&str> = vec!["-c", SKILLS_IN, "_"];
                argv.extend(roots.iter().copied());
                let text = self.run_ok("sh", &argv, None).unwrap_or_default();
                // One section per root, in the order asked, so a root that
                // holds nothing is an empty section rather than a missing one.
                let mut sections = text.split('\u{1e}');
                roots
                    .iter()
                    .map(|_| {
                        sections
                            .next()
                            .map(|s| {
                                s.lines()
                                    .map(str::trim)
                                    .filter(|l| !l.is_empty())
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default()
                    })
                    .collect()
            }
        }
    }

    pub fn mkdir_p(&self, path: &str) -> Result<()> {
        match self.host {
            Host::Local => Ok(std::fs::create_dir_all(path)?),
            _ => {
                self.run_ok("mkdir", &["-p", path], None)?;
                Ok(())
            }
        }
    }

    /// The file's text, `None` when it does not exist. Existence and
    /// readability are separated so a real read failure is an error, not a
    /// silent "no config" — the M6 rule, kept across the boundary.
    pub fn read_to_string(&self, path: &str) -> Result<Option<String>> {
        match self.host {
            Host::Local => match std::fs::read_to_string(path) {
                Ok(t) => Ok(Some(t)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.into()),
            },
            // One crossing, not two. Asking `exists` and then `cat` is two
            // processes through the doorway to answer one question, and
            // through `wsl.exe` a process is the expensive part — locally
            // both are a syscall, which is why the cost never showed here.
            //
            // The M6 distinction survives the fold: `cat` alone cannot tell
            // an absent file from an unreadable one, so absence gets a status
            // of its own rather than being inferred from a failure.
            _ => {
                let out = self.run("sh", &["-c", READ_IF_PRESENT, "_", path], None)?;
                match out.status.code() {
                    Some(ABSENT) => Ok(None),
                    // Raw stdout, not `run_ok`'s trim: file text is content.
                    Some(0) => Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned())),
                    _ => Err(anyhow!(
                        "reading {path}: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    )),
                }
            }
        }
    }

    /// The file's bytes from `offset` to the end, `None` when it does not
    /// exist. For append-only files read repeatedly — each call costs only
    /// what has grown since the last one. Bytes, not text: the caller owns
    /// deciding where a line ends, and a lossy conversion here would break
    /// the byte arithmetic the offset depends on.
    pub fn read_from(&self, path: &str, offset: u64) -> Result<Option<Vec<u8>>> {
        match self.host {
            Host::Local => {
                use std::io::{Read as _, Seek as _};
                let mut f = match std::fs::File::open(path) {
                    Ok(f) => f,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(e) => return Err(e.into()),
                };
                f.seek(std::io::SeekFrom::Start(offset))?;
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)?;
                Ok(Some(buf))
            }
            // Folded the same way `read_to_string` is, and it matters more
            // here: this one runs once per turn, per session, to keep the
            // token account.
            _ => {
                // `tail -c +N` is 1-based: +1 is the whole file.
                let from = format!("+{}", offset.saturating_add(1));
                let out = self.run("sh", &["-c", TAIL_IF_PRESENT, "_", path, &from], None)?;
                match out.status.code() {
                    Some(ABSENT) => Ok(None),
                    Some(0) => Ok(Some(out.stdout)),
                    _ => Err(anyhow!(
                        "reading {path} from byte {offset}: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    )),
                }
            }
        }
    }

    /// Delete one file inside the host. A file already gone is not an
    /// error — the point is the absence, not the act.
    pub fn remove_file(&self, path: &str) -> Result<()> {
        match self.host {
            Host::Local => match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            },
            _ => {
                self.run_ok("rm", &["-f", "--", path], None)?;
                Ok(())
            }
        }
    }

    /// Remove one *empty* directory inside the host. A directory already gone
    /// is not an error — the point is the absence, not the act.
    ///
    /// Empty is the whole safety of it: `rmdir` and `remove_dir` both refuse
    /// a directory still holding something, and the callers here are tidying
    /// up after their own scaffolding. A tidy-up that could take somebody's
    /// files with it would not be one.
    pub fn remove_dir(&self, path: &str) -> Result<()> {
        match self.host {
            Host::Local => match std::fs::remove_dir(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            },
            _ => {
                if !self.exists(path) {
                    return Ok(());
                }
                self.run_ok("rmdir", &["--", path], None)?;
                Ok(())
            }
        }
    }

    /// Write a file inside the host, creating its directory. The content
    /// travels as an *argument*, not stdin — one path through every doorway,
    /// and the quoting layer already knows how to armour it.
    pub fn write_file(&self, path: &str, contents: &str) -> Result<()> {
        match self.host {
            Host::Local => {
                if let Some(parent) = std::path::Path::new(path).parent() {
                    std::fs::create_dir_all(parent)?;
                }
                Ok(std::fs::write(path, contents)?)
            }
            _ => {
                let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or(".");
                let script = format!(
                    "mkdir -p {} && printf '%s' \"$1\" > {}",
                    sh_quote(dir),
                    sh_quote(path)
                );
                self.run_ok("sh", &["-c", &script, "_", contents], None)?;
                Ok(())
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/* World discovery                                                     */
/* ------------------------------------------------------------------ */

/// Distro names out of `wsl.exe -l -q`.
///
/// wsl.exe writes **UTF-16LE** — the classic landmine of every wrapper
/// that ever shelled out to it; a UTF-8 read shows one letter per two
/// bytes with NULs between. A BOM may lead. Docker Desktop's plumbing
/// distros are filtered: they are machinery, not worlds anyone opens a
/// repository in.
pub fn parse_wsl_list(bytes: &[u8]) -> Vec<String> {
    let text = if bytes.iter().take(64).any(|b| *b == 0) {
        let body = if bytes.starts_with(&[0xFF, 0xFE]) { &bytes[2..] } else { bytes };
        let units: Vec<u16> = body
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        // Honesty valve: should wsl ever grow a UTF-8 mode, or a test
        // feed plain text, ASCII-looking input decodes as itself.
        String::from_utf8_lossy(bytes).into_owned()
    };
    text.lines()
        .map(|l| l.trim_matches(['\r', ' ', '\u{0}']).to_string())
        .filter(|l| !l.is_empty())
        .filter(|l| l != "docker-desktop" && l != "docker-desktop-data")
        .collect()
}

/// `Host` aliases out of an `~/.ssh/config`, in file order.
///
/// Only names a person deliberately wrote: wildcard patterns (`*`, `?`)
/// are matching rules, not destinations, and negations (`!`) are
/// exclusions. `Include` files are not followed in v1 — the main config
/// is where aliases people reach for by hand live.
pub fn parse_ssh_config(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let Some(keyword) = tokens.next() else { continue };
        if !keyword.eq_ignore_ascii_case("host") {
            continue;
        }
        for name in tokens {
            if name.contains('*') || name.contains('?') || name.starts_with('!') {
                continue;
            }
            if !out.iter().any(|n| n == name) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// The variables worth carrying into a WSL command. The whole login
/// environment would be the ideal, but it rides on `wsl.exe`'s command line,
/// which is a Windows command line with a Windows-sized limit — so what
/// crosses is what commands actually resolve and run by: the host's own PATH
/// (probed, so version-manager shims are on it) and HOME.
fn carry_env(env: &ShellEnv) -> Vec<(String, String)> {
    ["PATH", "HOME"]
        .iter()
        .filter_map(|k| env.vars.get(*k).map(|v| (k.to_string(), v.clone())))
        .collect()
}

/// What crosses into a held shell: the same pair every wrapped command has
/// always carried, set once at the far end instead of on every command line.
pub fn carry_env_pub(env: &ShellEnv) -> Vec<(String, String)> {
    carry_env(env)
}

/// `carry_env` plus one call's extras, extras last: POSIX `env` applies
/// assignments left to right, so on a collision the extra wins — the same
/// precedence the local path gets from calling `.envs` twice.
fn carried_with(env: &ShellEnv, extra: &[(String, String)]) -> Vec<(String, String)> {
    let mut vars = carry_env(env);
    vars.extend(extra.iter().cloned());
    vars
}

/// The per-session extras a PTY launch carries across, on top of `carry_env`.
pub fn pty_env(env: &ShellEnv, extra: &[(String, String)]) -> Vec<(String, String)> {
    let mut vars = carry_env(env);
    vars.push(("TERM".to_string(), "xterm-256color".to_string()));
    vars.push(("COLORTERM".to_string(), "truecolor".to_string()));
    vars.extend(extra.iter().cloned());
    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CI 守門的 WSL 半場:MAROL_EXPECT_WSL_CLAUDE / MAROL_EXPECT_WSL_CODEX
    /// =1 表示某個 distro 裡真的裝了那支 CLI —— 那 app 的整條真實路徑
    /// (wsl.exe -l -q 的 UTF-16LE 列舉 → --shell-type login 的環境探測 →
    /// distro 內的 PATH 行走)就必須走得通。這正是使用者機器上 wsl://
    /// 世界的每一步。
    ///
    /// 一支一個開關:兩支 CLI 在 distro 裡是各自安裝的,一支缺席該報那
    /// 一支的實話,不該連帶把另一支的守門也關掉。
    #[test]
    fn a_promised_wsl_agent_is_reached_through_the_real_doorway() {
        let wanted: Vec<&str> = [("claude", "MAROL_EXPECT_WSL_CLAUDE"), ("codex", "MAROL_EXPECT_WSL_CODEX")]
            .into_iter()
            .filter(|(_, gate)| std::env::var(gate).as_deref() == Ok("1"))
            .map(|(agent, _)| agent)
            .collect();
        if wanted.is_empty() {
            eprintln!("skip: no MAROL_EXPECT_WSL_* gate is set");
            return;
        }
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let local = rt.block_on(crate::shell_env::resolve());
        let out = std::process::Command::new(wsl_exe(&local))
            .args(["-l", "-q"])
            .output()
            .expect("running wsl.exe -l -q");
        let distros = parse_wsl_list(&out.stdout);
        assert!(!distros.is_empty(), "wsl.exe enumerated no distro at all");
        let host = Host::Wsl { distro: distros[0].clone() };
        let env = host
            .probe_env(&local)
            .expect("probing the distro's login environment over wsl.exe");
        assert!(env.path().is_some(), "the distro's login env came back without a PATH");
        // 問版本要穿過門去問 —— distro 裡的檔案在門的另一邊,本機的
        // which() 摸不到(第一版測試的錯);core 的世界探測走的就是這條:
        // HostRef::run_ok 把指令包成 wsl.exe -d <distro> -e env … claude。
        let hr = HostRef { host: &host, local: &local, env: &env, channels: None };
        for agent in wanted {
            let out = hr
                .run_ok(agent, &["--version"], None)
                .unwrap_or_else(|e| panic!("{agent} --version through the wsl doorway: {e:#}"));
            assert!(
                out.chars().any(|c| c.is_ascii_digit()),
                "{agent} answered strangely: {out:?}"
            );
            eprintln!("{agent} in {} says {}", distros[0], out.trim());
        }
    }

    /// The classic landmine, reproduced: wsl.exe speaks UTF-16LE with a
    /// BOM and CRLF line ends — and Docker's plumbing distros are noise.
    #[test]
    fn wsl_list_decodes_utf16_and_drops_the_plumbing() {
        let text = "Ubuntu\r\nDebian\r\ndocker-desktop\r\ndocker-desktop-data\r\n";
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(parse_wsl_list(&bytes), vec!["Ubuntu", "Debian"]);
        // The honesty valve: plain text decodes as itself.
        assert_eq!(parse_wsl_list(b"Ubuntu\nAlpine\n"), vec!["Ubuntu", "Alpine"]);
        assert!(parse_wsl_list(b"").is_empty());
    }

    /// Aliases a person wrote, and nothing a matcher wrote: wildcards,
    /// negations, comments and Match blocks all stay out of the menu.
    #[test]
    fn ssh_config_yields_only_deliberate_aliases() {
        let config = "\
# personal hosts\n\
Host devbox\n\
  HostName dev.example.com\n\
  User me\n\
\n\
host build farm-a farm-b\n\
Host *.internal !prod *\n\
Match user root\n\
  Compression yes\n\
Host devbox\n";
        assert_eq!(
            parse_ssh_config(config),
            vec!["devbox", "build", "farm-a", "farm-b"]
        );
        assert!(parse_ssh_config("").is_empty());
    }

    #[test]
    fn plain_paths_stay_local_and_round_trip() {
        let l = locate("/Users/me/code/app").unwrap();
        assert_eq!(l.host, Host::Local);
        assert_eq!(l.path, "/Users/me/code/app");
        assert_eq!(stored(&l.host, &l.path), "/Users/me/code/app");
        assert_eq!(label(&l.host), None);
    }

    #[test]
    fn a_wsl_url_names_the_distro_and_keeps_the_posix_path() {
        let l = locate("wsl://Ubuntu/home/me/code/app").unwrap();
        assert_eq!(
            l.host,
            Host::Wsl {
                distro: "Ubuntu".into()
            }
        );
        assert_eq!(l.path, "/home/me/code/app");
        assert_eq!(stored(&l.host, &l.path), "wsl://Ubuntu/home/me/code/app");
        assert_eq!(label(&l.host).as_deref(), Some("wsl:Ubuntu"));
    }

    #[test]
    fn a_wsl_url_without_a_distro_or_path_is_refused_plainly() {
        assert!(locate("wsl://").is_err());
        assert!(locate("wsl://Ubuntu").is_err());
        assert!(locate("wsl:///home/me").is_err());
    }

    #[test]
    fn an_ssh_url_names_the_alias_and_keeps_the_posix_path() {
        let l = locate("ssh://devbox/home/me/app").unwrap();
        assert_eq!(
            l.host,
            Host::Ssh {
                host: "devbox".into()
            }
        );
        assert_eq!(l.path, "/home/me/app");
        assert_eq!(stored(&l.host, &l.path), "ssh://devbox/home/me/app");
        assert_eq!(label(&l.host).as_deref(), Some("ssh:devbox"));
        assert!(locate("ssh://").is_err());
        assert!(locate("ssh://devbox").is_err());
    }

    /// The quoting layer is the whole difference between the two doorways:
    /// wsl.exe hands argv over intact, ssh hands the remote shell a string.
    /// One word must survive anything a prompt can contain.
    #[test]
    fn shell_quoting_armours_what_prompts_actually_contain() {
        assert_eq!(sh_quote("plain"), "'plain'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_quote("多行
prompt"), "'多行
prompt'");
        assert_eq!(sh_quote("$HOME `id` \"x\""), "'$HOME `id` \"x\"'");
    }

    /// The one command line an SSH invocation carries: cd, env, exec — every
    /// word armoured exactly once.
    #[test]
    fn the_remote_command_enters_sets_and_becomes() {
        let cmd = remote_command(
            "claude",
            &["--continue".into(), "it's
done".into()],
            Some("/home/me/wt"),
            &[("MAROL_SESSION_ID".into(), "s1".into())],
        );
        assert_eq!(
            cmd,
            r"cd '/home/me/wt' && exec env 'MAROL_SESSION_ID=s1' 'claude' '--continue' 'it'\''s
done'"
        );
    }

    /// The PTY path forces a remote tty (`-t`) — a command being given stops
    /// ssh allocating one on its own, and the TUI is the whole point — and
    /// the command travels as ONE trailing argument.
    #[test]
    fn wrapping_for_ssh_forces_a_tty_and_carries_one_command_string() {
        let host = Host::Ssh {
            host: "devbox".into(),
        };
        let (prog, args, outer_cwd) = host.wrap(
            "claude",
            &["多行
prompt".into()],
            Some("/home/me/wt"),
            &[],
        );
        assert_eq!(prog, "ssh");
        assert!(args.contains(&"-t".to_string()));
        assert!(!args.contains(&"BatchMode=yes".to_string()), "the PTY is where a prompt CAN be answered");
        let host_pos = args.iter().position(|a| a == "devbox").unwrap();
        assert_eq!(args.len(), host_pos + 2, "everything after the host is one command string");
        assert!(args[host_pos + 1].starts_with("cd '/home/me/wt' && exec env"));
        assert_eq!(outer_cwd, None);
    }

    /// The wrapped command line is the contract with wsl.exe: no shell in the
    /// middle, so a multi-line prompt is one argv entry the whole way.
    #[test]
    fn wrapping_for_wsl_carries_cwd_env_and_argv_without_a_shell() {
        let host = Host::Wsl {
            distro: "Ubuntu".into(),
        };
        let (prog, args, outer_cwd) = host.wrap(
            "claude",
            &["--plugin-dir".into(), "/mnt/c/p".into(), "多行\nprompt".into()],
            Some("/home/me/wt"),
            &[("MAROL_SESSION_ID".into(), "s1".into())],
        );
        assert_eq!(prog, "wsl.exe");
        assert_eq!(
            args,
            vec![
                "-d",
                "Ubuntu",
                "--cd",
                "/home/me/wt",
                "-e",
                "env",
                "MAROL_SESSION_ID=s1",
                "claude",
                "--plugin-dir",
                "/mnt/c/p",
                "多行\nprompt"
            ]
        );
        // The real cwd only exists inside the distro.
        assert_eq!(outer_cwd, None);
    }

    #[test]
    fn wrapping_locally_is_the_identity() {
        let (prog, args, cwd) = Host::Local.wrap(
            "claude",
            &["--continue".into()],
            Some("/tmp/x"),
            &[("K".into(), "V".into())],
        );
        assert_eq!(prog, "claude");
        assert_eq!(args, vec!["--continue"]);
        assert_eq!(cwd, Some("/tmp/x"));
    }

    /// The hooks plugin lives on the app's disk; a claude inside WSL reads it
    /// through the drive mounts.
    #[test]
    fn a_windows_path_translates_to_its_mnt_mount() {
        assert_eq!(
            win_path_for_wsl(r"C:\Users\me\AppData\Marol\plugin"),
            "/mnt/c/Users/me/AppData/Marol/plugin"
        );
        assert_eq!(win_path_for_wsl("D:/code/x"), "/mnt/d/code/x");
        // Already-POSIX paths pass through — tests and shared filesystems.
        assert_eq!(win_path_for_wsl("/data/plugin"), "/data/plugin");
    }

    /// Non-local paths are joined as strings: `PathBuf` on Windows would
    /// insert backslashes into a POSIX path.
    #[test]
    fn host_side_paths_join_with_forward_slashes() {
        let wsl = Host::Wsl {
            distro: "Ubuntu".into(),
        };
        assert_eq!(wsl.join("/home/me/", "x"), "/home/me/x");
        assert_eq!(wsl.join("/home/me", "x"), "/home/me/x");
    }

    /// The env pipeline checkpoints ride on: a variable set for one call is
    /// there for that call and gone for the next — `GIT_INDEX_FILE` must
    /// never bleed into a later command that touches the agent's real index.
    ///
    /// Unix-gated because the *test* needs a POSIX shell to ask its question,
    /// not because the property is: it pins `PATH` to `/usr/bin:/bin` and
    /// reads the variable back through `sh -c`. On Windows there is no `sh`
    /// on a login-shell PATH, so this failed there for a reason that has
    /// nothing to do with `run_ok_with_env`. Found the first time the unit
    /// tests were ever run on Windows.
    #[cfg(unix)]
    #[test]
    fn run_with_env_sets_the_variable_for_that_call_only() {
        let mut env = ShellEnv {
            vars: Default::default(),
            shell: "sh".into(),
            resolved: true,
        };
        env.vars
            .insert("PATH".into(), "/usr/bin:/bin".into());
        let local = Host::Local;
        let hr = HostRef {
            host: &local,
            local: &env,
            env: &env,
            channels: None,
        };
        let extra = [("MAROL_PROBE".to_string(), "set".to_string())];
        let with = hr
            .run_ok_with_env("sh", &["-c", "printf %s \"${MAROL_PROBE:-unset}\""], None, &extra)
            .unwrap();
        assert_eq!(with, "set");
        let without = hr
            .run_ok("sh", &["-c", "printf %s \"${MAROL_PROBE:-unset}\""], None)
            .unwrap();
        assert_eq!(without, "unset");
    }

    /// Through the wrapped doorways the extras join the `env K=V` prefix
    /// after the carried pair, so on a collision the per-call value wins —
    /// the same precedence the local path gets from applying envs twice.
    #[test]
    fn per_call_extras_ride_after_the_carried_pair_and_win_collisions() {
        let mut env = ShellEnv {
            vars: Default::default(),
            shell: "sh".into(),
            resolved: true,
        };
        env.vars.insert("PATH".into(), "/usr/bin".into());
        env.vars.insert("HOME".into(), "/home/me".into());
        let carried = carried_with(
            &env,
            &[
                ("GIT_INDEX_FILE".to_string(), "/tmp/idx".to_string()),
                ("HOME".to_string(), "/elsewhere".to_string()),
            ],
        );
        let home_positions: Vec<usize> = carried
            .iter()
            .enumerate()
            .filter(|(_, (k, _))| k == "HOME")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(home_positions.len(), 2);
        assert_eq!(carried[home_positions[1]].1, "/elsewhere");
        assert!(carried.iter().any(|(k, v)| k == "GIT_INDEX_FILE" && v == "/tmp/idx"));
    }

    /// What crosses the boundary is bounded on purpose: the host's own PATH
    /// so programs resolve, HOME so config is found — never the whole dump,
    /// which would ride a Windows-sized command line.
    #[test]
    fn only_path_and_home_are_carried_across() {
        let mut env = ShellEnv {
            vars: Default::default(),
            shell: "sh".into(),
            resolved: true,
        };
        env.vars.insert("PATH".into(), "/nvm/bin:/usr/bin".into());
        env.vars.insert("HOME".into(), "/home/me".into());
        env.vars.insert("SECRET".into(), "x".into());
        let carried = carry_env(&env);
        assert!(carried.contains(&("PATH".into(), "/nvm/bin:/usr/bin".into())));
        assert!(carried.contains(&("HOME".into(), "/home/me".into())));
        assert!(!carried.iter().any(|(k, _)| k == "SECRET"));
    }
}
