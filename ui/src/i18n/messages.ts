/**
 * Every string the interface says, in both languages.
 *
 * `en` is the source of truth: its keys define `MessageKey`, and `zhTW` is
 * typed as a total map over that, so a key added to one language and not the
 * other fails the typecheck rather than silently rendering a raw key.
 *
 * Placeholders are `{name}` and are substituted by `t()`. They are deliberately
 * positional-free, because the two languages order their clauses differently.
 */
export const en = {
  /* ------------------------------ shared ------------------------------ */
  'common.cancel': 'Cancel',
  'common.close': 'Close',
  'common.open': 'Open',
  'common.create': 'Create',
  'common.start': 'Start',
  'common.choose': 'Choose…',
  'common.loading': 'Loading…',
  // Renamed from "Environment" once the panel grew sections and a search:
  // the diagnostics are one section of a settings panel now, not the whole
  // of it, and a name that undersells is its own discoverability problem.
  'common.env': 'Settings',
  'set.search': 'Search settings',
  'set.general': 'General',
  'set.sessions': 'Sessions',
  'set.terminal': 'Terminal',
  'set.advanced': 'Advanced',
  'set.prompting': 'Opening prompt',
  'set.promptingHint':
    'Appended to every attempt’s first prompt: the branch, the base, and where to commit.',
  'set.openTemplate': 'Open the template',
  'set.licenses': 'Third-party licenses',
  /* --- Why the setting you are looking for is not here. Each of these sits
     where the search for it ends, rather than in the README. --- */
  'note.agents':
    'Models, credentials, reasoning and sandbox behaviour live in each agent’s own config. Marol never holds a credential file.',
  'note.scrollback':
    'Scrollback is not written to disk and does not survive a restart. Marol keeps no copy.',
  'note.telemetry':
    'No telemetry switch because nothing is collected: no usage data, no crash reports, no account.',
  /* Joiners live in the catalog: a hardcoded 、 or ， reads Chinese
     punctuation into an English sentence, and vice versa. `sep` joins
     label-and-state phrases (aria-labels); `listSep` joins list items. */
  'common.sep': ', ',
  'common.listSep': ', ',

  /* ------------------------------ boot -------------------------------- */
  'boot.node': 'Node 20+ on your login shell PATH',
  'boot.sidecar': 'The sidecar built:',
  'boot.claude': 'Claude Code CLI installed and signed in',
  'boot.retry': 'Retry',
  'boot.resolving': 'Resolving the login shell environment…',

  /* -------------------------- new session ----------------------------- */
  'newSession.title': 'New session',
  'newSession.cwd': 'Working directory',
  'newSession.args': 'Launch arguments (optional)',
  'newSession.argsHint': 'Globs and $VARs are not expanded.',
  'newSession.submit': 'Open terminal',

  /* ---------------------------- new card ------------------------------ */
  'newTask.titleLabel': 'Title (optional)',
  // Sits in the optional field itself, at the moment the decision is made.
  'newTask.titleHint': 'The prompt’s first line',
  'newTask.promptLabel': 'What the agent should do',
  'newTask.repo': 'Repo',
  'newTask.repoHint': 'A local path, wsl://<distro>/<path>, or ssh://<host>/<path>.',
  'newTask.base': 'Base branch',
  'newTask.addRepo': 'Add another repo',
  'newTask.dropRepo': 'Remove',
  // The two ways out of the one dialog that used to be two. Starting is the
  // primary because it is what nearly every card is for; filing it in the
  // backlog is the planning path, kept and demoted.
  'newTask.createAndStart': 'Create and start',
  'newTask.createOnly': 'Add to backlog',
  'newTask.repoN': 'Repo {n}',
  'newTask.baseN': 'Base branch {n}',
  // The card's badge when it spans more than one: the first repo's name and
  // how many others ride with it.
  'board.repoPlus': '{name} +{n}',
  // 建卡成功後由 say() 唸出：對話框關掉、焦點跳到新卡片,這一刻只有
  // 視覺鏈知道發生了什麼 —— 朗讀鏈在這裡補上確認。
  'newTask.created': 'Card created: {title}',

  /* -------------------------- start attempt --------------------------- */
  'attempt.startTitle': 'Start attempt: {title}',
  'attempt.agent': 'Agent',
  'attempt.firstPrompt': 'First prompt',
  // Every measured CLI asks for folder trust, not only Claude Code.
  'attempt.trustHint': 'Sent after you answer the folder-trust prompt.',
  // A refusal owes its reason, and stops there.
  'attempt.unmeasuredHint':
    'Not sent automatically: {agent}’s argument conventions are unmeasured. The session still opens.',
  'attempt.copied': 'Copied',
  'attempt.copyPrompt': 'Copy prompt',
  'attempt.openNoPrompt': 'Open session (no prompt)',
  // The safety boundary, stated as mechanism: this is the most important
  // disclosure in the app, and it is made every time the mode is chosen.
  'attempt.yoloHint':
    'No permission prompts. Changes land only in this attempt’s worktree, never your checkout.',

  /* -------------------------- permission modes ------------------------- */
  'mode.normal': 'Ask each time',
  'mode.accept_edits': 'Auto-accept edits',
  'mode.yolo': 'Full auto (yolo)',

  /* ---------------------------- sidebar ------------------------------- */
  'sidebar.newSession': 'New session',
  'sidebar.waitingCount': '{count} waiting on you',
  'sidebar.empty': 'No sessions yet',
  'sidebar.markDone': 'Mark as done',
  'sidebar.unmarkDone': 'Clear done',
  'sidebar.closeTerminal': 'Close terminal',
  'sidebar.removeFromList': 'Remove from list',
  // The rename input has no label beside it to point at, so it borrows this
  // sentence — the same arrangement the tab strip's own rename uses.
  'sidebar.rename': 'Session name',
  'sidebar.renameHint': 'Rename (F2)',
  // Both directions get their own sentence rather than one "Toggle sidebar":
  // the button says what the click will do, which is the half a person needs.
  'sidebar.fold': 'Collapse the sidebar',
  'sidebar.unfold': 'Expand the sidebar',

  /* ---------------------------- overview ------------------------------ */
  'overview.noAction': 'Nothing in progress',
  'overview.noStatus': 'This agent does not report status',
  /* ----------------------------- panes -------------------------------- */
  // Both drop zones are invisible gestures, so they stay.
  'pane.dragHint': 'Drop on another pane’s centre to swap, on an edge to split',
  'pane.restore': 'Restore',
  'pane.zoom': 'Zoom to full',
  'pane.remove': 'Remove from layout',
  'pane.empty': 'Drag a session in from the left, or click one',
  'pane.emptyFirstRun': 'Press ＋ (top left) for a session, or add a card on the board',
  // 第一次的空終端牆:三行認鍵卡的說明文字。和弦本身由 chord() 依平台
  // 組出來,所以型錄裡只放「這顆鍵做什麼」。
  'pane.keymap1': 'terminal · board · overview',
  'pane.keymap2': 'command palette',
  'pane.keymap3': 'shortcuts',

  /* --------------------------- shortcuts ------------------------------ */
  'keys.title': 'Keyboard shortcuts',
  'keys.jump': 'Jump to the session waiting on you',
  'keys.last': 'Previous session',
  'keys.palette': 'Command palette',
  'keys.cyclePanes': 'Next / previous pane',
  'keys.moveCard': 'Move the focused card',
  'keys.cycleTabs': 'Next / previous tab',
  'keys.sidebar': 'Collapse / expand the sidebar',
  'keys.agentOwn': 'The agent’s own view',
  // Named as what it holds rather than what it is called, because "transcript
  // overlay" is the CLI's word and "everything said so far" is the question.
  'keys.codexTranscript': 'Everything said so far, in a view you can scroll',
  'keys.codexPager': 'Move around inside it (q closes)',
  'keys.agentOwnNote':
    'These belong to the CLI, not to Marol, so Marol cannot change them. The wheel scrolls whatever the agent on screen calls scrolling.',
  'keys.inspector': 'Toggle the inspector',
  'keys.escape': 'Close the dialog',
  'keys.shellNote': 'Inside a terminal, Marol’s shortcuts take Shift: Ctrl+Shift+E, not Ctrl+E.',
  'attempt.modeLabel': 'Permission mode',
  'attempt.acceptHint': 'Other actions still ask.',
  'splitter.hint': 'Drag to resize; double-click to reset to equal',
  'keys.gestures': 'Mouse and gestures',
  'gesture.pane': 'Pane header',
  'gesture.tab': 'Workspace tab',
  'gesture.tabWhat': 'Enter, F2 or double-click to rename',
  'gesture.splitter': 'Splitter',
  'gesture.row': 'Sidebar row',
  'gesture.rowWhat': 'Drag it into the grid',

  /* ----------------------------- palette ------------------------------ */
  'palette.placeholder': 'Search sessions, cards, actions…',
  'palette.sessions': 'Sessions',
  'palette.cards': 'Cards',
  'palette.actions': 'Actions',
  'palette.empty': 'Nothing matches',
  'palette.compose': 'Make this a card',

  /* ------------------------------ tabs -------------------------------- */
  'tabs.rename': 'Tab name',
  'tabs.busy': 'Running',
  'tabs.close': 'Close tab',
  'tabs.new': 'New tab',
  'tabs.defaultName': 'Workspace {n}',
  'tabs.strip': 'Workspace tabs',

  /* --------------------------- column picker -------------------------- */
  'cols.label': 'Columns',
  'cols.auto': 'Auto',
  'cols.one': '1 col',
  'cols.n': '{n} cols',
  'cols.custom': 'Custom',
  'cols.manualHint': 'Choosing a count discards this hand-built layout',

  /* ------------------------------ board ------------------------------- */
  'board.newCard': 'New card',
  'board.emptyBacklog': 'Add a card',
  'board.emptyDrop': 'Drag a card here',
  'board.concurrency': 'Running / limit',
  'board.less': 'Lower the limit',
  'board.more': 'Raise the limit',
  'board.queued': '· {count} queued',
  'board.start': 'Start',
  'board.cancelQueue': 'Leave the queue',
  'board.resume': 'Resume',
  'board.inspect': 'Inspect',
  'board.retry': 'Try again',
  'board.switchAgent': 'Switch agent',
  'board.retryHint': 'Opens another attempt on this card',
  'board.deleteCard': 'Delete card',
  'board.confirmDelete': 'Delete for good?',
  'board.deleteBusy': 'The agent is mid-turn. Deleting takes its session and worktree',
  'board.movedTo': '{title} moved to {col}',
  'board.reordered': '{title} moved to position {n}',
  // 螢幕閱讀器聽的「⚠」:卡片的 aria-label 用這個詞,不用圖形字元 ——
  // 朗讀器對 ⚠ 的處理不可靠,一個真正的詞才保證被唸出來。
  'board.needsYou': 'needs you',
  'announce.multi': '{count} sessions waiting on you: {titles}',
  'announce.finished': '{title} finished a turn',
  /* An error says what happened. It does not then explain the person's own
     trade back to them — where .git lives, what branches are usually called,
     that a wrong path could be checked. Whoever is running coding agents
     knows; the sentence that teaches it is the one that condescends. */
  'err.notDir': 'This path does not exist, or is not a folder.',
  'err.notGitRepo': 'This folder is not a git repository.',
  'err.noBranch': 'The repository has no branch named "{branch}".',
  'err.details': 'Details',
  'env.diagnostics': 'Diagnostics',
  'sidebar.title': 'Sessions',
  'toast.more': '{count} earlier · clear all',

  /* ----------------------------- theme -------------------------------- */
  'env.theme': 'Theme',
  'theme.ink': 'Ink',
  'theme.paper': 'Paper',
  'theme.pine': 'Pine',
  'theme.wisteria': 'Wisteria',
  'theme.sunset': 'Sunset',
  'theme.custom': 'Custom',
  'theme.customHint':
    'The other shades derive from these. Each chip is that text tier’s contrast on its background; 4.5 is the AA minimum.',
  'theme.bg': 'Background',
  'theme.fg': 'Text',
  'theme.accent': 'Accent',
  'theme.ok': 'Success',
  'theme.warn': 'Warning',
  'theme.err': 'Error',
  'theme.light': 'Light theme',
  'theme.cText': 'Body',
  'theme.cDim': 'Secondary',
  'theme.cFaint': 'Faintest',
  'theme.cAccent': 'Primary button',

  /* ---------------------------- lifecycle ----------------------------- */
  'lifecycle.backlog': 'Backlog',
  'lifecycle.running': 'In progress',
  'lifecycle.review': 'Review',
  'lifecycle.done': 'Done',
  'lifecycle.abandoned': 'Abandoned',

  /* ----------------------------- outcome ------------------------------ */
  'outcome.merged': 'Merged',
  'outcome.discarded': 'Discarded',
  'outcome.superseded': 'Superseded',

  /* ------------------------------ live -------------------------------- */
  'live.notStarted': 'Not started',
  'live.queued': 'Queued · #{position}',
  'live.stopped': 'Not running',
  'live.parked': 'Parked',
  'live.ended': 'Ended',

  /* ----------------------------- status ------------------------------- */
  'status.starting': 'Starting',
  'status.awaiting_trust': 'Waiting on folder trust',
  'status.running': 'Running',
  'status.waiting_permission': 'Waiting on permission',
  'status.waiting_input': 'Waiting on you',
  'status.idle': 'Idle',
  'status.saved': 'Closed',
  // The app was closed; the agent was not. Named for what is true of the
  // work, not for what the window did — and true in both halves of that
  // state: before the card is opened, and in the moment after it reattaches
  // when tmux has the agent but no hook has spoken yet. "Unwatched" was only
  // right for the first half.
  'status.detached': 'Running, not reporting',
  'status.exited': 'Exited',

  /* ----------------------------- sections ----------------------------- */
  'section.working': 'Working',
  'section.idle': 'Idle',
  'section.done': 'Done',

  /* ------------------------------ unseen ------------------------------ */
  'unseen.label': 'Finished, unseen',

  /* ----------------------------- welcome ------------------------------ */
  'welcome.title': 'Welcome to Marol',
  'welcome.found': 'Detected',
  'welcome.model': 'How it works',
  'welcome.model1': 'A card is a repo, a base branch, and something to do.',
  'welcome.model2':
    'An attempt runs in its own git worktree and terminal. The agent can only touch its branch, never your checkout.',
  'welcome.model3':
    'Finishing merges, opens a PR, or discards. The diff is frozen and kept.',
  'welcome.newCard': 'Create the first card',
  'welcome.newSession': 'Open an ad-hoc session',
  'welcome.reopen': 'Show the welcome panel',
  // The interface says what a control does; the README says why. This is the
  // door to it — without one, shortening the interface loses the reason.
  'env.docs': 'Documentation',
  'coach.replay': 'Show the first-run tip again',
  'coach.replayed': 'Reset ✓',
  // 一台沒有任何 agent CLI 的機器:卡片照開,attempt 需要 CLI ——
  // 界線照實說,不假裝一切就緒,也不擋人建卡。
  'welcome.noAgents':
    'No agent CLI on this shell’s PATH. Cards still work; an attempt needs claude, codex, gemini or aider installed and signed in.',
  'welcome.probeAgain': 'Probe again',

  /* ------------------------------ coach ------------------------------- */
  'coach.gotIt': 'Got it',
  // The one mark left: a keyboard trap nothing on screen can show.
  'coach.terminal.title': 'Shortcuts here take Shift',
  'coach.terminal.body':
    'Ctrl+letter goes to the shell, so Marol’s shortcuts take Shift: Ctrl+Shift+E.',

  /* ------------------------------ stats ------------------------------- */
  'stats.ahead': '{n} commits {branch} does not have yet',
  'stats.behind': '{branch} is {n} commits ahead; rebase before merging',
  'stats.hint': 'Lines changed vs {branch} · ↑ commits ahead · ↓ commits behind',

  /* ---------------------------- inspector ----------------------------- */
  'inspector.changes': 'Changes',
  'inspector.activity': 'Activity',
  'inspector.knows': 'Rules',
  'knows.project': 'From this repo',
  'knows.global': 'From this machine',
  'knows.shared': 'all agents',
  'knows.absent': 'not created',
  'inspector.reload': 'Reload',
  'inspector.closeView': 'Close inspector',
  'inspector.frozen': 'Frozen',
  'inspector.mergeInto': 'Merge into {branch}',
  'inspector.merged': 'Merged into {branch}',
  'inspector.confirmDiscard': 'Discard for good?',
  'inspector.confirmMerge': 'Really merge into {branch}?',
  'inspector.working': 'Working…',
  'inspector.frozenHint': 'Ended. The diff is kept, read-only',
  'inspector.openPr': 'Push + open PR',
  'inspector.discard': 'Discard',
  'inspector.discardHint': 'Deletes the worktree. The diff is kept, read-only',
  'inspector.noChanges': 'No changes yet',
  'inspector.noActivity': 'No activity yet',
  'inspector.eventsFailed': 'Could not read the activity: {err}',
  'inspector.diffSummary': '{files} files',
  'inspector.readAt': 'read {time}',
  'inspector.copyUrl': 'Copy link',
  'inspector.jumpLabel': 'Jump to a file',
  'inspector.viewedCount': '· viewed {seen}/{files}',
  'inspector.wrap': 'Wrap long lines',
  'inspector.markViewed': 'Mark as viewed',
  'inspector.unmarkViewed': 'Viewed',
  'inspector.resize': 'Resize the inspector',

  /* --------------------------- next action ----------------------------- */
  /* One reserved line on the card, so these three say the whole thing in
     roughly thirty characters. The inspector's banner uses the same keys and
     has room to spare; the card is the constraint, and the card is where
     they are read during triage. */
  'next.commit': 'Commit first',
  'next.rebase': '{branch} is {n} commits ahead; rebase first',
  'next.finish': 'Merge or open a PR',
  'inspector.runHint': 'Run `{name}` in a new terminal',
  'inspector.worktreeGroup': 'worktree',
  'inspector.shell': 'shell',
  'inspector.queued': 'A message will send when this turn ends',
  'timeline.waited': '· held {for}',

  /* ----------------------------- review ------------------------------- */
  'review.placeholder': 'What should change here?',
  'review.add': 'Add feedback',
  'review.remove': 'Remove',
  'review.send': 'Send {count} to the agent',
  'review.queue': 'Send {count} when this turn ends',
  'review.copy': 'Copy feedback',
  'review.header': '[Marol review] Feedback on the current diff:',
  'review.footer': 'Address each point, then commit on this branch.',

  /* ------------------------------ env --------------------------------- */
  'env.shell': 'shell',
  'env.source': 'environment source',
  'env.sourceLogin': 'login shell ✓',
  // Windows: the process env *is* the user's real environment — no shell
  // probe happens and nothing is degraded.
  'env.sourceSystem': 'system environment ✓',
  'env.sourceProcess': 'process environment (degraded)',
  'env.varCount': 'variables',
  'env.claude': 'claude',
  'env.codex': 'codex',
  'env.cliMissing': 'not found',
  // Found and wired for status are different facts: a CLI that is here but
  // too old to be wired runs perfectly well and tells this desk nothing.
  'env.cliReports': 'status ✓',
  'env.cliQuiet': 'no status',
  'env.db': 'database',
  'env.version': 'this build',
  'env.degraded':
    'Could not read the login shell environment, so this process’s own was used instead. npx-style MCP servers may fail to start.',

  /* --------------------------- folder picker -------------------------- */
  'pick.title': 'Choose a directory',
  'pick.path': 'Path',
  'pick.empty': 'No subdirectories here.',
  'pick.isRepo': '✓ a git repository',

  /* ------------------------------ updates ----------------------------- */
  'set.updates': 'Updates',
  'up.section': 'Updates',
  'up.check': 'Check now',
  'up.checking': 'Checking…',
  'up.current': 'Marol {version} — the newest there is.',
  'up.found': 'Marol {version} is out.',
  'up.notes': 'Release notes',
  'up.apply': 'Download and restart',
  'up.applying': 'Downloading… {pct}%',
  'up.swapping': 'Installing — the app will restart itself.',
  'up.enabled': 'Check for updates',
  // Not telemetry, and the sentence says exactly what leaves the machine so
  // that claim is checkable rather than asserted.
  'up.enabledHint':
    'Asks GitHub for the latest release number, once a day. Nothing about this machine is sent, and this is the only request Marol makes on its own behalf.',
  'up.never': 'not yet',
  'up.lastCheck': 'Last checked',
  // The absence, said where somebody went looking for the button.
  'up.unconfigured':
    'This build carries no update key, so it cannot verify a download. Updates are downloaded from the releases page by hand.',
  'up.managed':
    'This copy was installed by a package manager, which keeps its own record of the files it owns. Update it the way you installed it.',
  'up.openReleases': 'Open the releases page',
  // The two facts a restart turns on, kept separate because only one of them
  // is a cost.
  'up.held': '{n} agent session(s) will be handed back after the restart.',
  'up.lost': '{n} agent session(s) will end — nothing in their world is holding them.',
  'up.lostConfirm': 'End them and update',
  'up.backup': 'A copy of the board goes to {path} first, so this is reversible.',
  'up.newVersion': 'Marol {version} available',
  'env.language': 'Language',
  'env.messaging': 'Cross-session messaging',
  'env.messagingOff': 'needs Claude Code ≥ 2.1.224 (found {version})',
  'env.profiles': 'Agent profiles',
  'env.profilesHint': 'A CLI plus the flags it always gets.',
  'env.notifications': 'Notifications',
  'notify.hint': 'Only sent while the window is in the background.',
  'notify.permission': 'Permission and folder-trust prompts',
  'notify.input': 'Waiting on you',
  'notify.done': 'A turn finished',
  'notify.test': 'Send a test notification',
  'notify.sent': 'Sent ✓',

  /* ------------------- terminal screen reader mode --------------------- */
  'env.termSr': 'Terminal screen reader mode',
  // A real performance trade the reader is about to make and cannot see.
  'termSr.hint':
    'Makes all terminal text readable to a screen reader, permission prompts included. Disables GPU rendering, so heavy output scrolls less smoothly.',
  'termSr.toggle': 'Expose terminal text to screen readers',

  /* --------------------------- checkpoints ----------------------------- */
  'env.checkpoints': 'Checkpoints',
  // Every clause is a guarantee about what is touched and what is not.
  'ckpt.hint':
    'Kept in private git refs and deleted when the attempt ends. Your branches, index and stash are never touched.',
  'ckpt.onStop': 'Snapshot when a turn ends (Claude Code sessions)',
  'inspector.ckpt': 'Checkpoint',
  'inspector.ckptHint': 'Snapshot this worktree now',
  'inspector.ckptMade': 'Checkpoint #{n} ✓',
  'inspector.ckptNone': 'No changes since the last checkpoint',
  'ckpt.restoreHint': 'Restore the code to before this turn; the conversation stays',
  'ckpt.restoreArm': 'Restore to before this turn?',
  'ckpt.blocked': 'The agent is mid-turn',
  'ckpt.restored': 'Restored to checkpoint #{n}. The pre-restore state was snapshotted first.',
  'ckpt.restoredBase': 'Restored to the attempt’s base. The pre-restore state was snapshotted first.',
  'ckpt.tell': 'Tell the agent',
  // Agent-facing: the imperative register is correct here.
  'ckpt.note':
    'This worktree was restored to an earlier checkpoint. Re-read any file before editing it.',
  'board.park': 'Park',
  'board.parkHint':
    'Frees the worktree and its run slot. Branch, checkpoints and conversation are kept',
  'park.done': 'Parked. {branch} copied.',
  'park.restoreFailed':
    'Resumed, but restoring the parked work failed: {err}. Restore it from the timeline.',
  'park.restoreParked': 'Parked. Resume first, then restore',
  /* ---------------------------- preview ------------------------------ */
  'preview.title': 'Dev server preview',
  'preview.open': 'Preview',
  // A disabled control owes its reason, and stops there.
  'preview.sshHint': 'The port is on the remote host, not reachable from here',
  'preview.copy': 'Copy',
  'preview.reload': 'Reload',
  'preview.external': 'Open in browser',
  'preview.dead': 'The dev server’s terminal closed.',
  'preview.notListening': 'Nothing is listening on {url}.',
  'preview.retry': 'Check again',
  'preview.pick': '{component} · {file}:{line}',
  // Agent-facing: sentence 2 tells it to expect a follow-up, not to act.
  'preview.note':
    'In the preview I am pointing at {component} ({file}:{line}). My next message is about it.',
  'inspector.diffKeys': 'j/k lines · n/p files · e edit · v viewed · Enter comment',
  'ckpt.compare': 'Compare with',
  'ckpt.compareBase': 'Base',
  'ckpt.compareN': 'Checkpoint #{n} · {time}',
  /* -------------------------- editable diff --------------------------- */
  'edit.hint': 'Edit this file',
  'edit.oneAtATime': 'Close the open editor first',
  'edit.save': 'Save',
  'edit.saveHint': 'Save to {file} (⌘S)',
  'edit.saved': 'Saved ✓',
  'edit.close': 'Close',
  'edit.note': 'I hand-edited {file}. Re-read it before continuing.',
  'edit.failed': 'Could not read {file}: {err}',
  'edit.discardTitle': 'Unsaved changes',
  'edit.discardBody': 'Close the editor and lose the edits to {file}?',
  'edit.discard': 'Discard',
  'edit.keep': 'Keep editing',
  'edit.compareLocked': 'Close the editor to switch the baseline',
  'review.stale': 'line changed',
  'review.staleHint':
    'The quoted line is no longer in the diff. The note still sends with the original quote',
  /* ----------------------------- worlds ------------------------------- */
  'world.local': 'This machine',
  'world.where': 'Host',
  'world.pick': 'Where new cards and sessions open',
  'world.probing': 'checking…',
  'world.noAgent': 'no claude or codex on this host’s PATH',
  /* ------------------------ find in terminal -------------------------- */
  'term.find': 'Find in terminal',
  'term.findHint': 'Enter next match, Shift+Enter previous, Esc closes',
  'term.prev': 'Previous match',
  'term.next': 'Next match',
  'term.noMatch': 'No match',
  'keys.find': 'Find in the focused terminal',
  /* --------------------------- token account -------------------------- */
  'usage.line': 'context {ctx} · output {out}',
  // Four measured numbers shown nowhere else, plus the staleness caveat.
  'usage.tip':
    'Read from the transcript at each turn’s end. Context {context} is the last request’s prompt. Cumulative: {input} in · {output} out · {write} cache-written · {read} cache-read',

  /* ----------------------------- profiles ------------------------------ */
  'profile.namePlaceholder': 'opus, quiet-claude, …',
  'profile.add': 'Add profile',
  'profile.remove': 'Remove this profile',
  'profile.save': 'Save profiles',
  'profile.saved': 'Saved ✓',

  /* ------------------------------ views ------------------------------- */
  'view.overview': 'Overview',
  'view.board': 'Board',
  'view.inspector': 'Inspector',
  'view.terminal': 'Terminal',

  /* ------------------------------ errors ------------------------------ */
  'error.updateTab': 'Could not update the tab: {err}',
  'error.openSession': 'Could not open the session: {err}',
  'error.reopen': 'Could not reopen: {err}',
  'error.resumeAttempt': 'Could not resume the attempt: {err}',
  'error.moveCard': 'Could not move the card: {err}',
  'error.cancelQueue': 'Could not leave the queue: {err}',
  'error.park': 'Could not park the attempt: {err}',
  'error.deleteCard': 'Could not delete the card: {err}',
  'error.newTab': 'Could not add the tab: {err}',
  'error.runScript': 'Could not start the run script: {err}',
  'error.openShell': 'Could not open the worktree shell: {err}',
} as const;

export type MessageKey = keyof typeof en;

export const zhTW: Record<MessageKey, string> = {
  /* ------------------------------ shared ------------------------------ */
  'common.cancel': '取消',
  'common.close': '關閉',
  'common.open': '開啟',
  'common.create': '建立',
  'common.start': '開始',
  'common.choose': '選擇…',
  'common.loading': '讀取中…',
  'common.env': '設定',
  'set.search': '搜尋設定',
  'set.general': '一般',
  'set.sessions': 'Sessions',
  'set.terminal': '終端機',
  'set.advanced': '進階',
  'set.prompting': '起始 prompt',
  'set.promptingHint':
    '每個 attempt 的首則 prompt 都會附上：分支、base、commit 位置。',
  'set.openTemplate': '開啟模板',
  'set.licenses': '第三方授權',
  'note.agents':
    '模型、憑證、推理與沙箱設定都在各 agent 自己的設定檔裡。Marol 不代管任何憑證。',
  'note.scrollback':
    'Scrollback 不寫入磁碟，重開後不保留。Marol 不留副本。',
  'note.telemetry':
    '沒有遙測開關，因為不收集任何資料：沒有使用數據、沒有當機回報、沒有帳號。',
  'common.sep': '，',
  'common.listSep': '、',

  /* ------------------------------ boot -------------------------------- */
  'boot.node': 'login shell 的 PATH 上需有 Node 20+',
  'boot.sidecar': '需先建置 sidecar：',
  'boot.claude': '需已安裝並登入 Claude Code CLI',
  'boot.retry': '重試',
  'boot.resolving': '正在解析 login shell 環境…',

  /* -------------------------- new session ----------------------------- */
  'newSession.title': '新 session',
  'newSession.cwd': '工作目錄',
  'newSession.args': '啟動參數（選填）',
  'newSession.argsHint': '不會展開萬用字元與 $VAR。',
  'newSession.submit': '開啟終端機',

  /* ---------------------------- new card ------------------------------ */
  'newTask.titleLabel': '標題（選填）',
  'newTask.titleHint': 'prompt 的第一行',
  'newTask.promptLabel': '要 agent 做什麼',
  'newTask.repo': 'Repo',
  'newTask.repoHint': '本機路徑、wsl://<distro>/<path> 或 ssh://<host>/<path>。',
  'newTask.base': 'Base 分支',
  'newTask.addRepo': '再加一個 repo',
  'newTask.dropRepo': '移除',
  'newTask.createAndStart': '建立並開始',
  'newTask.createOnly': '放進待辦',
  'newTask.repoN': 'Repo {n}',
  'newTask.baseN': 'Base 分支 {n}',
  'board.repoPlus': '{name} +{n}',
  'newTask.created': '已建立卡片：「{title}」',

  /* -------------------------- start attempt --------------------------- */
  'attempt.startTitle': '開始 attempt：{title}',
  'attempt.agent': 'Agent',
  'attempt.firstPrompt': '首則 prompt',
  'attempt.trustHint': '回答資料夾信任提問後才送出。',
  'attempt.unmeasuredHint':
    '不會自動送出：{agent} 的參數慣例尚未實測。session 仍會開啟。',
  'attempt.copied': '已複製',
  'attempt.copyPrompt': '複製 prompt',
  'attempt.openNoPrompt': '開 session（不送 prompt）',
  'attempt.yoloHint':
    '完全不詢問權限。變更只會落在這個 attempt 的 worktree，碰不到你的 checkout。',

  /* -------------------------- permission modes ------------------------- */
  'mode.normal': '每次詢問',
  'mode.accept_edits': '自動接受檔案編輯',
  'mode.yolo': '全自動（yolo）',

  /* ---------------------------- sidebar ------------------------------- */
  'sidebar.newSession': '新 session',
  'sidebar.waitingCount': '{count} 個等你',
  'sidebar.empty': '尚無 session',
  'sidebar.markDone': '標記為完成',
  'sidebar.unmarkDone': '取消完成標記',
  'sidebar.closeTerminal': '關閉終端機',
  'sidebar.removeFromList': '從清單移除',
  'sidebar.rename': 'session 名稱',
  'sidebar.renameHint': '改名（F2）',
  'sidebar.fold': '收起側欄',
  'sidebar.unfold': '展開側欄',

  /* ---------------------------- overview ------------------------------ */
  'overview.noAction': '沒有進行中的動作',
  'overview.noStatus': '這個 agent 不回報狀態',

  /* ----------------------------- panes -------------------------------- */
  'pane.dragHint': '拖到別的窗格中央可對調，拖到邊緣可切分',
  'pane.restore': '還原',
  'pane.zoom': '放大到滿版',
  'pane.remove': '從佈局移除',
  'pane.empty': '把 session 從左側拖進來，或點選一個',
  'pane.emptyFirstRun': '按左上角的 ＋ 開 session，或到看板新增卡片',
  'pane.keymap1': '終端機 · 看板 · 總覽',
  'pane.keymap2': '命令面板',
  'pane.keymap3': '快捷鍵',

  /* --------------------------- shortcuts ------------------------------ */
  'keys.title': '鍵盤快捷鍵',
  'keys.jump': '跳到正在等你的 session',
  'keys.last': '上一個 session',
  'keys.palette': '命令面板',
  'keys.cyclePanes': '下一個 / 上一個窗格',
  'keys.moveCard': '搬動聚焦的卡片',
  'keys.cycleTabs': '下一個 / 上一個分頁',
  'keys.sidebar': '收起 / 展開側欄',
  'keys.agentOwn': 'agent 自己的畫面',
  'keys.codexTranscript': '目前為止說過的全部，在一個可以捲動的畫面裡',
  'keys.codexPager': '在裡面移動（q 關閉）',
  'keys.agentOwnNote':
    '這些鍵屬於 CLI 不屬於 Marol，Marol 改不動它們。滾輪捲動的是畫面上那個 agent 自己認定的捲動。',
  'keys.inspector': '開關檢視器',
  'keys.escape': '關閉對話框',
  'keys.shellNote': '在終端機內，Marol 的快捷鍵需加 Shift：Ctrl+Shift+E，不是 Ctrl+E。',
  'attempt.modeLabel': '權限模式',
  'attempt.acceptHint': '其他動作仍會詢問。',
  'splitter.hint': '拖曳調整比例；雙擊還原等分',
  'keys.gestures': '滑鼠與手勢',
  'gesture.pane': '窗格標頭',
  'gesture.tab': '工作區分頁',
  'gesture.tabWhat': 'Enter、F2 或雙擊改名',
  'gesture.splitter': '分隔線',
  'gesture.row': '側欄列',
  'gesture.rowWhat': '拖進網格',

  /* ----------------------------- palette ------------------------------ */
  'palette.placeholder': '搜尋 session、卡片、動作…',
  'palette.sessions': 'Sessions',
  'palette.cards': '卡片',
  'palette.actions': '動作',
  'palette.empty': '沒有符合的',
  'palette.compose': '建立成卡片',

  /* ------------------------------ tabs -------------------------------- */
  'tabs.rename': '分頁名稱',
  'tabs.busy': '執行中',
  'tabs.close': '關閉分頁',
  'tabs.new': '新分頁',
  'tabs.defaultName': '工作區 {n}',
  'tabs.strip': '工作區分頁',

  /* --------------------------- column picker -------------------------- */
  'cols.label': '欄數',
  'cols.auto': '自動',
  'cols.one': '1 欄',
  'cols.n': '{n} 欄',
  'cols.custom': '自訂',
  'cols.manualHint': '選欄數會捨棄手排的佈局',

  /* ------------------------------ board ------------------------------- */
  'board.newCard': '新增卡片',
  'board.emptyBacklog': '新增一張卡片',
  'board.emptyDrop': '把卡片拖到這裡',
  'board.concurrency': '執行中 / 上限',
  'board.less': '降低上限',
  'board.more': '提高上限',
  'board.queued': '· {count} 個排隊中',
  'board.start': '開始',
  'board.cancelQueue': '離開排隊',
  'board.resume': '繼續',
  'board.inspect': '檢視',
  'board.retry': '再試一次',
  'board.switchAgent': '換 agent',
  'board.retryHint': '在這張卡上再開一個 attempt',
  'board.deleteCard': '刪除卡片',
  'board.confirmDelete': '確定刪除？',
  'board.deleteBusy': 'agent 回合進行中，刪除會一併移除 session 與 worktree',
  'board.movedTo': '{title} 移到 {col}',
  'board.reordered': '{title} 移到第 {n} 位',
  'board.needsYou': '需要你',
  'announce.multi': '{count} 個 session 等你：{titles}',
  'announce.finished': '「{title}」回合結束',
  'err.notDir': '這個路徑不存在，或者不是資料夾。',
  'err.notGitRepo': '這個資料夾不是 git repository。',
  'err.noBranch': '這個 repository 沒有名為「{branch}」的分支。',
  'err.details': '詳細',
  'env.diagnostics': '診斷',
  'sidebar.title': 'Sessions',
  'toast.more': '較早的 {count} 則 · 全部清除',

  /* ----------------------------- theme -------------------------------- */
  'env.theme': '主題',
  'theme.ink': '墨',
  'theme.paper': '紙',
  'theme.pine': '松',
  'theme.wisteria': '紫藤',
  'theme.sunset': '落日',
  'theme.custom': '自訂',
  'theme.customHint':
    '其餘色階由這幾色推導。每個色塊是該階文字對背景的對比度，AA 最低 4.5。',
  'theme.bg': '背景',
  'theme.fg': '文字',
  'theme.accent': '強調色',
  'theme.ok': '成功',
  'theme.warn': '警告',
  'theme.err': '錯誤',
  'theme.light': '淺色主題',
  'theme.cText': '內文',
  'theme.cDim': '次要',
  'theme.cFaint': '最淡',
  'theme.cAccent': '主要按鈕',

  /* ---------------------------- lifecycle ----------------------------- */
  'lifecycle.backlog': '待辦',
  'lifecycle.running': '進行中',
  'lifecycle.review': '待檢視',
  'lifecycle.done': '已完成',
  'lifecycle.abandoned': '已放棄',

  /* ----------------------------- outcome ------------------------------ */
  'outcome.merged': '已合併',
  'outcome.discarded': '已丟棄',
  'outcome.superseded': '已被取代',

  /* ------------------------------ live -------------------------------- */
  'live.notStarted': '尚未開始',
  'live.queued': '排隊中 · 第 {position} 個',
  'live.stopped': '未執行',
  'live.parked': '已擱置',
  'live.ended': '已結案',

  /* ----------------------------- status ------------------------------- */
  'status.starting': '啟動中',
  'status.awaiting_trust': '等你確認資料夾',
  'status.running': '執行中',
  'status.waiting_permission': '等你授權',
  'status.waiting_input': '等你',
  'status.idle': '待命',
  'status.saved': '已關閉',
  'status.detached': '執行中，無回報',
  'status.exited': '已結束',

  /* ----------------------------- sections ----------------------------- */
  'section.working': '執行中',
  'section.idle': '待命',
  'section.done': '已完成',

  /* ------------------------------ unseen ------------------------------ */
  'unseen.label': '已完成未看',

  /* ----------------------------- welcome ------------------------------ */
  'welcome.title': '歡迎使用 Marol',
  'welcome.found': '偵測結果',
  'welcome.model': '運作方式',
  'welcome.model1': '一張卡片 = 一個 repo、一個 base 分支、一件要做的事。',
  'welcome.model2':
    '每個 attempt 都在自己的 git worktree 與終端機裡執行，agent 只碰得到自己的分支，碰不到你的 checkout。',
  'welcome.model3': '結束時合併、開 PR 或丟棄。diff 都會凍結保留。',
  'welcome.newCard': '開第一張卡',
  'welcome.newSession': '開啟臨時 session',
  'welcome.reopen': '顯示歡迎面板',
  'env.docs': '說明文件',
  'coach.replay': '重新顯示首次提示',
  'coach.replayed': '已重設 ✓',
  'welcome.noAgents':
    '這個 shell 的 PATH 上找不到 agent CLI。卡片仍可建立，開始 attempt 需先安裝並登入 claude、codex、gemini 或 aider。',
  'welcome.probeAgain': '重新偵測',

  /* ------------------------------ coach ------------------------------- */
  'coach.gotIt': '知道了',
  'coach.terminal.title': '這裡的快捷鍵要加 Shift',
  'coach.terminal.body':
    'Ctrl+字母會進到 shell，所以 Marol 的快捷鍵要加 Shift：Ctrl+Shift+E。',

  /* ------------------------------ stats ------------------------------- */
  'stats.ahead': '有 {n} 個 {branch} 還沒有的 commit',
  'stats.behind': '{branch} 已前進 {n} 個 commit，合併前請先 rebase',
  'stats.hint': '相對 {branch} 的行數變更 · ↑ 領先的 commit · ↓ 落後的 commit',

  /* ---------------------------- inspector ----------------------------- */
  'inspector.changes': '變更',
  'inspector.activity': '活動',
  'inspector.knows': '規則檔',
  'knows.project': '來自這個 repo',
  'knows.global': '來自這台機器',
  'knows.shared': '所有 agent',
  'knows.absent': '尚未建立',
  'inspector.reload': '重新讀取',
  'inspector.closeView': '關閉檢視器',
  'inspector.frozen': '已凍結',
  'inspector.mergeInto': '合併回 {branch}',
  'inspector.merged': '已合併回 {branch}',
  'inspector.confirmDiscard': '確定丟棄？',
  'inspector.confirmMerge': '確定合併回 {branch}？',
  'inspector.working': '處理中…',
  'inspector.frozenHint': '已結束，diff 唯讀保留',
  'inspector.openPr': 'push + 開 PR',
  'inspector.discard': '丟棄',
  'inspector.discardHint': '刪除 worktree，diff 唯讀保留',
  'inspector.noChanges': '尚無變更',
  'inspector.noActivity': '尚無活動',
  'inspector.eventsFailed': '讀取活動失敗：{err}',
  'inspector.diffSummary': '{files} 個檔案',
  'inspector.readAt': '{time} 讀取',
  'inspector.copyUrl': '複製連結',
  'inspector.jumpLabel': '跳到檔案',
  'inspector.viewedCount': '· 已看 {seen}/{files}',
  'inspector.wrap': '長行折行',
  'inspector.markViewed': '標為已看',
  'inspector.unmarkViewed': '已看',
  'inspector.resize': '調整檢視器寬度',

  /* --------------------------- next action ----------------------------- */
  'next.commit': '先 commit',
  'next.rebase': '{branch} 已前進 {n} 個 commit，請先 rebase',
  'next.finish': '可合併或開 PR',
  'inspector.runHint': '在新終端機執行 `{name}`',
  'inspector.worktreeGroup': 'worktree',
  'inspector.shell': 'shell',
  'inspector.queued': '一則訊息會在這個回合結束後送出',
  'timeline.waited': '· 等候 {for}',

  /* ----------------------------- review ------------------------------- */
  'review.placeholder': '這裡該怎麼改？',
  'review.add': '加入意見',
  'review.remove': '移除',
  'review.send': '送出 {count} 則意見給 agent',
  'review.queue': '這輪結束後送出 {count} 則',
  'review.copy': '複製意見',
  'review.header': '[Marol review] 對目前 diff 的意見：',
  'review.footer': '請逐點修改，然後 commit 到這個分支。',

  /* ------------------------------ env --------------------------------- */
  'env.shell': 'shell',
  'env.source': '環境來源',
  'env.sourceLogin': 'login shell ✓',
  'env.sourceSystem': '系統環境 ✓',
  'env.sourceProcess': '行程環境（降級）',
  'env.varCount': '變數數量',
  'env.claude': 'claude',
  'env.codex': 'codex',
  'env.cliMissing': '找不到',
  'env.cliReports': '狀態回報 ✓',
  'env.cliQuiet': '沒有狀態回報',
  'env.db': '資料庫',
  'env.version': '目前版本',
  'env.degraded':
    '無法從 login shell 取得環境，已退回本行程的環境。npx 型的 MCP server 可能起不來。',

  /* --------------------------- folder picker -------------------------- */
  'pick.title': '選擇資料夾',
  'pick.path': '路徑',
  'pick.empty': '這裡沒有子資料夾。',
  'pick.isRepo': '✓ 是一個 git repo',

  /* ------------------------------ updates ----------------------------- */
  'set.updates': '更新',
  'up.section': '更新',
  'up.check': '立刻檢查',
  'up.checking': '檢查中…',
  'up.current': 'Marol {version}，已經是最新的了。',
  'up.found': 'Marol {version} 出來了。',
  'up.notes': '版本說明',
  'up.apply': '下載並重啟',
  'up.applying': '下載中… {pct}%',
  'up.swapping': '安裝中，裝好會自己重啟。',
  'up.enabled': '檢查更新',
  'up.enabledHint':
    '每天一次，向 GitHub 問最新的版本號。不會送出這台機器的任何資料，而且這是 Marol 唯一一個為自己發出的請求。',
  'up.never': '還沒查過',
  'up.lastCheck': '上次檢查',
  'up.unconfigured': '這個 build 沒有帶更新用的金鑰，無法驗證下載的檔案。請到 releases 頁自己下載。',
  'up.managed': '這份是套件管理員裝的，它自己記著裝了哪些檔案。請用當初安裝的方式更新。',
  'up.openReleases': '開啟 releases 頁',
  'up.held': '重啟後會有 {n} 個 agent session 被交回來。',
  'up.lost': '有 {n} 個 agent session 會結束 —— 它們所在的世界沒有東西接著。',
  'up.lostConfirm': '結束它們並更新',
  'up.backup': '會先把看板複製一份到 {path}，所以這步可以退回來。',
  'up.newVersion': 'Marol {version} 可更新',
  'env.language': '語言',
  'env.messaging': '跨 session 互傳訊息',
  'env.messagingOff': '需要 Claude Code ≥ 2.1.224（目前 {version}）',
  'env.profiles': 'Agent 設定檔',
  'env.profilesHint': '一個 CLI，加上固定帶的參數。',
  'env.notifications': '通知',
  'notify.hint': '只在視窗不在前景時送出。',
  'notify.permission': '授權與資料夾信任',
  'notify.input': '等你',
  'notify.done': '回合結束',
  'notify.test': '送一則測試通知',
  'notify.sent': '已送出 ✓',

  /* ------------------- terminal screen reader mode --------------------- */
  'env.termSr': '終端機螢幕閱讀器模式',
  'termSr.hint':
    '讓所有終端機文字（含授權提示）都能被螢幕閱讀器讀到。會關閉 GPU 繪製，大量輸出時捲動較不順。',
  'termSr.toggle': '把終端機文字提供給螢幕閱讀器',

  /* --------------------------- checkpoints ----------------------------- */
  'env.checkpoints': '檢查點',
  'ckpt.hint':
    '存在私有 git ref 裡，attempt 結束即刪。不動你的分支、index 與 stash。',
  'ckpt.onStop': '回合結束時自動快照（Claude Code session）',
  'inspector.ckpt': '檢查點',
  'inspector.ckptHint': '立即快照這個 worktree',
  'inspector.ckptMade': '檢查點 #{n} ✓',
  'inspector.ckptNone': '距上一個檢查點沒有變更',
  'ckpt.restoreHint': '把程式碼還原到本回合之前；對話不動',
  'ckpt.restoreArm': '確定還原到本回合之前？',
  'ckpt.blocked': 'agent 回合進行中',
  'ckpt.restored': '已還原到檢查點 #{n}。還原前的狀態已先快照。',
  'ckpt.restoredBase': '已還原到 attempt 的 base。還原前的狀態已先快照。',
  'ckpt.tell': '告訴 agent',
  'ckpt.note':
    '這個 worktree 已還原到較早的檢查點。編輯任何檔案前請先重讀。',
  'board.park': '擱置',
  'board.parkHint': '釋出 worktree 與執行名額，分支、檢查點、對話都保留',
  'park.done': '已擱置。已複製分支 {branch}。',
  'park.restoreFailed':
    '已繼續，但擱置的工作還原失敗：{err}。可從時間軸還原。',
  'park.restoreParked': '已擱置。先繼續，再還原',
  /* ---------------------------- preview ------------------------------ */
  'preview.title': 'Dev server 預覽',
  'preview.open': '預覽',
  'preview.sshHint': '這個埠在遠端主機上，從這裡連不到',
  'preview.copy': '複製',
  'preview.reload': '重新載入',
  'preview.external': '用瀏覽器開啟',
  'preview.dead': 'dev server 的終端機已關閉。',
  'preview.notListening': '{url} 上沒有東西在監聽。',
  'preview.retry': '重新檢查',
  'preview.pick': '{component} · {file}:{line}',
  'preview.note':
    '我在預覽中選取了 {component}（{file}:{line}）。接下來的意見都是針對這個元件。',
  'inspector.diffKeys': 'j/k 逐行 · n/p 逐檔 · e 編輯 · v 已看 · Enter 留言',
  'ckpt.compare': '比較基準',
  'ckpt.compareBase': 'Base',
  'ckpt.compareN': '檢查點 #{n} · {time}',
  /* -------------------------- editable diff --------------------------- */
  'edit.hint': '編輯這個檔案',
  'edit.oneAtATime': '先關閉目前的編輯器',
  'edit.save': '存檔',
  'edit.saveHint': '儲存到 {file}（⌘S）',
  'edit.saved': '已存檔 ✓',
  'edit.close': '關閉',
  'edit.note': '我手動改了 {file}，重讀後再繼續。',
  'edit.failed': '讀取 {file} 失敗：{err}',
  'edit.discardTitle': '有未存的變更',
  'edit.discardBody': '關閉編輯器並放棄對 {file} 的修改？',
  'edit.discard': '放棄',
  'edit.keep': '繼續編輯',
  'edit.compareLocked': '關閉編輯器才能切換比較基準',
  'review.stale': '行已變',
  'review.staleHint': '引用的那行已不在 diff 裡，訊息照送，附上你當時看到的內容',
  /* ----------------------------- worlds ------------------------------- */
  'world.local': '本機',
  'world.where': '主機',
  'world.pick': '新卡片與 session 開在哪裡',
  'world.probing': '偵測中…',
  'world.noAgent': '這台主機的 PATH 上找不到 claude 或 codex',
  /* ------------------------ find in terminal -------------------------- */
  'term.find': '搜尋終端機',
  'term.findHint': 'Enter 下一個，Shift+Enter 上一個，Esc 關閉',
  'term.prev': '上一個符合',
  'term.next': '下一個符合',
  'term.noMatch': '沒有符合',
  'keys.find': '搜尋聚焦的終端機',
  /* --------------------------- token account -------------------------- */
  'usage.line': 'context {ctx} · 輸出 {out}',
  'usage.tip':
    '每回合結束時從 transcript 讀取。context {context} 為上一次請求的 prompt 大小。累計：輸入 {input}、輸出 {output}、快取寫入 {write}、快取讀取 {read}',

  /* ----------------------------- profiles ------------------------------ */
  'profile.namePlaceholder': 'opus、quiet-claude、…',
  'profile.add': '新增設定檔',
  'profile.remove': '移除這個設定檔',
  'profile.save': '儲存設定檔',
  'profile.saved': '已儲存 ✓',

  /* ------------------------------ views ------------------------------- */
  'view.overview': '總覽',
  'view.board': '看板',
  'view.inspector': '檢視器',
  'view.terminal': '終端機',

  /* ------------------------------ errors ------------------------------ */
  'error.updateTab': '更新分頁失敗：{err}',
  'error.openSession': '開啟 session 失敗：{err}',
  'error.reopen': '重新開啟失敗：{err}',
  'error.resumeAttempt': '繼續 attempt 失敗：{err}',
  'error.moveCard': '搬移卡片失敗：{err}',
  'error.cancelQueue': '離開排隊失敗：{err}',
  'error.park': '擱置 attempt 失敗：{err}',
  'error.deleteCard': '刪除卡片失敗：{err}',
  'error.newTab': '新增分頁失敗：{err}',
  'error.runScript': '啟動 run script 失敗：{err}',
  'error.openShell': '開 worktree shell 失敗：{err}',
};

export type Locale = 'en' | 'zh-TW';

export const CATALOG: Record<Locale, Record<MessageKey, string>> = { en, 'zh-TW': zhTW };

export type TFn = (key: MessageKey, vars?: Record<string, string | number>) => string;

/** Substitutes `{name}` placeholders. An unknown key renders as itself, which
    is louder in a screenshot than an empty string and easier to grep for. */
export function format(template: string, vars?: Record<string, string | number>): string {
  if (!vars) return template;
  return template.replace(/\{(\w+)\}/g, (whole, name: string) =>
    name in vars ? String(vars[name]) : whole,
  );
}

/** A translator for one language, with no React attached — which is what lets
    the model tests exercise label logic directly. */
export function translator(locale: Locale): TFn {
  return (key, vars) => format(CATALOG[locale][key] ?? key, vars);
}
