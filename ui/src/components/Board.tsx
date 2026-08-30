import { useEffect, useMemo, useRef, useState } from 'react';
import { api, type AttemptStat } from '../api';
import { isMeasured } from '../agents';
import { useT } from '../i18n';
import { useArmed } from './armed';
import { Icon } from './Icon';
import type { Attempt, Lifecycle, SessionMeta, Task } from '../types';
import { needsYou, taskRepos } from '../types';
import {
  columnOf,
  COLUMN_KEY,
  COLUMNS,
  dropIndex,
  hostLabel,
  liveLabel,
  liveStateOf,
  liveTone,
  looseOf,
  repoName,
  STATUS_KEY,
  TASK_MIME,
  type Live,
} from '../board';
import { elapsed } from '../sections';
import { nextAction, NEXT_KEY } from '../next';

interface Props {
  tasks: Task[];
  sessions: SessionMeta[];
  /** Sessions that finished a turn while their terminal was unwatched. */
  unseen: ReadonlySet<string>;
  /** Go to this session's terminal, with the caret in it. */
  onOpenSession: (id: string) => void;
  /** Peek at this session beside the board — hover or focus says which. */
  onPreview: (id: string) => void;
  onMove: (id: string, lifecycle: Lifecycle, position: number) => void;
  onStart: (task: Task) => void;
  onResume: (attemptId: string) => void;
  /** Park: keep the work and the conversation, give back the ground. */
  onPark: (attemptId: string) => void;
  /** Wake a parked attempt: worktree back, shelf restored, terminal on. */
  onResumeParked: (attemptId: string) => void;
  /** Open the diff and timeline for this attempt, beside its terminal. */
  onInspect: (attempt: Attempt) => void;
  onCancelQueued: (taskId: string) => void;
  onNewTask: () => void;
  onDeleteTask: (id: string) => void;
  /** End a card-less session's terminal. */
  onCloseSession: (id: string) => void;
  /** Take a finished card-less session off the board for good. */
  onArchiveSession: (id: string) => void;
  /** Say something through the app's aria-live channel. */
  onAnnounce: (text: string) => void;
  /** App 請看板聚焦這張卡 —— 面板選了沒有終端機的卡、或剛建立的新卡。
   *  聚焦落在卡片的門上(沒有門就落在卡片本身),完成後以 onFocusedTask
   *  回報,讓 App 清掉請求。 */
  focusTaskId?: string | null;
  onFocusedTask?: () => void;
}

/**
 * The board.
 *
 * Two axes, kept apart on purpose. The column is where a person put the card;
 * the light is what the agent's session is reporting right now. Nothing the
 * agent reports moves a card — `Stop` means "this turn ended", not "the work
 * is done" — so the only thing that changes a column is a drag.
 *
 * What this buys, and what neither a board nor a row of terminal tabs can do
 * on its own: a card sitting in 進行中 lights up 「⚠ 等你授權」by itself, and
 * clicking it puts you in the live TUI with the caret already there.
 */
export function Board({
  tasks,
  sessions,
  unseen,
  onOpenSession,
  onPreview,
  onMove,
  onStart,
  onResume,
  onPark,
  onResumeParked,
  onInspect,
  onCancelQueued,
  onNewTask,
  onDeleteTask,
  onCloseSession,
  onArchiveSession,
  onAnnounce,
  focusTaskId,
  onFocusedTask,
}: Props) {
  const t = useT();
  const [dragId, setDragId] = useState<string | null>(null);
  const [over, setOver] = useState<{ col: Lifecycle; taskId: string | null } | null>(null);
  /** The card to refocus once a keyboard move re-renders it in its new
   *  column — reparenting unmounts the node, and focus must follow the
   *  card, not fall on the floor.
   *  `door` 記的是落點:鍵盤搬移聚焦卡片本身(⌘←→ 要繼續落在群組上),
   *  App 的請求則落在門上 —— Enter 進門、Tab 就到旁邊的按鈕。 */
  const refocus = useRef<{ id: string; door: boolean } | null>(null);

  // App 的聚焦請求與鍵盤搬移共用同一套 refocus 機制:剛建立的卡片是
  // 廣播送達的,effect 掛在 tasks 上,卡片一進列表就會被接住。
  useEffect(() => {
    if (focusTaskId) refocus.current = { id: focusTaskId, door: true };
  }, [focusTaskId]);

  useEffect(() => {
    if (!refocus.current) return;
    const el = document.querySelector<HTMLElement>(
      `[data-testid="task-${refocus.current.id}"]`,
    );
    if (el) {
      const door = refocus.current.door
        ? el.querySelector<HTMLElement>('.card-door')
        : null;
      (door ?? el).focus();
      const wasRequest = refocus.current.door;
      refocus.current = null;
      if (wasRequest) onFocusedTask?.();
    }
  }, [tasks, focusTaskId, onFocusedTask]);

  /**
   * The keyboard's way across the board. Dragging is the gesture; this is
   * the path — the review loop is fully keyboard-driven, and filing the
   * card as done must not be the one act that demands a mouse.
   */
  const moveByKey = (task: Task, step: 1 | -1) => {
    const i = COLUMNS.indexOf(task.lifecycle);
    const next = COLUMNS[i + step];
    if (!next) return;
    onMove(task.id, next, columnOf(tasks, next).length);
    refocus.current = { id: task.id, door: false };
    onAnnounce(t('board.movedTo', { title: task.title, col: t(COLUMN_KEY[next]) }));
  };

  /** And the way up and down inside one: an order the drag could always
   *  say, finally sayable by the keyboard too. */
  const reorderByKey = (task: Task, step: 1 | -1) => {
    const column = columnOf(tasks, task.lifecycle);
    const at = column.findIndex((x) => x.id === task.id);
    const to = at + step;
    if (at < 0 || to < 0 || to >= column.length) return;
    onMove(task.id, task.lifecycle, to);
    refocus.current = { id: task.id, door: false };
    onAnnounce(t('board.reordered', { title: task.title, n: to + 1 }));
  };

  const running = sessions.filter((s) => s.live && s.attempt_id !== null).length;

  // One timer drives every blocked card's elapsed readout, same as the
  // sidebar's — triage is the board's whole job, and "how long has this one
  // been stuck" is the number triage runs on.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);

  /** Each open attempt's footprint: +N −M and where its branch stands
   *  against the base. Cheap numstat calls, never the rendered diff, and a
   *  chain of timeouts rather than an interval — a tick that reschedules
   *  only after it finishes cannot pile up behind a slow host (WSL, SSH). */
  const [stats, setStats] = useState<Record<string, AttemptStat>>({});
  const openIds = useMemo(
    () =>
      tasks
        .flatMap((task) => task.attempts)
        .filter((a) => a.outcome === null)
        .map((a) => a.id)
        .sort()
        .join(' '),
    [tasks],
  );
  useEffect(() => {
    const ids = openIds === '' ? [] : openIds.split(' ');
    if (ids.length === 0) {
      setStats({});
      return;
    }
    let stop = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = async () => {
      const results = await Promise.allSettled(ids.map((id) => api.attemptStats(id)));
      if (stop) return;
      const next: Record<string, AttemptStat> = {};
      results.forEach((r, i) => {
        // A refusal (worktree mid-setup or mid-teardown) costs a badge,
        // never a toast — the card's status line already tells that story.
        if (r.status === 'fulfilled') next[ids[i]] = r.value;
      });
      setStats(next);
      timer = setTimeout(() => void tick(), 15000);
    };
    void tick();
    return () => {
      stop = true;
      if (timer) clearTimeout(timer);
    };
  }, [openIds]);

  /**
   * Which card moved, and where it lands.
   *
   * The id comes off the drag itself rather than out of React state. State is
   * what `dragging` and the insertion marker are for — appearance — and a
   * render between `dragstart` and `drop` is not something a drop handler
   * should have to depend on having happened.
   */
  const drop = (e: React.DragEvent, col: Lifecycle, overId: string | null) => {
    const id = e.dataTransfer.getData(TASK_MIME) || dragId;
    setDragId(null);
    setOver(null);
    if (!id) return;
    onMove(id, col, dropIndex(columnOf(tasks, col), id, overId));
  };

  return (
    <div className="board" data-testid="board">
      <Concurrency running={running} tasks={tasks} />
      <div className="board-cols">
        {COLUMNS.map((col) => {
          const cards = columnOf(tasks, col);
          /* Sessions opened without a card sit in the same columns, sorted by
             what they are doing rather than by where anyone put them. They
             used to live in a strip of their own below the board, which meant
             the one surface built for "who needs me" answered only for half
             the running agents. */
          const loose = looseOf(sessions, col);
          return (
            <section
              key={col}
              className={`board-col${over?.col === col ? ' drop-over' : ''}`}
              data-col={col}
              data-testid={`col-${col}`}
              onDragOver={(e) => {
                if (!e.dataTransfer.types.includes(TASK_MIME)) return;
                e.preventDefault();
                setOver({ col, taskId: null });
              }}
              onDragLeave={(e) => {
                // Only when the pointer has actually left the column, not when
                // it crosses onto a card inside it.
                if (!e.currentTarget.contains(e.relatedTarget as globalThis.Node | null)) {
                  setOver((o) => (o?.col === col ? null : o));
                }
              }}
              // The column's own space appends; a card handles its own drop
              // and inserts before itself.
              onDrop={(e) => {
                e.preventDefault();
                drop(e, col, null);
              }}
            >
              <h2 className="board-col-head">
                {t(COLUMN_KEY[col])}
                <span className="section-count">{cards.length + loose.length}</span>
                {col === 'backlog' && (
                  <button className="icon" onClick={onNewTask} title={t('board.newCard')} aria-label={t('board.newCard')}>
                    ＋
                  </button>
                )}
              </h2>
              <div className="board-cards">
                {cards.map((task) => (
                  <Card
                    key={task.id}
                    task={task}
                    live={liveStateOf(task, sessions)}
                    stats={stats}
                    unseen={unseen}
                    dragging={dragId === task.id}
                    insertBefore={over?.col === col && over.taskId === task.id}
                    onDragStart={(e) => {
                      e.dataTransfer.setData(TASK_MIME, task.id);
                      e.dataTransfer.effectAllowed = 'move';
                      setDragId(task.id);
                    }}
                    onDragEnd={() => {
                      setDragId(null);
                      setOver(null);
                    }}
                    onDragOver={(e) => {
                      if (!e.dataTransfer.types.includes(TASK_MIME)) return;
                      e.preventDefault();
                      e.stopPropagation();
                      setOver({ col, taskId: task.id });
                    }}
                    onDrop={(e) => {
                      e.preventDefault();
                      e.stopPropagation();
                      drop(e, col, task.id);
                    }}
                    onOpenSession={onOpenSession}
                    onPreview={onPreview}
                    onStart={() => onStart(task)}
                    onResume={onResume}
                    onPark={onPark}
                    onResumeParked={onResumeParked}
                    onInspect={onInspect}
                    onCancelQueued={onCancelQueued}
                    onDelete={() => onDeleteTask(task.id)}
                    onMoveByKey={(step) => moveByKey(task, step)}
                    onReorderByKey={(step) => reorderByKey(task, step)}
                    now={now}
                  />
                ))}
                {loose.map((s) => (
                  <SessionCard
                    key={s.id}
                    session={s}
                    unread={unseen.has(s.id)}
                    now={now}
                    onOpen={() => onOpenSession(s.id)}
                    onPreview={() => s.live && onPreview(s.id)}
                    onClose={() => onCloseSession(s.id)}
                    onArchive={() => onArchiveSession(s.id)}
                  />
                ))}
                {cards.length === 0 &&
                  loose.length === 0 &&
                  (col === 'backlog' ? (
                    // The empty backlog is a door, not a caption: the words
                    // already say "add a card", so the words are the button.
                    <button
                      className="board-empty board-cta muted small"
                      data-testid="board-cta"
                      onClick={onNewTask}
                    >
                      {t('board.emptyBacklog')}
                    </button>
                  ) : (
                    <p className="board-empty muted small">{t('board.emptyDrop')}</p>
                  ))}
              </div>
            </section>
          );
        })}
      </div>
    </div>
  );
}

/**
 * A session that was opened without a card.
 *
 * It wears the card's shape so one column reads as one list, and says the two
 * things a card says that it cannot: no worktree behind it, and no branch of
 * its own. Everything else a card offers — start, park, inspect, merge —
 * belongs to attempts, so none of it appears here.
 */
function SessionCard({
  session: s,
  unread,
  now,
  onOpen,
  onPreview,
  onClose,
  onArchive,
}: {
  session: SessionMeta;
  unread: boolean;
  now: number;
  onOpen: () => void;
  onPreview: () => void;
  onClose: () => void;
  onArchive: () => void;
}) {
  const t = useT();
  const waiting = needsYou(s.status);
  const astir = !waiting && (s.status === 'running' || s.status === 'starting');
  const since = elapsed(s.last_active_at, now);

  return (
    <article
      className={[
        'board-card',
        'loose-card',
        waiting ? 'needs-you' : '',
        astir ? 'astir' : '',
        'enterable',
      ]
        .filter(Boolean)
        .join(' ')}
      data-testid={`loose-${s.id}`}
      data-live={s.live ? 'session' : 'stopped'}
      role="group"
      tabIndex={-1}
      aria-label={`${s.title}${t('common.sep')}${
        waiting ? `${t('board.needsYou')}${t('common.sep')}` : ''
      }${t(STATUS_KEY[s.status])}${unread ? `${t('common.sep')}${t('unseen.label')}` : ''}`}
      onMouseEnter={onPreview}
      onFocus={onPreview}
    >
      <header className="board-card-head">
        <span className={`dot ${s.status}`} />
        <button className="card-door board-card-title" onClick={onOpen}>
          {s.title}
        </button>
        {unread && (
          <span className="unseen-dot" data-testid={`unseen-loose-${s.id}`} title={t('unseen.label')} />
        )}
        <span className="ov-agent mono">{s.agent}</span>
      </header>

      {/* Where it is running, in place of the card's repo-and-branch row: a
          session has a directory and nothing else to say about git. */}
      <div className="board-card-repo mono small muted" title={s.cwd}>
        <span className="board-card-where">
          {hostLabel(s.cwd) && <span className="host-badge">{hostLabel(s.cwd)} · </span>}
          {repoName(s.cwd)}
        </span>
      </div>

      <div className="board-card-state" data-testid={`loose-state-${s.id}`}>
        {waiting && (
          <>
            <Icon name="warn" />{' '}
          </>
        )}
        {t(STATUS_KEY[s.status])}
        {waiting && since && <span className="card-elapsed"> · {since}</span>}
      </div>

      <div className="board-card-foot">
        {s.live ? (
          <button className="quiet" onClick={stop(onClose)}>
            {t('sidebar.closeTerminal')}
          </button>
        ) : (
          <button className="quiet" onClick={stop(onArchive)}>
            {t('sidebar.removeFromList')}
          </button>
        )}
      </div>
    </article>
  );
}

/**
 * How many attempts may hold a terminal at once.
 *
 * The thing being rationed is a person, not a machine — this is an attention
 * scheduler, and past three or four live TUIs nobody is keeping a thread on
 * all of them. Cards over the limit wait and then go by themselves.
 */
function Concurrency({ running, tasks }: { running: number; tasks: Task[] }) {
  const t = useT();
  const [max, setMax] = useState<number | null>(null);
  const queued = tasks.filter((t) => t.queued_at !== null).length;

  // Re-read whenever the running count moves: raising the limit releases what
  // was waiting, and the number here should agree with the board.
  useEffect(() => {
    void api
      .concurrency()
      .then((c) => setMax(c.max))
      .catch(() => setMax(null));
  }, [running, queued]);

  const change = (next: number) => {
    setMax(next);
    void api.setConcurrency(next).catch(() => {
      /* the next read puts it back */
    });
  };

  if (max === null) return null;
  return (
    <div className="board-limit" data-testid="concurrency">
      <span className="muted small">{t('board.concurrency')}</span>
      <button
        className="icon"
        disabled={max <= 1}
        aria-label={t('board.less')}
        onClick={() => change(max - 1)}
      >
        −
      </button>
      <strong data-testid="concurrency-max">
        {running} / {max}
      </strong>
      <button className="icon" aria-label={t('board.more')} onClick={() => change(max + 1)}>
        ＋
      </button>
      {queued > 0 && (
        <span className="muted small" data-testid="queue-count">
          {t('board.queued', { count: queued })}
        </span>
      )}
    </div>
  );
}

function Card({
  task,
  live,
  stats,
  unseen,
  dragging,
  insertBefore,
  onDragStart,
  onDragEnd,
  onDragOver,
  onDrop,
  onOpenSession,
  onPreview,
  onStart,
  onResume,
  onPark,
  onResumeParked,
  onInspect,
  onCancelQueued,
  onDelete,
  onMoveByKey,
  onReorderByKey,
  now,
}: {
  task: Task;
  live: Live;
  stats: Record<string, AttemptStat>;
  unseen: ReadonlySet<string>;
  dragging: boolean;
  insertBefore: boolean;
  onDragStart: (e: React.DragEvent) => void;
  onDragEnd: () => void;
  onDragOver: (e: React.DragEvent) => void;
  onDrop: (e: React.DragEvent) => void;
  onOpenSession: (id: string) => void;
  onPreview: (id: string) => void;
  onStart: () => void;
  onResume: (attemptId: string) => void;
  onPark: (attemptId: string) => void;
  onResumeParked: (attemptId: string) => void;
  onInspect: (attempt: Attempt) => void;
  onCancelQueued: (taskId: string) => void;
  onDelete: () => void;
  onMoveByKey: (step: 1 | -1) => void;
  onReorderByKey: (step: 1 | -1) => void;
  now: number;
}) {
  const t = useT();
  const waiting = live.kind === 'session' && needsYou(live.status);
  const hasAttempt = live.kind !== 'none' && live.kind !== 'queued';
  const agent = hasAttempt ? live.attempt.agent : null;
  const stat = hasAttempt ? stats[live.attempt.id] : undefined;
  /** Its session finished a turn while unwatched. A stopped card keys by
   *  the attempt's session — the CLI that exited is exactly the unread
   *  ending worth a dot. */
  const unread =
    live.kind === 'session'
      ? unseen.has(live.session.id)
      : live.kind === 'stopped' && live.attempt.session_id !== null
        ? unseen.has(live.attempt.session_id)
        : false;
  const del = useArmed(onDelete);

  // The whole card is the target when there is a session behind it: getting
  // into the TUI is the common act, and making people find a small button
  // for it would be the wrong thing to optimise.
  //
  // `detached` is a door too, and it was the one card that said so and then
  // did not open. tmux is still holding that agent; clicking reattaches to
  // the run in progress rather than starting anything. A card whose state
  // reads 「執行中」 and offers no way in is worse than one that reads
  // 「未執行」, because it is the state where something is actually at stake.
  const enter =
    live.kind === 'session' || live.kind === 'detached'
      ? () => onOpenSession(live.session.id)
      : undefined;

  /** A live turn in flight: parking checks this, and now so does the ✕ —
   *  deleting the card would take the running session and its worktree
   *  with it, and that is not a stray-click kind of loss. */
  const busy =
    live.kind === 'session' &&
    live.status !== 'idle' &&
    live.status !== 'saved' &&
    live.status !== 'exited';

  // The shimmer: mid-turn, and only mid-turn — the breath outranks it, so
  // a card that is both blocked and busy breathes and does not shimmer.
  const astir =
    !waiting &&
    live.kind === 'session' &&
    (live.session.status === 'running' || live.session.status === 'starting');

  return (
    <article
      className={[
        'board-card',
        waiting ? 'needs-you' : '',
        astir ? 'astir' : '',
        dragging ? 'dragging' : '',
        insertBefore ? 'insert-before' : '',
        enter ? 'enterable' : '',
      ]
        .filter(Boolean)
        .join(' ')}
      data-testid={`task-${task.id}`}
      data-lifecycle={task.lifecycle}
      data-live={live.kind}
      data-outcome={live.kind === 'finished' ? (live.attempt.outcome ?? undefined) : undefined}
      // A labeled group, not a button holding buttons. The label is title
      // plus state — the one thing the breathing card shouts must not be
      // silent to AT. Enterable cards put their tab stop on the door; the
      // rest stay focusable themselves so ⌘←/→ still has somewhere to land.
      role="group"
      tabIndex={enter ? -1 : 0}
      // 「⚠」換成型錄裡的詞：朗讀器對圖形字元的處理不可靠，一個真的
      // 詞才保證被唸到。權限模式也跟著唸（withMode）—— 徽章是啞的。
      aria-label={`${task.title}${t('common.sep')}${
        waiting ? `${t('board.needsYou')}${t('common.sep')}` : ''
      }${liveLabel(live, t, true)}${
        unread ? `${t('common.sep')}${t('unseen.label')}` : ''
      }`}
      onKeyDown={(e) => {
        if (!(e.metaKey || e.ctrlKey)) return;
        if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
          e.preventDefault();
          e.stopPropagation();
          onMoveByKey(e.key === 'ArrowRight' ? 1 : -1);
        } else if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
          e.preventDefault();
          e.stopPropagation();
          onReorderByKey(e.key === 'ArrowDown' ? 1 : -1);
        }
      }}
      draggable
      onDragStart={onDragStart}
      onDragEnd={onDragEnd}
      onDragOver={onDragOver}
      onDrop={onDrop}
      // The peek: pointing at a card (or landing focus on it) shows its
      // live terminal beside the board. Sticky — leaving the card keeps
      // the last peek, so glance and pointer can part ways.
      onMouseEnter={() => live.kind === 'session' && onPreview(live.session.id)}
      onFocus={() => live.kind === 'session' && onPreview(live.session.id)}
    >
      <header className="board-card-head">
        <span className={`dot ${liveTone(live)}`} />
        {/* The whole card is the target when there is a session behind it —
            as a real stretched button, so the click and the keyboard share
            one honest control instead of a clickable article. */}
        {enter ? (
          <button className="card-door board-card-title" onClick={enter}>
            {task.title}
          </button>
        ) : (
          <span className="board-card-title">{task.title}</span>
        )}
        {unread && (
          <span className="unseen-dot" data-testid={`unseen-card-${task.id}`} title={t('unseen.label')} />
        )}
        {agent && <span className="ov-agent mono">{agent}</span>}
        {/* A session running with fewer prompts wears it openly. Quiet
            autonomy that looks like ordinary supervision would be worse
            than either. */}
        {hasAttempt && live.attempt.mode !== 'normal' && (
          <span
            className={`mode-badge ${live.attempt.mode}`}
            data-testid={`mode-${task.id}`}
            title={t(live.attempt.mode === 'yolo' ? 'mode.yolo' : 'mode.accept_edits')}
          >
            <Icon name={live.attempt.mode === 'yolo' ? 'bolt' : 'pencil'} />
          </span>
        )}
      </header>

      {/* Which codebase this card is about. Cards from different repos share
          one board, and a title alone cannot say whose login page it means —
          or which machine's. A card spanning several says so with a count
          rather than a list: the row has one line, and the whole list is in
          the tooltip and the drawer. */}
      <div
        className="board-card-repo mono small muted"
        data-testid={`repo-${task.id}`}
        title={taskRepos(task)
          .map((r) => r.repo_path)
          .join('\n')}
      >
        <span className="board-card-where">
          {hostLabel(task.repo_path) && (
            <span className="host-badge">{hostLabel(task.repo_path)} · </span>
          )}
          {(task.extra_repos?.length ?? 0) > 0
            ? t('board.repoPlus', {
                name: repoName(task.repo_path),
                n: String(task.extra_repos?.length ?? 0),
              })
            : repoName(task.repo_path)}
          <span className="board-card-branch"> ⎇ {task.base_branch}</span>
        </span>
        {/* The attempt's footprint, on the row that names what it measures
            against. It sits at the far end and never shrinks, so the repo
            name is what gives way at narrow widths — the numbers triage runs
            on are the last thing that should. ↓ wears the warning color: it
            is a merge refusal you have not hit yet. */}
        {stat && (stat.adds > 0 || stat.dels > 0 || stat.ahead > 0 || stat.behind > 0) && (
          <span
            className="card-stat"
            data-testid={`stat-${task.id}`}
            title={t('stats.hint', { branch: task.base_branch })}
          >
            {stat.adds > 0 && <span className="diff-count add">+{stat.adds}</span>}
            {stat.dels > 0 && <span className="diff-count del">−{stat.dels}</span>}
            {stat.ahead > 0 && <span className="stat-ahead">↑{stat.ahead}</span>}
            {stat.behind > 0 && <span className="stat-behind">↓{stat.behind}</span>}
          </span>
        )}
      </div>

      <div className="board-card-state" data-testid={`state-${task.id}`}>
        {/* Drawn, not the unicode ⚠: the aria-label above still speaks the
            word, so AT loses nothing the eye gains in consistency. */}
        {waiting && (
          <>
            <Icon name="warn" />{' '}
          </>
        )}
        {liveLabel(live, t)}
        {hasAttempt && <span className="muted small mono"> #{live.attempt.seq}</span>}
        {/* Hooks belong to the CLIs that have them; for anyone else
            「安靜」 must never be read as 「沒事」— the absence of signal is
            itself the signal.

            The gate is `hooks_wired`, not `isMeasured`. Being measured only
            means this desk knows how to wire that CLI in principle; whether
            it *got* wired is a fact about the session — the world's own
            binary may be older than the release that grew a hooks engine, or
            the world may have no listener at all. A codex too old for hooks
            used to be treated as "measured, therefore expected to report",
            so it wore nothing and a card that would never say a word was
            indistinguishable from one working quietly. That is the one
            reading this surface must never allow.

            Still no flicker, which is what the old gate was protecting: an
            unwired session never reports, so the chip cannot withdraw
            itself. A wired one never shows it, even before its first hook. */}
        {live.kind === 'session' && !live.session.reports_status && !live.session.hooks_wired && (
          <span
            className="chip no-signal"
            data-testid={`nosignal-${task.id}`}
            title={
              isMeasured(live.attempt.agent)
                ? t('env.cliQuietWhy', { agent: live.attempt.agent })
                : t('env.cliQuietUnmeasured', { agent: live.attempt.agent })
            }
          >
            {t('env.cliQuiet')}
          </span>
        )}
        {/* How long it has been stuck — the number triage runs on, on the
            surface triage happens on, not only in the sidebar. Based on the
            last report, which for a blocked card is the moment it blocked. */}
        {waiting && live.kind === 'session' && elapsed(live.session.last_active_at, now) && (
          <span className="card-elapsed"> · {elapsed(live.session.last_active_at, now)}</span>
        )}
      </div>

      {/* One reserved line, always present, whether or not there is anything
          to put in it.

          Two things want to speak here and they are all but mutually
          exclusive: the next step is offered only when a decision is
          plausible (stopped, idle, or blocked on you), and the activity line
          only while a turn is actually running. So one row holds both, with
          the next step ahead of the activity — a decision outranks a
          description. The row exists even when both are silent, because a
          card that grows the moment its agent reaches for a tool is a card
          that moves out from under the cursor. */}
      {(() => {
        const decidable =
          live.kind === 'stopped' ||
          (live.kind === 'session' &&
            (live.status === 'idle' || needsYou(live.status)));
        const next = decidable && stat ? nextAction(stat) : null;
        const advice = next
          ? t(NEXT_KEY[next], { branch: task.base_branch, n: stat?.behind ?? 0 })
          : null;
        const activity =
          live.kind === 'session' && live.session.activity
            ? `${live.session.activity.tool} ${live.session.activity.detail}`
            : null;
        const text = advice ?? activity;
        return (
          <div
            className={`board-card-msg ${advice ? `card-next ${next}` : 'mono muted'}`}
            // Kept on the advice only: `next-<id>` means "this card is
            // offering a next step", and an activity line answering to it
            // would make the absence of advice untestable.
            data-testid={advice ? `next-${task.id}` : undefined}
            // The row clips to one line; the full sentence is a hover away,
            // and the inspector's banner says it in full besides.
            title={text ?? undefined}
          >
            {advice ?? activity}
          </div>
        );
      })()}

      <footer className="board-card-foot">
        {live.kind === 'none' && (
          <button className="primary" onClick={stop(onStart)}>
            {t('board.start')}
          </button>
        )}
        {/* Waiting for a slot. It will go on its own, so the only thing worth
            offering is a way to change your mind. */}
        {live.kind === 'queued' && (
          <button
            data-testid={`unqueue-${task.id}`}
            onClick={stop(() => onCancelQueued(task.id))}
          >
            {t('board.cancelQueue')}
          </button>
        )}
        {/* Every attempt is in this state after a restart — the app kills its
            PTYs on the way out — so resuming is a first-class button, not
            something to rediscover. It continues the agent's own history and
            does not send the prompt again. */}
        {live.kind === 'stopped' && (
          <button
            className="primary"
            data-testid={`resume-${task.id}`}
            onClick={stop(() => onResume(live.attempt.id))}
          >
            {t('board.resume')}
          </button>
        )}
        {/* Waking a parked card grows the worktree back at its old path,
            brings the shelf down, and continues the old conversation. */}
        {live.kind === 'parked' && (
          <button
            className="primary"
            data-testid={`resume-${task.id}`}
            onClick={stop(() => onResumeParked(live.attempt.id))}
          >
            {t('board.resume')}
          </button>
        )}
        {/* Park is offered exactly when restore is: a settled worktree.
            Single click — it is the reversible act; arming is for the
            irreversible ones. */}
        {(live.kind === 'stopped' ||
          (live.kind === 'session' &&
            (live.status === 'idle' || live.status === 'saved' || live.status === 'exited'))) && (
          <button
            // 停止卡的五鍵降噪:九成的答案是 Resume。Park 退到瞄準才現身
            //(hover 或鍵盤 focus 進卡)—— sidebar 列動作立下的同一條規矩,
            // CSS 只在 data-live='stopped' 時生效,其他狀態照舊全現。
            className="quiet"
            data-testid={`park-${task.id}`}
            title={t('board.parkHint')}
            onClick={stop(() => onPark(live.attempt.id))}
          >
            {t('board.park')}
          </button>
        )}
        {/* Answers "what did this one change, and what did it do" without
            reading the TUI — which is the whole job of the 待驗收 column. */}
        {hasAttempt && (
          <button
            data-testid={`inspect-${task.id}`}
            onClick={stop(() => onInspect(live.attempt))}
          >
            {t('board.inspect')}
          </button>
        )}
        {/* Another go at the same card, with a different agent. It leaves the
            attempt that is already there alone: two agents on one card, each
            in its own worktree, is a thing worth being able to do — comparing
            their diffs is the point. Deciding which one won is a separate,
            deliberate act, not a side effect of starting the second. */}
        {hasAttempt && (
          <button
            // 勝利的卡片不邀請重做:merged 之後「再試一次」退成一般按鈕,
            // primary 留給 discarded / superseded —— 還欠一次嘗試的結局。
            // 'quiet' 只在停止卡上生效(見 styles.css):換 agent 不是
            // 重啟後那一刻的答案,和 Park 一起退到瞄準才現身。
            className={
              live.kind === 'finished' && live.attempt.outcome !== 'merged' ? 'primary' : 'quiet'
            }
            data-testid={`retry-${task.id}`}
            onClick={stop(onStart)}
            title={t('board.retryHint')}
          >
            {live.kind === 'finished' ? t('board.retry') : t('board.switchAgent')}
          </button>
        )}
        <span className="spacer" />
        {busy ? (
          <button
            className="icon"
            disabled
            title={t('board.deleteBusy')}
            aria-label={t('board.deleteCard')}
          >
            ✕
          </button>
        ) : del.armed ? (
          <button
            className="confirm-delete"
            onClick={stop(del.fire)}
            data-testid={`confirm-delete-${task.id}`}
          >
            {t('board.confirmDelete')}
          </button>
        ) : (
          <button
            className="icon"
            onClick={stop(del.fire)}
            title={t('board.deleteCard')}
            aria-label={t('board.deleteCard')}
          >
            ✕
          </button>
        )}
      </footer>
    </article>
  );
}

/** Buttons inside a card must not also trigger the card's own click. */
function stop(fn: () => void) {
  return (e: React.MouseEvent) => {
    e.stopPropagation();
    fn();
  };
}
