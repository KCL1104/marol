import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type * as React from 'react';
import { api, subscribe } from './api';
import { useT } from './i18n';
import { Icon } from './components/Icon';
import { needsYou, taskRepos } from './types';
import type {
  BootStatus,
  Lifecycle,
  PermissionMode,
  SessionMeta,
  Status,
  Tab,
  Task,
  TaskRepo,
} from './types';
import { STATUS_KEY } from './sections';
import { followupSendable, type ReviewComment } from './review';
import { PreviewPanel } from './components/PreviewPanel';
import { BootGate } from './components/BootGate';
import { SessionList } from './components/SessionList';
import { EdgeDrop, EmptyGrid, Pane } from './components/Pane';
import { Splitter } from './components/Splitter';
import { Overview } from './components/Overview';
import { Board } from './components/Board';
import { TabStrip } from './components/TabStrip';
import { NewSessionDialog } from './components/NewSessionDialog';
import { NewTaskDialog, rememberRepo } from './components/NewTaskDialog';
import { StartAttemptDialog } from './components/StartAttemptDialog';
import { AttemptInspector } from './components/AttemptInspector';
import {
  addMember,
  autoCols,
  autoRows,
  dropOn,
  dropOnRoot,
  DRAG_MIME,
  formatLayout,
  geometry,
  gridStyle,
  isSplit,
  leaves,
  materialise,
  members as memberIds,
  nodeAt,
  parseLayout,
  reconcile,
  reconcileTree,
  removeLeaf,
  removeMember,
  resetFractions,
  sameIds,
  setFractions,
  type DragPayload,
  type Layout,
  type Node,
  type Zone,
} from './layout';
import { ColumnPicker } from './components/ColumnPicker';
import { SettingsPanel } from './components/SettingsPanel';
import { ShortcutsDialog } from './components/ShortcutsDialog';
import { WelcomeDialog } from './components/WelcomeDialog';
import { CoachMark } from './components/CoachMark';
import { CommandPalette } from './components/CommandPalette';
import { clearCoachSeen, coachSeen, markCoachSeen, type CoachId } from './coach';
import type { ActionCtx, ActionId } from './actions';
import { rememberFolded, storedFolded } from './sidebar';
import { useSize } from './useSize';

/** Terminals start at a sane size; the pane refits within a frame of mounting. */
const INITIAL_COLS = 120;
const INITIAL_ROWS = 32;

type View = 'terminal' | 'board' | 'overview';
const VIEW_KEY = 'marol.view';
const TAB_KEY = 'marol.activeTab';
const WELCOME_KEY = 'marol.welcomed';

/** Centre-drop within auto mode: the two trade places in the running order. */
function swapIds(ids: readonly string[], movingId: string, targetId: string): string[] {
  const ti = ids.indexOf(targetId);
  if (ti < 0) return [...ids];
  const mi = ids.indexOf(movingId);
  const next = ids.slice();
  next[ti] = movingId;
  // A session arriving from the sidebar has no slot to give back, so the pane
  // it landed on returns to the sidebar instead.
  if (mi >= 0) next[mi] = targetId;
  return next;
}

export default function App() {
  const t = useT();
  const [boot, setBoot] = useState<BootStatus | null>(null);
  const [sessions, setSessions] = useState<SessionMeta[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeTabId, setActiveTabId] = useState<string | null>(() =>
    localStorage.getItem(TAB_KEY),
  );
  const [view, setView] = useState<View>(
    () => (localStorage.getItem(VIEW_KEY) as View) || 'terminal',
  );
  /** Focus and zoom key on the session, never on a position. A position is not
   *  an identity: rearranging would silently move them to another agent. */
  const [focused, setFocused] = useState<string | null>(null);
  const [zoomedId, setZoomedId] = useState<string | null>(null);
  /** Live tree while a splitter is being dragged; `undefined` when it is not. */
  const [liveRoot, setLiveRoot] = useState<Node | null | undefined>(undefined);
  /** Something is being dragged, so the layout's own edges become targets. */
  const [dragging, setDragging] = useState(false);
  /** A tab that was just created opens in rename mode; naming it is the whole
   *  point of having more than one. */
  const [renameTabId, setRenameTabId] = useState<string | null>(null);
  const [showNew, setShowNew] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  /** The sidebar, folded down to its rail. Remembered across restarts: a
   *  person who wants the width back has said so about their desk, not about
   *  this session. */
  const [sidebarFolded, setSidebarFolded] = useState(storedFolded);
  const toggleSidebar = useCallback(() => setSidebarFolded((v) => !v), []);
  // Written from an effect rather than from inside the updater: an updater is
  // allowed to run twice, and a setter is not the place for the disk.
  useEffect(() => {
    rememberFolded(sidebarFolded);
  }, [sidebarFolded]);
  /** Errors stack instead of overwriting: parallel agents fail in parallel,
   *  and the second failure must not eat the first. Good news dismisses
   *  itself; a problem waits to be read. */
  const [toasts, setToasts] = useState<{ id: number; kind: 'error' | 'ok'; text: string }[]>([]);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [showNewTask, setShowNewTask] = useState(false);
  /** A goal typed into the palette, carried into the card dialog. Empty
   *  when the dialog was opened the ordinary way. */
  const [composed, setComposed] = useState('');
  /** The card whose start dialog is open. */
  const [starting, setStarting] = useState<Task | null>(null);
  /** 請看板聚焦這張卡：面板選了一張沒有終端機的卡、或剛建立一張新卡
   *  時,單純切到看板會把焦點留在 <body> 上 —— 鍵盤流就此斷掉。看板
   *  聚焦完成後回報,這裡清掉請求。 */
  const [boardFocusId, setBoardFocusId] = useState<string | null>(null);
  /** Kept beside the dialog rather than in the toast: a rejected repository
   *  is something to correct in the form, not to be told about elsewhere. */
  const [dialogError, setDialogError] = useState<string | null>(null);
  /** Which attempt the diff/timeline drawer is showing, or null when it is
   *  closed. It sits *beside* the terminal, so answering what you just read is
   *  one keystroke rather than a navigation. Held as an id rather than
   *  derived from the focused session, because a finished attempt has no
   *  session left and is exactly the thing you most want to read. */
  const [inspectId, setInspectId] = useState<string | null>(null);
  /** The shortcuts cheat sheet (⌘/Ctrl+/). */
  const [showKeys, setShowKeys] = useState(false);
  /** The command palette (⌘/Ctrl+K): the attention inbox, then everything
   *  by name. */
  const [showPalette, setShowPalette] = useState(false);
  /** Where focus was when the palette opened. Restored on cancel only —
   *  a palette action that lands in a terminal has already decided where
   *  focus belongs, and putting it back would undo the very jump. */
  const paletteReturn = useRef<HTMLElement | null>(null);
  /** The first-run panel: what the environment probe found, and the mental
   *  model in three sentences. Shown once, never to a desk already in use. */
  const [showWelcome, setShowWelcome] = useState(false);
  /** The one coaching card on screen, if any. One at a time: a second
   *  trigger while one is up simply stays unseen for its next occasion. */
  const [coach, setCoach] = useState<CoachId | null>(null);

  const teach = useCallback((id: CoachId) => {
    setCoach((cur) => (cur !== null || coachSeen(id) ? cur : id));
  }, []);
  /** Review feedback in progress, per attempt. Held here because the drawer
   *  unmounts on ⌘I and follows focus between panes — the dialogs already
   *  promise a stray click cannot discard typed text, and the flagship loop
   *  cannot keep less of that promise than a dialog does. */
  const [reviewDrafts, setReviewDrafts] = useState<Record<string, ReviewComment[]>>({});
  /** Files marked as viewed while reviewing, per attempt — the reviewer's
   *  own progress through a large diff, held here for the same reason the
   *  drafts are: ⌘I must not reset a review half walked. */
  const [reviewViewed, setReviewViewed] = useState<Record<string, string[]>>({});
  /** Sessions that finished a turn while their terminal was not in front of
   *  you. 「等你」 has a whole signal chain; without this, 「趁你不在時做完了」
   *  had none — finished work sat indistinguishable from work already read.
   *  Cleared the moment the pane is focused, or the agent runs again. */
  const [unseen, setUnseen] = useState<ReadonlySet<string>>(new Set());
  /** Each session's last status, so only transitions mark unseen — a
   *  re-broadcast of an idle list must not re-mark what was already read. */
  const lastStatus = useRef<Map<string, Status>>(new Map());
  /** What the screen reader hears when an agent starts waiting. The whole
   *  in-app signal chain is otherwise visual — a breathing card is silence
   *  to AT, and the OS notification only fires while the window is away. */
  const [announce, setAnnounce] = useState('');
  /** Per-session, the blocked status last spoken — not a set: a session
   *  moving between two blocked states (資料夾信任 → 等你授權) is news too,
   *  and set membership would swallow it. */
  const spokenStatus = useRef<Map<string, string>>(new Map());
  const announceTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const say = useCallback((text: string) => {
    setAnnounce(text);
    // Spoken once, then cleared: stale text lingering in the live region
    // reads back on every re-entry as if it just happened.
    if (announceTimer.current) clearTimeout(announceTimer.current);
    announceTimer.current = setTimeout(() => setAnnounce(''), 5000);
  }, []);

  useEffect(() => {
    const fresh: SessionMeta[] = [];
    const nowWaiting = new Set<string>();
    for (const s of sessions) {
      if (!needsYou(s.status)) continue;
      nowWaiting.add(s.id);
      if (spokenStatus.current.get(s.id) !== s.status) {
        fresh.push(s);
        spokenStatus.current.set(s.id, s.status);
      }
    }
    for (const id of [...spokenStatus.current.keys()]) {
      if (!nowWaiting.has(id)) spokenStatus.current.delete(id);
    }
    if (fresh.length === 1) {
      say(`${fresh[0].title} ${t(STATUS_KEY[fresh[0].status])}`);
    } else if (fresh.length > 1) {
      // Several blocked in one update: say all of them, not the first.
      say(
        t('announce.multi', {
          count: fresh.length,
          // The list separator is a word in a locale's own punctuation —
          // a hardcoded 、 reads Chinese into an English sentence.
          titles: fresh.map((s) => s.title).join(t('common.listSep')),
        }),
      );
    }
  }, [sessions, t, say]);

  const [gridRef, size] = useSize<HTMLDivElement>();

  const pushToast = useCallback((kind: 'error' | 'ok', text: string) => {
    const id = Date.now() + Math.random();
    // Nothing is evicted: the screen shows the newest three, and the rest
    // wait behind a count. Dropping the oldest error was exactly the
    // second-problem-eats-the-first this stack exists to prevent.
    setToasts((cur) => [...cur, { id, kind, text }]);
    if (kind === 'ok') {
      setTimeout(() => setToasts((cur) => cur.filter((t) => t.id !== id)), 4000);
    }
  }, []);
  const setError = useCallback(
    (text: string | null) => {
      if (text !== null) pushToast('error', text);
    },
    [pushToast],
  );

  useEffect(() => {
    localStorage.setItem(VIEW_KEY, view);
  }, [view]);

  useEffect(() => {
    let dispose: (() => void) | undefined;
    void (async () => {
      setBoot(await api.bootStatus());
      dispose = await subscribe({
        onSessions: (list) => {
          setSessions(list);
          setLoaded(true);
        },
        onTabs: setTabs,
        onTasks: setTasks,
        onExit: () => {},
        onBadge: () => {},
        onCoreReady: () => void api.bootStatus().then(setBoot),
        onCoreFailed: (error) => setBoot({ ready: false, error }),
      });

      // Subscribe first, then read the current state. The core broadcasts
      // both lists as it starts, which is before this window can be
      // listening — without the read the app would sit empty until something
      // happened to change.
      const [initialTabs, initialSessions, initialTasks] = await Promise.all([
        api.listTabs().catch(() => []),
        api.listSessions().catch(() => []),
        api.listTasks().catch(() => []),
      ]);
      setTabs((cur) => (cur.length ? cur : initialTabs));
      setSessions((cur) => (cur.length ? cur : initialSessions));
      setTasks((cur) => (cur.length ? cur : initialTasks));
      setLoaded(true);
    })();
    return () => dispose?.();
  }, []);

  // The arrangement lives on the tab, so it survives a restart. Only which tab
  // you were last looking at is local.
  const activeTab = useMemo(
    () => tabs.find((t) => t.id === activeTabId) ?? tabs[0] ?? null,
    [tabs, activeTabId],
  );
  const layout = useMemo(() => parseLayout(activeTab?.layout), [activeTab?.layout]);
  const members = useMemo(() => memberIds(activeTab?.slots), [activeTab?.slots]);

  useEffect(() => {
    if (activeTab) localStorage.setItem(TAB_KEY, activeTab.id);
    setZoomedId(null);
    setLiveRoot(undefined);
  }, [activeTab?.id]);

  const commit = useCallback(
    (nextLayout: Layout, nextMembers: string[]) => {
      if (!activeTab) return;
      // Members and the tree are two views of one thing, so every write
      // reconciles them rather than trusting them to have stayed in step.
      const final: Layout =
        nextLayout.mode === 'manual'
          ? { mode: 'manual', root: reconcileTree(nextLayout.root, nextMembers) }
          : nextLayout;
      void api
        .updateTab(activeTab.id, formatLayout(final), nextMembers)
        .catch((e) => setError(t('error.updateTab', { err: String(e) })));
    },
    [activeTab],
  );

  // A session that stopped running leaves the layout. Nothing is pulled in to
  // replace it: refilling a pane the user just ejected makes eject look
  // broken, and that is a bug this app has already had once.
  useEffect(() => {
    if (!activeTab || !loaded) return;
    const next = reconcile(members, sessions);
    if (!sameIds(next, members)) commit(layout, next);
  }, [sessions, members, layout, activeTab, loaded, commit]);

  /* ---------------------------- geometry ---------------------------- */

  const cols = autoCols(layout, size.w, members.length);
  const rows = autoRows(members.length, cols);

  const root = useMemo(
    () => (layout.mode === 'manual' ? reconcileTree(layout.root, members) : null),
    [layout, members],
  );
  const shownRoot = liveRoot !== undefined ? liveRoot : root;

  const geom = useMemo(
    () =>
      layout.mode === 'manual'
        ? geometry(shownRoot, { x: 0, y: 0, w: size.w, h: size.h })
        : null,
    [layout.mode, shownRoot, size.w, size.h],
  );

  const focusedId = focused && members.includes(focused) ? focused : (members[0] ?? null);
  const zoomed = zoomedId && members.includes(zoomedId) ? zoomedId : null;

  const active = useMemo(
    () => sessions.find((s) => s.id === focusedId) ?? null,
    [sessions, focusedId],
  );

  const attempts = useMemo(() => tasks.flatMap((t) => t.attempts), [tasks]);

  /** The attempt behind the focused session, if it has one. */
  const activeAttemptId = active?.attempt_id ?? null;

  const inspected = useMemo(
    () => attempts.find((a) => a.id === inspectId) ?? null,
    [attempts, inspectId],
  );

  /** Whether the inspected attempt has a previewable dev server: a live
   *  script session in its worktree carrying the recorded port. 'ssh'
   *  means one is running but its port lives on the remote — the button
   *  appears disabled, wearing that reason, instead of not existing. */
  const inspectedPreview = useMemo(() => {
    if (!inspected || inspected.outcome !== null) return null;
    const wt = inspected.worktree_path.replace(/\/$/, '');
    const script = sessions.find(
      (s) => s.live && s.agent === 'sh' && (s.cwd === wt || s.cwd.startsWith(`${wt}/`)),
    );
    if (!script) return null;
    if (script.preview_port == null) {
      return wt.startsWith('ssh://') ? ('ssh' as const) : null;
    }
    return { port: script.preview_port, sessionId: script.id };
  }, [inspected, sessions]);

  // With the drawer open, moving to another session's pane moves the drawer
  // with it. Reading one attempt's diff while looking at another's terminal
  // is the one arrangement that could actively mislead.
  useEffect(() => {
    if (!inspectId || !activeAttemptId) return;
    if (activeAttemptId !== inspectId) setInspectId(activeAttemptId);
  }, [activeAttemptId, inspectId]);

  // Every live session keeps a mounted terminal, whichever tab or view is
  // showing, so switching never costs a repaint or loses scrollback.
  const liveIds = useMemo(() => sessions.filter((s) => s.live).map((s) => s.id), [sessions]);

  /** The card the board is peeking at — set by hovering or focusing a card
   *  whose session is live, sticky until another takes it. */
  const [previewId, setPreviewId] = useState<string | null>(null);

  /** The dev-server preview: an iframe on the peek's patch of ground.
   *  One patch, and the explicit act outranks the hover — the peek is
   *  suppressed while this is open and comes back the moment it closes.
   *  Both at once means the external browser (the decision doc's call). */
  const [devPreview, setDevPreview] = useState<{
    port: number;
    sessionId: string;
    agentSessionId: string | null;
  } | null>(null);
  /** The element last picked inside the previewed page, via the opt-in
   *  inspect script's postMessage. */
  const [pick, setPick] = useState<{ component: string; file: string; line: number } | null>(
    null,
  );

  // The one sanctioned channel from the previewed page: the pick grammar,
  // from the preview's own origin, and nothing else.
  useEffect(() => {
    if (devPreview === null) return;
    const allowed = new Set([
      `http://localhost:${devPreview.port}`,
      `http://127.0.0.1:${devPreview.port}`,
    ]);
    const onMessage = (e: MessageEvent) => {
      if (!allowed.has(e.origin)) return;
      const d = e.data as
        | { type?: unknown; component?: unknown; file?: unknown; line?: unknown }
        | null;
      if (!d || d.type !== 'marol:pick' || typeof d.file !== 'string') return;
      setPick({
        component: typeof d.component === 'string' ? d.component : '?',
        file: d.file,
        line: typeof d.line === 'number' ? d.line : 0,
      });
    };
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, [devPreview]);

  /** The board's live peek: the hovered card's session, else the focused
   *  one. Claude Squad's list+preview, GUI-shaped — and because it is the
   *  same mounted pane, the preview is the terminal, not a copy of it. */
  const previewSession = useMemo(() => {
    if (view !== 'board' || devPreview !== null) return null;
    const liveOf = (id: string | null) => {
      const s = id ? sessions.find((x) => x.id === id) : undefined;
      return s?.live ? s.id : null;
    };
    return liveOf(previewId) ?? liveOf(focusedId);
  }, [view, previewId, sessions, focusedId, devPreview]);

  /** The open attempt behind each live session — for the ⚡ badge in the
   *  views where supervision actually happens, not only on the board. */
  const attemptBySession = useMemo(() => {
    const map = new Map<string, PermissionMode>();
    for (const a of attempts) {
      if (a.session_id !== null && a.outcome === null) map.set(a.session_id, a.mode);
    }
    return map;
  }, [attempts]);

  // Turn endings, filed as read or unread. A turn that ends (idle) or a CLI
  // that exits while its pane is not the one in front of you goes unread —
  // and only a transition marks it, so a re-broadcast of an idle list never
  // re-marks what was already read.
  useEffect(() => {
    const ended = (s: Status) => s === 'idle' || s === 'exited';
    const mark: string[] = [];
    const clear: string[] = [];
    const alive = new Set<string>();
    for (const s of sessions) {
      alive.add(s.id);
      const prev = lastStatus.current.get(s.id);
      lastStatus.current.set(s.id, s.status);
      if (prev === undefined || prev === s.status) continue;
      if (ended(s.status) && !ended(prev) && prev !== 'saved') {
        // Read live only if that pane was on screen with the caret and the
        // window itself had the user's attention.
        const inFront =
          document.hasFocus() &&
          view === 'terminal' &&
          focusedId === s.id &&
          (zoomed === null || zoomed === s.id);
        if (!inFront) mark.push(s.id);
      } else if (!ended(s.status)) {
        // Working again: whatever finished before has been acted on.
        clear.push(s.id);
      }
    }
    for (const id of [...lastStatus.current.keys()]) {
      if (!alive.has(id)) {
        lastStatus.current.delete(id);
        clear.push(id);
      }
    }
    if (mark.length === 0 && clear.length === 0) return;
    // 趁你不在時做完的那一刻，也讓朗讀器聽到：視覺鏈有 unseen 小點，
    // 朗讀鏈在這裡補上。mark 本身已是「僅轉變」過濾後的結果 —— 重播
    // 的 idle 清單進不了這裡，所以一個 session 變成未讀只說一次。
    if (mark.length > 0) {
      const titles = mark
        .map((id) => sessions.find((s) => s.id === id)?.title)
        .filter((title): title is string => title !== undefined);
      if (titles.length > 0) {
        say(t('announce.finished', { title: titles.join(t('common.listSep')) }));
      }
    }
    setUnseen((prev) => {
      const marks = mark.filter((id) => !prev.has(id));
      const clears = clear.filter((id) => prev.has(id));
      if (marks.length === 0 && clears.length === 0) return prev;
      const next = new Set(prev);
      for (const id of clears) next.delete(id);
      for (const id of marks) next.add(id);
      return next;
    });
  }, [sessions, view, focusedId, zoomed, say, t, teach]);

  // The first-run panel, decided once at readiness. A desk already in use
  // has nothing to introduce — it gets the flag set silently instead, so
  // an upgrade never greets a veteran.
  useEffect(() => {
    if (!loaded || !boot?.ready) return;
    if (localStorage.getItem(WELCOME_KEY) !== null) return;
    if (sessions.length > 0 || tasks.length > 0) {
      localStorage.setItem(WELCOME_KEY, '1');
      return;
    }
    // 真正的第一次落在看板:第一件要做的事是開第一張卡,而卡片的門
    // 在看板上。歡迎面板浮在它上面,所以每一條出口(關閉/建卡/臨時
    // session)關上門時,腳下都已經是看板 —— 不是一面空的終端牆。
    setView('board');
    setShowWelcome(true);
    // Deliberately not re-run on sessions/tasks: the question is what the
    // desk held at the moment it became ready, not after the first card.
  }, [loaded, boot?.ready]);

  // The first time the caret lands in a pane: which keys leave it, and that
  // Ctrl+letter belongs to the shell in there.
  useEffect(() => {
    if (view === 'terminal' && focusedId !== null) teach('terminal');
  }, [view, focusedId, teach]);

  // Looking at it is what reads it — including coming back to a window that
  // was elsewhere when the turn ended, which no state change announces.
  useEffect(() => {
    const readFocused = () => {
      if (view !== 'terminal' || focusedId === null) return;
      if (zoomed !== null && zoomed !== focusedId) return;
      setUnseen((prev) => {
        if (!prev.has(focusedId)) return prev;
        const next = new Set(prev);
        next.delete(focusedId);
        return next;
      });
    };
    readFocused();
    window.addEventListener('focus', readFocused);
    return () => window.removeEventListener('focus', readFocused);
  }, [view, focusedId, zoomed]);

  /* ----------------------------- actions ---------------------------- */

  const onCreate = useCallback(
    async (cwd: string, agent: string, args: string[]) => {
      setShowNew(false);
      setError(null);
      try {
        const id = await api.newSession(cwd, agent, args, INITIAL_COLS, INITIAL_ROWS);
        commit(layout, addMember(members, id));
        setFocused(id);
        setView('terminal');
      } catch (e) {
        // Without this the dialog just closes and nothing happens, which reads
        // as a dead button rather than a failure.
        setError(t('error.openSession', { err: String(e) }));
      }
    },
    [commit, layout, members],
  );

  /** Clicking a sidebar row adds it to the layout; dragging places it. */
  const onSelect = useCallback(
    async (id: string) => {
      const s = sessions.find((x) => x.id === id);
      // Reopening a saved session reattaches a terminal, continuing the
      // agent's own conversation history in that directory.
      //
      // **Reopen first, then take the slot.** The other order raced itself:
      // adding a session that is still `live: false` to the layout, then
      // waiting for the reopen, gave the reconcile effect above a render in
      // which to do exactly what it is for — drop the members whose sessions
      // have stopped — and its write landed last. The tab ended up empty, so
      // clicking a stopped card switched to the terminal wall and showed
      // nothing at all. Awaiting first means the session is live by the time
      // it is added, and there is nothing for reconcile to disagree with.
      if (s && !s.live) {
        try {
          await api.reopenSession(id, INITIAL_COLS, INITIAL_ROWS);
        } catch (e) {
          setError(t('error.reopen', { err: String(e) }));
          // No pane for a session that did not come back: an empty slot
          // claiming to hold a terminal is worse than no slot.
          return;
        }
      }
      commit(layout, addMember(members, id));
      setFocused(id);
    },
    [sessions, commit, layout, members],
  );

  /**
   * A pane or a sidebar row was dropped on the pane showing `targetId`.
   *
   * Dropping on an edge is what turns a row of four into a 2x2, and it is the
   * moment a tab stops being auto: the arrangement is now something the user
   * built rather than something the window width implied.
   */
  const onDropOnPane = useCallback(
    (targetId: string, payload: DragPayload, zone: Zone) => {
      if (payload.id === targetId) return;

      if (layout.mode === 'auto' && zone === 'center') {
        commit(layout, swapIds(members, payload.id, targetId));
        setFocused(payload.id);
        return;
      }

      const base = layout.mode === 'manual' ? root : materialise(members, cols);
      const next = dropOn(base, payload.id, targetId, zone);
      commit({ mode: 'manual', root: next }, leaves(next));
      setFocused(payload.id);
    },
    [commit, layout, members, root, cols],
  );

  const onEject = useCallback(
    (id: string) => {
      if (layout.mode === 'manual') {
        const next = removeLeaf(root, id);
        commit({ mode: 'manual', root: next }, leaves(next));
      } else {
        commit(layout, removeMember(members, id));
      }
    },
    [commit, layout, members, root],
  );

  /** Dropped on the grid itself rather than on a pane: put it at the end. */
  const onDropOnGrid = useCallback(
    (payload: DragPayload) => {
      commit(layout, addMember(members, payload.id));
      setFocused(payload.id);
    },
    [commit, layout, members],
  );

  /** Dropped on the layout's outer edge: it spans that whole side. */
  const onDropOnEdge = useCallback(
    (payload: DragPayload, zone: Zone) => {
      const base = layout.mode === 'manual' ? root : materialise(members, cols);
      const next = dropOnRoot(base, payload.id, zone);
      commit({ mode: 'manual', root: next }, leaves(next));
      setFocused(payload.id);
    },
    [commit, layout, members, root, cols],
  );

  const onPickCols = useCallback(
    (value: 'auto' | number) => {
      setZoomedId(null);
      commit({ mode: 'auto', cols: value }, members);
    },
    [commit, members],
  );

  /** From the overview or the board: focus a session and go look at it. */
  const onOpen = useCallback(
    async (id: string) => {
      await onSelect(id);
      setView('terminal');
    },
    [onSelect],
  );

  /**
   * The whole point of clicking a session is to look at it, so the sidebar
   * goes through `onOpen`, not bare `onSelect`. From the board or overview a
   * bare select would put the session into a layout that is not on screen —
   * the app's most urgent affordance answering its click with nothing.
   */
  const onOpenFromSidebar = useCallback((id: string) => void onOpen(id), [onOpen]);

  /**
   * Where you were before here.
   *
   * ⌘E answers whoever interrupted; this is the way back to what the
   * interruption cost you — the two halves of the same triage loop, one
   * for attention and one for memory. Deliberately one step deep: a real
   * history stack would need its own UI to be usable, and the palette
   * already lists every session for anything further back than "the one
   * before this".
   */
  /**
   * Kept in a ref advanced during render, not in an effect.
   *
   * An effect commits a frame late: the pane would already be showing the
   * session you just moved to while the way back was still the one before
   * *that*. A chord pressed in the gap did nothing — rare for a person,
   * reliable enough for a test to catch. Advancing here means the trail is
   * true on the same render that shows the move. The write is idempotent
   * per focused id, so a double render changes nothing.
   */
  const trail = useRef<{ cur: string | null; prev: string | null }>({ cur: null, prev: null });
  if (focusedId !== null && focusedId !== trail.current.cur) {
    trail.current = { cur: focusedId, prev: trail.current.cur };
  }

  /** The way back, only while it still exists: a session can end or be
   *  removed while you are away, and a door onto nothing is worse than no
   *  door — so the chord and the palette row both vanish with it. */
  const back = trail.current.prev;
  const backTo = back !== null && sessions.some((s) => s.id === back) ? back : null;

  /**
   * The keyboard set. Every chord is one a shell does not own — the bindings
   * follow the terminal-app convention (gnome-terminal, VS Code): letters on
   * ⌘/Ctrl, but a Ctrl+letter pressed *inside* a terminal belongs to readline
   * (Ctrl+E is end-of-line, Ctrl+I is Tab, Ctrl+[ is Esc), so from in there
   * you add Shift, exactly as Ctrl+Shift+C copies. Panes cycle on
   * ⌘/Ctrl+Alt+arrows and tabs on Ctrl+PgDn/PgUp, which no shell uses at all.
   *
   * An open dialog owns the keyboard outright; nothing here fires past a
   * backdrop.
   */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return;
      if (document.querySelector('.modal-backdrop')) return;
      const inTerminal =
        e.target instanceof Element && e.target.closest('.term-host') !== null;
      const shellsOwn = inTerminal && e.ctrlKey && !e.metaKey && !e.shiftKey;

      if (e.altKey) {
        if (
          (e.key === 'ArrowLeft' || e.key === 'ArrowRight') &&
          view === 'terminal' &&
          members.length > 1
        ) {
          e.preventDefault();
          const step = e.key === 'ArrowRight' ? 1 : -1;
          const i = Math.max(0, members.indexOf(focusedId ?? ''));
          setFocused(members[(i + step + members.length) % members.length]);
        }
        return;
      }

      if (e.key === 'PageDown' || e.key === 'PageUp') {
        // Tabs wrap around; a cycle with a dead end is a list.
        if (tabs.length > 1 && activeTab) {
          e.preventDefault();
          const step = e.key === 'PageDown' ? 1 : -1;
          const i = tabs.findIndex((t) => t.id === activeTab.id);
          setActiveTabId(tabs[(i + step + tabs.length) % tabs.length].id);
        }
      } else if (!e.shiftKey && (e.key === '1' || e.key === '2' || e.key === '3')) {
        e.preventDefault();
        setView((['terminal', 'board', 'overview'] as const)[Number(e.key) - 1]);
      } else if (e.shiftKey && (e.key === 'n' || e.key === 'N')) {
        // 新卡片(⌘/Ctrl+Shift+N)。Shift 生在和弦裡,所以終端機內外同一
        // 顆,不必像其他字母鍵那樣「在終端機裡再加 Shift」—— shellsOwn
        // 判的正是無 Shift 的 Ctrl+字母。做的事與面板的 new-card 同一份。
        e.preventDefault();
        setDialogError(null);
        setView('board');
        setShowNewTask(true);
      } else if ((e.key === 'e' || e.key === 'E') && !shellsOwn) {
        // Cycles, not jumps: with three blocked agents, each press lands on
        // the next one, so answering them all is E, answer, E, answer, E.
        const waiting = sessions.filter((s) => needsYou(s.status));
        if (waiting.length > 0) {
          e.preventDefault();
          const i = waiting.findIndex((s) => s.id === focusedId);
          void onOpen(waiting[(i + 1) % waiting.length].id);
        }
      } else if ((e.key === 'l' || e.key === 'L') && !shellsOwn) {
        // The other half of ⌘E. K was not available for this: inside a
        // terminal Ctrl+Shift+K is already how the palette is reached.
        if (backTo !== null) {
          e.preventDefault();
          void onOpen(backTo);
        }
      } else if ((e.key === 'b' || e.key === 'B') && !shellsOwn) {
        // The chord every editor with a side bar uses. Ctrl+B is readline's
        // backward-char, so from inside a terminal it is Ctrl+Shift+B —
        // the same rule E, L, F and I already follow, applied by `shellsOwn`.
        e.preventDefault();
        toggleSidebar();
      } else if ((e.key === 'k' || e.key === 'K') && !shellsOwn) {
        e.preventDefault();
        paletteReturn.current = document.activeElement as HTMLElement | null;
        setShowPalette(true);
      } else if ((e.key === 'f' || e.key === 'F') && !shellsOwn) {
        // Find in the focused terminal. Same shell rule as every letter:
        // Ctrl+F from inside a terminal is readline's cursor-forward, so
        // in there the chord is Ctrl+Shift+F. The pane owns the find bar;
        // the event names which one, the theme event's precedent. From
        // inside the drawer the chord stays dead: opening a terminal's
        // find while reading the diff would answer the wrong question.
        const inDrawer =
          e.target instanceof Element && e.target.closest('.inspector') !== null;
        if (view === 'terminal' && focusedId !== null && !inDrawer) {
          e.preventDefault();
          window.dispatchEvent(new CustomEvent('marol:find', { detail: focusedId }));
        }
      } else if ((e.key === 'i' || e.key === 'I') && !shellsOwn) {
        if (view === 'terminal' && (inspectId || activeAttemptId)) {
          e.preventDefault();
          const opening = !inspectId;
          setInspectId(inspectId ? null : activeAttemptId);
          // The flagship keyboard loop gets a keyboard entrance: opening
          // by chord lands focus on the diff, where j/k and n/p live.
          // Only by chord — the drawer also follows pane focus around,
          // and stealing the caret on every follow would be theft.
          if (opening) {
            // The diff arrives async, so the landing waits for it — but
            // only while the caret still sits where the chord left it. The
            // moment the user moves on, the entrance expires unclaimed.
            const from = document.activeElement;
            const t0 = performance.now();
            const land = () => {
              if (document.activeElement !== from) return;
              const body = document.querySelector<HTMLElement>('[data-testid="diff-body"]');
              if (body) body.focus();
              else if (performance.now() - t0 < 1000) requestAnimationFrame(land);
            };
            requestAnimationFrame(land);
          }
        }
      } else if (e.key === '/' && !shellsOwn) {
        e.preventDefault();
        setShowKeys((v) => !v);
      } else if (e.key === ',' && !shellsOwn) {
        // The platform's own settings chord. Not a letter, so the terminal
        // rule does not bite: Ctrl+comma is nobody's readline binding.
        e.preventDefault();
        setShowSettings(true);
      }
    };
    // Capture phase: xterm stops propagation on keys it maps (modified
    // arrows, Ctrl+[), so a bubble listener would never hear our chords from
    // inside a terminal. Capturing runs first; anything we do not
    // preventDefault still reaches the terminal untouched.
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [
    sessions,
    onOpen,
    view,
    inspectId,
    activeAttemptId,
    tabs,
    activeTab,
    members,
    focusedId,
    backTo,
    toggleSidebar,
  ]);

  /* ------------------------------ board ----------------------------- */

  const onCreateTask = useCallback(
    async (
      title: string,
      prompt: string,
      repoPath: string,
      baseBranch: string,
      extraRepos: TaskRepo[] = [],
    ) => {
      setDialogError(null);
      try {
        const id = await api.createTask(title, prompt, repoPath, baseBranch, extraRepos);
        // Every repository the card named, so a second card spanning the same
        // pair finds both in the recents.
        for (const r of [repoPath, ...extraRepos.map((e) => e.repo_path)]) rememberRepo(r);
        setShowNewTask(false);
        // 建立成功不是流程的終點,是下一步(開 attempt)的起點:切到看板、
        // 焦點落在新卡片上 —— Enter 進門、Tab 就是 Start。對話框關掉時
        // Modal 會把焦點還回原處,看板的聚焦在其後執行,所以會贏。
        setView('board');
        setBoardFocusId(id);
        // 朗讀鏈的那一份確認:對焦點的跳動,AT 只聽得到落點,聽不到原因。
        say(t('newTask.created', { title }));
      } catch (e) {
        // The core refuses a repository that is not one, or a base branch
        // that does not exist. Both are things to fix in this form.
        setDialogError(String(e));
      }
    },
    [say, t],
  );

  /**
   * Make the card and start it in one act, then land in the terminal.
   *
   * The board's ordinary path. It used to be two dialogs and seven fields —
   * make a card, find it on the board, press 開始, answer a second dialog —
   * for the case that is nearly every case: something to do, somewhere to do
   * it, go. The planning path did not disappear; it is the secondary button
   * in the same dialog, and a card left in 待辦 still starts the old way.
   *
   * The opening prompt is composed by the core exactly as the start dialog
   * composed it, so what reaches the agent is unchanged — only the step where
   * a human was asked to look at it again is gone.
   */
  const onCreateAndStart = useCallback(
    async (
      title: string,
      prompt: string,
      repoPath: string,
      baseBranch: string,
      extraRepos: TaskRepo[],
      agent: string,
      mode: PermissionMode,
    ) => {
      setDialogError(null);
      try {
        const id = await api.createTask(title, prompt, repoPath, baseBranch, extraRepos);
        for (const r of [repoPath, ...extraRepos.map((e) => e.repo_path)]) rememberRepo(r);
        setShowNewTask(false);
        setComposed('');
        setView('board');
        // What the start dialog would have shown in its textarea. A CLI whose
        // argument conventions are unmeasured gets the session and not the
        // prompt — the same refusal as before, made without a second dialog.
        const preview = await api.previewPrompt(id, agent).catch(() => null);
        const result = await api.openAttempt(
          id,
          agent,
          preview?.willSend === false ? null : (preview?.prompt ?? prompt),
          mode,
          INITIAL_COLS,
          INITIAL_ROWS,
        );
        if (result.attempt) await onOpen(result.attempt.session_id);
        else setBoardFocusId(id);
      } catch (e) {
        setDialogError(String(e));
      }
    },
    [onOpen],
  );

  /**
   * Start an attempt, then go straight into its terminal.
   *
   * Landing in the TUI is the point: the first thing a new worktree does is
   * ask whether you trust the folder, and the answer is one keystroke away
   * only if you are already looking at it.
   */
  const onStartAttempt = useCallback(
    async (task: Task, agent: string, prompt: string, mode: PermissionMode) => {
      setDialogError(null);
      try {
        const result = await api.openAttempt(
          task.id,
          agent,
          prompt,
          mode,
          INITIAL_COLS,
          INITIAL_ROWS,
        );
        setStarting(null);
        // Over the limit it waits its turn instead of starting. There is no
        // terminal to go to yet, so the board is where you want to be left —
        // the card says where it is in the queue.
        if (result.attempt) await onOpen(result.attempt.session_id);
      } catch (e) {
        setDialogError(String(e));
      }
    },
    [onOpen, teach],
  );

  /** Put a terminal back on an attempt — the state every attempt is in after
   *  a restart, so this has to land you in the TUI just like starting does. */
  const onResumeAttempt = useCallback(
    async (attemptId: string) => {
      try {
        const sessionId = await api.reopenAttempt(attemptId, INITIAL_COLS, INITIAL_ROWS);
        await onOpen(sessionId);
      } catch (e) {
        setError(t('error.resumeAttempt', { err: String(e) }));
      }
    },
    [onOpen],
  );

  /** Park: keep the work and the conversation, give back the ground. The
   *  branch name lands on the clipboard — the next thought after parking
   *  is usually "look at that branch somewhere else". */
  const onParkAttempt = useCallback(
    async (attemptId: string) => {
      try {
        const branch = await api.parkAttempt(attemptId);
        try {
          await navigator.clipboard.writeText(branch);
        } catch {
          /* headless or denied: the toast still names the branch */
        }
        pushToast('ok', t('park.done', { branch }));
      } catch (e) {
        pushToast('error', t('error.park', { err: String(e) }));
      }
    },
    [pushToast],
  );

  /** Resume a parked attempt and land in its terminal, conversation and
   *  all. A restore failure is reported, not rolled back: the worktree is
   *  honestly back on its branch either way. */
  const onResumeParked = useCallback(
    async (attemptId: string) => {
      try {
        const r = await api.resumeAttempt(attemptId, INITIAL_COLS, INITIAL_ROWS);
        if (r.restore_error !== null) {
          pushToast('error', t('park.restoreFailed', { err: r.restore_error }));
        }
        await onOpen(r.session_id);
      } catch (e) {
        pushToast('error', t('error.resumeAttempt', { err: String(e) }));
      }
    },
    [onOpen, pushToast],
  );

  /**
   * Review an attempt: open the drawer, and put its terminal up beside it.
   *
   * Both, not either. The diff answers what changed; the terminal is where
   * you say what is still wrong. A finished attempt has no session left, so
   * that half is simply absent rather than broken.
   */
  const onInspectAttempt = useCallback(
    async (attempt: { id: string; session_id: string | null }) => {
      setInspectId(attempt.id);
      if (attempt.session_id) await onOpen(attempt.session_id);
      else setView('terminal');
    },
    [onOpen],
  );

  /** Start a repo run script in an attempt's worktree, and go watch it. */
  const onRunScript = useCallback(
    async (attemptId: string, name: string) => {
      try {
        const id = await api.runScript(attemptId, name, INITIAL_COLS, INITIAL_ROWS);
        await onOpen(id);
      } catch (e) {
        setError(t('error.runScript', { err: String(e) }));
      }
    },
    [onOpen],
  );

  /** A shell of your own in an attempt's worktree — reused while it lives,
   *  so the button lands you in the shell already there rather than
   *  stacking a second. */
  const onOpenShell = useCallback(
    async (attemptId: string) => {
      try {
        const id = await api.openShell(attemptId, INITIAL_COLS, INITIAL_ROWS);
        await onOpen(id);
      } catch (e) {
        setError(t('error.openShell', { err: String(e) }));
      }
    },
    [onOpen],
  );

  const onMoveTask = useCallback((id: string, lifecycle: Lifecycle, position: number) => {
    void api.moveTask(id, lifecycle, position).catch((e) => setError(t('error.moveCard', { err: String(e) })));
  }, []);

  const onCancelQueued = useCallback((taskId: string) => {
    void api.cancelQueued(taskId).catch((e) => setError(t('error.cancelQueue', { err: String(e) })));
  }, []);

  const onDeleteTask = useCallback((id: string) => {
    void api.deleteTask(id).catch((e) => setError(t('error.deleteCard', { err: String(e) })));
  }, []);

  /** 重跑一次開機偵測。歡迎面板的「重新偵測」與重開歡迎的兩條路共用
   *  這一份 —— 偵測是真的:同一個 boot_status,回來什麼就畫什麼,
   *  沒有假的轉圈。 */
  const reprobe = useCallback(async () => {
    setBoot(await api.bootStatus());
  }, []);

  /** 再看一次歡迎面板:偵測先重跑,welcomed 旗標與 coach 記錄一概
   *  不動 —— 重看不是重來。 */
  const reopenWelcome = useCallback(() => {
    void reprobe();
    setShowWelcome(true);
  }, [reprobe]);

  /* ----------------------------- palette ---------------------------- */

  const paletteCtx: ActionCtx = useMemo(
    () => ({
      hasWaiting: sessions.some((s) => needsYou(s.status)),
      canInspect: Boolean(inspectId || activeAttemptId),
      hasPrevious: backTo !== null,
    }),
    [sessions, inspectId, activeAttemptId, backTo],
  );

  /** Closed without restoring focus: the jump decides where focus goes. */
  const paletteOpenSession = useCallback(
    (id: string) => {
      setShowPalette(false);
      void onOpen(id);
    },
    [onOpen],
  );

  /** What each registry id actually does — the App's half of the table. */
  const paletteRun = useCallback(
    (id: ActionId) => {
      setShowPalette(false);
      switch (id) {
        case 'jump-waiting': {
          // The same cycle ⌘E drives, so the palette row and the chord it
          // advertises can never disagree.
          const waiting = sessions.filter((s) => needsYou(s.status));
          if (waiting.length > 0) {
            const i = waiting.findIndex((s) => s.id === focusedId);
            void onOpen(waiting[(i + 1) % waiting.length].id);
          }
          break;
        }
        case 'last-session':
          // Same door the chord opens, same guard: the row is only offered
          // while there is somewhere alive to go back to.
          if (backTo !== null) void onOpen(backTo);
          break;
        case 'new-card':
          setDialogError(null);
          setView('board');
          setShowNewTask(true);
          break;
        case 'new-session':
          setShowNew(true);
          break;
        case 'toggle-sidebar':
          toggleSidebar();
          break;
        case 'toggle-inspector':
          // The drawer lives beside the terminals; opening it from another
          // view brings the terminals with it.
          setView('terminal');
          setInspectId(inspectId ? null : activeAttemptId);
          break;
        case 'view-terminal':
          setView('terminal');
          break;
        case 'view-board':
          setView('board');
          break;
        case 'view-overview':
          setView('overview');
          break;
        case 'open-settings':
          setShowSettings(true);
          break;
        case 'open-keys':
          setShowKeys(true);
          break;
        case 'show-welcome':
          reopenWelcome();
          break;
        case 'replay-coach':
          // Forgetting is the whole action; the marks fire on their own the
          // next time each moment comes round, so there is nothing to show
          // right now beyond saying it took.
          clearCoachSeen();
          pushToast('ok', t('coach.replayed'));
          break;
      }
    },
    [
      sessions,
      focusedId,
      onOpen,
      inspectId,
      activeAttemptId,
      reopenWelcome,
      backTo,
      pushToast,
      t,
      toggleSidebar,
    ],
  );

  const paletteCancel = useCallback(() => {
    setShowPalette(false);
    paletteReturn.current?.focus?.();
  }, []);

  if (!boot?.ready) {
    return <BootGate boot={boot} onRetry={() => void api.bootStatus().then(setBoot)} />;
  }

  const manual = layout.mode === 'manual';

  return (
    <div
      // The fold is a grid-track change, deliberately with no transition:
      // animating it would resize every terminal pane on every frame, and a
      // TUI answers a resize by reflowing itself. One layout, once.
      className={`app${sidebarFolded ? ' sidebar-folded' : ''}`}
      // Bubble phase, not capture: the source sets the payload in its own
      // handler, so during capture `types` is still empty.
      onDragStart={(e) => setDragging(e.dataTransfer.types.includes(DRAG_MIME))}
      onDragEnd={() => setDragging(false)}
      onDrop={() => setDragging(false)}
    >
      <SessionList
        sessions={sessions}
        activeId={focusedId}
        unseen={unseen}
        onSelect={onOpenFromSidebar}
        onNew={() => setShowNew(true)}
        onClose={(id) => void api.closeSession(id)}
        onArchive={(id) => void api.archiveSession(id)}
        onComplete={(id, completed) => void api.setCompleted(id, completed)}
        onRename={(id, title) => void api.renameSession(id, title)}
        onShowSettings={() => setShowSettings(true)}
        folded={sidebarFolded}
        onToggleFold={toggleSidebar}
      />

      <main className="main">
        <TabStrip
          tabs={tabs}
          activeId={activeTab?.id ?? null}
          sessions={sessions}
          unseen={unseen}
          onSelect={setActiveTabId}
          renameId={renameTabId}
          onCreate={() =>
            void api
              .createTab(t('tabs.defaultName', { n: tabs.length + 1 }))
              .then((id) => {
                setActiveTabId(id);
                setRenameTabId(id);
              })
              .catch((e) => setError(t('error.newTab', { err: String(e) })))
          }
          onRename={(id, name) => {
            setRenameTabId(null);
            void api.renameTab(id, name);
          }}
          onClose={(id) => void api.closeTab(id).catch((e) => setError(String(e)))}
        />

        <header className="topbar">
          {view === 'terminal' && active ? (
            <>
              <span className={`dot ${active.status}`} />
              <strong>{active.title}</strong>
              {/* A session running with fewer prompts wears it here too —
                  this bar names what you are supervising, and quiet
                  autonomy in exactly this view would be the worst kind. */}
              {(() => {
                const mode = attemptBySession.get(active.id);
                if (!mode || mode === 'normal') return null;
                return (
                  <span
                    className={`mode-badge ${mode}`}
                    data-testid="topbar-mode"
                    title={t(mode === 'yolo' ? 'mode.yolo' : 'mode.accept_edits')}
                  >
                    <Icon name={mode === 'yolo' ? 'bolt' : 'pencil'} />
                  </span>
                );
              })()}
              {/* 會截斷的路徑就要有 title —— 與其他截斷路徑同一條規矩。 */}
              <span className="muted mono" title={active.cwd}>
                {active.cwd}
              </span>
            </>
          ) : (
            <strong>
              {view === 'overview'
                ? t('view.overview')
                : view === 'board'
                  ? t('view.board')
                  : t('sidebar.empty')}
            </strong>
          )}
          <span className="spacer" />
          {view === 'terminal' && (activeAttemptId || inspected) && (
            <button
              className={inspectId ? 'active' : ''}
              data-testid="toggle-inspector"
              aria-pressed={inspectId !== null}
              title={`${t('view.inspector')} (⌘/Ctrl+I)`}
              onClick={() => setInspectId(inspectId ? null : activeAttemptId)}
            >
              {t('view.inspector')}
            </button>
          )}
          {view === 'terminal' && <ColumnPicker layout={layout} onPick={onPickCols} />}
          <div
            className="view-toggle"
            role="tablist"
            // role="tab" promises arrow keys, so the promise is kept.
            onKeyDown={(e) => {
              if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
              e.preventDefault();
              const order: View[] = ['terminal', 'board', 'overview'];
              const i = order.indexOf(view);
              const next = order[(i + (e.key === 'ArrowRight' ? 1 : 2)) % 3];
              setView(next);
              (e.currentTarget.children[order.indexOf(next)] as HTMLElement)?.focus();
            }}
          >
            <button
              role="tab"
              aria-selected={view === 'terminal'}
              tabIndex={view === 'terminal' ? 0 : -1}
              className={view === 'terminal' ? 'active' : ''}
              title={`${t('view.terminal')} (⌘/Ctrl+1)`}
              onClick={() => setView('terminal')}
            >
              {t('view.terminal')}
            </button>
            <button
              role="tab"
              aria-selected={view === 'board'}
              tabIndex={view === 'board' ? 0 : -1}
              className={view === 'board' ? 'active' : ''}
              data-testid="view-board"
              title={`${t('view.board')} (⌘/Ctrl+2)`}
              onClick={() => setView('board')}
            >
              {t('view.board')}
            </button>
            <button
              role="tab"
              aria-selected={view === 'overview'}
              tabIndex={view === 'overview' ? 0 : -1}
              className={view === 'overview' ? 'active' : ''}
              title={`${t('view.overview')} (⌘/Ctrl+3)`}
              onClick={() => setView('overview')}
            >
              {t('view.overview')}
            </button>
          </div>
        </header>

        {/* One row for every view, so the board can keep a live peek beside
            it: the terminals stay mounted (unmounting would dispose their
            scrollback), the board sits to their left when it is up. */}
        <div className="content-row">
        {view === 'board' && (
          <Board
            tasks={tasks}
            sessions={sessions}
            unseen={unseen}
            onOpenSession={onOpen}
            onPreview={setPreviewId}
            onMove={onMoveTask}
            onStart={(task) => {
              setDialogError(null);
              setStarting(task);
            }}
            onResume={onResumeAttempt}
            onPark={onParkAttempt}
            onResumeParked={onResumeParked}
            onInspect={onInspectAttempt}
            onCancelQueued={onCancelQueued}
            onNewTask={() => {
              setDialogError(null);
              setShowNewTask(true);
            }}
            onDeleteTask={onDeleteTask}
            onCloseSession={(id) => void api.closeSession(id)}
            onArchiveSession={(id) => void api.archiveSession(id)}
            onAnnounce={say}
            focusTaskId={boardFocusId}
            onFocusedTask={() => setBoardFocusId(null)}
          />
        )}

        {view === 'overview' && (
          <Overview
            sessions={sessions}
            unseen={unseen}
            onOpen={onOpen}
            onComplete={(id, completed) => void api.setCompleted(id, completed)}
            onClose={(id) => void api.closeSession(id)}
            onRename={(id, title) => void api.renameSession(id, title)}
          />
        )}

        <div
          className={`term-area${view === 'board' && previewSession ? ' as-preview' : ''}`}
          style={{
            display: view === 'terminal' || previewSession !== null ? 'flex' : 'none',
          }}
        >
        <div
          className="term-stack"
          ref={gridRef}
          data-mode={manual ? 'manual' : 'auto'}
          data-cols={manual ? undefined : cols}
        >
          <div
            className={`term-grid${manual ? ' manual' : ''}`}
            style={
              manual || zoomed
                ? undefined
                : { display: 'grid', ...gridStyle(cols, rows) }
            }
          >
            {liveIds.map((id) => {
              const session = sessions.find((s) => s.id === id);
              if (!session) return null;
              const index = members.indexOf(id);
              const shown = index >= 0 && (zoomed === null || zoomed === id);
              // The board's peek: this pane, alone, filling the preview
              // column — zoomed's geometry, whatever tab or layout it is
              // in, because a peek must not depend on the wall's slots.
              const previewing = view === 'board' && id === previewSession;

              let style: React.CSSProperties | undefined;
              if (zoomed === id || previewing) {
                style = { position: 'absolute', inset: 0 };
              } else if (manual) {
                const r = geom?.panes.get(id);
                style = r
                  ? { position: 'absolute', left: r.x, top: r.y, width: r.w, height: r.h }
                  : undefined;
              } else {
                // Panes stay in creation order in the DOM and are placed by
                // `order`, so rearranging never re-parents a terminal.
                style = { order: index };
              }

              return (
                <Pane
                  key={id}
                  session={session}
                  mode={attemptBySession.get(id) ?? null}
                  visible={(view === 'terminal' && shown) || previewing}
                  // Only on the wall: a peeked pane holds no caret, so keys
                  // pressed over the board can never reach an agent — and
                  // walking through a card re-focuses the terminal even
                  // though the peek already had it visible.
                  focused={id === focusedId && view === 'terminal'}
                  zoomed={zoomed === id}
                  style={style}
                  onFocus={() => setFocused(id)}
                  onToggleZoom={() => {
                    setFocused(id);
                    setZoomedId((z) => (z === id ? null : id));
                  }}
                  onDrop={(p, zone) => onDropOnPane(id, p, zone)}
                  onEject={() => onEject(id)}
                />
              );
            })}

            {members.length === 0 && (
              <EmptyGrid onDrop={onDropOnGrid} anySessions={sessions.length > 0} />
            )}

            {/* A pane-relative drop always splits below the pane it landed on,
                so without these there is no gesture that makes something span
                the whole layout — a shape the tree can hold perfectly well. */}
            {dragging &&
              !zoomed &&
              members.length > 0 &&
              (['top', 'bottom', 'left', 'right'] as const).map((zone) => (
                <EdgeDrop key={zone} zone={zone} onDrop={onDropOnEdge} />
              ))}

            {manual &&
              !zoomed &&
              view === 'terminal' &&
              geom?.handles.map((h) => {
                const node = nodeAt(shownRoot, h.path);
                if (!node || !isSplit(node)) return null;
                return (
                  <Splitter
                    key={`${h.path.join('.')}-${h.index}`}
                    handle={h}
                    fr={node.fr}
                    onPreview={(path, fr) => setLiveRoot(setFractions(root, path, fr))}
                    onCommit={(path, fr) => {
                      setLiveRoot(undefined);
                      commit({ mode: 'manual', root: setFractions(root, path, fr) }, members);
                    }}
                    onReset={(path) =>
                      commit({ mode: 'manual', root: resetFractions(root, path) }, members)
                    }
                  />
                );
              })}
          </div>
        </div>

        {/* Beside the terminal, never instead of it: the terminal stays
            mounted and one click away, so a follow-up after reading the diff
            is typing rather than navigating. */}
        {inspected && (
          <AttemptInspector
            attempt={inspected}
            session={sessions.find((s) => s.id === inspected.session_id) ?? null}
            baseBranch={
              tasks.find((t) => t.id === inspected.task_id)?.base_branch ?? 'base'
            }
            // Every base the merge will actually write to. One for nearly
            // every card; a card spanning a service and its client merges
            // into both, and a button naming only the first would be naming
            // half of what the second click does.
            bases={(() => {
              const task = tasks.find((t) => t.id === inspected.task_id);
              return task ? taskRepos(task).map((r) => r.base_branch) : [];
            })()}
            comments={reviewDrafts[inspected.id] ?? []}
            onComments={(c) => setReviewDrafts((d) => ({ ...d, [inspected.id]: c }))}
            viewed={reviewViewed[inspected.id] ?? []}
            onViewed={(files) => setReviewViewed((v) => ({ ...v, [inspected.id]: files }))}
            onClose={() => setInspectId(null)}
            // 終局不丟包:抽屜連著按鈕一起卸載時,鍵盤要有安排好的落點。
            // 牆上還有別的活 pane 就交給它(focusedId 落回 members[0],
            // 不把人從工作中的牆上拽走);牆會空掉時,切到看板、落在剛
            // 判定的那張卡上 —— 判決在哪,焦點就在哪。
            onDone={() => {
              const survivors = members.filter((m) => m !== inspected.session_id);
              setInspectId(null);
              if (view !== 'terminal' || survivors.length === 0) {
                setView('board');
                setBoardFocusId(inspected.task_id);
              }
            }}
            // The loop's peak action gets its confirmation moment: the drawer
            // closing alone reads as "gone", not "landed".
            onMerged={(branch) => pushToast('ok', t('inspector.merged', { branch }))}
            onRunScript={(name) => void onRunScript(inspected.id, name)}
            onOpenShell={() => void onOpenShell(inspected.id)}
            onPark={() => void onParkAttempt(inspected.id)}
            previewState={
              inspectedPreview === null ? null : inspectedPreview === 'ssh' ? 'ssh' : 'ready'
            }
            onOpenPreview={() => {
              if (inspectedPreview !== null && inspectedPreview !== 'ssh') {
                setPick(null);
                setDevPreview({ ...inspectedPreview, agentSessionId: inspected.session_id });
              }
            }}
          />
        )}

        {devPreview !== null &&
          (() => {
            const teller = devPreview.agentSessionId
              ? sessions.find((s) => s.id === devPreview.agentSessionId)
              : undefined;
            const canTell =
              pick !== null && teller !== undefined && teller.live && followupSendable(teller.agent);
            return (
              <PreviewPanel
                port={devPreview.port}
                live={sessions.some((s) => s.id === devPreview.sessionId && s.live)}
                pick={pick}
                onTell={
                  canTell
                    ? () => {
                        void api
                          .sendFollowup(
                            teller.id,
                            t('preview.note', {
                              component: pick.component,
                              file: pick.file,
                              line: pick.line,
                            }),
                          )
                          // The terminal this would land in may be off
                          // screen; a failed tell must not die silently.
                          .catch((e) => pushToast('error', String(e)));
                        setPick(null);
                      }
                    : null
                }
                onDismissPick={() => setPick(null)}
                onClose={() => {
                  setDevPreview(null);
                  setPick(null);
                }}
              />
            );
          })()}
        </div>
        </div>
      </main>

      <div className="visually-hidden" aria-live="polite" data-testid="live-announce">
        {announce}
      </div>

      {toasts.length > 0 && (
        <div className="toast-stack">
          {toasts.length > 3 && (
            <button
              className="toast toast-more"
              data-testid="toast-more"
              onClick={() => setToasts([])}
            >
              {t('toast.more', { count: toasts.length - 3 })}
            </button>
          )}
          {toasts.slice(-3).map((toast) => (
            <div
              key={toast.id}
              className={`toast ${toast.kind === 'error' ? 'error' : 'ok'}`}
              role={toast.kind === 'error' ? 'alert' : 'status'}
            >
              <span>{toast.text}</span>
              <button
                aria-label={t('common.close')}
                onClick={() => setToasts((cur) => cur.filter((x) => x.id !== toast.id))}
              >
                ✕
              </button>
            </div>
          ))}
        </div>
      )}

      {showNew && <NewSessionDialog onCancel={() => setShowNew(false)} onCreate={onCreate} />}
      {showNewTask && (
        <NewTaskDialog
          error={dialogError}
          goal={composed}
          onCancel={() => {
            setShowNewTask(false);
            setComposed('');
          }}
          onCreate={onCreateTask}
          onCreateAndStart={onCreateAndStart}
        />
      )}
      {starting && (
        <StartAttemptDialog
          task={starting}
          error={dialogError}
          onCancel={() => setStarting(null)}
          onStart={(agent, prompt, mode) => void onStartAttempt(starting, agent, prompt, mode)}
        />
      )}
      {showSettings && (
        <SettingsPanel
          boot={boot}
          onClose={() => setShowSettings(false)}
          // 從環境面板走去歡迎面板:先關這扇門再開那扇 —— 兩層 modal
          // 疊著,Esc 與焦點圈就說不清楚誰的了。
          onShowWelcome={() => {
            setShowSettings(false);
            reopenWelcome();
          }}
          // 重播導覽不關面板:忘掉記號是瞬間的事,而下一課要等它自己
          // 的那一刻才到 —— 現在唯一該發生的就是一句「收到了」。
          onReplayCoach={() => {
            clearCoachSeen();
            pushToast('ok', t('coach.replayed'));
          }}
        />
      )}
      {showKeys && <ShortcutsDialog onClose={() => setShowKeys(false)} />}
      {showPalette && (
        <CommandPalette
          sessions={sessions}
          tasks={tasks}
          unseen={unseen}
          ctx={paletteCtx}
          onOpenSession={paletteOpenSession}
          onOpenBoard={(taskId) => {
            setShowPalette(false);
            setView('board');
            // 只切視圖會把焦點留在 <body>:跳去 session 的路把焦點交給
            // 終端機,跳去卡片的路也要把焦點交到卡片手上。
            setBoardFocusId(taskId);
          }}
          onRun={paletteRun}
          // 打一句話就開卡:面板是唯一從任何地方都到得了的表面,所以也是
          // 這張桌子從「想到一件事」到「有個 agent 在做」最短的一條路。
          onCompose={(goal) => {
            setShowPalette(false);
            setDialogError(null);
            setComposed(goal);
            setView('board');
            setShowNewTask(true);
          }}
          onCancel={paletteCancel}
        />
      )}
      {showWelcome && (
        <WelcomeDialog
          boot={boot}
          onReprobe={reprobe}
          onClose={() => {
            localStorage.setItem(WELCOME_KEY, '1');
            setShowWelcome(false);
          }}
          onNewTask={() => {
            localStorage.setItem(WELCOME_KEY, '1');
            setShowWelcome(false);
            setView('board');
            setDialogError(null);
            setShowNewTask(true);
          }}
          onNewSession={() => {
            localStorage.setItem(WELCOME_KEY, '1');
            setShowWelcome(false);
            setShowNew(true);
          }}
        />
      )}

      {/* One coaching card at most, above the board but under any dialog —
          teaching must never block the thing it teaches. */}
      {coach && (
        <CoachMark
          id={coach}
          onDismiss={() => {
            markCoachSeen(coach);
            setCoach(null);
          }}
        />
      )}
    </div>
  );
}
