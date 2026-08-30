//! The attempt flow, end to end through the core.
//!
//! Driven by a stub agent rather than a real one: what is being checked here
//! is what Marol does — which worktree it opens, what it puts on the
//! command line, what it records, and what it gives back afterwards — none of
//! which needs a model to answer. `tests/prompt_injection.rs` covers the part
//! that genuinely needed measuring against the real `claude`.
//!
//!     cargo test --test attempts -- --nocapture

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[path = "../src/agent.rs"]
mod agent;
#[path = "../src/channel.rs"]
mod channel;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/core.rs"]
mod core;
#[path = "../src/host.rs"]
mod host;
#[path = "../src/hooks.rs"]
mod hooks;
#[path = "../src/i18n.rs"]
mod i18n;
#[path = "../src/prompt.rs"]
mod prompt;
#[path = "../src/pty.rs"]
mod pty;
#[path = "../src/shell_env.rs"]
mod shell_env;
#[path = "../src/store.rs"]
mod store;
#[path = "../src/update.rs"]
mod update;
#[path = "../src/worktree.rs"]
mod worktree;

use crate::core::{Core, Status, UiSink};
use crate::shell_env::ShellEnv;
use crate::store::{Lifecycle, Outcome, PermissionMode};

#[derive(Default)]
struct Events(Mutex<Vec<(String, serde_json::Value)>>);

impl UiSink for Events {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        self.0.lock().unwrap().push((event.to_string(), payload));
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git must be installed");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Everything one test needs: a repository, a stub agent on the PATH, and a
/// core wired to both.
struct Harness {
    root: PathBuf,
    repo: PathBuf,
    core: Arc<Core>,
    rt: tokio::runtime::Runtime,
    /// Kept so a test can stand a second core over the same directories —
    /// which is the only way to exercise what a restart sees.
    env: ShellEnv,
}

/// A stand-in for an agent CLI. Records the working directory and every
/// argument it was given, then stays alive reading its terminal — so the
/// session looks the way a real one does, and what a follow-up fed into the
/// PTY can be read back out.
///
/// NUL-separated, because the argument under test is a multi-line prompt and
/// a line-per-argument log cannot tell one argument containing newlines from
/// several arguments — which is exactly the distinction these tests exist to
/// make. One file per launch, named by pid, so reopening a session leaves the
/// first launch's record intact beside the second's. The stdin capture is
/// named `stdin.<session>.<pid>` so `launches` never mistakes it for a launch
/// record.
///
/// `--version` answers immediately, the way the real CLI does, because the
/// core probes it once at startup — a stub that hung there would slow every
/// test's boot by the probe's timeout.
const STUB: &str = r#"#!/bin/bash
if [ "$1" = "--version" ]; then echo "2.1.226 (Claude Code)"; exit 0; fi
printf '%s\0' "$PWD" "${MAROL_NAME_URL:-}" "${MAROL_PEERS_URL:-}" "${MAROL_SEND_URL:-}" "$@" > "$MAROL_STUB_LOG/${MAROL_SESSION_ID:-unknown}.$$"
# 宣告它所替身的那個 CLI 真的會宣告的模式:Claude Code 開啟 bracketed
# paste(DECSET 2004),而 `bracketed_followup` 只送給量測過會開它的 CLI。
# 這一行之前 stub 是個沉默的位元組水槽,而任何會照 2004 決定要不要轉發
# 標記的傳輸層(例如 tmux)看到的就是「這支程式沒要」。
printf '\033[?2004h'
exec cat > "$MAROL_STUB_LOG/stdin.${MAROL_SESSION_ID:-unknown}.$$"
"#;

/// The same stand-in wearing Codex's name and Codex's `--version` shape.
///
/// The string matters: Claude Code leads with the number and Codex leads
/// with its own name, and the version gate that decides whether the hook
/// config goes on the command line reads both through one parser. A stub
/// that answered in Claude Code's shape would let a parser that only knew
/// that shape pass.
const CODEX_STUB: &str = r#"#!/bin/bash
if [ "$1" = "--version" ]; then echo "codex-cli 0.147.0"; exit 0; fi
printf '%s\0' "$PWD" "${MAROL_NAME_URL:-}" "${MAROL_PEERS_URL:-}" "${MAROL_SEND_URL:-}" "$@" > "$MAROL_STUB_LOG/${MAROL_SESSION_ID:-unknown}.$$"
printf '\033[?2004h'
exec cat > "$MAROL_STUB_LOG/stdin.${MAROL_SESSION_ID:-unknown}.$$"
"#;

/// A CLI whose conventions nobody has measured. It records what it was
/// given for the tests that assert it was given nothing of ours.
const UNMEASURED_STUB: &str = r#"#!/bin/bash
if [ "$1" = "--version" ]; then echo "0.9.0"; exit 0; fi
printf '%s\0' "$PWD" "${MAROL_NAME_URL:-}" "${MAROL_PEERS_URL:-}" "${MAROL_SEND_URL:-}" "$@" > "$MAROL_STUB_LOG/${MAROL_SESSION_ID:-unknown}.$$"
exec cat > "$MAROL_STUB_LOG/stdin.${MAROL_SESSION_ID:-unknown}.$$"
"#;

/// A stand-in for a Claude Code release from before session names existed.
/// What matters is what it is NOT handed: `--name` would stop it starting.
const OLD_STUB: &str = r#"#!/bin/bash
if [ "$1" = "--version" ]; then echo "2.0.14 (Claude Code)"; exit 0; fi
printf '%s\0' "$PWD" "${MAROL_NAME_URL:-}" "${MAROL_PEERS_URL:-}" "${MAROL_SEND_URL:-}" "$@" > "$MAROL_STUB_LOG/${MAROL_SESSION_ID:-unknown}.$$"
exec cat > "$MAROL_STUB_LOG/stdin.${MAROL_SESSION_ID:-unknown}.$$"
"#;

impl Harness {
    fn new(name: &str) -> Self {
        Self::with_claude_stub(name, STUB)
    }

    /// The same harness with a different `claude` on the PATH — how the
    /// version gate is exercised against a CLI from another era.
    fn with_claude_stub(name: &str, claude_stub: &str) -> Self {
        let root = std::env::temp_dir().join(format!("marol-att-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main", "-q"]);
        git(&repo, &["config", "user.email", "t@marol.test"]);
        git(&repo, &["config", "user.name", "Marol Test"]);
        std::fs::write(repo.join("app.txt"), "one\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "first"]);

        // A PATH with our stubs first and the real tools behind them, so git
        // still resolves while `claude` and `codex` are ours.
        let bin = root.join("bin");
        let logs = root.join("logs");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&logs).unwrap();
        for (agent, stub) in [
            ("claude", claude_stub),
            ("codex", CODEX_STUB),
            ("gemini", UNMEASURED_STUB),
        ] {
            let p = bin.join(agent);
            std::fs::write(&p, stub).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }

        // One PATH for the harness and for the world behind the stand-in
        // wsl.exe, so the two cannot disagree about what exists. They did:
        // the world's list was written out by hand and left out
        // `/usr/local/bin`, so on macOS the distro had no tmux while the
        // machine did, and the tests that check a session is held in there
        // failed for a reason that had nothing to do with the app.
        //
        // The tail ends with wherever this machine's tmux actually is, found
        // rather than guessed. Every hold test gates on "does this machine
        // have tmux"; without this the answer to that and the answer the app
        // gets can differ, which is the exact class of bug `shell_env` exists
        // for — a Homebrew tmux is on one PATH and not the other.
        let mut path = format!("{}:/usr/bin:/bin:/usr/local/bin", bin.display());
        if let Some(dir) = std::env::var("PATH")
            .unwrap_or_default()
            .split(':')
            .find(|d| !d.is_empty() && Path::new(d).join("tmux").is_file())
        {
            path.push(':');
            path.push_str(dir);
        }

        // A stand-in for wsl.exe: the "distro" shares this machine's
        // filesystem, and its login environment is pinned to the harness's
        // own — stubs first on PATH, HOME at the harness root — so the whole
        // WSL route (probe, git, spawn, env crossing) runs for real against
        // local processes. What it cannot vouch for is the real wsl.exe's
        // quirks; that is what a Windows machine validates.
        let fake_wsl = format!(
            r#"#!/bin/bash
export PATH="{path}"
export HOME="{home}"
export MAROL_STUB_LOG="{logs}"
# One line per crossing, whatever the argv contains. Every process through
# this door is the thing the batched reads exist to avoid, so the tests can
# count them — and a script argument with newlines in it must not read as
# several crossings, which is exactly what a batched read passes.
printf '%s\n' "$(printf '%s ' "$@" | tr '\n' ' ')" >> "{logs}/wsl-calls"
while [ $# -gt 0 ]; do
  case "$1" in
    -d|--shell-type) shift 2 ;;
    --cd) cd "$2" || exit 1; shift 2 ;;
    -e|--) shift; break ;;
    *) shift ;;
  esac
done
exec "$@"
"#,
            path = path,
            home = root.display(),
            logs = logs.display(),
        );
        let p = bin.join("wsl.exe");
        std::fs::write(&p, fake_wsl).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let mut vars: HashMap<String, String> = HashMap::new();
        vars.insert("PATH".into(), path.clone());
        vars.insert("MAROL_STUB_LOG".into(), logs.to_string_lossy().into());
        vars.insert("HOME".into(), root.to_string_lossy().into());
        let env = ShellEnv {
            vars,
            shell: "/bin/bash".into(),
            resolved: true,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let core = rt
            .block_on(Core::start_with(
                env.clone(),
                Arc::new(Events::default()) as Arc<dyn UiSink>,
                root.join("marol.db"),
                root.join("data"),
                root.join("worktrees"),
            ))
            .expect("core");

        Self {
            root,
            repo,
            core,
            rt,
            env,
        }
    }

    fn card(&self, title: &str, prompt: &str) -> String {
        self.core
            .create_task(
                title.into(),
                prompt.into(),
                self.repo.to_string_lossy().into(),
                "main".into(),
                Vec::new(),
            )
            .expect("create task")
    }

    /// A second repository beside the harness's own, for the cards that span
    /// two. Same one-commit shape, so the two are told apart only by name.
    fn second_repo(&self, name: &str) -> PathBuf {
        let repo = self.root.join("repos").join(name);
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main", "-q"]);
        git(&repo, &["config", "user.email", "t@marol.test"]);
        git(&repo, &["config", "user.name", "Marol Test"]);
        std::fs::write(repo.join("service.txt"), "one\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "first"]);
        repo
    }

    /// A card spanning this fixture's repository and one more.
    fn card_spanning(&self, title: &str, prompt: &str, extra: &[(&str, &str)]) -> String {
        self.core
            .create_task(
                title.into(),
                prompt.into(),
                self.repo.to_string_lossy().into(),
                "main".into(),
                extra
                    .iter()
                    .map(|(repo, base)| crate::store::TaskRepo {
                        repo_path: (*repo).into(),
                        base_branch: (*base).into(),
                    })
                    .collect(),
            )
            .expect("create task")
    }

    /// Start an attempt now, through the same call the button makes. The
    /// default limit leaves room, so this never lands in the queue.
    fn start(&self, task_id: &str, agent: &str) -> crate::core::OpenedAttempt {
        self.core
            .start_attempt(task_id, agent.into(), None, PermissionMode::Normal, 100, 30)
            .expect("start attempt")
            .attempt
            .expect("there was a free slot")
    }

    /// Every time the stub agent was started for this session, oldest first.
    fn launches(&self, session_id: &str, at_least: usize) -> Vec<Launch> {
        let dir = self.root.join("logs");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut found: Vec<(std::time::SystemTime, Launch)> = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if !name.starts_with(&format!("{session_id}.")) {
                        continue;
                    }
                    let Ok(bytes) = std::fs::read(e.path()) else { continue };
                    let mut parts: Vec<String> = bytes
                        .split(|b| *b == 0)
                        .map(|s| String::from_utf8_lossy(s).into_owned())
                        .collect();
                    // A trailing separator leaves an empty final field.
                    if parts.last().is_some_and(|s| s.is_empty()) {
                        parts.pop();
                    }
                    // The working directory, the naming endpoint and the two
                    // messaging endpoints, then argv. A record missing any of
                    // those headers is a half-written file caught
                    // mid-`printf`, not a launch.
                    if parts.len() < 4 {
                        continue;
                    }
                    let when = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                    found.push((
                        when,
                        Launch {
                            cwd: parts.remove(0),
                            name_url: parts.remove(0),
                            peers_url: parts.remove(0),
                            send_url: parts.remove(0),
                            args: parts,
                        },
                    ));
                }
            }
            if found.len() >= at_least {
                found.sort_by_key(|(t, _)| *t);
                return found.into_iter().map(|(_, l)| l).collect();
            }
            if Instant::now() > deadline {
                panic!(
                    "expected {at_least} launch(es) of the stub agent for session {session_id}, saw {}",
                    found.len()
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn args_of(&self, session_id: &str) -> Vec<String> {
        self.launches(session_id, 1).pop().unwrap().args
    }

    /// Wait until nothing is holding this session's terminal any more.
    ///
    /// `close_session` asks tmux to end the server and returns; the server
    /// exits on its own time. Opening the session again inside that window
    /// finds the old one still answering, and `new-session -A -D` attaches to
    /// it — which drops the argv and is exactly not a start. A person takes
    /// long enough over the two clicks that this never bites them; a test
    /// doing both in the same microsecond has to say what it is waiting for.
    ///
    /// A world with no tmux holds nothing, so there is nothing to wait for
    /// and the first look already says so.
    fn wait_unheld(&self, id: &str) {
        let tag = pty::desk_tag(&self.root.join("data").to_string_lossy());
        let sock = pty::hold_socket(&tag, id);
        let alive = || {
            std::process::Command::new("tmux")
                .args(["-L", &sock, "has-session", "-t", "agent"])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        assert!(
            wait_for(Duration::from_secs(10), || !alive()),
            "the holder for {id} never let go",
        );
    }

    /// One session's row as the list has it right now.
    fn session(&self, id: &str) -> crate::core::SessionMeta {
        self.core
            .sessions()
            .into_iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("no session {id} on the list"))
    }

    /// The title, once it is the one expected. A name arriving from the
    /// listener leaves the hook path for a thread before it touches the
    /// store, so reading straight after the POST is a race.
    fn wait_for_title(&self, id: &str, want: &str) -> String {
        wait_for(Duration::from_secs(5), || self.session(id).title == want);
        self.session(id).title
    }

    /// Forget every crossing so far, so the next count measures one act.
    fn reset_crossings(&self) {
        let _ = std::fs::remove_file(self.root.join("logs").join("wsl-calls"));
    }

    /// How many times the stand-in `wsl.exe` has been run since the reset.
    ///
    /// The number Phase 1 is about: locally each of these is a fork nobody
    /// notices, and through a real `wsl.exe` it is a Windows process.
    fn crossings(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("logs").join("wsl-calls"))
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// Post a hook report the way Claude Code's own hook runner would.
    fn hook(&self, session_id: &str, state: &str, body: serde_json::Value) {
        use std::io::{Read as _, Write as _};
        let url = self.core.hook_url().expect("hook listener");
        // http://127.0.0.1:PORT/h/TOKEN
        let rest = url.trim_start_matches("http://");
        let (addr, path) = rest.split_once('/').expect("url has a path");
        let body = if body.is_null() { String::new() } else { body.to_string() };
        let mut sock = std::net::TcpStream::connect(addr).expect("connect to the hook listener");
        let req = format!(
            "POST /{path}?state={state} HTTP/1.1\r\nHost: localhost\r\n\
             X-Marol-Session: {session_id}\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        sock.write_all(req.as_bytes()).unwrap();
        let mut resp = String::new();
        let _ = sock.read_to_string(&mut resp);
        assert!(resp.starts_with("HTTP/1.1 200"), "hook was not answered: {resp}");
    }

    /// The timeline, once it has at least `at_least` rows. The writer runs on
    /// its own thread, so this is the honest way to read it.
    fn timeline(&self, attempt_id: &str, at_least: usize) -> Vec<crate::store::AttemptEvent> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let rows = self.core.attempt_events(attempt_id).unwrap_or_default();
            if rows.len() >= at_least {
                return rows;
            }
            if Instant::now() > deadline {
                panic!("expected {at_least} timeline rows, saw {}: {rows:?}", rows.len());
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn cwd_of(&self, session_id: &str) -> String {
        self.launches(session_id, 1).pop().unwrap().cwd
    }

    /// Give the harness repository an `.marol/config.json`.
    fn config(&self, json: &str) {
        let dir = self.repo.join(".marol");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), json).unwrap();
    }

    /// Whether any launch record exists for this session, without waiting.
    fn launched(&self, session_id: &str) -> bool {
        std::fs::read_dir(self.root.join("logs"))
            .map(|entries| {
                entries.flatten().any(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(&format!("{session_id}."))
                })
            })
            .unwrap_or(false)
    }

    /// Everything the session's terminal has been fed, once anything has.
    /// The stub's `cat` writes what it reads, so this is the input as the
    /// agent would have received it.
    /// Everything written to this session's stdin, once `settled` agrees it
    /// has all arrived.
    ///
    /// Waiting on the *content* rather than on "anything at all". A write to
    /// a pty arrives in as many pieces as the kernel feels like splitting it
    /// into, and under tmux there is a further hop, so a read that returns
    /// the moment the file is non-empty can catch a bracketed paste holding
    /// its opening marker and nothing else. That is precisely what it did on
    /// macOS, intermittently, and never once on Linux — which is why it
    /// survived until the suite was run somewhere other than Linux.
    fn stdin_when(&self, session_id: &str, settled: impl Fn(&str) -> bool) -> String {
        let dir = self.root.join("logs");
        let prefix = format!("stdin.{session_id}.");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut all = String::new();
            if let Ok(entries) = std::fs::read_dir(&dir) {
                let mut files: Vec<_> = entries
                    .flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
                    .collect();
                files.sort_by_key(|e| e.file_name());
                for f in files {
                    if let Ok(s) = std::fs::read_to_string(f.path()) {
                        all.push_str(&s);
                    }
                }
            }
            if settled(&all) {
                return all;
            }
            if Instant::now() > deadline {
                // Say what did arrive: "never settled" alone cannot tell a
                // write that was cut short from one that never started.
                panic!("session {session_id}'s stdin never settled; it holds {all:?}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// A session naming itself, spelled the way the skill tells it to: the whole
/// address as one string, the name as the plain-text body.
fn post_name(url: &str, name: &str) {
    use std::io::{Read as _, Write as _};
    let rest = url.trim_start_matches("http://");
    let (addr, target) = rest.split_once('/').expect("url has a path");
    let mut sock = std::net::TcpStream::connect(addr).expect("connect to the hook listener");
    let req = format!(
        "POST /{target} HTTP/1.1\r\nHost: localhost\r\ncontent-length: {}\r\n\r\n{name}",
        name.len()
    );
    sock.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    let _ = sock.read_to_string(&mut resp);
    assert!(resp.starts_with("HTTP/1.1 200"), "the name was not answered: {resp}");
}

/// Poll until `done`, for the things another thread makes true.
fn wait_for(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

struct Launch {
    cwd: String,
    /// Where this session was told to post its own name, out of the
    /// environment the process actually got. Empty when the world had no
    /// listener to point at — which is the honest answer, not a failure.
    name_url: String,
    /// The two channels a session can ask on, each already carrying this
    /// session's own token. Empty for the same reason `name_url` is.
    peers_url: String,
    send_url: String,
    args: Vec<String>,
}

/// Where a world keeps this desk's sockets: `/tmp/marol-<uid>`, the same
/// place the app puts them and for the same reason tmux keeps its own there —
/// a socket address has about 104 bytes, and a home directory is unbounded.
fn world_socket_dir() -> PathBuf {
    let uid = std::process::Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    PathBuf::from(format!("/tmp/marol-{uid}"))
}

/// The socket holding one session, if a world is holding it.
///
/// Matched by session id rather than by taking the only entry: that directory
/// is per-user, not per-test, so a whole run's worth of harnesses shares it —
/// and so does whatever desk the person running the tests has open.
fn held_socket(session_id: &str) -> Option<PathBuf> {
    std::fs::read_dir(world_socket_dir())
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.to_string_lossy().ends_with(session_id))
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.core.shutdown();
        // `shutdown` detaches held sessions rather than ending them — that is
        // the feature, and in production the next start sweeps whatever has
        // no card left. A test is the one case where nothing ever comes back:
        // the sweep only reaches sockets tagged with its own data directory,
        // and the next test has a different one. Left alone, a full run ends
        // with one idle tmux server per session.
        let tag = pty::desk_tag(&self.root.join("data").to_string_lossy());
        for s in self.core.sessions() {
            let sock = pty::hold_socket(&tag, &s.id);
            let _ = std::process::Command::new("tmux")
                .args(["-L", &sock, "kill-server"])
                .output();
            // The server exits but leaves its socket inode; a full run would
            // otherwise strew hundreds of dead files through the tmux dir.
            if let Some(dir) = core::tmux_socket_dir() {
                let _ = std::fs::remove_file(dir.join(&sock));
            }
        }
        // The same for the worlds behind the stand-in wsl.exe. Those sockets
        // live in a directory shared with every other harness and with the
        // person's own desk, so only this harness's own sessions are touched.
        for s in self.core.sessions() {
            if let Some(p) = held_socket(&s.id) {
                let _ = std::process::Command::new("tmux")
                    .args(["-S", &p.to_string_lossy(), "kill-server"])
                    .output();
                let _ = std::fs::remove_file(&p);
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The whole of M1 in one pass: a card becomes a worktree, a branch, a
/// running session that was handed the prompt, and a row that remembers all
/// of it.
#[test]
fn opening_an_attempt_puts_an_agent_in_a_worktree_of_its_own() {
    let h = Harness::new("open");
    let _guard = h.rt.enter();
    let task = h.card("修好登入", "登入頁在 Safari 會白畫面");

    let opened = h.start(&task, "claude");

    assert_eq!(opened.branch, "marol/task-".to_string() + &task[..8] + "-1");
    assert!(opened.prompt_sent, "claude's conventions are measured");
    assert!(Path::new(&opened.worktree_path).is_dir());

    // The agent is in the worktree, not in the repository the person works in.
    assert_eq!(
        std::fs::canonicalize(h.cwd_of(&opened.session_id)).unwrap(),
        std::fs::canonicalize(&opened.worktree_path).unwrap()
    );

    // The prompt is the last argument, after every option.
    let args = h.args_of(&opened.session_id);
    assert_eq!(
        args.last().map(String::as_str),
        Some(opened.prompt.as_str()),
        "the prompt was not the final argument: {args:?}"
    );
    assert!(
        !args.iter().any(|a| a == "--continue"),
        "a fresh worktree has no history to continue: {args:?}"
    );
    // And it arrived whole, newlines and all, as one argument.
    assert!(opened.prompt.contains("登入頁在 Safari 會白畫面"));
    assert!(opened.prompt.contains(&opened.branch));

    // The card moved itself onto the board's running column.
    let board = h.core.task_board();
    assert_eq!(board[0].task.lifecycle, Lifecycle::Running);
    assert_eq!(board[0].attempts.len(), 1);
    assert_eq!(
        board[0].attempts[0].session_id.as_deref(),
        Some(opened.session_id.as_str())
    );

    // A brand-new worktree opens on the folder-trust prompt, which no hook
    // can report. If this were `Starting`, the badge would miss the state
    // every attempt begins in.
    let session = h
        .core
        .sessions()
        .into_iter()
        .find(|s| s.id == opened.session_id)
        .unwrap();
    assert_eq!(session.status, Status::AwaitingTrust);
    assert!(session.status.needs_you());

    // What the agent was asked is recorded as sent, not reconstructed later.
    let events = h.core.attempt_events(&opened.attempt_id).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "prompt");
    assert_eq!(events[0].detail.as_deref(), Some(opened.prompt.as_str()));
}

#[test]
fn two_attempts_at_one_card_get_a_worktree_each() {
    let h = Harness::new("two");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");

    let a = h.start(&task, "claude");
    let b = h.start(&task, "claude");

    assert_eq!(a.branch, "marol/fix-login-1");
    assert_eq!(b.branch, "marol/fix-login-2");
    assert_ne!(a.worktree_path, b.worktree_path);

    // Both are live, and neither can see the other's files.
    std::fs::write(Path::new(&a.worktree_path).join("only-a.txt"), "a").unwrap();
    assert!(!Path::new(&b.worktree_path).join("only-a.txt").exists());

    let board = h.core.task_board();
    assert_eq!(board[0].attempts.len(), 2);
    // The card is still one card, on the board once.
    assert_eq!(board.len(), 1);
}

/// The step that must not be skipped, and the order it has to happen in.
#[test]
fn finishing_an_attempt_freezes_the_diff_before_taking_the_worktree_back() {
    let h = Harness::new("finish");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");

    // What the agent did: an edit and a new file.
    std::fs::write(Path::new(&a.worktree_path).join("app.txt"), "fixed\n").unwrap();
    std::fs::write(Path::new(&a.worktree_path).join("new.rs"), "fn main() {}\n").unwrap();

    h.core.finish_attempt(&a.attempt_id, Outcome::Merged).unwrap();

    assert!(
        !Path::new(&a.worktree_path).exists(),
        "the worktree is still on disk; this is how the disk fills up"
    );
    let listed = git(&h.repo, &["worktree", "list"]);
    assert!(!listed.contains(&a.worktree_path), "git still lists it: {listed}");

    // The diff outlived the directory it described.
    let diff = h.core.attempt_diff(&a.attempt_id).unwrap();
    assert!(diff.contains("fixed"), "the edit was lost with the worktree:\n{diff}");
    assert!(diff.contains("new.rs"), "the new file was lost:\n{diff}");

    // The branch stays: it is what a merged attempt was merged from.
    assert!(git(&h.repo, &["branch", "--list", &a.branch]).contains(&a.branch));
}

/// A finished attempt has no worktree, so there is nothing to reopen into.
/// Saying so beats spawning a terminal in a directory that is not there.
#[test]
fn a_finished_attempt_cannot_be_reopened() {
    let h = Harness::new("reopenfin");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");
    h.core.finish_attempt(&a.attempt_id, Outcome::Discarded).unwrap();

    let err = h
        .core
        .reopen_attempt(&a.attempt_id, 100, 30)
        .expect_err("a removed worktree cannot host a session");
    assert!(err.to_string().contains("finished"), "unhelpful: {err}");
}

/// After a restart every attempt is in this state. Reopening continues the
/// agent's own history and must not send the prompt again — a second copy
/// would set it off doing the whole card from the beginning.
#[test]
fn reopening_an_attempt_continues_instead_of_asking_again() {
    let h = Harness::new("reopen");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");
    h.launches(&a.session_id, 1); // let the first launch land

    h.core.close_session(&a.session_id).unwrap();
    let session_id = h.core.reopen_attempt(&a.attempt_id, 100, 30).expect("reopen");

    let second = h.launches(&session_id, 2).pop().unwrap();
    assert!(
        second.args.iter().any(|a| a == "--continue"),
        "reopening did not pass --continue: {:?}",
        second.args
    );
    assert!(
        !second.args.iter().any(|a| a.contains("[Marol")),
        "the prompt was sent a second time; the agent would redo the whole card: {:?}",
        second.args
    );
}

/// Honest degradation: only two CLIs' argument conventions are measured.
/// For anything else the session is still real, and the prompt is built and
/// handed to the person instead of guessed at.
#[test]
fn an_agent_whose_conventions_we_have_not_measured_is_not_sent_a_prompt() {
    let h = Harness::new("unmeasured");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");

    let opened = h.start(&task, "gemini");

    assert!(!opened.prompt_sent);
    // Built and available to copy, just not delivered.
    assert!(opened.prompt.contains("make it work"));

    let args = h.args_of(&opened.session_id);
    assert!(
        args.is_empty(),
        "an unmeasured CLI was handed arguments anyway: {args:?}"
    );
    // The worktree is real regardless — this is a working session, not a stub.
    assert!(Path::new(&opened.worktree_path).is_dir());
}

/// The prompt dialog is editable, and what it sends is what gets recorded.
#[test]
fn an_edited_prompt_is_what_gets_sent_and_what_gets_recorded() {
    let h = Harness::new("edited");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "the original request");

    let edited = "我改過的 prompt\n\n第二行".to_string();
    let opened = h
        .core
        .start_attempt(&task, "claude".into(), Some(edited.clone()), PermissionMode::Normal, 100, 30)
        .unwrap()
        .attempt
        .unwrap();

    assert_eq!(opened.prompt, edited);
    assert_eq!(h.args_of(&opened.session_id).last(), Some(&edited));
    let events = h.core.attempt_events(&opened.attempt_id).unwrap();
    assert_eq!(events[0].detail.as_deref(), Some(edited.as_str()));
    assert!(
        !events[0].detail.as_deref().unwrap().contains("the original request"),
        "the timeline shows the template, not what was actually sent"
    );
}

/* -------------------------- queue and finishing ------------------------ */

/// Over the limit, a start waits instead of being refused. The answer to
/// "too many at once" is "later", not "no".
#[test]
fn a_start_over_the_limit_waits_its_turn_and_then_goes_by_itself() {
    let h = Harness::new("queue");
    let _guard = h.rt.enter();
    h.core.set_max_concurrent(1).unwrap();

    let first = h.card("First", "p");
    let second = h.card("Second", "p");

    let a = h
        .core
        .start_attempt(&first, "claude".into(), None, PermissionMode::Normal, 100, 30)
        .unwrap();
    assert!(a.attempt.is_some(), "the first one had room");

    let b = h
        .core
        .start_attempt(&second, "claude".into(), Some("我排隊的 prompt".into()), PermissionMode::Normal, 100, 30)
        .unwrap();
    assert!(b.attempt.is_none(), "the second should not have started");
    assert_eq!(b.queued_at, Some(1));
    assert_eq!(h.core.queue().len(), 1);

    // The board shows where it is waiting.
    let board = h.core.task_board();
    let waiting = board.iter().find(|t| t.task.id == second).unwrap();
    assert_eq!(waiting.queued_at, Some(1));
    assert!(waiting.attempts.is_empty());

    // A slot frees, and the queue moves on its own.
    let session = a.attempt.unwrap().session_id;
    h.core.close_session(&session).unwrap();

    let started = wait_for(Duration::from_secs(10), || {
        h.core
            .task_board()
            .into_iter()
            .find(|t| t.task.id == second)
            .map(|t| !t.attempts.is_empty())
            .unwrap_or(false)
    });
    assert!(started, "the queue never moved after a slot came free");
    assert!(h.core.queue().is_empty());

    // And it sent the prompt that was approved, not a fresh render.
    let attempt = h
        .core
        .task_board()
        .into_iter()
        .find(|t| t.task.id == second)
        .unwrap()
        .attempts[0]
        .attempt
        .clone();
    let events = h.core.attempt_events(&attempt.id).unwrap();
    assert_eq!(events[0].detail.as_deref(), Some("我排隊的 prompt"));
}

/// Raising the limit is a way of saying "go now", so it has to be one.
#[test]
fn raising_the_limit_releases_what_was_waiting() {
    let h = Harness::new("raise");
    let _guard = h.rt.enter();
    h.core.set_max_concurrent(1).unwrap();

    let first = h.card("First", "p");
    let second = h.card("Second", "p");
    h.core.start_attempt(&first, "claude".into(), None, PermissionMode::Normal, 100, 30).unwrap();
    h.core.start_attempt(&second, "claude".into(), None, PermissionMode::Normal, 100, 30).unwrap();
    assert_eq!(h.core.queue().len(), 1);

    h.core.set_max_concurrent(2).unwrap();
    assert!(h.core.queue().is_empty(), "raising the limit left it waiting");
    assert_eq!(h.core.running_attempts(), 2);
}

/// Pressing 開始 again on a card that is already waiting means "these are the
/// settings I want", not "run it twice".
#[test]
fn a_card_can_only_be_in_the_queue_once() {
    let h = Harness::new("once");
    let _guard = h.rt.enter();
    h.core.set_max_concurrent(1).unwrap();

    let first = h.card("First", "p");
    let second = h.card("Second", "p");
    h.core.start_attempt(&first, "claude".into(), None, PermissionMode::Normal, 100, 30).unwrap();
    h.core
        .start_attempt(&second, "claude".into(), Some("first try".into()), PermissionMode::Normal, 100, 30)
        .unwrap();
    h.core
        .start_attempt(&second, "codex".into(), Some("changed my mind".into()), PermissionMode::Normal, 100, 30)
        .unwrap();

    let queue = h.core.queue();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].agent, "codex");
    assert_eq!(queue[0].prompt, "changed my mind");
}

#[test]
fn a_queued_card_can_be_taken_back_out() {
    let h = Harness::new("cancel");
    let _guard = h.rt.enter();
    h.core.set_max_concurrent(1).unwrap();
    let first = h.card("First", "p");
    let second = h.card("Second", "p");
    h.core.start_attempt(&first, "claude".into(), None, PermissionMode::Normal, 100, 30).unwrap();
    h.core.start_attempt(&second, "claude".into(), None, PermissionMode::Normal, 100, 30).unwrap();

    h.core.cancel_queued(&second).unwrap();
    assert!(h.core.queue().is_empty());
    assert_eq!(
        h.core.task_board().into_iter().find(|t| t.task.id == second).unwrap().queued_at,
        None
    );
}

/// Only attempts count against the limit. An ad-hoc session is something a
/// person opened deliberately and is already looking at.
#[test]
fn ad_hoc_sessions_do_not_use_up_the_limit() {
    let h = Harness::new("adhoclimit");
    let _guard = h.rt.enter();
    h.core.set_max_concurrent(1).unwrap();

    h.core
        .new_session(h.repo.to_string_lossy().into(), "claude".into(), vec![], 100, 30)
        .unwrap();
    let task = h.card("First", "p");
    let r = h.core.start_attempt(&task, "claude".into(), None, PermissionMode::Normal, 100, 30).unwrap();
    assert!(r.attempt.is_some(), "an ad-hoc session took the attempt's slot");
}

/* ------------------------------ merging -------------------------------- */

#[test]
fn merging_folds_the_branch_into_the_base_and_closes_the_attempt_out() {
    let h = Harness::new("merge");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");

    std::fs::write(Path::new(&a.worktree_path).join("app.txt"), "fixed\n").unwrap();
    git(Path::new(&a.worktree_path), &["add", "-A"]);
    git(Path::new(&a.worktree_path), &["commit", "-qm", "fix it"]);

    h.core.merge_attempt(&a.attempt_id).expect("merge");

    // The work is on the base branch, in the checkout the person works in.
    assert_eq!(
        std::fs::read_to_string(h.repo.join("app.txt")).unwrap(),
        "fixed\n"
    );
    // `--no-ff`, so the attempt stays legible as one piece of work.
    assert!(git(&h.repo, &["log", "--oneline", "--merges", "-1"]).contains("Merge marol/"));

    // And the attempt is closed out: worktree gone, diff kept.
    assert!(!Path::new(&a.worktree_path).exists());
    assert!(h.core.attempt_diff(&a.attempt_id).unwrap().contains("fixed"));
}

/// The prompt asks the agent to commit. When it has not, merging the branch
/// would produce a merge that does not contain the work — and the work is in
/// a directory that is about to be removed.
#[test]
fn merging_refuses_while_the_worktree_still_has_uncommitted_work() {
    let h = Harness::new("dirtywt");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");
    std::fs::write(Path::new(&a.worktree_path).join("app.txt"), "not committed\n").unwrap();

    let err = h
        .core
        .merge_attempt(&a.attempt_id)
        .expect_err("a merge that would drop the work must not happen");
    assert!(
        err.to_string()
            .contains(&i18n::merge_dirty_worktree(i18n::Locale::default(), &a.branch)),
        "unhelpful: {err}"
    );

    // Nothing was given up on the way to finding out.
    assert!(Path::new(&a.worktree_path).exists());
    assert!(h.core.task_board()[0].attempts[0].attempt.outcome.is_none());
}

/// Merging into a checkout that is on another branch would rewrite what the
/// person is in the middle of.
#[test]
fn merging_refuses_when_the_checkout_is_somewhere_else() {
    let h = Harness::new("otherbranch");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");
    std::fs::write(Path::new(&a.worktree_path).join("app.txt"), "fixed\n").unwrap();
    git(Path::new(&a.worktree_path), &["add", "-A"]);
    git(Path::new(&a.worktree_path), &["commit", "-qm", "fix it"]);

    git(&h.repo, &["checkout", "-q", "-b", "something-else"]);
    let err = h
        .core
        .merge_attempt(&a.attempt_id)
        .expect_err("must not merge into whatever happens to be checked out");
    assert!(err.to_string().contains("something-else"), "unhelpful: {err}");
    assert!(Path::new(&a.worktree_path).exists());
}

/// Two agents on one card is a comparison; the merge is what decides it. The
/// attempt that did not land is superseded — its worktree comes back, its
/// diff freezes — rather than left holding a directory forever with nothing
/// left to decide about it.
#[test]
fn merging_one_attempt_supersedes_the_other_still_open_one() {
    let h = Harness::new("supersede");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");
    let b = h.start(&task, "codex");

    // Both worked; only one gets merged.
    std::fs::write(Path::new(&a.worktree_path).join("app.txt"), "a's fix\n").unwrap();
    git(Path::new(&a.worktree_path), &["add", "-A"]);
    git(Path::new(&a.worktree_path), &["commit", "-qm", "fix it"]);
    std::fs::write(Path::new(&b.worktree_path).join("app.txt"), "b's fix\n").unwrap();

    h.core.merge_attempt(&a.attempt_id).expect("merge");

    let board = h.core.task_board();
    let outcomes: Vec<_> = board[0]
        .attempts
        .iter()
        .map(|x| (x.attempt.seq, x.attempt.outcome))
        .collect();
    assert_eq!(
        outcomes,
        vec![(1, Some(Outcome::Merged)), (2, Some(Outcome::Superseded))]
    );

    // The loser's worktree came back, and its evidence did not go with it.
    assert!(!Path::new(&b.worktree_path).exists());
    let frozen = h.core.attempt_diff(&b.attempt_id).unwrap();
    assert!(frozen.contains("b's fix"), "the superseded diff was lost:\n{frozen}");

    // Its branch keeps its number reserved, exactly like any finished attempt.
    assert_eq!(h.core.running_attempts(), 0);
}

#[test]
fn merging_says_so_when_the_attempt_did_nothing() {
    let h = Harness::new("nothing");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");

    let err = h
        .core
        .merge_attempt(&a.attempt_id)
        .expect_err("an empty branch has nothing to merge");
    assert!(
        err.to_string().contains(&i18n::merge_nothing_ahead(
            i18n::Locale::default(),
            &a.branch,
            "main"
        )),
        "unhelpful: {err}"
    );
}

/* --------------------------- cards spanning repos ---------------------- */

/// The whole shape, end to end: one card, two repositories, one session
/// standing in a workspace that holds a checkout of each — and both of the
/// person's own checkouts untouched, which is the safety argument surviving
/// the generalisation.
#[test]
fn a_card_spanning_two_repositories_runs_one_session_over_both() {
    let h = Harness::new("span");
    let _guard = h.rt.enter();
    let api = h.second_repo("api");
    let task = h.card_spanning(
        "把欄位對起來",
        "兩邊一起改",
        &[(&api.to_string_lossy(), "main")],
    );
    let a = h.start(&task, "claude");

    // The session's directory is the workspace, not either checkout.
    let root = Path::new(&a.worktree_path);
    assert!(root.is_dir());
    assert!(!root.join(".git").exists(), "the workspace is not a repository");
    assert!(root.join("repo").join("app.txt").exists());
    assert!(root.join("api").join("service.txt").exists());
    assert_eq!(h.launches(&a.session_id, 1)[0].cwd, a.worktree_path);

    // One branch name, in both.
    for dir in ["repo", "api"] {
        assert_eq!(
            git(&root.join(dir), &["rev-parse", "--abbrev-ref", "HEAD"]),
            a.branch,
            "{dir} is not on the attempt's branch"
        );
    }
    // Neither of the person's own checkouts moved.
    assert_eq!(git(&h.repo, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    assert_eq!(git(&api, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");

    // The opening message names both, or the agent would go looking for the
    // files in the directory it woke up in and find folders.
    let prompt = &h.args_of(&a.session_id).pop().unwrap();
    assert!(prompt.contains("repo/"), "{prompt}");
    assert!(prompt.contains("api/"), "{prompt}");
}

/// The diff is one diff over both checkouts, and its paths are relative to
/// where the session is standing — which is what makes a review comment
/// naming `api/service.txt` point at something the agent can open.
#[test]
fn the_diff_covers_every_checkout_with_paths_the_agent_can_open() {
    let h = Harness::new("span-diff");
    let _guard = h.rt.enter();
    let api = h.second_repo("api");
    let task = h.card_spanning("x", "y", &[(&api.to_string_lossy(), "main")]);
    let a = h.start(&task, "claude");
    let root = Path::new(&a.worktree_path);

    std::fs::write(root.join("repo").join("app.txt"), "client side\n").unwrap();
    std::fs::write(root.join("api").join("service.txt"), "server side\n").unwrap();

    let diff = h.core.attempt_diff(&a.attempt_id).unwrap();
    assert!(diff.contains("b/repo/app.txt"), "{diff}");
    assert!(diff.contains("b/api/service.txt"), "{diff}");
    assert!(diff.contains("client side") && diff.contains("server side"), "{diff}");

    // And the editable diff resolves those same paths back to the right
    // checkout — writing the client's file into the service is the failure
    // this lookup exists to prevent.
    let f = h.core.attempt_file(&a.attempt_id, "api/service.txt").unwrap();
    assert_eq!(f.work.as_deref(), Some("server side\n"));
    assert_eq!(f.base.as_deref(), Some("one\n"));
    // Saving is refused mid-turn, here as anywhere.
    h.core.close_session(&a.session_id).unwrap();
    h.core
        .write_attempt_file(&a.attempt_id, "api/service.txt", "edited by hand\n", None)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("api").join("service.txt")).unwrap(),
        "edited by hand\n"
    );

    // A path in no checkout is refused rather than guessed at.
    assert!(h.core.attempt_file(&a.attempt_id, "nowhere/x.txt").is_err());
}

/// Merging is several merges, and every refusal is asked before any of them
/// runs: discovering the second repository's uncommitted work after the first
/// has landed is exactly the half-landed state those refusals prevent.
#[test]
fn merging_a_spanning_card_refuses_as_a_whole_before_it_moves_anything() {
    let h = Harness::new("span-merge");
    let _guard = h.rt.enter();
    let api = h.second_repo("api");
    let task = h.card_spanning("x", "y", &[(&api.to_string_lossy(), "main")]);
    let a = h.start(&task, "claude");
    let root = Path::new(&a.worktree_path);

    // The first is committed; the second is not.
    std::fs::write(root.join("repo").join("app.txt"), "client side\n").unwrap();
    git(&root.join("repo"), &["add", "-A"]);
    git(&root.join("repo"), &["commit", "-qm", "client"]);
    std::fs::write(root.join("api").join("service.txt"), "server side\n").unwrap();

    let err = h
        .core
        .merge_attempt(&a.attempt_id)
        .expect_err("a merge that would half-land must not start");
    assert!(
        err.to_string()
            .contains(&i18n::merge_dirty_worktree(i18n::Locale::default(), &a.branch)),
        "unhelpful: {err}"
    );
    // Nothing moved: the first repository is still where it was.
    assert_eq!(
        std::fs::read_to_string(h.repo.join("app.txt")).unwrap(),
        "one\n",
        "the first repository was merged before the second was checked"
    );
    assert!(h.core.task_board()[0].attempts[0].attempt.outcome.is_none());

    // Commit the second and it goes through, both of them.
    git(&root.join("api"), &["add", "-A"]);
    git(&root.join("api"), &["commit", "-qm", "server"]);
    h.core.merge_attempt(&a.attempt_id).expect("merge");
    assert_eq!(
        std::fs::read_to_string(h.repo.join("app.txt")).unwrap(),
        "client side\n"
    );
    assert_eq!(
        std::fs::read_to_string(api.join("service.txt")).unwrap(),
        "server side\n"
    );

    // Closed out, and the whole workspace given back — checkouts and the
    // directory that held them.
    assert!(!root.exists(), "the workspace outlived the attempt");
    let frozen = h.core.attempt_diff(&a.attempt_id).unwrap();
    assert!(frozen.contains("client side") && frozen.contains("server side"), "{frozen}");
}

/// A checkpoint is a moment in the work, not a count of how often one
/// checkout happened to change. The ordinals are shared, so restoring to one
/// reassembles a workspace that actually existed.
#[test]
fn a_checkpoint_numbers_one_moment_the_same_in_every_checkout() {
    let h = Harness::new("span-ckpt");
    let _guard = h.rt.enter();
    let api = h.second_repo("api");
    let task = h.card_spanning("x", "y", &[(&api.to_string_lossy(), "main")]);
    let a = h.start(&task, "claude");
    let root = Path::new(&a.worktree_path);

    // Moment one touches only the client.
    std::fs::write(root.join("repo").join("app.txt"), "client v1\n").unwrap();
    let one = h.core.checkpoint_now(&a.attempt_id).unwrap().expect("a snapshot");
    assert_eq!(one.n, 1);

    // Moment two touches only the service. Numbered against the attempt, not
    // against the checkout — this is 2 even though it is the service's first.
    std::fs::write(root.join("api").join("service.txt"), "server v1\n").unwrap();
    let two = h.core.checkpoint_now(&a.attempt_id).unwrap().expect("a snapshot");
    assert_eq!(two.n, 2, "the service restarted the numbering");
    assert_eq!(
        h.core
            .list_checkpoints(&a.attempt_id)
            .unwrap()
            .iter()
            .map(|c| c.n)
            .collect::<Vec<_>>(),
        vec![1, 2],
        "one timeline for the attempt, not one per repository"
    );

    // Ruin both, then walk back to moment one. The client returns to what it
    // held then; the service had no snapshot at 1, so it returns to its base
    // — which is how it looked at that moment.
    std::fs::write(root.join("repo").join("app.txt"), "ruined\n").unwrap();
    std::fs::write(root.join("api").join("service.txt"), "ruined\n").unwrap();
    // Restoring is refused mid-turn, here as anywhere.
    h.core.close_session(&a.session_id).unwrap();
    h.core.restore_checkpoint(&a.attempt_id, 1).expect("restore");
    assert_eq!(
        std::fs::read_to_string(root.join("repo").join("app.txt")).unwrap(),
        "client v1\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("api").join("service.txt")).unwrap(),
        "one\n",
        "a checkout with no snapshot at that moment must come back to its base"
    );
}

/// Park gives back every checkout and keeps every branch; resume grows them
/// all back at the paths `--continue` will look for, with the shelf on top.
#[test]
fn parking_a_spanning_card_gives_back_every_checkout_and_resume_grows_them_back() {
    let h = Harness::new("span-park");
    let _guard = h.rt.enter();
    let api = h.second_repo("api");
    let task = h.card_spanning("x", "y", &[(&api.to_string_lossy(), "main")]);
    let a = h.start(&task, "claude");
    let root = Path::new(&a.worktree_path);

    std::fs::write(root.join("repo").join("app.txt"), "half done\n").unwrap();
    std::fs::write(root.join("api").join("notes.txt"), "todo\n").unwrap();
    h.core.close_session(&a.session_id).unwrap();

    h.core.park_attempt(&a.attempt_id).expect("park");
    assert!(!root.join("repo").exists(), "a checkout survived the park");
    assert!(!root.join("api").exists(), "a checkout survived the park");

    h.core.resume_attempt(&a.attempt_id, 100, 30).expect("resume");
    // Both back, at the same paths, with the uncommitted work restored.
    assert_eq!(
        std::fs::read_to_string(root.join("repo").join("app.txt")).unwrap(),
        "half done\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("api").join("notes.txt")).unwrap(),
        "todo\n",
        "a file that only ever lived on the shelf did not come back"
    );
}

/// Two refusals a card has to make before it can exist, both because the
/// workspace it describes could not.
#[test]
fn a_card_cannot_span_two_worlds_or_name_one_repository_twice() {
    let h = Harness::new("span-refuse");
    let _guard = h.rt.enter();
    let here = h.repo.to_string_lossy().to_string();

    let err = h
        .core
        .create_task(
            "x".into(),
            "y".into(),
            here.clone(),
            "main".into(),
            vec![crate::store::TaskRepo {
                repo_path: format!("wsl://TestOS{here}"),
                base_branch: "main".into(),
            }],
        )
        .expect_err("a directory cannot straddle a world boundary");
    assert!(
        err.to_string()
            .contains("must be on the same host"),
        "{err}"
    );

    let err = h
        .core
        .create_task(
            "x".into(),
            "y".into(),
            here.clone(),
            "main".into(),
            vec![crate::store::TaskRepo {
                repo_path: here.clone(),
                base_branch: "main".into(),
            }],
        )
        .expect_err("one repository twice is two worktrees of one branch");
    assert!(
        err.to_string()
            .contains(&i18n::repo_twice(i18n::Locale::default(), &here)),
        "{err}"
    );

    assert!(h.core.task_board().is_empty(), "a refused card was created anyway");
}

/// Each repository's own setup script runs, in its own checkout. Running only
/// the first would start the agent in a workspace half of which does not
/// build.
#[test]
fn every_repositorys_setup_script_runs_in_its_own_checkout() {
    let h = Harness::new("span-setup");
    let _guard = h.rt.enter();
    let api = h.second_repo("api");
    for (repo, marker) in [(&h.repo, "client"), (&api, "server")] {
        std::fs::create_dir_all(repo.join(".marol")).unwrap();
        std::fs::write(
            repo.join(".marol/config.json"),
            format!(r#"{{"setup": "printf '%s' {marker} > setup-ran.txt"}}"#),
        )
        .unwrap();
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-qm", "config"]);
    }

    let task = h.card_spanning("x", "y", &[(&api.to_string_lossy(), "main")]);
    let a = h.start(&task, "claude");
    let root = Path::new(&a.worktree_path);
    h.launches(&a.session_id, 1);

    // Each landed in its own checkout, not both in the workspace root.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let client = std::fs::read_to_string(root.join("repo").join("setup-ran.txt"));
        let server = std::fs::read_to_string(root.join("api").join("setup-ran.txt"));
        if let (Ok(c), Ok(s)) = (&client, &server) {
            assert_eq!(c, "client");
            assert_eq!(s, "server");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "setup scripts did not both run: {client:?} / {server:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !root.join("setup-ran.txt").exists(),
        "a setup script ran in the workspace instead of its checkout"
    );
}

/// Two repositories can each have a `dev`, and two buttons saying `dev` would
/// be two nobody could tell apart.
#[test]
fn run_scripts_from_two_repositories_are_named_by_their_checkout() {
    let h = Harness::new("span-run");
    let _guard = h.rt.enter();
    let api = h.second_repo("api");
    for repo in [&h.repo, &api] {
        std::fs::create_dir_all(repo.join(".marol")).unwrap();
        std::fs::write(
            repo.join(".marol/config.json"),
            r#"{"run": [{"name": "dev", "command": "printf '%s' $PWD > ran.txt"}]}"#,
        )
        .unwrap();
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-qm", "config"]);
    }

    let task = h.card_spanning("x", "y", &[(&api.to_string_lossy(), "main")]);
    let a = h.start(&task, "claude");
    let root = Path::new(&a.worktree_path);

    let mut names = h.core.list_run_scripts(&a.attempt_id).unwrap();
    names.sort();
    assert_eq!(names, vec!["api:dev", "repo:dev"]);

    // And pressing one starts it where its own `package.json` would be.
    h.core.run_script(&a.attempt_id, "api:dev", 100, 30).expect("run");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !root.join("api").join("ran.txt").exists() {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        root.join("api").join("ran.txt").exists(),
        "the service's dev server did not start in the service"
    );
    assert!(!root.join("repo").join("ran.txt").exists(), "the wrong one ran");
}

/// `$MAROL_ROOT_PATH` names the *first* repository, not the first one that
/// happened to have a setup script. Taking it from the first step would point
/// a `cp "$MAROL_ROOT_PATH/.env"` at the wrong repository whenever the first
/// had nothing to run — and the copy would succeed, quietly, with the wrong
/// file.
#[test]
fn the_root_path_the_agent_inherits_is_the_first_repository() {
    let h = Harness::new("span-root");
    let _guard = h.rt.enter();
    let api = h.second_repo("api");
    // Only the *second* repository declares a setup script.
    std::fs::create_dir_all(api.join(".marol")).unwrap();
    std::fs::write(api.join(".marol/config.json"), r#"{"setup": "true"}"#).unwrap();
    git(&api, &["add", "-A"]);
    git(&api, &["commit", "-qm", "config"]);

    let task = h.card_spanning("x", "y", &[(&api.to_string_lossy(), "main")]);
    let a = h.start(&task, "claude");
    let launch = h.launches(&a.session_id, 1).pop().unwrap();
    // The wrap runs `sh -c`, and the whole script is on the command line.
    let line = launch.args.join(" ");
    assert!(
        line.contains(&h.repo.to_string_lossy().to_string()),
        "the first repository is not the root path handed to the setup:\n{line}"
    );
}

/// A checkpoint number no moment carries is a mistake to report, not a
/// baseline to approximate: `at_or_before` would quietly diff against an
/// older snapshot, and `restore_checkpoint` — which does check — would then
/// refuse the very number the drawer had just compared against.
#[test]
fn diffing_against_a_checkpoint_that_never_existed_is_refused() {
    let h = Harness::new("ckpt-unknown");
    let _guard = h.rt.enter();
    let task = h.card("x", "y");
    let a = h.start(&task, "claude");

    std::fs::write(Path::new(&a.worktree_path).join("app.txt"), "changed\n").unwrap();
    h.core.checkpoint_now(&a.attempt_id).unwrap().expect("a snapshot");

    assert!(h.core.attempt_diff_from(&a.attempt_id, Some(1)).is_ok());
    let err = h
        .core
        .attempt_diff_from(&a.attempt_id, Some(9))
        .expect_err("a number no checkpoint carries must be reported");
    assert!(err.to_string().contains("checkpoint #9"), "{err}");
    // And the two paths agree about it.
    assert!(h.core.restore_checkpoint(&a.attempt_id, 9).is_err());
}

/// The Knows tab answers a question about the repositories, and for a
/// workspace that is every checkout under it. Asking the workspace itself
/// would report "no CLAUDE.md here" about a session whose agent has read two.
#[test]
fn the_knows_tab_reads_every_checkouts_conventions() {
    let h = Harness::new("span-knows");
    let _guard = h.rt.enter();
    let api = h.second_repo("api");
    std::fs::write(api.join("CLAUDE.md"), "service rules\n").unwrap();
    git(&api, &["add", "-A"]);
    git(&api, &["commit", "-qm", "rules"]);

    let task = h.card_spanning("x", "y", &[(&api.to_string_lossy(), "main")]);
    let a = h.start(&task, "claude");

    let docs = h.core.agent_docs(&a.worktree_path).unwrap();
    let claude: Vec<_> = docs
        .iter()
        .filter(|d| d.scope == "project" && d.name == "CLAUDE.md")
        .collect();
    assert_eq!(claude.len(), 2, "one slot per checkout: {claude:?}");
    assert!(
        claude.iter().any(|d| d.exists && d.path.contains("/api/")),
        "the service's own rules file was never looked for: {claude:?}"
    );
    // And each says which checkout it is, or the two rows would read as one
    // file listed twice rather than as the two different files they are.
    let mut dirs: Vec<&str> = claude.iter().map(|d| d.dir.as_str()).collect();
    dirs.sort();
    assert_eq!(dirs, vec!["api", "repo"]);
    // The machine's own rules belong to nobody's checkout.
    assert!(docs
        .iter()
        .filter(|d| d.scope == "global")
        .all(|d| d.dir.is_empty()));
}

/// One checkout that will not come back must not strand the others. The
/// attempt is already finished in the database by the time the removals run,
/// so bailing on the first failure would leave the rest of the workspace on
/// disk with nothing left pointing at it.
#[test]
fn a_checkout_that_will_not_come_back_does_not_strand_the_others() {
    let h = Harness::new("span-stuck");
    let _guard = h.rt.enter();
    let api = h.second_repo("api");
    let task = h.card_spanning("x", "y", &[(&api.to_string_lossy(), "main")]);
    let a = h.start(&task, "claude");
    let root = Path::new(&a.worktree_path);

    // The first repository is gone from under its worktree, so giving that
    // one back cannot succeed.
    std::fs::remove_dir_all(&h.repo).unwrap();

    let err = h
        .core
        .finish_attempt(&a.attempt_id, Outcome::Discarded)
        .expect_err("a checkout nobody could take back has to be reported");
    assert!(
        format!("{err:#}").contains("repo"),
        "the error must name what is stuck: {err:#}"
    );

    // The other one went back anyway, and the attempt is closed out.
    assert!(
        !root.join("api").exists(),
        "the second checkout was stranded by the first one's failure"
    );
    assert_eq!(
        h.core.task_board()[0].attempts[0].attempt.outcome,
        Some(Outcome::Discarded)
    );
}

/* ------------------------------ follow-ups ----------------------------- */

/// The review loop's delivery: feedback composed against the diff goes back
/// into the session's own terminal as ONE pasted message, and onto the
/// timeline as what was actually asked.
#[test]
fn a_followup_reaches_the_terminal_whole_and_lands_on_the_timeline() {
    let h = Harness::new("followup");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");
    h.launches(&a.session_id, 1); // the terminal is up

    let text = "[Marol 檢視回饋]\n1. auth.py:12 還是回 None\n2. 缺一個測試";
    h.core.send_followup(&a.session_id, text).expect("send");

    // The record: the opening prompt, then this, verbatim.
    let rows = h.timeline(&a.attempt_id, 2);
    assert_eq!(rows[1].kind, "prompt");
    assert_eq!(rows[1].detail.as_deref(), Some(text));

    // The delivery: newlines ride inside the bracketed paste, so the message
    // arrives as one message rather than one per line.
    // Wait for the *closing* marker, since that is the last thing written:
    // anything short of it means the paste is still on its way, not that it
    // arrived broken.
    let stdin = h.stdin_when(&a.session_id, |s| s.contains("\u{1b}[201~"));
    assert!(stdin.contains("\u{1b}[200~"), "no paste start: {stdin:?}");
    assert!(
        stdin.contains("還是回 None\n2. 缺一個測試"),
        "the message's own newlines did not survive: {stdin:?}"
    );
}

/// The same honesty the first prompt has: an unmeasured CLI's input
/// conventions are not guessed at. The text is the person's to paste, and
/// nothing lands on the timeline claiming it was sent.
#[test]
fn a_followup_to_an_unmeasured_cli_is_refused_rather_than_guessed() {
    let h = Harness::new("fugemini");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "gemini");
    h.launches(&a.session_id, 1);

    let err = h
        .core
        .send_followup(&a.session_id, "改一下")
        .expect_err("gemini's input conventions are not measured");
    assert!(err.to_string().contains("gemini"), "unhelpful: {err}");

    std::thread::sleep(Duration::from_millis(200));
    let rows = h.core.attempt_events(&a.attempt_id).unwrap();
    assert_eq!(rows.len(), 1, "a refused send still reached the timeline: {rows:?}");
}

/// The queue is a queue now, and that is what a second sender needs.
///
/// It used to be one slot per session: a second message overwrote the first,
/// with neither sender told and no trace the older one ever existed. Fine
/// while the only sender was the person in front of it; not fine the moment
/// another session can send one.
#[test]
fn several_queued_messages_all_arrive_and_keep_their_order() {
    let h = Harness::new("fuqueue");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");
    h.launches(&a.session_id, 1);

    h.core.queue_followup(&a.session_id, "first thing").expect("queue 1");
    h.core.queue_followup(&a.session_id, "second thing").expect("queue 2");
    assert!(h.core.sessions().iter().any(|s| s.id == a.session_id && s.has_followup));

    // The turn ends: Stop is what drains the queue.
    h.hook(&a.session_id, "idle", serde_json::json!({}));

    let stdin = h.stdin_when(&a.session_id, |s| s.contains("second thing"));
    let first = stdin.find("first thing").expect("the older message was dropped");
    let second = stdin.find("second thing").expect("the newer message was dropped");
    assert!(first < second, "the queue did not keep its order: {stdin:?}");
    // Coalesced into ONE turn: a second paste would land inside the turn the
    // first just started, which is the interleaving the queue exists to stop.
    assert_eq!(
        stdin.matches("\u{1b}[200~").count(),
        1,
        "the queue was delivered as more than one turn: {stdin:?}"
    );

    // Drained, so the row stops advertising a message that already went.
    let settled = Instant::now() + Duration::from_secs(5);
    while Instant::now() < settled {
        if h.core.sessions().iter().any(|s| s.id == a.session_id && !s.has_followup) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the follow-up flag survived the flush");
}

/// The bound exists so a session that never ends a turn cannot collect
/// messages for as long as the app runs — and the refusal is the feature:
/// a caller told "full" can say so to whoever sent it, which the single slot
/// could never do.
#[test]
fn a_full_queue_refuses_rather_than_dropping_the_oldest() {
    let h = Harness::new("fufull");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");
    h.launches(&a.session_id, 1);

    for i in 0..16 {
        h.core
            .queue_followup(&a.session_id, &format!("message {i}"))
            .unwrap_or_else(|e| panic!("message {i} should have fitted: {e}"));
    }
    let err = h
        .core
        .queue_followup(&a.session_id, "one too many")
        .expect_err("the seventeenth should not have fitted");
    assert!(err.to_string().contains("16"), "the refusal hides the limit: {err}");

    // And the one that was refused is the one that is missing — the first is
    // still there, which is the whole point of refusing at the back.
    h.hook(&a.session_id, "idle", serde_json::json!({}));
    let stdin = h.stdin_when(&a.session_id, |s| s.contains("message 15"));
    assert!(stdin.contains("message 0"), "the oldest was evicted after all: {stdin:?}");
    assert!(!stdin.contains("one too many"), "a refused message was sent: {stdin:?}");
}

#[test]
fn an_empty_followup_is_not_sent() {
    let h = Harness::new("fuempty");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");

    assert!(h.core.send_followup(&a.session_id, "  \n").is_err());
}

/* ---------------------------- crossings -------------------------------- */

/// What the two perf phases actually bought, counted rather than described.
///
/// Phase 1 made each read cost a process per *answer* instead of one per
/// question. Phase 2 took the process away: the world holds a shell open, so
/// a read is a line written to a pipe. Together they are the difference
/// between a card that costs thirty Windows processes on a timer and one that
/// costs none.
///
/// Zero rather than a small number, and exact rather than a bound. A bound
/// would let a regression put one crossing back per card per fifteen seconds
/// and still pass — which is precisely the shape of the bug being prevented.
///
/// The shell itself is a process, paid once for the life of the world; by the
/// time an attempt has been started its world has long since been reached, so
/// what is measured here is the steady state a running desk actually lives in.
#[test]
fn a_read_through_a_doorway_costs_no_process_at_all() {
    let h = Harness::new("crossings");
    let _guard = h.rt.enter();

    let repo_url = format!("wsl://TestOS{}", h.repo.display());
    let task = h
        .core
        .create_task("修好登入".into(), "make it work".into(), repo_url, "main".into(), Vec::new())
        .expect("a wsl:// card");
    let a = h.start(&task, "claude");
    // The launch is its own crossing and it happens on a thread; waiting for
    // it to be recorded is what stops it being counted against the read that
    // follows.
    h.launches(&a.session_id, 1);
    let inner = a.worktree_path.strip_prefix("wsl://TestOS").unwrap().to_string();

    // Untracked files are the ones that used to cost a crossing each: the
    // count came from `git diff --no-index` per file, in a loop on this side
    // of the door.
    for n in 0..5 {
        std::fs::write(Path::new(&inner).join(format!("new{n}.txt")), "hello\n").unwrap();
    }

    // The board's timer, which asks for this per open attempt every 15s.
    h.reset_crossings();
    let stat = h.core.attempt_stats(&a.attempt_id).expect("stats");
    let calls = h.crossings();
    assert!(
        calls.is_empty(),
        "the footprint cost {} crossings: {calls:#?}",
        calls.len()
    );
    // And it is still the right answer, five untracked files included.
    assert_eq!(stat.files, 5, "{stat:?}");

    // The diff, which the drawer asks for on open.
    h.reset_crossings();
    let diff = h.core.attempt_diff(&a.attempt_id).expect("diff");
    let calls = h.crossings();
    assert!(calls.is_empty(), "the diff cost {} crossings: {calls:#?}", calls.len());
    assert!(diff.contains("new4.txt"), "the untracked files left the diff: {diff}");

    // The Knows tab: six rules slots and two skill roots, which used to be
    // six crossings plus a listing and a test per skill.
    std::fs::create_dir_all(Path::new(&inner).join(".claude/skills/tidy")).unwrap();
    std::fs::write(Path::new(&inner).join(".claude/skills/tidy/SKILL.md"), "x").unwrap();
    std::fs::create_dir_all(Path::new(&inner).join(".claude/skills/notes")).unwrap();
    h.reset_crossings();
    let docs = h.core.agent_docs(&a.worktree_path).expect("docs");
    let calls = h.crossings();
    assert!(
        calls.is_empty(),
        "the Knows tab cost {} crossings: {calls:#?}",
        calls.len()
    );
    // A directory without a SKILL.md is somebody's notes, not a skill.
    let skills: Vec<&str> = docs
        .iter()
        .filter(|d| d.kind == "skill")
        .map(|d| d.name.as_str())
        .collect();
    assert_eq!(skills, vec!["tidy"], "{docs:#?}");
    // And the rules slots are all still answered, present and absent alike.
    assert!(docs.iter().any(|d| d.name == "CLAUDE.md" && d.kind == "rules"));

    // The folder picker, one step of a walk.
    h.reset_crossings();
    let listing = h.core.list_dir("wsl://TestOS", Some(&inner)).expect("list");
    let calls = h.crossings();
    assert!(calls.is_empty(), "one step cost {} crossings: {calls:#?}", calls.len());
    assert!(listing.is_repo, "the worktree did not read as a checkout");
    // The listing survives a directory whose *last* entry is a plain file.
    // The script ends in a `for` loop, so its own exit status used to be the
    // last `[ -d ]` test — and a folder ending in a file therefore answered
    // "cannot be opened". Latent until something looked into a directory
    // shaped like this one.
    assert!(
        listing.dirs.iter().any(|d| d == ".claude"),
        "the walk stopped early: {:?}",
        listing.dirs
    );
    assert!(
        !listing.dirs.iter().any(|d| d.ends_with(".txt")),
        "a plain file was offered as a directory: {:?}",
        listing.dirs
    );

    // Held, not re-opened. A pool that started a shell per call would answer
    // every assertion above and still be the thing this replaced.
    h.reset_crossings();
    for _ in 0..20 {
        h.core.attempt_stats(&a.attempt_id).expect("stats");
    }
    let calls = h.crossings();
    assert!(
        calls.is_empty(),
        "twenty reads opened {} shells: {calls:#?}",
        calls.len()
    );
}

/* -------------------------- session to session ------------------------- */

/// A tiny HTTP client, because the sender under test is a `curl` one-liner
/// and the thing worth checking is exactly what that one-liner would get
/// back — a status line and a plain-text body.
fn post(url: &str, headers: &[(&str, &str)], body: &str) -> (String, String) {
    use std::io::{Read as _, Write as _};
    let rest = url.trim_start_matches("http://");
    let (addr, path) = rest.split_once('/').expect("url has a path");
    let extra: String = headers
        .iter()
        .map(|(k, v)| format!("{k}: {v}\r\n"))
        .collect();
    let mut sock = std::net::TcpStream::connect(addr).expect("connect to the listener");
    let req = format!(
        "POST /{path} HTTP/1.1\r\nHost: localhost\r\n{extra}content-length: {}\r\n\r\n{body}",
        body.len()
    );
    sock.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    let _ = sock.read_to_string(&mut resp);
    let (head, body) = resp.split_once("\r\n\r\n").unwrap_or((resp.as_str(), ""));
    let status = head.lines().next().unwrap_or_default().to_string();
    (status, body.to_string())
}

fn get(url: &str) -> (String, String) {
    use std::io::{Read as _, Write as _};
    let rest = url.trim_start_matches("http://");
    let (addr, path) = rest.split_once('/').expect("url has a path");
    let mut sock = std::net::TcpStream::connect(addr).expect("connect to the listener");
    sock.write_all(format!("GET /{path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
        .unwrap();
    let mut resp = String::new();
    let _ = sock.read_to_string(&mut resp);
    let (head, body) = resp.split_once("\r\n\r\n").unwrap_or((resp.as_str(), ""));
    (head.lines().next().unwrap_or_default().to_string(), body.to_string())
}

/// The whole bridge, end to end and across CLIs: a codex session lists the
/// desk, finds a claude session by id, writes to it, and the message lands in
/// that session's terminal wearing the frame that says whose it is.
///
/// Cross-CLI is the point rather than a variation. Claude Code's own
/// cross-session messaging cannot do this at all — it is Claude Code's, and
/// per machine besides — so this is the case the desk's own channel exists
/// for.
#[test]
fn a_codex_session_can_message_a_claude_session_and_it_arrives_marked() {
    let h = Harness::new("bridge");
    let _guard = h.rt.enter();

    let t1 = h.card("Fix login", "make it work");
    let a1 = h.start(&t1, "claude");
    let claude = h.launches(&a1.session_id, 1);
    let t2 = h.card("Port the tests", "port them");
    let a2 = h.start(&t2, "codex");
    let codex = h.launches(&a2.session_id, 1);

    // Both were handed the two endpoints, each with its own token.
    assert!(!codex[0].send_url.is_empty(), "codex got no send endpoint");
    assert!(!claude[0].send_url.is_empty(), "claude got no send endpoint");
    assert_ne!(
        codex[0].send_url, claude[0].send_url,
        "two sessions were handed the same address"
    );

    // The codex session looks around. It sees the claude session and not
    // itself — an address list holding your own address invites a loop.
    let (status, listing) = get(&codex[0].peers_url);
    assert!(status.contains("200"), "peers refused: {status}");
    assert!(
        listing.contains(&a1.session_id),
        "the claude session is not on the list: {listing:?}"
    );
    assert!(
        !listing.contains(&a2.session_id),
        "a session was offered its own address: {listing:?}"
    );

    // And writes to it, addressing by the id the listing gave.
    let (status, reply) = post(
        &codex[0].send_url,
        &[("X-Marol-To", &a1.session_id)],
        "auth.py is mine — do not touch it",
    );
    assert!(status.contains("200"), "send refused: {status} {reply}");

    // It arrives in the claude session's terminal, framed.
    let stdin = h.stdin_when(&a1.session_id, |s| s.contains("do not touch it"));
    assert!(stdin.contains("[marol]"), "the message wore no frame: {stdin:?}");
    assert!(
        stdin.contains("Not from the person"),
        "the frame does not disclaim the person: {stdin:?}"
    );
    assert!(
        stdin.contains("Port the tests"),
        "the frame does not name the sender: {stdin:?}"
    );

    // And the record says who actually spoke. Filed as a `prompt` it would
    // have the timeline claim the person said this — the same lie the frame
    // stops in the terminal, told instead to whoever reads the record later.
    // Found by kind rather than by position: the `idle` that drains the
    // queue also writes a status row, both stamped the same millisecond from
    // different threads, so "the last row" is a coin toss.
    let rows = h.timeline(&a1.attempt_id, 2);
    let msg = rows
        .iter()
        .find(|r| r.kind == "message")
        .unwrap_or_else(|| panic!("no relayed-message row: {rows:?}"));
    assert_eq!(msg.kind, "message", "a relayed message was filed as: {msg:?}");
    // The sender's row name, which is the card title plus its attempt number
    // — the name the person actually sees in the sidebar, not the card's.
    assert_eq!(msg.tool.as_deref(), Some("Port the tests #1"), "{msg:?}");
    // The stored text is what was said, not how it travelled: the frame is
    // delivery, and a record of it would be a record of our own plumbing.
    assert_eq!(msg.detail.as_deref(), Some("auth.py is mine — do not touch it"));
    assert!(!msg.detail.as_deref().unwrap_or_default().contains("[marol]"));

    // The sender's own card says whom it is talking to, in the shape the
    // board already uses for Claude Code's native SendMessage.
    let sender = h
        .core
        .sessions()
        .into_iter()
        .find(|s| s.id == a2.session_id)
        .expect("the sending session");
    let act = sender.activity.expect("the sender reports no activity");
    assert_eq!(act.tool, "SendMessage");
    assert!(act.detail.starts_with("→ Fix login"), "{:?}", act.detail);
}

/// The brake on an unattended chain.
///
/// Two agents answering each other is the runaway `MAX_PENDING` cannot see:
/// neither queue ever holds more than the one message, so a pair could trade
/// turns until the app closed and never reach a depth of two. What runs away
/// is the chain, and every link in it is a whole agent turn somebody pays
/// for. So the count is the chain's length, and the way out of it is the
/// person — which is both what the refusal says and what clears it.
#[test]
fn a_relay_chain_stops_where_a_person_would_have_to_be_asked() {
    let h = Harness::new("relaycap");
    let _guard = h.rt.enter();

    let t1 = h.card("Fix login", "make it work");
    let a1 = h.start(&t1, "claude");
    let one = h.launches(&a1.session_id, 1);
    let t2 = h.card("Port the tests", "port them");
    let a2 = h.start(&t2, "codex");
    let two = h.launches(&a2.session_id, 1);

    // Eight relays, alternating. Each one is delivered before the next is
    // sent, because it is the delivery that tells the receiver how far from a
    // person it now stands.
    let mut sender_is_one = true;
    for hop in 1..=8 {
        let (url, to) = if sender_is_one {
            (&one[0].send_url, &a2.session_id)
        } else {
            (&two[0].send_url, &a1.session_id)
        };
        let text = format!("relay {hop}");
        let (status, reply) = post(url, &[("X-Marol-To", to.as_str())], &text);
        assert!(status.contains("200"), "hop {hop} was refused: {status} {reply}");
        h.stdin_when(to, |s| s.contains(&text));
        sender_is_one = !sender_is_one;
    }

    // The ninth is the one that would have run unattended, and it does not.
    let ninth = post(
        &one[0].send_url,
        &[("X-Marol-To", a2.session_id.as_str())],
        "relay 9",
    );
    assert!(ninth.0.contains("409"), "the ninth relay went through: {ninth:?}");
    // A refusal an agent cannot act on is a dropped message with extra steps.
    assert!(
        ninth.1.contains("ask the person"),
        "the refusal does not say what to do instead: {:?}",
        ninth.1
    );

    // And the way out is the person. Typing into the terminal is the
    // supervision the ceiling exists to require, so it clears the count —
    // the ninth relay was never wrong, only unwatched.
    h.core.write(&a1.session_id, "carry on\r").expect("type into the terminal");
    let after = post(
        &one[0].send_url,
        &[("X-Marol-To", a2.session_id.as_str())],
        "relay 9 again",
    );
    assert!(
        after.0.contains("200"),
        "a person spoke and the chain stayed shut: {after:?}"
    );
}

/// The relay's own delivery must not read as a person, or the brake could
/// never engage: each hop would clear the count it was supposed to raise.
#[test]
fn a_relayed_paste_does_not_pass_for_somebody_typing() {
    let h = Harness::new("relaypaste");
    let _guard = h.rt.enter();

    let t1 = h.card("Fix login", "make it work");
    let a1 = h.start(&t1, "claude");
    let one = h.launches(&a1.session_id, 1);
    let t2 = h.card("Port the tests", "port them");
    let a2 = h.start(&t2, "codex");
    let two = h.launches(&a2.session_id, 1);

    let (status, reply) = post(
        &one[0].send_url,
        &[("X-Marol-To", a2.session_id.as_str())],
        "one hop",
    );
    assert!(status.contains("200"), "send refused: {status} {reply}");
    h.stdin_when(&a2.session_id, |s| s.contains("one hop"));

    let depth = h
        .core
        .sessions()
        .into_iter()
        .find(|s| s.id == a2.session_id)
        .expect("the receiving session")
        .relay_hops;
    assert_eq!(depth, 1, "the paste that delivered a relay counted as a person");
}

/// A person's own follow-up is still the person's. The queue carries both
/// kinds now, and the row each one leaves has to say which it was.
#[test]
fn a_persons_followup_is_still_recorded_as_a_prompt() {
    let h = Harness::new("bridgemine");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");
    h.launches(&a.session_id, 1);

    h.core.queue_followup(&a.session_id, "one more thing").expect("queue");
    h.hook(&a.session_id, "idle", serde_json::json!({}));
    h.stdin_when(&a.session_id, |s| s.contains("one more thing"));

    let rows = h.timeline(&a.attempt_id, 2);
    let note = rows
        .iter()
        .find(|r| r.detail.as_deref() == Some("one more thing"))
        .unwrap_or_else(|| panic!("the note never reached the record: {rows:?}"));
    assert_eq!(note.kind, "prompt", "the person's own note was reattributed: {note:?}");
    assert_eq!(note.tool, None);
    assert!(
        !rows.iter().any(|r| r.kind == "message"),
        "a person's note was filed as a relay: {rows:?}"
    );
    // And it wears no envelope — it *is* the person.
    let stdin = h.stdin_when(&a.session_id, |s| s.contains("one more thing"));
    assert!(!stdin.contains("[marol]"), "a person's note was framed as a relay: {stdin:?}");
}

/// The token is what turns `sid` from a guess into an identity. Without the
/// check, any session on the desk could read a sibling's uuid and write to
/// the board on its behalf — this channel puts text into another agent, so
/// an address anything can guess is not enough to stand behind it.
#[test]
fn a_borrowed_address_without_its_token_is_refused() {
    let h = Harness::new("bridgetok");
    let _guard = h.rt.enter();
    let t1 = h.card("Fix login", "make it work");
    let a1 = h.start(&t1, "claude");
    h.launches(&a1.session_id, 1);
    let t2 = h.card("Port the tests", "port them");
    let a2 = h.start(&t2, "codex");
    let codex = h.launches(&a2.session_id, 1);

    let forged = codex[0]
        .send_url
        .split("&tok=")
        .next()
        .map(|head| format!("{head}&tok=00000000000000000000000000000000&send=1"))
        .expect("the endpoint carries a token at all");
    let (status, body) = post(&forged, &[("X-Marol-To", &a1.session_id)], "let me in");
    assert!(status.contains("409"), "a forged token was accepted: {status}");
    assert!(body.contains("token"), "the refusal does not say why: {body:?}");

    // And nothing reached the terminal.
    std::thread::sleep(Duration::from_millis(300));
    let stdin = h.stdin_when(&a1.session_id, |_| true);
    assert!(!stdin.contains("let me in"), "a forged message got through: {stdin:?}");
}

/// A refusal is an answer, not a silence: the sender is an agent that can act
/// on it. Every way a send can fail names itself.
#[test]
fn every_refused_send_hands_back_a_reason() {
    let h = Harness::new("bridgewhy");
    let _guard = h.rt.enter();
    let t1 = h.card("Fix login", "make it work");
    let a1 = h.start(&t1, "claude");
    let claude = h.launches(&a1.session_id, 1);

    // A session that is not here.
    let (status, body) = post(
        &claude[0].send_url,
        &[("X-Marol-To", "11111111-2222-3333-4444-555555555555")],
        "hello?",
    );
    assert!(status.contains("409"), "{status}");
    assert!(body.contains("no session here"), "{body:?}");

    // Itself. A session talking to itself is a loop with an extra step.
    let (status, body) = post(&claude[0].send_url, &[("X-Marol-To", &a1.session_id)], "hi me");
    assert!(status.contains("409"), "{status}");
    assert!(body.contains("cannot message itself"), "{body:?}");
}

/* ------------------------------ WSL bridge ----------------------------- */

/// M10a end to end, through a stand-in wsl.exe: a `wsl://` card's worktree is
/// created inside the distro under the distro's own home, the agent launches
/// there with its argv intact and its session identity carried across the
/// boundary, the diff reads back through the host, and closing returns the
/// tree. Everything the app does with WSL, minus the real wsl.exe's quirks.
#[test]
fn a_wsl_repository_runs_its_whole_attempt_inside_the_distro() {
    let h = Harness::new("wsl");
    let _guard = h.rt.enter();

    let repo_url = format!("wsl://TestOS{}", h.repo.display());
    let task = h
        .core
        .create_task(
            "修好登入".into(),
            "make it work".into(),
            repo_url,
            "main".into(),
            Vec::new(),
        )
        .expect("a wsl:// repository must be checkable through the doorway");

    let a = h.start(&task, "claude");

    // Stored in the app's path space, so every later reader knows the host…
    assert!(
        a.worktree_path.starts_with("wsl://TestOS/"),
        "{}",
        a.worktree_path
    );
    let inner = a.worktree_path.strip_prefix("wsl://TestOS").unwrap();
    // …and living under the distro's own home, never the app machine's root.
    assert!(
        inner.starts_with(&format!("{}/.marol/worktrees", h.root.display())),
        "the worktree left the distro: {inner}"
    );
    assert!(Path::new(inner).is_dir(), "the worktree was never created");

    // The agent is running inside: right directory, prompt still the last
    // argv entry — and the launch record existing at all proves
    // MAROL_SESSION_ID crossed the boundary, because the stub names its
    // log file after it.
    let launch = h.launches(&a.session_id, 1).pop().unwrap();
    assert_eq!(
        std::fs::canonicalize(&launch.cwd).unwrap(),
        std::fs::canonicalize(inner).unwrap()
    );
    assert_eq!(launch.args.last(), Some(&a.prompt));
    // The distro's claude answered the version probe, so the session still
    // gets its card's name for cross-session messaging.
    assert!(
        launch
            .args
            .windows(2)
            .any(|w| w[0] == "--name" && w[1] == "修好登入 #1"),
        "{:?}",
        launch.args
    );

    // The diff reads through the host, freezes on close, and the tree goes
    // back to the distro.
    std::fs::write(Path::new(inner).join("app.txt"), "fixed\n").unwrap();
    assert!(h.core.attempt_diff(&a.attempt_id).unwrap().contains("fixed"));
    h.core.finish_attempt(&a.attempt_id, Outcome::Discarded).unwrap();
    assert!(!Path::new(inner).exists(), "the worktree outlived its attempt");
    assert!(
        h.core.attempt_diff(&a.attempt_id).unwrap().contains("fixed"),
        "the frozen diff was lost with the worktree"
    );
}

/// A session in another world survives the app too, and the next start finds
/// it there.
///
/// The half of persistence that was missing. Holding was built on `-L`, which
/// asks tmux where its own socket directory is — a question only this machine
/// can answer. In a distro the answer depends on a uid and a profile this side
/// cannot see, so the app names the path instead and tells tmux. Everything
/// downstream follows from that one change: the config has to be written into
/// the world, the sweep has to read the world's directory, and the "is it
/// alive" question has to travel through the doorway like every other command.
///
/// The stand-in wsl.exe shares this machine's filesystem, so the test can look
/// at what the app put in the distro's home and check tmux is really holding
/// it. What it cannot vouch for is the real wsl.exe; that is what CI's Windows
/// leg and a person's own machine are for.
#[test]
fn a_session_in_another_world_is_held_there_and_found_again() {
    if std::process::Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("no tmux on PATH — nothing holds sessions here");
        return;
    }
    // A deliberately long root, because that is the bug this found. The
    // socket used to live under the world's home, and a home is unbounded: a
    // macOS runner's temp home put the address at 135 bytes against a limit
    // of 104, and every session in that world failed to start with the
    // message going to a pty that closed with it. A name this long reproduces
    // it; the socket now lives in `/tmp`, so the home's length is irrelevant.
    let h = Harness::new("wsl-hold-in-a-world-with-a-very-long-home-directory");
    let _guard = h.rt.enter();
    assert!(
        h.root.to_string_lossy().len() > 45,
        "this test is only a test while its home is long: {}",
        h.root.display()
    );
    let repo_url = format!("wsl://TestOS{}", h.repo.display());
    let task = h
        .core
        .create_task(
            "修好登入".into(),
            "make it work".into(),
            repo_url,
            "main".into(),
            Vec::new(),
        )
        .expect("a wsl:// repository must be checkable through the doorway");
    let a = h.start(&task, "claude");
    h.launches(&a.session_id, 1);

    // The config went into the distro, not onto the app's disk. Without it
    // tmux starts on its defaults and draws a status line over the agent's
    // terminal — and it does not complain about a `-f` that is not there, so
    // this is a failure that would only ever show up as a stripe at the
    // bottom of somebody's screen.
    let conf = h.root.join(".marol").join("tmux.conf");
    assert!(
        std::fs::read_to_string(&conf)
            .unwrap_or_default()
            .contains("status off"),
        "no tmux config in the distro at {}",
        conf.display()
    );

    // And the socket landed in the app's own directory inside the world,
    // where a `-L` would have gone looking in tmux's instead — and *not*
    // under the home, whose length is nobody's to promise.
    let held = held_socket(&a.session_id).expect("the world is not holding the session");
    let sock = held.to_string_lossy().to_string();
    assert!(
        !sock.starts_with(&*h.root.to_string_lossy()),
        "the socket went back under the home, so its length is the home's: {sock}"
    );
    assert!(
        sock.len() < 104,
        "a {} byte socket path will not fit in a sockaddr_un: {sock}",
        sock.len()
    );
    // Not the tag this desk uses at home. Over there the data directory alone
    // does not identify a desk — two laptops belonging to one person have the
    // same path, and if they both reached this host they would agree on a tag
    // and then sweep each other's running agents. What tells them apart is
    // written once, here.
    let name = held.file_name().unwrap().to_string_lossy().into_owned();
    let tag = name.strip_suffix(&format!("-{}", a.session_id)).unwrap();
    assert_ne!(tag, pty::desk_tag(&h.root.join("data").to_string_lossy()));
    assert!(
        h.root.join("data").join("machine-id").exists(),
        "nothing was written down to tell this machine from another"
    );
    assert!(
        std::process::Command::new("tmux")
            .args(["-S", &sock, "has-session", "-t", "agent"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
        "tmux is not holding the session at {sock}"
    );

    // Quitting drops the client. The agent stays.
    h.core.shutdown();
    let core2 = h
        .rt
        .block_on(Core::start_with(
            h.env.clone(),
            Arc::new(Events::default()) as Arc<dyn UiSink>,
            h.root.join("marol.db"),
            h.root.join("data"),
            h.root.join("worktrees"),
        ))
        .expect("second core");

    // Off the first paint, so it is answered on a thread — the board must not
    // wait on a distro, still less on an SSH host. It arrives shortly after.
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        let s = core2
            .sessions()
            .into_iter()
            .find(|s| s.id == a.session_id)
            .expect("the session is still on the list");
        if s.status != Status::Saved || Instant::now() > deadline {
            break s.status;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(
        status,
        Status::Detached,
        "a session the distro's tmux kept running came back as {status:?}",
    );

    // Attaching reaches the same agent, and does not claim it is starting:
    // `new-session -A -D` drops the argv, so no SessionStart will ever fire.
    core2
        .reopen_session(&a.session_id, 100, 30)
        .expect("reattach through the doorway");
    let after = core2
        .sessions()
        .into_iter()
        .find(|s| s.id == a.session_id)
        .expect("still on the list");
    assert_eq!(after.status, Status::Detached, "running, and not yet heard from");
    assert!(after.live, "a terminal in this process carries it now");
    core2.shutdown();
}

/// A world without tmux keeps exactly the behaviour it always had.
///
/// Not an edge case: a fresh Ubuntu on WSL has no tmux, so this is what most
/// distros are on the day this ships. Persistence is a property of a world,
/// and a world that hasn't got it must lose nothing — the session still opens,
/// still runs, still carries its identity across the boundary. What it must
/// not do is half-hold: a `tmux` that is not there cannot be asked to keep
/// anything, and a socket path invented for it would be a promise to come back
/// for something nobody is holding.
#[test]
fn a_world_without_tmux_still_runs_its_sessions() {
    let h = Harness::new("wsl-no-tmux");
    let _guard = h.rt.enter();
    // A tmux on the world's PATH that answers nothing. Findable, so this is
    // the harder case than an absent one: only actually asking finds out.
    let broken = h.root.join("bin").join("tmux");
    std::fs::write(&broken, "#!/bin/bash\nexit 127\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let repo_url = format!("wsl://TestOS{}", h.repo.display());
    let task = h
        .core
        .create_task(
            "修好登入".into(),
            "make it work".into(),
            repo_url,
            "main".into(),
            Vec::new(),
        )
        .expect("a wsl:// repository is checkable without tmux");
    let a = h.start(&task, "claude");

    // The agent is running, in the right place, with its argv intact.
    let launch = h.launches(&a.session_id, 1).pop().unwrap();
    assert_eq!(launch.args.last(), Some(&a.prompt));
    assert!(
        !launch.args.contains(&"new-session".to_string()),
        "tmux got into the command line of a world that has none: {:?}",
        launch.args
    );
    assert!(
        !h.root.join(".marol").join("s").exists(),
        "a socket directory in a world that holds nothing"
    );
}

/// The sweep reaches into the world, and stops at its edge.
///
/// A held session whose card is gone is an agent nobody will look at again and
/// nothing can name — the reason the sweep exists. Over there it also has to
/// take the socket file with it, because there is no second visit: this
/// process cannot reach that filesystem, and on the next run a dead socket and
/// a live one look exactly alike.
///
/// The other half is what it must *not* touch: a socket wearing another
/// desk's tag. Two laptops with the same username have the same data
/// directory, and if both reach one host they would agree on a tag unless
/// something told them apart — at which point one desk's tidying kills the
/// other's running work, silently.
#[test]
fn the_sweep_ends_forgotten_sessions_in_a_world_and_leaves_other_desks_alone() {
    if std::process::Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("no tmux on PATH — nothing holds sessions here");
        return;
    }
    let h = Harness::new("wsl-sweep");
    let _guard = h.rt.enter();
    let repo_url = format!("wsl://TestOS{}", h.repo.display());
    let task = h
        .core
        .create_task(
            "修好登入".into(),
            "make it work".into(),
            repo_url,
            "main".into(),
            Vec::new(),
        )
        .expect("a wsl:// repository is checkable");
    let a = h.start(&task, "claude");
    h.launches(&a.session_id, 1);

    let socks = world_socket_dir();
    let mine = held_socket(&a.session_id).expect("the session was held");
    // The name is `<desk tag>-<session id>`, and the id is what we came with.
    let tag = mine
        .file_name()
        .unwrap()
        .to_string_lossy()
        .strip_suffix(&format!("-{}", a.session_id))
        .expect("the socket is named for its session")
        .to_string();
    assert!(!tag.is_empty(), "{}", mine.display());

    // Two more servers in the same directory: one this desk left behind and
    // forgot, one belonging to a different desk entirely.
    let orphan = socks.join(format!("{tag}-{}", uuid::Uuid::new_v4()));
    let stranger = socks.join(format!("beef1234-{}", uuid::Uuid::new_v4()));
    for p in [&orphan, &stranger] {
        assert!(
            std::process::Command::new("tmux")
                .args([
                    "-S",
                    &p.to_string_lossy(),
                    "new-session",
                    "-d",
                    "-s",
                    "agent",
                    "--",
                    "sleep",
                    "300",
                ])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            "could not stage {}",
            p.display()
        );
    }

    h.core.shutdown();
    let core2 = h
        .rt
        .block_on(Core::start_with(
            h.env.clone(),
            Arc::new(Events::default()) as Arc<dyn UiSink>,
            h.root.join("marol.db"),
            h.root.join("data"),
            h.root.join("worktrees"),
        ))
        .expect("second core");

    let alive = |p: &PathBuf| {
        std::process::Command::new("tmux")
            .args(["-S", &p.to_string_lossy(), "has-session", "-t", "agent"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    while alive(&orphan) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!alive(&orphan), "a held session with no card left is still running");
    assert!(
        !orphan.exists(),
        "the socket file outlived its server, and nothing over there will ever \
         remove it — the next sweep cannot tell it from a live one"
    );
    assert!(
        alive(&stranger),
        "the sweep killed another desk's running agent"
    );
    assert!(stranger.exists());

    // That directory belongs to the user, not to this harness, and nothing
    // else will come back for what the test left in it.
    let _ = std::process::Command::new("tmux")
        .args(["-S", &stranger.to_string_lossy(), "kill-server"])
        .output();
    let _ = std::fs::remove_file(&stranger);
    let _ = std::fs::remove_file(&orphan);
    core2.shutdown();
}

/// A host nobody can reach fails at first contact, in the dialog, with the
/// probe's own words — never as a phantom "no such directory".
#[test]
fn an_unreachable_ssh_host_fails_the_card_with_the_probes_reason() {
    if std::env::var("MAROL_SSH_TEST").is_err() {
        eprintln!("skipping: set MAROL_SSH_TEST=1 to run the ssh tests");
        return;
    }
    let h = Harness::new("sshghost");
    let _guard = h.rt.enter();
    let err = h
        .core
        .create_task(
            "x".into(),
            "y".into(),
            "ssh://marol-no-such-host/home/me/app".into(),
            "main".into(),
            Vec::new(),
        )
        .expect_err("an unreachable host cannot back a card");
    assert!(
        err.to_string().contains("marol-no-such-host"),
        "the error must name the host: {err}"
    );
}

/* ------------------------------- SSH host ------------------------------ */

/// A real sshd on a loopback port, a real ssh steered through a private
/// config by a wrapper on the harness PATH, and this machine standing in for
/// the remote. Everything is real — the login-shell probe, multiplexing, the
/// reverse tunnel, remote plugin provisioning — except the distance.
struct SshFixture {
    sshd: std::process::Child,
    /// The stub agent we wrote onto the remote login PATH, to remove after.
    stub: Option<PathBuf>,
    /// Whether the stub answers to `claude` there (full assertions), or only
    /// to `codex` (mechanics only — a real claude was shadowing the name).
    claude_stubbed: bool,
}

impl SshFixture {
    /// `None` skips the test: no sshd on this machine, or no way to put the
    /// stub on the remote login PATH without touching anything real.
    fn start(h: &Harness) -> Option<Self> {
        let sshd_bin = Path::new("/usr/sbin/sshd");
        if !sshd_bin.exists() {
            eprintln!("skipping: /usr/sbin/sshd is not installed");
            return None;
        }
        let dir = h.root.join("sshd");
        std::fs::create_dir_all(&dir).unwrap();
        let keygen = |path: &Path| {
            std::process::Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(path)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        if !keygen(&dir.join("hostkey")) || !keygen(&dir.join("userkey")) {
            eprintln!("skipping: ssh-keygen unavailable");
            return None;
        }
        std::fs::copy(dir.join("userkey.pub"), dir.join("authorized_keys")).unwrap();

        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let user = String::from_utf8_lossy(
            &std::process::Command::new("id").arg("-un").output().unwrap().stdout,
        )
        .trim()
        .to_string();

        std::fs::write(
            dir.join("sshd_config"),
            format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {hk}\nPidFile {pid}\n\
                 AuthorizedKeysFile {ak}\nStrictModes no\nUsePAM no\n\
                 PasswordAuthentication no\nPermitRootLogin prohibit-password\n\
                 AllowTcpForwarding yes\n",
                hk = dir.join("hostkey").display(),
                pid = dir.join("sshd.pid").display(),
                ak = dir.join("authorized_keys").display(),
            ),
        )
        .unwrap();

        let sshd = std::process::Command::new(sshd_bin)
            .args(["-f"])
            .arg(dir.join("sshd_config"))
            .args(["-D", "-e"])
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let up = wait_for(Duration::from_secs(5), || {
            std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
        });
        if !up {
            eprintln!("skipping: sshd never came up");
            return None;
        }

        // The client side: a private config, reached through a wrapper `ssh`
        // that the harness PATH puts in front of the real one — the same
        // trick as the stand-in wsl.exe, except everything behind it is real.
        std::fs::write(
            dir.join("ssh_config"),
            format!(
                "Host marol-test\n  HostName 127.0.0.1\n  Port {port}\n  User {user}\n\
                 \x20 IdentityFile {ik}\n  IdentitiesOnly yes\n  StrictHostKeyChecking no\n\
                 \x20 UserKnownHostsFile /dev/null\n  LogLevel ERROR\n",
                ik = dir.join("userkey").display(),
            ),
        )
        .unwrap();
        let wrapper = h.root.join("bin").join("ssh");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/bash\nexec /usr/bin/ssh -F {} \"$@\"\n",
                dir.join("ssh_config").display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        // A stub agent on the REMOTE login PATH — which is this machine's
        // real one, so nothing real may be touched: only a free name in a
        // writable standard directory, removed afterwards. `claude` when the
        // name is free, `codex` when a real claude is shadowing it.
        let logs = h.root.join("logs");
        let remote_stub = |name: &str| -> Option<PathBuf> {
            for cand in ["/usr/local/bin", "/usr/bin"] {
                let path = Path::new(cand).join(name);
                if path.exists() {
                    continue; // never clobber anything real
                }
                let body = format!(
                    "#!/bin/bash\nif [ \"$1\" = \"--version\" ]; then echo \"2.1.226 (Claude Code)\"; exit 0; fi\n\
                     printf '%s\\0' \"$PWD\" \"${{MAROL_NAME_URL:-}}\" \"${{MAROL_PEERS_URL:-}}\" \"${{MAROL_SEND_URL:-}}\" \"$@\" > \"{logs}/${{MAROL_SESSION_ID:-unknown}}.$$\"\n\
                     exec cat > \"{logs}/stdin.${{MAROL_SESSION_ID:-unknown}}.$$\"\n",
                    logs = logs.display()
                );
                if std::fs::write(&path, &body).is_err() {
                    continue; // not writable here; try the next
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
                }
                // Exact match through the remote login shell, or it does not
                // count: a real claude earlier on the PATH must never be the
                // thing a test launches.
                let seen = std::process::Command::new(&wrapper)
                    .args(["marol-test", &format!("$SHELL -lc 'command -v {name}'")])
                    .output()
                    .ok();
                let found = seen
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim() == path.to_string_lossy())
                    .unwrap_or(false);
                if found {
                    return Some(path);
                }
                let _ = std::fs::remove_file(&path);
            }
            None
        };

        let (stub, claude_stubbed) = match remote_stub("claude") {
            Some(p) => (Some(p), true),
            None => (remote_stub("codex"), false),
        };
        if stub.is_none() {
            eprintln!("skipping: no writable directory on the remote login PATH for the stub");
            sshd_cleanup(&mut Some(sshd));
            return None;
        }

        Some(Self {
            sshd,
            stub,
            claude_stubbed,
        })
    }
}

fn sshd_cleanup(child: &mut Option<std::process::Child>) {
    if let Some(mut c) = child.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

impl Drop for SshFixture {
    fn drop(&mut self) {
        let _ = self.sshd.kill();
        let _ = self.sshd.wait();
        if let Some(p) = &self.stub {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// M10b end to end against a real sshd: the card checks out, the worktree
/// opens in the remote home, the agent launches through a forced-tty ssh with
/// its argv armoured and its identity across, the diff reads back, the hook
/// plugin is provisioned remotely with a tunnel URL, and closing returns the
/// tree. Gated: set MAROL_SSH_TEST=1 (CI does).
#[test]
fn an_ssh_repository_runs_its_whole_attempt_on_the_remote() {
    if std::env::var("MAROL_SSH_TEST").is_err() {
        eprintln!("skipping: set MAROL_SSH_TEST=1 to run the ssh tests");
        return;
    }
    let h = Harness::new("sshfull");
    let _guard = h.rt.enter();
    let Some(fx) = SshFixture::start(&h) else {
        return;
    };

    let repo_url = format!("ssh://marol-test{}", h.repo.display());
    let task = h
        .core
        .create_task(
            "修好登入".into(),
            "make it work".into(),
            repo_url,
            "main".into(),
            Vec::new(),
        )
        .expect("an ssh:// repository must be checkable over the wire");

    let agent = if fx.claude_stubbed { "claude" } else { "codex" };
    let a = h.start(&task, agent);

    // Remote paths, remote home.
    assert!(a.worktree_path.starts_with("ssh://marol-test/"), "{}", a.worktree_path);
    let inner = a.worktree_path.strip_prefix("ssh://marol-test").unwrap();
    let home = dirs::home_dir().unwrap();
    assert!(
        inner.starts_with(&format!("{}/.marol/worktrees", home.display())),
        "the worktree left the remote home: {inner}"
    );
    assert!(Path::new(inner).is_dir());

    // The agent runs there: right cwd, identity across the wire (the launch
    // record's very name is the session id), and for a claude stub the
    // prompt arrived whole as the last word of an armoured command line.
    let launch = h.launches(&a.session_id, 1).pop().unwrap();
    assert_eq!(
        std::fs::canonicalize(&launch.cwd).unwrap(),
        std::fs::canonicalize(inner).unwrap()
    );
    if fx.claude_stubbed {
        assert_eq!(launch.args.last(), Some(&a.prompt));
        assert!(a.prompt.contains('\n'), "the prompt under test must be multi-line");
        assert!(
            launch.args.windows(2).any(|w| w[0] == "--name" && w[1] == "修好登入 #1"),
            "{:?}",
            launch.args
        );
    }

    // The hook plugin was provisioned into the remote home, its URL pointing
    // back through the reverse tunnel — never at the app's own listener
    // address, which means nothing on the remote.
    let hooks_file = home.join(".marol/plugin/hooks/hooks.json");
    let hooks_text = std::fs::read_to_string(&hooks_file).expect("remote plugin missing");
    assert!(hooks_text.contains("http://127.0.0.1:"), "{hooks_text}");

    // And that port was written down. Claude Code reads hooks.json once, so
    // an agent this host holds through a restart keeps reporting to whatever
    // address is in that file for the rest of its life; the next run has to
    // ask for the same port back rather than draw a new one.
    let port = hooks_text
        .split("http://127.0.0.1:")
        .nth(1)
        .and_then(|s| s.split('/').next())
        .expect("no port in the hook url")
        .to_string();
    let tunnels = std::fs::read_to_string(h.root.join("data").join("tunnels"))
        .expect("the tunnel port was not remembered");
    assert!(
        tunnels.contains(&format!("marol-test\t{port}")),
        "remembered {tunnels:?}, but the plugin was told {port}"
    );

    // The remote is holding the session. Same mechanism as WSL — a socket the
    // app named, in a directory it made, inside the world — which is the only
    // shape that works when this side cannot see the world's tmux directory.
    let held = held_socket(&a.session_id).expect("the remote is not holding the session");
    assert!(
        std::process::Command::new("tmux")
            .args(["-S", &held.to_string_lossy(), "has-session", "-t", "agent"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
        "nothing is answering on {}",
        held.display()
    );

    // Diff over the wire; close returns the tree and keeps the evidence.
    std::fs::write(Path::new(inner).join("app.txt"), "fixed over ssh\n").unwrap();
    assert!(h.core.attempt_diff(&a.attempt_id).unwrap().contains("fixed over ssh"));
    h.core.finish_attempt(&a.attempt_id, Outcome::Discarded).unwrap();
    assert!(!Path::new(inner).exists());
    assert!(h.core.attempt_diff(&a.attempt_id).unwrap().contains("fixed over ssh"));

    // Tidy the remote worktree directory this repo was given, and the server
    // holding the session — the "remote" here shares this machine's home, so
    // nothing else is going to.
    if let Some(parent) = Path::new(inner).parent() {
        let _ = std::fs::remove_dir_all(parent);
    }
    let _ = std::process::Command::new("tmux")
        .args(["-S", &held.to_string_lossy(), "kill-server"])
        .output();
    let _ = std::fs::remove_file(&held);
}

/* ----------------------- cross-session messaging ----------------------- */

/// Claude Code's cross-session messaging addresses a session by name, and
/// left to itself the CLI derives one from the directory — a worktree slug
/// with a counter. Marol knows the card, so the session answers to what
/// the board calls it, and one card's agent can message another's by the
/// title a person would actually say.
#[test]
fn a_claude_session_is_named_after_its_card_for_messaging() {
    let h = Harness::new("msgname");
    let _guard = h.rt.enter();
    let task = h.card("修好登入", "make it work");

    let a = h.start(&task, "claude");
    let args = h.args_of(&a.session_id);
    let named = args
        .windows(2)
        .any(|w| w[0] == "--name" && w[1] == "修好登入 #1");
    assert!(named, "the session did not get its card's name: {args:?}");
    assert_eq!(args.last(), Some(&a.prompt), "the prompt must stay last");

    // An ad-hoc session answers to its directory's name, same as its title.
    let id = h
        .core
        .new_session(h.repo.to_string_lossy().into(), "claude".into(), vec![], 100, 30)
        .unwrap();
    let args = h.args_of(&id);
    assert!(
        args.windows(2).any(|w| w[0] == "--name" && w[1] == "repo"),
        "{args:?}"
    );
}

/// The gate that keeps this safe: a claude from before session names existed
/// is never handed `--name` — the flag would stop it starting at all.
#[test]
fn an_older_claude_is_not_handed_the_name_flag() {
    let h = Harness::with_claude_stub("msgold", OLD_STUB);
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");

    let a = h.start(&task, "claude");
    let args = h.args_of(&a.session_id);
    assert!(
        !args.iter().any(|x| x == "--name"),
        "an unmeasured flag was handed to an older CLI: {args:?}"
    );
    // Everything else about the session is untouched by the gate.
    assert_eq!(args.last(), Some(&a.prompt));
    assert!(args.iter().any(|x| x == "--plugin-dir"));
}

/* --------------------------- naming a session -------------------------- */

/// The name on the row is the person's to change.
///
/// A session opened without a card can only be called after its directory,
/// and that is frequently the same directory as the session beside it. The
/// rename is the way out, and it has to survive a restart or it is a label
/// rather than a name.
#[test]
fn a_session_can_be_renamed_and_the_name_outlives_the_desk() {
    let h = Harness::new("rename");
    let _guard = h.rt.enter();
    let id = h
        .core
        .new_session(h.repo.to_string_lossy().into(), "claude".into(), vec![], 100, 30)
        .unwrap();
    assert_eq!(h.session(&id).title, "repo");
    // Waited for on purpose, and not only to read the name off the command
    // line: closing a session in the microsecond before its holder has
    // finished coming up is a race with tmux, not with this feature.
    let first = h.args_of(&id);
    assert!(first.windows(2).any(|w| w[0] == "--name" && w[1] == "repo"), "{first:?}");

    h.core.rename_session(&id, "  改登入導向\n ").unwrap();
    // Repaired rather than refused: the surrounding whitespace goes, the
    // name stays.
    assert_eq!(h.session(&id).title, "改登入導向");

    // Nothing is not a name. The old one is kept rather than blanked, which
    // would leave a row nobody could pick out.
    assert!(h.core.rename_session(&id, "   ").is_err());
    assert_eq!(h.session(&id).title, "改登入導向");

    // The name the CLI answers to is fixed on a running command line, so the
    // rename reaches it at the session's next start — the one claim about
    // this that could quietly stop being true. Closing tears the holder down
    // too, so reopening is a start rather than a reattach.
    h.core.close_session(&id).unwrap();
    h.wait_unheld(&id);
    h.core.reopen_session(&id, 100, 30).unwrap();
    let again = h.launches(&id, 2).pop().unwrap().args;
    assert!(
        again.windows(2).any(|w| w[0] == "--name" && w[1] == "改登入導向"),
        "the second start still answered to the old name: {again:?}"
    );

    h.core.shutdown();
    let core2 = h
        .rt
        .block_on(Core::start_with(
            h.env.clone(),
            Arc::new(Events::default()) as Arc<dyn UiSink>,
            h.root.join("marol.db"),
            h.root.join("data"),
            h.root.join("worktrees"),
        ))
        .expect("second core");
    let restored = core2
        .sessions()
        .into_iter()
        .find(|s| s.id == id)
        .expect("the session is still on the list");
    assert_eq!(restored.title, "改登入導向", "the rename did not reach the disk");
}

/// The other half of the same fact: the agent in the session can set it.
///
/// It goes through the listener the status hooks already use, addressed by
/// the one variable the session was launched with — so an agent that has
/// worked out what it is actually doing can say so on the board without a
/// person typing it in.
#[test]
fn an_agent_names_its_own_session_through_the_endpoint_it_was_given() {
    let h = Harness::new("selfname");
    let _guard = h.rt.enter();
    let task = h.card("修好登入", "make it work");
    let a = h.start(&task, "claude");
    assert_eq!(h.session(&a.session_id).title, "修好登入 #1");

    // The address the session was actually handed, read back out of the
    // environment the process got rather than reconstructed here.
    let url = h.launches(&a.session_id, 1).pop().unwrap().name_url;
    assert!(!url.is_empty(), "the session was launched without an address to name itself at");
    assert!(url.contains(&format!("sid={}", a.session_id)), "{url}");

    post_name(&url, "改 session 端的導向\n");
    assert_eq!(h.wait_for_title(&a.session_id, "改 session 端的導向"), "改 session 端的導向");

    // And it reaches the disk on the same terms a person's rename does.
    let stored = h
        .core
        .sessions()
        .into_iter()
        .find(|s| s.id == a.session_id)
        .unwrap();
    assert_eq!(stored.title, "改 session 端的導向");

    // A name for a session this desk does not have is dropped rather than
    // landing on a neighbour.
    let stray = url.replace(&a.session_id, "00000000-0000-0000-0000-000000000000");
    post_name(&stray, "somebody else's");
    assert_eq!(h.session(&a.session_id).title, "改 session 端的導向");
}

/// Three terminals in one checkout is the ordinary thing to do here, and it
/// used to produce three rows saying the same word — on screen, and in the
/// `--name` each one answers to for messages from the others.
#[test]
fn ad_hoc_sessions_in_one_directory_do_not_all_answer_to_one_name() {
    let h = Harness::new("dupname");
    let _guard = h.rt.enter();
    let cwd: String = h.repo.to_string_lossy().into();
    let ids: Vec<String> = (0..3)
        .map(|_| {
            h.core
                .new_session(cwd.clone(), "claude".into(), vec![], 100, 30)
                .unwrap()
        })
        .collect();

    let titles: Vec<String> = ids.iter().map(|id| h.session(id).title).collect();
    assert_eq!(titles, vec!["repo", "repo 2", "repo 3"]);

    // The counter is not decoration: it is what the CLI answers to, so two
    // sessions can address each other at all.
    for (id, title) in ids.iter().zip(&titles) {
        let args = h.args_of(id);
        assert!(
            args.windows(2).any(|w| w[0] == "--name" && &w[1] == title),
            "{title} is not the name it launched with: {args:?}"
        );
    }
}

/* ------------------------------ profiles ------------------------------- */

/// A profile is a name for "this CLI, with these flags, every time". Picking
/// it launches the real agent with the standing arguments first — and the
/// prompt still last — while the attempt records the CLI underneath, so
/// delivery, hooks and resume all behave by what actually ran.
#[test]
fn a_profile_launches_its_agent_with_its_standing_arguments() {
    let h = Harness::new("profile");
    let _guard = h.rt.enter();
    h.core
        .set_profiles(vec![crate::store::Profile {
            name: "opus 版".into(),
            agent: "claude".into(),
            args: vec!["--model".into(), "opus".into()],
        }])
        .unwrap();
    let task = h.card("Fix login", "make it work");

    let opened = h
        .core
        .start_attempt(&task, "opus 版".into(), None, PermissionMode::Normal, 100, 30)
        .unwrap()
        .attempt
        .unwrap();

    let args = h.args_of(&opened.session_id);
    let pair = args.windows(2).any(|w| w[0] == "--model" && w[1] == "opus");
    assert!(pair, "the profile's arguments never arrived: {args:?}");
    assert_eq!(args.last(), Some(&opened.prompt), "the prompt must stay last");
    assert!(
        args.iter().any(|a| a == "--plugin-dir"),
        "a claude profile still reports status: {args:?}"
    );
    assert!(opened.prompt_sent, "a claude profile still sends the prompt");

    // The record names the CLI, not the nickname.
    assert_eq!(h.core.task_board()[0].attempts[0].attempt.agent, "claude");
}

#[test]
fn an_ad_hoc_session_can_start_from_a_profile_and_own_args_come_after() {
    let h = Harness::new("profileadhoc");
    let _guard = h.rt.enter();
    h.core
        .set_profiles(vec![crate::store::Profile {
            name: "opus 版".into(),
            agent: "claude".into(),
            args: vec!["--model".into(), "opus".into()],
        }])
        .unwrap();

    let id = h
        .core
        .new_session(
            h.repo.to_string_lossy().into(),
            "opus 版".into(),
            vec!["--verbose".into()],
            100,
            30,
        )
        .unwrap();

    let args = h.args_of(&id);
    let model = args.iter().position(|a| a == "--model").expect("profile args present");
    let verbose = args.iter().position(|a| a == "--verbose").expect("own args present");
    assert!(model < verbose, "the person's own arguments must come after, so they can override: {args:?}");
    // The row remembers the resolved CLI, so reopening runs `claude`.
    let session = h.core.sessions().into_iter().find(|s| s.id == id).unwrap();
    assert_eq!(session.agent, "claude");
}

/// The queue stores the profile's *name*; what runs is whatever the profile
/// says when the slot finally frees.
#[test]
fn a_queued_start_resolves_its_profile_when_its_turn_comes() {
    let h = Harness::new("profilequeue");
    let _guard = h.rt.enter();
    h.core
        .set_profiles(vec![crate::store::Profile {
            name: "opus 版".into(),
            agent: "claude".into(),
            args: vec!["--model".into(), "opus".into()],
        }])
        .unwrap();
    h.core.set_max_concurrent(1).unwrap();
    let first = h.card("First", "p");
    let second = h.card("Second", "p");

    let a = h.start(&first, "claude");
    h.core
        .start_attempt(&second, "opus 版".into(), None, PermissionMode::Normal, 100, 30)
        .unwrap();
    h.core.close_session(&a.session_id).unwrap();

    let started = wait_for(Duration::from_secs(10), || {
        h.core
            .task_board()
            .into_iter()
            .find(|t| t.task.id == second)
            .map(|t| !t.attempts.is_empty())
            .unwrap_or(false)
    });
    assert!(started, "the queue never moved");
    let view = h
        .core
        .task_board()
        .into_iter()
        .find(|t| t.task.id == second)
        .unwrap();
    assert_eq!(view.attempts[0].attempt.agent, "claude");
    let session = view.attempts[0].session_id.clone().unwrap();
    let args = h.args_of(&session);
    assert!(args.windows(2).any(|w| w[0] == "--model" && w[1] == "opus"), "{args:?}");
}

/// Names have to mean one thing: no empties, no repeats, and none of them
/// may be an agent's own name while meaning something else.
#[test]
fn profiles_that_could_not_be_offered_are_refused() {
    let h = Harness::new("profilebad");
    let _guard = h.rt.enter();
    let profile = |name: &str, agent: &str| crate::store::Profile {
        name: name.into(),
        agent: agent.into(),
        args: Vec::new(),
    };

    assert!(h.core.set_profiles(vec![profile("", "claude")]).is_err());
    assert!(h.core.set_profiles(vec![profile("x", " ")]).is_err());
    assert!(
        h.core.set_profiles(vec![profile("claude", "codex")]).is_err(),
        "a profile shadowing an agent's own name is the confusion names exist to prevent"
    );
    assert!(h
        .core
        .set_profiles(vec![profile("mine", "claude"), profile("mine", "codex")])
        .is_err());

    // And nothing broken was stored along the way.
    assert!(h.core.profiles().unwrap().is_empty());

    // The launcher list is the four bare agents plus whatever is stored.
    h.core.set_profiles(vec![profile("mine", "claude")]).unwrap();
    let names: Vec<String> = h.core.launchers().unwrap().into_iter().map(|l| l.name).collect();
    assert_eq!(names, vec!["claude", "codex", "gemini", "aider", "mine"]);
}

/* --------------------------- permission modes -------------------------- */

/// The auto-accept switch, with the worktree as the safety case: the attempt
/// can only spend its own branch. Yolo adds Claude Code's own flag as an
/// option — and the prompt still rides last, after it.
#[test]
fn a_yolo_attempt_launches_claude_with_the_skip_permissions_flag() {
    let h = Harness::new("yolo");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");

    let opened = h
        .core
        .start_attempt(&task, "claude".into(), None, PermissionMode::Yolo, 100, 30)
        .unwrap()
        .attempt
        .unwrap();

    let args = h.args_of(&opened.session_id);
    assert!(
        args.contains(&"--dangerously-skip-permissions".to_string()),
        "yolo did not reach the command line: {args:?}"
    );
    assert_eq!(args.last(), Some(&opened.prompt));

    // Recorded on the attempt: the card can say this one runs unprompted.
    let attempt = h.core.task_board()[0].attempts[0].attempt.clone();
    assert_eq!(attempt.mode, PermissionMode::Yolo);
}

#[test]
fn accept_edits_maps_to_claudes_own_permission_mode_flag() {
    let h = Harness::new("acceptedits");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");

    let opened = h
        .core
        .start_attempt(&task, "claude".into(), None, PermissionMode::AcceptEdits, 100, 30)
        .unwrap()
        .attempt
        .unwrap();

    let args = h.args_of(&opened.session_id);
    let pair = args
        .windows(2)
        .any(|w| w[0] == "--permission-mode" && w[1] == "acceptEdits");
    assert!(pair, "acceptEdits did not reach the command line: {args:?}");
}

/// The mode was approved for the attempt, not for one launch: a resume after
/// a restart runs with it again, alongside `--continue`.
#[test]
fn resuming_a_yolo_attempt_keeps_the_mode() {
    let h = Harness::new("yoloresume");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h
        .core
        .start_attempt(&task, "claude".into(), None, PermissionMode::Yolo, 100, 30)
        .unwrap()
        .attempt
        .unwrap();
    h.launches(&a.session_id, 1);

    h.core.close_session(&a.session_id).unwrap();
    let session_id = h.core.reopen_attempt(&a.attempt_id, 100, 30).expect("reopen");

    let second = h.launches(&session_id, 2).pop().unwrap();
    assert!(second.args.iter().any(|x| x == "--continue"), "{:?}", second.args);
    assert!(
        second.args.iter().any(|x| x == "--dangerously-skip-permissions"),
        "the resume dropped the approved mode: {:?}",
        second.args
    );
}

/// Only the measured CLIs' flags exist. Another CLI launches without any of
/// them no matter what the mode says — a flag guessed wrong can mean
/// anything, including "print this and exit".
#[test]
fn another_cli_is_not_handed_a_measured_clis_permission_flags() {
    let h = Harness::new("yologemini");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");

    let opened = h
        .core
        .start_attempt(&task, "gemini".into(), None, PermissionMode::Yolo, 100, 30)
        .unwrap()
        .attempt
        .unwrap();

    assert!(
        h.args_of(&opened.session_id).is_empty(),
        "an unmeasured CLI was handed flags that belong to another"
    );
}

/// Codex's own spelling of the same three things — the prompt on the command
/// line, the mode as a sandbox and an approval policy, and the hook config
/// as `-c` overrides — all in the order that survives its parser.
#[test]
fn codex_gets_the_prompt_its_own_mode_flags_and_its_own_hook_config() {
    let h = Harness::new("codexlaunch");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");

    let opened = h
        .core
        .start_attempt(
            &task,
            "codex".into(),
            None,
            PermissionMode::AcceptEdits,
            100,
            30,
        )
        .unwrap()
        .attempt
        .unwrap();

    assert!(opened.prompt_sent, "codex takes its prompt positionally");
    let args = h.args_of(&opened.session_id);

    // Claude Code's flags must not have leaked across: they mean nothing
    // here, and `--permission-mode` would stop codex before it drew a
    // terminal.
    for claudes in ["--permission-mode", "--plugin-dir", "--continue", "--name"] {
        assert!(!args.iter().any(|a| a == claudes), "{claudes} in {args:?}");
    }
    // Its own, in its own words.
    assert!(args.iter().any(|a| a == "--sandbox"), "{args:?}");
    assert!(args.iter().any(|a| a == "workspace-write"), "{args:?}");
    assert!(args.iter().any(|a| a == "--ask-for-approval"), "{args:?}");

    // The hook config rides as `-c` overrides, one per event, and every one
    // of them names the listener this desk is running.
    let overrides: Vec<&String> = args
        .iter()
        .zip(args.iter().skip(1))
        .filter(|(flag, _)| *flag == "-c")
        .map(|(_, value)| value)
        .collect();
    assert!(!overrides.is_empty(), "codex was wired for no hooks: {args:?}");
    for value in &overrides {
        assert!(value.starts_with("hooks."), "not a hooks override: {value}");
        assert!(value.contains("127.0.0.1"), "no listener in {value}");
    }

    // And the prompt is last, after every one of them.
    let last = args.last().expect("a command line");
    assert!(last.contains("make it work"), "the prompt is not last: {args:?}");
}

/// Reopening a codex attempt resumes through its subcommand, with the
/// approved mode still on, and without a second copy of the prompt — the
/// same three promises `--continue` keeps for Claude Code, kept in the other
/// CLI's grammar.
#[test]
fn reopening_a_codex_attempt_resumes_through_its_subcommand() {
    let h = Harness::new("codexreopen");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");

    let opened = h
        .core
        .start_attempt(&task, "codex".into(), None, PermissionMode::Yolo, 100, 30)
        .unwrap()
        .attempt
        .unwrap();
    h.launches(&opened.session_id, 1);

    h.core.close_session(&opened.session_id).unwrap();
    h.core.reopen_attempt(&opened.attempt_id, 100, 30).unwrap();
    let second = h.launches(&opened.session_id, 2).pop().unwrap();

    let at = second
        .args
        .iter()
        .position(|a| a == "resume")
        .unwrap_or_else(|| panic!("no resume in {:?}", second.args));
    assert_eq!(second.args[at + 1], "--last");
    assert_eq!(at + 2, second.args.len(), "codex reads what follows `resume` as a session to resume: {:?}", second.args);
    assert!(
        second.args.iter().any(|a| a == "--dangerously-bypass-approvals-and-sandbox"),
        "the resume dropped the approved mode: {:?}",
        second.args
    );
    assert!(
        !second.args.iter().any(|a| a.contains("[Marol")),
        "the prompt was sent a second time: {:?}",
        second.args
    );
}

/// What was approved is what runs, even from the queue.
#[test]
fn a_queued_start_keeps_its_mode_when_its_turn_comes() {
    let h = Harness::new("yoloqueue");
    let _guard = h.rt.enter();
    h.core.set_max_concurrent(1).unwrap();
    let first = h.card("First", "p");
    let second = h.card("Second", "p");

    let a = h
        .core
        .start_attempt(&first, "claude".into(), None, PermissionMode::Normal, 100, 30)
        .unwrap();
    h.core
        .start_attempt(&second, "claude".into(), None, PermissionMode::Yolo, 100, 30)
        .unwrap();

    h.core.close_session(&a.attempt.unwrap().session_id).unwrap();
    let started = wait_for(Duration::from_secs(10), || {
        h.core
            .task_board()
            .into_iter()
            .find(|t| t.task.id == second)
            .map(|t| !t.attempts.is_empty())
            .unwrap_or(false)
    });
    assert!(started, "the queue never moved");

    let view = h
        .core
        .task_board()
        .into_iter()
        .find(|t| t.task.id == second)
        .unwrap();
    assert_eq!(view.attempts[0].attempt.mode, PermissionMode::Yolo);
    let session = view.attempts[0].session_id.clone().unwrap();
    assert!(h
        .args_of(&session)
        .contains(&"--dangerously-skip-permissions".to_string()));
}

/* --------------------------- workspace scripts ------------------------- */

/// M6's core promise: a fresh worktree is made runnable before the agent
/// starts, in the same terminal, and the agent still gets its argv untouched.
#[test]
fn setup_runs_in_the_worktree_before_the_agent_starts() {
    let h = Harness::new("setup");
    let _guard = h.rt.enter();
    h.config(r#"{ "setup": "echo tools-ready > setup-ran.txt" }"#);
    let task = h.card("Fix login", "make it work");

    let a = h.start(&task, "claude");

    // The setup left its mark in the worktree…
    let marker = std::path::PathBuf::from(&a.worktree_path).join("setup-ran.txt");
    assert!(
        wait_for(Duration::from_secs(10), || marker.exists()),
        "setup never ran in the worktree"
    );

    // …and the agent still launched with the prompt as its last argument,
    // exactly as it would have without the wrap. This is the property the
    // `exec "$0" "$@"` construction exists to keep.
    let args = h.args_of(&a.session_id);
    assert_eq!(args.last(), Some(&a.prompt));
    assert_eq!(
        std::fs::canonicalize(h.cwd_of(&a.session_id)).unwrap(),
        std::fs::canonicalize(&a.worktree_path).unwrap()
    );
}

/// `set -e`: a setup that fails stops in front of the person instead of
/// starting an agent in a half-made workspace.
#[test]
fn a_failed_setup_stops_before_the_agent_ever_starts() {
    let h = Harness::new("setupfail");
    let _guard = h.rt.enter();
    h.config(r#"{ "setup": "echo broken deps >&2; exit 7" }"#);
    let task = h.card("Fix login", "make it work");

    let a = h.start(&task, "claude");

    let exited = wait_for(Duration::from_secs(10), || {
        h.core
            .sessions()
            .iter()
            .any(|s| s.id == a.session_id && s.status == Status::Exited)
    });
    assert!(exited, "the failed setup did not end the session");
    assert!(
        !h.launched(&a.session_id),
        "the agent was started despite setup failing"
    );
}

/// A run script gets a terminal of its own in the attempt's worktree, a free
/// port, and the way back to the root repository — and it takes no slot,
/// because the quota rations agents, not dev servers.
#[test]
fn a_run_script_gets_its_own_terminal_a_port_and_the_root_path() {
    let h = Harness::new("runscript");
    let _guard = h.rt.enter();
    h.config(
        r#"{ "run": [{ "name": "srv",
             "command": "echo $MAROL_PORT > port.txt; echo \"$MAROL_ROOT_PATH\" > root.txt; exec cat" }] }"#,
    );
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");

    assert_eq!(h.core.list_run_scripts(&a.attempt_id).unwrap(), vec!["srv"]);
    let session_id = h.core.run_script(&a.attempt_id, "srv", 100, 30).expect("run");

    let wt = std::path::PathBuf::from(&a.worktree_path);
    assert!(
        wait_for(Duration::from_secs(10), || wt.join("port.txt").exists()
            && wt.join("root.txt").exists()),
        "the run script never wrote its files"
    );
    let port: u16 = std::fs::read_to_string(wt.join("port.txt"))
        .unwrap()
        .trim()
        .parse()
        .expect("MAROL_PORT was not a port number");
    assert!(port > 0);
    assert_eq!(
        std::fs::read_to_string(wt.join("root.txt")).unwrap().trim(),
        h.repo.to_string_lossy()
    );

    // Ad-hoc: on nobody's card, against nobody's quota.
    let session = h
        .core
        .sessions()
        .into_iter()
        .find(|s| s.id == session_id)
        .expect("the run session is in the list");
    assert_eq!(session.attempt_id, None);
    assert_eq!(h.core.running_attempts(), 1, "the dev server took an agent's slot");

    // Closing the attempt takes the squatter with the directory it lived in.
    h.core.finish_attempt(&a.attempt_id, Outcome::Discarded).unwrap();
    assert!(
        !h.core.sessions().iter().any(|s| s.id == session_id),
        "a terminal survived the deletion of its own directory"
    );
}

/// The archive script runs while the worktree still exists, and the worktree
/// still comes back afterwards.
#[test]
fn the_archive_script_runs_before_the_worktree_goes_back() {
    let h = Harness::new("archive");
    let _guard = h.rt.enter();
    h.config(r#"{ "archive": "echo closed > \"$MAROL_ROOT_PATH/archive-ran.txt\"" }"#);
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");

    h.core.finish_attempt(&a.attempt_id, Outcome::Discarded).unwrap();

    assert!(
        h.repo.join("archive-ran.txt").exists(),
        "the archive script never ran"
    );
    assert!(!std::path::Path::new(&a.worktree_path).exists());
}

/// A typo in the config fails the start in the dialog, not silently at the
/// first moment someone wonders why the worktree is broken.
#[test]
fn a_malformed_config_fails_the_start_where_the_person_can_see_it() {
    let h = Harness::new("badcfg");
    let _guard = h.rt.enter();
    h.config(r#"{ "setup": ["not", "a", "string"] }"#);
    let task = h.card("Fix login", "make it work");

    let err = h
        .core
        .start_attempt(&task, "claude".into(), None, PermissionMode::Normal, 100, 30)
        .expect_err("a config typo must be an error someone sees");
    assert!(err.to_string().contains("config.json"), "unhelpful: {err}");
    // And the worktree it would have used was given back.
    assert_eq!(h.core.running_attempts(), 0);
}

/* ------------------------------ the timeline --------------------------- */

/// The acceptance for M3's second half: enough of a record to say what this
/// attempt did without opening its terminal.
///
/// Hooks were only ever used to compute a badge and then dropped. This drives
/// the whole chain — listener, router, channel, writer thread, database.
#[test]
fn the_timeline_records_what_the_agent_reached_for() {
    let h = Harness::new("timeline");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");

    let tool = |name: &str, input: serde_json::Value| {
        serde_json::json!({ "hook_event_name": "PreToolUse", "tool_name": name, "tool_input": input })
    };
    h.hook(&a.session_id, "running", tool("Bash", serde_json::json!({ "command": "pytest -v" })));
    h.hook(
        &a.session_id,
        "running",
        tool("Edit", serde_json::json!({ "file_path": "/repo/auth.py" })),
    );
    // A repeat of the tool before it is still its own moment.
    h.hook(
        &a.session_id,
        "running",
        tool("Edit", serde_json::json!({ "file_path": "/repo/auth.py" })),
    );
    h.hook(&a.session_id, "waiting_permission", serde_json::Value::Null);
    h.hook(&a.session_id, "idle", serde_json::Value::Null);

    // The opening prompt, three tool calls, and two status changes.
    let rows = h.timeline(&a.attempt_id, 6);

    let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
    assert_eq!(kinds, vec!["prompt", "tool", "tool", "tool", "status", "status"]);

    let tools: Vec<Option<&str>> = rows.iter().map(|r| r.tool.as_deref()).collect();
    assert_eq!(
        tools,
        vec![None, Some("Bash"), Some("Edit"), Some("Edit"), None, None]
    );
    assert_eq!(rows[1].detail.as_deref(), Some("pytest -v"));
    assert_eq!(rows[2].detail.as_deref(), Some("/repo/auth.py"));
    assert_eq!(rows[4].detail.as_deref(), Some("waiting_permission"));
    assert_eq!(rows[5].detail.as_deref(), Some("idle"));
}

/// `running` is already implied by the tool call that carried it, and a
/// status line between every pair of tool calls would bury them.
#[test]
fn the_timeline_does_not_narrate_every_status_report() {
    let h = Harness::new("quiet");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");

    for _ in 0..3 {
        h.hook(
            &a.session_id,
            "running",
            serde_json::json!({ "hook_event_name": "UserPromptSubmit" }),
        );
    }
    h.hook(&a.session_id, "idle", serde_json::Value::Null);
    // Reported twice; it only changed once.
    h.hook(&a.session_id, "idle", serde_json::Value::Null);

    // Wait for the two we do expect, then give any extras time to show up
    // before asserting that there are none.
    h.timeline(&a.attempt_id, 2);
    std::thread::sleep(Duration::from_millis(300));
    let rows = h.core.attempt_events(&a.attempt_id).unwrap();
    assert_eq!(
        rows.len(),
        2,
        "expected the prompt and one idle, got {rows:?}"
    );
    assert_eq!(rows[1].detail.as_deref(), Some("idle"));
}

/// Hooks from an ad-hoc session have no attempt to file against. They still
/// have to drive the badge without inventing a timeline.
#[test]
fn an_ad_hoc_sessions_hooks_do_not_land_on_anybodys_timeline() {
    let h = Harness::new("adhoc");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");
    let scratch = h
        .core
        .new_session(h.repo.to_string_lossy().into(), "claude".into(), vec![], 100, 30)
        .unwrap();

    h.hook(
        &scratch,
        "running",
        serde_json::json!({ "hook_event_name": "PreToolUse", "tool_name": "Bash",
                            "tool_input": { "command": "ls" } }),
    );
    h.hook(&scratch, "waiting_permission", serde_json::Value::Null);
    std::thread::sleep(Duration::from_millis(300));

    // Only the opening prompt, from the attempt itself.
    assert_eq!(h.core.attempt_events(&a.attempt_id).unwrap().len(), 1);
    // But the ad-hoc session is still blocking a person, so it still counts.
    let waiting = h.core.sessions().iter().filter(|s| s.status.needs_you()).count();
    assert_eq!(waiting, 2);
}

/// The diff has to answer "what changed" while the attempt is still running,
/// not only once it has been closed out.
#[test]
fn a_running_attempts_diff_is_read_live_from_its_worktree() {
    let h = Harness::new("livediff");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");

    assert_eq!(
        h.core.attempt_diff(&a.attempt_id).unwrap(),
        "",
        "an attempt that has changed nothing has an empty diff"
    );

    std::fs::write(Path::new(&a.worktree_path).join("app.txt"), "half done\n").unwrap();
    std::fs::write(Path::new(&a.worktree_path).join("scratch.rs"), "fn new() {}\n").unwrap();

    let diff = h.core.attempt_diff(&a.attempt_id).unwrap();
    assert!(diff.contains("half done"), "the edit is missing:\n{diff}");
    assert!(diff.contains("scratch.rs"), "the new file is missing:\n{diff}");
    assert!(diff.contains("fn new() {}"), "its contents are missing:\n{diff}");
}

/* ------------------------------ the board ------------------------------ */

#[test]
fn a_card_pointing_at_something_that_is_not_a_repository_is_refused() {
    let h = Harness::new("notrepo");
    let _guard = h.rt.enter();
    let plain = h.root.join("plain");
    std::fs::create_dir_all(&plain).unwrap();

    let err = h
        .core
        .create_task(
            "x".into(),
            "y".into(),
            plain.to_string_lossy().into(),
            "main".into(),
            Vec::new(),
        )
        .expect_err("a card that can never run must not be created");
    assert!(err.to_string().contains("not a git repository"), "{err}");
    assert!(h.core.task_board().is_empty());
}

#[test]
fn a_card_naming_a_base_branch_that_does_not_exist_is_refused() {
    let h = Harness::new("nobranch");
    let _guard = h.rt.enter();
    let err = h
        .core
        .create_task(
            "x".into(),
            "y".into(),
            h.repo.to_string_lossy().into(),
            "develop".into(),
            Vec::new(),
        )
        .expect_err("a missing base branch must be caught when the card is made");
    assert!(err.to_string().contains("no branch `develop`"), "{err}");
}

/// Dragging a card renumbers the column it left as well as the one it joined,
/// or the gap it leaves behind changes what "position 2" means.
#[test]
fn moving_a_card_renumbers_the_column_it_left_and_the_one_it_joined() {
    let h = Harness::new("move");
    let _guard = h.rt.enter();
    let a = h.card("A", "p");
    let b = h.card("B", "p");
    let c = h.card("C", "p");

    // Backlog is A, B, C. Move B to the front of review.
    h.core.move_task(&b, Lifecycle::Review, 0).unwrap();

    let board = h.core.task_board();
    let backlog: Vec<_> = board
        .iter()
        .filter(|t| t.task.lifecycle == Lifecycle::Backlog)
        .map(|t| (t.task.id.clone(), t.task.position))
        .collect();
    assert_eq!(backlog, vec![(a.clone(), 0), (c.clone(), 1)], "gap left behind");

    let review: Vec<_> = board
        .iter()
        .filter(|t| t.task.lifecycle == Lifecycle::Review)
        .map(|t| (t.task.id.clone(), t.task.position))
        .collect();
    assert_eq!(review, vec![(b.clone(), 0)]);

    // And back again, into the middle this time.
    h.core.move_task(&b, Lifecycle::Backlog, 1).unwrap();
    let order: Vec<_> = h
        .core
        .task_board()
        .into_iter()
        .filter(|t| t.task.lifecycle == Lifecycle::Backlog)
        .map(|t| t.task.id)
        .collect();
    assert_eq!(order, vec![a, b, c]);
}

/// Deleting a card must take its worktrees with it, or the directories
/// outlive every record that they ever existed.
#[test]
fn deleting_a_card_gives_back_the_worktrees_its_attempts_were_holding() {
    let h = Harness::new("delete");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");
    assert!(Path::new(&a.worktree_path).is_dir());

    h.core.delete_task(&task).unwrap();

    assert!(h.core.task_board().is_empty());
    assert!(
        !Path::new(&a.worktree_path).exists(),
        "the worktree outlived the card that made it"
    );
}

/* --------------------------- editable diff ------------------------------ */

/// The editable diff's two commands through the core: the mid-turn refusal
/// happens here and not just in the UI, the settled write lands on disk
/// exactly, and both sides of the file read back — base as committed, work
/// as written.
#[test]
fn a_hand_edit_is_refused_mid_turn_and_lands_once_the_attempt_settles() {
    let h = Harness::new("editfile");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");

    // Fresh from launch the session is live and unsettled: the core must
    // refuse, whatever buttons the UI happens to be hiding.
    let err = h
        .core
        .write_attempt_file(&a.attempt_id, "app.txt", "hand edit\n", None)
        .expect_err("a mid-turn write must be refused");
    assert!(err.to_string().contains("mid-turn"), "unhelpful: {err}");

    // The turn ends; the same write is now a person editing their own repo.
    h.hook(&a.session_id, "idle", serde_json::Value::Null);
    assert!(
        wait_for(Duration::from_secs(5), || h
            .core
            .write_attempt_file(&a.attempt_id, "app.txt", "hand edit\n", Some("one\n"))
            .is_ok()),
        "a settled write must go through"
    );
    assert_eq!(
        std::fs::read_to_string(Path::new(&a.worktree_path).join("app.txt")).unwrap(),
        "hand edit\n"
    );

    let file = h.core.attempt_file(&a.attempt_id, "app.txt").unwrap();
    assert_eq!(file.base.as_deref(), Some("one\n"));
    assert_eq!(file.work.as_deref(), Some("hand edit\n"));

    // The freshness contract: an editor still believing the disk holds the
    // base text is stale — the write above moved it — and last-write-wins
    // would erase that unseen. Refused, with the reason.
    let err = h
        .core
        .write_attempt_file(&a.attempt_id, "app.txt", "third\n", Some("one\n"))
        .expect_err("a stale write must be refused");
    assert!(err.to_string().contains("changed on disk"), "unhelpful: {err}");
    h.core
        .write_attempt_file(&a.attempt_id, "app.txt", "third\n", Some("hand edit\n"))
        .expect("the fresh expectation goes through");

    // A file the attempt never touched at base, deleted in the worktree:
    // work side None, not an error.
    std::fs::remove_file(Path::new(&a.worktree_path).join("app.txt")).unwrap();
    let gone = h.core.attempt_file(&a.attempt_id, "app.txt").unwrap();
    assert!(gone.work.is_none());

    // Paths that would leave the worktree stop at the invoke boundary.
    assert!(h
        .core
        .write_attempt_file(&a.attempt_id, "../escape.txt", "x", None)
        .is_err());

    // Parked there is no ground to read or write; both commands say so.
    h.core.park_attempt(&a.attempt_id).unwrap();
    assert!(h.core.attempt_file(&a.attempt_id, "app.txt").is_err());
    assert!(h
        .core
        .write_attempt_file(&a.attempt_id, "app.txt", "y", None)
        .is_err());
}

/* ---------------------------- token account ----------------------------- */

/// The whole cost pipeline end to end: a Stop hook whose body names the
/// transcript, the turn-end read on its own thread, and the account landing
/// on the session for the next broadcast — incrementally, so the second
/// turn only pays for its own lines.
#[test]
fn a_turns_end_reads_the_transcript_and_the_account_lands_on_the_session() {
    let h = Harness::new("usage");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");

    let transcript = h.root.join("transcript.jsonl");
    std::fs::write(
        &transcript,
        concat!(
            r#"{"type":"assistant","message":{"usage":{"input_tokens":10,"output_tokens":100,"cache_read_input_tokens":1000,"cache_creation_input_tokens":50}}}"#,
            "\n",
        ),
    )
    .unwrap();

    // The Stop payload, as Claude Code posts it: common fields in the body.
    h.hook(
        &a.session_id,
        "idle",
        serde_json::json!({
            "hook_event_name": "Stop",
            "transcript_path": transcript.to_string_lossy(),
        }),
    );
    assert!(
        wait_for(Duration::from_secs(5), || {
            h.core
                .sessions()
                .iter()
                .find(|s| s.id == a.session_id)
                .and_then(|s| s.usage)
                .is_some_and(|u| u.output == 100 && u.context == 1060)
        }),
        "the first turn's account never landed: {:?}",
        h.core.sessions().iter().find(|s| s.id == a.session_id).and_then(|s| s.usage)
    );

    // The next turn appends — a sidechain (spend counts, context must not
    // move to it) and a main-line row. Totals accumulate across reads.
    let mut file = std::fs::OpenOptions::new().append(true).open(&transcript).unwrap();
    use std::io::Write as _;
    writeln!(
        file,
        r#"{{"type":"assistant","isSidechain":true,"message":{{"usage":{{"input_tokens":1,"output_tokens":40,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
    )
    .unwrap();
    writeln!(
        file,
        r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":2,"output_tokens":60,"cache_read_input_tokens":2000,"cache_creation_input_tokens":8}}}}}}"#
    )
    .unwrap();
    drop(file);

    h.hook(&a.session_id, "running", serde_json::Value::Null);
    h.hook(&a.session_id, "idle", serde_json::Value::Null);
    assert!(
        wait_for(Duration::from_secs(5), || {
            h.core
                .sessions()
                .iter()
                .find(|s| s.id == a.session_id)
                .and_then(|s| s.usage)
                .is_some_and(|u| u.output == 200 && u.context == 2010 && u.input == 13)
        }),
        "the second turn's increment never landed: {:?}",
        h.core.sessions().iter().find(|s| s.id == a.session_id).and_then(|s| s.usage)
    );
}

/// The badge counts what is blocking a person, across the board and the
/// ad-hoc sessions alike, because they are the same list underneath.
#[test]
fn the_badge_counts_attempts_and_ad_hoc_sessions_together() {
    let h = Harness::new("badge");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    h.start(&task, "claude");
    h.core
        .new_session(h.repo.to_string_lossy().into(), "claude".into(), vec![], 100, 30)
        .unwrap();

    let waiting = h.core.sessions().iter().filter(|s| s.status.needs_you()).count();
    // The attempt is on its trust prompt; the ad-hoc session is not, because
    // its directory is one the person already chose.
    assert_eq!(waiting, 1, "{:?}", h.core.sessions());
}

/// A held session survives the app, and the next start says so.
///
/// `from_stored` marks everything `Saved`, which was true when every terminal
/// died with the app. With tmux holding local sessions it is not, and a card
/// reading "closed" over a working agent is how somebody starts a second
/// attempt onto the same worktree.
///
/// An agent held from before the app was renamed is reattached, not doubled.
///
/// This is the one thing the rename could not simply leave behind. A local
/// socket lives in tmux's own shared directory, so its name has to say whose
/// it is — which means the rename reaches it. The server is bound to
/// `agentdesk-…`; asking `new-session -A` for `marol-…` would not find it and
/// would cheerfully start a *second* agent in the same worktree, which is
/// precisely the accident holding sessions was built to prevent.
///
/// Staged the only way that is honest: a real tmux server under the old name,
/// holding a real process, with the card's row saying `Saved` exactly as a
/// restart leaves it.
#[test]
fn an_agent_held_from_before_the_rename_is_reattached_not_started_again() {
    if std::process::Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("no tmux on PATH — nothing holds sessions here");
        return;
    }
    let h = Harness::new("renamed");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");
    h.launches(&a.session_id, 1);

    // What the last run left: a held server under the name of the day.
    let tag = pty::desk_tag(&h.root.join("data").to_string_lossy());
    let former = format!("agentdesk-{tag}-{}", a.session_id);
    let current = pty::hold_socket(&tag, &a.session_id);
    h.core.shutdown();
    assert!(
        std::process::Command::new("tmux")
            .args(["-L", &current, "kill-server"])
            .output()
            .is_ok()
    );
    if let Some(d) = core::tmux_socket_dir() {
        let _ = std::fs::remove_file(d.join(&current));
    }
    assert!(
        std::process::Command::new("tmux")
            .args(["-L", &former, "new-session", "-d", "-s", "agent", "--", "sleep", "300"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        "could not stage a session under the old name"
    );

    let core2 = h
        .rt
        .block_on(Core::start_with(
            h.env.clone(),
            Arc::new(Events::default()) as Arc<dyn UiSink>,
            h.root.join("marol.db"),
            h.root.join("data"),
            h.root.join("worktrees"),
        ))
        .expect("second core");

    // (a) The board does not call it closed. A row reading 已關閉 over a
    // working agent is how somebody starts a second attempt on the same tree.
    let seen = core2
        .sessions()
        .into_iter()
        .find(|s| s.id == a.session_id)
        .expect("still on the list");
    assert_eq!(
        seen.status,
        Status::Detached,
        "an agent held under the old name came back as {:?}",
        seen.status,
    );

    // (b) Opening it reaches *that* server. Asserted as a client arriving on
    // the old socket rather than as a second socket failing to appear: the
    // spawn is asynchronous, so "the file is not there" is true for a moment
    // whatever happens, and a test that reads it too early passes for the
    // wrong reason. A client on that server is positive evidence, and it can
    // be waited for.
    core2
        .reopen_session(&a.session_id, 100, 30)
        .expect("reattach to the held session");
    let clients = |sock: &str| {
        std::process::Command::new("tmux")
            .args(["-L", sock, "list-clients", "-t", "agent"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while clients(&former).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !clients(&former).is_empty(),
        "nothing attached to {former}; the agent held from before the rename was abandoned",
    );
    // And nothing was started beside it. By now the reattach has happened, so
    // a socket under the new name could only be a second agent on the same
    // worktree.
    assert!(
        !core::tmux_socket_dir()
            .map(|d| d.join(&current).exists())
            .unwrap_or(false),
        "a second agent was started under {current} while {former} was still running",
    );

    core2.shutdown();
    let _ = std::process::Command::new("tmux")
        .args(["-L", &former, "kill-server"])
        .output();
    if let Some(d) = core::tmux_socket_dir() {
        let _ = std::fs::remove_file(d.join(&former));
    }
}

/// Skipped where tmux is absent: there is nothing to hold sessions with, so
/// there is nothing to be honest or dishonest about.
#[test]
fn a_session_tmux_kept_running_does_not_come_back_as_closed() {
    if std::process::Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("no tmux on PATH — nothing holds sessions here");
        return;
    }
    let h = Harness::new("detached");
    let _guard = h.rt.enter();
    let task = h.card("Fix login", "make it work");
    let a = h.start(&task, "claude");
    h.launches(&a.session_id, 1);

    // The endpoint this session was told to report to. Its plugin config has
    // this string baked into it now, and Claude Code reads that file once, so
    // this is the only address it will ever have.
    let told = h.core.hook_url().expect("the hook server is up");

    // The agent is running under tmux. Quitting drops the client and leaves
    // it there — which is `shutdown`, exactly what closing the window does.
    h.core.shutdown();

    // A fresh core over the same database and data dir: the restart.
    let core2 = h
        .rt
        .block_on(Core::start_with(
            h.env.clone(),
            Arc::new(Events::default()) as Arc<dyn UiSink>,
            h.root.join("marol.db"),
            h.root.join("data"),
            h.root.join("worktrees"),
        ))
        .expect("second core");

    let seen = core2
        .sessions()
        .into_iter()
        .find(|s| s.id == a.session_id)
        .expect("the session is still on the list");
    assert_eq!(
        seen.status,
        Status::Detached,
        "a session tmux kept running came back as {:?}",
        seen.status,
    );
    // Not live: no pty in *this* process carries it yet. Opening attaches.
    assert!(!seen.live, "nothing is attached to it in this process");

    // The cause, not the symptom. Whether the held agent's reports arrive is
    // decided entirely by whether this string is the one it was told, so that
    // is what is asserted: not "a status eventually appeared", which could
    // pass for a dozen unrelated reasons, but "the address did not move".
    assert_eq!(
        core2.hook_url().as_deref(),
        Some(told.as_str()),
        "the endpoint moved, so every session held through the restart is \
         posting into nothing for the rest of its life",
    );

    // Attaching is not starting. `new-session -A -D` reattaches to the agent
    // and drops the argv, so no SessionStart will ever fire for it — a row
    // that said 啟動中 here would say it forever.
    core2
        .reopen_session(&a.session_id, 100, 30)
        .expect("reattach to the held session");
    let after = core2
        .sessions()
        .into_iter()
        .find(|s| s.id == a.session_id)
        .expect("still on the list");
    assert_ne!(
        after.status,
        Status::Starting,
        "reattaching claimed the agent was starting; nothing would ever correct that",
    );
    assert_eq!(after.status, Status::Detached, "running, and not yet heard from");
    assert!(after.live, "a terminal in this process carries it now");

    core2.shutdown();
}

/* --------------------------- the folder picker --------------------------- */

/// The listing a folder picker reads: where it actually is, a way back up,
/// directories only, and the dotfile noise after the thing somebody came for.
///
/// This exists because no platform folder dialog can answer the same question
/// for all three worlds — it browses the machine the app runs on, which is
/// the wrong filesystem for a WSL card and a filesystem that is not mounted
/// at all for an SSH one. Local is the case that could have used the dialog,
/// and it goes through the same door so there is only one to keep correct.
#[test]
fn a_listing_names_the_directories_and_nothing_else() {
    let h = Harness::new("lsdir");
    let _guard = h.rt.enter();

    let dir = h.root.join("browse");
    std::fs::create_dir_all(dir.join("project")).unwrap();
    std::fs::create_dir_all(dir.join("Apples")).unwrap();
    std::fs::create_dir_all(dir.join(".config")).unwrap();
    std::fs::write(dir.join("notes.txt"), "not a directory").unwrap();

    let listing = h
        .core
        .list_dir("", Some(&dir.to_string_lossy()))
        .expect("the directory lists");

    assert_eq!(
        listing.dirs,
        vec!["Apples", "project", ".config"],
        "case-insensitive alphabetical, dotfiles last, and the file not at all"
    );
    assert!(
        listing.parent.is_some(),
        "this is not a root, so there is a way back up"
    );
    assert!(!listing.is_repo);
}

/// `None` starts where a person starts: that world's own home. Not this
/// machine's, and not a remembered path that may not exist over there.
#[test]
fn no_path_starts_at_the_worlds_own_home() {
    let h = Harness::new("lshome");
    let _guard = h.rt.enter();

    let listing = h.core.list_dir("", None).expect("home lists");

    assert_eq!(
        listing.path,
        std::fs::canonicalize(&h.root).unwrap().to_string_lossy(),
        "the harness sets HOME to its own root, and that is where a picker opens"
    );
}

/// A path that is not there is a refusal naming it, not an empty listing —
/// which would read as "this directory happens to have nothing in it".
#[test]
fn a_missing_directory_says_so_rather_than_reading_as_empty() {
    let h = Harness::new("lsmissing");
    let _guard = h.rt.enter();

    let missing = h.root.join("no-such-directory-ever");
    let err = h
        .core
        .list_dir("", Some(&missing.to_string_lossy()))
        .expect_err("a directory that is not there cannot be listed");

    assert!(
        format!("{err:#}").contains("no-such-directory-ever"),
        "the refusal names what could not be opened: {err:#}"
    );
}

/// A checkout is called out where it stands. The picker is nearly always
/// looking for one, and saying so beats making somebody descend to find out.
#[test]
fn a_repository_is_named_as_one_where_it_stands() {
    let h = Harness::new("lsrepo");
    let _guard = h.rt.enter();

    let listing = h
        .core
        .list_dir("", Some(&h.repo.to_string_lossy()))
        .expect("the harness repo lists");

    assert!(
        listing.is_repo,
        "the harness builds a real git checkout there"
    );
}

/// The resolved path is the world's answer, not an echo of the question.
/// A picker that echoed would build its next step on a guess — and one
/// symlink in the way makes every path below it wrong.
#[test]
fn the_listing_reports_where_it_really_is() {
    let h = Harness::new("lsreal");
    let _guard = h.rt.enter();

    let real = h.root.join("actual");
    std::fs::create_dir_all(real.join("inside")).unwrap();

    let listing = h
        .core
        .list_dir("", Some(&format!("{}/./actual", h.root.to_string_lossy())))
        .expect("lists through the detour");

    assert!(
        !listing.path.contains("/./"),
        "the path came back resolved, not as it was typed: {}",
        listing.path
    );
    assert_eq!(listing.dirs, vec!["inside"]);
}
