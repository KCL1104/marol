#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod channel;
mod config;
mod core;
mod host;
mod hooks;
mod i18n;
mod pty;
mod shell_env;
mod prompt;
mod store;
mod update;
mod worktree;

use ::core::result::Result as StdResult;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;

use crate::core::{Core, SessionMeta, UiSink};

/// Whether the main window has the user's eyes. One window, one flag; written
/// by the window-event handler, read on the notification path.
static FOCUSED: AtomicBool = AtomicBool::new(true);

/// Bridges the transport-agnostic core onto Tauri. Most events go straight to
/// the webview; `notify` and `badge` are handled natively because the OS is
/// the right renderer for both — they are exactly the signals that must reach
/// someone who is not looking at the app.
struct TauriSink(AppHandle);

impl UiSink for TauriSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        if event == "notify" {
            // Only when the window is unfocused: with the app in front of
            // you, the in-app banner already says it, and an OS notification
            // on top would just be an echo. `force` is the test button's
            // exemption — the person pressing it is focused by definition,
            // and the gate would swallow exactly what they asked to see.
            let force = payload["force"].as_bool().unwrap_or(false);
            if force || !FOCUSED.load(Ordering::Relaxed) {
                let title = payload["title"].as_str().unwrap_or("Marol");
                let body = payload["body"].as_str().unwrap_or_default();
                if let Err(e) = self.0.notification().builder().title(title).body(body).show() {
                    eprintln!("[tauri] notification failed: {e}");
                }
            }
        }
        if event == "badge" {
            // The dock/taskbar wears the waiting count, so "how many agents
            // need me" survives minimising the window. macOS and Unity
            // launchers render it; elsewhere the call is a harmless no-op —
            // which on Windows means no-op entirely, and is why the same
            // number also goes to the tray below.
            let count = payload["count"].as_i64().unwrap_or(0);
            let app = self.0.clone();
            let run = self.0.run_on_main_thread(move || {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.set_badge_count((count > 0).then_some(count));
                }
                paint_tray(&app, count.max(0) as usize);
            });
            if let Err(e) = run {
                eprintln!("[tauri] badge update failed: {e}");
            }
        }
        if let Err(e) = self.0.emit(event, payload) {
            eprintln!("[tauri] emit {event} failed: {e}");
        }
    }
}

/// Put the waiting count on the tray, and remember it.
///
/// Remembered because the count and the language arrive on different clocks:
/// the badge fires when a session changes state, `set_locale` when a person
/// changes the picker, and each has to be able to redraw without the other
/// happening again.
///
/// Must be called on the main thread — every caller here is already inside a
/// `run_on_main_thread` or a menu handler, which is one.
fn paint_tray(app: &AppHandle, waiting: usize) {
    *app.state::<AppState>().waiting.lock().unwrap() = waiting;
    let Some(tray) = app.tray_by_id("main") else { return };
    let locale = app
        .state::<AppState>()
        .core()
        .map(|c| c.locale.get())
        .unwrap_or_default();
    let _ = tray.set_title(Some(i18n::tray_title(waiting)));
    let _ = tray.set_tooltip(Some(i18n::tray_tooltip(locale, waiting)));
}

/// Bring the window back and put the caret in it.
///
/// `show` as well as `set_focus`: a window the platform minimised to the tray
/// is hidden, and focusing something hidden focuses nothing.
fn open_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// The tray: an icon that says whether anything is waiting, and a way back
/// into the window.
///
/// It exists mostly for Windows. macOS and Unity draw the waiting count on
/// the dock icon, so there the tray repeats something already said; on
/// Windows there is no badge at all, and until now a closed window meant no
/// signal of any kind that an agent was blocked on you.
///
/// Deliberately three lines and no more:
///
///   * closing the window still does what the platform says it does. Making
///     close mean hide is a thing tray apps do, and it surprises everyone who
///     meant to quit — the more so now that quitting is cheap, since the
///     agents outlive it.
///   * quitting from here goes through `ExitRequested` like every other
///     quit, so tmux-held sessions are *detached* rather than orphaned and
///     the hook port is given back.
///   * the menu does not list the waiting sessions by name. That is a real
///     idea and a bigger one: it needs the list rebuilt on every state
///     change and a click route into the webview, and the count already
///     answers the question the tray exists to answer.
fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let locale = i18n::Locale::default();
    let show = MenuItem::with_id(app, "tray.show", i18n::tray_show(locale), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "tray.quit", i18n::tray_quit(locale), true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    *app.state::<AppState>().tray_items.lock().unwrap() = Some((show, quit));

    let mut builder = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .tooltip(i18n::tray_tooltip(locale, 0))
        // The menu belongs to the right button. A left click is the
        // impatient version of "open it", and swallowing that into a menu
        // makes the common act cost two clicks.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "tray.show" => open_window(app),
            "tray.quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                open_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

#[derive(Default)]
struct AppState {
    core: Mutex<Option<Arc<Core>>>,
    boot_error: Mutex<Option<String>>,
    /// The tray's two menu items, held so a language change can rewrite
    /// them. The tray is built before the webview has said which language
    /// it is in — it has to exist from the first moment, since its whole
    /// job is to be there when the window is not — so it is built in
    /// English and corrected when the answer arrives.
    tray_items: Mutex<Option<(MenuItem<tauri::Wry>, MenuItem<tauri::Wry>)>>,
    /// The last count the badge carried, so a language change can redraw the
    /// tooltip without waiting for the next session to change state.
    waiting: Mutex<usize>,
}

impl AppState {
    fn core(&self) -> StdResult<Arc<Core>, String> {
        self.core
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| match self.boot_error.lock().unwrap().clone() {
                Some(e) => format!("Marol failed to start: {e}"),
                None => "Marol is still starting up.".to_string(),
            })
    }
}

/* ------------------------------------------------------------------ */
/* Commands                                                            */
/* ------------------------------------------------------------------ */

/// Run a command's real work off the thread the window is drawn on.
///
/// **A synchronous `#[tauri::command]` runs its whole body on the main
/// thread.** Not a detail: `tauri-macros` picks `body_blocking` for any `fn`
/// without `async`, and on Windows the WebView2 handler carrying an invoke
/// fires on the thread that created the controller. So a command that takes
/// 300ms is 300ms in which the window does not repaint, input is not
/// processed, and the terminal output this app `emit`s cannot reach the
/// webview.
///
/// Locally that never showed, because locally these calls are `std::fs` and
/// cost microseconds. Behind a doorway every one of them is a *process* —
/// `wsl.exe` or `ssh` — and the board asks for several per open attempt on a
/// timer. That is why a WSL card felt like a hung app while a local card felt
/// fine: the same code, three orders of magnitude apart.
///
/// `spawn_blocking`, deliberately, and not `#[tauri::command(async)]`. That
/// attribute hands the still-synchronous body to `async_runtime::spawn`,
/// which parks it on a tokio *worker* — and the hook listener every agent
/// reports to lives on that same runtime. Blocking those workers would let a
/// board refresh stall the hook calls agents are waiting on, which is the
/// worse bug: a slow desk would have become a slow agent.
///
/// What stays synchronous, and why, is pinned by the test at the foot of this
/// file rather than left to memory.
async fn blocking<T, F>(f: F) -> StdResult<T, String>
where
    F: FnOnce() -> StdResult<T, String> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("that call could not be run: {e}"))?
}

#[tauri::command]
fn set_locale(app: AppHandle, state: State<'_, AppState>, locale: String) -> StdResult<(), String> {
    // Best-effort: the language is a display preference, so a call that lands
    // before the core is up is not worth surfacing as an error to the webview.
    let parsed = i18n::Locale::parse(&locale);
    if let Ok(core) = state.core() {
        core.locale.set(parsed);
    }
    // The tray was built before anyone had said which language this is, so
    // this is where it finds out. Rewriting the two labels rather than
    // rebuilding the menu: a menu replaced under an open tray is a menu that
    // closes itself in the user's hand.
    if let Some((show, quit)) = state.tray_items.lock().unwrap().as_ref() {
        let _ = show.set_text(i18n::tray_show(parsed));
        let _ = quit.set_text(i18n::tray_quit(parsed));
    }
    let waiting = *state.waiting.lock().unwrap();
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || paint_tray(&handle, waiting));
    Ok(())
}

#[tauri::command]
fn boot_status(state: State<'_, AppState>) -> serde_json::Value {
    let core = state.core.lock().unwrap().clone();
    match core {
        Some(c) => serde_json::json!({
            "ready": true,
            "shell": c.env.shell,
            "envResolved": c.env.resolved,
            // Where the environment came from, for the diagnostics label:
            // a probed login shell, Windows' own process environment (the
            // real thing there, not a fallback), or the degraded fallback.
            "envSource": if !c.env.resolved { "process" }
                else if cfg!(windows) { "system" }
                else { "login" },
            "envVarCount": c.env.vars.len(),
            "path": c.env.path(),
            "claude": c.env.which("claude").map(|p| p.to_string_lossy().to_string()),
            "claudeVersion": c.cli_version("claude"),
            "codex": c.env.which("codex").map(|p| p.to_string_lossy().to_string()),
            "codexVersion": c.cli_version("codex"),
            // Every agent CLI this environment can actually see — the
            // first-run panel's detection report, from the same resolved
            // PATH the sessions get. `version` is filled in for the CLIs
            // whose conventions this app knows, and `reports` says whether
            // the one installed here is new enough to be wired for status:
            // "found" and "will show you what it is doing" are different
            // facts, and a panel that only reported the first would be
            // silent about the commonest reason a card sits blank.
            "agents": core::BARE_AGENTS.iter().map(|a| serde_json::json!({
                "name": a,
                "path": c.env.which(a).map(|p| p.to_string_lossy().to_string()),
                "version": c.cli_version(a),
                "reports": c.reports_status(a),
            })).collect::<Vec<_>>(),
            // Whether this desk's claude sessions can name themselves and,
            // with that, message each other across cards.
            "messaging": c.named_sessions(),
            // What the held shells in each world have saved, and what they
            // have cost. Declining is silent by design, so a world where the
            // channel never opens looks exactly like one where it is working
            // — until this says which.
            "channels": c.channel_tallies().into_iter().map(|(world, t)| serde_json::json!({
                "world": world,
                "held": t.held,
                "spawned": t.spawned,
                "lost": t.lost,
            })).collect::<Vec<_>>(),
            "db": store::default_path().to_string_lossy(),
            "hookUrl": c.hook_url(),
            // The one text this desk puts into a session on its own. Naming
            // the file is how the settings can offer to open it.
            "promptTemplate": c.prompt_template_path(),
        }),
        None => serde_json::json!({
            "ready": false,
            "error": state.boot_error.lock().unwrap().clone(),
        }),
    }
}

#[tauri::command]
async fn new_session(
    state: State<'_, AppState>,
    cwd: String,
    agent: Option<String>,
    args: Option<Vec<String>>,
    cols: u16,
    rows: u16,
) -> StdResult<String, String> {
    let core = state.core()?;
    blocking(move || {
        core.new_session(
            cwd,
            agent.unwrap_or_else(|| "claude".into()),
            args.unwrap_or_default(),
            cols,
            rows,
        )
        .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
async fn reopen_session(
    state: State<'_, AppState>,
    id: String,
    cols: u16,
    rows: u16,
) -> StdResult<(), String> {
    let core = state.core()?;
    blocking(move || {
        core.reopen_session(&id, cols, rows)
        .map_err(|e| e.to_string())
    })
    .await
}

/// Keystrokes from xterm.js, forwarded to the PTY verbatim.
///
/// **Deliberately synchronous, and it is the one place where that matters.**
/// A keystroke is a write to a pipe this process already holds open — no
/// doorway, no process, microseconds — so there is nothing here to move off
/// the main thread. Moving it would cost the one guarantee typing has:
/// `spawn_blocking` tasks are not ordered against each other, so two quick
/// keys could reach the PTY reversed. `term_resize` stays for the same
/// reason — a resize that overtook the keys before it would reflow the TUI
/// around the wrong buffer.
#[tauri::command]
fn term_write(state: State<'_, AppState>, id: String, data: String) -> StdResult<(), String> {
    state.core()?.write(&id, &data).map_err(|e| e.to_string())
}

#[tauri::command]
fn term_resize(
    state: State<'_, AppState>,
    id: String,
    cols: u16,
    rows: u16,
) -> StdResult<(), String> {
    state
        .core()?
        .resize(&id, cols, rows)
        .map_err(|e| e.to_string())
}

/// Replay buffer for a pane that is mounting after its PTY already started.
#[tauri::command]
fn term_snapshot(state: State<'_, AppState>, id: String) -> StdResult<serde_json::Value, String> {
    let (data, seq) = state.core()?.snapshot(&id).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "data": data, "seq": seq }))
}

#[tauri::command]
async fn close_session(state: State<'_, AppState>, id: String) -> StdResult<(), String> {
    let core = state.core()?;
    blocking(move || core.close_session(&id).map_err(|e| e.to_string())).await
}

/// Mark a session done, or undo it. See `Core::set_completed`.
#[tauri::command]
fn set_completed(state: State<'_, AppState>, id: String, completed: bool) -> StdResult<(), String> {
    state
        .core()?
        .set_completed(&id, completed)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn archive_session(state: State<'_, AppState>, id: String) -> StdResult<(), String> {
    let core = state.core()?;
    blocking(move || core.archive_session(&id).map_err(|e| e.to_string())).await
}

/// Rename a session's row. The same door the agent's own naming goes through
/// — see `Core::rename_session` for what a rename does and does not reach.
#[tauri::command]
fn rename_session(state: State<'_, AppState>, id: String, title: String) -> StdResult<(), String> {
    state
        .core()?
        .rename_session(&id, &title)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_tabs(state: State<'_, AppState>) -> StdResult<Vec<store::StoredTab>, String> {
    Ok(state.core()?.tabs())
}

#[tauri::command]
fn create_tab(state: State<'_, AppState>, name: String) -> StdResult<String, String> {
    state.core()?.create_tab(name).map_err(|e| e.to_string())
}

#[tauri::command]
fn rename_tab(state: State<'_, AppState>, id: String, name: String) -> StdResult<(), String> {
    state.core()?.rename_tab(&id, name).map_err(|e| e.to_string())
}

#[tauri::command]
fn close_tab(state: State<'_, AppState>, id: String) -> StdResult<(), String> {
    state.core()?.close_tab(&id).map_err(|e| e.to_string())
}

/// Set a tab's layout and slot assignment. Claiming a session here removes it
/// from any other tab — see `Core::update_tab`.
#[tauri::command]
fn update_tab(
    state: State<'_, AppState>,
    id: String,
    layout: String,
    slots: Vec<Option<String>>,
) -> StdResult<(), String> {
    state
        .core()?
        .update_tab(&id, layout, slots)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_sessions(state: State<'_, AppState>) -> StdResult<Vec<SessionMeta>, String> {
    Ok(state.core()?.sessions())
}

/* ----------------------------- board ------------------------------- */

#[tauri::command]
fn list_tasks(state: State<'_, AppState>) -> StdResult<Vec<crate::core::TaskView>, String> {
    Ok(state.core()?.task_board())
}

/// `extra_repos` is the repositories beside the first, for a card that spans
/// several. Absent is the ordinary card, and every caller written before this
/// existed keeps working unchanged.
#[tauri::command]
async fn create_task(
    state: State<'_, AppState>,
    title: String,
    prompt: String,
    repo_path: String,
    base_branch: String,
    extra_repos: Option<Vec<store::TaskRepo>>,
) -> StdResult<String, String> {
    let core = state.core()?;
    blocking(move || {
        core.create_task(
            title,
            prompt,
            repo_path,
            base_branch,
            extra_repos.unwrap_or_default(),
        )
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/// Move a card between columns, or reorder it within one. Only a drag calls
/// this — see `Core::move_task`.
#[tauri::command]
async fn move_task(
    state: State<'_, AppState>,
    id: String,
    lifecycle: String,
    position: i64,
) -> StdResult<(), String> {
    let lifecycle = store::Lifecycle::parse(&lifecycle)
        .ok_or_else(|| format!("unknown lifecycle: {lifecycle}"))?;
    let core = state.core()?;
    blocking(move || {
        core.move_task(&id, lifecycle, position)
        .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
async fn delete_task(state: State<'_, AppState>, id: String) -> StdResult<(), String> {
    let core = state.core()?;
    blocking(move || core.delete_task(&id).map_err(|e| format!("{e:#}"))).await
}

/* ---------------------------- attempts ----------------------------- */

/// The first message as it would be sent, for the dialog to show and let the
/// person edit before any worktree is created.
#[tauri::command]
async fn preview_prompt(
    state: State<'_, AppState>,
    task_id: String,
    agent: String,
) -> StdResult<serde_json::Value, String> {
    let core = state.core()?;
    blocking(move || {
        core.preview_prompt(&task_id, &agent)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/// Start an attempt, or queue it when every slot is taken. `mode` is the
/// permission mode the dialog offered — parsed leniently, because an unknown
/// value must degrade to asking, never to not asking.
#[tauri::command]
async fn open_attempt(
    state: State<'_, AppState>,
    task_id: String,
    agent: Option<String>,
    prompt: Option<String>,
    mode: Option<String>,
    cols: u16,
    rows: u16,
) -> StdResult<crate::core::StartResult, String> {
    let core = state.core()?;
    blocking(move || {
        core.start_attempt(
            &task_id,
            agent.unwrap_or_else(|| "claude".into()),
            prompt,
            store::PermissionMode::parse(mode.as_deref().unwrap_or("")),
            cols,
            rows,
        )
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

#[tauri::command]
async fn cancel_queued(state: State<'_, AppState>, task_id: String) -> StdResult<(), String> {
    let core = state.core()?;
    blocking(move || core.cancel_queued(&task_id).map_err(|e| e.to_string())).await
}

/// How many attempts may hold a terminal at once. The thing being rationed
/// is a person's attention, not a machine.
#[tauri::command]
async fn concurrency(state: State<'_, AppState>) -> StdResult<serde_json::Value, String> {
    let core = state.core()?;
    blocking(move || {
        Ok(serde_json::json!({
            "max": core.max_concurrent(),
            "running": core.running_attempts(),
            "queued": core.queue().len(),
        }))
    })
    .await
}

#[tauri::command]
async fn set_concurrency(state: State<'_, AppState>, max: i64) -> StdResult<(), String> {
    let core = state.core()?;
    blocking(move || core.set_max_concurrent(max).map_err(|e| e.to_string())).await
}

/// Fold the attempt's branch back into its base, then close it out.
#[tauri::command]
async fn merge_attempt(state: State<'_, AppState>, attempt_id: String) -> StdResult<String, String> {
    let core = state.core()?;
    blocking(move || {
        core.merge_attempt(&attempt_id)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/// Push the branch and open a pull request. The attempt stays open: review is
/// exactly when there is still something to change.
#[tauri::command]
async fn open_pr(state: State<'_, AppState>, attempt_id: String) -> StdResult<String, String> {
    let core = state.core()?;
    blocking(move || core.open_pr(&attempt_id).map_err(|e| format!("{e:#}"))).await
}

/// Put a terminal back on an attempt that is not running — the state every
/// attempt is in after a restart.
#[tauri::command]
async fn reopen_attempt(
    state: State<'_, AppState>,
    attempt_id: String,
    cols: u16,
    rows: u16,
) -> StdResult<String, String> {
    let core = state.core()?;
    blocking(move || {
        core.reopen_attempt(&attempt_id, cols, rows)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/// End an attempt: freeze its diff, then give the worktree back.
#[tauri::command]
async fn finish_attempt(
    state: State<'_, AppState>,
    attempt_id: String,
    outcome: String,
) -> StdResult<(), String> {
    let outcome =
        store::Outcome::parse(&outcome).ok_or_else(|| format!("unknown outcome: {outcome}"))?;
    let core = state.core()?;
    blocking(move || {
        core.finish_attempt(&attempt_id, outcome)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/// Send a later message into an attempt's live terminal — the review drawer's
/// way of saying what is still wrong without leaving the diff.
#[tauri::command]
async fn send_followup(state: State<'_, AppState>, id: String, text: String) -> StdResult<(), String> {
    let core = state.core()?;
    blocking(move || {
        core.send_followup(&id, &text)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/// Hold a message for the end of this turn — sent when Stop lands.
#[tauri::command]
async fn queue_followup(state: State<'_, AppState>, id: String, text: String) -> StdResult<(), String> {
    let core = state.core()?;
    blocking(move || {
        core.queue_followup(&id, &text)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

#[tauri::command]
fn cancel_followup(state: State<'_, AppState>, id: String) -> StdResult<(), String> {
    state.core()?.cancel_followup(&id);
    Ok(())
}

/// The repository's branches, recency first, for the base picker.
#[tauri::command]
async fn list_branches(
    state: State<'_, AppState>,
    repo_path: String,
) -> StdResult<Vec<String>, String> {
    let core = state.core()?;
    blocking(move || {
        core.list_branches(&repo_path)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/// The rules and skills an agent working in this directory will read —
/// every supported CLI's convention, present or not.
#[tauri::command]
async fn agent_docs(state: State<'_, AppState>, cwd: String) -> StdResult<Vec<core::AgentDoc>, String> {
    let core = state.core()?;
    blocking(move || core.agent_docs(&cwd).map_err(|e| format!("{e:#}"))).await
}

/// The attempt's diff — against its base, or, when `n` names a checkpoint,
/// against that snapshot instead.
#[tauri::command]
async fn attempt_diff(
    state: State<'_, AppState>,
    attempt_id: String,
    n: Option<u64>,
) -> StdResult<String, String> {
    let core = state.core()?;
    blocking(move || {
        core.attempt_diff_from(&attempt_id, n)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/// Numstat counts and ahead/behind for an open attempt — the board's card
/// badges, cheap enough to ask for on a timer.
#[tauri::command]
async fn attempt_stats(
    state: State<'_, AppState>,
    attempt_id: String,
) -> StdResult<crate::worktree::DiffStat, String> {
    let core = state.core()?;
    blocking(move || {
        core.attempt_stats(&attempt_id)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

#[tauri::command]
fn attempt_events(
    state: State<'_, AppState>,
    attempt_id: String,
) -> StdResult<Vec<store::AttemptEvent>, String> {
    state
        .core()?
        .attempt_events(&attempt_id)
        .map_err(|e| e.to_string())
}

/// The repository's run scripts, for the drawer's buttons.
#[tauri::command]
async fn list_run_scripts(
    state: State<'_, AppState>,
    attempt_id: String,
) -> StdResult<Vec<String>, String> {
    let core = state.core()?;
    blocking(move || {
        core.list_run_scripts(&attempt_id)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/// A shell of your own in the attempt's worktree — reused while it lives.
#[tauri::command]
async fn open_shell(
    state: State<'_, AppState>,
    attempt_id: String,
    cols: u16,
    rows: u16,
) -> StdResult<String, String> {
    let core = state.core()?;
    blocking(move || {
        core.open_shell(&attempt_id, cols, rows)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/// Start a run script in the attempt's worktree, in a terminal of its own.
#[tauri::command]
async fn run_script(
    state: State<'_, AppState>,
    attempt_id: String,
    name: String,
    cols: u16,
    rows: u16,
) -> StdResult<String, String> {
    let core = state.core()?;
    blocking(move || {
        core.run_script(&attempt_id, &name, cols, rows)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/* --------------------------- dev preview --------------------------- */

/// Whether anything answers on localhost at this port right now — the
/// difference between "the dev server is up" and a blank iframe that
/// could mean anything. Blank and broken must not look alike.
#[tauri::command]
async fn probe_port(port: u16) -> StdResult<bool, String> {
    // 400ms of connect timeout is 400ms of frozen window when it lands on the
    // main thread — and a dead port is exactly the case that spends all of it.
    blocking(move || {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let timeout = std::time::Duration::from_millis(400);
        Ok(std::net::TcpStream::connect_timeout(&addr, timeout).is_ok())
    })
    .await
}

/* ----------------------------- worlds ------------------------------ */

/// The worlds a card can live in — enumerated from `wsl -l` and
/// `~/.ssh/config`, never invented, never probed remotely.
#[tauri::command]
async fn list_worlds(state: State<'_, AppState>) -> StdResult<core::Worlds, String> {
    let core = state.core()?;
    blocking(move || Ok(core.list_worlds())).await
}

/// Reach one world and report its claude, or the whole reason it could
/// not be reached. Deliberately lazy: called on a person's pick, never
/// at startup.
#[tauri::command]
async fn probe_world(state: State<'_, AppState>, world: String) -> StdResult<core::WorldProbe, String> {
    let core = state.core()?;
    blocking(move || Ok(core.probe_world(&world))).await
}

/// One directory inside a world, for the folder picker.
///
/// `path` of `null` starts at that world's own home. See `Core::list_dir` for
/// why this exists rather than the platform's folder dialog.
#[tauri::command]
async fn list_dir(
    state: State<'_, AppState>,
    world: String,
    path: Option<String>,
) -> StdResult<core::DirListing, String> {
    let core = state.core()?;
    blocking(move || {
        core.list_dir(&world, path.as_deref())
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/* ------------------------- editable diff --------------------------- */

/// Both sides of one file in an attempt's diff, as full text: the base
/// commit's copy and the worktree's. What the in-place editor edits.
#[tauri::command]
async fn attempt_file(
    state: State<'_, AppState>,
    attempt_id: String,
    path: String,
) -> StdResult<core::AttemptFile, String> {
    let core = state.core()?;
    blocking(move || {
        core.attempt_file(&attempt_id, &path)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/// Write one file in the attempt's worktree — a human's own edit. Refused
/// mid-turn in the core, not just hidden in the UI; refused too when the
/// disk no longer matches `expected`, the text the editor loaded.
#[tauri::command]
async fn write_attempt_file(
    state: State<'_, AppState>,
    attempt_id: String,
    path: String,
    contents: String,
    expected: Option<String>,
) -> StdResult<(), String> {
    let core = state.core()?;
    blocking(move || {
        core.write_attempt_file(&attempt_id, &path, &contents, expected.as_deref())
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/* ---------------------------- parked ------------------------------- */

/// Park an attempt: keep the branch, the checkpoints and the conversation,
/// give back the worktree and the concurrency slot. Returns the branch
/// name — the UI puts it on the clipboard.
#[tauri::command]
async fn park_attempt(state: State<'_, AppState>, attempt_id: String) -> StdResult<String, String> {
    let core = state.core()?;
    blocking(move || {
        core.park_attempt(&attempt_id)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/// Resume a parked attempt: worktree back at its old path on its branch,
/// shelf checkpoint restored, terminal reopened with the old conversation.
#[tauri::command]
async fn resume_attempt(
    state: State<'_, AppState>,
    attempt_id: String,
    cols: u16,
    rows: u16,
) -> StdResult<core::Resumed, String> {
    let core = state.core()?;
    blocking(move || {
        core.resume_attempt(&attempt_id, cols, rows)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/* -------------------------- checkpoints ---------------------------- */

/* ---------------------------- updates ----------------------------- */

/// Whether this build was given a public key to check a manifest against.
///
/// There is no key in this repository — the same ruling the Apple signing
/// variables get, and for the same reason: a key referenced before it exists
/// is an empty string that fails somewhere further from the cause. So the
/// updater is wired, configured, and honest about being unarmed, and the
/// panel says "not configured for this build" rather than offering a button
/// that could only produce an error.
fn updater_configured(app: &AppHandle) -> bool {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|c| c.get("pubkey"))
        .and_then(|k| k.as_str())
        .is_some_and(|k| !k.trim().is_empty())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Everything the panel needs that costs nothing to answer.
///
/// Deliberately free of network: this is what paints, and a status that has
/// to wait for GitHub is a status that makes the settings panel hang on a
/// train. The one field that needs the network — whether a newer version
/// exists — arrives from `update_check` and is `null` until it does.
#[tauri::command]
fn update_status(app: AppHandle, state: State<'_, AppState>) -> serde_json::Value {
    let core = state.core().ok();
    let cost = core.as_ref().map(|c| c.restart_cost()).unwrap_or_default();
    serde_json::json!({
        "version": update::current_version(),
        "configured": updater_configured(&app),
        // Absent core means the desk failed to boot; the setting lives in its
        // database, and true is what a fresh install would read anyway.
        "enabled": core.as_ref().map(|c| c.update_enabled()).unwrap_or(true),
        "selfContained": update::install_kind() == update::Install::SelfContained,
        "held": cost.held,
        "lost": cost.lost,
        "lastCheck": core.as_ref().and_then(|c| c.update_last_check()),
        "due": core.as_ref().map(|c| c.update_check_due(now_secs())).unwrap_or(true),
        "releases": RELEASES_URL,
    })
}

/// The releases page, for the installs this app will not update itself and
/// for anyone who would rather read the notes first.
const RELEASES_URL: &str = "https://github.com/KCL1104/marol/releases/latest";

/// Ask what the newest release is. `None` is "you are on it".
///
/// Silence is the whole error handling. Offline, rate-limited, GitHub down,
/// a proxy that eats it — none of those are things a person can act on, and
/// a desk that interrupts the work to report that it could not perform a
/// courtesy has made the courtesy into a cost. The failure goes to stderr
/// and the panel keeps saying what it last knew.
#[tauri::command]
async fn update_check(app: AppHandle) -> StdResult<Option<update::Available>, String> {
    use tauri_plugin_updater::UpdaterExt;

    if !updater_configured(&app) {
        return Ok(None);
    }
    let core = app.state::<AppState>().core().ok();
    if core.as_ref().is_some_and(|c| !c.update_enabled()) {
        return Ok(None);
    }

    let updater = app.updater().map_err(|e| format!("{e}"))?;
    let found = updater.check().await.map_err(|e| format!("{e}"))?;

    if let Some(c) = core {
        let _ = c.mark_update_checked(now_secs());
    }

    Ok(found.and_then(|u| {
        // The plugin compares versions itself, but this desk's own rule is
        // the one it has a test for — and a manifest that ever offered a
        // downgrade would be applied without this.
        if !update::is_newer(&u.version, update::current_version()) {
            return None;
        }
        Some(update::Available {
            version: u.version.clone(),
            notes: u.body.clone().filter(|b| !b.trim().is_empty()),
            date: u.date.map(|d| d.to_string()),
        })
    }))
}

/// Download the new version, put a copy of the database somewhere safe, swap
/// the binary, and restart into it.
///
/// In that order, and the order is the point: the snapshot is taken before
/// anything is replaced, because the case it exists for is the one where the
/// new version is the problem.
///
/// `acknowledged` carries a person's answer to the count of agents this would
/// end. It is never sent by default — see `update::check_restart`.
#[tauri::command]
async fn update_apply(app: AppHandle, acknowledged: bool) -> StdResult<(), String> {
    use tauri_plugin_updater::UpdaterExt;

    if !updater_configured(&app) {
        return Err("this build has no update key, so it cannot verify a download".into());
    }
    if update::install_kind() == update::Install::PackageManaged {
        return Err(
            "this copy was installed by a package manager, which keeps its own record of \
             the files it owns. Update it the way it was installed"
                .into(),
        );
    }

    let core = app.state::<AppState>().core().map_err(|e| e.to_string())?;
    update::check_restart(core.restart_cost().lost, acknowledged).map_err(|e| format!("{e:#}"))?;

    let updater = app.updater().map_err(|e| format!("{e}"))?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("{e}"))?
        .ok_or_else(|| "there is no newer version to install".to_string())?;

    // Before a single byte is replaced.
    let snapshot = core
        .snapshot_db_before(update::current_version())
        .map_err(|e| format!("{e:#}"))?;
    eprintln!("[update] database copied to {}", snapshot.display());

    let handle = app.clone();
    let mut got: u64 = 0;
    update
        .download_and_install(
            move |chunk, total| {
                got += chunk as u64;
                let _ = handle.emit(
                    "update:progress",
                    serde_json::json!({ "got": got, "total": total }),
                );
            },
            || {},
        )
        .await
        .map_err(|e| format!("{e}"))?;

    // Explicitly, rather than trusting the restart to raise `ExitRequested`.
    // Two things have to have happened before the next process starts, and
    // both are this function's: the held agents have to be *detached* rather
    // than orphaned, and the hook port has to be handed back. That port is
    // baked into the config of every session already running — it is a
    // photograph, not a pointer — so a new process that finds it still bound
    // takes a different one, and every agent this update was careful not to
    // kill goes silent for the rest of its life instead.
    if let Some(c) = app.state::<AppState>().core.lock().unwrap().clone() {
        c.shutdown();
    }
    app.restart();
}

#[tauri::command]
fn set_update_enabled(state: State<'_, AppState>, on: bool) -> StdResult<(), String> {
    state
        .core()?
        .set_update_enabled(on)
        .map_err(|e| format!("{e:#}"))
}

/* -------------------------- checkpoints --------------------------- */

#[tauri::command]
fn checkpoints_enabled(state: State<'_, AppState>) -> StdResult<bool, String> {
    Ok(state.core()?.checkpoints_enabled())
}

#[tauri::command]
fn set_checkpoints_enabled(state: State<'_, AppState>, on: bool) -> StdResult<(), String> {
    state
        .core()?
        .set_checkpoints_enabled(on)
        .map_err(|e| format!("{e:#}"))
}

/// The manual snapshot button. `None` means the worktree matches the last
/// checkpoint already — nothing new to keep.
#[tauri::command]
async fn checkpoint_now(
    state: State<'_, AppState>,
    attempt_id: String,
) -> StdResult<Option<crate::worktree::Checkpoint>, String> {
    let core = state.core()?;
    blocking(move || {
        core.checkpoint_now(&attempt_id)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

#[tauri::command]
async fn list_checkpoints(
    state: State<'_, AppState>,
    attempt_id: String,
) -> StdResult<Vec<crate::worktree::Checkpoint>, String> {
    let core = state.core()?;
    blocking(move || {
        core.list_checkpoints(&attempt_id)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/// Put the worktree back to checkpoint `n` (`0` = the attempt's base).
/// Code only; refused while a turn is in flight.
#[tauri::command]
async fn restore_checkpoint(
    state: State<'_, AppState>,
    attempt_id: String,
    n: u64,
) -> StdResult<core::Restored, String> {
    let core = state.core()?;
    blocking(move || {
        core.restore_checkpoint(&attempt_id, n)
        .map_err(|e| format!("{e:#}"))
    })
    .await
}

/* -------------------------- notifications -------------------------- */

/// Which notifications the desk raises. Read by the environment panel.
#[tauri::command]
fn notify_prefs(state: State<'_, AppState>) -> StdResult<core::NotifyPrefs, String> {
    Ok(state.core()?.notify_prefs())
}

#[tauri::command]
fn set_notify_prefs(
    state: State<'_, AppState>,
    prefs: core::NotifyPrefs,
) -> StdResult<(), String> {
    state
        .core()?
        .set_notify_prefs(prefs)
        .map_err(|e| format!("{e:#}"))
}

/// Fire one now — the only honest way to check the channel reaches the OS.
#[tauri::command]
fn test_notification(state: State<'_, AppState>) -> StdResult<(), String> {
    state.core()?.test_notification();
    Ok(())
}

/* ---------------------------- profiles ----------------------------- */

/// Everything a launch dialog can offer: bare agents, then profiles.
#[tauri::command]
fn list_launchers(state: State<'_, AppState>) -> StdResult<Vec<crate::core::Launcher>, String> {
    state.core()?.launchers().map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn list_profiles(state: State<'_, AppState>) -> StdResult<Vec<store::Profile>, String> {
    state.core()?.profiles().map_err(|e| format!("{e:#}"))
}

/// Replace the profiles wholesale — there are few enough that the editor
/// works on the whole list.
#[tauri::command]
fn save_profiles(
    state: State<'_, AppState>,
    profiles: Vec<store::Profile>,
) -> StdResult<(), String> {
    state
        .core()?
        .set_profiles(profiles)
        .map_err(|e| format!("{e:#}"))
}

/* ------------------------------------------------------------------ */
/* Main                                                                */
/* ------------------------------------------------------------------ */

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        // For the PR URL: an anchor inside the webview would navigate the
        // app itself, so external links go out through the opener.
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(|_, event| {
            if let tauri::WindowEvent::Focused(focused) = event {
                FOCUSED.store(*focused, Ordering::Relaxed);
            }
        })
        .manage(AppState::default())
        .setup(|app| {
            // Before the core: the tray's whole job is to be there when the
            // window is not, and a boot that fails is exactly a moment when
            // somebody needs a way back to the window.
            if let Err(e) = build_tray(app.handle()) {
                eprintln!("[tauri] tray unavailable: {e}");
            }
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let sink: Arc<dyn UiSink> = Arc::new(TauriSink(handle.clone()));
                let state = handle.state::<AppState>();
                let data_dir = store::default_path()
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(std::env::temp_dir);
                match Core::start(sink, store::default_path(), data_dir).await {
                    Ok(core) => {
                        *state.core.lock().unwrap() = Some(core);
                        let _ = handle.emit("core:ready", serde_json::json!({}));
                        eprintln!("[main] core ready");
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        eprintln!("[main] core failed to start: {msg}");
                        *state.boot_error.lock().unwrap() = Some(msg.clone());
                        let _ = handle.emit("core:failed", serde_json::json!({ "error": msg }));
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            boot_status,
            set_locale,
            new_session,
            reopen_session,
            term_write,
            term_resize,
            term_snapshot,
            close_session,
            archive_session,
            rename_session,
            set_completed,
            list_sessions,
            list_tabs,
            create_tab,
            rename_tab,
            close_tab,
            update_tab,
            list_tasks,
            create_task,
            move_task,
            delete_task,
            preview_prompt,
            open_attempt,
            reopen_attempt,
            finish_attempt,
            agent_docs,
            attempt_diff,
            attempt_stats,
            attempt_events,
            send_followup,
            queue_followup,
            cancel_followup,
            list_branches,
            list_run_scripts,
            run_script,
            open_shell,
            list_launchers,
            list_profiles,
            save_profiles,
            notify_prefs,
            set_notify_prefs,
            test_notification,
            checkpoints_enabled,
            set_checkpoints_enabled,
            checkpoint_now,
            list_checkpoints,
            restore_checkpoint,
            park_attempt,
            resume_attempt,
            probe_port,
            list_worlds,
            probe_world,
            list_dir,
            attempt_file,
            write_attempt_file,
            cancel_queued,
            concurrency,
            set_concurrency,
            merge_attempt,
            open_pr,
            update_status,
            update_check,
            update_apply,
            set_update_enabled,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Marol")
        .run(|handle, event| {
            // Kill child terminals on quit rather than leaving orphaned
            // `claude` processes behind.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                if let Some(core) = handle.state::<AppState>().core.lock().unwrap().clone() {
                    core.shutdown();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    /// Stated as a test so nobody quietly puts a doorway back on the window's
    /// own thread.
    ///
    /// A `#[tauri::command]` written as a plain `fn` runs its whole body on
    /// the main thread — see `blocking` for the chain that makes that true.
    /// Every command that can reach another world therefore has to be `async`
    /// and hand its work to `blocking`. The exceptions are listed here with
    /// the reason each one earns, because "this one is fine" should cost a
    /// line of prose rather than a shrug.
    ///
    /// Checked in both directions: a new synchronous command that is not
    /// named here fails, and a name here that has since become `async` fails
    /// too — so the list cannot rot into a wishlist.
    #[test]
    fn no_command_that_can_reach_a_world_runs_on_the_main_thread() {
        // (command, why it may stay synchronous)
        const ALLOWED: &[(&str, &str)] = &[
            // The terminal's hot path. See `term_write`: microseconds, and
            // ordering matters more than the microseconds.
            ("term_write", "ordering — a keystroke must not overtake its predecessor"),
            ("term_resize", "ordering — a resize must not overtake the keys before it"),
            ("term_snapshot", "base64 of an in-memory buffer this process owns"),
            // Main-thread affine by nature: it rewrites the tray's menu items.
            ("set_locale", "touches the tray, which belongs to the main thread"),
            // Everything below reads or writes only this process's memory and
            // its own SQLite file. No process is spawned, no host is reached.
            ("boot_status", "reads the environment probed once at startup"),
            ("update_status", "settings and an in-memory count"),
            ("list_sessions", "in-memory"),
            ("list_tasks", "in-memory and SQLite"),
            ("list_tabs", "SQLite"),
            ("create_tab", "SQLite"),
            ("rename_tab", "SQLite"),
            ("close_tab", "SQLite"),
            ("update_tab", "SQLite"),
            ("set_completed", "SQLite"),
            ("rename_session", "SQLite"),
            ("attempt_events", "SQLite"),
            ("cancel_followup", "in-memory"),
            ("list_launchers", "SQLite"),
            ("list_profiles", "SQLite"),
            ("save_profiles", "SQLite"),
            ("notify_prefs", "in-memory"),
            ("set_notify_prefs", "SQLite"),
            ("test_notification", "hands one line to the OS notifier"),
            ("checkpoints_enabled", "SQLite"),
            ("set_checkpoints_enabled", "SQLite"),
            ("set_update_enabled", "SQLite"),
        ];

        let src = include_str!("main.rs");
        let (mut sync, mut not_sync) = (Vec::new(), Vec::new());
        for block in src.split("#[tauri::command]").skip(1) {
            let Some(line) = block
                .lines()
                .find(|l| l.starts_with("fn ") || l.starts_with("async fn "))
            else {
                continue;
            };
            let (list, rest) = match line.strip_prefix("async fn ") {
                Some(rest) => (&mut not_sync, rest),
                None => (&mut sync, &line["fn ".len()..]),
            };
            list.push(rest.split('(').next().unwrap_or_default());
        }
        assert!(
            sync.len() + not_sync.len() > 40,
            "the scan found nothing to check — has the command shape changed?"
        );

        for name in &sync {
            assert!(
                ALLOWED.iter().any(|(n, _)| n == name),
                "`{name}` is a synchronous command, so its whole body runs on the \
                 main thread. Make it `async` and put the work through `blocking`, \
                 or add it to ALLOWED with the reason it is safe."
            );
        }
        for (name, _) in ALLOWED {
            assert!(
                sync.contains(name),
                "ALLOWED still names `{name}`, which is no longer a synchronous \
                 command. Drop the entry rather than leaving the list to rot."
            );
        }
    }
}
