//! PTY-hosted agent sessions.
//!
//! A session is a real pseudo-terminal running the real `claude` binary, so
//! the pane shows exactly what a terminal shows: the same TUI chrome, the same
//! slash-command menu, the same permission prompts. Nothing is re-rendered or
//! reinterpreted on the way.
//!
//! Two details make it behave like a terminal rather than a pipe:
//!
//!   * `claude` detects a tty and runs its full interactive UI. Spawned with
//!     plain pipes it would fall back to non-interactive mode.
//!   * The environment comes from the user's login shell (`shell_env`), not
//!     from this GUI process, so version-manager shims and MCP servers resolve
//!     the same way they do in Terminal.app.

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use crate::shell_env::ShellEnv;

/// Output is forwarded in chunks rather than per byte; a redraw-heavy TUI can
/// emit thousands of small writes and one IPC message each would swamp the
/// webview.
const READ_BUF: usize = 8 * 1024;

/// tmux, used for exactly one thing: holding the agent's process after this
/// app exits.
///
/// **One server per session, never a shared one.** A tmux session inherits
/// the environment of the *server*, not of the client that asked for it, so
/// on a shared server every session after the first would be handed the
/// first one's `MAROL_SESSION_ID` — measured, not feared: the second
/// session read back the first session's id. Cards would light up for the
/// wrong agent. A socket per session makes the environment right by
/// construction rather than by hope, and costs one idle process each.
///
/// tmux also never gets to draw. The config this desk writes turns the
/// status line off and unbinds every key, so it cannot repaint a cell or
/// swallow a keystroke — a byte tmux drew would be a byte the TUI did not,
/// and that is the one promise this app makes.
#[derive(Debug, Clone)]
pub struct Hold {
    /// The command that ends this session for good, already composed for the
    /// world it runs in: `tmux -L … kill-server` on this machine, the same
    /// thing behind a doorway anywhere else.
    ///
    /// Composed by the core rather than here. Which world a session lives in
    /// is the core's knowledge — `pty` opens a terminal onto a command and
    /// has never known whose machine that command lands on — and a hold that
    /// built its own tmux line could only ever build a local one.
    pub destroy: (String, Vec<String>),
    /// The socket's file, when it is on *this* machine. tmux leaves the inode
    /// behind when a server exits, so closing a session would otherwise leave
    /// a dead file that every later sweep has to look at. `None` for a socket
    /// in another world, whose filesystem this process cannot reach: the
    /// destroy command unlinks it there.
    pub socket_file: Option<String>,
}

/// The single session inside each socket. There is only ever one, so the
/// name carries no information and exists because tmux wants one.
pub const HOLD_SESSION: &str = "agent";

/// What `-f` points at. Written every app start.
/// The two keys this config does bind, and the reason `unbind-key -a` is not
/// the last line any more.
///
/// A wheel notch over a held pane used to become a cursor key, on the theory
/// that the alternate screen has no scrollback to move. That theory read the
/// wrong program's state: it is `tmux` that is on the alternate screen, from
/// the moment it attaches, and what the program *inside* it is doing is a
/// separate question. When that program draws inline — which Codex does, its
/// alternate screen being reserved for overlays — the conversation is in
/// `tmux`'s own scrollback, an Up key cannot reach it, and the composer takes
/// the key as a walk through prompt history instead. Which is what it looked
/// like: scrolling the wheel rewrote the prompt.
///
/// `#{alternate_on}` is the question actually worth asking, and only `tmux`
/// can answer it, because only `tmux` can see the inner program's screen
/// state. So the branch lives here rather than in the app: an inline pane
/// scrolls `tmux`'s history, a full-screen one gets the cursor key it always
/// got. `copy-mode -e` exits itself at the bottom, so scrolling back down
/// returns to the prompt with nothing to dismiss.
///
/// `set -g mouse on` would have let `tmux` do this from its own default
/// binding, and was refused twice for two different reasons. It costs
/// text selection, which `tmux` would take over from the terminal; and it
/// hands the wheel to xterm.js's mouse-report path, which damps deltas under
/// 50px by 0.3 and then sends one report however many lines it just computed
/// — the two defects Marol's own wheel arithmetic exists to correct.
pub const HOLD_CONF: &str = "\
set -g status off
set -g escape-time 0
set -g mouse off
set -g default-terminal \"screen-256color\"
set -ga terminal-overrides \",*:Tc\"
set -g destroy-unattached off
unbind-key -a
bind-key -T root M-PPage if-shell -F '#{alternate_on}' 'send-keys Up' 'copy-mode -e ; send-keys -X scroll-up'
bind-key -T root M-NPage if-shell -F '#{alternate_on}' 'send-keys Down' 'if-shell -F \"#{pane_in_mode}\" \"send-keys -X scroll-down\" \"\"'
";

/// Which socket, and how tmux is told about it.
///
/// Two shapes because the question "where does tmux keep its sockets" has an
/// answer on this machine and no answer anywhere else. `-L` lets tmux decide,
/// which is right here: the directory is `$TMUX_TMPDIR/tmux-<uid>` and this
/// process can read both halves. In another world both depend on a uid and a
/// profile this side cannot see, and a sweep that guessed wrong would look
/// into an empty directory and call every live agent gone. So there, the app
/// names the path and tmux is told it.
///
/// Local stays `-L` rather than being unified onto `-S`: it works, it is
/// tested, and moving it would strand every session a previous version left
/// held — their sockets would still be there, still running, under a name
/// nothing looks for any more.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Socket {
    /// `-L <name>`, tmux's own directory. This machine only.
    Named(String),
    /// `-S <path>`, chosen by the app. Any world whose socket directory this
    /// process cannot see.
    Path(String),
}

impl Socket {
    /// The two arguments that name it, whichever shape it is.
    pub fn args(&self) -> [String; 2] {
        match self {
            Socket::Named(n) => ["-L".to_string(), n.clone()],
            Socket::Path(p) => ["-S".to_string(), p.clone()],
        }
    }
}

/// The command that starts a held session, or reattaches to the one already
/// running under this socket.
///
/// `new-session -A` is create-or-attach in one call, so a restart that finds
/// its session alive takes the same path as the start that made it: one code
/// path, and no window in which the two could disagree. `-D` detaches any
/// other client, because two attached clients would fight over the pty's
/// size.
///
/// Returned as a plain (program, args) pair so the caller can put it through
/// a doorway. That is the whole reason it lives here rather than inside
/// `spawn`: a tmux line built at spawn time is a tmux line on this machine,
/// and the worlds that need holding most are the other ones.
pub fn hold_attach(
    socket: &Socket,
    conf: &str,
    cwd: Option<&str>,
    program: &str,
    args: &[String],
) -> (String, Vec<String>) {
    let mut a = socket.args().to_vec();
    a.extend([
        "-f".to_string(),
        conf.to_string(),
        "new-session".to_string(),
        "-A".to_string(),
        "-D".to_string(),
        "-s".to_string(),
        HOLD_SESSION.to_string(),
    ]);
    if let Some(dir) = cwd {
        a.push("-c".to_string());
        a.push(dir.to_string());
    }
    a.push("--".to_string());
    a.push(program.to_string());
    a.extend(args.iter().cloned());
    ("tmux".to_string(), a)
}

/// The command that ends one for good.
///
/// `kill-server`, not `kill-session`: this socket holds exactly one session,
/// so ending it should not leave a server behind waiting for a session that
/// will never come.
///
/// A socket in another world is unlinked in the same breath, because there is
/// no second visit: this process cannot reach that filesystem, and anything
/// coming back later would first have to tell a dead socket from a live one,
/// which is precisely what tmux leaving the inode behind makes hard. `;`
/// rather than `&&` — tmux exits non-zero when the server has already gone,
/// and that is the case where the file most needs removing.
pub fn hold_destroy(socket: &Socket) -> (String, Vec<String>) {
    match socket {
        Socket::Named(_) => {
            let mut a = socket.args().to_vec();
            a.push("kill-server".to_string());
            ("tmux".to_string(), a)
        }
        // The path travels as an *argument*, never spliced into the script —
        // the same rule `write_file` keeps, and the reason neither has to
        // think about what a directory name might contain.
        Socket::Path(p) => (
            "sh".to_string(),
            vec![
                "-c".to_string(),
                r#"tmux -S "$1" kill-server; rm -f "$1""#.to_string(),
                "_".to_string(),
                p.clone(),
            ],
        ),
    }
}

/// Is anything answering on this socket? The one question the startup check
/// and the orphan sweep both ask, in the form both can run.
pub fn hold_alive(socket: &Socket) -> (String, Vec<String>) {
    let mut a = socket.args().to_vec();
    a.extend([
        "has-session".to_string(),
        "-t".to_string(),
        HOLD_SESSION.to_string(),
    ]);
    ("tmux".to_string(), a)
}

/// The socket name for one session of one desk.
///
/// `desk` distinguishes installs sharing a machine — without it, one desk's
/// orphan sweep would happily kill another's live agents, and the tests
/// would do it to each other.
pub fn hold_socket(desk: &str, session_id: &str) -> String {
    format!("marol-{desk}-{session_id}")
}

/// The same socket, under the name this app used before it was called Marol.
///
/// Local sockets live in tmux's own shared directory, so their name has to
/// say whose they are — which means the app's rename reaches them, unlike the
/// remote ones, which sit in a directory of ours and are named for the desk
/// alone.
///
/// This exists because a held agent is the one thing a rename cannot simply
/// leave behind. Its server is answering on the old name; a `new-session -A`
/// against the new one would not find it and would cheerfully start a *second*
/// agent on the same worktree — the precise accident holding sessions was
/// built to prevent, arranged by us.
pub fn hold_socket_former(desk: &str, session_id: &str) -> String {
    format!("agentdesk-{desk}-{session_id}")
}

/// What a `sockaddr_un` has room for, minus a byte for the terminator.
///
/// 104 on macOS, 108 on Linux; the smaller is the one that has to hold. This
/// is not a stylistic budget — go past it and `tmux -S` fails to start the
/// session at all, with a message that goes to a pty which then closes.
pub const SOCKET_PATH_LIMIT: usize = 104;

/// The socket *file* for one session of one desk, in a world whose tmux
/// directory this process cannot see.
///
/// Shorter than the local name, and not out of taste: see `SOCKET_PATH_LIMIT`.
/// A 36-character session id already spends a third of it. The `marol-`
/// the local name wears is what keeps it apart from the person's own sockets
/// in tmux's shared directory; this directory belongs to us, so the prefix
/// would be ten bytes of saying so twice.
pub fn hold_socket_path(dir: &str, desk: &str, session_id: &str) -> String {
    format!(
        "{}/{}",
        dir.trim_end_matches('/'),
        hold_socket_name(desk, session_id)
    )
}

/// Just the file's name, for the sweep — which reads a directory listing and
/// has to recognise its own. The same function as the path builds from, so the
/// two cannot drift into naming one thing and looking for another.
pub fn hold_socket_name(desk: &str, session_id: &str) -> String {
    format!("{desk}-{session_id}")
}

/// The prefix every socket of one desk shares, in each of the two worlds.
/// The sweep matches on it, so it lives beside the names it has to match.
pub fn hold_prefix(desk: &str, remote: bool) -> String {
    if remote {
        format!("{desk}-")
    } else {
        format!("marol-{desk}-")
    }
}

/// Both prefixes a local socket of this desk may wear. The sweep reads a
/// directory listing, and a leftover under the old name is exactly the kind
/// of thing a sweep is for — it would otherwise be an agent nobody has a card
/// for and nothing left alive can name.
pub fn hold_prefixes(desk: &str) -> [String; 2] {
    [hold_prefix(desk, false), format!("agentdesk-{desk}-")]
}

/// A short, stable tag for a desk, from wherever it keeps its data.
///
/// FNV-1a because it needs to be stable across runs and short enough to read
/// in `tmux -L`, not because anything here is a secret.
pub fn desk_tag(data_dir: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data_dir.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{:08x}", h as u32)
}

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    /// How to end the tmux server holding this session, when one is. Resolved
    /// at spawn so the close path needs no environment of its own.
    destroy: Option<(std::path::PathBuf, Vec<String>, Option<String>)>,
    /// Everything the terminal has emitted, bounded.
    ///
    /// A PTY starts producing the moment it is spawned, but the pane that
    /// displays it only mounts on the next render. Without a replay buffer the
    /// whole first paint — for Claude Code, its entire opening screen — is
    /// emitted to nobody and the pane comes up blank.
    scrollback: Arc<Mutex<Scrollback>>,
}

/// Bounded byte buffer with a monotonic sequence number, so a late-attaching
/// pane can be handed the history *and* know which live chunks it has already
/// been given.
#[derive(Default)]
pub struct Scrollback {
    bytes: Vec<u8>,
    /// Sequence of the most recent chunk appended.
    pub seq: u64,
}

/// Roughly a few full screens of a redraw-heavy TUI.
const SCROLLBACK_LIMIT: usize = 512 * 1024;

impl Scrollback {
    fn append(&mut self, chunk: &[u8]) -> u64 {
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() > SCROLLBACK_LIMIT {
            // Drop from the front. A TUI repaints from escape sequences, so a
            // truncated prefix costs history, never correctness of the frame.
            let excess = self.bytes.len() - SCROLLBACK_LIMIT;
            self.bytes.drain(0..excess);
        }
        self.seq += 1;
        self.seq
    }

    fn snapshot(&self) -> (String, u64) {
        (BASE64.encode(&self.bytes), self.seq)
    }
}

impl PtySession {
    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow!("resize failed: {e}"))
    }

    /// End the client. When tmux is holding the process this only detaches:
    /// the session has `destroy-unattached off`, so the agent keeps running.
    /// That is exactly what quitting the app should do.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }

    /// End the session for good, tmux's server included.
    ///
    /// The deliberate close, as opposed to quitting the app. Without this the
    /// two would be indistinguishable from here — the client dies either way,
    /// and only one of them means "I am finished with this work".
    pub fn destroy(&mut self) {
        if let Some((tmux, args, socket_file)) = self.destroy.clone() {
            let _ = std::process::Command::new(tmux).args(args).output();
            // The server exits; its socket inode does not. Closing a session
            // should leave nothing at all behind.
            if let Some(f) = socket_file {
                let _ = std::fs::remove_file(f);
            }
        }
        let _ = self.child.kill();
    }
}

/// What a spawned session reports back.
pub trait PtySink: Send + Sync + 'static {
    /// A chunk of terminal output, base64-encoded, with its sequence number.
    ///
    /// Bytes, not text. A read boundary lands wherever the kernel put it, so
    /// decoding each chunk as UTF-8 here would replace any multi-byte
    /// character that straddles the boundary with U+FFFD — and a TUI is full
    /// of 3-byte box-drawing characters, so the frame would visibly break
    /// apart. Passing bytes through lets the terminal emulator's own
    /// stateful decoder stitch the boundary back together.
    fn on_output(&self, id: &str, data: String, seq: u64);
    /// The process exited with this status string.
    fn on_exit(&self, id: &str, status: String);
}

#[derive(Default)]
pub struct PtyRegistry {
    sessions: Mutex<HashMap<String, PtySession>>,
}

impl PtyRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Launch `program` under a PTY in `cwd`.
    ///
    /// `program` is resolved against the login-shell PATH, so `claude`,
    /// `codex` or any other agent CLI is found the same way the user's shell
    /// finds it.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        &self,
        id: &str,
        program: &str,
        args: &[String],
        // `None` when the real working directory only exists inside a host
        // (a WSL distro): the outer process runs from wherever the app is,
        // and the wrapping carries the true cwd across.
        cwd: Option<&str>,
        env: &ShellEnv,
        // Per-session variables layered on top of the shell environment — how
        // a status hook learns which session it is reporting for.
        extra_env: &[(String, String)],
        cols: u16,
        rows: u16,
        sink: Arc<dyn PtySink>,
        // When set, the command above is already a tmux line (see
        // `hold_attach`) and this carries what it takes to end it. `None`
        // runs the command as a direct child, the way every world without
        // tmux still does.
        hold: Option<&Hold>,
    ) -> Result<()> {
        if self.sessions.lock().unwrap().contains_key(id) {
            return Err(anyhow!("session {id} already has a terminal"));
        }

        let exe = env
            .which(program)
            .ok_or_else(|| anyhow!("`{program}` not found on the login-shell PATH"))?;

        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow!("openpty failed: {e}"))?;

        // CreateProcessW runs the resolved path as the process image, and a
        // batch file is not an image — only cmd.exe can host one. npm
        // installs `claude` on Windows as exactly such a shim (claude.cmd),
        // so the shim rides as cmd's argument instead.
        let batch = exe
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
        let mut cmd = if batch {
            let comspec = env
                .vars
                .get("COMSPEC")
                .or_else(|| env.vars.get("ComSpec"))
                .map(String::as_str)
                .unwrap_or("cmd.exe");
            let mut c = CommandBuilder::new(comspec);
            c.arg("/c");
            c.arg(&exe);
            for a in args {
                c.arg(a);
            }
            c
        } else {
            let mut c = CommandBuilder::new(&exe);
            for a in args {
                c.arg(a);
            }
            c
        };
        if let Some(dir) = cwd {
            cmd.cwd(dir);
        }
        for (k, v) in &env.vars {
            cmd.env(k, v);
        }
        // A TUI needs a terminal type it can drive; the login shell's own TERM
        // may be `dumb` when the probe ran non-interactively.
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        // Layered last so per-session values win over the shell's.
        for (k, v) in extra_env {
            cmd.env(k, v);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("spawning {}", exe.display()))?;
        // Dropping the slave lets the master see EOF when the child exits.
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| anyhow!("cloning pty reader failed: {e}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| anyhow!("taking pty writer failed: {e}"))?;

        let session_id = id.to_string();
        let out_sink = Arc::clone(&sink);
        let scrollback = Arc::new(Mutex::new(Scrollback::default()));
        let reader_scrollback = Arc::clone(&scrollback);
        // Blocking reads on a dedicated thread: portable-pty's reader has no
        // async interface, and a TUI stream is effectively continuous.
        std::thread::spawn(move || {
            let mut buf = [0u8; READ_BUF];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        // Record before emitting, so a pane that attaches
                        // between the two sees this chunk in the snapshot
                        // rather than missing it in both.
                        let seq = reader_scrollback.lock().unwrap().append(&buf[..n]);
                        out_sink.on_output(&session_id, BASE64.encode(&buf[..n]), seq);
                    }
                    Err(e) => {
                        eprintln!("[pty] {session_id} read error: {e}");
                        break;
                    }
                }
            }
            out_sink.on_exit(&session_id, "closed".to_string());
        });

        self.sessions.lock().unwrap().insert(
            id.to_string(),
            PtySession {
                master: pair.master,
                writer,
                child,
                // Resolved through the login-shell PATH, like every other
                // program this app runs, rather than left as a bare name for
                // `Command` to find on the process PATH. Those two disagree —
                // a Homebrew tmux is on one and not the other — and a destroy
                // that silently cannot find its tmux would leave the server
                // running with nothing left to attach to it.
                destroy: hold.and_then(|h| {
                    env.which(&h.destroy.0)
                        .map(|exe| (exe, h.destroy.1.clone(), h.socket_file.clone()))
                }),
                scrollback,
            },
        );

        Ok(())
    }

    /// Everything this terminal has produced so far, plus the sequence number
    /// it ends at. A pane subscribes first, calls this, writes the snapshot,
    /// then replays only the live chunks newer than `seq` — so nothing is
    /// dropped and nothing is written twice.
    pub fn snapshot(&self, id: &str) -> Result<(String, u64)> {
        let sessions = self.sessions.lock().unwrap();
        let s = sessions
            .get(id)
            .ok_or_else(|| anyhow!("no terminal for session {id}"))?;
        let sb = s.scrollback.lock().unwrap();
        Ok(sb.snapshot())
    }

    pub fn write(&self, id: &str, data: &str) -> Result<()> {
        let mut sessions = self.sessions.lock().unwrap();
        let s = sessions
            .get_mut(id)
            .ok_or_else(|| anyhow!("no terminal for session {id}"))?;
        s.write(data.as_bytes())
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<()> {
        let sessions = self.sessions.lock().unwrap();
        let s = sessions
            .get(id)
            .ok_or_else(|| anyhow!("no terminal for session {id}"))?;
        s.resize(cols, rows)
    }

    /// Close one session for good — tmux's copy of it included.
    pub fn kill(&self, id: &str) {
        if let Some(mut s) = self.sessions.lock().unwrap().remove(id) {
            s.destroy();
        }
    }

    /// Quitting the app.
    ///
    /// Deliberately `kill`, not `destroy`: a directly-spawned child dies with
    /// its client, which is the old behaviour and still right, while a held
    /// session only loses its viewer. That difference is the whole feature —
    /// quitting is not the same as being finished with the work.
    pub fn kill_all(&self) {
        let mut sessions = self.sessions.lock().unwrap();
        for (_, mut s) in sessions.drain() {
            s.kill();
        }
    }

    pub fn is_live(&self, id: &str) -> bool {
        self.sessions.lock().unwrap().contains_key(id)
    }

    /// Whether a tmux server is holding this session, so quitting the app
    /// detaches it rather than ending it.
    ///
    /// The same field the close path uses, asked as a question: `destroy` is
    /// resolved at spawn and is `Some` exactly when a holder was found. That
    /// makes it the honest answer rather than a second opinion — a world that
    /// reported tmux but whose binary the login shell could not resolve has
    /// no `destroy`, and this says "not held" for it, which is what actually
    /// happens to it on quit.
    pub fn is_held(&self, id: &str) -> bool {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|s| s.destroy.is_some())
    }
}

#[cfg(test)]
mod hold_tests {
    use super::*;

    /// The config is the whole safety argument: tmux holds the process and
    /// draws nothing. A status line or a live key binding would put bytes on
    /// screen the TUI did not write, which is the one thing this app promises
    /// never happens.
    #[test]
    fn the_config_leaves_tmux_nothing_to_draw_and_no_key_to_eat() {
        assert!(HOLD_CONF.contains("set -g status off"));
        assert!(HOLD_CONF.contains("unbind-key -a"));
        // Detaching must not be the same as ending: the app quitting drops
        // the client, and the agent has to survive that.
        assert!(HOLD_CONF.contains("set -g destroy-unattached off"));
        // Truecolor survives the wrapping, or every themed TUI loses its
        // palette the moment a world gains tmux.
        assert!(HOLD_CONF.contains("*:Tc"));
        // The wheel's two keys, and the branch that makes them right. An
        // inline pane scrolls tmux's own history; a full-screen one gets the
        // cursor key it always got, and only tmux can tell them apart.
        assert!(HOLD_CONF.contains("M-PPage"), "the scroll-up key is gone");
        assert!(HOLD_CONF.contains("M-NPage"), "the scroll-down key is gone");
        assert!(HOLD_CONF.contains("alternate_on"), "the branch is gone");
        // Selection stays the terminal's. Turning tmux's mouse on would take
        // it, and would hand the wheel back to the damped mouse-report path.
        assert!(HOLD_CONF.contains("set -g mouse off"));
    }

    /// The config is a string handed to somebody else's parser, so it is
    /// checked against that parser rather than against our idea of it.
    ///
    /// Skipped where tmux is not installed — which is every Windows runner,
    /// and the same reason a world without tmux holds nothing.
    #[test]
    fn tmux_accepts_the_hold_config_and_binds_both_wheel_keys() {
        let Ok(tmux) = which_tmux() else {
            eprintln!("skip: no tmux");
            return;
        };
        let dir = std::env::temp_dir().join(format!("marol-conf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let conf = dir.join("hold.conf");
        std::fs::write(&conf, HOLD_CONF).expect("write conf");
        let sock = format!("marolconf{}", std::process::id());

        let run = |args: &[&str]| {
            std::process::Command::new(&tmux)
                .args(["-L", &sock])
                .args(args)
                .output()
                .expect("tmux")
        };
        let started = std::process::Command::new(&tmux)
            .args(["-L", &sock, "-f"])
            .arg(&conf)
            .args(["new-session", "-d", "-x", "80", "-y", "10", "sleep 20"])
            .output()
            .expect("tmux");
        assert!(
            started.status.success(),
            "tmux refused the config: {}",
            String::from_utf8_lossy(&started.stderr)
        );

        let keys = run(&["list-keys", "-T", "root"]);
        let listed = String::from_utf8_lossy(&keys.stdout).into_owned();
        let _ = run(&["kill-server"]);
        let _ = std::fs::remove_dir_all(&dir);

        // Both bound, and both still carrying the branch: a binding that
        // parsed but lost its condition would scroll the wrong thing.
        for key in ["M-PPage", "M-NPage"] {
            let line = listed
                .lines()
                .find(|l| l.contains(key))
                .unwrap_or_else(|| panic!("tmux did not bind {key}:\n{listed}"));
            assert!(line.contains("alternate_on"), "{key} lost its branch: {line}");
        }
    }

    fn which_tmux() -> Result<String, ()> {
        for dir in std::env::var("PATH").unwrap_or_default().split(':') {
            let p = std::path::Path::new(dir).join("tmux");
            if p.is_file() {
                return Ok(p.to_string_lossy().into_owned());
            }
        }
        Err(())
    }

    /// The socket carries both the desk and the session. Without the desk,
    /// one install's orphan sweep would kill another's live agents — and two
    /// tests running at once would do it to each other.
    #[test]
    fn a_socket_belongs_to_one_desk_and_one_session() {
        let a = hold_socket(&desk_tag("/home/me/.marol"), "s7");
        let b = hold_socket(&desk_tag("/home/me/.marol-beta"), "s7");
        assert_ne!(a, b);
        assert!(a.starts_with("marol-"));
        assert!(a.ends_with("-s7"));
        // Stable across runs, or a restart would fail to find its own.
        assert_eq!(desk_tag("/home/me/.marol"), desk_tag("/home/me/.marol"));
    }

    /// The line that starts or reattaches, and the two things about it that
    /// are load-bearing.
    ///
    /// `-A` is create-or-attach, so the first start and every later reattach
    /// are the same call and cannot drift apart. `--` is what stops tmux
    /// reading the agent's own flags: `claude --continue --permission-mode
    /// acceptEdits` past a tmux that is still parsing options is a tmux that
    /// eats them, and the agent then starts without the mode the person
    /// approved.
    #[test]
    fn the_attach_line_is_create_or_attach_and_hands_the_agent_its_own_flags() {
        let (prog, args) = hold_attach(
            &Socket::Named("marol-d-s1".to_string()),
            "/data/tmux.conf",
            Some("/wt/card-1"),
            "claude",
            &["--continue".to_string(), "--permission-mode".to_string(), "acceptEdits".to_string()],
        );
        // A pair, not a spawn: this is what lets the whole line go through a
        // doorway into another world.
        assert_eq!(prog, "tmux");
        assert!(args.contains(&"-A".to_string()), "not create-or-attach: {args:?}");
        assert!(args.contains(&"-D".to_string()), "two clients would fight over the size");

        let sep = args.iter().position(|a| a == "--").expect("no -- separator");
        let after: Vec<&String> = args[sep + 1..].iter().collect();
        assert_eq!(after[0], "claude");
        assert_eq!(after[1], "--continue");
        assert_eq!(after[3], "acceptEdits");
        // Everything tmux is meant to read is on tmux's side of it.
        assert!(args[..sep].contains(&"/data/tmux.conf".to_string()));
        assert!(args[..sep].contains(&"/wt/card-1".to_string()));
    }

    /// Ending it kills the server, not the session: this socket holds exactly
    /// one, so a surviving server would be a daemon waiting for something
    /// that is never coming back.
    #[test]
    fn ending_a_held_session_takes_its_server_with_it() {
        let (prog, args) = hold_destroy(&Socket::Named("marol-d-s1".to_string()));
        assert_eq!(prog, "tmux");
        assert!(args.contains(&"kill-server".to_string()), "{args:?}");
        assert!(!args.contains(&"kill-session".to_string()));
        assert!(args.contains(&"marol-d-s1".to_string()), "wrong socket: {args:?}");
    }

    /// Which flag names the socket is the whole difference between a world
    /// this process shares a filesystem with and one it does not, and it has
    /// to be the *same* difference in all three commands — attach, kill and
    /// ask. Name a session with `-L` and ask after it with `-S` and tmux
    /// answers about a socket nobody ever created: every held agent reads as
    /// gone, and the sweep is then free to kill what it cannot see.
    #[test]
    fn every_command_names_the_socket_the_same_way() {
        let named = Socket::Named("marol-d-s1".to_string());
        let path = Socket::Path("/home/me/.marol/sockets/marol-d-s1".to_string());
        assert_eq!(named.args(), ["-L", "marol-d-s1"]);
        assert_eq!(
            path.args(),
            ["-S", "/home/me/.marol/sockets/marol-d-s1"]
        );
        for sock in [&named, &path] {
            let head = sock.args();
            for (_, args) in [
                hold_attach(sock, "/c", None, "claude", &[]),
                hold_alive(sock),
            ] {
                assert_eq!(&args[..2], &head[..], "socket flags differ: {args:?}");
            }
        }
        // Asking is a question, never an action: a `has-session` that could
        // start one would make the sweep's "is this alive?" create the very
        // thing it is deciding whether to kill.
        let (_, args) = hold_alive(&named);
        assert!(args.contains(&"has-session".to_string()), "{args:?}");
        assert!(!args.iter().any(|a| a.starts_with("new-")), "{args:?}");
    }

    /// Ending a session in another world has to take the socket file with it.
    ///
    /// Locally the caller unlinks the inode tmux leaves behind. Over there it
    /// cannot: it has no filesystem to reach and no way to tell a dead socket
    /// from a live one afterwards. So the kill and the unlink are one command
    /// or the directory grows a file per session, for ever, and every later
    /// sweep has to open each one to find out it is nothing.
    #[test]
    fn ending_a_session_in_another_world_takes_its_socket_file_too() {
        let p = "/home/me/.marol/s/1a2b-s1";
        let (prog, args) = hold_destroy(&Socket::Path(p.to_string()));
        assert_eq!(prog, "sh");
        let line = &args[1];
        assert!(line.contains("kill-server"), "{line}");
        assert!(line.contains("rm -f"), "{line}");
        // `;`, not `&&`: an already-dead server exits non-zero, and that is
        // exactly when the file is rubbish that must still go.
        assert!(!line.contains("&&"), "{line}");
        // The path is an argument, not text spliced into a script, and both
        // halves read the same one.
        assert_eq!(args.last().unwrap(), p);
        assert!(!line.contains(p), "the path was pasted into the script: {line}");
        assert_eq!(line.matches("$1").count(), 2, "{line}");
    }

    /// The path a socket lands on has to fit in a `sockaddr_un`, which is
    /// about 104 bytes and not negotiable. A home directory plus ours plus a
    /// uuid is most of that already, so the budget is pinned here rather than
    /// discovered as "tmux: socket name too long" on somebody's machine.
    #[test]
    fn a_remote_socket_path_fits_in_a_unix_socket_address() {
        // The shape the app actually builds: `/tmp`, one directory per uid,
        // and the name. Not the home — a home is unbounded, and a macOS temp
        // one put this at 135 bytes, over the limit, where every session in
        // that world failed to start with nothing to read about it.
        let p = hold_socket_path(
            "/tmp/marol-4294967295",
            &desk_tag("/Users/someone/Library/Application Support/marol"),
            "550e8400-e29b-41d4-a716-446655440000",
        );
        assert!(p.len() < SOCKET_PATH_LIMIT, "{} bytes: {p}", p.len());
        // Trailing slashes come from joins upstream and must not double.
        assert!(!hold_socket_path("/d/", "t", "s").contains("//"));
        // Two desks share one remote home; the tag is what keeps their
        // sockets — and their sweeps — apart.
        assert_ne!(
            hold_socket_path("/d", "aaaa", "s1"),
            hold_socket_path("/d", "bbbb", "s1"),
        );
        assert!(hold_socket_path("/d", "aaaa", "s1").starts_with("/d/aaaa-"));
        assert_eq!(hold_prefix("aaaa", true), "aaaa-");
        assert_eq!(hold_prefix("aaaa", false), "marol-aaaa-");
    }

    /// The reason there is a socket per session at all, stated as a test so
    /// nobody "optimises" it back into a shared server: a tmux session
    /// inherits the *server's* environment, so on a shared server the second
    /// session reads back the first one's id — measured, not feared.
    #[test]
    fn a_socket_per_session_is_what_makes_the_environment_right() {
        assert_ne!(
            hold_socket(&desk_tag("/d"), "s1"),
            hold_socket(&desk_tag("/d"), "s2"),
        );
    }
}
