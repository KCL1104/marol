//! Session status via the agents' own hooks.
//!
//! The app needs to know when a session is waiting for you, and what it is
//! doing while it is not — that is what makes several agents at once
//! manageable. Scraping the terminal for it would mean parsing ANSI and would
//! break the next time a TUI changes, so instead we ask the CLI to tell us.
//!
//! Both measured CLIs post to the same listener, in the same shape, and are
//! configured in the way each one actually offers — never by editing a file
//! the person owns. Claude Code loads a **plugin** named by `--plugin-dir`;
//! `--settings` was rejected because it overrides same-named keys, so
//! injecting a `hooks` key there would silently disable the user's own hooks.
//! Codex has no per-launch plugin flag, so its hooks ride in as `-c`
//! overrides — config for one launch, touching nothing on disk. See
//! `agent.rs` for which is which.
//!
//! Three decisions on the Claude Code side were settled by measurement, not
//! by documentation:
//!
//!   * Most events use the **`http`** hook type: no subprocess per tool call,
//!     and the request body carries the full payload including `tool_name`
//!     and `tool_input`. Session identity rides in a header, expanded from
//!     `MAROL_SESSION_ID` via `allowedEnvVars`.
//!   * **`SessionStart` is the exception** — an `http` hook on it never
//!     fires, while a `command` hook does. It runs once per session, so the
//!     one subprocess costs nothing.
//!   * A `command` hook must always exit 0: a non-zero exit *blocks* the
//!     action it fired on, so a stopped app must never be able to wedge a
//!     session.
//!
//! Codex offers only `command` hooks, so it pays a `curl` per tool call. That
//! is its price rather than a choice of ours, and it is why every Codex hook
//! carries a short timeout: the default is ten minutes, and a listener that
//! has gone away must cost a session a blink, not a coffee break.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Longest hook body we will read into memory. A `Write` tool's input can
/// carry a whole file; we only need the head of it, but the rest still has to
/// be drained so the sender never blocks.
const MAX_BODY: usize = 64 * 1024;

/// What a hook tells us about a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookState {
    Started,
    Running,
    WaitingPermission,
    WaitingInput,
    Idle,
    Ended,
}

impl HookState {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "started" => Self::Started,
            "running" => Self::Running,
            "waiting_permission" => Self::WaitingPermission,
            "waiting_input" => Self::WaitingInput,
            "idle" => Self::Idle,
            "ended" => Self::Ended,
            _ => return None,
        })
    }
}

/// What the agent is doing right now, for the session list and overview.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Activity {
    pub tool: String,
    /// The interesting argument: a command line, a path, a pattern.
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct HookReport {
    /// The session this claims to be from, when the id reached the hook
    /// intact. `None` when it did not — see `expanded`.
    pub session_id: Option<String>,
    /// The directory the agent is working in, from the payload. Every hook
    /// of both CLIs carries it, and it is the only way home for a report
    /// whose session id never got expanded.
    pub cwd: Option<String>,
    pub state: HookState,
    pub activity: Option<Activity>,
    /// Where the CLI keeps this conversation's JSONL — the one honest
    /// source of token usage. A common field of every hook payload of both
    /// CLIs; carried here so nobody has to reconstruct the path by guessing
    /// at either one's escaping rules.
    pub transcript_path: Option<String>,
}

/// A session saying what it should be called.
///
/// The other direction of the same channel the status hooks use, and the only
/// thing on it that is not a status. It arrives from the agent's own shell
/// rather than from a hook the CLI fires, so the session id is already in the
/// URL: nothing has to expand, and `cmd.exe` cannot lose it.
///
/// Unlike a status report there is no falling back on the working directory
/// here. A status report that cannot be placed is a gap in a status; a name
/// landing on the wrong session renames somebody else's card, and two agents
/// sharing a directory is exactly the situation this app is built for.
#[derive(Debug, Clone)]
pub struct NameReport {
    /// Which session is naming itself. Baked into the URL that was handed to
    /// the session, so it is here whenever the URL was not edited.
    pub session_id: String,
    /// What it wants to be called, exactly as the body carried it. Cleaning
    /// it is the core's business — the same cleaning a person's rename gets.
    pub name: String,
}

/// What one world needs in order to point a CLI at this listener.
///
/// Both halves are per-world, not per-session: an SSH host gets a plugin
/// written into its own filesystem and a URL that comes back through the
/// reverse tunnel, and a WSL distro reads this machine's plugin through the
/// drive mounts. Which of the two a given CLI uses is `agent.rs`'s business.
#[derive(Debug, Clone)]
pub struct Wiring {
    /// The directory `--plugin-dir` names, spelled the way that world spells
    /// paths.
    pub plugin_dir: String,
    /// The endpoint the hooks post to, from inside that world.
    pub url: String,
}

/// The URL a session posts its own name to.
///
/// One session's address, not a world's: the id is in the query rather than
/// left for a shell to expand, so the whole thing can be handed over as a
/// single environment variable and used verbatim. That is the difference
/// between an agent that can name itself in one line and one that has to
/// compose a URL correctly under whichever shell it happens to have.
pub fn name_url(hook_url: &str, session_id: &str) -> String {
    format!("{hook_url}?sid={session_id}&set=name")
}

/// Where a session asks who else is on this desk, and where it writes to one.
///
/// Both carry a per-session token that `name_url` deliberately does not.
/// Naming is a session talking about *itself*, and the worst a forged one can
/// do is retitle a row. These two are different in kind: `peers` discloses the
/// desk's other sessions, and `send` puts text into one of them. That turns
/// the endpoint from "report about yourself" into "act on someone else", and
/// an address anything on the machine could guess is not enough to stand
/// behind that.
///
/// The token is minted per session and never leaves this process except into
/// that session's own environment — in particular it is not on `SessionMeta`,
/// which is serialised straight to the webview.
pub fn peers_url(hook_url: &str, session_id: &str, token: &str) -> String {
    format!("{hook_url}?sid={session_id}&tok={token}&peers=1")
}

pub fn send_url(hook_url: &str, session_id: &str, token: &str) -> String {
    format!("{hook_url}?sid={session_id}&tok={token}&send=1")
}

/// What arrived on the listener.
pub enum Incoming {
    Status(HookReport),
    Name(NameReport),
    /// A session asking what else is running on this desk.
    Peers { session_id: String, token: String },
    /// A session writing to another one, named by its id.
    Send {
        session_id: String,
        token: String,
        to: String,
        text: String,
    },
}

pub trait HookHandler: Send + Sync + 'static {
    fn on_hook(&self, report: HookReport);

    /// A session saying what it should be called. Ignored by default, so a
    /// handler that only cares about status stays as short as it was.
    fn on_name(&self, _report: NameReport) {}

    /// Who else is on this desk, as plain text — one session per line,
    /// `id<TAB>name<TAB>status`. `None` refuses: an unknown session, or a
    /// token that is not that session's.
    ///
    /// Ids rather than names as the address, and that is what keeps the whole
    /// channel free of escaping: a uuid is safe in a header and a query, while
    /// a name is a person's sentence and may hold anything.
    fn on_peers(&self, _session_id: &str, _token: &str) -> Option<String> {
        None
    }

    /// One session writing to another. `Err` is a reason to hand back to the
    /// sender — a wrong token, a session that is gone, a full queue.
    fn on_send(
        &self,
        _session_id: &str,
        _token: &str,
        _to: &str,
        _text: &str,
    ) -> Result<(), String> {
        Err("this desk is not carrying messages".to_string())
    }
}

pub struct HookServer {
    pub port: u16,
    /// Shared secret in the URL, so another local process cannot forge status.
    pub token: String,
    pub plugin_dir: PathBuf,
    /// The accept loop. Held so shutting the desk down gives the port back
    /// rather than sitting on it: the port is part of the address held
    /// sessions were told to use, and the next run has to be able to take it.
    accept: tokio::task::AbortHandle,
}

impl HookServer {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/h/{}", self.port, self.token)
    }

    /// Stop listening and release the port.
    pub fn stop(&self) {
        self.accept.abort();
    }
}

/// Where the port and token are kept between runs. See `start`.
const ENDPOINT_FILE: &str = "hook-endpoint";

/// The endpoint the last run used, if it is still readable and sane.
///
/// The token is checked, not merely read. It goes straight into a URL path,
/// so a file that has been edited by hand — or truncated by a full disk —
/// must not be able to smuggle a second path segment into the route.
fn remembered(data_dir: &Path) -> (Option<u16>, Option<String>) {
    let Ok(text) = std::fs::read_to_string(data_dir.join(ENDPOINT_FILE)) else {
        return (None, None);
    };
    let mut lines = text.lines();
    let port = lines.next().and_then(|s| s.trim().parse::<u16>().ok());
    let token = lines
        .next()
        .map(str::trim)
        .filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric()))
        .map(str::to_string);
    (port, token)
}

/// Write the endpoint down for the next run.
///
/// The token is the only thing between another local process and the ability
/// to forge status for a session, so it is written the way a key is written.
/// The mode is asked for at creation *and* set afterwards: `mode()` is
/// ignored when the file already exists, and a chmod that follows the write
/// leaves a window where the secret is readable — a secret is only as good as
/// its narrowest moment.
fn remember(data_dir: &Path, port: u16, token: &str) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join(ENDPOINT_FILE);
    let body = format!("{port}\n{token}\n");
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        f.write_all(body.as_bytes())?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    std::fs::write(&path, body)?;
    Ok(())
}

/// Take last run's port if it is still free, otherwise any port.
///
/// Briefly patient about it. The commonest reason the old port is busy is the
/// previous instance still letting go of it — a relaunch, or an upgrade
/// restarting the app — and giving up on the first refusal would silence
/// every session that instance left running, for the sake of a fifth of a
/// second.
async fn bind_preferring(port: Option<u16>) -> Result<TcpListener> {
    if let Some(p) = port.filter(|p| *p != 0) {
        let mut last = None;
        for _ in 0..10 {
            match TcpListener::bind(("127.0.0.1", p)).await {
                Ok(l) => return Ok(l),
                Err(e) => last = Some(e),
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        if let Some(e) = last {
            // Somebody else has it for good: a second Marol install, or an
            // unrelated program. Taking a fresh port loses the reports from
            // sessions the previous run left running, which is exactly where
            // this was before any of it was remembered — so it degrades to the
            // old behaviour rather than refusing to start.
            eprintln!("[hooks] port {p} is taken ({e}); sessions held by the last run will stay quiet");
        }
    }
    Ok(TcpListener::bind("127.0.0.1:0").await?)
}

/// Bind a loopback listener and write the companion plugin.
///
/// **The endpoint is the same one as last time, when it can be.** A session
/// tmux held through a restart is still running, but the URL it reports to
/// was baked into `hooks.json` when the session started, and Claude Code
/// reads that file once. Both halves of that URL used to be fresh every run —
/// an ephemeral port and a new uuid — so a held agent kept posting into
/// nothing: it ran on, and the desk went blind to it for the rest of its
/// life. Remembering the pair is not a second channel; it is the existing one
/// made to survive the thing it was already meant to survive.
pub async fn start(data_dir: &Path, handler: Arc<dyn HookHandler>) -> Result<HookServer> {
    let (kept_port, kept_token) = remembered(data_dir);
    let listener = bind_preferring(kept_port)
        .await
        .context("binding the hook listener")?;
    let port = listener.local_addr()?.port();
    let token = kept_token.unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
    if let Err(e) = remember(data_dir, port, &token) {
        // Not fatal: this run works either way. Only the *next* one loses the
        // sessions this one leaves behind.
        eprintln!("[hooks] could not remember the endpoint: {e}");
    }

    let plugin_dir = data_dir.join("plugin");
    let url = format!("http://127.0.0.1:{port}/h/{token}");
    write_plugin(&plugin_dir, &url).context("writing the status plugin")?;

    let want = format!("/h/{token}");
    let accept = tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(e) => {
                    eprintln!("[hooks] accept failed: {e}");
                    continue;
                }
            };
            let handler = Arc::clone(&handler);
            let want = want.clone();
            tokio::spawn(async move { serve(stream, &want, handler).await });
        }
    })
    .abort_handle();

    eprintln!("[hooks] listening on 127.0.0.1:{port}, plugin at {}", plugin_dir.display());
    Ok(HookServer {
        port,
        token,
        plugin_dir,
        accept,
    })
}

/// Handle one request.
///
/// Hand-rolled rather than a web framework: there is one route, the only
/// clients are Claude Code's own hook runner and our `curl` one-liner, and the
/// reply is always the same. Every request is answered 200 so a hook never
/// fails and never blocks the agent.
///
/// The route carries two kinds of message now — a status report, and a
/// session saying what it should be called — told apart by `set=name` in the
/// query. One route rather than two because the *token* is the hard part:
/// it survives restarts, rides a WSL mount and an SSH tunnel, and every one
/// of those arrangements already knows this URL. A second endpoint would have
/// had to be taught all of it again.
async fn serve(mut stream: tokio::net::TcpStream, want_prefix: &str, handler: Arc<dyn HookHandler>) {
    // A hook report is answered 200 whatever happens to it — a non-200 is a
    // failed hook as far as the CLI is concerned, and a desk's opinion must
    // never become an agent's problem. The two channels a session *asks*
    // something on are different: there, a refusal is the answer.
    let (status, body) = match read_request(&mut stream, want_prefix).await {
        Some(Incoming::Status(report)) => {
            handler.on_hook(report);
            ("200 OK", String::new())
        }
        Some(Incoming::Name(report)) => {
            handler.on_name(report);
            ("200 OK", String::new())
        }
        Some(Incoming::Peers { session_id, token }) => match handler.on_peers(&session_id, &token) {
            Some(list) => ("200 OK", list),
            None => ("403 Forbidden", "not this session's token\n".to_string()),
        },
        Some(Incoming::Send {
            session_id,
            token,
            to,
            text,
        }) => match handler.on_send(&session_id, &token, &to, &text) {
            Ok(()) => ("200 OK", "sent\n".to_string()),
            // 409 rather than 400: the request was well formed and the desk
            // declined it — a full queue, a session that has gone. The body
            // is the reason, in the sender's hands, which is the whole point
            // of refusing rather than dropping.
            Err(why) => ("409 Conflict", format!("{why}\n")),
        },
        None => ("200 OK", String::new()),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/plain; charset=utf-8\r\n\
         content-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn read_request(
    stream: &mut tokio::net::TcpStream,
    want_prefix: &str,
) -> Option<Incoming> {
    // Read until the headers are complete.
    let mut buf = Vec::with_capacity(8 * 1024);
    let mut chunk = [0u8; 8 * 1024];
    let head_end = loop {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        if buf.len() > MAX_BODY {
            return None;
        }
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let target = lines.next()?.split_whitespace().nth(1)?.to_string();
    if !target.starts_with(want_prefix) {
        return None;
    }

    // Session id arrives in a header from `http` hooks and in the query from
    // the `command` hook that covers SessionStart.
    let mut session_id: Option<String> = None;
    let mut to: Option<String> = None;
    let mut content_length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            // Both spellings. A session this desk held through the rename is
            // still running against the plugin config it was started with,
            // and that file is a photograph: Claude Code read it once, when
            // the app was still called AgentDesk. Refusing its header would
            // make every agent that survived the rename go dark for the rest
            // of its life — the same failure the stable endpoint exists to
            // prevent, caused by us instead of by a port.
            "x-marol-session" | "x-agentdesk-session" => session_id = Some(value.to_string()),
            // Whom a message is for, as that session's id. A header rather
            // than the query because it keeps `send_url` a constant the agent
            // uses verbatim — the same property that makes `$MAROL_NAME_URL`
            // safe to hand to a shell.
            "x-marol-to" => to = Some(value.to_string()),
            "content-length" => content_length = value.parse().unwrap_or(0),
            _ => {}
        }
    }

    let mut state = None;
    let mut naming = false;
    let mut peers = false;
    let mut sending = false;
    let mut token: Option<String> = None;
    if let Some(query) = target.split_once('?').map(|(_, q)| q) {
        for pair in query.split('&') {
            match pair.split_once('=') {
                Some(("sid", v)) if !v.is_empty() => session_id.get_or_insert(v.to_string()),
                Some(("state", v)) => {
                    state = HookState::parse(v);
                    continue;
                }
                Some(("set", "name")) => {
                    naming = true;
                    continue;
                }
                Some(("peers", _)) => {
                    peers = true;
                    continue;
                }
                Some(("send", _)) => {
                    sending = true;
                    continue;
                }
                Some(("tok", v)) if !v.is_empty() => {
                    token = Some(v.to_string());
                    continue;
                }
                _ => continue,
            };
        }
    }

    // Drain the body fully — a sender blocked on a half-read body would stall
    // the agent — but only keep the head of it for parsing.
    //
    // Bytes received and bytes kept are counted separately on purpose. Using
    // the kept length as the loop condition deadlocks once it is clamped at
    // the cap: it stops growing, the condition stays true, and the read waits
    // forever for data the sender already finished writing.
    let mut body = buf.split_off(head_end + 4);
    let mut received = body.len();
    while received < content_length {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        received += n;
        if body.len() < MAX_BODY {
            let take = (MAX_BODY - body.len()).min(n);
            body.extend_from_slice(&chunk[..take]);
        }
    }

    // A name is not a hook payload: the body is the name, as plain text, so
    // an agent can send one without composing JSON or percent-encoding a
    // query. Handled before the JSON parse for that reason — there is
    // nothing here for `serde_json` to read.
    // Both of these are answered rather than merely accepted, so they are
    // decided here beside naming — before the JSON parse, which has nothing
    // to read in either one.
    if peers || sending {
        let session_id = session_id.filter(|s| expanded(s))?;
        let token = token?;
        if peers {
            return Some(Incoming::Peers { session_id, token });
        }
        let text = String::from_utf8_lossy(&body).trim().to_string();
        let to = to.filter(|t| expanded(t))?;
        if text.is_empty() {
            return None;
        }
        return Some(Incoming::Send {
            session_id,
            token,
            to,
            text,
        });
    }

    if naming {
        let name = String::from_utf8_lossy(&body).trim().to_string();
        let session_id = session_id.filter(|s| expanded(s))?;
        if name.is_empty() {
            return None;
        }
        return Some(Incoming::Name(NameReport { session_id, name }));
    }

    let payload = serde_json::from_slice::<serde_json::Value>(&body).ok();
    let activity = payload.as_ref().and_then(activity_from_payload);
    let str_field = |key: &str| {
        payload
            .as_ref()
            .and_then(|v| v.get(key))
            .and_then(|v| v.as_str())
            .map(String::from)
    };

    let session_id = session_id.filter(|s| expanded(s));
    let cwd = str_field("cwd");
    // One or the other has to be there, or the report has nothing to land on
    // and inventing a session for it would be worse than dropping it.
    if session_id.is_none() && cwd.is_none() {
        return None;
    }

    Some(Incoming::Status(HookReport {
        session_id,
        cwd,
        state: state?,
        activity,
        transcript_path: str_field("transcript_path"),
    }))
}

/// Whether a session id is a session id rather than the name of one.
///
/// A `command` hook's URL is a string the CLI hands to a shell, so
/// `$MAROL_SESSION_ID` becomes the id — on a shell that spells variables
/// that way. On `cmd.exe` it stays four­teen literal characters, and a report
/// filed under `$MAROL_SESSION_ID` would be filed under a session that
/// cannot exist. Better to admit the id did not arrive and fall back to the
/// working directory, which every payload carries and no shell rewrites.
///
/// The test is deliberately about the *shape* of a variable reference rather
/// than a list of the two spellings: session ids here are uuids, so a `$` or
/// a `%` anywhere in one means something did not expand.
fn expanded(id: &str) -> bool {
    !id.is_empty() && !id.contains('$') && !id.contains('%')
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Turn a `PreToolUse` payload into a one-line description of the work.
fn activity_from_payload(v: &serde_json::Value) -> Option<Activity> {
    let tool = v.get("tool_name")?.as_str()?.to_string();
    let input = v.get("tool_input");

    let pick = |key: &str| -> Option<String> {
        input?
            .get(key)?
            .as_str()
            .map(|s| s.chars().take(160).collect())
    };

    // A message to another session names two things a human would ask for —
    // whom, and what — where everything below is a single argument.
    if tool == "SendMessage" {
        let to = pick("to").unwrap_or_default();
        let what = pick("summary")
            .or_else(|| pick("message"))
            .unwrap_or_default();
        let detail: String = if to.is_empty() {
            what
        } else {
            format!("→ {to}: {what}").chars().take(160).collect()
        };
        return Some(Activity { tool, detail });
    }

    // The argument a human would name the action by, per tool.
    let detail = pick("command")
        .or_else(|| pick("file_path"))
        .or_else(|| pick("path"))
        .or_else(|| pick("pattern"))
        .or_else(|| pick("url"))
        .or_else(|| pick("description"))
        .or_else(|| pick("query"))
        .unwrap_or_default();

    Some(Activity { tool, detail })
}

/* ------------------------------------------------------------------ */
/* Plugin generation                                                   */
/* ------------------------------------------------------------------ */

/// An `http` hook: Claude Code posts it itself, so there is no subprocess per
/// tool call and the body carries the full payload.
fn http_reporter(url: &str, state: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "http",
        "url": format!("{url}?state={state}"),
        "headers": { "X-Marol-Session": "$MAROL_SESSION_ID" },
        "allowedEnvVars": ["MAROL_SESSION_ID"],
        // Short: an unreachable listener should cost the agent no more than a
        // blink, and a refused connection fails instantly anyway.
        "timeout": 2
    })
}

/// A `command` hook, for the one event an `http` hook never fires on.
///
/// `--max-time` bounds a hung listener; `|| true` forces exit 0, because a
/// hook exiting non-zero blocks the action it fired on.
fn command_reporter(url: &str, state: &str) -> serde_json::Value {
    let cmd = format!(
        "curl -sS --max-time 2 -X POST \
         \"{url}?sid=$MAROL_SESSION_ID&state={state}\" -o /dev/null || true"
    );
    serde_json::json!({
        "type": "command",
        "command": cmd,
        // `shell` is deliberately unset. Setting it to "sh" makes Claude Code
        // skip the hook silently — no error, no report, nothing to debug from.
        // ("bash" works, as does omitting the field.) Measured, not documented.
        "async": true,
        "timeout": 5
    })
}

fn hooks_json(url: &str) -> serde_json::Value {
    serde_json::json!({
        "hooks": {
            // Measured: an `http` hook on SessionStart never fires; a
            // `command` hook does. It runs once per session either way.
            "SessionStart":     [{ "hooks": [command_reporter(url, "started")] }],
            "UserPromptSubmit": [{ "hooks": [http_reporter(url, "running")] }],
            // The one that carries what the agent is actually doing.
            "PreToolUse":       [{ "matcher": "*", "hooks": [http_reporter(url, "running")] }],
            // The two that matter most: the agent cannot continue without you.
            "PermissionRequest": [{ "matcher": "*", "hooks": [http_reporter(url, "waiting_permission")] }],
            "Notification": [
                { "matcher": "permission_prompt", "hooks": [http_reporter(url, "waiting_permission")] },
                { "matcher": "idle_prompt",       "hooks": [http_reporter(url, "waiting_input")] }
            ],
            "Stop":       [{ "hooks": [http_reporter(url, "idle")] }],
            "SessionEnd": [{ "hooks": [http_reporter(url, "ended")] }]
        }
    })
}

/* ------------------------------------------------------------------ */
/* Codex                                                               */
/* ------------------------------------------------------------------ */

/// Which Codex event reports which state, and what it may match on.
///
/// The events are Codex's names, not ours, and the mapping is the same one
/// the Claude Code plugin makes — the same six moments, reported the same
/// way, so a card cannot tell which CLI is behind it.
///
/// `waiting_input` has no entry, and that is honest rather than an omission:
/// Claude Code raises an idle prompt and says so, Codex does not, and a
/// state nothing can ever report is a state the card would wait for forever.
///
/// The last field is the seconds a hook may take. Codex's default is six
/// hundred — fine for a linter, absurd for a status ping — and `SessionEnd`
/// is capped at three by Codex itself, so it is asked for less than that
/// rather than for a number that will be quietly reduced.
const CODEX_EVENTS: [(&str, Option<&str>, &str, u32); 6] = [
    ("SessionStart", None, "started", 5),
    ("UserPromptSubmit", None, "running", 5),
    // The one that carries what the agent is actually doing.
    ("PreToolUse", Some("*"), "running", 5),
    // The one that matters most: the agent cannot continue without you.
    ("PermissionRequest", Some("*"), "waiting_permission", 5),
    ("Stop", None, "idle", 5),
    ("SessionEnd", None, "ended", 2),
];

/// The one-liner every Codex hook runs.
///
/// Four properties, each of which had a wrong version:
///
///   * **The payload is forwarded.** Codex has no `http` hook type, so the
///     JSON that carries `tool_name`, `tool_input` and `transcript_path`
///     arrives on the hook's stdin — `--data-binary @-` is what puts it in
///     the request body where the listener already knows how to read it.
///   * **It exits 0.** `|| exit 0` rather than `|| true`, because this
///     string is handed to whichever shell the platform has: `exit 0` means
///     the same thing in `sh` and in `cmd.exe`, where `true` is not a
///     command at all.
///   * **It is bounded.** An unreachable listener costs the agent two
///     seconds, once, rather than the hook timeout.
///   * **It has no single quotes.** The whole line goes into a TOML literal
///     string, which is the only kind that leaves a `$` alone — and a
///     literal string ends at the first `'`. Hence `-H content-type:...`
///     unquoted, which needs no quoting because it contains no space.
/// Escape a string to sit inside a shell **double**-quoted word.
///
/// Double quotes rather than single, and that is forced rather than chosen:
/// the whole command lives inside a TOML *literal* string in a `-c` argument,
/// and a literal string ends at the first `'`. So a single quote cannot
/// appear anywhere in a Codex hook command, and everything the shell would
/// otherwise interpret has to be turned off by hand.
///
/// `$` is escaped along with the rest, which is the point for this one
/// caller: the text names `$MAROL_PEERS_URL` so the agent reads a variable
/// name, and a shell that expanded it would put this session's token into the
/// model's context and into the transcript on disk instead.
fn dq_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        if matches!(c, '\\' | '"' | '$' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// What a Codex session is told about the desk it is running on.
///
/// The other half of `PEERS_SKILL`. Claude Code learns the channel from a
/// skill in the plugin `--plugin-dir` carries; Codex has no per-launch
/// equivalent, and its skills live in `~/.codex/skills` — the person's own
/// configuration, which this app does not write into. So it learns the same
/// thing through the one door Codex does offer: a `SessionStart` hook may
/// return `additionalContext`, and Codex records it as a developer message on
/// the conversation.
///
/// One line, deliberately. A JSON string cannot hold a raw newline, and the
/// alternative — escaping them through TOML, the shell and JSON in turn — is
/// three layers of quoting to save a paragraph break.
///
/// **Not measured.** `NAME_SKILL` carries a token figure because
/// `claude plugin details` can produce one; Codex offers no equivalent, and a
/// number invented to sit beside a measured one would be worse than none.
///
/// **What is verified, and what is read.** That Codex loads this `-c` value
/// is checked against a real CLI on a schedule by
/// `codex_loads_the_hook_config_this_app_passes` — a broken escape would show
/// up there as config it refused. That the hook still runs and still reports
/// is checked by `a_real_codex_reports_through_the_hooks_this_app_configures`.
/// What no test here can see is whether Codex still *honours*
/// `hookSpecificOutput.additionalContext` on `SessionStart`: that shape is
/// read from Codex's own source (`hooks/src/engine/output_parser.rs`
/// `parse_session_start`, `core/src/hook_runtime.rs`
/// `record_additional_contexts`), not measured through the binary. Should it
/// change, this stops teaching and nothing else breaks — the report still
/// goes, the session still runs, and a Codex agent is simply back to not
/// knowing about the channel.
const CODEX_PEERS_CONTEXT: &str = "You are running in a Marol window beside other agent sessions, which may be a different CLI. `curl -sS --max-time 3 \"$MAROL_PEERS_URL\"` lists them, one per line, as id<TAB>name<TAB>status. To send one a message: `curl -sS --max-time 3 -X POST \"$MAROL_SEND_URL\" -H \"X-Marol-To: <the id>\" --data-binary \"your message\"`. Use both variables exactly as they are; each already carries this session identity. The message arrives in that session terminal marked as coming from you and explicitly not from the person, so send facts, findings and warnings — another agent cannot approve anything on the person behalf, and neither can you. If either variable is unset, this session is not wired for it: do nothing and do not mention it. Use this when work here depends on, blocks, or duplicates work another session is doing, not to chat.";

/// The `SessionStart` hook: report the session, then tell it about the desk.
///
/// The report goes second so the context is on stdout whatever the network
/// does — a listener that has gone away must cost a session its status, not
/// the one thing it was going to be told.
///
/// Changing this text changes the hook hash, and Codex records trust against
/// that hash, so an upgrade that edits it asks for `/hooks` once more. That
/// is the mechanism working rather than a cost to route around: the person is
/// being shown a command that is about to run in their terminal.
fn codex_session_start_command(url: &str, max_time: u32) -> String {
    let payload = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": CODEX_PEERS_CONTEXT,
        }
    })
    .to_string();
    format!(
        "printf %s \"{}\"; {}",
        dq_escape(&payload),
        codex_command(url, "started", max_time)
    )
}

fn codex_command(url: &str, state: &str, max_time: u32) -> String {
    format!(
        "curl -sS --max-time {max_time} -X POST -H content-type:application/json \
         --data-binary @- \"{url}?sid=$MAROL_SESSION_ID&state={state}\" -o /dev/null || exit 0"
    )
}

/// The hooks table as Codex's config spells it, one `-c key=value` per event.
///
/// TOML, not JSON: `-c` parses its value as TOML, and the two disagree about
/// inline tables in a way that is silent — `{"a": 1}` is valid JSON and not
/// valid TOML, and Codex's own documented fallback is to keep an unparseable
/// value as a literal string, so a mistake here configures nothing at all
/// rather than failing. The parity workflow runs the real CLI against these
/// arguments and reads back `codex doctor`'s verdict for exactly that reason.
///
/// **Every launch passes the same text**, and that is the point rather than
/// an accident. Measured against Codex 0.147: a hook configured this way
/// does not run until it has been reviewed and trusted, and trust is
/// recorded against the hook's own hash. A definition carrying a session id
/// would be a different hook every time, and so would ask to be reviewed
/// once per attempt, forever. The id is therefore left as
/// `$MAROL_SESSION_ID` for the shell to expand — measured too: it arrives
/// expanded — and the listener knows what to do when a shell does not.
///
/// So the first Codex session on a machine says its hooks need review, in
/// its own terminal, in its own words, and one `/hooks` answers it for every
/// session afterwards. That prompt is Codex's, not ours, and it is left
/// where the person can see it: this app does not pass
/// `--dangerously-bypass-hook-trust`, which would also wave through any
/// hooks the repository itself carries.
pub fn codex_config_args(url: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(CODEX_EVENTS.len() * 2);
    for (event, matcher, state, timeout) in CODEX_EVENTS {
        let matcher = match matcher {
            Some(m) => format!("matcher=\"{m}\","),
            None => String::new(),
        };
        // Always shorter than the hook's own budget, so a slow listener is
        // curl giving up rather than Codex reporting a failed hook.
        // Always shorter than the hook's own budget, so a slow listener is
        // curl giving up rather than Codex reporting a failed hook. One rule,
        // applied to both shapes — SessionStart carries a context as well as
        // a report, but the part that can be slow is the same curl.
        let max_time = timeout.saturating_sub(1).min(2);
        let command = if event == "SessionStart" {
            codex_session_start_command(url, max_time)
        } else {
            codex_command(url, state, max_time)
        };
        out.push("-c".to_string());
        out.push(format!(
            "hooks.{event}=[{{{matcher}hooks=[{{type=\"command\",command='{command}',timeout={timeout}}}]}}]"
        ));
    }
    out
}

/// The plugin as files, for provisioning into a host that cannot see our
/// disk: an SSH host gets these written into its own filesystem, with a URL
/// that points back through the reverse tunnel.
pub fn plugin_files(url: &str) -> Vec<(&'static str, String)> {
    let manifest = serde_json::json!({
        "name": "marol-status",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Reports session status to the Marol window, and lets a session say what it should be called there. Adds no tools and changes nothing about how the agent works."
    });
    vec![
        (
            ".claude-plugin/plugin.json",
            serde_json::to_string_pretty(&manifest).unwrap_or_default(),
        ),
        (
            "hooks/hooks.json",
            serde_json::to_string_pretty(&hooks_json(url)).unwrap_or_default(),
        ),
        ("skills/name-this-session/SKILL.md", NAME_SKILL.to_string()),
        ("skills/message-another-session/SKILL.md", PEERS_SKILL.to_string()),
    ]
}

/// The one thing in the plugin that is not a hook.
///
/// A skill rather than a hook because naming is a judgement, and the only
/// party who knows what this session turned out to be about is the agent
/// running in it. It is offered, never required: a session that never uses it
/// keeps the name Marol gave it, which is what every session did before this
/// existed.
///
/// **What it costs, measured rather than assumed.** Claude Code 2.1.229,
/// `claude --plugin-dir <this> plugin details marol-status`: one skill, ~90
/// tokens always-on in every session, ~430 more on the turn it fires. The
/// hooks beside it are harness-only and cost the model nothing — so this file
/// is the first thing this app has ever put in an agent's context, and the
/// number is here because a claim of that shape should be checkable.
///
/// **No URL is baked in.** `hooks.json` is a photograph — Claude Code reads it
/// once, so the endpoint had to be stable across restarts for it to keep
/// working. This file is read by the agent at the moment it acts, so it can
/// point at the environment instead, and `$MAROL_NAME_URL` is already this
/// session's own address with its id in it. One variable, used verbatim: no
/// composing a URL under whichever shell the platform has.
///
/// It renames the row on the person's board and nothing else. The name the
/// CLI itself answers to for cross-session messaging was fixed by `--name` at
/// launch and cannot be changed from inside a running session — so this says
/// so, rather than implying an address that would not work.
const NAME_SKILL: &str = r#"---
name: name-this-session
description: Name this session on the Marol board, so the person running several agents at once can tell at a glance which row is this piece of work. Use once you know what the session is actually about — after reading the request, or when the work turns out to be something other than what it was opened for.
---

# Name this session

This session is running in a Marol window, beside other agents. Its row there
carries a name, and by default that name is whatever Marol could tell from the
outside: the card's title, or the folder the terminal opened in. Several
sessions in one directory therefore look alike, which is the problem this
solves.

Set it with one request:

```bash
curl -sS --max-time 2 -X POST "$MAROL_NAME_URL" --data-binary "Fix the login redirect"
```

- `$MAROL_NAME_URL` is already this session's own address. Use it exactly as
  it is; do not build a URL out of its parts.
- The body is the name, as plain text. No JSON, no escaping.
- Short and concrete beats complete: it is read in a narrow sidebar, at a
  glance, next to a dozen others. Say the work, not the repository — the row
  already shows where it is.
- Say it in the language the person is speaking to you in.
- If `$MAROL_NAME_URL` is unset, this session is not wired for it. Do nothing
  and do not mention it.

Naming again replaces the name; do it when the work changes, not on a timer.
The rename lands on the person's board immediately. It does not change the
name the CLI answers to for messages from other sessions, which was fixed when
this session started.
"#;

/// How a session reaches the others on this desk.
///
/// The second thing in the plugin that is not a hook, and it exists because
/// Claude Code's own cross-session messaging cannot answer the question this
/// app actually has. That feature is per machine — a socket under `/tmp` and
/// a registry in `~/.claude` — while a Marol desk routinely spans a WSL
/// distro and an SSH host, whose filesystems share neither. And it is Claude
/// Code's, so a Codex session can neither be addressed by it nor use it.
///
/// This channel is the desk's own, so it crosses those boundaries the same
/// way status reports already do, and either measured CLI can be on either
/// end of it. Delivery into a session is a paste into its terminal, which is
/// exactly what a person's own follow-up is — so nothing new had to be taught
/// about how a message *arrives*, only about how one is *sent*.
///
/// **Addressed by id, not by name.** A name is a person's sentence and may
/// hold a quote, a space, a newline; an id is a uuid. That single choice is
/// why nothing in here needs escaping, percent-encoding, or JSON — the same
/// property that makes `$MAROL_NAME_URL` safe to hand to a shell.
///
/// **Token cost is not measured yet.** `NAME_SKILL` carries a measured
/// figure because a claim of that shape should be checkable; this one has not
/// been put through `claude plugin details` on a real CLI, and saying so is
/// better than inventing a number beside a measured one.
const PEERS_SKILL: &str = r#"---
name: message-another-session
description: Send a message to another agent session running beside this one on the same Marol desk, and list which sessions those are. Use when work here depends on, blocks, or duplicates work another session is doing — not to chat.
---

# Message another session

This session runs in a Marol window beside other agents, some of which may be
a different CLI entirely. Each has a row on the person's board, an id, and a
terminal of its own.

## Who else is here

```bash
curl -sS --max-time 3 "$MAROL_PEERS_URL"
```

One session per line, tab separated: `id`, name, status. The id is the
address; the name is what the person calls it.

## Send one a message

```bash
curl -sS --max-time 3 -X POST "$MAROL_SEND_URL"   -H "X-Marol-To: <the id from the list>"   --data-binary "The auth fix landed on branch fix-login; rebase before you touch session.py."
```

- Use `$MAROL_PEERS_URL` and `$MAROL_SEND_URL` exactly as they are. Do not
  build a URL out of their parts; each already carries this session's identity.
- The body is the message, as plain text. No JSON, no escaping.
- The reply is `sent`, or a plain-text reason it was not: a wrong id, a
  session whose terminal has gone, a queue that is full.
- If either variable is unset, this session is not wired for it. Do nothing
  and do not mention it.

## What actually happens to it

The message is put in that session's queue and delivered when its current
turn ends — or straight away if it is not mid-turn. It arrives in its
terminal marked as coming from you and explicitly not from the person, so it
will be read as information from a peer rather than as an instruction from
whoever is at the keyboard.

That is the line to keep in mind when writing one: another agent cannot
approve anything on the person's behalf, and neither can you. Send facts,
findings and warnings. Anything that needs a human decision still needs one.

Say who you are and what you want in the first sentence — it is read cold, in
the middle of somebody else's work.
"#;

/// Write (or refresh) the plugin so an app upgrade updates the hooks too.
///
/// The URL is baked in here rather than read at hook time because most of
/// these are `http` hooks, whose `url` is a literal string with no shell
/// behind it to resolve anything. That is why `start` goes to the trouble of
/// keeping the same URL across runs: for a session that is already running,
/// this file is a photograph, not a pointer.
fn write_plugin(dir: &Path, url: &str) -> Result<()> {
    for (rel, contents) in plugin_files(url) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const URL: &str = "http://127.0.0.1:1234/h/tok";

    fn all_hooks() -> Vec<serde_json::Value> {
        let hooks = hooks_json(URL);
        let mut out = Vec::new();
        for (_event, entries) in hooks["hooks"].as_object().unwrap() {
            for entry in entries.as_array().unwrap() {
                for hook in entry["hooks"].as_array().unwrap() {
                    out.push(hook.clone());
                }
            }
        }
        out
    }

    /// The remembered token is the path segment of the route, so a file that
    /// has been hand-edited, half-written, or filled with someone else's idea
    /// of a good time must not be able to add a segment of its own. A rejected
    /// token costs one run's worth of held sessions; an accepted bad one
    /// changes what the server is listening for.
    #[test]
    fn a_remembered_token_that_is_not_a_token_is_refused() {
        let dir = std::env::temp_dir().join(format!("marol-hooks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        for bad in [
            "9000\n../../h/other\n",     // a second path segment
            "9000\nabc def\n",           // a space, which would split the request line
            "9000\n\n",                  // empty
            "9000\ntok?state=idle\n",    // a query of its own
        ] {
            std::fs::write(dir.join(ENDPOINT_FILE), bad).unwrap();
            let (_, token) = remembered(&dir);
            assert!(token.is_none(), "accepted {bad:?} as a token");
        }

        // The two halves are independent, and only one of them is a secret.
        // An unreadable port with a good token keeps the token and takes a
        // fresh port: that loses the sessions the last run held, which is a
        // cost, where reusing a token nobody can vouch for is a hazard.
        std::fs::write(dir.join(ENDPOINT_FILE), "not-a-port\ncafef00d\n").unwrap();
        assert_eq!(remembered(&dir), (None, Some("cafef00d".to_string())));

        // And the round trip it is actually for.
        remember(&dir, 41234, "cafef00d").unwrap();
        assert_eq!(remembered(&dir), (Some(41234), Some("cafef00d".to_string())));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(ENDPOINT_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "the token is readable by other users");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_start_uses_a_command_hook_because_http_never_fires_on_it() {
        let hooks = hooks_json(URL);
        let start = &hooks["hooks"]["SessionStart"][0]["hooks"][0];
        assert_eq!(start["type"], "command", "measured: http on SessionStart is silently dropped");
        // Every other event is cheaper as http.
        for event in ["PreToolUse", "Stop", "Notification", "SessionEnd"] {
            let first = &hooks["hooks"][event][0]["hooks"][0];
            assert_eq!(first["type"], "http", "{event} should not spawn a process per call");
        }
    }

    #[test]
    fn http_hooks_carry_the_session_id_and_allow_its_expansion() {
        for hook in all_hooks().iter().filter(|h| h["type"] == "http") {
            assert_eq!(hook["headers"]["X-Marol-Session"], "$MAROL_SESSION_ID");
            // Without the allowlist the header is sent literally and every
            // report lands on a session that does not exist.
            let allowed = hook["allowedEnvVars"].as_array().unwrap();
            assert!(allowed.iter().any(|v| v == "MAROL_SESSION_ID"));
        }
    }

    #[test]
    fn command_hooks_cannot_block_the_agent() {
        for hook in all_hooks().iter().filter(|h| h["type"] == "command") {
            let cmd = hook["command"].as_str().unwrap();
            assert!(cmd.ends_with("|| true"), "a non-zero exit blocks the action: {cmd}");
            assert!(cmd.contains("--max-time"), "unbounded curl: {cmd}");
            match hook.get("shell").and_then(|v| v.as_str()) {
                None | Some("bash") => {}
                Some(other) => panic!("`shell: {other}` is not known to fire"),
            }
        }
    }

    #[test]
    fn every_state_the_plugin_emits_is_one_the_server_understands() {
        for hook in all_hooks() {
            let text = hook["url"]
                .as_str()
                .map(String::from)
                .or_else(|| hook["command"].as_str().map(String::from))
                .unwrap();
            let state = text
                .split("state=")
                .nth(1)
                .and_then(|s| s.split(['&', '"', ' ']).next())
                .expect("carries a state");
            assert!(
                HookState::parse(state).is_some(),
                "plugin emits `{state}`, which the server would drop"
            );
        }
    }

    #[test]
    fn activity_names_the_argument_a_human_would_use() {
        let bash = activity_from_payload(&json!({
            "tool_name": "Bash",
            "tool_input": { "command": "pytest tests/test_auth.py -v", "description": "Run tests" }
        }))
        .unwrap();
        assert_eq!(bash.tool, "Bash");
        // The command itself, not the model's prose description of it.
        assert_eq!(bash.detail, "pytest tests/test_auth.py -v");

        let edit = activity_from_payload(&json!({
            "tool_name": "Edit",
            "tool_input": { "file_path": "/repo/src/auth.py", "old_string": "a", "new_string": "b" }
        }))
        .unwrap();
        assert_eq!(edit.detail, "/repo/src/auth.py");

        let grep = activity_from_payload(&json!({
            "tool_name": "Grep", "tool_input": { "pattern": "TODO" }
        }))
        .unwrap();
        assert_eq!(grep.detail, "TODO");
    }

    /// A cross-session message is the one tool call whose interesting
    /// argument is two arguments: whom, and what. The timeline showing
    /// 「→ 修好登入 #1: schema 改了」 is how coordination between cards
    /// stays legible without opening either terminal.
    #[test]
    fn a_message_to_another_session_names_the_receiver_and_the_gist() {
        let a = activity_from_payload(&json!({
            "tool_name": "SendMessage",
            "tool_input": { "to": "修好登入 #1", "message": "schema 改了，tenant_id 上了 main",
                            "summary": "schema 改了" }
        }))
        .unwrap();
        assert_eq!(a.tool, "SendMessage");
        assert_eq!(a.detail, "→ 修好登入 #1: schema 改了");

        // Without a summary the message itself is the gist.
        let b = activity_from_payload(&json!({
            "tool_name": "SendMessage",
            "tool_input": { "to": "payments", "message": "migration finished" }
        }))
        .unwrap();
        assert_eq!(b.detail, "→ payments: migration finished");
    }

    #[test]
    fn a_tool_with_no_recognizable_argument_still_reports_its_name() {
        let a = activity_from_payload(&json!({ "tool_name": "TodoWrite", "tool_input": {} })).unwrap();
        assert_eq!(a.tool, "TodoWrite");
        assert!(a.detail.is_empty());
    }

    #[test]
    fn a_payload_that_is_not_a_tool_call_yields_no_activity() {
        // Stop and Notification bodies have no tool_name; they must not
        // overwrite the last real activity with an empty one.
        assert!(activity_from_payload(&json!({ "hook_event_name": "Stop" })).is_none());
    }

    #[test]
    fn oversized_detail_is_truncated_rather_than_carried_whole() {
        let long = "x".repeat(5000);
        let a = activity_from_payload(&json!({
            "tool_name": "Bash", "tool_input": { "command": long }
        }))
        .unwrap();
        assert_eq!(a.detail.chars().count(), 160);
    }

    /* ----------------------------- naming ---------------------------- */

    #[derive(Default)]
    struct Heard {
        names: std::sync::Mutex<Vec<(String, String)>>,
        states: std::sync::Mutex<Vec<HookState>>,
    }

    impl HookHandler for Heard {
        fn on_hook(&self, r: HookReport) {
            self.states.lock().unwrap().push(r.state);
        }
        fn on_name(&self, r: NameReport) {
            self.names.lock().unwrap().push((r.session_id, r.name));
        }
    }

    /// One POST, spelled by hand so the test proves what is on the wire
    /// rather than what a client library thinks it means.
    async fn post(url: &str, body: &str) {
        let rest = url.strip_prefix("http://").unwrap();
        let (authority, target) = rest.split_once('/').unwrap();
        let mut s = tokio::net::TcpStream::connect(authority).await.unwrap();
        let req = format!(
            "POST /{target} HTTP/1.1\r\nhost: {authority}\r\n\
             content-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        let _ = s.read_to_end(&mut buf).await;
    }

    /// The whole naming path, on a real listener.
    ///
    /// It shares a route with the status reports on purpose — the token, the
    /// remembered port, the WSL mount and the SSH tunnel all know that one
    /// URL already — so the thing worth proving is that the two kinds of
    /// message stay told apart, and that everything which could put a name on
    /// the wrong row drops it instead.
    #[test]
    fn a_session_names_itself_over_the_endpoint_the_hooks_already_use() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let dir = std::env::temp_dir().join(format!("marol-naming-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let heard = Arc::new(Heard::default());
            let server = start(&dir, Arc::clone(&heard) as Arc<dyn HookHandler>)
                .await
                .unwrap();
            let base = server.url();

            // The ordinary case: the variable the session was handed, used
            // verbatim, with the name as the body. `curl --data-binary` sends
            // no trailing newline; a person's `echo` would, and either is a
            // perfectly good name.
            post(&name_url(&base, "sess-1"), "Fix the login redirect\n").await;
            post(&name_url(&base, "sess-2"), "改登入導向").await;

            // Nothing to say is not a rename. A row whose name had been
            // blanked would be a row you could no longer pick out at all.
            post(&name_url(&base, "sess-3"), "   ").await;

            // A URL the shell never expanded — `cmd.exe` leaves the literal
            // text — would file the name under a session that cannot exist.
            post(&name_url(&base, "$MAROL_SESSION_ID"), "nowhere").await;

            // No id at all: there is no working directory to fall back on
            // here, and guessing would rename somebody else's card.
            post(&format!("{base}?set=name"), "unaddressed").await;

            // Someone else's token is someone else's business.
            let forged = base.replace("/h/", "/h/x") + "?sid=sess-9&set=name";
            post(&forged, "forged").await;

            // And the status route is untouched by any of it.
            post(&format!("{base}?sid=sess-1&state=idle"), "{}").await;

            server.stop();
            let names = heard.names.lock().unwrap().clone();
            assert_eq!(
                names,
                vec![
                    ("sess-1".to_string(), "Fix the login redirect".to_string()),
                    ("sess-2".to_string(), "改登入導向".to_string()),
                ],
                "a name landed somewhere it should not have, or one was lost"
            );
            assert_eq!(*heard.states.lock().unwrap(), vec![HookState::Idle]);
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// The address is one variable, not a recipe.
    ///
    /// It carries the session's own id because the alternative is asking an
    /// agent to compose a URL under whichever shell the platform handed it —
    /// which is the exact failure `expanded` exists to catch on the hook
    /// side, and there is no need to invite it twice.
    #[test]
    fn the_name_url_is_one_sessions_whole_address() {
        assert_eq!(
            name_url(URL, "abc-123"),
            "http://127.0.0.1:1234/h/tok?sid=abc-123&set=name"
        );
    }

    /// The plugin's one non-hook file, and the two things about it that are
    /// load-bearing: it reads the endpoint out of the environment at the
    /// moment it acts (unlike `hooks.json`, which is a photograph), and it
    /// tells the agent to use that variable whole.
    #[test]
    fn the_naming_skill_points_at_the_environment_rather_than_a_baked_url() {
        let files = plugin_files(URL);
        let (_, skill) = files
            .iter()
            .find(|(rel, _)| *rel == "skills/name-this-session/SKILL.md")
            .expect("the plugin ships the naming skill");
        assert!(skill.starts_with("---\nname: name-this-session\n"));
        assert!(skill.contains("description:"), "a skill with no description is never reached");
        assert!(skill.contains("$MAROL_NAME_URL"));
        assert!(
            !skill.contains(URL) && !skill.contains("127.0.0.1"),
            "a URL baked into the skill would go stale the way hooks.json cannot afford to"
        );
    }

    /* ----------------------------- codex ----------------------------- */

    /// Every state Codex is configured to emit is one the listener parses,
    /// and every event reports one. The Claude Code half has had this test
    /// since the plugin existed; the failure it catches — a typo in a state
    /// name, silently dropped at the door — is identical here.
    #[test]
    fn every_state_codex_emits_is_one_the_server_understands() {
        let args = codex_config_args(URL);
        assert_eq!(args.len(), CODEX_EVENTS.len() * 2, "one -c per event");
        for pair in args.chunks(2) {
            assert_eq!(pair[0], "-c");
            let state = pair[1]
                .split("state=")
                .nth(1)
                .and_then(|s| s.split(['&', '"']).next())
                .expect("carries a state");
            assert!(
                HookState::parse(state).is_some(),
                "codex would emit `{state}`, which the server would drop"
            );
        }
    }

    /// The four properties of the one-liner, checked on the text that
    /// actually ships rather than on the format string that made it.
    #[test]
    fn a_codex_hook_forwards_its_payload_exits_zero_and_is_bounded() {
        for pair in codex_config_args(URL).chunks(2) {
            let value = &pair[1];
            assert!(
                value.contains("--data-binary @-"),
                "the payload never reaches the body: {value}"
            );
            assert!(
                value.contains("|| exit 0"),
                "a non-zero exit is a failed hook in front of the person: {value}"
            );
            assert!(value.contains("--max-time"), "unbounded curl: {value}");
            // A single quote would end the TOML literal string the command
            // lives in, and everything after it would be parsed as TOML.
            let command = value
                .split_once("command='")
                .and_then(|(_, rest)| rest.split_once('\''))
                .map(|(cmd, _)| cmd)
                .expect("the command is a TOML literal string");
            assert!(!command.contains('\''), "{command}");
            assert!(command.contains(URL), "{command}");
        }
    }

    /// Codex's default hook timeout is ten minutes. A status ping that can
    /// hold a tool call for ten minutes is worse than no status at all, and
    /// `SessionEnd` is capped at three seconds by Codex itself — asking for
    /// more there is asking to be quietly overruled.
    #[test]
    fn no_codex_hook_can_hold_a_session_for_longer_than_a_breath() {
        for (event, _, _, timeout) in CODEX_EVENTS {
            assert!(timeout <= 5, "{event} may hold a tool call for {timeout}s");
            if event == "SessionEnd" {
                assert!(timeout <= 3, "codex caps SessionEnd at 3s");
            }
        }
        // And curl always gives up before the hook's own budget runs out, so
        // the failure a person sees is nothing rather than a hook error.
        for pair in codex_config_args(URL).chunks(2) {
            let value = &pair[1];
            let max_time: u32 = value
                .split("--max-time ")
                .nth(1)
                .and_then(|s| s.split(' ').next())
                .and_then(|s| s.parse().ok())
                .expect("a bounded curl");
            let timeout: u32 = value
                .rsplit("timeout=")
                .next()
                .and_then(|s| s.split(['}', ',']).next())
                .and_then(|s| s.parse().ok())
                .expect("a hook timeout");
            assert!(max_time < timeout, "{value}");
        }
    }

    /// The identity carried by every launch is the *name* of the variable,
    /// not its value. Codex records hook trust against the hook's hash, so
    /// baking a session id in would be a fresh hook — and a fresh review —
    /// for every attempt anybody ever starts.
    #[test]
    fn the_codex_hook_definition_is_the_same_text_for_every_session() {
        let a = codex_config_args(URL);
        let b = codex_config_args(URL);
        assert_eq!(a, b);
        assert!(
            a.iter().any(|v| v.contains("$MAROL_SESSION_ID")),
            "the session id is baked in rather than expanded: {a:?}"
        );
    }

    /// The one hook that also *tells* the session something, checked by
    /// running it.
    ///
    /// Three layers of quoting stand between the sentence and the model —
    /// a Rust literal, a TOML literal string, and a shell double-quoted word
    /// — and the JSON inside has quoting of its own. Nothing short of
    /// executing it proves they compose, so this extracts the command Codex
    /// would run, runs it, and reads what Codex would read.
    #[cfg(unix)]
    #[test]
    fn the_session_start_hook_hands_codex_a_context_it_can_actually_parse() {
        let args = codex_config_args(URL);
        let value = args
            .chunks(2)
            .map(|p| p[1].clone())
            .find(|v| v.starts_with("hooks.SessionStart="))
            .expect("a SessionStart hook");

        // The command sits in a TOML *literal* string, which ends at the
        // first apostrophe — so one anywhere in it would truncate the value
        // and Codex would keep the remains as a literal string it never runs.
        let start = value.find("command='").expect("a command") + "command='".len();
        let end = start + value[start..].find("',").expect("the command ends");
        let command = &value[start..end];
        assert!(!command.contains('\''), "an apostrophe would end the TOML string: {command}");

        // Run it the way Codex does. The report goes to a port nothing is
        // listening on, so curl fails fast — or is absent entirely, which
        // `|| exit 0` swallows. Either way the context is already on stdout,
        // which is why the printf goes first.
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .expect("running the hook");
        assert!(out.status.success(), "a hook must never exit non-zero");

        let parsed: serde_json::Value =
            serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
                panic!("Codex could not parse what the hook printed: {e}\n{}",
                       String::from_utf8_lossy(&out.stdout))
            });
        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "SessionStart");
        let context = parsed["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("an additionalContext string");

        // It names the variables rather than carrying their values. A shell
        // that expanded them would put this session's token into the model's
        // context and into the transcript on disk.
        assert!(context.contains("$MAROL_PEERS_URL"), "{context}");
        assert!(context.contains("$MAROL_SEND_URL"), "{context}");
        assert!(!context.contains("&tok="), "a token reached the context: {context}");
        assert!(!context.contains(URL), "the listener URL reached the context: {context}");
        // And it carries the one clause that keeps a peer from borrowing the
        // person's authority, the same one `peer_envelope` carries.
        assert!(context.contains("not from the person"), "{context}");
    }

    /// A shell that does not spell variables with `$` hands the listener the
    /// name instead of the id. Filing the report under that name would put
    /// it on a session that cannot exist; the working directory is the way
    /// home, and it is in every payload.
    #[test]
    fn an_unexpanded_session_id_is_refused_rather_than_believed() {
        assert!(expanded("6f1c9a2e4b7d4f0a"));
        assert!(!expanded(""));
        assert!(!expanded("$MAROL_SESSION_ID"));
        assert!(!expanded("%MAROL_SESSION_ID%"));
    }

    #[test]
    fn writes_a_plugin_claude_code_can_load() {
        let dir = std::env::temp_dir().join(format!("marol-plugin-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_plugin(&dir, URL).unwrap();

        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(".claude-plugin/plugin.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["name"], "marol-status");

        let hooks: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("hooks/hooks.json")).unwrap())
                .unwrap();
        assert!(hooks["hooks"]["Stop"].is_array());
        // The listener port changes every run, so the URL must be baked in.
        assert!(hooks["hooks"]["Stop"][0]["hooks"][0]["url"]
            .as_str()
            .unwrap()
            .starts_with(URL));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
