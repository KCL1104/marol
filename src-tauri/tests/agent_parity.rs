//! Every flag and every line of config this app puts in front of an agent
//! CLI, held to the CLI that is actually installed.
//!
//! `agent.rs` is a table of somebody else's conventions. Tables like that do
//! not fail loudly when they go stale — a renamed flag is a session that
//! exits before it draws a terminal, and a config key that quietly became a
//! literal string is a card that simply never shows status. Neither looks
//! like a bug in this repository.
//!
//! So the table is checked against the real thing, on a schedule, by
//! `.github/workflows/agent-parity.yml`:
//!
//!   * **the flags** — every dashed token `agent.rs` can emit has to appear
//!     in that CLI's own `--help`, and every value it pairs with one
//!     (`acceptEdits`, `workspace-write`) has to appear there too, because a
//!     flag that survives a rename of its values is a flag that fails at
//!     launch
//!   * **the resume subcommand** — `codex resume` has to still be a
//!     subcommand, and `--continue` still an option
//!   * **the hook config** — Codex's own `doctor` has to report the exact
//!     `-c` arguments this app passes as config it loaded, not as config it
//!     shrugged at
//!   * **the whole pipeline** — a real `codex` started with those arguments
//!     has to reach this app's real listener, with the session id expanded
//!     and the payload in the body
//!   * **bracketed paste** — both TUIs have to turn it on, because it is the
//!     only thing keeping a five-point review one message instead of five
//!
//! Nothing here needs credentials. The `codex exec` one works because
//! `SessionStart` and `UserPromptSubmit` both fire before the first request
//! goes out; the request then fails on authentication, long after the part
//! being measured. The paste one works because `ESC [ ? 2004 h` is in the
//! first bytes either CLI writes, before it has an opinion about who you
//! are.
//!
//! Each test skips, loudly, when its CLI is not installed — unless
//! `MAROL_EXPECT_CLAUDE` / `MAROL_EXPECT_CODEX` is set, which is CI saying
//! "this runner installed it, a skip here is a failure".
//!
//!     cargo test --test agent_parity -- --nocapture

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[path = "../src/agent.rs"]
mod agent;
#[path = "../src/hooks.rs"]
mod hooks;
#[path = "../src/pty.rs"]
mod pty;
#[path = "../src/shell_env.rs"]
mod shell_env;
#[path = "../src/store.rs"]
mod store;

use crate::agent::{Cli, Resume};
use crate::hooks::{HookHandler, HookReport, HookState};
use crate::pty::{PtyRegistry, PtySink};
use crate::store::PermissionMode;

/// Whether this run is required to find `agent`, and where it is.
///
/// Not being installed is a skip on a laptop and a failure in CI, and the
/// difference is one environment variable — the same shape the claude
/// detection gate has always used.
fn require(env: &shell_env::ShellEnv, agent: &str) -> Option<String> {
    let gate = format!("MAROL_EXPECT_{}", agent.to_ascii_uppercase());
    let expected = std::env::var(&gate).as_deref() == Ok("1");
    match env.which(agent) {
        Some(path) => Some(path.to_string_lossy().to_string()),
        None if expected => panic!("{gate}=1 but this app's own PATH walk cannot find `{agent}`"),
        None => {
            eprintln!("skip {agent}: not on this shell's PATH (set {gate}=1 to require it)");
            None
        }
    }
}

fn resolve_env() -> shell_env::ShellEnv {
    tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(shell_env::resolve())
}

/// Run a CLI and return stdout+stderr together.
///
/// Both streams, because `--help` goes to one or the other depending on the
/// CLI and the mood, and a parity check that read only stdout would report
/// every flag missing on whichever of them chose stderr.
fn output(exe: &str, args: &[&str], env: &shell_env::ShellEnv) -> String {
    let out = std::process::Command::new(exe)
        .args(args)
        .envs(&env.vars)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("running `{exe} {}`: {e}", args.join(" ")));
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The words of a help page, split on everything a usage line uses to
/// decorate them.
///
/// Membership in this set is the test, rather than a substring search:
/// `help.contains("-c")` is true of any page that mentions `--config`, or
/// `utf-8`, or a hyphenated sentence. A flag either is a word here or is not
/// in this CLI.
fn words(help: &str) -> HashSet<String> {
    help.split(|c: char| c.is_whitespace() || "[]<>(),|=\"'`".contains(c))
        .filter(|w| !w.is_empty())
        .map(|w| w.trim_end_matches(['.', ':', ';']).to_string())
        .collect()
}

/* ------------------------------------------------------------------ */
/* The flag contract                                                   */
/* ------------------------------------------------------------------ */

#[test]
fn every_flag_this_app_passes_is_one_the_installed_cli_accepts() {
    let env = resolve_env();
    let mut checked = 0;

    for cli in [Cli::Claude, Cli::Codex] {
        let Some(exe) = require(&env, cli.name()) else {
            continue;
        };
        checked += 1;
        let help = output(&exe, &["--help"], &env);
        let vocabulary = words(&help);
        eprintln!("{}: {} words of help", cli.name(), vocabulary.len());

        for flag in cli.every_flag() {
            assert!(
                vocabulary.contains(flag),
                "`{}` no longer takes `{flag}`; a session handed it would exit \
                 before drawing a terminal.\n--- help ---\n{help}",
                cli.name()
            );
        }

        // The values are half of every flag that takes one. `--sandbox` can
        // outlive `workspace-write` — and a launch with the flag intact and
        // the value renamed fails exactly as hard as one with neither.
        for mode in [PermissionMode::AcceptEdits, PermissionMode::Yolo] {
            for value in cli.mode_args(mode).iter().filter(|a| !a.starts_with('-')) {
                assert!(
                    vocabulary.contains(*value),
                    "`{}` no longer names `{value}` (for {:?})\n--- help ---\n{help}",
                    cli.name(),
                    mode
                );
            }
        }

        // Resuming: an option is a word on the front page; a subcommand has
        // to be a subcommand, which is a help page of its own.
        match cli.resume() {
            Resume::Option(words_) => {
                for w in words_ {
                    assert!(vocabulary.contains(*w), "`{}` lost `{w}`", cli.name());
                }
            }
            Resume::Subcommand(words_) => {
                let (sub, flags) = words_.split_first().expect("a subcommand has a name");
                let sub_help = output(&exe, &[sub, "--help"], &env);
                let sub_words = words(&sub_help);
                for f in flags {
                    assert!(
                        sub_words.contains(*f),
                        "`{} {sub}` no longer takes `{f}`\n--- help ---\n{sub_help}",
                        cli.name()
                    );
                }
            }
        }

        // Updating is the CLI's own job and this app only asks for it, so
        // what has to keep being true is that the asking still works: the
        // subcommand exists and is a subcommand. It is measured for the same
        // reason as the rest — the desk runs it unattended at startup, and a
        // rename would turn a silent success into a silent failure, which is
        // the worst of the two silences because the person keeps being told
        // by the CLI to update and keeps believing it was already done.
        //
        // Only that it is *recognised*. Running it for real here would
        // upgrade the runner's CLI mid-test and measure the parity of a
        // version this run never probed.
        assert!(
            vocabulary.contains(agent::UPDATE_SUBCOMMAND),
            "`{}` no longer lists `{}` on its front page\n--- help ---\n{help}",
            cli.name(),
            agent::UPDATE_SUBCOMMAND
        );
        let update_help = output(&exe, &[agent::UPDATE_SUBCOMMAND, "--help"], &env);
        assert!(
            words(&update_help).contains(agent::UPDATE_SUBCOMMAND),
            "`{} {}` is no longer a subcommand of its own\n--- help ---\n{update_help}",
            cli.name(),
            agent::UPDATE_SUBCOMMAND
        );

        // And the first message still rides on the command line. Both CLIs
        // spell the slot the same way in their usage line; a CLI that
        // stopped taking one would leave every attempt started but silent.
        assert!(
            vocabulary.contains("PROMPT") || vocabulary.contains("prompt"),
            "`{}` no longer takes a prompt as its positional argument\n--- help ---\n{help}",
            cli.name()
        );
    }

    if checked == 0 {
        eprintln!("skip: neither CLI is installed");
    }
}

/* ------------------------------------------------------------------ */
/* The review loop's one assumption                                    */
/* ------------------------------------------------------------------ */

#[derive(Default)]
struct Screen {
    bytes: Mutex<Vec<u8>>,
}

impl PtySink for Screen {
    fn on_output(&self, _id: &str, data: String, _seq: u64) {
        use base64::Engine as _;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .unwrap_or_default();
        self.bytes.lock().unwrap().extend_from_slice(&decoded);
    }
    fn on_exit(&self, _id: &str, _status: String) {}
}

/// A follow-up goes in through the terminal wrapped in bracketed-paste
/// markers, which is the only thing keeping a multi-line review **one**
/// message rather than one message per line.
///
/// That works because the TUI turned bracketed paste on — `ESC [ ? 2004 h`,
/// which it emits on startup. A TUI that stopped doing it would not fail
/// loudly: the markers would arrive as literal keystrokes and every newline
/// in a five-point review would submit, so the agent would start acting on
/// point one while still reading point five. This is the cheapest possible
/// check on that, and it needs no account: the sequence is in the first
/// bytes either CLI writes, long before it has an opinion about who you are.
/// Measured with an empty `HOME` as well as a signed-in one — both draw a
/// screen you could paste into before they draw a screen you could work in,
/// so a runner with no credentials still answers this honestly.
#[test]
fn both_clis_turn_bracketed_paste_on_so_a_review_arrives_as_one_message() {
    let env = resolve_env();
    let dir = std::env::temp_dir().join(format!("marol-paste-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);

    for cli in [Cli::Claude, Cli::Codex] {
        if require(&env, cli.name()).is_none() {
            continue;
        }
        let screen = Arc::new(Screen::default());
        let ptys = PtyRegistry::new();
        let id = format!("paste-{}", cli.name());
        ptys.spawn(
            &id,
            cli.name(),
            &[],
            Some(&dir.to_string_lossy()),
            &env,
            &[],
            100,
            30,
            Arc::clone(&screen) as Arc<dyn PtySink>,
            None,
        )
        .unwrap_or_else(|e| panic!("spawning {}: {e:#}", cli.name()));

        // The sequence comes with the first frame. Waiting on the property
        // rather than on a fixed sleep keeps this quick when it passes and
        // honest when it does not.
        let saw = wait_until(Duration::from_secs(30), || {
            let bytes = screen.bytes.lock().unwrap();
            bytes
                .windows(8)
                .any(|w| w == b"\x1b[?2004h")
        });
        ptys.kill_all();

        assert!(
            saw,
            "`{}` never enabled bracketed paste; a multi-line review sent into it \
             would arrive as one message per line",
            cli.name()
        );
        eprintln!("{}: bracketed paste on", cli.name());
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/* ------------------------------------------------------------------ */
/* Codex: the config, as Codex reads it                                */
/* ------------------------------------------------------------------ */

/// `-c` keeps an unparseable value as a literal string rather than failing,
/// so "Codex started" proves nothing about whether it understood us. Its own
/// `doctor` does: it loads the config and says whether it could.
#[test]
fn codex_loads_the_hook_config_this_app_passes() {
    let env = resolve_env();
    let Some(exe) = require(&env, "codex") else {
        return;
    };

    let args = hooks::codex_config_args("http://127.0.0.1:1/h/tok");
    let mut argv: Vec<&str> = vec!["doctor", "--json"];
    argv.extend(args.iter().map(String::as_str));
    let report = output(&exe, &argv, &env);

    let status = serde_json::from_str::<serde_json::Value>(&report)
        .ok()
        .and_then(|v| {
            v.get("checks")?
                .get("config.load")?
                .get("status")?
                .as_str()
                .map(String::from)
        });
    assert_eq!(
        status.as_deref(),
        Some("ok"),
        "codex could not load the config this app passes it.\n--- doctor ---\n{report}"
    );

    // The check is only worth having if it can fail: a value that is not a
    // hooks table has to come back as a refusal, or "ok" above means
    // nothing. (`doctor` reports auth failures too — this reads one check,
    // not the overall verdict.)
    let broken = output(
        &exe,
        &["doctor", "--json", "-c", "hooks.Stop=not a hook at all"],
        &env,
    );
    let broken_status = serde_json::from_str::<serde_json::Value>(&broken)
        .ok()
        .and_then(|v| {
            v.get("checks")?
                .get("config.load")?
                .get("status")?
                .as_str()
                .map(String::from)
        });
    assert_eq!(
        broken_status.as_deref(),
        Some("fail"),
        "codex accepts anything as a hooks table, so the check above proves nothing"
    );
}

/* ------------------------------------------------------------------ */
/* Codex: the whole pipeline                                           */
/* ------------------------------------------------------------------ */

#[derive(Default)]
struct Recorder {
    reports: Mutex<Vec<HookReport>>,
}

impl HookHandler for Recorder {
    fn on_hook(&self, report: HookReport) {
        eprintln!(
            "  hook: {:?} {:?} cwd={:?}",
            report.session_id, report.state, report.cwd
        );
        self.reports.lock().unwrap().push(report);
    }
}

fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// codex → our `-c` hooks → curl → our listener, end to end, on a real CLI.
///
/// `codex exec` fires `SessionStart` before it has an opinion about
/// credentials, so this measures the whole chain without an account. Two
/// things are being proved that no amount of reading the reference can:
/// that `$MAROL_SESSION_ID` reaches the hook expanded, and that the payload
/// on the hook's stdin reaches the request body where this app reads
/// `transcript_path` and `tool_name` from it.
///
/// `--dangerously-bypass-hook-trust` appears here and **nowhere in the
/// app**. Codex will not run an unreviewed hook, and a test cannot answer a
/// review prompt; a person can, once, in the terminal Marol puts in front of
/// them. Vetting the hook source is exactly what this test does.
#[test]
fn a_real_codex_reports_through_the_hooks_this_app_configures() {
    let env = resolve_env();
    let Some(exe) = require(&env, "codex") else {
        return;
    };

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let dir = std::env::temp_dir().join(format!("marol-codex-hooks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a working directory");

    let recorder = Arc::new(Recorder::default());
    let server = rt
        .block_on(hooks::start(&dir, Arc::clone(&recorder) as Arc<dyn HookHandler>))
        .expect("the hook listener");
    let _guard = rt.enter();

    let session_id = "parity-session-1";
    let mut argv: Vec<String> = vec![
        "exec".into(),
        "--skip-git-repo-check".into(),
        "--dangerously-bypass-hook-trust".into(),
    ];
    argv.extend(hooks::codex_config_args(&server.url()));
    argv.push("reply with PARITY-OK and use no tools".into());

    let mut child = std::process::Command::new(&exe)
        .args(&argv)
        .envs(&env.vars)
        .env("MAROL_SESSION_ID", session_id)
        .current_dir(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawning codex");

    let arrived = wait_until(Duration::from_secs(60), || {
        !recorder.reports.lock().unwrap().is_empty()
    });
    let _ = child.kill();
    let _ = child.wait();
    server.stop();

    let reports = recorder.reports.lock().unwrap();
    assert!(
        arrived,
        "no codex hook reached the listener in 60s; the chain is broken somewhere \
         between the `-c` arguments and the request body"
    );

    let start = reports
        .iter()
        .find(|r| r.state == HookState::Started)
        .expect("SessionStart is the first thing a session does");
    // The identity: expanded by the shell, not the name of the variable.
    // Only where a `$` means that — `cmd.exe` leaves it standing, which is
    // why the listener also knows the way home from a directory, and why
    // this half of the assertion is the one that runs everywhere.
    if cfg!(unix) {
        assert_eq!(
            start.session_id.as_deref(),
            Some(session_id),
            "the session id did not reach the hook expanded"
        );
    } else {
        assert!(
            start.session_id.as_deref() == Some(session_id) || start.cwd.is_some(),
            "the report can be placed on neither a session nor a directory"
        );
    }
    // And the payload, which is the whole reason the hook forwards stdin.
    assert!(
        start
            .transcript_path
            .as_deref()
            .is_some_and(|p| p.ends_with(".jsonl")),
        "no transcript path in the payload: {:?}",
        start.transcript_path
    );
    assert!(start.cwd.is_some(), "no cwd in the payload");

    let _ = std::fs::remove_dir_all(&dir);
}
