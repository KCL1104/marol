//! Transport-agnostic application core.
//!
//! A session is a real terminal running a real agent CLI. The core owns the
//! PTYs, the session list, its persistence, and the hook-reported status; it
//! knows nothing about Tauri and talks to the outside world through `UiSink`,
//! so the same core can later be driven by an axum websocket without being
//! rewritten.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent::{self, Cli, Ledger, Resume};
use crate::config;
use crate::hooks::{self, Activity, HookHandler, HookReport, HookServer, HookState};
use crate::i18n;
use crate::host::{self, Host, HostRef};
use crate::prompt::{self, Delivery};
use crate::pty::{self as pty, PtyRegistry, PtySink};
use crate::shell_env::{self, ShellEnv};
use crate::store::{
    Lifecycle, Outcome, PermissionMode, Profile, Store, StoredAttempt, StoredSession, StoredTab,
    StoredTask, StoredTree, TaskRepo,
};
use crate::worktree::{self, Worktrees};

pub trait UiSink: Send + Sync + 'static {
    fn emit(&self, event: &str, payload: serde_json::Value);
}

/// A new tab lets the window width decide how many columns to draw.
///
/// The core never interprets this string — arranging panes is entirely the
/// frontend's business, and a stored grid size is meaningless here. It is
/// spelled out rather than left empty only so a fresh tab round-trips through
/// the database as something the frontend recognises.
const DEFAULT_LAYOUT: &str = r#"{"mode":"auto","cols":"auto"}"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Saved, with no terminal attached right now.
    Saved,
    /// Still running, with nobody watching.
    ///
    /// Only reachable in a world that holds sessions: the app was closed,
    /// tmux kept the agent, and this start found it alive. Distinct from
    /// `Saved` because they are opposite facts — one says the work ended,
    /// the other says it did not — and a card that says "closed" over a
    /// running agent invites a second one onto the same worktree.
    ///
    /// Not `live`: no pty in *this* process is carrying it yet. Opening the
    /// session attaches to what is already there.
    Detached,
    /// Terminal is up; the agent has not reported anything yet.
    Starting,
    /// Sitting on the CLI's folder-trust prompt.
    ///
    /// Every attempt opens a worktree the agent has never seen, so every
    /// attempt starts here — both measured CLIs ask before working in a new
    /// directory. No hook can report it: nothing runs until the prompt is
    /// answered, `SessionStart` included. Measured — see
    /// `tests/prompt_injection.rs`.
    ///
    /// So the core sets it directly, which it can do honestly because it
    /// created the directory a moment earlier and knows this is its first
    /// launch. Without it the badge would miss the one state every new
    /// attempt begins in, and an auto-started queued attempt would look like
    /// it was running while it sat waiting for a keystroke.
    AwaitingTrust,
    /// The agent is working.
    Running,
    /// Blocked on a permission decision — it cannot continue without you.
    WaitingPermission,
    /// Idle long enough that the CLI raised an idle prompt. Claude Code
    /// does; Codex has no such event, so its sessions go to `Idle` and stay
    /// there rather than being given a state nothing can ever report.
    WaitingInput,
    /// Finished its turn; your move.
    Idle,
    Exited,
}

impl Status {
    /// Whether this state means a human is being waited on.
    pub fn needs_you(self) -> bool {
        matches!(
            self,
            Status::WaitingPermission | Status::WaitingInput | Status::AwaitingTrust
        )
    }

    fn from_hook(state: HookState) -> Self {
        match state {
            HookState::Started => Status::Running,
            HookState::Running => Status::Running,
            HookState::WaitingPermission => Status::WaitingPermission,
            HookState::WaitingInput => Status::WaitingInput,
            HookState::Idle => Status::Idle,
            HookState::Ended => Status::Exited,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionMeta {
    pub id: String,
    pub cwd: String,
    pub title: String,
    /// Which agent CLI this session runs: `claude`, `codex`, ...
    pub agent: String,
    pub status: Status,
    pub created_at: u64,
    pub last_active_at: u64,
    pub live: bool,
    /// True once the status plugin has reported at least once, so the UI can
    /// distinguish "idle" from "this CLI does not report status".
    pub reports_status: bool,
    /// Whether this session's CLI was actually wired for status when it
    /// launched.
    ///
    /// Not the same question as `reports_status`, and the difference is the
    /// whole point: that one says "it has spoken", this one says "it was
    /// given a mouth". A session with `hooks_wired` false will never report,
    /// ever, and the card can say so at once instead of waiting out a
    /// silence that has no end.
    ///
    /// Per session rather than per CLI because the answer is per *world*: a
    /// distro's own codex may be new enough while this machine's is not, and
    /// `host_env` probes each world's CLIs separately. A global "does codex
    /// report" would be a fact about the wrong computer.
    pub hooks_wired: bool,
    /// What the agent is doing right now, from the last `PreToolUse` report.
    pub activity: Option<Activity>,
    /// When that activity started, for an elapsed counter.
    pub activity_since: u64,
    /// Marked done by the user. Completion is a human judgement — an agent
    /// session never reports it, because `Stop` means "this turn ended", not
    /// "the work is finished".
    pub completed: bool,
    /// The attempt this session is running, or `None` for an ad-hoc session
    /// that lives outside the board.
    pub attempt_id: Option<String>,
    /// Whether this session runs an agent, as opposed to a run script or a
    /// worktree shell.
    ///
    /// Said rather than inferred. The CLI's name cannot answer it — `zsh` is
    /// a worktree shell and `aider` is an agent, and both are strings this
    /// desk was handed rather than a list it keeps. What actually separates
    /// them is a decision made at spawn: only an agent is ever given a tmux
    /// holder, because only an agent is a thing you started to *leave*
    /// running. This carries that decision out to whoever needs it.
    pub agent_session: bool,
    /// A message is queued to go in when this turn ends. Transient, like
    /// the PTY it waits on — never stored, false on every restore.
    pub has_followup: bool,
    /// The `$MAROL_PORT` a run script was handed, when the app can
    /// reach it (local and WSL; an SSH host's port lives on the remote).
    /// Transient like the followup flag: the server dies with the PTY, and
    /// a persisted port would be a column that lies after every restart.
    pub preview_port: Option<u16>,
    /// The conversation's token account, read off its transcript at each
    /// turn's end. In-memory: the transcript is the durable record, and a
    /// recompute is one read away.
    pub usage: Option<Usage>,
    /// Where that transcript lives, as the hook payload names it. Not
    /// serialized — a host-side path is plumbing, not something the UI
    /// renders.
    #[serde(skip)]
    pub transcript_path: Option<String>,
}

/// A session's token account. `context` is the last main-line request's
/// prompt size — where the next turn starts from; the other four are
/// cumulative across the conversation, sidechains included, because a
/// sub-agent's spend is real spend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub context: u64,
}

/// What quitting would do to the agents currently running, split by whether
/// something other than this app is holding them.
///
/// Two numbers rather than one because they are different facts and only one
/// of them is a cost. `held` agents are detached and handed back on the next
/// run; `lost` agents end. An update that restarts the desk has to be able to
/// say which it is about to do, and to whom.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RestartCost {
    /// Live agents a `tmux` in their own world will hand back.
    pub held: i64,
    /// Live agents that end with this process, because nothing is holding
    /// them — a world without `tmux`, native Windows chief among them.
    pub lost: i64,
}

/// One session's progress through its transcript.
#[derive(Debug, Clone, Copy, Default)]
struct UsageState {
    /// Bytes already consumed — always at a line boundary.
    offset: u64,
    acc: Usage,
}

/// What a stretch of transcript said about what it cost.
///
/// `account` is a delta for a per-message ledger and a running total for a
/// cumulative one, which is the whole reason the two are not the same
/// function: adding up Codex's rows would multiply the bill by the number of
/// turns, and taking Claude Code's last row would report the last message as
/// the whole session. `None` means the stretch said nothing about cost, and
/// what came before it stands.
struct Spend {
    account: Option<Usage>,
    /// The prompt the next turn grows from, when the stretch said.
    context: Option<u64>,
}

/// Read the usage in a stretch of transcript JSONL, the way this CLI writes
/// it down.
///
/// Rows that fail to parse are skipped, not fatal: one malformed line — a
/// half-flushed tail, most often — must not zero a session's account.
fn parse_usage(ledger: Ledger, text: &str) -> Spend {
    match ledger {
        Ledger::PerMessage => parse_usage_per_message(text),
        Ledger::Cumulative => parse_usage_cumulative(text),
    }
}

/// Claude Code: one row per assistant message, each carrying its own usage.
///
/// Returns the totals of every assistant row in the text, and the context
/// size of the last **main-line** one (`input + cache_read + cache_write`
/// ≈ the prompt the next turn will grow from). Sidechain rows count toward
/// the totals — their spend is real — but never set the context: a
/// sub-agent's prompt belongs to its own conversation.
fn parse_usage_per_message(text: &str) -> Spend {
    let mut sum = Usage::default();
    let mut context = None;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(u) = v.get("message").and_then(|m| m.get("usage")) else {
            continue;
        };
        let g = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        let (i, o, cr, cw) = (
            g("input_tokens"),
            g("output_tokens"),
            g("cache_read_input_tokens"),
            g("cache_creation_input_tokens"),
        );
        sum.input += i;
        sum.output += o;
        sum.cache_read += cr;
        sum.cache_write += cw;
        if v.get("isSidechain").and_then(|x| x.as_bool()) != Some(true) {
            context = Some(i + cr + cw);
        }
    }
    Spend {
        account: Some(sum),
        context,
    }
}

/// Codex: a `token_count` event whose `total_token_usage` is the running
/// total for the whole session, and whose `last_token_usage` is the request
/// that just went out.
///
/// So the last such row *is* the account, and the one arithmetic step is
/// splitting Codex's `input_tokens` into the three columns this app shows.
/// Codex counts the cached and cache-written parts **inside** `input_tokens`
/// rather than beside it, where Claude Code keeps three disjoint buckets —
/// so the fresh column is the remainder. Getting that backwards
/// double-counts the cache, which is most of a long session.
///
/// Subtracting is also the safe direction if a future Codex moves one of
/// those out of `input_tokens`: the three columns still add up to the prompt
/// it reported, and only the split between them would be off.
///
/// The context comes from `last_token_usage.input_tokens`: the prompt the
/// model was actually handed last time, cache and all, which is the same
/// quantity the Claude Code side reports.
fn parse_usage_cumulative(text: &str) -> Spend {
    let mut account = None;
    let mut context = None;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let payload = match v.get("payload") {
            Some(p) => p,
            // Older rollouts wrote the event at the top level.
            None => &v,
        };
        if payload.get("type").and_then(|t| t.as_str()) != Some("token_count") {
            continue;
        }
        let Some(info) = payload.get("info") else {
            // A `token_count` with no info at all is Codex saying the turn
            // used nothing it can attribute — not a reason to forget what
            // the rows before it said.
            continue;
        };
        let read = |slot: &str, key: &str| -> Option<u64> {
            info.get(slot)?.get(key)?.as_u64()
        };
        if let Some(input) = read("total_token_usage", "input_tokens") {
            let cached = read("total_token_usage", "cached_input_tokens").unwrap_or(0);
            // Newer field, and absent on some models even where it exists —
            // missing reads as zero, which is a column left empty rather
            // than a number nobody measured.
            let written = read("total_token_usage", "cache_write_input_tokens").unwrap_or(0);
            account = Some(Usage {
                // Saturating, because a report where the cache exceeds the
                // input is one to degrade on rather than panic on.
                input: input.saturating_sub(cached).saturating_sub(written),
                output: read("total_token_usage", "output_tokens").unwrap_or(0),
                cache_read: cached,
                cache_write: written,
                context: 0,
            });
        }
        if let Some(last) = read("last_token_usage", "input_tokens") {
            context = Some(last);
        }
    }
    Spend { account, context }
}

impl SessionMeta {
    fn to_stored(&self) -> StoredSession {
        StoredSession {
            id: self.id.clone(),
            cwd: self.cwd.clone(),
            title: self.title.clone(),
            agent: self.agent.clone(),
            created_at: self.created_at,
            last_active_at: self.last_active_at,
            archived: false,
            completed: self.completed,
            attempt_id: self.attempt_id.clone(),
        }
    }

    fn from_stored(s: StoredSession) -> Self {
        Self {
            id: s.id,
            cwd: s.cwd,
            title: s.title,
            agent: s.agent,
            status: Status::Saved,
            created_at: s.created_at,
            last_active_at: s.last_active_at,
            live: false,
            reports_status: false,
            hooks_wired: false,
            activity: None,
            activity_since: 0,
            completed: s.completed,
            attempt_id: s.attempt_id,
            // Not stored, and deliberately not given a column: the only
            // reader wants to know what a restart would end, and a restored
            // row is not live, so it is never asked. The one path that makes
            // one live again is `reopen_session`, which is the agent path.
            agent_session: true,
            has_followup: false,
            preview_port: None,
            usage: None,
            transcript_path: None,
        }
    }
}

/// Whether a status change is worth a line on the timeline.
///
/// `running` and `starting` are not: a run of tool calls already says the
/// agent was working, and a status line between each of them would bury them.
fn timeline_worthy(s: Status) -> bool {
    matches!(
        s,
        Status::WaitingPermission | Status::WaitingInput | Status::Idle | Status::Exited
    )
}

fn status_name(s: Status) -> &'static str {
    match s {
        Status::Saved => "saved",
        Status::Detached => "detached",
        Status::Starting => "starting",
        Status::AwaitingTrust => "awaiting_trust",
        Status::Running => "running",
        Status::WaitingPermission => "waiting_permission",
        Status::WaitingInput => "waiting_input",
        Status::Idle => "idle",
        Status::Exited => "exited",
    }
}

/// What goes at the end of a command line, after every option.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tail {
    /// Neither a prompt nor a subcommand — a plain launch.
    Nothing,
    /// The first message, as the positional argument.
    Prompt(String),
    /// A subcommand: `resume --last`. Everything that modifies the run has
    /// to be in front of it.
    Sub(&'static [&'static str]),
}

/// Assemble a command line: options first, then the hook wiring, then the
/// prompt or the subcommand.
///
/// Kept apart from spawning because the ordering is the whole point and it is
/// easy to undo by accident. A positional argument sitting in front of an
/// option leaves the parse to whatever the CLI happens to do with it, and the
/// symptom — a session that starts and then does nothing — looks like a dozen
/// other problems.
///
/// The subcommand goes last for a sharper reason, measured against the real
/// CLI: `codex resume` takes positionals of its own, `[SESSION_ID] [PROMPT]`.
/// Anything of ours that ended up after it would not be rejected — it would
/// be read as the name of a session to resume. Options before the
/// subcommand parse the same either way, so the order that cannot be
/// misread is the one to keep.
fn build_args(opts: Vec<String>, hook_args: Vec<String>, tail: Tail) -> Vec<String> {
    let mut args = opts;
    args.extend(hook_args);
    match tail {
        Tail::Nothing => {}
        Tail::Prompt(p) => args.push(p),
        Tail::Sub(words) => args.extend(words.iter().map(|w| w.to_string())),
    }
    args
}

/// The options and the tail for picking a conversation back up.
///
/// One function for both resume paths — a restart's and an archived row's —
/// because they are the same sentence said twice, and the version where they
/// drifted is the version where reopening an attempt loses its permission
/// mode on one of the two roads. The prompt is deliberately not re-sent on
/// either: a second copy would set the agent off doing the whole card again.
fn resume_line(cli: Option<Cli>, mode: PermissionMode) -> (Vec<String>, Tail) {
    let Some(cli) = cli else {
        // A CLI nobody measured is opened in the directory and left to its
        // own devices. Whether it can continue the conversation there is its
        // business, said honestly rather than guessed at.
        return (Vec::new(), Tail::Nothing);
    };
    let mut opts: Vec<String> = Vec::new();
    if let Resume::Option(words) = cli.resume() {
        opts.extend(words.iter().map(|w: &&str| w.to_string()));
    }
    opts.extend(cli.mode_args(mode).iter().map(|s: &&str| s.to_string()));
    let tail = match cli.resume() {
        Resume::Option(_) => Tail::Nothing,
        Resume::Subcommand(words) => Tail::Sub(words),
    };
    (opts, tail)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The CLIs whose names the dialogs always offer, profile or no profile.
///
/// A profile may not take one of these names: "claude" meaning something
/// other than `claude` is exactly the confusion names exist to prevent.
pub const BARE_AGENTS: [&str; 4] = ["claude", "codex", "gemini", "aider"];

/// One entry in a launch dialog's list: a bare agent, or a named profile.
#[derive(Debug, Clone, Serialize)]
pub struct Launcher {
    /// What the person picks — a bare agent's own name, or a profile's.
    pub name: String,
    /// The CLI it resolves to, so the dialog knows which conventions apply
    /// (prompt delivery, permission modes) without resolving anything itself.
    pub agent: String,
    /// True for a profile, so the list can say which entries are yours.
    pub profile: bool,
}

/// One instruction file an agent working in a directory will read.
///
/// `exists` is the honest half: a rules file that is not there is still worth
/// naming, because the question people actually have is "where does this go",
/// and an empty list answers it with silence.
#[derive(Debug, Clone, Serialize)]
pub struct AgentDoc {
    /// `global` (the machine's) or `project` (this checkout's).
    pub scope: &'static str,
    /// Which CLI reads it: `claude`, `codex`, `gemini`, or `shared` for the
    /// file all of them have agreed to look at.
    pub agent: &'static str,
    /// `rules` or `skill`.
    pub kind: &'static str,
    /// Which checkout this belongs to, for a session standing in a workspace
    /// that holds several. Empty when there is only one — and for everything
    /// `global`, which belongs to the machine rather than to a checkout.
    ///
    /// Without it a card spanning two repositories lists `CLAUDE.md` twice
    /// with nothing to tell the rows apart, which reads as a duplicate rather
    /// than as the two different files it is.
    pub dir: String,
    pub name: String,
    pub path: String,
    pub exists: bool,
}

/// The Claude Code release that added `--name` and cross-session messaging.
///
/// Handing `--name` to an older CLI stops it from starting at all, so the
/// installed version is measured once per launch of the app and the flag is
/// only used where it is known to be understood.
const NAMED_SESSIONS_SINCE: (u64, u64, u64) = (2, 1, 224);

/// Ask an installed CLI its version, bounded.
///
/// Best effort by design: a CLI that cannot answer in five seconds, or is not
/// installed at all, reads as "version unknown" — and unknown means every
/// version-gated flag stays off, the direction that never breaks a session.
async fn probe_version(env: &ShellEnv, agent: &str) -> Option<(u64, u64, u64)> {
    let exe = env.which(agent)?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("--version")
        .envs(&env.vars)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let out = tokio::time::timeout(std::time::Duration::from_secs(5), cmd.output())
        .await
        .ok()?
        .ok()?;
    parse_version(&String::from_utf8_lossy(&out.stdout))
}

/// `2.1.226 (Claude Code)` → `(2, 1, 226)`, `codex-cli 0.145.0` → `(0, 145, 0)`.
///
/// Measured against both CLIs' real output, which is why it looks at every
/// word rather than only the first: Claude Code leads with the number and
/// Codex leads with its own name, and a parser that only knew one of those
/// would report the other as "unknown" — which is a silent loss of every
/// version-gated feature, not a visible failure. Anything with no
/// three-part number in it anywhere stays unknown, never a guess.
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    s.split_whitespace().find_map(|word| {
        let mut parts = word.split('.');
        let a = parts.next()?.parse::<u64>().ok()?;
        let b = parts.next()?.parse::<u64>().ok()?;
        // A prerelease suffix is still that release's number: `0.145.0-rc.1`
        // is a 0.145.0, and reading it as unknown would turn every gated
        // feature off for anybody testing a release candidate.
        let third = parts.next()?;
        let digits: String = third.chars().take_while(char::is_ascii_digit).collect();
        Some((a, b, digits.parse().ok()?))
    })
}

/// Every measured CLI's version in one world, probed together.
///
/// One round trip per world rather than per CLI per session: the answer does
/// not change under a running app, and asking a WSL distro anything costs a
/// doorway crossing.
#[derive(Debug, Clone, Copy, Default)]
pub struct Versions {
    pub claude: Option<(u64, u64, u64)>,
    pub codex: Option<(u64, u64, u64)>,
}

impl Versions {
    fn of(&self, cli: Cli) -> Option<(u64, u64, u64)> {
        match cli {
            Cli::Claude => self.claude,
            Cli::Codex => self.codex,
        }
    }
}

/// One repository's setup script, waiting to wrap a launch. See
/// `Core::launch`.
struct SetupWrap {
    script: String,
    /// Where the script runs, relative to the session's own directory: empty
    /// for a one-repository attempt, whose directory *is* the checkout.
    dir: String,
    /// The repository the worktree was opened from — where untracked files
    /// worth copying (`.env`) live. Exposed as `MAROL_ROOT_PATH`.
    root_path: String,
}

/// Every setup script an attempt has to run before its agent starts, in
/// checkout order.
///
/// A card spanning a service and its client spans two `npm install`s, and
/// running only the first would start the agent in a workspace half of which
/// does not build. They are chained into one script rather than run
/// separately so the whole of it stays in front of the person, in the
/// session's own scrollback, exactly as one always was.
struct Setup {
    steps: Vec<SetupWrap>,
    /// The card's first repository — what the agent's own process inherits as
    /// `MAROL_ROOT_PATH`. Carried separately from the steps because the first
    /// repository need not have a setup script at all, and taking it from the
    /// first *step* would silently point the variable at the second
    /// repository whenever the first had nothing to run.
    root_path: String,
}

impl Setup {
    /// One `sh` script for the lot: each step in its own checkout, and — with
    /// `set -e` in front — the first failure stops the run there rather than
    /// letting a later step paper over it.
    ///
    /// `MAROL_ROOT_PATH` is re-exported per step, because it names *that*
    /// repository: a script copying `$MAROL_ROOT_PATH/.env` must land the
    /// client's env in the client and the service's in the service.
    fn script(&self) -> String {
        let mut out = String::new();
        for step in &self.steps {
            if step.dir.is_empty() {
                out.push_str(&format!(
                    "export MAROL_ROOT_PATH={0} AGENTDESK_ROOT_PATH={0}\n{1}\n",
                    host::sh_quote(&step.root_path),
                    step.script
                ));
            } else {
                // A subshell, so a step's `cd` cannot leak into the next one
                // and run the service's setup inside the client.
                out.push_str(&format!(
                    "(cd {} && export MAROL_ROOT_PATH={1} AGENTDESK_ROOT_PATH={1}\n{2}\n)\n",
                    host::sh_quote(&step.dir),
                    host::sh_quote(&step.root_path),
                    step.script
                ));
            }
        }
        out
    }

}

/// A file path the editable diff may touch: relative, and inside the
/// worktree. The paths normally come from the diff itself, but they arrive
/// through an invoke boundary — an absolute path or a `..` step would turn
/// "edit this attempt's file" into "write anywhere on the host".
fn ensure_worktree_relative(path: &str) -> Result<()> {
    let escapes = path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.split(['/', '\\']).any(|c| c == "..")
        // `C:...` — a Windows drive-absolute path has no leading slash.
        || path.as_bytes().get(1) == Some(&b':');
    if escapes {
        return Err(anyhow!("`{path}` is not a path inside the worktree"));
    }
    Ok(())
}

/// A run script's name as the drawer shows it: bare when the attempt has one
/// checkout, `<checkout>:<name>` when it has several. Two repositories with a
/// `dev` each would otherwise be two buttons saying the same word.
fn qualified_script(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}:{name}")
    }
}

/// The inverse: what this checkout would call the script the drawer pressed,
/// or `None` when the name belongs to a different one.
fn bare_script(dir: &str, qualified: &str) -> Option<String> {
    if dir.is_empty() {
        return Some(qualified.to_string());
    }
    qualified
        .strip_prefix(&format!("{dir}:"))
        .map(str::to_string)
}

/// A port nothing is listening on right now, for `MAROL_PORT`.
///
/// Asked of the kernel rather than counted up from a base, so two attempts'
/// dev servers never fight over 3000. The listener is dropped before the
/// script starts — the standard small race, accepted everywhere.
fn free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

/// Run the repository's archive script, bounded.
///
/// Best effort by design: the worktree is being taken back either way, and a
/// script that hangs must not hold the attempt open forever — thirty seconds
/// is long enough to stop a container and short enough to still feel like
/// "closing", and what happened is logged rather than swallowed.
fn run_archive(hr: &HostRef, script: &str, worktree: &str, root: &str) {
    use std::process::{Command, Stdio};
    let mut cmd = match hr.host {
        Host::Local => {
            if !cfg!(unix) {
                eprintln!("[core] archive scripts need a POSIX shell; skipped on this platform");
                return;
            }
            let sh = hr
                .env
                .which("sh")
                .unwrap_or_else(|| std::path::PathBuf::from("/bin/sh"));
            let mut c = Command::new(sh);
            c.args(["-c", script])
                .current_dir(worktree)
                .envs(&hr.env.vars)
                .env("MAROL_ROOT_PATH", root)
                .env("AGENTDESK_ROOT_PATH", root);
            c
        }
        // Inside a host the script's environment rides the argv, the same
        // way a launch's does.
        _ => {
            let envs = host::pty_env(
                hr.env,
                &under_both_names(vec![("MAROL_ROOT_PATH".to_string(), root.to_string())]),
            );
            let (outer, args, _) = hr.host.wrap(
                "sh",
                &["-c".to_string(), script.to_string()],
                Some(worktree),
                &envs,
            );
            let mut c = Command::new(hr.local.which(&outer).unwrap_or_else(|| outer.clone().into()));
            c.args(args);
            c
        }
    };
    let child = cmd.stdin(Stdio::null()).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[core] archive script failed to start: {e}");
            return;
        }
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    eprintln!("[core] archive script exited with {status}");
                }
                return;
            }
            Ok(None) if std::time::Instant::now() > deadline => {
                eprintln!(
                    "[core] archive script still running after 30s; killed so the worktree can go back"
                );
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(e) => {
                eprintln!("[core] archive script: {e}");
                return;
            }
        }
    }
}


/// One row on its way to an attempt's timeline.
#[derive(Debug)]
struct PendingEvent {
    attempt_id: String,
    at: u64,
    kind: &'static str,
    tool: Option<String>,
    detail: Option<String>,
}

/// Routes PTY output onto the UI bus and keeps session status in step.
/// Which notifications the desk raises, chosen in the environment panel.
///
/// Blocked states default on — a stuck agent is the one thing this app
/// exists to surface. A finished turn defaults off: every turn ends, and a
/// default that noisy would get the whole channel disabled at the OS.
///
/// `#[serde(default)]` so a settings row written by an older build (or a
/// future one with more fields) reads as "the defaults, plus what it said".
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct NotifyPrefs {
    /// A permission decision, or the folder-trust gate.
    pub permission: bool,
    /// The idle prompt — waiting on a reply.
    pub input: bool,
    /// A turn ended. Pairs with the unread dot in the interface.
    pub done: bool,
}

impl Default for NotifyPrefs {
    fn default() -> Self {
        Self {
            permission: true,
            input: true,
            done: false,
        }
    }
}

const NOTIFY_PREFS_KEY: &str = "notify_prefs";

/// Whether a Stop hook snapshots the worktree. Default on: the cost is a
/// stat walk per turn, and the payoff is the retreat that makes letting an
/// agent run affordable. The environment panel can turn it off.
const CHECKPOINTS_KEY: &str = "checkpoints_on";

struct Router {
    sink: Arc<dyn UiSink>,
    /// The same cell the core holds, so a notification never has to upgrade
    /// the weak core reference just to know what language to speak.
    locale: Arc<crate::i18n::LocaleCell>,
    /// Shared with the core the same way the locale is: written from a
    /// command once in a while, read on every hook.
    notify_prefs: Arc<Mutex<NotifyPrefs>>,
    sessions: Arc<Mutex<HashMap<String, SessionMeta>>>,
    /// Each wired session's own token for the two channels it can *ask* on.
    ///
    /// Shared with the core rather than reached through the weak reference,
    /// because it is read on every message and a `send` that raced the core's
    /// teardown should refuse rather than upgrade a dead pointer.
    ///
    /// Never on `SessionMeta`: that struct is serialised to the webview on
    /// every broadcast, and a token in it would be a token in the page.
    send_tokens: Arc<Mutex<HashMap<String, String>>>,
    /// Set once the core exists, so an exiting terminal can let the queue know
    /// a slot just came free. Weak, because the core owns this router.
    core: OnceLock<std::sync::Weak<Core>>,
    /// Timeline rows leave through here rather than being written inline.
    ///
    /// `on_hook` runs on the path that must never make an agent wait: a hook
    /// that hangs is a tool call that hangs. Writing to SQLite there would put
    /// a lock shared with every broadcast in the middle of it, on every single
    /// tool call. Handing the row to a writer thread costs a channel send.
    events: std::sync::mpsc::Sender<PendingEvent>,
}

impl Router {
    /// Whether this really is that session speaking.
    ///
    /// Constant-time is not the property that matters here — the token is a
    /// v4 uuid handed only to one local process's environment, and anything
    /// positioned to time this endpoint is already inside the machine. What
    /// matters is that it is checked at all: `sid` alone is a uuid a sibling
    /// session could read out of its own environment and reuse.
    fn token_ok(&self, session_id: &str, token: &str) -> bool {
        !token.is_empty()
            && self
                .send_tokens
                .lock()
                .unwrap()
                .get(session_id)
                .is_some_and(|t| t == token)
    }

    fn broadcast(&self) {
        let mut list: Vec<SessionMeta> = self.sessions.lock().unwrap().values().cloned().collect();
        list.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));
        let waiting = list.iter().filter(|s| s.status.needs_you()).count();
        if let Ok(v) = serde_json::to_value(&list) {
            self.sink.emit("sessions:changed", v);
        }
        self.sink
            .emit("badge", serde_json::json!({ "count": waiting }));
    }
}

impl PtySink for Router {
    fn on_output(&self, id: &str, data: String, seq: u64) {
        self.sink.emit(
            "term:output",
            serde_json::json!({ "id": id, "data": data, "seq": seq }),
        );
    }

    fn on_exit(&self, id: &str, status: String) {
        let freed = {
            let mut sessions = self.sessions.lock().unwrap();
            match sessions.get_mut(id) {
                Some(s) => {
                    s.status = Status::Exited;
                    s.live = false;
                    s.attempt_id.is_some()
                }
                None => false,
            }
        };
        self.sink
            .emit("term:exit", serde_json::json!({ "id": id, "status": status }));
        self.broadcast();

        // An attempt's terminal ending is the commonest way a slot comes
        // free, so it is the main thing that makes the queue move.
        if freed {
            if let Some(core) = self.core.get().and_then(|w| w.upgrade()) {
                core.drain_queue();
                core.emit_tasks();
            }
        }
    }
}

/// The longest a session's name may be.
///
/// It is read in a narrow sidebar column, at a glance, beside a dozen others
/// — the width at which a long name stops being information and becomes a
/// truncation. Generous enough that nobody writing a name meets it, low
/// enough that nobody pasting a paragraph gets one.
const MAX_TITLE: usize = 80;

/// A name as it goes on the row: one line, trimmed, bounded.
///
/// Repaired rather than refused, because half the names arriving here come
/// from an agent's `curl` rather than a person's keyboard, and a trailing
/// newline is not a reason to throw away a perfectly good name. Only an empty
/// one is refused, and that is the one case where there is nothing to keep.
fn clean_title(raw: &str) -> Option<String> {
    let one_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let kept: String = one_line
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_TITLE)
        .collect();
    let kept = kept.trim().to_string();
    (!kept.is_empty()).then_some(kept)
}

/// Which session a report belongs to.
///
/// The id is the answer whenever it survived the trip — a header for an
/// `http` hook, an expanded variable for a `command` one. When it did not,
/// the working directory is the way home: every payload of both CLIs carries
/// it, no shell rewrites it, and an attempt's worktree belongs to exactly one
/// session by construction.
///
/// Ambiguity is refused rather than resolved. Two live sessions in one
/// directory is a thing a person can do — two ad-hoc terminals in the same
/// checkout — and picking one of them would silently attribute a whole
/// session's work to the wrong card.
fn session_for(
    sessions: &HashMap<String, SessionMeta>,
    id: Option<&str>,
    cwd: Option<&str>,
) -> Option<String> {
    if let Some(id) = id.filter(|id| sessions.contains_key(*id)) {
        return Some(id.to_string());
    }
    let cwd = cwd?;
    let mut found = None;
    for s in sessions.values().filter(|s| s.live) {
        // The session's path carries its world (`wsl://Ubuntu/home/…`); the
        // agent inside that world only ever knew the plain one.
        if s.cwd == cwd || s.cwd.ends_with(cwd) {
            if found.is_some() {
                return None;
            }
            found = Some(s.id.clone());
        }
    }
    found
}

impl HookHandler for Router {
    fn on_hook(&self, report: HookReport) {
        let HookReport {
            session_id,
            cwd,
            state,
            activity,
            transcript_path,
        } = report;
        let status = Status::from_hook(state);
        let at = now_ms();
        let mut timeline: Vec<PendingEvent> = Vec::new();

        let (session_id, notify, turn_done) = {
            let mut sessions = self.sessions.lock().unwrap();
            // A hook from a session we cannot place: a stale terminal from a
            // previous run of the app, or two of them sharing a directory.
            // Ignore it rather than inventing a row for it or guessing which
            // row it meant.
            let Some(session_id) =
                session_for(&sessions, session_id.as_deref(), cwd.as_deref())
            else {
                return;
            };
            let Some(s) = sessions.get_mut(&session_id) else {
                return;
            };
            s.reports_status = true;
            s.last_active_at = at;
            // Where the token account lives. First report wins — the path
            // is stable for the life of the conversation.
            if let Some(tp) = transcript_path {
                s.transcript_path.get_or_insert(tp);
            }
            let attempt_id = s.attempt_id.clone();

            // Only a tool call carries activity. A Stop or Notification report
            // has none, and must not blank out what the agent last did.
            if let Some(next) = activity {
                if s.activity.as_ref() != Some(&next) {
                    s.activity_since = at;
                }
                // Every tool call is its own moment on the timeline, including
                // a repeat of the one before it. The card shows the latest;
                // the timeline is the record.
                if let Some(id) = attempt_id.clone() {
                    timeline.push(PendingEvent {
                        attempt_id: id,
                        at,
                        kind: "tool",
                        tool: Some(next.tool.clone()),
                        detail: Some(next.detail.clone()),
                    });
                }
                s.activity = Some(next);
            }

            // Status goes on the timeline only when it actually changes, and
            // only for the states worth reading back later. `running` is
            // already implied by the tool call that carried it.
            let changed = s.status != status;
            if changed {
                if let (Some(id), true) = (attempt_id, timeline_worthy(status)) {
                    timeline.push(PendingEvent {
                        attempt_id: id,
                        at,
                        kind: "status",
                        tool: None,
                        detail: Some(status_name(status).to_string()),
                    });
                }
            }

            // Only announce a transition *into* needing a human, so a session
            // that reports the same state twice does not notify twice. A
            // turn ending (Stop → idle) is its own class, off by default —
            // and each class answers to its toggle in the environment panel.
            let entering = status.needs_you() && !s.status.needs_you();
            let turn_done = status == Status::Idle && s.status != Status::Idle;
            s.status = status;
            let prefs = *self.notify_prefs.lock().unwrap();
            let fire = if entering {
                match status {
                    Status::WaitingPermission | Status::AwaitingTrust => prefs.permission,
                    _ => prefs.input,
                }
            } else if turn_done {
                prefs.done
            } else {
                false
            };
            (
                session_id,
                fire.then(|| (s.title.clone(), s.cwd.clone())),
                turn_done,
            )
        };

        for e in timeline {
            // A full or closed channel must not stall the agent. Losing a
            // timeline row is a gap in a record; blocking here is a stuck
            // tool call.
            let _ = self.events.send(e);
        }

        if let Some((title, cwd)) = notify {
            let locale = self.locale.get();
            let body = match status {
                Status::WaitingPermission => crate::i18n::waiting_permission(locale),
                Status::AwaitingTrust => crate::i18n::awaiting_trust(locale),
                Status::Idle => crate::i18n::turn_done(locale),
                _ => crate::i18n::waiting_input(locale),
            };
            self.sink.emit(
                "notify",
                serde_json::json!({
                    "title": format!("{title} {body}"),
                    "body": cwd,
                    "sessionId": session_id.clone(),
                }),
            );
        }

        self.broadcast();

        // The queued follow-up's moment: a turn just ended, and whatever
        // waited for it goes in as the next one. The same moment is the
        // checkpoint's — the worktree is quiet, so a snapshot has no tear
        // race — and the snapshot leaves the hook path immediately.
        if turn_done {
            if let Some(core) = self.core.get().and_then(|w| w.upgrade()) {
                core.flush_followup(&session_id);
                core.snapshot_after_turn(&session_id);
                core.usage_after_turn(&session_id);
            }
        }
    }

    /// Who else is on this desk, for a session that wants to write to one.
    ///
    /// Live agent sessions only, and never the asker itself — a list holding
    /// your own address invites a loop, and a shell or a run script is not
    /// something to address. Plain text, one per line, because the sender is
    /// a shell one-liner and `cut -f1` is the whole parser it should need.
    fn on_peers(&self, session_id: &str, token: &str) -> Option<String> {
        if !self.token_ok(session_id, token) {
            return None;
        }
        let sessions = self.sessions.lock().unwrap();
        let mut rows: Vec<String> = sessions
            .values()
            .filter(|s| s.id != session_id && s.live && s.agent_session)
            .map(|s| format!("{}\t{}\t{}", s.id, s.title, status_name(s.status)))
            .collect();
        rows.sort();
        Some(rows.join("\n") + if rows.is_empty() { "" } else { "\n" })
    }

    /// One session writing to another.
    ///
    /// Queued rather than typed straight in, always: the target may be
    /// mid-turn, and a paste landing in the middle of one steers it instead
    /// of answering it. A target that is *not* mid-turn has its queue flushed
    /// at once, so a message to an idle agent arrives now rather than waiting
    /// for a turn that may never come.
    ///
    /// Every refusal names itself, because the sender is an agent that can
    /// act on the answer — which is the whole reason this returns a reason
    /// rather than dropping the message.
    fn on_send(&self, session_id: &str, token: &str, to: &str, text: &str) -> Result<(), String> {
        if !self.token_ok(session_id, token) {
            return Err("not this session's token".to_string());
        }
        if session_id == to {
            return Err("a session cannot message itself".to_string());
        }
        let (from, target_idle) = {
            let sessions = self.sessions.lock().unwrap();
            let from = sessions
                .get(session_id)
                .ok_or_else(|| "the sending session is not on this desk".to_string())?
                .title
                .clone();
            let target = sessions
                .get(to)
                .ok_or_else(|| format!("no session here with id {to}"))?;
            if !target.live {
                return Err(format!("「{}」 has no terminal any more", target.title));
            }
            (
                from,
                !matches!(target.status, Status::Running | Status::Starting),
            )
        };
        let core = self
            .core
            .get()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| "this desk is shutting down".to_string())?;
        core.enqueue_followup(to, text, Some(from))
            .map_err(|e| format!("{e:#}"))?;
        if target_idle {
            core.flush_followup(to);
        }
        Ok(())
    }

    /// A session saying what it should be called.
    ///
    /// The rename writes to SQLite, and the iron law of this path is that an
    /// agent never waits on us — so it leaves for a thread, as everything
    /// that touches the disk from here does. A rename is a once-a-session
    /// event, so a thread apiece costs nothing worth measuring.
    ///
    /// A name for a session this desk does not have is dropped, quietly and
    /// on purpose: it is a terminal from a previous run of the app, and
    /// inventing a row for it would be worse than losing a name.
    fn on_name(&self, report: hooks::NameReport) {
        let Some(core) = self.core.get().and_then(|w| w.upgrade()) else {
            return;
        };
        std::thread::spawn(move || {
            if let Err(e) = core.rename_session(&report.session_id, &report.name) {
                eprintln!("[core] a session could not name itself: {e:#}");
            }
        });
    }
}

/// A card as the board needs it: the row, its attempts, and which session
/// each attempt is running in right now.
#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    #[serde(flatten)]
    pub task: StoredTask,
    pub attempts: Vec<AttemptView>,
    /// Where this card sits in the start queue, counting from 1, when every
    /// slot was taken at the moment 開始 was pressed.
    pub queued_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttemptView {
    #[serde(flatten)]
    pub attempt: StoredAttempt,
    /// `None` once the attempt's session has been archived out from under it.
    pub session_id: Option<String>,
}

/// How many attempts may hold a terminal at once, before anyone says.
///
/// The product is an attention scheduler, and the thing actually being
/// rationed is a person. Three is about as many TUIs as one human can keep a
/// thread on.
const DEFAULT_MAX_CONCURRENT: i64 = 3;
const MAX_CONCURRENT_KEY: &str = "max_concurrent";

/// What pressing 開始 did.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartResult {
    /// Set when there was room and it started now.
    pub attempt: Option<OpenedAttempt>,
    /// Set when there was not: where it sits in the queue, counting from 1.
    pub queued_at: Option<i64>,
}

/// What opening an attempt produced.
#[derive(Debug, Clone, Serialize)]
pub struct OpenedAttempt {
    pub attempt_id: String,
    pub session_id: String,
    pub branch: String,
    pub worktree_path: String,
    /// The prompt as it was sent — or as it was built, when the agent's
    /// conventions are unknown and it is waiting to be pasted in.
    pub prompt: String,
    /// False when this CLI is one whose argument conventions have not been
    /// measured. The session is real either way; only the first message is
    /// the person's to deliver.
    pub prompt_sent: bool,
}

pub struct Core {
    pub env: ShellEnv,
    /// Language for the strings the OS renders, pushed down by the webview.
    /// Shared with the router, which raises the notifications.
    pub locale: Arc<crate::i18n::LocaleCell>,
    store: Arc<Store>,
    ptys: PtyRegistry,
    sessions: Arc<Mutex<HashMap<String, SessionMeta>>>,
    tabs: Mutex<Vec<StoredTab>>,
    sink: Arc<dyn UiSink>,
    router: Arc<Router>,
    hooks: OnceLock<HookServer>,
    /// Where the database is, kept so the pre-update snapshot can be put
    /// beside it. `data_dir` is its parent today and the two would be the
    /// same answer, but only one of them is the file being copied.
    db_path: std::path::PathBuf,
    data_dir: std::path::PathBuf,
    worktrees: Worktrees,
    /// The installed CLIs' versions, measured once at startup. `None` for
    /// one of them means unknown, and unknown keeps every version-gated flag
    /// off.
    versions: Versions,
    /// Which notifications to raise — the router's copy of the same cell.
    notify_prefs: Arc<Mutex<NotifyPrefs>>,
    /// Each attempt's worktree shell, while one is live. One shell per
    /// attempt — asking again returns the session already there, so the
    /// button is idempotent and the shells never pile up. In-memory only:
    /// a shell does not outlive the app any more than the PTYs do.
    shells: Mutex<HashMap<String, String>>,
    /// Messages held for the end of a session's turn. Typing into a running
    /// claude steers the turn in flight; this is the other thing a person
    /// means — "when you are done, then this". Like the shells it is
    /// transient: the turn it waits on cannot outlive the app either.
    ///
    /// A queue rather than the single slot it used to be. One slot was right
    /// while the only sender was the person in front of it, where a second
    /// message plainly supersedes the first. It stops being right the moment
    /// another *session* can send one: two peers writing to the same agent
    /// would have had the second silently evict the first, with neither
    /// sender told and no trace that a message ever existed.
    followups: Mutex<HashMap<String, VecDeque<Pending>>>,
    /// Per-session tokens for the peers/send channels. See `Router`.
    send_tokens: Arc<Mutex<HashMap<String, String>>>,
    /// Everything known about each execution environment, resolved on first
    /// use and kept: a WSL distro's login environment costs a probe, and the
    /// answer does not change under a running app.
    hosts: Mutex<HashMap<Host, Arc<HostEnv>>>,
    /// Whether the end of a turn snapshots the worktree (see
    /// `CHECKPOINTS_KEY`).
    checkpoints_on: Mutex<bool>,
    /// Attempts with a snapshot in flight. Two Stops racing — or a manual
    /// click during one — would compute the same ordinal and fight over the
    /// temp index; the second caller finds the flag and leaves.
    checkpointing: Mutex<std::collections::HashSet<String>>,
    /// Per-session progress through its transcript: the byte already
    /// consumed and the totals so far, so each turn's read costs only what
    /// the turn wrote. In-memory like the transcript path itself — the
    /// JSONL is the durable record, and a cached copy that survives a
    /// restart is a cache that lies after one.
    usage_state: Mutex<HashMap<String, UsageState>>,
    /// What tells this machine apart from another with the same data
    /// directory. Only remote socket names need it, so it is read — and on a
    /// first run, written — the first time one is asked for.
    machine_id: OnceLock<String>,
}

/// What a resume did. `restore_error` set means the worktree is back on
/// its branch but the shelf checkpoint did not come down cleanly — half
/// done and visible, retryable from the timeline, never rolled back.
#[derive(Debug, Clone, Serialize)]
pub struct Resumed {
    pub session_id: String,
    pub restore_error: Option<String>,
}

/// The worlds a card can live in, enumerated — never invented. WSL comes
/// from `wsl.exe -l -q`, SSH from the aliases the person already wrote
/// into `~/.ssh/config`; an empty list is an honest "none here".
#[derive(Debug, Clone, Serialize)]
pub struct Worlds {
    pub wsl: Vec<String>,
    pub ssh: Vec<String>,
}

/// What asking a world "are you there, and which agents do you have" found.
/// A version of `None` with no error is itself an answer: reachable, but
/// that CLI is not on this world's login-shell PATH.
#[derive(Debug, Clone, Serialize)]
pub struct WorldProbe {
    pub claude: Option<String>,
    pub codex: Option<String>,
    pub error: Option<String>,
}

/// List the current directory: where it really is, then its subdirectories.
///
/// Run with the target as the process's working directory, so nothing about
/// the path is interpolated into this text — see `Core::list_dir`.
///
/// `pwd -P` first, on its own line, because the answer to "where am I" is a
/// fact only the world can supply: `~`, a symlink, and a relative step all
/// arrive here as something else, and a picker that echoed back what it was
/// asked for would build its next path on a guess.
///
/// The `case` skips `.` and `..` rather than a `find` with `-maxdepth`,
/// whose `-printf` is GNU-only — this has to run on a BSD userland over SSH
/// as readily as on a WSL Ubuntu. An empty directory leaves the globs
/// unexpanded and `[ -d ]` discards the literals, which is why there is no
/// `nullglob` here to depend on.
const LIST_DIR: &str = r#"pwd -P
for e in .* *; do
  case "$e" in .|..) continue;; esac
  [ -d "$e" ] && printf '%s\n' "$e"
done"#;

/// Where `..` goes from an absolute path, or `None` at a root.
///
/// String work rather than `Path`: this answers for the world the path came
/// from, not the one this process runs on, and a `PathBuf` on Windows would
/// join a WSL path with backslashes — the same trap `worktree.rs` documents.
fn parent_of(path: &str) -> Option<String> {
    let sep = if path.contains('\\') && !path.starts_with('/') {
        '\\'
    } else {
        '/'
    };
    let trimmed = path.trim_end_matches(sep);

    // Two roots, and neither has anywhere above it: `/`, which trims away to
    // nothing, and a Windows drive letter, which trims to `C:`. Both must
    // answer `None` rather than a `..` that walks in a circle.
    if trimmed.is_empty() || is_drive_root(trimmed) {
        return None;
    }

    match trimmed.rsplit_once(sep) {
        // The last step out of a drive: `C:\Users` → `C:\`, keeping the
        // separator, because `C:` alone means "wherever that drive last was"
        // to Windows rather than its root.
        Some((head, _)) if is_drive_root(head) => Some(format!("{head}{sep}")),
        // The last step out of a POSIX tree: `/home` → `/`.
        Some(("", _)) => Some(sep.to_string()),
        Some((head, _)) => Some(head.to_string()),
        None => None,
    }
}

/// `C:` — a drive letter and a colon, and nothing else.
fn is_drive_root(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// One directory in some world, as a folder picker needs it.
///
/// The picker exists because the platform's own dialog cannot answer this
/// question for two of the three worlds. A native dialog browses the machine
/// the app is running on: it can be pointed at `\\wsl$\<distro>` and made to
/// work, slowly, through Explorer's idea of a filesystem — and it has nothing
/// at all to say about an SSH host, where there is no local mount to browse.
/// So the desk asks the world itself, which is one code path for all three
/// instead of one that half-works and one that cannot exist.
#[derive(Debug, Clone, Serialize)]
pub struct DirListing {
    /// The path actually listed, absolute and symlink-resolved by the world
    /// itself rather than guessed at from the string that was asked for.
    pub path: String,
    /// Where `..` goes, or `None` at the root.
    pub parent: Option<String>,
    /// Subdirectory names, sorted, dotfiles last. Names only — the caller
    /// joins them, because only the world knows what its separator is.
    pub dirs: Vec<String>,
    /// Whether this directory is itself a git repository. The picker is
    /// almost always looking for one, and saying so where it stands beats
    /// making somebody descend to find out.
    pub is_repo: bool,
}

/// Both sides of one file in an attempt's diff, as full text — the data
/// model an editable diff needs, where a patch string cannot be edited.
/// `base` is `None` for a file the attempt created; `work` is `None` for
/// one it deleted.
#[derive(Debug, Clone, Serialize)]
pub struct AttemptFile {
    pub base: Option<String>,
    pub work: Option<String>,
}

/// What a restore did: where the worktree now stands, and the automatic
/// "now" checkpoint kept first so the restore itself can be reverted.
#[derive(Debug, Clone, Serialize)]
pub struct Restored {
    /// The checkpoint the worktree now matches — `0` is the attempt's base.
    pub to_n: u64,
    pub to_sha: String,
    /// `None` when nothing had changed since the last checkpoint.
    pub saved: Option<crate::worktree::Checkpoint>,
}

/// One execution environment, resolved: its login environment, its agent
/// CLIs, and where its worktrees live. The local one mirrors the core's own
/// fields; a WSL distro's is probed through `wsl.exe` on first contact.
pub struct HostEnv {
    pub host: Host,
    pub env: ShellEnv,
    /// This world's CLIs, not ours. A WSL distro has its own claude and its
    /// own codex, at its own versions, and the flags a launch may use are
    /// decided by the binary that will actually run.
    pub versions: Versions,
    /// `~/.marol/worktrees` *inside the host* — a worktree lives in the
    /// same filesystem as its repository, never across a boundary.
    pub worktree_root: String,
    /// How this host's agents reach the status listener: the plugin
    /// directory for the CLI that loads one, the URL for the CLI that is
    /// configured with it. The app's own dir locally, the same dir through
    /// `/mnt` for WSL, a remotely provisioned copy (URL pointing back
    /// through the tunnel) for SSH. `None` when the hook listener is down or
    /// the tunnel could not be raised — sessions run either way, they just
    /// show no status.
    pub hooks: Option<hooks::Wiring>,
    /// What it takes to hold a session in this world past the app's own life,
    /// or `None` when this world cannot: no tmux in it, or nowhere to put the
    /// config. Resolved beside the environment probe, because both answer
    /// "what can this world do", both cost a round trip into it, and neither
    /// should cost one per session.
    pub hold: Option<WorldHold>,
}

/// What one world takes to hold a session, once the world has been asked.
#[derive(Debug, Clone)]
pub struct WorldHold {
    /// The config `-f` points at, spelled the way that world spells paths.
    ///
    /// Never optional, and never the person's own `~/.tmux.conf`. tmux does
    /// not complain about a `-f` file that is not there — it starts with its
    /// defaults, status line and all — so a config this app failed to write
    /// is not an error anyone would see, it is tmux quietly drawing over the
    /// agent's terminal. Hence: no config, no hold.
    conf: String,
    /// Where the socket files go, when this process cannot see the world's
    /// own tmux directory. `None` for the local world, where `-L` and tmux's
    /// answer are both available and already in use.
    socket_dir: Option<String>,
}

/// What holding one session in one world takes.
///
/// Three strings rather than a command, because the command has to be built
/// twice — once to start or reattach, once to end — and both have to go
/// through the world's doorway on the way out.
/// One message waiting for a session's turn to end.
///
/// `from` is the whole difference between a person's follow-up and a peer's
/// message: absent means the human typed it and it carries their authority,
/// present means another session sent it and `prompt::peer_envelope` has to
/// say so before it goes in.
#[derive(Debug, Clone)]
struct Pending {
    text: String,
    from: Option<String>,
}

impl Pending {
    /// What actually goes into the terminal for this one.
    fn rendered(&self) -> String {
        match &self.from {
            Some(from) => crate::prompt::peer_envelope(from, &self.text),
            None => self.text.clone(),
        }
    }
}

/// How many messages may wait on one session before the desk starts refusing
/// them.
///
/// A bound rather than a preference: the queue is drained by a turn ending,
/// and a session that never ends a turn would otherwise collect messages for
/// as long as the app runs. Refusing loudly at a limit is the honest failure
/// — the alternative this replaced dropped the *older* message and told
/// nobody.
const MAX_PENDING: usize = 16;

struct HoldPlan {
    /// Which socket, and in which of the two shapes. Carries the desk and the
    /// session, so two installs cannot collect each other's.
    socket: pty::Socket,
    /// `-f`. Never the user's own `~/.tmux.conf`: their prefix key, their
    /// status line and their bindings belong to their terminal, not to a
    /// process this app is only babysitting.
    conf: String,
    /// Where the socket file lands, when that is on this machine. `None` in
    /// another world, whose filesystem this process cannot reach: there, the
    /// destroy command unlinks it.
    socket_file: Option<String>,
}

impl HostEnv {
    /// The pair of environments everything that executes needs.
    fn hr<'a>(&'a self, local: &'a ShellEnv) -> HostRef<'a> {
        HostRef {
            host: &self.host,
            local,
            env: &self.env,
        }
    }
}

impl Core {
    pub async fn start(
        sink: Arc<dyn UiSink>,
        db_path: std::path::PathBuf,
        data_dir: std::path::PathBuf,
    ) -> Result<Arc<Self>> {
        let env = shell_env::resolve().await;
        Self::start_with(env, sink, db_path, data_dir, Worktrees::default_root()).await
    }

    /// Start against a given environment and worktree root.
    ///
    /// The seam exists so the whole core can be driven without touching the
    /// person's home directory or their real agent — and so the worktree root
    /// can become a setting later without moving anything.
    pub async fn start_with(
        env: ShellEnv,
        sink: Arc<dyn UiSink>,
        db_path: std::path::PathBuf,
        data_dir: std::path::PathBuf,
        worktree_root: std::path::PathBuf,
    ) -> Result<Arc<Self>> {
        let store = Arc::new(Store::open(&db_path)?);

        let restored: HashMap<String, SessionMeta> = store
            .list_sessions()
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.id.clone(), SessionMeta::from_stored(s)))
            .collect();
        eprintln!(
            "[core] restored {} sessions from {}",
            restored.len(),
            db_path.display()
        );

        // Slots can name sessions that were archived between runs; drop them
        // so a restored tab never points at something the sidebar has no row
        // for.
        let known: std::collections::HashSet<String> = restored.keys().cloned().collect();
        let mut tabs = store.list_tabs().unwrap_or_default();
        for t in &mut tabs {
            t.slots
                .retain(|s| s.as_ref().is_some_and(|id| known.contains(id)));
        }
        if tabs.is_empty() {
            let first = StoredTab {
                id: uuid::Uuid::new_v4().to_string(),
                name: crate::i18n::default_tab_name(crate::i18n::Locale::default()).to_string(),
                layout: DEFAULT_LAYOUT.to_string(),
                slots: Vec::new(),
                position: 0,
            };
            let _ = store.upsert_tab(&first);
            tabs.push(first);
        }

        let sessions = Arc::new(Mutex::new(restored));

        // Timeline writes leave the hook path here. A plain thread rather than
        // a task: the work is a blocking SQLite insert, and the point of the
        // hand-off is to keep that off the path an agent is waiting on.
        let (events_tx, events_rx) = std::sync::mpsc::channel::<PendingEvent>();
        let writer_store = Arc::clone(&store);
        std::thread::spawn(move || {
            // Ends when the last sender drops, which is when the core goes.
            for e in events_rx {
                if let Err(err) = writer_store.append_event(
                    &e.attempt_id,
                    e.at,
                    e.kind,
                    e.tool.as_deref(),
                    e.detail.as_deref(),
                ) {
                    eprintln!("[core] timeline write failed: {err}");
                }
            }
        });

        let locale = Arc::new(crate::i18n::LocaleCell::default());

        // A malformed row reads as the defaults — same contract as the
        // profiles: a bad setting must not keep the app from starting.
        let notify_prefs = Arc::new(Mutex::new(
            store
                .setting(NOTIFY_PREFS_KEY)
                .ok()
                .flatten()
                .and_then(|raw| serde_json::from_str(&raw).ok())
                .unwrap_or_default(),
        ));

        // Same malformed-row contract as the notify prefs; absent means the
        // default, which is on.
        let checkpoints_on = store
            .setting(CHECKPOINTS_KEY)
            .ok()
            .flatten()
            .map(|raw| raw != "0")
            .unwrap_or(true);

        // Both at once: neither answer depends on the other, and a machine
        // with both installed should not pay for them one after the next on
        // the path to the first paint.
        let (claude, codex) = tokio::join!(
            probe_version(&env, "claude"),
            probe_version(&env, "codex")
        );
        let versions = Versions { claude, codex };
        for (name, v) in [("claude", claude), ("codex", codex)] {
            if let Some((a, b, c)) = v {
                eprintln!("[core] {name} {a}.{b}.{c}");
            }
        }

        let send_tokens: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
        let router = Arc::new(Router {
            sink: Arc::clone(&sink),
            locale: Arc::clone(&locale),
            notify_prefs: Arc::clone(&notify_prefs),
            sessions: Arc::clone(&sessions),
            send_tokens: Arc::clone(&send_tokens),
            core: OnceLock::new(),
            events: events_tx,
        });

        let core = Arc::new(Self {
            env,
            locale,
            store,
            ptys: PtyRegistry::new(),
            sessions,
            send_tokens,
            tabs: Mutex::new(tabs),
            sink: Arc::clone(&sink),
            router: Arc::clone(&router),
            hooks: OnceLock::new(),
            db_path: db_path.clone(),
            data_dir: data_dir.clone(),
            worktrees: Worktrees::new(worktree_root),
            versions,
            notify_prefs,
            shells: Mutex::new(HashMap::new()),
            followups: Mutex::new(HashMap::new()),
            hosts: Mutex::new(HashMap::new()),
            checkpoints_on: Mutex::new(checkpoints_on),
            checkpointing: Mutex::new(std::collections::HashSet::new()),
            usage_state: Mutex::new(HashMap::new()),
            machine_id: OnceLock::new(),
        });

        // Status reporting is a nicety: if the listener cannot bind, sessions
        // still run, they just show no status.
        match hooks::start(&data_dir, Arc::clone(&router) as Arc<dyn HookHandler>).await {
            Ok(server) => {
                let _ = core.hooks.set(server);
            }
            Err(e) => eprintln!("[core] status hooks unavailable: {e:#}"),
        }

        let _ = router.core.set(Arc::downgrade(&core));

        // Before the first paint: whatever tmux kept running is not "closed".
        core.mark_detached();

        core.broadcast();
        core.emit_tabs();
        core.emit_tasks();
        // Every terminal died with the last run, so anything that was waiting
        // for a slot has one now.
        core.drain_queue();

        // Crash leftovers: checkpoint refs whose attempt is no longer open.
        // Off the startup path — it is git work across every known repo.
        {
            let core = Arc::clone(&core);
            std::thread::spawn(move || core.sweep_checkpoint_orphans());
        }

        // The other kind of leftover, and one this desk created by asking
        // tmux to hold things: a held session whose card is gone.
        {
            let core = Arc::clone(&core);
            std::thread::spawn(move || core.sweep_held_orphans());
        }

        // The same two questions for every other world, where asking costs a
        // probe and so cannot happen before the board is on screen.
        {
            let core = Arc::clone(&core);
            std::thread::spawn(move || core.visit_remote_holds());
        }
        Ok(core)
    }

    /* ---------------------------- hosts ---------------------------- */

    /// The resolved environment for one host, probed on first contact.
    ///
    /// Resolution happens outside the map lock: a WSL probe takes a moment,
    /// and nothing else should queue behind it. Two first contacts racing
    /// probe twice and the second insert wins — wasteful once, wrong never.
    fn host_env(&self, h: &Host) -> Result<Arc<HostEnv>> {
        if let Some(he) = self.hosts.lock().unwrap().get(h) {
            return Ok(Arc::clone(he));
        }
        let he = Arc::new(match h {
            Host::Local => HostEnv {
                host: Host::Local,
                env: self.env.clone(),
                versions: self.versions,
                worktree_root: self.worktrees.local_root(),
                hooks: self.hooks.get().map(|s| hooks::Wiring {
                    plugin_dir: s.plugin_dir.to_string_lossy().to_string(),
                    url: s.url(),
                }),
                hold: self.local_hold(),
            },
            _ => {
                let env = h.probe_env(&self.env)?;
                let home = env.vars.get("HOME").cloned().ok_or_else(|| {
                    anyhow!("the host's environment came back without a HOME")
                })?;
                // The host's CLIs, not ours — their versions gate their flags.
                let hr = HostRef {
                    host: h,
                    local: &self.env,
                    env: &env,
                };
                let probe = |exe: &str| {
                    hr.run_ok(exe, &["--version"], None)
                        .ok()
                        .and_then(|s| parse_version(&s))
                };
                let versions = Versions {
                    claude: probe("claude"),
                    codex: probe("codex"),
                };
                let wiring = match h {
                    Host::Local => unreachable!(),
                    // The plugin sits on the app's disk; an agent inside WSL
                    // reads it through the drive mounts, and posts to the
                    // same loopback URL this machine is listening on.
                    Host::Wsl { .. } => self.hooks.get().map(|s| hooks::Wiring {
                        plugin_dir: host::win_path_for_wsl(&s.plugin_dir.to_string_lossy()),
                        url: s.url(),
                    }),
                    // An SSH host cannot see our disk at all: the plugin is
                    // provisioned into the host, and its URL points back
                    // through the reverse tunnel on the standing connection.
                    Host::Ssh { host } => self.hooks.get().and_then(|server| {
                        let Some(remote_port) = host::open_ssh_master(
                            &self.env,
                            host,
                            &self.tunnel_ports(host),
                            server.port,
                        ) else {
                            return None;
                        };
                        self.remember_tunnel(host, remote_port);
                        let url =
                            format!("http://127.0.0.1:{remote_port}/h/{}", server.token);
                        let dir = format!("{home}/.marol/plugin");
                        for (rel, contents) in hooks::plugin_files(&url) {
                            if let Err(e) = hr.write_file(&format!("{dir}/{rel}"), &contents) {
                                eprintln!("[core] provisioning hooks on `{host}` failed: {e:#}");
                                return None;
                            }
                        }
                        Some(hooks::Wiring {
                            plugin_dir: dir,
                            url,
                        })
                    }),
                };
                let hold = world_hold(&hr, &home);
                HostEnv {
                    host: h.clone(),
                    env,
                    versions,
                    worktree_root: format!("{home}/.marol/worktrees"),
                    hooks: wiring,
                    hold,
                }
            }
        });
        self.hosts
            .lock()
            .unwrap()
            .insert(h.clone(), Arc::clone(&he));
        Ok(he)
    }

    /// Split a stored path and resolve its host in one motion — the shape
    /// nearly every caller wants.
    fn located(&self, raw: &str) -> Result<(host::Located, Arc<HostEnv>)> {
        let loc = host::locate(raw)?;
        let he = self.host_env(&loc.host)?;
        Ok((loc, he))
    }

    /* ---------------------------- tasks ---------------------------- */

    /// Make a card.
    ///
    /// Every repository is checked here rather than when someone first tries
    /// to run the card, so a card that can never produce an attempt cannot be
    /// created in the first place. Ad-hoc sessions are subject to none of
    /// this — they are just a directory.
    ///
    /// A card may span several repositories, under two refusals that are not
    /// tidiness. **One world**: the attempt's checkouts share a directory, and
    /// a directory cannot straddle the boundary between this machine and a WSL
    /// distro or an SSH host — a card mixing them describes a workspace that
    /// cannot exist. **No repetition**: two checkouts of one repository in one
    /// workspace would be two worktrees of the same branch, which git refuses
    /// anyway and which nothing downstream could tell apart.
    pub fn create_task(
        &self,
        title: String,
        prompt: String,
        repo_path: String,
        base_branch: String,
        extra_repos: Vec<TaskRepo>,
    ) -> Result<String> {
        let task = StoredTask {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            prompt,
            repo_path,
            base_branch,
            extra_repos,
            lifecycle: Lifecycle::Backlog,
            position: self
                .store
                .list_tasks()
                .unwrap_or_default()
                .iter()
                .filter(|t| t.lifecycle == Lifecycle::Backlog)
                .count() as i64,
            created_at: now_ms(),
        };

        let repos = task.repos();
        let mut seen: HashMap<String, ()> = HashMap::new();
        let first_host = host::locate(&repos[0].repo_path)?.host;
        for r in &repos {
            let (loc, he) = self.located(&r.repo_path)?;
            if loc.host != first_host {
                return Err(anyhow!(
                    i18n::repos_cross_host(
                        self.locale.get(),
                        &repos[0].repo_path,
                        &r.repo_path
                    )
                ));
            }
            if seen.insert(loc.path.clone(), ()).is_some() {
                return Err(anyhow!(i18n::repo_twice(self.locale.get(), &r.repo_path)));
            }
            self.worktrees
                .check_repo(&he.hr(&self.env), &loc.path, &r.base_branch)?;
        }

        let id = task.id.clone();
        self.store.upsert_task(&task)?;
        self.emit_tasks();
        Ok(id)
    }

    /// Every repository a card spans, located and paired with its base — the
    /// shape the worktree layer opens an attempt from.
    fn repo_specs(&self, task: &StoredTask) -> Result<Vec<worktree::RepoSpec>> {
        task.repos()
            .into_iter()
            .map(|r| {
                Ok(worktree::RepoSpec {
                    repo: host::locate(&r.repo_path)?.path,
                    base_branch: r.base_branch,
                })
            })
            .collect()
    }

    /// An attempt's checkouts, first repository first.
    ///
    /// Attempts opened before a card could span two have no rows of their
    /// own: their single checkout *is* the attempt's own columns, and it is
    /// synthesised here rather than backfilled into a table on somebody's
    /// behalf. Everything downstream iterates this and never asks how many
    /// there are.
    fn trees(&self, attempt: &StoredAttempt) -> Result<Vec<StoredTree>> {
        let rows = self.store.list_trees(&attempt.id)?;
        if !rows.is_empty() {
            return Ok(rows);
        }
        let task = self.task(&attempt.task_id)?;
        Ok(vec![StoredTree {
            attempt_id: attempt.id.clone(),
            position: 0,
            repo_path: task.repo_path,
            base_branch: task.base_branch,
            dir: String::new(),
            worktree_path: attempt.worktree_path.clone(),
            branch: attempt.branch.clone(),
            base_sha: attempt.base_sha.clone(),
        }])
    }

    /// Move a card, or reorder it within its column.
    ///
    /// Only ever called from a drag. Nothing the agent reports reaches this:
    /// a `Stop` hook means "this turn ended", not "the work is finished", and
    /// the distance between those two is the entire reason the board and the
    /// session lights are separate axes.
    pub fn move_task(&self, id: &str, lifecycle: Lifecycle, position: i64) -> Result<()> {
        let mut tasks = self.store.list_tasks()?;
        let Some(idx) = tasks.iter().position(|t| t.id == id) else {
            return Err(anyhow!("no such task: {id}"));
        };

        let mut moved = tasks.remove(idx);
        let was = moved.lifecycle;
        moved.lifecycle = lifecycle;

        // Renumber both affected columns from scratch. Positions are only
        // meaningful relative to their neighbours, and rewriting them is far
        // cheaper than reasoning about which of them shifted.
        let mut column: Vec<StoredTask> =
            tasks.iter().filter(|t| t.lifecycle == lifecycle).cloned().collect();
        let at = (position.max(0) as usize).min(column.len());
        column.insert(at, moved);

        for (i, t) in column.iter_mut().enumerate() {
            t.position = i as i64;
            self.store.upsert_task(t)?;
        }
        if was != lifecycle {
            for (i, t) in tasks
                .iter_mut()
                .filter(|t| t.lifecycle == was)
                .enumerate()
            {
                t.position = i as i64;
                self.store.upsert_task(t)?;
            }
        }
        self.emit_tasks();
        Ok(())
    }

    pub fn delete_task(&self, id: &str) -> Result<()> {
        // Attempts still holding a worktree have to give it back first, or
        // the directories outlive every record that they exist.
        for attempt in self.store.list_attempts(id)? {
            if attempt.outcome.is_none() {
                let _ = self.close_attempt(&attempt, Outcome::Discarded);
            }
        }
        self.store.delete_task(id)?;
        self.emit_tasks();
        Ok(())
    }

    /// Every card, with its attempts and their live sessions.
    pub fn task_board(&self) -> Vec<TaskView> {
        let by_attempt: HashMap<String, String> = self
            .sessions
            .lock()
            .unwrap()
            .values()
            .filter_map(|s| s.attempt_id.clone().map(|a| (a, s.id.clone())))
            .collect();

        let queue: HashMap<String, i64> = self
            .store
            .queue()
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(i, q)| (q.task_id, i as i64 + 1))
            .collect();

        self.store
            .list_tasks()
            .unwrap_or_default()
            .into_iter()
            .map(|task| {
                let attempts = self
                    .store
                    .list_attempts(&task.id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|attempt| AttemptView {
                        session_id: by_attempt.get(&attempt.id).cloned(),
                        attempt,
                    })
                    .collect();
                let queued_at = queue.get(&task.id).copied();
                TaskView {
                    task,
                    attempts,
                    queued_at,
                }
            })
            .collect()
    }

    /* --------------------------- attempts -------------------------- */

    /// The first message, as it would be sent, for the dialog to show and let
    /// the person edit before anything is spawned.
    ///
    /// The branch and base here are the best guess available before the
    /// worktree exists. `open_attempt` renders again against what git
    /// actually handed back, so an edited prompt is used verbatim and an
    /// unedited one is never left quoting a number it did not get.
    pub fn preview_prompt(&self, task_id: &str, agent: &str) -> Result<serde_json::Value> {
        let task = self.task(task_id)?;
        let text = self.render_prompt(&task, None)?;
        // A profile resolves before the question is asked: what matters is
        // the CLI underneath, not what the person calls it.
        let (agent, _) = self.resolve_launcher(agent);
        Ok(serde_json::json!({
            "prompt": text,
            // So the dialog can say plainly that this one will not be sent
            // for you, rather than letting you press a button that quietly
            // does nothing.
            "willSend": prompt::delivery_for(&agent) == Delivery::Positional,
        }))
    }

    /// Open a worktree for this card and start an agent in it.
    ///
    /// `first_prompt` is what the dialog showed, after any edits. It is sent
    /// as written and recorded on the timeline as written, so what the agent
    /// was actually asked is never inferred after the fact.
    /// Start an attempt, or put it in the queue if every slot is taken.
    ///
    /// Queueing rather than refusing, because the answer to "too many at
    /// once" is "later", not "no". The prompt is stored exactly as approved:
    /// when its turn comes it sends what the person saw, not a re-render of a
    /// template that may have been edited since.
    pub fn start_attempt(
        &self,
        task_id: &str,
        agent: String,
        first_prompt: Option<String>,
        mode: PermissionMode,
        cols: u16,
        rows: u16,
    ) -> Result<StartResult> {
        let task = self.task(task_id)?;
        if self.running_attempts() >= self.max_concurrent() {
            let prompt = match first_prompt {
                Some(p) => p,
                None => self.render_prompt(&task, None)?,
            };
            let position = self.store.next_queue_position()?;
            self.store.enqueue_start(&crate::store::QueuedStart {
                id: uuid::Uuid::new_v4().to_string(),
                task_id: task_id.to_string(),
                agent,
                prompt,
                mode,
                cols,
                rows,
                position,
                created_at: now_ms(),
            })?;
            let at = self
                .store
                .queue()?
                .iter()
                .position(|q| q.task_id == task_id)
                .map(|i| i as i64 + 1)
                .unwrap_or(1);
            self.emit_tasks();
            return Ok(StartResult {
                attempt: None,
                queued_at: Some(at),
            });
        }

        let opened = self.open_attempt(task_id, agent, first_prompt, mode, cols, rows)?;
        Ok(StartResult {
            attempt: Some(opened),
            queued_at: None,
        })
    }

    /// How many attempts hold a terminal right now. This is the thing the
    /// quota rations — a saved attempt costs nobody any attention.
    pub fn running_attempts(&self) -> i64 {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.live && s.attempt_id.is_some())
            .count() as i64
    }

    /// What quitting right now would cost, counted in agent sessions.
    ///
    /// An update ends with a restart, and a restart is only cheap where
    /// something else is holding the agents: a world that answered `tmux -V`
    /// detaches them and hands them back on the next run, and a world that
    /// did not ends them. Native Windows is the whole of the second category
    /// and is not a corner case — there is no native Windows tmux to be the
    /// holder, so every agent on that desk is in the second column.
    ///
    /// Run scripts and worktree shells are deliberately not counted. They are
    /// never held, by the same ruling that holds agents: a script is a thing
    /// you started to watch and it goes when the desk does. Counting them
    /// would price a loss that is not one, on the one number a person uses to
    /// decide whether to restart.
    pub fn restart_cost(&self) -> RestartCost {
        let mut cost = RestartCost::default();
        for s in self.sessions.lock().unwrap().values() {
            if !s.live || !s.agent_session {
                continue;
            }
            if self.ptys.is_held(&s.id) {
                cost.held += 1;
            } else {
                cost.lost += 1;
            }
        }
        cost
    }

    pub fn max_concurrent(&self) -> i64 {
        self.store
            .setting(MAX_CONCURRENT_KEY)
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .filter(|n| *n >= 1)
            .unwrap_or(DEFAULT_MAX_CONCURRENT)
    }

    /// Raising the limit lets waiting cards go at once; that is the point of
    /// raising it.
    pub fn set_max_concurrent(&self, n: i64) -> Result<()> {
        self.store
            .set_setting(MAX_CONCURRENT_KEY, &n.max(1).to_string())?;
        self.drain_queue();
        self.emit_tasks();
        Ok(())
    }

    pub fn cancel_queued(&self, task_id: &str) -> Result<()> {
        self.store.dequeue(task_id)?;
        self.emit_tasks();
        Ok(())
    }

    /// Start whatever the freed slots can take.
    ///
    /// Called whenever a slot might have opened. A queued start that fails —
    /// its repository moved, its base branch went — is dropped from the queue
    /// with a note rather than retried forever in front of the ones behind it.
    pub fn drain_queue(&self) {
        loop {
            if self.running_attempts() >= self.max_concurrent() {
                return;
            }
            let Some(next) = self.store.queue().ok().and_then(|q| q.into_iter().next()) else {
                return;
            };
            // Off the queue first, so a failure cannot loop on it.
            let _ = self.store.dequeue(&next.task_id);
            match self.open_attempt(
                &next.task_id,
                next.agent.clone(),
                Some(next.prompt.clone()),
                next.mode,
                next.cols,
                next.rows,
            ) {
                Ok(opened) => {
                    eprintln!(
                        "[core] queue: started {} on {}",
                        next.task_id, opened.branch
                    );
                }
                Err(e) => {
                    eprintln!("[core] queue: {} could not start: {e:#}", next.task_id);
                    self.sink.emit(
                        "notify",
                        serde_json::json!({
                            "title": crate::i18n::queued_start_failed(self.locale.get()),
                            "body": format!("{e:#}"),
                            "sessionId": serde_json::Value::Null,
                        }),
                    );
                }
            }
        }
    }

    pub fn queue(&self) -> Vec<crate::store::QueuedStart> {
        self.store.queue().unwrap_or_default()
    }

    fn open_attempt(
        &self,
        task_id: &str,
        agent: String,
        first_prompt: Option<String>,
        mode: PermissionMode,
        cols: u16,
        rows: u16,
    ) -> Result<OpenedAttempt> {
        let task = self.task(task_id)?;
        // Every repository on a card is in one world — refused at creation —
        // so one host answers for the whole attempt.
        let (_, he) = self.located(&task.repo_path)?;
        let specs = self.repo_specs(&task)?;
        let seq = self.store.next_attempt_seq(task_id)?;
        let slug = worktree::slug(&task.title, &task.id);

        let wt = self.worktrees.create(
            &he.hr(&self.env),
            &he.worktree_root,
            &specs,
            &slug,
            seq,
        )?;

        // From here on a failure has worktrees to give back — all of them,
        // and the workspace above them.
        let opened = self.finish_opening(&task, agent, first_prompt, mode, &he, &wt, cols, rows);
        if opened.is_err() {
            for tree in &wt.trees {
                let _ = self
                    .worktrees
                    .remove(&he.hr(&self.env), &tree.repo, &tree.path);
            }
            if wt.trees.len() > 1 {
                let _ = self.worktrees.remove_root(&he.hr(&self.env), &wt.root);
            }
        }
        let opened = opened?;

        self.move_task(task_id, Lifecycle::Running, 0)?;
        self.emit_tasks();
        self.broadcast();
        Ok(opened)
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_opening(
        &self,
        task: &StoredTask,
        agent: String,
        first_prompt: Option<String>,
        mode: PermissionMode,
        he: &HostEnv,
        wt: &worktree::OpenedWorktree,
        cols: u16,
        rows: u16,
    ) -> Result<OpenedAttempt> {
        // The picked launcher becomes an actual CLI here — a queued start
        // carries the profile's *name* and resolves only now, so it runs
        // whatever the profile says at the moment it actually starts.
        let (agent, profile_args) = self.resolve_launcher(&agent);
        let attempt_id = uuid::Uuid::new_v4().to_string();
        // Stored in the app's path space, so every later reader knows which
        // host to ask; inside the host it is `wt.root` plain.
        let cwd = host::stored(&he.host, &wt.root);

        let text = match first_prompt {
            Some(edited) => edited,
            None => self.render_prompt(task, Some(wt))?,
        };

        // Each repository's own word on how its checkout becomes runnable. A
        // malformed file fails the start here, in the dialog, rather than
        // producing a workspace that is mysteriously not set up.
        let mut steps = Vec::new();
        for tree in &wt.trees {
            if let Some(script) = self.repo_config(he, &tree.repo)?.unwrap_or_default().setup {
                steps.push(SetupWrap {
                    script,
                    dir: tree.dir.clone(),
                    // The path scripts see is the host's own: `$MAROL_ROOT_PATH`
                    // is for `cp`, and `cp` runs inside.
                    root_path: tree.repo.clone(),
                });
            }
        }
        let setup = (!steps.is_empty()).then(|| Setup {
            steps,
            root_path: wt.first().repo.clone(),
        });

        let delivery = prompt::delivery_for(&agent);
        let positional = match delivery {
            Delivery::Positional => Some(text.clone()),
            Delivery::Manual => None,
        };

        let session_id = uuid::Uuid::new_v4().to_string();
        let at = now_ms();
        // A brand-new worktree always opens on the folder-trust prompt, and
        // no hook can report that. See `Status::AwaitingTrust`.
        //
        // With a setup script in front, the trust prompt arrives whenever the
        // script finishes — which the core cannot see. `Starting` is the
        // honest label for "watch the terminal", and the setup's own output
        // is right there explaining what the wait is.
        let status = if setup.is_some() {
            Status::Starting
        } else if delivery == Delivery::Positional {
            Status::AwaitingTrust
        } else {
            Status::Starting
        };

        let meta = SessionMeta {
            id: session_id.clone(),
            cwd: cwd.clone(),
            title: format!("{} #{}", task.title, wt.seq),
            agent: agent.clone(),
            status,
            created_at: at,
            last_active_at: at,
            live: true,
            reports_status: false,
            hooks_wired: false,
            activity: None,
            activity_since: 0,
            completed: false,
            attempt_id: Some(attempt_id.clone()),
            agent_session: true,
            has_followup: false,
            preview_port: None,
            usage: None,
            transcript_path: None,
        };

        // Visible before it can speak. The PTY reports its exit against the
        // sessions map, and a setup script that fails in milliseconds beats
        // the rest of this function to that report — so the session goes on
        // the record first and launches second, or an instant death would
        // land on a map that had never heard of it and the session would sit
        // at "starting" forever.
        self.sessions
            .lock()
            .unwrap()
            .insert(session_id.clone(), meta.clone());

        // The profile's standing arguments first, then the mode's flags —
        // all options, ahead of the hook wiring and the prompt. The mode's
        // flags are the ones this CLI actually spells; a CLI whose
        // conventions nobody measured launches without them rather than
        // being handed a guess. The mode is still recorded either way — it
        // is what the person approved.
        let mut opts = profile_args;
        if let Some(cli) = Cli::of(&agent) {
            opts.extend(cli.mode_args(mode).iter().map(|s| s.to_string()));
        }

        // No resume here, in either spelling: this worktree has no history
        // to continue, and the prompt is what starts the work.
        let tail = match positional {
            Some(p) => Tail::Prompt(p),
            None => Tail::Nothing,
        };
        if let Err(e) = self.launch(
            &session_id,
            &agent,
            opts,
            tail,
            &cwd,
            cols,
            rows,
            setup.as_ref(),
        ) {
            self.sessions.lock().unwrap().remove(&session_id);
            return Err(e);
        }

        self.store.insert_attempt(&StoredAttempt {
            id: attempt_id.clone(),
            task_id: task.id.clone(),
            seq: wt.seq,
            agent,
            worktree_path: cwd.clone(),
            branch: wt.branch.clone(),
            base_sha: wt.first().base_sha.clone(),
            mode,
            outcome: None,
            frozen_diff: None,
            created_at: at,
            parked_at: None,
        })?;
        // Every checkout, including the only one a single-repository attempt
        // has: what the rest of the core iterates has to exist for all of
        // them, or the ordinary case would take a different road through
        // every function that touches a worktree.
        self.store.insert_trees(
            &wt.trees
                .iter()
                .enumerate()
                .map(|(i, t)| StoredTree {
                    attempt_id: attempt_id.clone(),
                    position: i as i64,
                    repo_path: host::stored(&he.host, &t.repo),
                    base_branch: t.base_branch.clone(),
                    dir: t.dir.clone(),
                    worktree_path: host::stored(&he.host, &t.path),
                    branch: t.branch.clone(),
                    base_sha: t.base_sha.clone(),
                })
                .collect::<Vec<_>>(),
        )?;

        self.persist(&meta);

        // Recorded as sent, not as templated: the dialog is editable, and the
        // timeline has to show what the agent was actually asked.
        let _ = self
            .store
            .append_event(&attempt_id, at, "prompt", None, Some(&text));

        Ok(OpenedAttempt {
            attempt_id,
            session_id,
            branch: wt.branch.clone(),
            worktree_path: cwd,
            prompt: text,
            prompt_sent: delivery == Delivery::Positional,
        })
    }

    /// Put a terminal back on an attempt that is not running.
    ///
    /// After a restart this is the state every attempt is in — the app kills
    /// its PTYs on the way out and the agent's own history on disk is what
    /// survives. `--continue` reads that history; the prompt is deliberately
    /// not sent again, because a second copy would set the agent off doing the
    /// whole card from the beginning.
    pub fn reopen_attempt(&self, attempt_id: &str, cols: u16, rows: u16) -> Result<String> {
        let attempt = self
            .store
            .get_attempt(attempt_id)?
            .ok_or_else(|| anyhow!("no such attempt: {attempt_id}"))?;
        if attempt.outcome.is_some() {
            return Err(anyhow!(
                "this attempt is finished; its worktree has been removed"
            ));
        }
        if attempt.parked_at.is_some() {
            return Err(anyhow!(
                "attempt {attempt_id} is parked — resume it, which grows the worktree back first"
            ));
        }
        let (wt_loc, he) = self.located(&attempt.worktree_path)?;
        if !he.hr(&self.env).is_dir(&wt_loc.path) {
            return Err(anyhow!(
                "the worktree at {} is gone",
                attempt.worktree_path
            ));
        }

        let existing = self
            .sessions
            .lock()
            .unwrap()
            .values()
            .find(|s| s.attempt_id.as_deref() == Some(attempt_id))
            .map(|s| s.id.clone());

        if let Some(id) = existing {
            if self.ptys.is_live(&id) {
                return Err(anyhow!("attempt {attempt_id} already has a terminal"));
            }
            self.reopen_session(&id, cols, rows)?;
            return Ok(id);
        }

        // The session row was archived out from under the attempt. Give it a
        // new terminal on the same worktree; the agent's history is in the
        // directory, not in our row.
        let session_id = uuid::Uuid::new_v4().to_string();
        let at = now_ms();
        // The permission mode rides along on a resume: it is part of what
        // was approved for this attempt, not a per-launch choice.
        let (opts, tail) = resume_line(Cli::of(&attempt.agent), attempt.mode);
        let meta = SessionMeta {
            id: session_id.clone(),
            cwd: attempt.worktree_path.clone(),
            title: format!("attempt #{}", attempt.seq),
            agent: attempt.agent.clone(),
            status: Status::Starting,
            created_at: at,
            last_active_at: at,
            live: true,
            reports_status: false,
            hooks_wired: false,
            activity: None,
            activity_since: 0,
            completed: false,
            attempt_id: Some(attempt_id.to_string()),
            agent_session: true,
            has_followup: false,
            preview_port: None,
            usage: None,
            transcript_path: None,
        };
        // On the record before it can exit — see `finish_opening`.
        self.sessions
            .lock()
            .unwrap()
            .insert(session_id.clone(), meta.clone());
        if let Err(e) = self.launch(
            &session_id,
            &attempt.agent,
            opts,
            tail,
            &attempt.worktree_path,
            cols,
            rows,
            // Setup ran when the worktree was made; reopening continues.
            None,
        ) {
            self.sessions.lock().unwrap().remove(&session_id);
            return Err(e);
        }
        self.persist(&meta);
        self.broadcast();
        Ok(session_id)
    }

    /// End an attempt: freeze what it did, then give the worktree back.
    ///
    /// The order matters. Removing the worktree first would take the diff
    /// with it, and an attempt whose evidence is gone cannot be reviewed —
    /// which is the whole reason a superseded attempt is kept at all.
    pub fn finish_attempt(&self, attempt_id: &str, outcome: Outcome) -> Result<()> {
        let attempt = self
            .store
            .get_attempt(attempt_id)?
            .ok_or_else(|| anyhow!("no such attempt: {attempt_id}"))?;
        self.close_attempt(&attempt, outcome)?;
        self.emit_tasks();
        self.broadcast();
        self.drain_queue();
        Ok(())
    }

    fn close_attempt(&self, attempt: &StoredAttempt, outcome: Outcome) -> Result<()> {
        let worktree = attempt.worktree_path.clone();
        let trees = self.trees(attempt).unwrap_or_default();

        // Best effort: a worktree that has already been deleted by hand must
        // not stop the attempt from being closed out. The whole attempt's
        // diff, checkout by checkout — the same text the drawer was showing,
        // so what is frozen is what was reviewed.
        let diff = self.attempt_diff_from(&attempt.id, None).ok();

        self.store
            .finish_attempt(&attempt.id, outcome, diff.as_deref())?;

        // The session goes with the directory it was running in — and so does
        // anything else living there. A dev server started from the Run
        // button is an ad-hoc session whose cwd is this worktree, and a
        // terminal whose directory has been deleted is a trap that looks
        // alive.
        let doomed: Vec<String> = self
            .sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| {
                // Compared in the stored path space, where both sides carry
                // their host prefix — a WSL dev server matches its WSL
                // worktree and nothing else's.
                s.attempt_id.as_deref() == Some(&attempt.id)
                    || s.cwd == worktree
                    || s.cwd
                        .starts_with(&format!("{}/", worktree.trim_end_matches('/')))
            })
            .map(|s| s.id.clone())
            .collect();
        for id in doomed {
            self.ptys.kill(&id);
            let _ = self.store.archive_session(&id);
            self.sessions.lock().unwrap().remove(&id);
        }

        // The first checkout that would not come back, kept to raise once
        // every other one has had its turn.
        let mut failed: Option<anyhow::Error> = None;
        for tree in &trees {
            let (Ok((repo_loc, he)), Ok(wt_loc)) = (
                self.located(&tree.repo_path),
                host::locate(&tree.worktree_path),
            ) else {
                continue;
            };
            let hr = he.hr(&self.env);
            // The frozen diff is the record from here on; the snapshots go
            // with the attempt. Best effort, and against the main checkout —
            // the refs live in the shared git dir, not the worktree.
            if let Err(e) = self
                .worktrees
                .clear_checkpoints(&hr, &repo_loc.path, &attempt.id)
            {
                eprintln!("[core] checkpoint refs for {} not cleared: {e:#}", attempt.id);
            }
            // The archive script gets its chance while the directory still
            // exists — the place to stop containers or give back whatever
            // setup borrowed.
            if hr.is_dir(&wt_loc.path) {
                match self.repo_config(&he, &repo_loc.path) {
                    Ok(Some(cfg)) => {
                        if let Some(script) = cfg.archive {
                            run_archive(&hr, &script, &wt_loc.path, &repo_loc.path);
                        }
                    }
                    Ok(None) => {}
                    // Closing must not be stopped by a config typo; the
                    // person is taking the worktree back either way.
                    Err(e) => eprintln!("[core] archive script skipped: {e:#}"),
                }
            }
            // One checkout that will not come back must not strand the
            // others. The attempt is already finished in the database by
            // now, so a `?` here would leave the rest of the workspace on
            // disk with nothing left pointing at it — and no way to ask for
            // it back. Say which one, take the rest.
            if let Err(e) = self.worktrees.remove(&hr, &repo_loc.path, &wt_loc.path) {
                eprintln!(
                    "[core] worktree {} not given back: {e:#}",
                    tree.worktree_path
                );
                failed = failed.or(Some(e));
            }
        }

        // The workspace above them, once they are all gone. Only ever the
        // empty shell a multi-repository attempt left behind: `remove_dir`
        // refuses a directory still holding anything, and one still holding
        // something is somebody's to look at rather than ours to delete.
        if trees.len() > 1 {
            if let Ok((loc, he)) = self.located(&worktree) {
                if let Err(e) = self.worktrees.remove_root(&he.hr(&self.env), &loc.path) {
                    eprintln!("[core] workspace {worktree} left standing: {e:#}");
                }
            }
        }
        // Raised last, so it reports a checkout nobody could take back rather
        // than hiding the ones that were.
        match failed {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    pub fn attempt_events(&self, attempt_id: &str) -> Result<Vec<crate::store::AttemptEvent>> {
        self.store.list_events(attempt_id)
    }

    /* --------------------------- profiles --------------------------- */

    /// Everything a launch dialog can offer: the bare agents, then the
    /// person's profiles. The dialogs render this instead of carrying their
    /// own list, so a new profile — or one day a new agent — is data, not a
    /// frontend change.
    pub fn launchers(&self) -> Result<Vec<Launcher>> {
        let mut list: Vec<Launcher> = BARE_AGENTS
            .iter()
            .map(|a| Launcher {
                name: a.to_string(),
                agent: a.to_string(),
                profile: false,
            })
            .collect();
        for p in self.store.profiles()? {
            list.push(Launcher {
                name: p.name,
                agent: p.agent,
                profile: true,
            });
        }
        Ok(list)
    }

    pub fn profiles(&self) -> Result<Vec<Profile>> {
        self.store.profiles()
    }

    /// Replace the profiles, after checking they can actually be offered:
    /// every name says something, no two say the same thing, and none of
    /// them says "claude" while meaning something else.
    pub fn set_profiles(&self, profiles: Vec<Profile>) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        for p in &profiles {
            let name = p.name.trim();
            if name.is_empty() {
                return Err(anyhow!("a profile needs a name"));
            }
            if p.agent.trim().is_empty() {
                return Err(anyhow!("profile `{name}` names no agent CLI"));
            }
            if BARE_AGENTS.contains(&name) {
                return Err(anyhow!(
                    "`{name}` is an agent's own name; a profile may not shadow it"
                ));
            }
            if !seen.insert(name.to_string()) {
                return Err(anyhow!("two profiles are both called `{name}`"));
            }
        }
        self.store.set_profiles(&profiles)
    }

    /// What a picked launcher name means: a profile's agent and standing
    /// arguments, or — for any other string — a bare binary with none. The
    /// fallback is today's semantics kept honest: `agent` has always been a
    /// binary resolved on the login-shell PATH, so a profile deleted while a
    /// card sat in the queue degrades to a name the spawn will report as not
    /// found, rather than to a silent guess.
    fn resolve_launcher(&self, name: &str) -> (String, Vec<String>) {
        if let Ok(profiles) = self.store.profiles() {
            if let Some(p) = profiles.into_iter().find(|p| p.name == name) {
                return (p.agent, p.args);
            }
        }
        (name.to_string(), Vec::new())
    }

    /// The names of the repositories' run scripts, for the drawer's buttons.
    ///
    /// A card spanning a service and its client has two dev servers, and both
    /// are things to watch. Their names are prefixed with the checkout they
    /// belong to — `web:dev` — because `dev` twice on one drawer is two
    /// buttons nobody can tell apart, and because that prefix is what
    /// `run_script` reads back to know which one was pressed.
    pub fn list_run_scripts(&self, attempt_id: &str) -> Result<Vec<String>> {
        let attempt = self
            .store
            .get_attempt(attempt_id)?
            .ok_or_else(|| anyhow!("no such attempt: {attempt_id}"))?;
        let mut out = Vec::new();
        for tree in self.trees(&attempt)? {
            let (loc, he) = self.located(&tree.repo_path)?;
            for r in self.repo_config(&he, &loc.path)?.unwrap_or_default().run {
                out.push(qualified_script(&tree.dir, &r.name));
            }
        }
        Ok(out)
    }

    /// The repository's own word on how a worktree becomes runnable, under
    /// whichever of its two names it wears — see `config::FILES`. Reads
    /// through the host, so a `wsl://` or `ssh://` repository answers the same
    /// way a local one does.
    fn repo_config(&self, he: &HostEnv, repo: &str) -> Result<Option<config::RepoConfig>> {
        let hr = he.hr(&self.env);
        for name in config::FILES {
            let path = he.host.join(repo, name);
            if let Some(text) = hr.read_to_string(&path)? {
                return Ok(Some(config::parse(&text, &path)?));
            }
        }
        Ok(None)
    }

    /// Start one of the repository's run scripts in the attempt's worktree.
    ///
    /// The script gets a terminal of its own — a dev server's output is a
    /// thing to watch, and watching is what this app does. The session is
    /// ad-hoc on purpose: it has no lifecycle and takes no slot, because the
    /// quota rations agents (attention), and a dev server asks for none.
    /// `MAROL_PORT` carries a port nothing else is on, so two attempts'
    /// servers never fight over 3000.
    pub fn run_script(
        &self,
        attempt_id: &str,
        name: &str,
        cols: u16,
        rows: u16,
    ) -> Result<String> {
        let attempt = self
            .store
            .get_attempt(attempt_id)?
            .ok_or_else(|| anyhow!("no such attempt: {attempt_id}"))?;
        if attempt.outcome.is_some() {
            return Err(anyhow!("this attempt is finished; its worktree has been removed"));
        }
        // Which checkout's script this is comes from the name the drawer
        // pressed — `web:dev` — so two repositories may each have a `dev`.
        let trees = self.trees(&attempt)?;
        let tree = trees
            .iter()
            .find(|t| bare_script(&t.dir, name).is_some())
            .ok_or_else(|| anyhow!("no run script named `{name}` in {}", config::FILE))?;
        let bare = bare_script(&tree.dir, name)
            .ok_or_else(|| anyhow!("no run script named `{name}` in {}", config::FILE))?;

        let (repo_loc, he) = self.located(&tree.repo_path)?;
        let wt_loc = host::locate(&tree.worktree_path)?;
        // A local host needs a local POSIX shell; a WSL host brings its own.
        if matches!(he.host, Host::Local) && !cfg!(unix) {
            return Err(anyhow!("run scripts need a POSIX shell"));
        }
        let config_path = he.host.join(&repo_loc.path, config::FILE);
        let config = he
            .hr(&self.env)
            .read_to_string(&config_path)?
            .map(|t| config::parse(&t, &config_path))
            .transpose()?
            .ok_or_else(|| anyhow!("{} has no {}", tree.repo_path, config::FILE))?;
        let script = config
            .run
            .into_iter()
            .find(|r| r.name == bare)
            .ok_or_else(|| anyhow!("no run script named `{name}` in {}", config::FILE))?;

        let port = match &he.host {
            Host::Local => free_port()?,
            // The kernel that owns the port is the host's; asked from here
            // the answer would describe the wrong machine. A high port drawn
            // from randomness collides rarely, and a colliding dev server
            // fails loudly in its own terminal.
            _ => 20000 + (uuid::Uuid::new_v4().as_u128() % 40000) as u16,
        };
        let id = uuid::Uuid::new_v4().to_string();
        let at = now_ms();
        let meta = SessionMeta {
            id: id.clone(),
            // The checkout the script belongs to, not the workspace above it:
            // `npm run dev` has to run where the `package.json` is.
            cwd: tree.worktree_path.clone(),
            title: format!("▶ {name}"),
            agent: "sh".to_string(),
            status: Status::Starting,
            created_at: at,
            last_active_at: at,
            live: true,
            reports_status: false,
            hooks_wired: false,
            activity: None,
            activity_since: 0,
            completed: false,
            // Ad-hoc: no lifecycle, no slot. The attempt link would also put
            // it on the card, and the card is about the agent.
            attempt_id: None,
            // A script is watched, not left running: it is never held, and
            // ending it with the desk is what it is for.
            agent_session: false,
            has_followup: false,
            // Reachable worlds only: local directly, WSL through mirrored
            // networking. An SSH host's port lives on the remote, and a
            // recorded port nobody can dial would put a preview button on
            // a door that opens onto a wall.
            preview_port: match &he.host {
                Host::Ssh { .. } => None,
                _ => Some(port),
            },
            usage: None,
            transcript_path: None,
        };

        let script_env = under_both_names(vec![
            ("MAROL_PORT".to_string(), port.to_string()),
            ("MAROL_ROOT_PATH".to_string(), repo_loc.path.clone()),
        ]);
        let (program, args, outer_cwd, outer_env): (String, Vec<String>, Option<String>, Vec<(String, String)>) =
            match &he.host {
                Host::Local => (
                    "sh".to_string(),
                    vec!["-c".to_string(), script.command],
                    Some(wt_loc.path.clone()),
                    script_env.to_vec(),
                ),
                _ => {
                    let envs = host::pty_env(&he.env, &script_env);
                    let (p, a, _) = he.host.wrap(
                        "sh",
                        &["-c".to_string(), script.command],
                        Some(&wt_loc.path),
                        &envs,
                    );
                    (p, a, None, Vec::new())
                }
            };

        // On the record before it can exit — see `finish_opening`. A script
        // that dies at once (`command not found`) must die visibly.
        self.sessions.lock().unwrap().insert(id.clone(), meta.clone());
        if let Err(e) = self.ptys.spawn(
            &id,
            &program,
            &args,
            outer_cwd.as_deref(),
            &self.env,
            &outer_env,
            cols.max(20),
            rows.max(5),
            Arc::clone(&self.router) as Arc<dyn PtySink>,
            // Not held: a script and a shell are things you started to
            // watch, and they end when the desk does.
            None,
        ) {
            self.sessions.lock().unwrap().remove(&id);
            return Err(e);
        }

        self.persist(&meta);
        self.broadcast();
        Ok(id)
    }

    /// A shell of the person's own, in the attempt's worktree.
    ///
    /// Reviewing an agent's work keeps demanding ad-hoc commands — run the
    /// tests, `git log`, grep — in *its* worktree, not yours. The ▶ scripts
    /// cover what the repository predicted; this covers everything it did
    /// not, without typing into the agent's terminal and without hunting
    /// the worktree path to `cd` into. The shell is the host's own login
    /// shell, so inside WSL it is the distro's, with the distro's PATH.
    pub fn open_shell(&self, attempt_id: &str, cols: u16, rows: u16) -> Result<String> {
        let attempt = self
            .store
            .get_attempt(attempt_id)?
            .ok_or_else(|| anyhow!("no such attempt: {attempt_id}"))?;
        if attempt.outcome.is_some() {
            return Err(anyhow!(
                "this attempt is finished; its worktree has been removed"
            ));
        }

        // One shell per attempt: while it lives, the button returns it
        // rather than stacking a second.
        if let Some(existing) = self.shells.lock().unwrap().get(attempt_id) {
            if self
                .sessions
                .lock()
                .unwrap()
                .get(existing)
                .is_some_and(|s| s.live)
            {
                return Ok(existing.clone());
            }
        }

        let task = self.task(&attempt.task_id)?;
        // The card's first repository, for the world this runs in and for
        // `$MAROL_ROOT_PATH`. Every repository on a card is in one world, so
        // one of them answers for the shell either way.
        let (repo_loc, he) = self.located(&task.repo_path)?;
        // The shell opens in the attempt's own directory — the workspace when
        // the card spans several repositories, which is where the agent is
        // standing and so where a `git log` in one of them starts from.
        let wt_loc = host::locate(&attempt.worktree_path)?;
        if matches!(he.host, Host::Local) && !cfg!(unix) {
            return Err(anyhow!("a worktree shell needs a POSIX shell"));
        }

        let id = uuid::Uuid::new_v4().to_string();
        let at = now_ms();
        let shell = he.env.shell.clone();
        let meta = SessionMeta {
            id: id.clone(),
            cwd: attempt.worktree_path.clone(),
            title: format!("$ {} #{}", task.title, attempt.seq),
            agent: shell
                .rsplit(['/', '\\'])
                .find(|s| !s.is_empty())
                .unwrap_or("sh")
                .to_string(),
            status: Status::Starting,
            created_at: at,
            last_active_at: at,
            live: true,
            reports_status: false,
            hooks_wired: false,
            activity: None,
            activity_since: 0,
            completed: false,
            // Ad-hoc, like the ▶ scripts: no lifecycle, no slot — the card
            // is about the agent, and this terminal is about you.
            attempt_id: None,
            // And for the same reason, not held and not a loss on restart.
            agent_session: false,
            has_followup: false,
            preview_port: None,
            usage: None,
            transcript_path: None,
        };

        // The same variable the scripts see, because the same need exists:
        // the repository the worktree was opened from is where untracked
        // things worth reaching (.env) live.
        let shell_env = under_both_names(vec![(
            "MAROL_ROOT_PATH".to_string(),
            repo_loc.path.clone(),
        )]);
        let (program, args, outer_cwd, outer_env): (
            String,
            Vec<String>,
            Option<String>,
            Vec<(String, String)>,
        ) = match &he.host {
            Host::Local => (
                shell,
                Vec::new(),
                Some(wt_loc.path.clone()),
                shell_env.to_vec(),
            ),
            _ => {
                let envs = host::pty_env(&he.env, &shell_env);
                let (p, a, _) = he.host.wrap(&shell, &[], Some(&wt_loc.path), &envs);
                (p, a, None, Vec::new())
            }
        };

        self.sessions.lock().unwrap().insert(id.clone(), meta.clone());
        if let Err(e) = self.ptys.spawn(
            &id,
            &program,
            &args,
            outer_cwd.as_deref(),
            &self.env,
            &outer_env,
            cols.max(20),
            rows.max(5),
            Arc::clone(&self.router) as Arc<dyn PtySink>,
            // Not held: a script and a shell are things you started to
            // watch, and they end when the desk does.
            None,
        ) {
            self.sessions.lock().unwrap().remove(&id);
            return Err(e);
        }

        self.shells
            .lock()
            .unwrap()
            .insert(attempt_id.to_string(), id.clone());
        self.persist(&meta);
        self.broadcast();
        Ok(id)
    }

    /// The attempt's footprint at a glance — numstat counts and where its
    /// branch stands against the base, for the card badges. A finished
    /// attempt has no worktree left and no standing to measure; its frozen
    /// diff already says everything it will ever say.
    ///
    /// Across every checkout, added up. A card spanning two repositories did
    /// one piece of work and the badge answers "how big is it" — splitting
    /// the number would answer a question nobody asked of a card. `dirty` is
    /// any of them, because any one uncommitted checkout is enough to refuse
    /// the merge, which is what that flag exists to warn about.
    pub fn attempt_stats(&self, attempt_id: &str) -> Result<worktree::DiffStat> {
        let attempt = self
            .store
            .get_attempt(attempt_id)?
            .ok_or_else(|| anyhow!("no such attempt: {attempt_id}"))?;
        if attempt.outcome.is_some() {
            return Err(anyhow!("attempt is finished"));
        }
        let mut total = worktree::DiffStat::default();
        for tree in self.trees(&attempt)? {
            let (wt_loc, he) = self.located(&tree.worktree_path)?;
            let one = self.worktrees.stat(
                &he.hr(&self.env),
                &wt_loc.path,
                &tree.base_sha,
                &tree.base_branch,
            )?;
            total.files += one.files;
            total.adds += one.adds;
            total.dels += one.dels;
            total.ahead += one.ahead;
            total.behind += one.behind;
            total.dirty |= one.dirty;
        }
        Ok(total)
    }

    /// The attempt's diff: live from the worktree while it still exists, and
    /// the frozen copy once it does not.
    /// What the agent working here already knows before anyone types.
    ///
    /// Slots, not discoveries: a rules file that is missing is still listed,
    /// with its path and marked absent, because the useful answer to "where
    /// do the conventions go" is the path itself. Skills are the exception —
    /// a skill is whatever somebody wrote — so those are read off disk.
    ///
    /// Every supported CLI's convention appears, not only the one this
    /// session happens to run — the slots come from `agent::DOCS`, which is
    /// the same table the rest of the core reads. The question people open
    /// this tab with is about the repository, not about the session, and
    /// narrowing it to the running agent would answer a smaller one.
    pub fn agent_docs(&self, cwd: &str) -> Result<Vec<AgentDoc>> {
        let (loc, he) = self.located(cwd)?;
        let hr = he.hr(&self.env);
        let home = he
            .env
            .vars
            .get("HOME")
            .or_else(|| he.env.vars.get("USERPROFILE"))
            .cloned();
        let mut out = Vec::new();

        // A workspace is not a checkout: for a card spanning several
        // repositories the project-scoped conventions live one directory
        // down, in each of them. Asking the workspace itself would answer
        // "no CLAUDE.md here" about a session whose agent has read two — the
        // one lie this tab exists to prevent.
        // `(folder inside the workspace, path on the host)`. The folder is
        // what tells two checkouts' `CLAUDE.md` apart in the list, and is
        // empty for the ordinary session, whose own directory is the
        // checkout and which therefore has nothing to disambiguate.
        let projects: Vec<(String, String)> = self
            .attempt_at(cwd)
            .filter(|trees| trees.len() > 1)
            .map(|trees| {
                trees
                    .iter()
                    .filter_map(|t| {
                        host::locate(&t.worktree_path)
                            .ok()
                            .map(|l| (t.dir.clone(), l.path))
                    })
                    .collect()
            })
            .unwrap_or_else(|| vec![(String::new(), loc.path.clone())]);

        for (dir, project) in &projects {
            for (name, agent) in agent::DOCS.project_rules {
                let path = hr.join(project, name);
                out.push(AgentDoc {
                    scope: "project",
                    agent,
                    kind: "rules",
                    dir: dir.clone(),
                    exists: hr.exists(&path),
                    name: name.to_string(),
                    path,
                });
            }
        }

        if let Some(home) = home.as_deref() {
            for (dir, name, agent) in agent::DOCS.global_rules {
                let path = hr.join(&hr.join(home, dir), name);
                out.push(AgentDoc {
                    scope: "global",
                    agent,
                    kind: "rules",
                    dir: String::new(),
                    exists: hr.exists(&path),
                    name: name.to_string(),
                    path,
                });
            }
        }

        // One directory per skill, each holding a SKILL.md. A directory
        // without one is somebody's notes, not a skill, and stays out.
        //
        // Both CLIs look in their own `<dir>/skills`, and the file inside is
        // the same file — a skill written for one is read by the other. So
        // both roots are walked, and each entry says which CLI's shelf it
        // was on rather than pretending there is only one shelf.
        for (dir, agent) in agent::DOCS.skill_roots {
            let mut roots: Vec<(&'static str, String, String)> = projects
                .iter()
                .map(|(d, p)| ("project", d.clone(), hr.join(&hr.join(p, dir), "skills")))
                .collect();
            roots.push((
                "global",
                String::new(),
                home.as_deref()
                    .map(|h| hr.join(&hr.join(h, dir), "skills"))
                    .unwrap_or_default(),
            ));
            for (scope, where_, root) in roots {
                if root.is_empty() {
                    continue;
                }
                for entry in hr.list_dir(&root) {
                    let path = hr.join(&hr.join(&root, &entry), "SKILL.md");
                    if hr.exists(&path) {
                        out.push(AgentDoc {
                            scope,
                            agent,
                            kind: "skill",
                            dir: where_.clone(),
                            exists: true,
                            name: entry,
                            path,
                        });
                    }
                }
            }
        }
        Ok(out)
    }

    /// The checkouts of the attempt whose directory this is, if it is one.
    ///
    /// Asked by directory rather than by id because the callers that need it
    /// — the Knows tab most of all — are handed a session's cwd and nothing
    /// else. `None` for an ad-hoc session, which is exactly what it should
    /// be: a directory somebody pointed at is a directory, not a workspace.
    fn attempt_at(&self, cwd: &str) -> Option<Vec<StoredTree>> {
        let attempt = self
            .store
            .open_attempts()
            .ok()?
            .into_iter()
            .find(|a| a.worktree_path == cwd)?;
        self.trees(&attempt).ok()
    }

    pub fn attempt_diff(&self, attempt_id: &str) -> Result<String> {
        self.attempt_diff_from(attempt_id, None)
    }

    /// The same diff with the baseline swapped: against checkpoint `against`
    /// instead of the attempt's base, answering "what has happened since
    /// that snapshot" with the rendering the drawer already has. `0` (or
    /// `None`) is the base itself.
    ///
    /// One diff for the whole attempt, its checkouts one after another. They
    /// concatenate rather than needing anything to join them because each
    /// one's paths are rendered relative to the directory the session stands
    /// in — so `web/api.ts` and `api/routes.py` are two files in one diff,
    /// and a review comment naming either points somewhere the agent can
    /// open without being told where it is.
    pub fn attempt_diff_from(&self, attempt_id: &str, against: Option<u64>) -> Result<String> {
        let attempt = self
            .store
            .get_attempt(attempt_id)?
            .ok_or_else(|| anyhow!("no such attempt: {attempt_id}"))?;
        if let Some(frozen) = attempt.frozen_diff {
            return match against {
                None | Some(0) => Ok(frozen),
                Some(n) => Err(anyhow!(
                    "a finished attempt has no checkpoint #{n} to compare against"
                )),
            };
        }
        // Asked once for the attempt, not once per checkout: a checkpoint is
        // a moment in the work, and a number no moment carries is a mistake
        // to report rather than a baseline to approximate. Without this the
        // per-checkout `at_or_before` would quietly answer with an older
        // snapshot — and `restore_checkpoint`, which does check, would refuse
        // the very number the drawer had just diffed against.
        if let Some(n) = against.filter(|n| *n > 0) {
            if !self.list_checkpoints(attempt_id)?.iter().any(|c| c.n == n) {
                return Err(anyhow!("this attempt has no checkpoint #{n}"));
            }
        }
        let mut out = String::new();
        for tree in self.trees(&attempt)? {
            let piece = self.tree_diff_from(&attempt, &tree, against)?;
            if piece.is_empty() {
                continue;
            }
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&piece);
        }
        Ok(out)
    }

    /// One checkout's share of that diff.
    fn tree_diff_from(
        &self,
        attempt: &StoredAttempt,
        tree: &StoredTree,
        against: Option<u64>,
    ) -> Result<String> {
        // Parked: no worktree to diff against, but the shelf checkpoint is
        // the worktree as it was parked — so the diff runs tree against
        // tree in the main checkout, ending at the shelf.
        if attempt.parked_at.is_some() {
            let (repo_loc, he) = self.located(&tree.repo_path)?;
            let hr = he.hr(&self.env);
            let cps = self
                .worktrees
                .checkpoints(&hr, &repo_loc.path, &attempt.id)?;
            let Some(last) = cps.last() else {
                // Parked clean at base: nothing ever changed here.
                return Ok(String::new());
            };
            let from = match against {
                None | Some(0) => tree.base_sha.clone(),
                // At-or-before, not exactly-at: a checkout untouched at that
                // moment grew no ref for it, and the honest baseline is the
                // newest snapshot it does have.
                Some(n) => worktree::at_or_before(&cps, n)
                    .map(|c| c.sha.clone())
                    .unwrap_or_else(|| tree.base_sha.clone()),
            };
            if from == last.sha {
                return Ok(String::new());
            }
            return self
                .worktrees
                .diff_range(&hr, &repo_loc.path, &from, &last.sha, &tree.dir);
        }
        let (wt_loc, he) = self.located(&tree.worktree_path)?;
        let hr = he.hr(&self.env);
        let base = match against {
            None | Some(0) => tree.base_sha.clone(),
            Some(n) => {
                let cps = self.worktrees.checkpoints(&hr, &wt_loc.path, &attempt.id)?;
                worktree::at_or_before(&cps, n)
                    .map(|c| c.sha.clone())
                    .unwrap_or_else(|| tree.base_sha.clone())
            }
        };
        self.worktrees.diff(&hr, &wt_loc.path, &base, &tree.dir)
    }

    /// Which checkout a path from the diff belongs to, and what it is called
    /// inside that checkout's own repository.
    ///
    /// The paths the drawer hands back are the ones it was shown — relative
    /// to the directory the session stands in — so for a card spanning two
    /// repositories the first component names the checkout. A path in no
    /// checkout is refused rather than resolved against the first: writing
    /// the client's file into the service is the failure this lookup exists
    /// to prevent.
    fn tree_for_path<'a>(
        &self,
        trees: &'a [StoredTree],
        path: &str,
    ) -> Result<(&'a StoredTree, String)> {
        if let [only] = trees {
            if only.dir.is_empty() {
                return Ok((only, path.to_string()));
            }
        }
        for tree in trees {
            if let Some(rest) = path.strip_prefix(&format!("{}/", tree.dir)) {
                if !rest.is_empty() {
                    return Ok((tree, rest.to_string()));
                }
            }
        }
        Err(anyhow!(
            "`{path}` is not inside any of this attempt's checkouts"
        ))
    }

    /* ---------------------------- worlds --------------------------- */

    /// Enumerate the worlds a card could live in. Cheap by construction:
    /// one `wsl.exe -l -q` (milliseconds, and an instant failure anywhere
    /// wsl.exe does not exist) and one local file read — never a remote
    /// probe, so a dead SSH host cannot slow the menu down.
    pub fn list_worlds(&self) -> Worlds {
        let wsl = std::process::Command::new("wsl.exe")
            .args(["-l", "-q"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| host::parse_wsl_list(&o.stdout))
            .unwrap_or_default();
        let ssh = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok()
            .map(|home| std::path::Path::new(&home).join(".ssh").join("config"))
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|text| host::parse_ssh_config(&text))
            .unwrap_or_default();
        Worlds { wsl, ssh }
    }

    /// Ask one world whether it is reachable and what claude it carries.
    /// `world` is the stored-path prefix ('' for local, `wsl://Ubuntu`,
    /// `ssh://devbox`). Runs the full login-shell probe on first contact
    /// and answers from the `hosts` cache afterwards — the same cache a
    /// card's first attempt would warm anyway.
    pub fn probe_world(&self, world: &str) -> WorldProbe {
        let raw = if world.is_empty() {
            "/".to_string()
        } else {
            format!("{world}/")
        };
        let spell = |v: Option<(u64, u64, u64)>| v.map(|(a, b, c)| format!("{a}.{b}.{c}"));
        match self.located(&raw) {
            Ok((_, he)) => WorldProbe {
                claude: spell(he.versions.claude),
                codex: spell(he.versions.codex),
                error: None,
            },
            Err(e) => WorldProbe {
                claude: None,
                codex: None,
                error: Some(format!("{e:#}")),
            },
        }
    }

    /// List one directory inside a world, for the folder picker.
    ///
    /// `path` of `None` means "start where a person starts", which is that
    /// world's own home — not this machine's, and not a remembered path that
    /// may not exist over there.
    ///
    /// Absolute paths are the whole point here, so the invoke boundary's
    /// usual refusal of them does not apply: that rule guards a *relative*
    /// path being resolved inside an attempt's worktree, where an absolute
    /// one would escape it. This is a person browsing their own filesystem
    /// at their own request, and the only thing it can read is the names of
    /// directories.
    pub fn list_dir(&self, world: &str, path: Option<&str>) -> Result<DirListing> {
        // Resolve the world through the same door every other path takes, so
        // `wsl://Ubuntu` and `ssh://devbox` mean here what they mean anywhere.
        let probe = if world.is_empty() {
            "/".to_string()
        } else {
            format!("{world}/")
        };
        let (_, he) = self.located(&probe)?;
        let hr = he.hr(&self.env);

        let start = match path {
            Some(p) if !p.trim().is_empty() => p.trim().to_string(),
            // The world's own HOME. Windows has no HOME to speak of, so the
            // profile directory answers for it there.
            _ => he
                .env
                .vars
                .get("HOME")
                .or_else(|| he.env.vars.get("USERPROFILE"))
                .cloned()
                .unwrap_or_else(|| "/".to_string()),
        };

        let (resolved, mut dirs) = match &he.host {
            // Locally this is a filesystem call, not a shell. Windows is the
            // reason: `sh` is not on a Windows login-shell PATH, and the one
            // world that would need the fallback most is the one that cannot
            // run it.
            Host::Local => {
                let base = std::path::Path::new(&start)
                    .canonicalize()
                    .with_context(|| format!("{start} cannot be opened"))?;
                let mut names = Vec::new();
                for entry in std::fs::read_dir(&base)
                    .with_context(|| format!("{} cannot be read", base.display()))?
                    .flatten()
                {
                    // `file_type` rather than `metadata`: a symlink pointing
                    // nowhere should be skipped, not raise an error that
                    // hides every sibling it stands next to.
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        names.push(entry.file_name().to_string_lossy().to_string());
                    }
                }
                (base.to_string_lossy().to_string(), names)
            }
            _ => {
                // The path rides as the working directory rather than inside
                // the script, so a directory with a quote or a space in its
                // name is carried by the doorway's own escaping instead of
                // this string's.
                let out = hr
                    .run_ok("sh", &["-c", LIST_DIR], Some(&start))
                    .with_context(|| format!("{start} cannot be opened"))?;
                let mut lines = out.lines();
                let resolved = lines
                    .next()
                    .ok_or_else(|| anyhow!("{start} answered with nothing"))?
                    .to_string();
                (resolved, lines.map(|s| s.to_string()).collect())
            }
        };

        // Dotfiles last, then alphabetical within each half: a home directory
        // is mostly `.config`-shaped noise, and the thing being looked for is
        // almost never in it.
        dirs.sort_by(|a, b| {
            let dot = |s: &String| s.starts_with('.');
            dot(a)
                .cmp(&dot(b))
                .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
        });

        let parent = parent_of(&resolved);
        let is_repo = hr.exists(&hr.join(&resolved, ".git"));
        Ok(DirListing {
            path: resolved,
            parent,
            dirs,
            is_repo,
        })
    }

    /// Both sides of one file in the diff, as full text — what the editable
    /// diff edits. The base side comes from the attempt's recorded base
    /// commit, the work side from the worktree as it stands. A finished or
    /// parked attempt has no ground to read: its diff is a record.
    pub fn attempt_file(&self, attempt_id: &str, path: &str) -> Result<AttemptFile> {
        ensure_worktree_relative(path)?;
        let attempt = self
            .store
            .get_attempt(attempt_id)?
            .ok_or_else(|| anyhow!("no such attempt: {attempt_id}"))?;
        if attempt.outcome.is_some() {
            return Err(anyhow!(
                "this attempt is finished; its files are read-only"
            ));
        }
        if attempt.parked_at.is_some() {
            return Err(anyhow!("this attempt is parked; resume it to read its files"));
        }
        let trees = self.trees(&attempt)?;
        let (tree, rel) = self.tree_for_path(&trees, path)?;
        let (wt_loc, he) = self.located(&tree.worktree_path)?;
        let hr = he.hr(&self.env);
        let base = self
            .worktrees
            .file_at_rev(&hr, &wt_loc.path, &tree.base_sha, &rel)?;
        let work = hr.read_to_string(&hr.join(&wt_loc.path, &rel))?;
        Ok(AttemptFile { base, work })
    }

    /// Write one file in the attempt's worktree — a human's own edit, made
    /// where the eye already is. This is not the app touching agent state:
    /// a person can change any file in their repository with any editor,
    /// and this only removes the navigation. Restore's two rules carry
    /// over whole: settled only — re-verified here, because UI gating goes
    /// stale — and the "tell the agent" note stays a human act, upstairs.
    ///
    /// `expected` is the text the editor believes the disk holds — what it
    /// loaded, or last saved. When it is given and the disk disagrees, the
    /// save is refused: a shell, a run script, or a turn that started and
    /// settled while the editor sat open has written here, and last-write-
    /// wins would destroy that work without anyone seeing it go.
    pub fn write_attempt_file(
        &self,
        attempt_id: &str,
        path: &str,
        contents: &str,
        expected: Option<&str>,
    ) -> Result<()> {
        ensure_worktree_relative(path)?;
        let attempt = self
            .store
            .get_attempt(attempt_id)?
            .ok_or_else(|| anyhow!("no such attempt: {attempt_id}"))?;
        if attempt.outcome.is_some() {
            return Err(anyhow!(
                "this attempt is finished; its files are read-only"
            ));
        }
        if attempt.parked_at.is_some() {
            return Err(anyhow!(
                "this attempt is parked; resume it first, then edit"
            ));
        }
        let busy = self.sessions.lock().unwrap().values().any(|s| {
            s.attempt_id.as_deref() == Some(attempt_id)
                && s.live
                && !matches!(s.status, Status::Idle | Status::Saved | Status::Exited)
        });
        if busy {
            return Err(anyhow!(
                "the agent is mid-turn in this worktree. Saving now would change files under \
                 its feet while it is still writing its own. Wait for the turn to end — or \
                 close the session — and save then"
            ));
        }
        let trees = self.trees(&attempt)?;
        let (tree, rel) = self.tree_for_path(&trees, path)?;
        let (wt_loc, he) = self.located(&tree.worktree_path)?;
        let hr = he.hr(&self.env);
        let full = hr.join(&wt_loc.path, &rel);
        if let Some(expected) = expected {
            let current = hr.read_to_string(&full)?.unwrap_or_default();
            if current != expected {
                return Err(anyhow!(
                    "{path} changed on disk after the editor read it — a shell, a script, or \
                     another turn wrote here. Close the editor and reopen it to see the current \
                     text; saving now would overwrite that work unseen"
                ));
            }
        }
        hr.write_file(&full, contents)?;
        Ok(())
    }

    /// The first message for this card.
    ///
    /// With the worktrees in hand it names the branch and the bases that were
    /// really handed out. Without them — previewing, or queueing before
    /// anything has been created — it names the best guess available, and
    /// `open_attempt` renders again against what git actually gave it.
    fn render_prompt(
        &self,
        task: &StoredTask,
        wt: Option<&worktree::OpenedWorktree>,
    ) -> Result<String> {
        let locale = self.locale.get();
        let template = prompt::load_or_create(&self.data_dir, locale)?;

        // `(dir, repo, base_branch, base_sha)`, owned here so the borrowed
        // `TreeVar`s below have something to point at.
        let (branch, trees): (String, Vec<(String, String, String, String)>) = match wt {
            Some(w) => (
                w.branch.clone(),
                w.trees
                    .iter()
                    .map(|t| {
                        (
                            t.dir.clone(),
                            t.repo.clone(),
                            t.base_branch.clone(),
                            t.base_sha.clone(),
                        )
                    })
                    .collect(),
            ),
            None => {
                let seq = self.store.next_attempt_seq(&task.id)?;
                let slug = worktree::slug(&task.title, &task.id);
                let repos = task.repos();
                let specs = self.repo_specs(task).unwrap_or_default();
                let dirs = worktree::preview_dirs(&specs);
                let trees = repos
                    .iter()
                    .zip(dirs)
                    .map(|(r, dir)| {
                        let sha = self
                            .located(&r.repo_path)
                            .and_then(|(loc, he)| {
                                self.worktrees
                                    .head_of(&he.hr(&self.env), &loc.path, &r.base_branch)
                            })
                            .unwrap_or_default();
                        (dir, r.repo_path.clone(), r.base_branch.clone(), sha)
                    })
                    .collect();
                (format!("marol/{slug}-{seq}"), trees)
            }
        };

        let vars: Vec<prompt::TreeVar> = trees
            .iter()
            .map(|(dir, repo, base_branch, base_sha)| prompt::TreeVar {
                dir,
                repo,
                base_branch,
                base_sha,
            })
            .collect();
        // The first checkout is what `{base_branch}` and `{base_sha}` have
        // always meant, and templates written before a card could span two
        // still say them.
        let first = vars.first();
        Ok(prompt::render(
            &template,
            &prompt::Vars {
                title: &task.title,
                branch: &branch,
                base_branch: first.map(|t| t.base_branch).unwrap_or(&task.base_branch),
                base_sha: first.map(|t| t.base_sha).unwrap_or(""),
                trees: &vars,
                prompt: &task.prompt,
                locale,
            },
        ))
    }

    /* ---------------------------- finishing ------------------------ */

    /// Fold an attempt's branch back into its base, then close the attempt
    /// out. The merge has to succeed before anything is given up.
    ///
    /// For a card spanning several repositories that is several merges, and
    /// they are checked *all* before any of them runs. Every refusal
    /// `merge_to_base` makes is one that would otherwise lose work quietly —
    /// uncommitted changes, a checkout sitting on the wrong branch — and
    /// discovering the second repository's on the far side of having already
    /// mutated the first is the one shape of that failure this app can still
    /// prevent. A dry run is not a promise: the second merge can still fail
    /// on a conflict once the first has landed, and then what happened is
    /// reported rather than pretended away. But it turns the common case —
    /// somebody forgot to commit in one of the two — back into the plain
    /// refusal it is everywhere else.
    pub fn merge_attempt(&self, attempt_id: &str) -> Result<String> {
        let attempt = self
            .store
            .get_attempt(attempt_id)?
            .ok_or_else(|| anyhow!("no such attempt: {attempt_id}"))?;
        let trees = self.trees(&attempt)?;
        let locale = self.locale.get();

        let mut situated = Vec::with_capacity(trees.len());
        for tree in &trees {
            let (repo_loc, he) = self.located(&tree.repo_path)?;
            let wt_loc = host::locate(&tree.worktree_path)?;
            self.worktrees.check_merge(
                &he.hr(&self.env),
                &repo_loc.path,
                &wt_loc.path,
                &tree.branch,
                &tree.base_branch,
                locale,
            )?;
            situated.push((tree, repo_loc, he, wt_loc));
        }

        let mut lines = Vec::with_capacity(situated.len());
        let mut done: Vec<&StoredTree> = Vec::new();
        for (tree, repo_loc, he, wt_loc) in &situated {
            let merged = self.worktrees.merge_to_base(
                &he.hr(&self.env),
                &repo_loc.path,
                &wt_loc.path,
                &tree.branch,
                &tree.base_branch,
                locale,
            );
            match merged {
                Ok(sha) => {
                    lines.push(if tree.dir.is_empty() {
                        sha
                    } else {
                        format!("{}: {sha}", tree.dir)
                    });
                    done.push(tree);
                }
                // Nothing is rolled back and the attempt stays open: the
                // repositories that landed are landed, and saying which is
                // the only way the person can finish the job by hand.
                Err(e) => {
                    let landed = done
                        .iter()
                        .map(|t| t.repo_path.as_str())
                        .collect::<Vec<_>>()
                        .join(i18n::list_sep(locale));
                    return Err(anyhow!(i18n::merge_partial(
                        locale,
                        &tree.repo_path,
                        &format!("{e:#}"),
                        &landed,
                    )));
                }
            }
        }
        let sha = lines.join("\n");

        self.close_attempt(&attempt, Outcome::Merged)?;

        // The merge is the moment the card's question is answered. Any other
        // attempt still open on it was a candidate for the same work, and the
        // candidate that did not land is superseded — not discarded, because
        // nobody threw it away; it lost. Its diff freezes like any close, so
        // comparing what the losing agent did remains possible afterwards.
        for other in self.store.list_attempts(&attempt.task_id)? {
            if other.id != attempt.id && other.outcome.is_none() {
                if let Err(e) = self.close_attempt(&other, Outcome::Superseded) {
                    // The merge itself succeeded; a sibling whose worktree
                    // would not come back is a mess to report, not to undo.
                    eprintln!("[core] superseding attempt {} failed: {e:#}", other.id);
                }
            }
        }

        self.emit_tasks();
        self.broadcast();
        self.drain_queue();
        Ok(sha)
    }

    /// Send a later message into an attempt's live terminal.
    ///
    /// This is how the review drawer answers "what is still wrong" without a
    /// navigation: the composed feedback goes in through the PTY the same way
    /// a person's paste would, and lands on the timeline as what was actually
    /// asked. Only for CLIs whose conventions are measured — for the rest the
    /// text is the person's to paste, exactly like the first prompt.
    pub fn send_followup(&self, session_id: &str, text: &str) -> Result<()> {
        if text.trim().is_empty() {
            return Err(anyhow!("nothing to send"));
        }
        let (agent, attempt_id) = {
            let sessions = self.sessions.lock().unwrap();
            let s = sessions
                .get(session_id)
                .ok_or_else(|| anyhow!("no such session: {session_id}"))?;
            (s.agent.clone(), s.attempt_id.clone())
        };
        if prompt::delivery_for(&agent) != Delivery::Positional {
            return Err(anyhow!(
                "`{agent}`'s input conventions have not been measured; copy the text in instead"
            ));
        }

        self.write(session_id, &prompt::bracketed_followup(text))?;

        // Recorded as sent, like the first prompt: the timeline is the record
        // of what the agent was asked, follow-ups included.
        if let Some(id) = attempt_id {
            let _ = self.store.append_event(&id, now_ms(), "prompt", None, Some(text));
        }
        Ok(())
    }

    /// The repository's branches, recency first, for the base picker.
    pub fn list_branches(&self, repo_path: &str) -> Result<Vec<String>> {
        let (loc, he) = self.located(repo_path)?;
        self.worktrees.branches(&he.hr(&self.env), &loc.path)
    }

    /// Hold a message for the end of this turn.
    ///
    /// The same gates as sending now — a live terminal, measured input
    /// conventions — checked at queue time, because a refusal the moment
    /// you press the button beats one after the turn you waited out.
    pub fn queue_followup(&self, session_id: &str, text: &str) -> Result<()> {
        if text.trim().is_empty() {
            return Err(anyhow!("nothing to queue"));
        }
        {
            let sessions = self.sessions.lock().unwrap();
            let s = sessions
                .get(session_id)
                .ok_or_else(|| anyhow!("no such session: {session_id}"))?;
            if !s.live {
                return Err(anyhow!("no terminal for session {session_id}"));
            }
            if prompt::delivery_for(&s.agent) != Delivery::Positional {
                return Err(anyhow!(
                    "`{}`'s input conventions have not been measured; copy the text in instead",
                    s.agent
                ));
            }
        }
        self.enqueue_followup(session_id, text, None)
    }

    /// Put one message on a session's queue, from a person (`from` absent) or
    /// from another session (`from` naming it).
    ///
    /// The refusal is the point of the cap: a caller that is told "full"
    /// can say so to whoever sent the message, which is exactly what the
    /// single slot could never do.
    fn enqueue_followup(&self, session_id: &str, text: &str, from: Option<String>) -> Result<()> {
        {
            let mut queues = self.followups.lock().unwrap();
            let queue = queues.entry(session_id.to_string()).or_default();
            if queue.len() >= MAX_PENDING {
                return Err(anyhow!(
                    "{} already has {MAX_PENDING} messages waiting for its turn to end",
                    session_id
                ));
            }
            queue.push_back(Pending {
                text: text.to_string(),
                from,
            });
        }
        self.set_followup_flag(session_id, true);
        Ok(())
    }

    pub fn cancel_followup(&self, session_id: &str) {
        self.followups.lock().unwrap().remove(session_id);
        self.set_followup_flag(session_id, false);
    }

    /// The Stop hook's half: the turn just ended, so what waited for it
    /// goes in as the next one — through the same paste a live follow-up
    /// uses, recorded on the timeline the same way.
    pub(crate) fn flush_followup(&self, session_id: &str) {
        let pending: Vec<Pending> = match self.followups.lock().unwrap().remove(session_id) {
            Some(q) if !q.is_empty() => q.into(),
            _ => return,
        };
        // Coalesced into one delivery rather than sent one after another.
        // Each send is a paste followed by a return, so a second one would
        // land in the middle of the turn the first just started — the exact
        // interleaving the end-of-turn queue exists to avoid. Several
        // messages become several paragraphs of one turn.
        let text = pending
            .iter()
            .map(Pending::rendered)
            .collect::<Vec<_>>()
            .join("\n\n");
        if let Err(e) = self.send_followup(session_id, &text) {
            // The session died between queue and Stop. The message is
            // dropped rather than retried into a terminal that is gone.
            eprintln!("[core] queued follow-up for {session_id} failed: {e:#}");
        }
        self.set_followup_flag(session_id, false);
    }

    fn set_followup_flag(&self, session_id: &str, value: bool) {
        if let Some(s) = self.sessions.lock().unwrap().get_mut(session_id) {
            s.has_followup = value;
        }
        self.broadcast();
    }

    /// Push the branch and open a pull request.
    ///
    /// The attempt is deliberately *not* closed out: the worktree stays until
    /// the pull request is resolved, because that is when there is still
    /// something to change in response to review. Reviewing and merging a
    /// pull request is somebody else's tool.
    /// One pull request per repository the card spans, in order, and every
    /// URL comes back.
    ///
    /// They cannot be one pull request — a pull request belongs to a
    /// repository — so what this can honestly offer is the set of them, each
    /// pointing at the same branch name and carrying the same description.
    /// A failure part-way stops there and names what already went up: the
    /// pull requests that opened are open, and a person finishing by hand
    /// needs to know which.
    pub fn open_pr(&self, attempt_id: &str) -> Result<String> {
        let attempt = self
            .store
            .get_attempt(attempt_id)?
            .ok_or_else(|| anyhow!("no such attempt: {attempt_id}"))?;
        let task = self.task(&attempt.task_id)?;
        let trees = self.trees(&attempt)?;
        let locale = self.locale.get();

        let mut urls: Vec<String> = Vec::with_capacity(trees.len());
        for tree in &trees {
            let body = format!(
                "Marol attempt #{} ({}), from `{}` @ {}.\n\n---\n\n{}",
                attempt.seq,
                attempt.agent,
                tree.base_branch,
                &tree.base_sha[..tree.base_sha.len().min(8)],
                task.prompt
            );
            let (wt_loc, he) = self.located(&tree.worktree_path)?;
            let opened = self.worktrees.push_and_open_pr(
                &he.hr(&self.env),
                &wt_loc.path,
                &tree.branch,
                &tree.base_branch,
                &task.title,
                &body,
                locale,
            );
            match opened {
                Ok(url) => urls.push(url),
                Err(e) if urls.is_empty() => return Err(e),
                Err(e) => {
                    return Err(anyhow!(i18n::pr_partial(
                        locale,
                        &tree.repo_path,
                        &format!("{e:#}"),
                        &urls.join("\n"),
                    )))
                }
            }
        }
        Ok(urls.join("\n"))
    }

    fn task(&self, id: &str) -> Result<StoredTask> {
        self.store
            .list_tasks()?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| anyhow!("no such task: {id}"))
    }

    fn emit_tasks(&self) {
        if let Ok(v) = serde_json::to_value(self.task_board()) {
            self.sink.emit("tasks:changed", v);
        }
    }

    /* --------------------------- commands -------------------------- */

    /// Open a new terminal session running `agent` in `cwd`.
    ///
    /// `extra_args` is passed through verbatim — `--continue`, `--model
    /// sonnet`, anything the CLI accepts, exactly as the user would type it.
    pub fn new_session(
        &self,
        cwd: String,
        agent: String,
        extra_args: Vec<String>,
        cols: u16,
        rows: u16,
    ) -> Result<String> {
        // A profile name resolves to its CLI and standing arguments; the
        // person's own arguments come after, so they can override. The row
        // remembers the resolved CLI — reopening runs `claude`, whatever the
        // profile was called.
        let (agent, mut opts) = self.resolve_launcher(&agent);
        opts.extend(extra_args);
        let extra_args = opts;
        let id = uuid::Uuid::new_v4().to_string();
        let base = std::path::Path::new(&cwd)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| cwd.clone());
        let title = self.unique_title(&base);
        let at = now_ms();

        let meta = SessionMeta {
            id: id.clone(),
            cwd: cwd.clone(),
            title,
            agent: agent.clone(),
            status: Status::Starting,
            created_at: at,
            last_active_at: at,
            live: true,
            reports_status: false,
            hooks_wired: false,
            activity: None,
            activity_since: 0,
            completed: false,
            attempt_id: None,
            // A session opened without a card is still an agent, and is held
            // and lost on the same terms as one with a card.
            agent_session: true,
            has_followup: false,
            preview_port: None,
            usage: None,
            transcript_path: None,
        };

        // On the record before it can exit — see `finish_opening`.
        self.sessions.lock().unwrap().insert(id.clone(), meta.clone());
        if let Err(e) = self.launch(&id, &agent, extra_args, Tail::Nothing, &cwd, cols, rows, None) {
            self.sessions.lock().unwrap().remove(&id);
            return Err(e);
        }

        self.persist(&meta);
        self.broadcast();
        Ok(id)
    }

    /// Reattach a terminal to a saved session, continuing the agent's own
    /// conversation history in that directory.
    pub fn reopen_session(&self, id: &str, cols: u16, rows: u16) -> Result<()> {
        let meta = self
            .sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("no such session: {id}"))?;

        if self.ptys.is_live(id) {
            return Err(anyhow!("session {id} already has a terminal"));
        }

        // Each CLI's own way of picking up the most recent conversation in
        // this directory, which is what reopening means to the user. An
        // attempt session resumes with the permission mode chosen for the
        // attempt — approved once, kept until the attempt ends; an ad-hoc
        // session has no attempt and so no mode to carry.
        let mode = meta
            .attempt_id
            .as_deref()
            .and_then(|id| self.store.get_attempt(id).ok().flatten())
            .map(|a| a.mode)
            .unwrap_or_default();
        let (args, tail) = resume_line(Cli::of(&meta.agent), mode);

        // Marked live before the launch, not after: a child that exits at
        // once reports Exited in between, and writing "starting, live" over
        // that report would leave a zombie row nothing can ever update.
        //
        // **Attaching is not starting.** A session tmux held through a
        // restart is already running, and `new-session -A -D` reattaches to
        // it and drops the argv on the floor — so no `SessionStart` fires,
        // and nothing would ever move the row off 啟動中. It would sit there
        // for the rest of the session's life, which is the same lie the
        // status label used to tell, told from the other side.
        //
        // So the honest state is kept: running, and this desk has not heard
        // from it yet. The first real hook replaces it, which is now a thing
        // that can happen again (see `hooks::start`).
        let attaching = self
            .sessions
            .lock()
            .unwrap()
            .get(id)
            .map(|s| s.status == Status::Detached)
            .unwrap_or(false);
        if let Some(s) = self.sessions.lock().unwrap().get_mut(id) {
            s.status = if attaching {
                Status::Detached
            } else {
                Status::Starting
            };
            s.live = true;
            s.last_active_at = now_ms();
        }
        if let Err(e) = self.launch(id, &meta.agent, args, tail, &meta.cwd, cols, rows, None) {
            if let Some(s) = self.sessions.lock().unwrap().get_mut(id) {
                s.status = Status::Saved;
                s.live = false;
            }
            return Err(e);
        }
        self.broadcast();
        Ok(())
    }

    /// Spawn the PTY, adding the status plugin and its identifying env.
    ///
    /// `opts` and `positional` are kept apart so the command line can be
    /// assembled in the only order that is safe: every option, then the
    /// prompt. Appending `--plugin-dir` to a vector that already ended with
    /// the prompt would put a positional argument in front of an option and
    /// leave the parse to the CLI's goodwill.
    ///
    /// With a `setup` wrap, the launch becomes `sh -c 'set -e; <setup>;
    /// exec "$0" "$@"' <agent> <args…>` — the script runs first, in the same
    /// terminal, and then *becomes* the agent. The arguments ride as real
    /// argv entries, untouched by the shell, so the multi-line prompt needs
    /// no quoting and arrives exactly as it would have without the wrap.
    #[allow(clippy::too_many_arguments)]
    fn launch(
        &self,
        id: &str,
        agent: &str,
        opts: Vec<String>,
        tail: Tail,
        cwd: &str,
        cols: u16,
        rows: u16,
        setup: Option<&Setup>,
    ) -> Result<()> {
        // Which world this session's directory lives in decides everything
        // below: whose CLI, whose PATH, and whether the whole command
        // line gets wrapped through the doorway.
        let loc = host::locate(cwd)?;
        let he = self.host_env(&loc.host)?;

        let cli = Cli::of(agent);
        let mut session_env = Vec::new();
        // Which session this is, for whatever ends up reporting on it.
        // Every hook reports under this id — Claude Code expands it into a
        // header, Codex into the query of its curl — and it is set for any
        // session in a world that has a listener at all, whether or not
        // this app knows how to wire that CLI up: it is a fact about the
        // session rather than about the CLI, and somebody's own hook is
        // entitled to read it too.
        if let Some(wiring) = &he.hooks {
            session_env.push(("MAROL_SESSION_ID".to_string(), id.to_string()));
            // Where a session says what it should be called. This session's
            // own address, id and all, so an agent uses one variable
            // verbatim instead of composing a URL under whichever shell the
            // platform handed it.
            //
            // It carries the listener's token, which the session could
            // already read out of the plugin `--plugin-dir` points at — this
            // hands the agent nothing its own configuration did not already
            // give it. What it does add is that the agent's *subprocesses*
            // inherit it, which is the price of the one-liner working from
            // inside a tool call at all.
            session_env.push((
                "MAROL_NAME_URL".to_string(),
                hooks::name_url(&wiring.url, id),
            ));
            // The two channels a session can *ask* on, each carrying a token
            // minted for this session alone. Minted fresh per launch, so a
            // token learned from a session that has since been restarted
            // stops working — and never stored, because it is worth exactly
            // one running process.
            let token = uuid::Uuid::new_v4().simple().to_string();
            self.send_tokens
                .lock()
                .unwrap()
                .insert(id.to_string(), token.clone());
            session_env.push((
                "MAROL_PEERS_URL".to_string(),
                hooks::peers_url(&wiring.url, id, &token),
            ));
            session_env.push((
                "MAROL_SEND_URL".to_string(),
                hooks::send_url(&wiring.url, id, &token),
            ));
        }

        // Whether this world can be wired at all was settled when the host
        // was first contacted; see `host_env`. Whether *this CLI* can be is
        // its own version's business, and unknown means no — an unrecognised
        // flag is the one failure a person cannot work around from inside
        // the terminal, because there is no terminal.
        let hook_args = match (cli, &he.hooks) {
            (Some(cli), Some(wiring)) if cli.hooks_ok(he.versions.of(cli)) => cli.hook_args(wiring),
            _ => Vec::new(),
        };

        // Recorded, not inferred later: this is the moment the answer is
        // known, and it is the only moment — the version that decided it
        // belongs to the world this session launched into, and nothing
        // downstream can see that world again.
        if let Some(s) = self.sessions.lock().unwrap().get_mut(id) {
            s.hooks_wired = !hook_args.is_empty();
        }

        // Cross-session messaging addresses a session by name, and left to
        // itself the CLI derives one from the worktree's directory — a slug
        // with a counter. Marol knows the card, so a claude session is
        // named what its own list calls it: 「修好登入 #1」, reachable by the
        // name a person would actually say. Version-gated on the claude that
        // will actually run — the host's — because an older CLI refuses to
        // start on a flag it does not know. Codex has no equivalent, and is
        // handed nothing rather than a guess.
        let mut opts = opts;
        if cli == Some(Cli::Claude) && he.versions.claude >= Some(NAMED_SESSIONS_SINCE) {
            if let Some(title) = self.sessions.lock().unwrap().get(id).map(|s| s.title.clone()) {
                opts.push("--name".to_string());
                opts.push(title);
            }
        }

        let args = build_args(opts, hook_args, tail);

        // A local host needs a local POSIX shell for the setup wrap; a WSL
        // host brings its own.
        let posix = cfg!(unix) || !matches!(he.host, Host::Local);
        let (program, args) = match setup {
            Some(wrap) if posix => {
                session_env.extend(under_both_names(vec![(
                "MAROL_ROOT_PATH".to_string(),
                wrap.root_path.clone(),
            )]));
                // `set -e` so a failed setup stops in front of the person,
                // in the terminal, instead of starting an agent in a
                // half-made workspace.
                let script = format!("set -e\n{}\nexec \"$0\" \"$@\"", wrap.script());
                let mut wrapped = vec!["-c".to_string(), script, agent.to_string()];
                wrapped.extend(args);
                ("sh".to_string(), wrapped)
            }
            Some(_) => {
                eprintln!(
                    "[core] setup scripts need a POSIX shell; launching {agent} directly"
                );
                (agent.to_string(), args)
            }
            None => (agent.to_string(), args),
        };

        // Only the agent's own session is held. A run script and a worktree
        // shell are things you started to watch; an agent is a thing you
        // started to leave running, and that difference is the whole reason
        // to involve tmux at all.
        //
        // Composed here, *before* the doorway, so the tmux that holds the
        // process is the one belonging to the world the process runs in. Put
        // it after the wrap and it could only ever be this machine's — which
        // is useless for a WSL world, since a WSL world only exists on a
        // Windows host and there is no native Windows tmux to be the holder.
        let plan = self.hold_plan(&he, id);
        let (program, args) = match &plan {
            Some(p) => pty::hold_attach(&p.socket, &p.conf, Some(&loc.path), &program, &args),
            None => (program, args),
        };

        // Locally the PTY applies cwd and env natively; inside a host both
        // ride the wrapped argv, and the outer process is the doorway.
        let (program, args, outer_cwd, outer_env): (String, Vec<String>, Option<String>, Vec<(String, String)>) =
            match &he.host {
                Host::Local => (program, args, Some(loc.path.clone()), session_env),
                _ => {
                    let envs = host::pty_env(&he.env, &session_env);
                    let (p, a, _) = he.host.wrap(&program, &args, Some(&loc.path), &envs);
                    (p, a, None, Vec::new())
                }
            };

        // Ending it travels the same road, but not in the same company: this
        // one runs from a close button with no terminal behind it, so it goes
        // through the quiet doorway. Over SSH that is the difference between
        // failing in a moment and hanging for ever on a password prompt
        // nothing can answer.
        let hold = plan.map(|p| {
            let (dp, da) = pty::hold_destroy(&p.socket);
            let (dp, da) = match &he.host {
                Host::Local => (dp, da),
                _ => he.host.wrap_quiet(&dp, &da),
            };
            pty::Hold {
                destroy: (dp, da),
                socket_file: p.socket_file,
            }
        });

        self.ptys.spawn(
            id,
            &program,
            &args,
            outer_cwd.as_deref(),
            &self.env,
            &outer_env,
            cols.max(20),
            rows.max(5),
            Arc::clone(&self.router) as Arc<dyn PtySink>,
            hold.as_ref(),
        )
    }

    /// Forward keystrokes to the terminal, verbatim.
    pub fn write(&self, id: &str, data: &str) -> Result<()> {
        if let Some(s) = self.sessions.lock().unwrap().get_mut(id) {
            s.last_active_at = now_ms();
        }
        self.ptys.write(id, data)
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<()> {
        self.ptys.resize(id, cols.max(20), rows.max(5))
    }

    /// Everything the terminal has emitted so far, for a pane that is only
    /// now mounting. See `PtyRegistry::snapshot`.
    pub fn snapshot(&self, id: &str) -> Result<(String, u64)> {
        self.ptys.snapshot(id)
    }

    pub fn close_session(&self, id: &str) -> Result<()> {
        self.ptys.kill(id);
        let freed = {
            let mut sessions = self.sessions.lock().unwrap();
            match sessions.get_mut(id) {
                Some(s) => {
                    s.status = Status::Saved;
                    s.live = false;
                    s.attempt_id.is_some()
                }
                None => false,
            }
        };
        self.broadcast();
        if freed {
            self.drain_queue();
            self.emit_tasks();
        }
        Ok(())
    }

    /// Drop the session from the list. Its scrollback is gone either way —
    /// the agent's own conversation history on disk is untouched.
    pub fn archive_session(&self, id: &str) -> Result<()> {
        self.ptys.kill(id);
        self.store.archive_session(id)?;
        self.sessions.lock().unwrap().remove(id);
        self.broadcast();
        Ok(())
    }

    /// Give a session a different name.
    ///
    /// Two callers, one path: a person editing the row in the sidebar, and
    /// the agent in the session posting to the plugin's own endpoint. The
    /// name a session wears is the same fact either way, so it gets the same
    /// cleaning and the same round trip through the store.
    ///
    /// **It renames the row, and only the row.** The name Claude Code answers
    /// to for cross-session messages is `--name`, fixed on the command line
    /// when the session started, and there is no way to move it from outside
    /// a running CLI. So a rename reaches that name at the session's next
    /// start and not before — which is worth saying plainly rather than
    /// leaving someone to address a name nothing replies to.
    pub fn rename_session(&self, id: &str, title: &str) -> Result<()> {
        let title =
            clean_title(title).ok_or_else(|| anyhow!("a session's name cannot be empty"))?;
        let meta = {
            let mut sessions = self.sessions.lock().unwrap();
            let s = sessions
                .get_mut(id)
                .ok_or_else(|| anyhow!("no such session: {id}"))?;
            if s.title == title {
                return Ok(());
            }
            s.title = title;
            s.clone()
        };
        self.persist(&meta);
        self.broadcast();
        Ok(())
    }

    /// Mark a session done, or undo that. Nothing infers this: `Stop` means
    /// "this turn ended", never "the work is finished".
    pub fn set_completed(&self, id: &str, completed: bool) -> Result<()> {
        let meta = {
            let mut sessions = self.sessions.lock().unwrap();
            let s = sessions
                .get_mut(id)
                .ok_or_else(|| anyhow!("no such session: {id}"))?;
            s.completed = completed;
            if completed {
                s.activity = None;
            }
            s.clone()
        };
        self.persist(&meta);
        self.broadcast();
        Ok(())
    }

    /* ---------------------------- tabs ----------------------------- */

    pub fn tabs(&self) -> Vec<StoredTab> {
        self.tabs.lock().unwrap().clone()
    }

    pub fn create_tab(&self, name: String) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let tab = {
            let mut tabs = self.tabs.lock().unwrap();
            let tab = StoredTab {
                id: id.clone(),
                name,
                layout: DEFAULT_LAYOUT.to_string(),
                slots: Vec::new(),
                position: tabs.len() as i64,
            };
            tabs.push(tab.clone());
            tab
        };
        self.store.upsert_tab(&tab)?;
        self.emit_tabs();
        Ok(id)
    }

    pub fn rename_tab(&self, id: &str, name: String) -> Result<()> {
        let tab = {
            let mut tabs = self.tabs.lock().unwrap();
            let t = tabs
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or_else(|| anyhow!("no such tab: {id}"))?;
            t.name = name;
            t.clone()
        };
        self.store.upsert_tab(&tab)?;
        self.emit_tabs();
        Ok(())
    }

    /// Remove a tab. The sessions it showed are untouched — they keep running
    /// and stay in the sidebar, because a tab is a view, not a container.
    pub fn close_tab(&self, id: &str) -> Result<()> {
        {
            let mut tabs = self.tabs.lock().unwrap();
            if tabs.len() <= 1 {
                return Err(anyhow!("the last tab cannot be closed"));
            }
            tabs.retain(|t| t.id != id);
            for (i, t) in tabs.iter_mut().enumerate() {
                t.position = i as i64;
            }
        }
        self.store.delete_tab(id)?;
        for t in self.tabs.lock().unwrap().iter() {
            let _ = self.store.upsert_tab(t);
        }
        self.emit_tabs();
        Ok(())
    }

    /// Set a tab's arrangement.
    ///
    /// A session appears in at most one tab. It owns a single PTY and
    /// therefore a single size, so being shown in two arrangements at once
    /// would mean resizing it against itself every time you switched. Claiming
    /// a session here removes it from wherever it was — it *leaves* the other
    /// tab's list rather than blanking a position in it, because a position
    /// nobody occupies is indistinguishable from one the user deliberately
    /// emptied, and the frontend used to have to guess between the two.
    pub fn update_tab(&self, id: &str, layout: String, slots: Vec<Option<String>>) -> Result<()> {
        let claimed: std::collections::HashSet<&str> =
            slots.iter().filter_map(|s| s.as_deref()).collect();

        let changed = {
            let mut tabs = self.tabs.lock().unwrap();
            if !tabs.iter().any(|t| t.id == id) {
                return Err(anyhow!("no such tab: {id}"));
            }
            for t in tabs.iter_mut() {
                if t.id == id {
                    t.layout = layout.clone();
                    t.slots = slots.clone();
                } else {
                    t.slots
                        .retain(|s| !s.as_deref().is_some_and(|x| claimed.contains(x)));
                }
            }
            tabs.clone()
        };

        for t in &changed {
            self.store.upsert_tab(t)?;
        }
        self.emit_tabs();
        Ok(())
    }

    fn emit_tabs(&self) {
        if let Ok(v) = serde_json::to_value(self.tabs()) {
            self.sink.emit("tabs:changed", v);
        }
    }

    pub fn shutdown(&self) {
        self.ptys.kill_all();
        // Give the hook port back. It is part of the address every held
        // session was told to report to, so the next run has to be able to
        // take it again — and a listener nobody stops keeps it for as long as
        // this process lives.
        if let Some(h) = self.hooks.get() {
            h.stop();
        }
        // Close the standing SSH connections, tunnels and all — with
        // ControlPersist they would otherwise outlive the app.
        for h in self.hosts.lock().unwrap().keys() {
            if let Host::Ssh { host } = h {
                host::close_ssh_master(&self.env, host);
            }
        }
    }

    pub fn sessions(&self) -> Vec<SessionMeta> {
        let mut v: Vec<_> = self.sessions.lock().unwrap().values().cloned().collect();
        v.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));
        v
    }

    pub fn hook_url(&self) -> Option<String> {
        self.hooks.get().map(|h| h.url())
    }

    /// Ask tmux which of the sessions we just loaded are still running.
    ///
    /// `from_stored` marks everything `Saved`, which was true when every
    /// terminal died with the app. It is not true any more, and a card that
    /// says "closed" over a working agent is worse than a missing feature —
    /// somebody reads it, believes the work is gone, and starts a second
    /// attempt onto the same worktree.
    ///
    /// Runs before the first paint rather than on a thread: a status that
    /// corrects itself a moment later is a flicker on the one surface whose
    /// whole job is being trusted at a glance. `has-session` is a socket
    /// connect, so the cost is a millisecond apiece.
    ///
    /// Local sessions only, and deliberately. Another world's answer costs a
    /// probe of that world before the first question can even be asked — a
    /// login shell, and over SSH a connection — and a board that will not
    /// paint until a laptop has finished talking to a server is worse than a
    /// board that corrects one row a moment later. `visit_remote_holds` asks
    /// the rest, on a thread, and repaints when it knows.
    ///
    /// Sessions in worlds that hold nothing simply have no socket to find,
    /// so they stay `Saved` without needing to be asked about separately.
    fn mark_detached(&self) {
        // The resolved path, not the bare name. The guard used to ask the
        // login-shell PATH and the call then asked the *process* PATH, which
        // is the Finder stub `shell_env` exists to work around: on a Mac
        // whose tmux came from Homebrew, `which` finds it, the spawn fails
        // ENOENT, and every held session comes back Saved — the exact lie
        // this state was added to stop.
        let Some(tmux) = self.env.which("tmux") else { return };
        let tag = self.desk_tag();
        let mut sessions = self.sessions.lock().unwrap();
        for (id, meta) in sessions.iter_mut() {
            if meta.status != Status::Saved || !is_local(&meta.cwd) {
                continue;
            }
            // Both names: a session held across the app's rename is running
            // under the old one, and a board that called it closed would be
            // the very lie this pass exists to stop.
            let held = [pty::hold_socket(&tag, id), pty::hold_socket_former(&tag, id)]
                .into_iter()
                .any(|n| tmux_answers(&tmux, &pty::Socket::Named(n)));
            if held {
                meta.status = Status::Detached;
            }
        }
    }

    /// What this machine takes to hold a local session.
    ///
    /// The socket keeps its `-L` shape: it works, it is tested, and moving it
    /// onto `-S` would strand every session a previous version left held —
    /// still running, under a name nothing looks for any more.
    fn local_hold(&self) -> Option<WorldHold> {
        self.env.which("tmux")?;
        let conf = self.data_dir.join("tmux.conf");
        // Rewritten each start rather than once: an upgrade that changes what
        // tmux is told must not wait for someone to delete a stale file.
        if std::fs::write(&conf, pty::HOLD_CONF).is_err() {
            return None;
        }
        Some(WorldHold {
            conf: conf.to_string_lossy().to_string(),
            socket_dir: None,
        })
    }

    /// Which socket holds this one session, and how tmux is told about it.
    ///
    /// Persistence is a property of a **world**, not a premise of the app —
    /// the same ruling `worlds.md` already made about which machine a card
    /// runs on. A world that answered `tmux -V` gets it; a world that did not
    /// keeps the behaviour it always had, and nothing here has to know which
    /// of the two it is looking at.
    fn hold_plan(&self, he: &HostEnv, session_id: &str) -> Option<HoldPlan> {
        let world = he.hold.as_ref()?;
        let (socket, socket_file) = match &world.socket_dir {
            None => {
                let name = self.local_hold_socket(session_id);
                let file =
                    tmux_socket_dir().map(|d| d.join(&name).to_string_lossy().to_string());
                (pty::Socket::Named(name), file)
            }
            Some(dir) => {
                let path = pty::hold_socket_path(dir, &self.remote_tag(), session_id);
                // The one failure tmux gives no useful sign of: too long an
                // address and the session simply does not start, with an
                // error nobody sees because it went to a pty that closed with
                // it. Refusing to hold is the honest version of the same
                // outcome, and it says so.
                if path.len() >= pty::SOCKET_PATH_LIMIT {
                    eprintln!(
                        "[core] {} bytes is too long for a socket address; \
                         this session runs but will not be held: {path}",
                        path.len()
                    );
                    return None;
                }
                (pty::Socket::Path(path), None)
            }
        };
        Some(HoldPlan {
            socket,
            conf: world.conf.clone(),
            socket_file,
        })
    }

    /// Which local socket holds this session — the new name, unless a server
    /// under the old one is still answering for it.
    ///
    /// A held agent is the one thing the app's rename cannot leave behind. Its
    /// tmux is bound to `agentdesk-…`, and asking `new-session -A` for
    /// `marol-…` would not reattach to it: it would start a second agent in
    /// the same worktree, which is exactly the accident holding sessions was
    /// built to prevent.
    ///
    /// Guarded by the socket file's existence, so this is a `stat` in the
    /// common case and asks tmux only when there is genuinely something there.
    /// Once the last session from before the rename is closed, no file is left
    /// and the question stops being asked at all.
    fn local_hold_socket(&self, session_id: &str) -> String {
        let tag = self.desk_tag();
        let name = pty::hold_socket(&tag, session_id);
        let (Some(dir), Some(tmux)) = (tmux_socket_dir(), self.env.which("tmux")) else {
            return name;
        };
        let former = pty::hold_socket_former(&tag, session_id);
        if dir.join(&former).exists()
            && tmux_answers(&tmux, &pty::Socket::Named(former.clone()))
        {
            eprintln!("[core] reattaching to {former}, held from before the rename");
            return former;
        }
        name
    }

    /// This desk's tag, from where it keeps its data — so two installs on
    /// one machine never collect each other's held sessions.
    fn desk_tag(&self) -> String {
        pty::desk_tag(&self.data_dir.to_string_lossy())
    }

    /// This desk's tag *as another machine sees it*.
    ///
    /// The data directory alone identifies a desk here, because two installs
    /// on one machine cannot share a path. It does not identify one over
    /// there. Two laptops belonging to the same person have the same data
    /// directory, and if they both reach one SSH host they would agree on a
    /// tag — at which point one desk's orphan sweep, whose entire job is
    /// killing sockets no card claims, finds the other's live agents and
    /// kills them. Nothing reports it. The work is simply gone.
    ///
    /// So the remote tag mixes in something only this machine has. Not the
    /// hostname: hostnames get renamed and reassigned, and a tag that moved
    /// would strand every session held under the old one.
    fn remote_tag(&self) -> String {
        pty::desk_tag(&format!(
            "{}\n{}",
            self.data_dir.to_string_lossy(),
            self.machine_id()
        ))
    }

    /// Which remote ports to offer for this host's hook tunnel, best first.
    ///
    /// **The first one is the port this desk used here last time.** A session
    /// the host held through a restart is still running, and the URL it reports
    /// to was written into its plugin config when it started — Claude Code
    /// reads that file once. A fresh port each run is a held agent posting into
    /// nothing for the rest of its life: exactly the bug the local endpoint
    /// already learned to avoid, one hop further out.
    ///
    /// With nothing remembered the first is derived, not drawn: from the host
    /// and from this machine's id. Both halves matter. The host, so one desk's
    /// two servers do not collide; the machine, because the port is bound on
    /// the *remote* side, and two desks reaching one server would otherwise
    /// ask it for the same port and the second would quietly get no tunnel.
    ///
    /// The rest are fallbacks, for the day something else on that host is
    /// already listening where we would like to.
    fn tunnel_ports(&self, host: &str) -> Vec<u16> {
        tunnel_ports(host, &self.machine_id(), self.remembered_tunnel(host))
    }

    /// Where the tunnel ports are kept between runs: one `host<TAB>port` line
    /// each, beside the data this desk already writes down.
    fn tunnels_file(&self) -> std::path::PathBuf {
        self.data_dir.join("tunnels")
    }

    fn remembered_tunnel(&self, host: &str) -> Option<u16> {
        std::fs::read_to_string(self.tunnels_file())
            .ok()?
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .find(|(h, _)| *h == host)
            .and_then(|(_, p)| p.trim().parse().ok())
    }

    fn remember_tunnel(&self, host: &str, port: u16) {
        // Tabs separate the two fields, so a host alias containing one would
        // write a line that reads back as a different host. Aliases come from
        // the person's own ssh config and never contain whitespace, but the
        // file is written rather than trusted: drop the entry instead.
        if host.contains('\t') || host.contains('\n') {
            return;
        }
        let mut out: Vec<String> = std::fs::read_to_string(self.tunnels_file())
            .unwrap_or_default()
            .lines()
            .filter(|l| l.split_once('\t').map(|(h, _)| h != host).unwrap_or(false))
            .map(str::to_string)
            .collect();
        out.push(format!("{host}\t{port}"));
        let _ = std::fs::create_dir_all(&self.data_dir);
        let _ = std::fs::write(self.tunnels_file(), out.join("\n") + "\n");
    }

    /// A random id written once into the data directory, and read for ever
    /// after. Its lifetime is exactly right: lose the data directory and the
    /// sessions it named are gone too, so there is nothing left to strand.
    ///
    /// Held in memory as well as on disk. A disk that will not take the write
    /// would otherwise hand out a fresh id per call, and every socket named
    /// this run would be unfindable by the next call in the same run.
    fn machine_id(&self) -> String {
        self.machine_id
            .get_or_init(|| {
                let f = self.data_dir.join("machine-id");
                if let Ok(s) = std::fs::read_to_string(&f) {
                    let s = s.trim().to_string();
                    if !s.is_empty() {
                        return s;
                    }
                }
                let id = uuid::Uuid::new_v4().to_string();
                let _ = std::fs::write(&f, &id);
                id
            })
            .clone()
    }

    /// Kill held sessions this desk no longer has a card for.
    ///
    /// Asking tmux to outlive the app is a promise to come back for what was
    /// left running. A session removed from the list, or a crash between the
    /// spawn and the write, leaves an agent nobody will look at again and
    /// nothing else can name — so the sweep is not tidiness, it is the other
    /// half of the feature.
    ///
    /// Sockets are read off disk because that is the only place an id this
    /// desk has forgotten still exists. Scoped by the desk tag, so a sweep
    /// can only ever reach this install's own leftovers.
    fn sweep_held_orphans(&self) {
        let Some(tmux) = self.env.which("tmux") else { return };
        let Some(dir) = tmux_socket_dir() else { return };
        // Both prefixes, and every card claims both of its possible names: a
        // socket left over from before the rename is still this desk's to
        // sweep, and a card still holding one is still this desk's to spare.
        let prefixes = pty::hold_prefixes(&self.desk_tag());
        let known: std::collections::HashSet<String> = self
            .sessions
            .lock()
            .unwrap()
            .keys()
            .flat_map(|id| {
                [
                    pty::hold_socket(&self.desk_tag(), id),
                    pty::hold_socket_former(&self.desk_tag(), id),
                ]
            })
            .collect();
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !prefixes.iter().any(|p| name.starts_with(p)) || known.contains(&name) {
                continue;
            }
            let sock = pty::Socket::Named(name.clone());
            if tmux_answers(&tmux, &sock) {
                let (_, args) = pty::hold_destroy(&sock);
                let _ = std::process::Command::new(&tmux).args(&args).output();
            }
            // tmux leaves the socket inode behind when a server exits, so a
            // dead socket looks exactly like a live one from here. Unlinking
            // is what stops the directory — and this sweep's work — growing
            // without bound across restarts.
            //
            // Asked again rather than assumed: unlinking the socket of a
            // server that is still answering takes away the only name that
            // agent has, and nothing could ever attach to it again. A socket
            // is only rubbish once nothing replies on it.
            if tmux_answers(&tmux, &sock) {
                eprintln!("[core] {name} still answers after kill-server; left alone");
                continue;
            }
            let _ = std::fs::remove_file(e.path());
            eprintln!("[core] swept a held session with no card left: {name}");
        }
    }

    /// The same two startup jobs — what survived, and what should not have —
    /// for every world that is not this machine.
    ///
    /// Both need the same thing first: the world, probed. That is a login
    /// shell and, over SSH, a connection, which is why this is a thread and
    /// not part of the first paint. It reads a row as `Detached` a moment
    /// after the board appears rather than holding the board until a server
    /// answers.
    ///
    /// **Only worlds this desk currently has a session in.** Reaching an SSH
    /// host opens a connection to it, and opening one nobody asked for is not
    /// tidiness — it is a machine on someone's network waking up because an
    /// app felt thorough. The cost is that a world whose every card was
    /// deleted keeps its sockets until somebody opens a card there again;
    /// they are a handful of files in a directory of ours, and that is the
    /// cheaper mistake.
    ///
    /// **A world that does not answer is left entirely alone.** Not reachable
    /// is not the same as not there: a laptop off the VPN would otherwise
    /// decide every agent on the work server had vanished.
    fn visit_remote_holds(&self) {
        let mut worlds: Vec<Host> = Vec::new();
        for meta in self.sessions.lock().unwrap().values() {
            if let Ok(loc) = host::locate(&meta.cwd) {
                if !matches!(loc.host, Host::Local) && !worlds.contains(&loc.host) {
                    worlds.push(loc.host);
                }
            }
        }
        for h in worlds {
            let he = match self.host_env(&h) {
                Ok(he) => he,
                // Unreachable, or nothing there to talk to. Say so and change
                // nothing: every card in this world keeps the state it had.
                Err(e) => {
                    eprintln!("[core] cannot reach {h:?} to ask about held sessions: {e:#}");
                    continue;
                }
            };
            let Some(dir) = he.hold.as_ref().and_then(|w| w.socket_dir.clone()) else {
                continue;
            };
            let hr = he.hr(&self.env);
            let tag = self.remote_tag();

            // What survived. Only rows the last run left as `Saved`: anything
            // else is either live in this process or already known.
            let saved: Vec<String> = self
                .sessions
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, m)| {
                    m.status == Status::Saved
                        && host::locate(&m.cwd).map(|l| l.host == h).unwrap_or(false)
                })
                .map(|(id, _)| id.clone())
                .collect();
            let mut found = false;
            for id in &saved {
                let sock =
                    pty::Socket::Path(pty::hold_socket_path(&dir, &tag, id));
                if !hold_answers(&hr, &sock) {
                    continue;
                }
                if let Some(m) = self.sessions.lock().unwrap().get_mut(id) {
                    m.status = Status::Detached;
                    found = true;
                }
            }
            if found {
                self.broadcast();
            }

            // And what should not have. Read off the world's own directory,
            // because a session this desk has forgotten exists nowhere else.
            let known: std::collections::HashSet<String> = self
                .sessions
                .lock()
                .unwrap()
                .keys()
                .map(|id| pty::hold_socket_name(&tag, id))
                .collect();
            let prefix = pty::hold_prefix(&tag, true);
            for name in hr.list_dir(&dir) {
                if !name.starts_with(&prefix) || known.contains(&name) {
                    continue;
                }
                // Kill and unlink in one command, which is also the one the
                // close button fires: over there this desk gets no second
                // visit, and a socket left behind is indistinguishable from a
                // live one on the next sweep.
                let sock = pty::Socket::Path(format!("{dir}/{name}"));
                let (p, a) = pty::hold_destroy(&sock);
                let a: Vec<&str> = a.iter().map(String::as_str).collect();
                match hr.run(&p, &a, None) {
                    Ok(_) => eprintln!("[core] swept a held session with no card left: {name}"),
                    Err(e) => eprintln!("[core] could not sweep {name}: {e:#}"),
                }
            }
        }
    }

    /// Where the opening-prompt template lives, written out on first use.
    ///
    /// The start-attempt dialog has always shown the composed prompt and let
    /// it be edited for that one attempt; this is the same text one level up,
    /// where editing it changes every attempt after. Surfacing the path is
    /// what turns "the app adds something to your session" from a fact buried
    /// in the README into a file the settings can open.
    pub fn prompt_template_path(&self) -> String {
        crate::prompt::template_path(&self.data_dir)
            .to_string_lossy()
            .to_string()
    }

    /// One measured CLI's version, as read at startup.
    pub fn cli_version(&self, agent: &str) -> Option<String> {
        Cli::of(agent)
            .and_then(|cli| self.versions.of(cli))
            .map(|(a, b, c)| format!("{a}.{b}.{c}"))
    }

    /// Whether a measured CLI is new enough for this app to wire its status
    /// hooks up. False also means "not installed", which is the same answer
    /// as far as the panel that reports it is concerned: no status here.
    pub fn reports_status(&self, agent: &str) -> bool {
        Cli::of(agent).is_some_and(|cli| {
            self.versions.of(cli).is_some() && cli.hooks_ok(self.versions.of(cli))
        })
    }

    /// Whether the installed claude supports session names and, with them,
    /// cross-session messaging between this desk's sessions.
    pub fn named_sessions(&self) -> bool {
        self.versions.claude >= Some(NAMED_SESSIONS_SINCE)
    }

    /* ------------------------- notifications ----------------------- */

    pub fn notify_prefs(&self) -> NotifyPrefs {
        *self.notify_prefs.lock().unwrap()
    }

    pub fn set_notify_prefs(&self, prefs: NotifyPrefs) -> Result<()> {
        *self.notify_prefs.lock().unwrap() = prefs;
        self.store
            .set_setting(NOTIFY_PREFS_KEY, &serde_json::to_string(&prefs)?)
    }

    /// A notification fired on request, so the panel's toggles can be
    /// checked against the OS without waiting for an agent to block.
    ///
    /// `force`, because the person pressing the button is by definition
    /// focused on the window — the focus gate would swallow exactly the
    /// notification being tested.
    pub fn test_notification(&self) {
        let locale = self.locale.get();
        self.sink.emit(
            "notify",
            serde_json::json!({
                "title": crate::i18n::test_title(locale),
                "body": crate::i18n::test_body(locale),
                "force": true,
            }),
        );
    }

    /* ---------------------------- parked --------------------------- */

    /// Park: keep the work and the conversation, give back the ground.
    ///
    /// The branch and the checkpoint refs stay; the worktree, every session
    /// living in it — the attempt shell included — and the concurrency slot
    /// are returned. What is uncommitted rides a pre-park checkpoint across
    /// (a failure to keep it aborts the park: losing work silently is the
    /// one failure this feature must not have). Refused mid-turn, for
    /// exactly restore's reason. Returns the branch name, for the clipboard.
    pub fn park_attempt(&self, attempt_id: &str) -> Result<String> {
        let attempt = self
            .store
            .get_attempt(attempt_id)?
            .ok_or_else(|| anyhow!("no such attempt: {attempt_id}"))?;
        if attempt.outcome.is_some() {
            return Err(anyhow!("this attempt is finished"));
        }
        if attempt.parked_at.is_some() {
            return Err(anyhow!("this attempt is already parked"));
        }
        let busy = self.sessions.lock().unwrap().values().any(|s| {
            s.attempt_id.as_deref() == Some(attempt_id)
                && s.live
                && !matches!(s.status, Status::Idle | Status::Saved | Status::Exited)
        });
        if busy {
            return Err(anyhow!(
                "the agent is mid-turn in this worktree. Parking now would pull the ground out \
                 from under its edits. Wait for the turn to end — or close the session — and \
                 park then"
            ));
        }

        // The shelf: whatever is uncommitted goes into a checkpoint the
        // worktree's removal cannot take with it. Every checkout, and the
        // failure of any one aborts the park — losing work silently is the
        // one failure this feature must not have.
        self.snapshot_attempt(attempt_id)?;

        let trees = self.trees(&attempt)?;

        // The sessions living in the directory go with it — the attempt's
        // own, the shell, a dev server — same rule as finishing.
        let doomed: Vec<String> = self
            .sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| {
                s.attempt_id.as_deref() == Some(attempt_id)
                    || s.cwd == attempt.worktree_path
                    || s.cwd
                        .starts_with(&format!("{}/", attempt.worktree_path.trim_end_matches('/')))
            })
            .map(|s| s.id.clone())
            .collect();
        for id in doomed {
            self.ptys.kill(&id);
            let _ = self.store.archive_session(&id);
            self.sessions.lock().unwrap().remove(&id);
        }
        self.shells.lock().unwrap().remove(attempt_id);

        for tree in &trees {
            let (repo_loc, he) = self.located(&tree.repo_path)?;
            let wt_loc = host::locate(&tree.worktree_path)?;
            self.worktrees
                .remove(&he.hr(&self.env), &repo_loc.path, &wt_loc.path)?;
        }
        // The workspace directory itself stays, empty, for exactly as long as
        // the attempt is parked. `--continue` finds its conversation by cwd,
        // so the resume needs this path back — and a directory nothing else
        // can claim in the meantime is the cheapest way to keep it.
        self.store.set_parked(attempt_id, Some(now_ms() as i64))?;

        self.emit_tasks();
        self.broadcast();
        // The whole point: the slot is free now, and the queue should know.
        self.drain_queue();
        Ok(attempt.branch)
    }

    /// Resume: grow the ground back and walk the old road.
    ///
    /// The worktree reattaches to the attempt's branch at its recorded path
    /// — `--continue` finds the conversation by cwd, so the path is not
    /// negotiable — then the shelf checkpoint restores the exact content
    /// that was parked, and the existing reopen flow takes it from there.
    /// Attach succeeding *is* the resume; a restore failure afterwards is
    /// reported and retryable, never rolled back into fake cleanliness.
    pub fn resume_attempt(&self, attempt_id: &str, cols: u16, rows: u16) -> Result<Resumed> {
        let attempt = self
            .store
            .get_attempt(attempt_id)?
            .ok_or_else(|| anyhow!("no such attempt: {attempt_id}"))?;
        if attempt.outcome.is_some() {
            return Err(anyhow!("this attempt is finished and cannot be resumed"));
        }
        if attempt.parked_at.is_none() {
            return Err(anyhow!("this attempt is not parked"));
        }
        let trees = self.trees(&attempt)?;

        // Every checkout back at its own recorded path. A failure part-way
        // leaves the ones already attached standing and stops: half a
        // workspace is visible and finishable, where an unwind would have
        // taken back ground the person can see.
        for tree in &trees {
            let (repo_loc, he) = self.located(&tree.repo_path)?;
            let wt_path = host::locate(&tree.worktree_path)?.path;
            self.worktrees
                .attach(&he.hr(&self.env), &repo_loc.path, &wt_path, &tree.branch)?;
        }
        self.store.set_parked(attempt_id, None)?;

        // The shelf comes down before the agent looks: the branch tip may
        // be behind what was parked, and skipping this would lose work
        // quietly. Restore before any terminal exists — no one to race.
        let restore_error = (|| -> Result<()> {
            for tree in &trees {
                let (repo_loc, he) = self.located(&tree.repo_path)?;
                let hr = he.hr(&self.env);
                let wt_path = host::locate(&tree.worktree_path)?.path;
                let cps = self.worktrees.checkpoints(&hr, &repo_loc.path, attempt_id)?;
                if let Some(cp) = cps.last() {
                    self.worktrees.restore_checkpoint(&hr, &wt_path, &cp.sha)?;
                }
            }
            Ok(())
        })()
        .err()
        .map(|e| format!("{e:#}"));

        let session_id = self.reopen_attempt(attempt_id, cols, rows)?;
        self.emit_tasks();
        Ok(Resumed {
            session_id,
            restore_error,
        })
    }

    /* --------------------------- updates --------------------------- */

    /// Whether this desk may ask GitHub what the newest release is.
    ///
    /// On by default and off by one switch. It is not telemetry — nothing
    /// about this machine is sent, and the request is the same one a browser
    /// makes opening the releases page — but it is the only outbound request
    /// the app makes on its own behalf, and a product that says it phones
    /// nobody should be able to prove it by being told not to.
    pub fn update_enabled(&self) -> bool {
        self.store
            .setting(crate::update::ENABLED_KEY)
            .ok()
            .flatten()
            .map(|v| v != "0")
            .unwrap_or(true)
    }

    pub fn set_update_enabled(&self, on: bool) -> Result<()> {
        self.store
            .set_setting(crate::update::ENABLED_KEY, if on { "1" } else { "0" })
    }

    /// When the last check happened, so ten launches in a day are one
    /// request. Kept in the database rather than in memory because the thing
    /// it rate-limits is *starting the app*, which memory does not survive.
    pub fn update_last_check(&self) -> Option<u64> {
        self.store
            .setting(crate::update::LAST_CHECK_KEY)
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
    }

    pub fn mark_update_checked(&self, at: u64) -> Result<()> {
        self.store
            .set_setting(crate::update::LAST_CHECK_KEY, &at.to_string())
    }

    /// Whether enough time has passed to ask again.
    pub fn update_check_due(&self, now: u64) -> bool {
        match self.update_last_check() {
            None => true,
            Some(last) => now.saturating_sub(last) >= crate::update::CHECK_INTERVAL_SECS,
        }
    }

    /// Copy the database aside so the version being installed can be walked
    /// back out of. See `update::snapshot_db` for why this is not optional.
    pub fn snapshot_db_before(&self, leaving: &str) -> Result<std::path::PathBuf> {
        crate::update::snapshot_db(&self.store, &self.db_path, leaving)
    }

    /* ------------------------- checkpoints ------------------------- */

    pub fn checkpoints_enabled(&self) -> bool {
        *self.checkpoints_on.lock().unwrap()
    }

    pub fn set_checkpoints_enabled(&self, on: bool) -> Result<()> {
        *self.checkpoints_on.lock().unwrap() = on;
        self.store
            .set_setting(CHECKPOINTS_KEY, if on { "1" } else { "0" })
    }

    /// An attempt's checkpoints, oldest first — read straight off the refs.
    ///
    /// One list for the whole attempt. A checkpoint is a moment in the work,
    /// and a card spanning two repositories has one timeline, not two: the
    /// ordinals are shared by construction (see `snapshot_attempt_inner`), so
    /// this merges the checkouts' refs by number and reports each moment
    /// once, at the latest time any checkout wrote it down.
    pub fn list_checkpoints(&self, attempt_id: &str) -> Result<Vec<crate::worktree::Checkpoint>> {
        let attempt = self
            .store
            .get_attempt(attempt_id)?
            .ok_or_else(|| anyhow!("no such attempt: {attempt_id}"))?;
        if attempt.outcome.is_some() {
            // Finished: the refs are gone by design, the frozen diff remains.
            return Ok(Vec::new());
        }
        let mut merged: Vec<crate::worktree::Checkpoint> = Vec::new();
        for cp in self.checkpoints_per_tree(&attempt)?.into_iter().flatten() {
            match merged.iter_mut().find(|m| m.n == cp.n) {
                Some(seen) => seen.at = seen.at.max(cp.at),
                None => merged.push(cp),
            }
        }
        merged.sort_by_key(|c| c.n);
        Ok(merged)
    }

    /// Each checkout's own checkpoint refs, in checkout order.
    ///
    /// A parked attempt has no worktrees, but the refs live in each
    /// repository's shared git dir — so they are read from the main checkouts
    /// and the answer is the same either way.
    fn checkpoints_per_tree(
        &self,
        attempt: &StoredAttempt,
    ) -> Result<Vec<Vec<crate::worktree::Checkpoint>>> {
        let parked = attempt.parked_at.is_some();
        self.trees(attempt)?
            .into_iter()
            .map(|tree| {
                let where_ = if parked {
                    &tree.repo_path
                } else {
                    &tree.worktree_path
                };
                let (loc, he) = self.located(where_)?;
                self.worktrees
                    .checkpoints(&he.hr(&self.env), &loc.path, &attempt.id)
            })
            .collect()
    }

    /// The manual checkpoint — any agent, any moment a human chooses.
    /// `None` means nothing changed since the last one (or one was already
    /// in flight, which amounts to the same snapshot).
    pub fn checkpoint_now(&self, attempt_id: &str) -> Result<Option<crate::worktree::Checkpoint>> {
        self.snapshot_attempt(attempt_id)
    }

    /// The Stop-hook path: leave immediately, snapshot on a thread of its
    /// own. `on_hook` is a path an agent is waiting on, and this is real
    /// git work.
    pub(crate) fn snapshot_after_turn(self: &Arc<Self>, session_id: &str) {
        if !self.checkpoints_enabled() {
            return;
        }
        let attempt_id = self
            .sessions
            .lock()
            .unwrap()
            .get(session_id)
            .and_then(|s| s.attempt_id.clone());
        let Some(attempt_id) = attempt_id else { return };
        let core = Arc::clone(self);
        std::thread::spawn(move || match core.snapshot_attempt(&attempt_id) {
            Ok(_) => {}
            Err(e) => eprintln!("[core] checkpoint for {attempt_id} failed: {e:#}"),
        });
    }

    /// The other thing a turn's end settles: the token account. Same seam
    /// as the snapshot, same shape — leave the hook path now, read on a
    /// thread of its own. Sessions with no recorded transcript (any agent
    /// that is not claude, or a claude too old to say) simply never appear
    /// in the books: honest absence, not a zero.
    pub(crate) fn usage_after_turn(self: &Arc<Self>, session_id: &str) {
        let (cwd, transcript, ledger) = {
            let sessions = self.sessions.lock().unwrap();
            let Some(s) = sessions.get(session_id) else { return };
            let Some(tp) = s.transcript_path.clone() else { return };
            // A session with a transcript is a session whose CLI is in the
            // table — the path came out of that CLI's own hook payload.
            let Some(cli) = Cli::of(&s.agent) else { return };
            (s.cwd.clone(), tp, cli.ledger())
        };
        let core = Arc::clone(self);
        let sid = session_id.to_string();
        std::thread::spawn(move || {
            if let Err(e) = core.read_usage(&sid, &cwd, &transcript, ledger) {
                eprintln!("[core] usage read for {sid} failed: {e:#}");
            }
        });
    }

    /// Read what the transcript has grown since last time and fold it into
    /// the session's account. The offset only ever advances to a line
    /// boundary — a half-written line is the next read's problem.
    ///
    /// How the fold works is the ledger's business, and getting it wrong is
    /// invisible rather than loud: a running total added to itself once per
    /// turn still looks like a number.
    fn read_usage(
        &self,
        session_id: &str,
        cwd: &str,
        transcript: &str,
        ledger: Ledger,
    ) -> Result<()> {
        let (_, he) = self.located(cwd)?;
        let hr = he.hr(&self.env);
        let from = self
            .usage_state
            .lock()
            .unwrap()
            .get(session_id)
            .map(|u| u.offset)
            .unwrap_or(0);
        let Some(bytes) = hr.read_from(transcript, from)? else {
            return Ok(());
        };
        let consumed = match bytes.iter().rposition(|b| *b == b'\n') {
            Some(i) => i + 1,
            None => return Ok(()),
        };
        let spend = parse_usage(ledger, &String::from_utf8_lossy(&bytes[..consumed]));
        let usage = {
            let mut states = self.usage_state.lock().unwrap();
            let st = states.entry(session_id.to_string()).or_default();
            st.offset = from + consumed as u64;
            match (ledger, spend.account) {
                (Ledger::PerMessage, Some(d)) => {
                    st.acc.input += d.input;
                    st.acc.output += d.output;
                    st.acc.cache_read += d.cache_read;
                    st.acc.cache_write += d.cache_write;
                }
                // The row is already the whole session's total; the previous
                // total is superseded, not added to.
                (Ledger::Cumulative, Some(total)) => {
                    let context = st.acc.context;
                    st.acc = total;
                    st.acc.context = context;
                }
                (_, None) => {}
            }
            if let Some(ctx) = spend.context {
                st.acc.context = ctx;
            }
            st.acc
        };
        if let Some(s) = self.sessions.lock().unwrap().get_mut(session_id) {
            s.usage = Some(usage);
        }
        self.broadcast();
        Ok(())
    }

    fn snapshot_attempt(&self, attempt_id: &str) -> Result<Option<crate::worktree::Checkpoint>> {
        if !self.claim_checkpointing(attempt_id) {
            return Ok(None);
        }
        let result = self.snapshot_attempt_inner(attempt_id);
        self.checkpointing.lock().unwrap().remove(attempt_id);
        result
    }

    fn claim_checkpointing(&self, attempt_id: &str) -> bool {
        self.checkpointing
            .lock()
            .unwrap()
            .insert(attempt_id.to_string())
    }

    /// The snapshot itself — call only while holding the attempt's
    /// `checkpointing` claim.
    fn snapshot_attempt_inner(
        &self,
        attempt_id: &str,
    ) -> Result<Option<crate::worktree::Checkpoint>> {
        let attempt = self
            .store
            .get_attempt(attempt_id)?
            .ok_or_else(|| anyhow!("no such attempt: {attempt_id}"))?;
        // Finished has nothing left to snapshot; parked already holds its
        // shelf checkpoint and has no worktree to read.
        if attempt.outcome.is_some() || attempt.parked_at.is_some() {
            return Ok(None);
        }
        let trees = self.trees(&attempt)?;

        // One number for the moment, across every checkout — the highest any
        // of them has reached, plus one. Numbering each repository on its own
        // would make "checkpoint 3" mean a different instant in each, and the
        // restore that walked back to it would reassemble a workspace that
        // never existed.
        let n = self
            .checkpoints_per_tree(&attempt)?
            .iter()
            .filter_map(|cps| cps.last().map(|c| c.n))
            .max()
            .unwrap_or(0)
            + 1;

        // A checkout that changed nothing this turn grows no ref, which is
        // the honest record: there was nothing to snapshot there. The moment
        // still counts as taken if any of them wrote one down.
        let mut taken: Option<crate::worktree::Checkpoint> = None;
        for tree in &trees {
            let (loc, he) = self.located(&tree.worktree_path)?;
            let cp = self.worktrees.checkpoint(
                &he.hr(&self.env),
                &loc.path,
                attempt_id,
                &tree.base_sha,
                n,
            )?;
            if let Some(cp) = cp {
                taken = Some(match taken {
                    Some(seen) => crate::worktree::Checkpoint {
                        at: seen.at.max(cp.at),
                        ..seen
                    },
                    None => cp,
                });
            }
        }
        if taken.is_some() {
            self.sink.emit(
                "checkpoints:changed",
                serde_json::json!({ "attemptId": attempt_id, "n": n }),
            );
        }
        Ok(taken)
    }

    /// Restore an attempt's worktree to checkpoint `n` — `0` is the
    /// attempt's base. Code only, the conversation is never touched, and the
    /// restore is itself restorable: a "now" snapshot is taken first.
    ///
    /// Refused while a turn is in flight. Restoring under a running agent
    /// would pull files out from under its edits, and it would go on
    /// believing in work that is no longer there — the decoupling the
    /// decision document rules out. Stopped, idle and exited sessions are
    /// the moments a person can honestly rewind.
    pub fn restore_checkpoint(&self, attempt_id: &str, n: u64) -> Result<Restored> {
        let attempt = self
            .store
            .get_attempt(attempt_id)?
            .ok_or_else(|| anyhow!("no such attempt: {attempt_id}"))?;
        if attempt.outcome.is_some() {
            return Err(anyhow!(
                "this attempt is finished; its worktree is gone, so there is nothing to restore into"
            ));
        }
        if attempt.parked_at.is_some() {
            return Err(anyhow!(
                "this attempt is parked; resume it first, then restore"
            ));
        }
        let busy = self.sessions.lock().unwrap().values().any(|s| {
            s.attempt_id.as_deref() == Some(attempt_id)
                && s.live
                && !matches!(s.status, Status::Idle | Status::Saved | Status::Exited)
        });
        if busy {
            return Err(anyhow!(
                "the agent is mid-turn in this worktree. Restoring now would pull files out from \
                 under its edits, and it would keep believing in work that is no longer there. \
                 Wait for the turn to end — or close the session — and restore then"
            ));
        }
        // One claim covers the pre-save and the restore: a Stop-triggered
        // snapshot arriving mid-restore must not capture a half-restored
        // tree as a checkpoint.
        if !self.claim_checkpointing(attempt_id) {
            return Err(anyhow!(
                "a checkpoint is being taken right now; try again in a moment"
            ));
        }
        let result = (|| {
            // The retreat from the retreat, kept before anything moves.
            let saved = self.snapshot_attempt_inner(attempt_id)?;
            let trees = self.trees(&attempt)?;
            if n > 0 && !self.list_checkpoints(attempt_id)?.iter().any(|c| c.n == n) {
                return Err(anyhow!("this attempt has no checkpoint #{n}"));
            }
            let mut shas = Vec::with_capacity(trees.len());
            for tree in &trees {
                let (loc, he) = self.located(&tree.worktree_path)?;
                let hr = he.hr(&self.env);
                let to_sha = if n == 0 {
                    tree.base_sha.clone()
                } else {
                    // At-or-before: a checkout untouched at that moment grew
                    // no ref for it, and the state it was in then is the one
                    // its newest earlier snapshot holds. No snapshot at all
                    // means it had never changed — its base is the answer.
                    let cps = self.worktrees.checkpoints(&hr, &loc.path, attempt_id)?;
                    crate::worktree::at_or_before(&cps, n)
                        .map(|c| c.sha.clone())
                        .unwrap_or_else(|| tree.base_sha.clone())
                };
                self.worktrees.restore_checkpoint(&hr, &loc.path, &to_sha)?;
                shas.push(if tree.dir.is_empty() {
                    to_sha
                } else {
                    format!("{}: {to_sha}", tree.dir)
                });
            }
            self.sink.emit(
                "checkpoints:changed",
                serde_json::json!({ "attemptId": attempt_id }),
            );
            Ok(Restored {
                to_n: n,
                to_sha: shas.join("\n"),
                saved,
            })
        })();
        self.checkpointing.lock().unwrap().remove(attempt_id);
        result
    }

    /// Startup sweep: checkpoint refs belong to open attempts; anything else
    /// is a leftover from a run that ended without its cleanup. Local repos
    /// only — reaching a WSL or SSH repo would cost a host probe at startup,
    /// and their strays go when any of their attempts next closes.
    fn sweep_checkpoint_orphans(&self) {
        let live: std::collections::HashSet<String> = match self.store.open_attempts() {
            Ok(list) => list.into_iter().map(|a| a.id).collect(),
            Err(e) => {
                eprintln!("[core] checkpoint sweep skipped: {e:#}");
                return;
            }
        };
        let repos: std::collections::HashSet<String> = match self.store.list_tasks() {
            Ok(tasks) => tasks
                .iter()
                .flat_map(|t| t.repos())
                .map(|r| r.repo_path)
                .collect(),
            Err(e) => {
                eprintln!("[core] checkpoint sweep skipped: {e:#}");
                return;
            }
        };
        for repo in repos {
            let Ok(loc) = host::locate(&repo) else { continue };
            if loc.host != Host::Local {
                continue;
            }
            let hr = HostRef {
                host: &Host::Local,
                local: &self.env,
                env: &self.env,
            };
            if !hr.is_dir(&loc.path) {
                continue;
            }
            match self.worktrees.sweep_checkpoints(&hr, &loc.path, &live) {
                Ok(0) => {}
                Ok(n) => eprintln!("[core] swept {n} orphan checkpoint refs in {repo}"),
                Err(e) => eprintln!("[core] checkpoint sweep in {repo} failed: {e:#}"),
            }
        }
    }

    /* --------------------------- helpers --------------------------- */

    fn persist(&self, meta: &SessionMeta) {
        if let Err(e) = self.store.upsert_session(&meta.to_stored()) {
            eprintln!("[core] persisting session {} failed: {e}", meta.id);
        }
    }

    /// A default name no session on the list already has.
    ///
    /// A directory's name is the only thing there is to call a session opened
    /// without a card, and opening several terminals in one checkout is the
    /// ordinary thing to do here — so the list filled up with rows that all
    /// said the same word and could only be told apart by hovering for the
    /// path. The same string is what `--name` hands Claude Code, so the
    /// sessions were also indistinguishable to *each other*.
    ///
    /// A counter is not a name and is not pretending to be one. It is what
    /// the list says before anyone has said anything better, and renaming —
    /// by hand or by the agent itself — is the rest of the answer.
    fn unique_title(&self, base: &str) -> String {
        let taken: std::collections::HashSet<String> = self
            .sessions
            .lock()
            .unwrap()
            .values()
            .map(|s| s.title.clone())
            .collect();
        if !taken.contains(base) {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base} {n}"))
            .find(|candidate| !taken.contains(candidate))
            .unwrap_or_else(|| base.to_string())
    }

    fn broadcast(&self) {
        let list = self.sessions();
        let waiting = list.iter().filter(|s| s.status.needs_you()).count();
        if let Ok(v) = serde_json::to_value(&list) {
            self.sink.emit("sessions:changed", v);
        }
        self.sink
            .emit("badge", serde_json::json!({ "count": waiting }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StoredTab;

    /// Walking up out of a POSIX tree, and stopping at the top rather than
    /// offering a `..` that goes in circles.
    #[test]
    fn the_way_up_ends_at_the_root() {
        assert_eq!(parent_of("/home/you/project").as_deref(), Some("/home/you"));
        assert_eq!(parent_of("/home").as_deref(), Some("/"));
        assert_eq!(parent_of("/"), None, "the root has nowhere above it");
        assert_eq!(
            parent_of("/home/you/").as_deref(),
            Some("/home"),
            "a trailing slash is not an extra level"
        );
    }

    /// The same, on the one platform whose root is not `/`. A drive keeps its
    /// separator: `C:` on its own means "wherever that drive last was" to
    /// Windows, which is not a place a picker can stand.
    #[test]
    fn a_drive_is_a_root_too() {
        assert_eq!(
            parent_of(r"C:\Users\you\project").as_deref(),
            Some(r"C:\Users\you")
        );
        assert_eq!(parent_of(r"C:\Users").as_deref(), Some(r"C:\"));
        assert_eq!(parent_of(r"C:\"), None);
        assert_eq!(parent_of("C:"), None);
    }

    /// A WSL path is POSIX even when this process is Windows. Deciding the
    /// separator from the *path* rather than from `cfg!(windows)` is what
    /// keeps `wsl://Ubuntu/home/you` walking up correctly from a Windows
    /// desk — the same trap `worktree.rs` documents about `PathBuf::join`.
    #[test]
    fn a_posix_path_stays_posix_wherever_it_is_read() {
        assert_eq!(parent_of("/home/you").as_deref(), Some("/home"));
        assert_eq!(
            parent_of("/mnt/c/Users").as_deref(),
            Some("/mnt/c"),
            "a path that mentions drives is still POSIX if it starts at /"
        );
    }

    // The listing itself needs a whole Core, so those tests live where the
    // harness that builds one does: `tests/attempts.rs`.

    /// CI 守門的第二半:找到的 CLI 要真的答得出 `--version`,而且版本
    /// 字串解析得出來 ——「偵測到」不是檔案存在,是問得到話。跟著
    /// shell_env 的守門測試一起,由 MAROL_EXPECT_CLAUDE / MAROL_EXPECT_CODEX
    /// 啟用,一支一個開關:一台只裝了其中一支的 runner 該報那一支的實話。
    #[test]
    fn a_promised_real_cli_answers_the_version_probe() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let env = rt.block_on(crate::shell_env::resolve());
        let mut ran = false;
        for (agent, gate) in [("claude", "MAROL_EXPECT_CLAUDE"), ("codex", "MAROL_EXPECT_CODEX")] {
            if std::env::var(gate).as_deref() != Ok("1") {
                eprintln!("skip {agent}: {gate} != 1");
                continue;
            }
            ran = true;
            let v = rt.block_on(probe_version(&env, agent));
            assert!(
                v.is_some(),
                "{agent} --version did not run or did not parse"
            );
            let (a, b, c) = v.unwrap();
            eprintln!("{agent} {a}.{b}.{c} answered the probe");

            // Version-gated features are decided from this number, so a
            // probe that parses but comes back too old is worth naming here
            // rather than discovering as a card that never shows status.
            let cli = Cli::of(agent).expect("a measured CLI");
            if !cli.hooks_ok(v) {
                panic!(
                    "{agent} {a}.{b}.{c} predates the hook wiring this app uses ({:?}); \
                     sessions would run without status",
                    cli.hooks_since()
                );
            }
        }
        if !ran {
            eprintln!("skip: no MAROL_EXPECT_* gate is set");
        }
    }

    /// The port an SSH host's hook tunnel lands on, and the three things that
    /// have to be true of it.
    ///
    /// It must not move between runs: the agent the host held through the
    /// restart is still posting to the URL baked into its plugin config, and a
    /// new port every start is a session that runs on while the desk goes
    /// blind to it. It must differ per machine, because the port is bound on
    /// the *remote* side and two laptops reaching one server would otherwise
    /// ask it for the same one — the second silently getting no tunnel. And
    /// what worked last time has to win over what would be derived today, or
    /// A script written against the old variable names still gets its values.
    ///
    /// `$MAROL_ROOT_PATH` and `$MAROL_PORT` are not internal plumbing: they
    /// are read by shell lines the *person* wrote, in a config file inside
    /// their own repository. This app deciding to change its name is not a
    /// reason for `cp "$AGENTDESK_ROOT_PATH/.env" .env` to start copying from
    /// nowhere — and a setup script that silently stopped working would look
    /// exactly like a worktree that is mysteriously broken.
    #[test]
    fn scripts_written_against_the_old_variable_names_still_get_their_values() {
        let vars = under_both_names(vec![
            ("MAROL_PORT".to_string(), "5173".to_string()),
            ("MAROL_ROOT_PATH".to_string(), "/repo".to_string()),
        ]);
        let get = |k: &str| {
            vars.iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("{k} is not set: {vars:?}"))
        };
        // Both names, the same value: the two name one thing, so a repository
        // half-brought-forward reads the same either way.
        assert_eq!(get("MAROL_PORT"), "5173");
        assert_eq!(get("AGENTDESK_PORT"), "5173");
        assert_eq!(get("MAROL_ROOT_PATH"), "/repo");
        assert_eq!(get("AGENTDESK_ROOT_PATH"), "/repo");

        // The new name is set last, so it wins wherever a later entry does —
        // which is how every one of these lists is applied.
        let names: Vec<&str> = vars.iter().map(|(k, _)| k.as_str()).collect();
        let pos = |k: &str| names.iter().position(|n| *n == k).unwrap();
        assert!(pos("MAROL_PORT") > pos("AGENTDESK_PORT"), "{names:?}");

        // Nothing else is doubled. This is a compatibility shim for our own
        // prefix, not a rule about environments in general.
        let plain = under_both_names(vec![("PATH".to_string(), "/bin".to_string())]);
        assert_eq!(plain.len(), 1, "{plain:?}");
    }

    /// remembering it was pointless.
    #[test]
    fn a_hosts_tunnel_port_holds_still_across_runs_but_not_across_machines() {
        let a = tunnel_ports("dev-box", "machine-a", None);
        assert_eq!(a, tunnel_ports("dev-box", "machine-a", None), "it moved");
        assert_ne!(
            a[0],
            tunnel_ports("dev-box", "machine-b", None)[0],
            "two desks would fight over one port on the same server",
        );
        assert_ne!(
            a[0],
            tunnel_ports("other-box", "machine-a", None)[0],
            "one desk's two servers would be told the same port",
        );
        // Inside the range ssh can bind unprivileged, and all distinct: a
        // fallback that repeats the candidate that just failed is not one.
        for p in &a {
            assert!((20000..60000).contains(p), "{p}");
        }
        let uniq: std::collections::HashSet<_> = a.iter().collect();
        assert_eq!(uniq.len(), a.len(), "{a:?}");

        // What actually worked leads, and does not also appear further down.
        let remembered = tunnel_ports("dev-box", "machine-a", Some(31337));
        assert_eq!(remembered[0], 31337);
        assert_eq!(remembered.iter().filter(|&&p| p == 31337).count(), 1);
        assert_eq!(
            tunnel_ports("dev-box", "machine-a", Some(a[2]))[0],
            a[2],
            "a remembered port that is also derived must still lead",
        );
    }

    /// The transcript arithmetic, against real-shaped rows: totals count
    /// everything including sidechains, context follows only the main line,
    /// and a malformed row is skipped rather than zeroing the account.
    #[test]
    fn usage_totals_count_sidechains_but_context_follows_the_main_line() {
        let lines = [
            r#"{"type":"user","message":{"role":"user"}}"#,
            r#"{"type":"assistant","isSidechain":false,"message":{"usage":{"input_tokens":10,"output_tokens":100,"cache_read_input_tokens":1000,"cache_creation_input_tokens":50}}}"#,
            "this line is not json",
            r#"{"type":"assistant","isSidechain":true,"message":{"usage":{"input_tokens":5,"output_tokens":200,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
            r#"{"type":"assistant","message":{"usage":{"input_tokens":2,"output_tokens":30,"cache_read_input_tokens":2000,"cache_creation_input_tokens":8}}}"#,
        ]
        .join("\n");
        let spend = parse_usage(Ledger::PerMessage, &lines);
        let sum = spend.account.expect("a per-message stretch always accounts");
        assert_eq!(sum.input, 17);
        assert_eq!(sum.output, 330, "the sidechain's spend is real spend");
        assert_eq!(sum.cache_read, 3000);
        assert_eq!(sum.cache_write, 58);
        // The last main-line row: 2 + 2000 + 8. The sidechain in between
        // must not have hijacked the context.
        assert_eq!(spend.context, Some(2010));
    }

    #[test]
    fn a_transcript_with_no_assistant_rows_has_no_context_yet() {
        let spend = parse_usage(Ledger::PerMessage, r#"{"type":"user","message":{}}"#);
        assert_eq!(spend.account, Some(Usage::default()));
        assert_eq!(spend.context, None);
    }

    /// Codex's rollout writes running totals, so the account is the **last**
    /// row rather than the sum of them. Reading it the other way is the
    /// mistake that does not look like one: a session's bill would grow by
    /// its whole history every turn, and every number on the panel would
    /// still be a plausible number.
    #[test]
    fn a_cumulative_ledger_is_read_as_a_total_not_as_a_delta() {
        let lines = [
            r#"{"type":"session_meta","payload":{"id":"x"}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":900,"output_tokens":50},"last_token_usage":{"input_tokens":1000}}}}"#,
            "half a line, still being written",
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":3000,"cached_input_tokens":2500,"cache_write_input_tokens":400,"output_tokens":120},"last_token_usage":{"input_tokens":2000}}}}"#,
        ]
        .join("\n");
        let spend = parse_usage(Ledger::Cumulative, &lines);
        let acc = spend.account.expect("the rows said what it cost");
        // The later row *is* the account. 1000 + 3000 would be the bug.
        assert_eq!(acc.cache_read, 2500);
        assert_eq!(acc.cache_write, 400);
        // The cached and written parts are counted inside `input_tokens`, so
        // the fresh column is the remainder — and the three still add back
        // up to the prompt Codex reported.
        assert_eq!(acc.input, 100, "the cache was double-counted");
        assert_eq!(acc.input + acc.cache_read + acc.cache_write, 3000);
        assert_eq!(acc.output, 120);
        // The prompt the next turn grows from is the last request's, not
        // the running total.
        assert_eq!(spend.context, Some(2000));
    }

    /// A stretch that said nothing about cost must leave the account alone.
    /// For a cumulative ledger that is the difference between "quiet turn"
    /// and "the session suddenly cost nothing".
    #[test]
    fn a_cumulative_stretch_with_no_counts_leaves_the_account_standing() {
        let spend = parse_usage(
            Ledger::Cumulative,
            r#"{"type":"event_msg","payload":{"type":"agent_message","message":"hi"}}"#,
        );
        assert_eq!(spend.account, None);
        assert_eq!(spend.context, None);
    }

    /// The invoke boundary check for the editable diff: everything a diff
    /// legitimately names passes; everything that would leave the worktree
    /// does not.
    #[test]
    fn worktree_relative_paths_are_told_from_escapes() {
        for ok in ["src/app.ts", "README.md", "a/b/c.rs", "weird..name.txt", "深/中文.md"] {
            assert!(ensure_worktree_relative(ok).is_ok(), "{ok} should pass");
        }
        for bad in [
            "",
            "/etc/passwd",
            "\\server\\share",
            "../outside.txt",
            "src/../../outside.txt",
            "src\\..\\..\\outside.txt",
            "C:/Windows/system32",
        ] {
            assert!(ensure_worktree_relative(bad).is_err(), "{bad} should be refused");
        }
    }

    fn tab(id: &str, slots: Vec<&str>) -> StoredTab {
        StoredTab {
            id: id.into(),
            name: id.into(),
            layout: DEFAULT_LAYOUT.into(),
            slots: slots.into_iter().map(|s| Some(s.to_string())).collect(),
            position: 0,
        }
    }

    fn ids(t: &StoredTab) -> Vec<&str> {
        t.slots.iter().filter_map(|s| s.as_deref()).collect()
    }

    /// The uniqueness rule, isolated from the app so it can be exercised
    /// without a running core: claiming a session must vacate it elsewhere.
    fn claim(tabs: &mut [StoredTab], id: &str, slots: Vec<Option<String>>) {
        let claimed: std::collections::HashSet<&str> =
            slots.iter().filter_map(|s| s.as_deref()).collect();
        for t in tabs.iter_mut() {
            if t.id == id {
                t.slots = slots.clone();
            } else {
                t.slots
                    .retain(|s| !s.as_deref().is_some_and(|x| claimed.contains(x)));
            }
        }
    }

    #[test]
    fn a_session_claimed_by_one_tab_leaves_every_other() {
        let mut tabs = vec![tab("a", vec!["s1", "s2"]), tab("b", vec![])];
        claim(&mut tabs, "b", vec![Some("s1".into())]);

        // s1 has one PTY and therefore one size; two tabs showing it would
        // resize it against each other on every switch.
        assert_eq!(ids(&tabs[0]), vec!["s2"]);
        assert_eq!(ids(&tabs[1]), vec!["s1"]);
    }

    /// Losing a session must close the gap rather than leave one. A blank
    /// position is indistinguishable from one the user emptied on purpose,
    /// and every rule that tried to tell them apart guessed wrong somewhere.
    #[test]
    fn a_claimed_session_leaves_no_hole_behind() {
        let mut tabs = vec![tab("a", vec!["s1", "s2", "s3"]), tab("b", vec![])];
        claim(&mut tabs, "b", vec![Some("s2".into())]);
        assert_eq!(ids(&tabs[0]), vec!["s1", "s3"]);
    }

    #[test]
    fn claiming_does_not_disturb_sessions_it_did_not_ask_for() {
        let mut tabs = vec![tab("a", vec!["s1", "s2"]), tab("b", vec![])];
        claim(&mut tabs, "b", vec![Some("s3".into())]);
        assert_eq!(ids(&tabs[0]), vec!["s1", "s2"]);
    }

    #[test]
    fn only_blocking_states_count_as_needing_you() {
        assert!(Status::WaitingPermission.needs_you());
        assert!(Status::WaitingInput.needs_you());
        // A finished turn is your move, but it is not blocking the agent, so
        // it must not raise a notification or a badge.
        assert!(!Status::Idle.needs_you());
        assert!(!Status::Running.needs_you());
        assert!(!Status::Saved.needs_you());
        assert!(!Status::Exited.needs_you());
    }

    /// The prompt goes last, after every option. The hook wiring is appended
    /// by us, so building the vector in the obvious order — user args, then
    /// prompt, then ours — would put a positional argument in front of an
    /// option.
    #[test]
    fn the_prompt_is_the_last_argument_on_the_command_line() {
        let hook = Cli::Claude.hook_args(&wiring());
        let args = build_args(
            Vec::new(),
            hook,
            Tail::Prompt("[Marol 任務] 修好登入\n\n多行的 prompt".into()),
        );
        assert_eq!(args[0], "--plugin-dir");
        assert_eq!(args[1], "/data/plugin");
        assert!(args[2].starts_with("[Marol"));
        assert_eq!(args.len(), 3);
    }

    #[test]
    fn reopening_passes_continue_as_an_option_and_sends_no_prompt() {
        let (opts, tail) = resume_line(Some(Cli::Claude), PermissionMode::Normal);
        let args = build_args(opts, Cli::Claude.hook_args(&wiring()), tail);
        assert_eq!(args, vec!["--continue", "--plugin-dir", "/data/plugin"]);
    }

    /// The same sentence in Codex's grammar, where resuming is a subcommand
    /// that takes `[SESSION_ID] [PROMPT]` of its own. Nothing of ours may
    /// follow it: an argument there is not rejected, it is read as the name
    /// of a session to resume.
    #[test]
    fn a_codex_resume_puts_every_option_in_front_of_the_subcommand() {
        let (opts, tail) = resume_line(Some(Cli::Codex), PermissionMode::AcceptEdits);
        let args = build_args(opts, Cli::Codex.hook_args(&wiring()), tail);
        let resume = args
            .iter()
            .position(|a| a == "resume")
            .expect("a codex resume names its subcommand");
        assert_eq!(args[resume + 1], "--last");
        assert_eq!(resume + 2, args.len(), "nothing may follow the subcommand");
        for flag in ["--sandbox", "-c"] {
            let at = args.iter().position(|a| a == flag).expect(flag);
            assert!(at < resume, "{flag} landed after the subcommand");
        }
        // And no `--continue`, which is not a thing Codex has.
        assert!(!args.iter().any(|a| a == "--continue"));
    }

    /// A CLI nobody measured is opened in its directory and left alone: no
    /// hook wiring it would refuse to start on, no resume flag it does not
    /// have, no permission mode translated into a guess.
    #[test]
    fn an_unmeasured_agent_is_handed_nothing_of_ours() {
        assert_eq!(Cli::of("gemini"), None);
        let (opts, tail) = resume_line(Cli::of("gemini"), PermissionMode::Yolo);
        let args = build_args(
            {
                let mut o = vec!["--model".to_string(), "o3".to_string()];
                o.extend(opts);
                o
            },
            Vec::new(),
            tail,
        );
        assert_eq!(args, vec!["--model", "o3"]);
    }

    fn wiring() -> hooks::Wiring {
        hooks::Wiring {
            plugin_dir: "/data/plugin".to_string(),
            url: "http://127.0.0.1:1234/h/tok".to_string(),
        }
    }

    /// Where a report lands when the session id did not survive the trip.
    ///
    /// The id wins whenever it names a session we have. Otherwise the
    /// working directory is the way home — and only when it names exactly
    /// one, because attributing a whole session's work to the wrong card is
    /// worse than showing no status at all.
    #[test]
    fn a_report_finds_its_session_by_id_first_and_by_directory_only_when_sure() {
        let mut sessions = HashMap::new();
        for (id, cwd, live) in [
            ("s-attempt", "/home/me/.marol/worktrees/login-1", true),
            ("s-wsl", "wsl://Ubuntu/home/me/.marol/worktrees/pay-1", true),
            ("s-adhoc-a", "/repo", true),
            ("s-adhoc-b", "/repo", true),
            ("s-closed", "/gone", false),
        ] {
            let mut m = meta(id);
            m.cwd = cwd.to_string();
            m.live = live;
            sessions.insert(id.to_string(), m);
        }

        // The id, when it is one we know.
        assert_eq!(
            session_for(&sessions, Some("s-attempt"), Some("/anywhere")).as_deref(),
            Some("s-attempt")
        );
        // A stale id from a previous run falls through to the directory.
        assert_eq!(
            session_for(
                &sessions,
                Some("from-the-last-run"),
                Some("/home/me/.marol/worktrees/login-1")
            )
            .as_deref(),
            Some("s-attempt")
        );
        // No id at all: the directory alone.
        assert_eq!(
            session_for(&sessions, None, Some("/home/me/.marol/worktrees/login-1")).as_deref(),
            Some("s-attempt")
        );
        // The agent inside a world only ever knew the plain path; the row
        // carries the world in front of it.
        assert_eq!(
            session_for(&sessions, None, Some("/home/me/.marol/worktrees/pay-1")).as_deref(),
            Some("s-wsl")
        );
        // Two live sessions in one directory: refused, not guessed.
        assert_eq!(session_for(&sessions, None, Some("/repo")), None);
        // A session with no terminal is not a candidate — its hooks are over.
        assert_eq!(session_for(&sessions, None, Some("/gone")), None);
        // And nothing to go on is nothing to place it on.
        assert_eq!(session_for(&sessions, None, None), None);
    }

    fn meta(id: &str) -> SessionMeta {
        SessionMeta {
            id: id.to_string(),
            cwd: String::new(),
            title: id.to_string(),
            agent: "claude".to_string(),
            status: Status::Running,
            created_at: 0,
            last_active_at: 0,
            live: true,
            reports_status: false,
            hooks_wired: false,
            activity: None,
            activity_since: 0,
            completed: false,
            attempt_id: None,
            agent_session: true,
            has_followup: false,
            preview_port: None,
            usage: None,
            transcript_path: None,
        }
    }

    /// The gate that keeps `--name` off an older CLI: unknown or old reads
    /// as "do not", because the flag stops that claude from starting at all.
    ///
    /// Both CLIs' real `--version` output is here, because they do not
    /// agree about it: Claude Code leads with the number, Codex leads with
    /// its own name. A parser that only knew the first would read every
    /// Codex as "unknown" — which is not a visible failure, it is every
    /// version-gated feature silently off.
    #[test]
    fn the_name_flag_is_gated_on_a_measured_version() {
        assert_eq!(parse_version("2.1.226 (Claude Code)"), Some((2, 1, 226)));
        assert_eq!(parse_version("10.0.0"), Some((10, 0, 0)));
        assert_eq!(parse_version("codex-cli 0.145.0"), Some((0, 145, 0)));
        assert_eq!(parse_version("0.146.0-alpha.3"), Some((0, 146, 0)));
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("claude: command not found"), None);
        assert_eq!(parse_version("codex: command not found"), None);
        assert_eq!(parse_version("2.1"), None);

        let since = Some(NAMED_SESSIONS_SINCE);
        assert!(Some((2, 1, 224)) >= since);
        assert!(Some((2, 2, 0)) >= since);
        assert!(Some((2, 1, 223)) < since, "one release short must stay off");
        assert!(None::<(u64, u64, u64)> < since, "unknown must stay off");
    }

    /// A name is repaired, not refused.
    ///
    /// Half of these arrive from an agent's `curl` rather than a person's
    /// keyboard, and the shapes below are what that actually produces: a
    /// trailing newline from `echo`, a wrapped line from a heredoc, an escape
    /// sequence from a terminal that thought it was talking to a screen. All
    /// of them carry a perfectly good name, and rejecting one would leave the
    /// row saying nothing about a session that just tried to say something.
    #[test]
    fn a_name_is_made_into_one_line_rather_than_turned_away() {
        assert_eq!(clean_title("  改登入導向\n"), Some("改登入導向".into()));
        assert_eq!(
            clean_title("Fix the login\n   redirect"),
            Some("Fix the login redirect".into())
        );
        assert_eq!(clean_title("bell\u{7}rings"), Some("bellrings".into()));

        // Only nothing is nothing. A row whose name had been blanked is a row
        // that can no longer be picked out at all.
        assert_eq!(clean_title(""), None);
        assert_eq!(clean_title("  \n\t "), None);

        // Bounded, in characters rather than bytes — the cap is about how
        // wide the row is, and 「改」 is one column-ish and three bytes.
        let long = clean_title(&"改".repeat(200)).unwrap();
        assert_eq!(long.chars().count(), MAX_TITLE);
        assert_eq!(clean_title(&"x".repeat(200)).unwrap().len(), MAX_TITLE);
    }

    #[test]
    fn hook_states_map_onto_session_status() {
        assert_eq!(Status::from_hook(HookState::Running), Status::Running);
        assert_eq!(
            Status::from_hook(HookState::WaitingPermission),
            Status::WaitingPermission
        );
        assert_eq!(Status::from_hook(HookState::Idle), Status::Idle);
        assert_eq!(Status::from_hook(HookState::Ended), Status::Exited);
    }
}

/// Every `MAROL_*` variable, set under its old spelling too.
///
/// These are not internal plumbing: `$MAROL_ROOT_PATH` and `$MAROL_PORT` are
/// read by shell lines the *person* wrote, in a config file that lives inside
/// their own repository and is usually committed. This app deciding to change
/// its name is not a reason for `cp "$AGENTDESK_ROOT_PATH/.env" .env` to stop
/// working — a setup script that silently stopped copying would show up as
/// "the worktree is mysteriously broken", which is the exact failure the
/// config file was added to prevent.
///
/// Both are set rather than one translated, because a repository may well
/// have been edited already: the two names name the same thing, so a script
/// using either gets the same answer.
fn under_both_names(vars: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(vars.len() * 2);
    for (k, v) in vars {
        if let Some(rest) = k.strip_prefix("MAROL_") {
            out.push((format!("AGENTDESK_{rest}"), v.clone()));
        }
        out.push((k, v));
    }
    out
}

/// The derivation behind `Core::tunnel_ports`, kept apart from the disk so it
/// can be asked the questions that matter without one.
fn tunnel_ports(host: &str, machine: &str, remembered: Option<u16>) -> Vec<u16> {
    let seed = pty::desk_tag(&format!("{host}\n{machine}"));
    let base = u32::from_str_radix(&seed, 16).unwrap_or(0) as u64;
    let mut ports: Vec<u16> = (0..6)
        .map(|i| 20000 + ((base + i * 4093) % 40000) as u16)
        .collect();
    if let Some(p) = remembered {
        ports.retain(|&x| x != p);
        ports.insert(0, p);
    }
    ports
}

/// Ask a world whether it can hold sessions, and set it up if it can.
///
/// Four questions, in the order that makes a "no" cheapest: is there a tmux in
/// there, can this app put a config where that tmux will read it, who are we
/// over there, and can we make a directory of our own. Any "no" and the world
/// simply does not hold — the same answer a machine without tmux has always
/// given, and every card in it goes on working exactly as before.
///
/// One round trip apiece, once per host per run, beside the environment probe
/// that already costs the same.
fn world_hold(hr: &HostRef, home: &str) -> Option<WorldHold> {
    if let Err(e) = hr.run_ok("tmux", &["-V"], None) {
        eprintln!("[core] no tmux in this world, so nothing holds its sessions: {e:#}");
        return None;
    }
    // The config is a regular file and can live anywhere; the home is where it
    // belongs, beside everything else this app leaves in a world.
    let conf = format!("{home}/.marol/tmux.conf");
    if let Err(e) = hr.write_file(&conf, pty::HOLD_CONF) {
        eprintln!("[core] could not write the tmux config into this world: {e:#}");
        return None;
    }
    // The socket cannot, and the reason is arithmetic. A unix socket address
    // has about 104 bytes for its entire path, of which a session id already
    // spends 36 — and a home directory is unbounded. Measured, not feared: a
    // macOS temp home put the path at 135 bytes and every session in that
    // world silently failed to start.
    //
    // So sockets go where tmux keeps its own, and for the same reason:
    // `/tmp`, short and always there, one directory per uid because `/tmp` is
    // everyone's. `0700` on it is what makes that safe, and it is asked for
    // rather than inherited — a umask this side never sees decides the rest.
    let Some(uid) = hr
        .run_ok("id", &["-u"], None)
        .ok()
        .filter(|u| !u.is_empty() && u.chars().all(|c| c.is_ascii_digit()))
    else {
        eprintln!("[core] this world would not say who we are there; nothing will be held");
        return None;
    };
    // Out here the app's name is on the *directory*, not on the sockets — they
    // are named for the desk alone, because this directory is already ours. So
    // a world still holding sessions from before the rename goes on using the
    // directory they are in, and moves only once it is empty. Unlinking a live
    // agent's socket would take away the only name it has.
    let former = format!("/tmp/agentdesk-{uid}");
    let socket_dir = if hr.list_dir(&former).is_empty() {
        format!("/tmp/marol-{uid}")
    } else {
        eprintln!("[core] this world still holds sessions in {former}; staying there");
        former
    };
    if let Err(e) = hr.mkdir_p(&socket_dir) {
        eprintln!("[core] could not make a socket directory in this world: {e:#}");
        return None;
    }
    if let Err(e) = hr.run_ok("chmod", &["700", &socket_dir], None) {
        eprintln!("[core] could not lock down {socket_dir}: {e:#}");
        return None;
    }
    Some(WorldHold {
        conf,
        socket_dir: Some(socket_dir),
    })
}

/// Is anything still answering on this socket, here?
///
/// The one question both the local startup check and the local sweep ask, so
/// they cannot answer it differently. A failure to run tmux at all reads as
/// "no", which is the safe direction for the check and, in the sweep, is why
/// the unlink asks again rather than trusting a kill it may never have run.
fn tmux_answers(tmux: &std::path::Path, socket: &pty::Socket) -> bool {
    let (_, args) = pty::hold_alive(socket);
    std::process::Command::new(tmux)
        .args(&args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The same question, asked inside another world. A failure to reach the
/// world at all reads as "no", which is safe for the startup check — a row
/// stays `Saved`, which is what it already said — and never reaches the
/// sweep, which only ever runs against a world that has already answered.
fn hold_answers(hr: &HostRef, socket: &pty::Socket) -> bool {
    let (p, a) = pty::hold_alive(socket);
    let a: Vec<&str> = a.iter().map(String::as_str).collect();
    hr.run(&p, &a, None)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Does this stored path name this machine? Cheaper than resolving the world,
/// and the only thing the local startup pass needs to know.
fn is_local(cwd: &str) -> bool {
    host::locate(cwd).map(|l| l.host == Host::Local).unwrap_or(true)
}

/// Where tmux keeps its sockets: `$TMUX_TMPDIR` or `/tmp`, then `tmux-<uid>`.
///
/// Read rather than asked for, because the ids this desk has forgotten exist
/// nowhere else — `list-sessions` can only speak for a server you can already
/// name. Unix only, which costs nothing: this is the *local* socket directory,
/// and a machine with no tmux holds nothing to look for. Another world's
/// sockets live where the app put them and need none of this.
#[cfg(unix)]
pub fn tmux_socket_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var("TMUX_TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    // SAFETY: getuid is always safe; it reads a process property and cannot fail.
    let uid = unsafe { libc_getuid() };
    Some(std::path::PathBuf::from(base).join(format!("tmux-{uid}")))
}

#[cfg(not(unix))]
pub fn tmux_socket_dir() -> Option<std::path::PathBuf> {
    None
}

// The one libc call this crate needs, declared rather than adding a
// dependency for a single symbol. A plain comment: rustc does not accept a
// doc comment on an extern block, and warns rather than rendering it.
#[cfg(unix)]
extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}
