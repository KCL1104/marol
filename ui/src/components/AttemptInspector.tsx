import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { isMeasured } from '../agents';
import { api, type AgentDoc, type AttemptStat, type Checkpoint } from '../api';
import type { Attempt, AttemptEvent, SessionMeta } from '../types';
import { useT, type MessageKey } from '../i18n';
import { useArmed } from './armed';
import { Icon } from './Icon';
import { FileEditor } from './FileEditor';
import { FriendlyError } from './FriendlyError';
import { Modal } from './Modal';
import { elapsed, tokens, STATUS_KEY } from '../sections';
import { rollup } from '../timeline';
import {
  autoCollapse,
  commentable,
  composeReview,
  followupSendable,
  parseDiff,
  tint,
  type DiffLine,
  type ReviewComment,
} from '../review';
import { nextAction, NEXT_KEY, type NextAction } from '../next';

interface Props {
  attempt: Attempt;
  /** The attempt's live session, if any — for what only a session knows:
      whether the agent is mid-turn, and whether a message is queued. */
  session: SessionMeta | null;
  /** The first repo's base, named wherever one branch is the subject: the
      ahead/behind hints, and what the next action suggests. */
  baseBranch: string;
  /** Every base the merge will actually write to, first repo first. One for
      nearly every card; a card spanning a service and its client merges into
      both, and a button naming only the first would name half of what the
      second click does. */
  bases: string[];
  /** The feedback batch in progress, held by the App keyed per attempt.
      The drawer unmounts on ⌘I and follows focus between panes — if the
      batch lived here, either act would destroy typed feedback, which is
      exactly the loss the dialogs' dirty-guard exists to prevent. */
  comments: ReviewComment[];
  onComments: (comments: ReviewComment[]) => void;
  /** Files already reviewed, held by the App for the same reason: the
      viewed marks are the reviewer's progress through a large diff, and
      ⌘I must not reset a review half walked. */
  viewed: string[];
  onViewed: (files: string[]) => void;
  onClose: () => void;
  /** The attempt ended: nothing is left to inspect here. */
  onDone: () => void;
  /** The merge landed — the one outcome worth saying out loud. */
  onMerged?: (branch: string) => void;
  /** Start one of the repo's run scripts in this attempt's worktree. */
  onRunScript: (name: string) => void;
  /** A shell of your own in this attempt's worktree. */
  onOpenShell: () => void;
  /** Park this attempt: ground given back, work and conversation kept. */
  onPark: () => void;
  /** A dev server is up in this worktree ('ready'), or up but unreachable
      because its port lives on an SSH remote ('ssh'), or absent (null). */
  previewState: 'ready' | 'ssh' | null;
  onOpenPreview: () => void;
}

type Pane = 'diff' | 'timeline' | 'knows';

/** The strip, as data. Was a hardcoded pair with a two-way arrow toggle;
 *  a third tab made the toggle a rotation, and a rotation wants a list. */
const PANES: readonly { id: Pane; title: MessageKey; testid: string }[] = [
  { id: 'diff', title: 'inspector.changes', testid: 'inspector-diff-tab' },
  { id: 'timeline', title: 'inspector.activity', testid: 'inspector-timeline-tab' },
  { id: 'knows', title: 'inspector.knows', testid: 'inspector-knows-tab' },
];

/** The line a comment is being written against. */
interface Picked {
  file: string | null;
  line: number | null;
  excerpt: string;
}

/**
 * What an attempt changed, and what it did, without reading its terminal.
 *
 * A drawer beside the TUI rather than a screen instead of it. Reviewing ends
 * in one of two things — accepting the work, or telling the agent what is
 * still wrong — and the second is only cheap if the live session is still
 * right there to type into. A review screen that replaced the terminal would
 * turn a follow-up into a navigation problem, which is the point at which
 * this stops being a session manager and becomes a board.
 *
 * Saying what is still wrong has a short path of its own: click a diff line,
 * attach feedback, and send the batch back into the session as one message.
 * The terminal is still the place for conversation; this is for the review
 * that reads the diff line by line.
 */
/** The drawer's width, remembered. Bounds keep both neighbours honest: a
 *  diff needs room to be code, and the terminal beside it needs room to
 *  stay a terminal. */
const WIDTH_KEY = 'marol.inspectorWidth';
/** 寬度的允許範圍 —— clamp 與把手的 aria-valuenow 共用同一對數字。 */
const MIN_WIDTH = 340;
const MAX_WIDTH = 900;
const clampWidth = (w: number) => Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, w));

function storedWidth(): number {
  const w = Number(localStorage.getItem(WIDTH_KEY));
  return Number.isFinite(w) && w > 0 ? clampWidth(w) : 460;
}

export function AttemptInspector({
  attempt,
  session,
  baseBranch,
  bases,
  comments,
  onComments,
  viewed,
  onViewed,
  onClose,
  onDone,
  onMerged,
  onRunScript,
  onOpenShell,
  onPark,
  previewState,
  onOpenPreview,
}: Props) {
  const t = useT();
  const [width, setWidth] = useState(storedWidth);

  /** Drag the left edge; the pane grid beside it refits as it goes. The
   *  pointer is captured so a fast drag cannot escape a 6px handle. */
  const onGripDown = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const grip = e.currentTarget;
    const startX = e.clientX;
    const startW = width;
    grip.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => setWidth(clampWidth(startW + (startX - ev.clientX)));
    const up = () => {
      grip.removeEventListener('pointermove', move);
      grip.removeEventListener('pointerup', up);
      setWidth((w) => {
        localStorage.setItem(WIDTH_KEY, String(w));
        return w;
      });
    };
    grip.addEventListener('pointermove', move);
    grip.addEventListener('pointerup', up);
  };

  /** role="separator" promises keyboard adjustability; the promise is
   *  kept — ← widens the drawer, → gives the space back. */
  const onGripKeys = (e: React.KeyboardEvent) => {
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
    e.preventDefault();
    const next = clampWidth(width + (e.key === 'ArrowLeft' ? 24 : -24));
    setWidth(next);
    localStorage.setItem(WIDTH_KEY, String(next));
  };
  const [pane, setPane] = useState<Pane>('diff');
  const [diff, setDiff] = useState<string | null>(null);
  /** When the diff on screen was read. The worktree keeps moving while you
      read — refresh is deliberate, so the staleness has to be visible. */
  const [fetchedAt, setFetchedAt] = useState<number | null>(null);
  const [events, setEvents] = useState<AttemptEvent[]>([]);
  const [error, setError] = useState<string | null>(null);
  /** The timeline's own failure, kept apart from the diff's: a fetch error
      rendered as "no activity yet" would be a lie on exactly the surface
      that audits what an agent did. */
  const [eventsError, setEventsError] = useState<string | null>(null);
  const [stat, setStat] = useState<AttemptStat | null>(null);
  const [picked, setPicked] = useState<Picked | null>(null);
  const [runScripts, setRunScripts] = useState<string[]>([]);
  /** The manual checkpoint's answer, worn briefly by its own button —
      "kept #3" or "nothing new" is the whole story, and a toast for it
      would outlive the interest. */
  const [ckptSay, setCkptSay] = useState<string | null>(null);
  const [ckptBusy, setCkptBusy] = useState(false);
  useEffect(() => {
    if (ckptSay === null) return;
    const timer = setTimeout(() => setCkptSay(null), 4000);
    return () => clearTimeout(timer);
  }, [ckptSay]);
  /** The attempt's checkpoints, for the timeline's ↩ anchors. */
  const [cps, setCps] = useState<Checkpoint[]>([]);
  /** What the last restore did (or refused), shown over the timeline until
      dismissed — it names the retreat that was kept, which outlives 4s. */
  const [restored, setRestored] = useState<string | null>(null);
  /** What the diff is measured against: 0 for the attempt's base — the
      whole story — or a checkpoint's n for "what happened since". */
  const [compareTo, setCompareTo] = useState(0);
  /** The file open for editing in place — one at a time; a drawer with
      three editors open is not an editor, it is an accident. */
  const [editing, setEditing] = useState<string | null>(null);
  /** Whether that editor holds unsaved text. Held here, not in the
      editor, because closing arrives from more than one door — the Close
      chip, the file's fold button — and every door must hit the guard. */
  const [editorDirty, setEditorDirty] = useState(false);
  const [confirmClose, setConfirmClose] = useState(false);

  // The repo's run scripts, for the ▶ buttons. Read once per attempt: the
  // config is a file in the repository, and it does not move underneath an
  // open drawer any more than the base branch does.
  useEffect(() => {
    setRunScripts([]);
    if (attempt.outcome !== null) return;
    void api
      .listRunScripts(attempt.id)
      .then(setRunScripts)
      .catch(() => {
        /* a malformed config already fails the start, loudly */
      });
  }, [attempt.id, attempt.outcome]);

  const refresh = useCallback(() => {
    setError(null);
    setEventsError(null);
    void api
      .attemptDiff(attempt.id, compareTo || undefined)
      .then((d) => {
        setDiff(d);
        setFetchedAt(Date.now());
      })
      .catch((e) => setError(String(e)));
    void api
      .attemptEvents(attempt.id)
      .then(setEvents)
      .catch((e) => {
        // The diff half is still worth showing; the timeline says what
        // actually went wrong instead of pleading empty.
        setEventsError(String(e));
      });
    // Where the branch stands against its base. Only an open attempt has a
    // worktree to measure; a refusal here (worktree mid-teardown, base
    // branch renamed) costs a badge, not the drawer.
    if (attempt.outcome === null) {
      void api
        .attemptStats(attempt.id)
        .then(setStat)
        .catch(() => setStat(null));
      // The refs behind the timeline's ↩ anchors. A finished attempt has
      // none by design — the frozen diff is its record.
      void api
        .listCheckpoints(attempt.id)
        .then(setCps)
        .catch(() => setCps([]));
    } else {
      setStat(null);
      setCps([]);
    }
  }, [attempt.id, attempt.outcome, compareTo]);

  // Read on open and whenever the attempt changes. Not on a timer: a diff
  // that reflows under you while you are reading it is worse than one you
  // asked to refresh.
  useEffect(refresh, [refresh]);

  // The line being commented on is transient; the batch is not. Comments
  // live with the App keyed per attempt, so switching attempts shows each
  // one its own batch rather than wiping anything.
  useEffect(() => {
    setPicked(null);
    setRestored(null);
    setCompareTo(0);
    setEditing(null);
    setEditorDirty(false);
    setConfirmClose(false);
  }, [attempt.id]);

  const parked = typeof attempt.parked_at === 'number';
  /** A turn in flight blocks restoring — the agent would keep believing in
      work that is no longer there — and so does parked: there is no ground
      to restore onto. The buttons stay, disabled, wearing the right reason
      — the merge-refusal pattern, ahead of the click. */
  const midTurn =
    session !== null &&
    session.live &&
    session.status !== 'idle' &&
    session.status !== 'saved' &&
    session.status !== 'exited';
  const restoreBlocked = parked ? t('park.restoreParked') : midTurn ? t('ckpt.blocked') : null;

  const doRestore = (n: number) => {
    void api
      .restoreCheckpoint(attempt.id, n)
      .then(() => {
        setRestored(n === 0 ? t('ckpt.restoredBase') : t('ckpt.restored', { n }));
        refresh();
      })
      .catch((e) => setRestored(String(e)));
  };

  /** Edit chips appear exactly where the park button does — an open,
      settled, present worktree. The core re-verifies on save; this gate
      only decides what is offered. */
  const canEdit = attempt.outcome === null && !parked && !midTurn;

  /** Every door out of the editor comes through here, so unsaved text is
      never lost to a click that meant something milder. */
  const closeEditor = () => {
    if (editorDirty) setConfirmClose(true);
    else setEditing(null);
  };

  /** The save landed: the diff on screen is stale, and so is the file's
      viewed mark — it changed, "seen" has expired. */
  const onEditSaved = (file: string) => {
    refresh();
    onViewed(viewed.filter((f) => f !== file));
  };

  return (
    <aside className="inspector" style={{ width }} data-testid="inspector">
      {/* 與 Splitter 同一種單位：aria-valuenow 用百分比（0–100），這裡
          是寬度在 340–900px 允許範圍裡的位置。拖曳與 ← → 都走 width
          state，數字跟著每次 render 更新。 */}
      <div
        className="inspector-grip"
        role="separator"
        aria-orientation="vertical"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(((width - MIN_WIDTH) / (MAX_WIDTH - MIN_WIDTH)) * 100)}
        tabIndex={0}
        data-testid="inspector-grip"
        title={t('inspector.resize')}
        aria-label={t('inspector.resize')}
      aria-keyshortcuts="ArrowLeft ArrowRight"
        onPointerDown={onGripDown}
        onKeyDown={onGripKeys}
      />
      <header className="inspector-head">
        <div
          className="view-toggle"
          role="tablist"
          // The same contract the topbar keeps: one tab stop, arrows move.
          onKeyDown={(e) => {
            if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
            e.preventDefault();
            // Wraps: a cycle with a dead end is a list, the same rule the
            // workspace tabs keep.
            const i = PANES.findIndex((p) => p.id === pane);
            const step = e.key === 'ArrowRight' ? 1 : -1;
            const next = PANES[(i + step + PANES.length) % PANES.length];
            setPane(next.id);
            (e.currentTarget.children[PANES.indexOf(next)] as HTMLElement)?.focus();
          }}
        >
          {PANES.map((p) => (
            <button
              key={p.id}
              role="tab"
              aria-selected={pane === p.id}
              tabIndex={pane === p.id ? 0 : -1}
              className={pane === p.id ? 'active' : ''}
              data-testid={p.testid}
              onClick={() => setPane(p.id)}
            >
              {t(p.title)}
            </button>
          ))}
        </div>
        <span className="spacer" />
        <button className="icon" onClick={refresh} title={t('inspector.reload')} aria-label={t('inspector.reload')}>
          <Icon name="reload" />
        </button>
        <button className="icon" onClick={onClose} title={t('common.close')} aria-label={t('inspector.closeView')}>
          ✕
        </button>
      </header>

      <div className="inspector-meta mono small muted">
        <span>{attempt.branch}</span>
        <span title={attempt.base_sha}>base {attempt.base_sha.slice(0, 8)}</span>
        {/* Where the branch stands against its base — ↓ is the one that
            matters, because it is the merge refusal you have not hit yet. */}
        {stat && stat.ahead > 0 && (
          <span title={t('stats.ahead', { n: stat.ahead, branch: baseBranch })}>↑{stat.ahead}</span>
        )}
        {stat && stat.behind > 0 && (
          <span
            className="stat-behind"
            data-testid="inspector-behind"
            title={t('stats.behind', { n: stat.behind, branch: baseBranch })}
          >
            ↓{stat.behind}
          </span>
        )}
        {/* The conversation's token account — context is where the next
            turn starts from, ↑ is what it has written so far. Tokens, not
            dollars or percentages: a price table goes stale and a context
            window we did not measure would be an invented denominator.
            Absent for agents with no transcript — honest absence. */}
        {session?.usage != null && (
          <span
            data-testid="inspector-usage"
            title={t('usage.tip', {
              context: session.usage.context.toLocaleString(),
              input: session.usage.input.toLocaleString(),
              output: session.usage.output.toLocaleString(),
              write: session.usage.cache_write.toLocaleString(),
              read: session.usage.cache_read.toLocaleString(),
            })}
          >
            {t('usage.line', {
              ctx: tokens(session.usage.context),
              out: tokens(session.usage.output),
            })}
          </span>
        )}
        {attempt.mode !== 'normal' && (
          <span className={`mode-badge ${attempt.mode}`}>
            <Icon name={attempt.mode === 'yolo' ? 'bolt' : 'pencil'} />{' '}
            {t(attempt.mode === 'yolo' ? 'mode.yolo' : 'mode.accept_edits')}
          </span>
        )}
        {attempt.outcome && (
          <span className="inspector-frozen" title={t('inspector.frozenHint')}>
            {t('inspector.frozen')}
          </span>
        )}
      </div>

      {/* The worktree's own terminals: a shell of yours, always — reviewing
          keeps demanding ad-hoc commands in *its* worktree, not yours — and
          the repo's ▶ scripts when it declares any. */}
      {/* 收成一個具名的 worktree 群:五顆工具 chip 作為一組出場,靜止時
          抽屜的帶數回到五以內,不再與 finish footer 逐顆搶話。 */}
      {attempt.outcome === null && !parked && (
        <div
          className="inspector-run"
          data-testid="run-scripts"
          role="group"
          aria-label={t('inspector.worktreeGroup')}
        >
          <span className="run-label" aria-hidden="true">
            {t('inspector.worktreeGroup')}
          </span>
          <button
            className="chip mono"
            data-testid="open-shell"
           
            onClick={onOpenShell}
          >
            $ {t('inspector.shell')}
          </button>
          {runScripts.map((name) => (
            <button
              key={name}
              className="chip mono"
              data-testid={`run-${name}`}
              title={t('inspector.runHint', { name })}
              onClick={() => onRunScript(name)}
            >
              <Icon name="play" /> {name}
            </button>
          ))}
          {/* The manual snapshot — every agent's checkpoint, where Stop
              only covers the CLIs that report one. The button answers on
              itself. */}
          <button
            className="chip mono"
            data-testid="checkpoint-now"
            title={t('inspector.ckptHint')}
            disabled={ckptBusy}
            onClick={() => {
              setCkptBusy(true);
              void api
                .checkpointNow(attempt.id)
                .then((cp) =>
                  setCkptSay(
                    cp ? t('inspector.ckptMade', { n: cp.n }) : t('inspector.ckptNone'),
                  ),
                )
                .catch((e) => setCkptSay(String(e)))
                .finally(() => setCkptBusy(false));
            }}
          >
            {ckptSay ?? (
              <>
                <Icon name="flag" /> {t('inspector.ckpt')}
              </>
            )}
          </button>
          {/* The dev server, on the desk — or, for an SSH attempt, the
              honest refusal: the port lives on the remote, and a disabled
              button wearing the reason beats a button that never appears. */}
          {previewState !== null && (
            <button
              className="chip mono"
              data-testid="open-preview"
              disabled={previewState === 'ssh'}
              title={previewState === 'ssh' ? t('preview.sshHint') : undefined}
              onClick={onOpenPreview}
            >
              <Icon name="frame" /> {t('preview.open')}
            </button>
          )}
          {/* Park, offered exactly when restore is: a settled worktree.
              Single click — the reversible act needs no arming. */}
          {!midTurn && (
            <button
              className="chip mono"
              data-testid="park-attempt"
              title={t('board.parkHint')}
              onClick={onPark}
            >
              <Icon name="pause" /> {t('board.park')}
            </button>
          )}
        </div>
      )}

      {/* A message is holding for the end of this turn. Visible where it
          was queued, with the one act that still applies: changing your
          mind before Stop spends it. */}
      {session?.has_followup && (
        <p className="queued-banner" data-testid="queued-followup">
          {/* Whose message, when it is not yours. A note you left yourself
              and a queue two other agents are waiting on are different
              situations, and the one sentence used to describe both. */}
          <span>
            {session.pending_from && session.pending_from.length > 0
              ? t('inspector.queuedFrom', { who: session.pending_from.join(t('common.listSep')) })
              : t('inspector.queued')}
          </span>
          <button
            className="chip"
            data-testid="cancel-followup"
            onClick={() =>
              void api.cancelFollowup(session.id).catch(() => {
                /* the next broadcast says what actually stuck */
              })
            }
          >
            {t('common.cancel')}
          </button>
        </p>
      )}

      {error && (
        <p className="dialog-error" role="alert" data-testid="inspector-error">
          {error}
        </p>
      )}

      {pane === 'knows' && <Knows cwd={attempt.worktree_path} />}

      {pane === 'diff' ? (
        <>
          {/* Swap the baseline: the whole attempt, or what has happened
              since a checkpoint. Only offered once there is a checkpoint to
              compare against — a select with one honest option is furniture. */}
          {attempt.outcome === null && cps.length > 0 && (
            <div className="compare-row mono small">
              <label className="muted" htmlFor="ckpt-compare">
                {t('ckpt.compare')}
              </label>
              <select
                id="ckpt-compare"
                data-testid="ckpt-compare"
                value={compareTo}
                // Swapping the baseline remounts the pane, and the open
                // editor with it — locked rather than quietly destructive.
                disabled={editing !== null}
                title={editing !== null ? t('edit.compareLocked') : undefined}
                onChange={(e) => setCompareTo(Number(e.target.value) || 0)}
              >
                <option value={0}>{t('ckpt.compareBase')}</option>
                {cps.map((c) => (
                  <option key={c.n} value={c.n}>
                    {t('ckpt.compareN', { n: c.n, time: clock(c.at * 1000) })}
                  </option>
                ))}
              </select>
            </div>
          )}
          {/* Keyed by attempt and baseline: the fold state describes one
              diff's files, and a different comparison is a different diff. */}
          <DiffPane
            key={`${attempt.id}@${compareTo}`}
            attemptId={attempt.id}
            diff={diff}
            fetchedAt={fetchedAt}
            comments={comments}
            viewed={viewed}
            onViewed={onViewed}
            onPick={setPicked}
            canEdit={canEdit}
            editing={editing}
            onEditOpen={setEditing}
            onEditClose={closeEditor}
            onEditDirty={setEditorDirty}
            onEditSaved={onEditSaved}
            // The pre-composed note, sent by a human hand — the same
            // contract restore's banner keeps, and only where sending is
            // measured. Mid-turn it queues for Stop, exactly as a review
            // batch does: sent now it would land inside the turn it is
            // warning about. A failure surfaces — the terminal it would
            // show up in may not even be on screen.
            editTell={
              session?.live === true && isMeasured(session.agent)
                ? (file) => {
                    const text = t('edit.note', { file });
                    const deliver =
                      session.status === 'running'
                        ? api.queueFollowup(session.id, text)
                        : api.sendFollowup(session.id, text);
                    void deliver.catch((e) => setError(String(e)));
                  }
                : null
            }
          />
          {confirmClose && editing !== null && (
            <Modal onCancel={() => setConfirmClose(false)}>
              <h2>{t('edit.discardTitle')}</h2>
              <p className="small muted">{t('edit.discardBody', { file: editing })}</p>
              {/* Keep first and primary: the Modal focuses its first control,
                  and Enter — the reflex key after a surprising dialog — must
                  land on the choice that loses nothing. Friction stays
                  proportional to consequence even in a two-button modal. */}
              <div className="row">
                <button
                  className="primary"
                  data-testid="editor-keep"
                  onClick={() => setConfirmClose(false)}
                >
                  {t('edit.keep')}
                </button>
                <button
                  className="danger"
                  data-testid="editor-discard"
                  onClick={() => {
                    setConfirmClose(false);
                    setEditorDirty(false);
                    setEditing(null);
                  }}
                >
                  {t('edit.discard')}
                </button>
              </div>
            </Modal>
          )}
        </>
      ) : pane === 'timeline' ? (
        <>
          {restored !== null && (
            <p className="restore-banner small" data-testid="restore-say" aria-live="polite">
              <span>{restored}</span>
              {/* The pre-composed note, sent only by a human hand: the
                  worktree moved under the agent's feet, and a measured CLI
                  can be told through the same paste a follow-up rides. */}
              {session?.live === true && isMeasured(session.agent) && (
                <button
                  className="chip"
                  data-testid="restore-tell"
                  onClick={() => {
                    // A failed tell lands back on the banner it left from —
                    // its terminal may not even be on screen to show it.
                    void api
                      .sendFollowup(session.id, t('ckpt.note'))
                      .then(() => setRestored(null))
                      .catch((e) => setRestored(String(e)));
                  }}
                >
                  {t('ckpt.tell')}
                </button>
              )}
              <button
                className="chip"
                aria-label={t('common.close')}
                onClick={() => setRestored(null)}
              >
                ✕
              </button>
            </p>
          )}
          <Timeline
            events={events}
            error={eventsError}
            checkpoints={cps}
            onRestore={attempt.outcome === null ? doRestore : null}
            blocked={restoreBlocked}
          />
        </>
      ) : null}

      {pane === 'diff' && (picked !== null || comments.length > 0) && (
        <Review
          attempt={attempt}
          session={session}
          picked={picked}
          comments={comments}
          diffText={diff}
          onPick={setPicked}
          onChange={onComments}
          onSent={refresh}
          onProblem={setError}
        />
      )}

      {attempt.outcome === null && (
        <Finish
          attempt={attempt}
          baseBranch={baseBranch}
          bases={bases}
          next={stat ? nextAction(stat) : null}
          behind={stat?.behind ?? 0}
          onDone={onDone}
          onMerged={onMerged}
        />
      )}
    </aside>
  );
}

/**
 * The feedback being gathered, and the one way it leaves.
 *
 * The batch goes back as a single message — each send is a turn, and five
 * turns would have the agent acting on the first point before it has read
 * the fifth. Sending goes through the session's own terminal, so it lands
 * exactly as if it had been pasted there; for a CLI whose input conventions
 * are not measured the composed text is offered to copy instead, the same
 * honesty the first prompt has.
 */
function Review({
  attempt,
  session,
  picked,
  comments,
  diffText,
  onPick,
  onChange,
  onSent,
  onProblem,
}: {
  attempt: Attempt;
  session: SessionMeta | null;
  picked: Picked | null;
  comments: ReviewComment[];
  /** The diff as currently shown — a pending comment whose quoted line is
      no longer in it gets marked stale rather than aging silently. */
  diffText: string | null;
  onPick: (p: Picked | null) => void;
  onChange: (c: ReviewComment[]) => void;
  onSent: () => void;
  onProblem: (e: string | null) => void;
}) {
  const t = useT();
  const [note, setNote] = useState('');
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);

  const canSend =
    attempt.outcome === null &&
    attempt.session_id !== null &&
    followupSendable(attempt.agent);

  /** Mid-turn, the batch queues instead of steering: sent now it would
   *  land inside the turn under review; held for Stop it arrives as the
   *  next one, about a diff that has stopped moving. */
  const midTurn = session?.status === 'running';

  const add = () => {
    if (!picked || note.trim() === '') return;
    onChange([...comments, { ...picked, note: note.trim() }]);
    setNote('');
    onPick(null);
    // The compose box borrowed the caret; hand it back to the diff so the
    // next j resumes the walk instead of stranding focus on <body>.
    document.querySelector<HTMLElement>('[data-testid="diff-body"]')?.focus();
  };

  const send = () => {
    if (!attempt.session_id) return;
    setBusy(true);
    onProblem(null);
    const text = composeReview(comments, t);
    const deliver = midTurn
      ? api.queueFollowup(attempt.session_id, text)
      : api.sendFollowup(attempt.session_id, text);
    void deliver
      .then(() => {
        onChange([]);
        onPick(null);
        // The timeline now carries what was just asked — or, queued, the
        // banner above says what is waiting to be.
        onSent();
        // Send 借走的 caret 也要還 —— 與 add() 同一條規矩:下一個 j 從
        // 走查停下的地方繼續,而不是把焦點丟在 <body> 上。
        document.querySelector<HTMLElement>('[data-testid="diff-body"]')?.focus();
      })
      .catch((e) => onProblem(String(e)))
      .finally(() => setBusy(false));
  };

  const copy = () => {
    void navigator.clipboard?.writeText(composeReview(comments, t));
    setCopied(true);
  };

  return (
    <div className="review" data-testid="review">
      {picked && (
        <div className="review-compose">
          <div className="mono small muted review-target">
            {picked.file === null
              ? ''
              : picked.line === null
                ? picked.file
                : `${picked.file}:${picked.line}`}
            <span className="review-excerpt"> {picked.excerpt}</span>
          </div>
          <textarea
            rows={2}
            autoFocus
            value={note}
            placeholder={t('review.placeholder')}
            data-testid="review-note"
            onChange={(e) => {
              setNote(e.target.value);
              setCopied(false);
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) add();
            }}
          />
          <div className="row">
            <button
              className="primary"
              disabled={note.trim() === ''}
              data-testid="review-add"
              onClick={add}
            >
              {t('review.add')}
            </button>
            <button
              onClick={() => {
                onPick(null);
                setNote('');
              }}
            >
              {t('common.cancel')}
            </button>
          </div>
        </div>
      )}

      {comments.length > 0 && (
        <>
          <ul className="review-pending" data-testid="review-pending">
            {comments.map((c, i) => (
              <li key={i}>
                <span className="mono small muted">
                  {c.file === null ? '' : c.line === null ? c.file : `${c.file}:${c.line}`}
                </span>
                <span className="review-note-text small">{c.note}</span>
                {/* The quoted line has left the diff — a hand edit, a
                    restore, a refresh. The note still sends, quoting what
                    was seen; the reader deserves to know it is history. */}
                {diffText !== null && c.excerpt !== '' && !diffText.includes(c.excerpt) && (
                  <span className="review-stale" title={t('review.staleHint')}>
                    {t('review.stale')}
                  </span>
                )}
                <button
                  className="icon"
                  aria-label={t('review.remove')}
                  title={t('review.remove')}
                  onClick={() => onChange(comments.filter((_, j) => j !== i))}
                >
                  ✕
                </button>
              </li>
            ))}
          </ul>
          <div className="row review-actions">
            {canSend ? (
              <button
                className="primary"
                disabled={busy}
                data-testid="review-send"
                onClick={send}
              >
                {t(midTurn ? 'review.queue' : 'review.send', { count: comments.length })}
              </button>
            ) : (
              <button data-testid="review-copy" onClick={copy}>
                {copied ? t('attempt.copied') : t('review.copy')}
              </button>
            )}
          </div>
        </>
      )}
    </div>
  );
}

/**
 * The two ways an attempt ends, and the one way it is thrown away.
 *
 * This is where it stops. Reviewing a pull request, chasing its checks and
 * merging it are somebody else's tool and a much larger one — trying to be
 * that as well would dilute the part of this that is actually deep.
 *
 * Merging closes the attempt out and takes the worktree back. Opening a pull
 * request deliberately does not: review is exactly when there is still
 * something to change, and the worktree is where changing it happens.
 */
function Finish({
  attempt,
  baseBranch,
  bases,
  next,
  behind,
  onDone,
  onMerged,
}: {
  attempt: Attempt;
  baseBranch: string;
  bases: string[];
  /** The merge path's own checks, run ahead of the click — what would
      refuse (uncommitted work), what would risk (a base that moved), or
      that the way is clear. */
  next: NextAction;
  behind: number;
  onDone: () => void;
  onMerged?: (branch: string) => void;
}) {
  const t = useT();
  const [busy, setBusy] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  /** One per repository the card spans — a pull request belongs to a
      repository, so a card spanning two produces two. The core returns them
      newline-separated, in the order the card names them. */
  const [prUrls, setPrUrls] = useState<string[]>([]);
  const [copied, setCopied] = useState(false);

  // What the merge writes to, said in full. Deduplicated because two repos
  // very often share a base name, and "main、main" names nothing extra.
  const target = [...new Set(bases.length > 0 ? bases : [baseBranch])].join('、');

  const run = (what: string, fn: () => Promise<unknown>) => () => {
    setBusy(what);
    setProblem(null);
    void fn()
      .then((r) => {
        if (what === 'pr' && typeof r === 'string') {
          setPrUrls(r.split('\n').filter((u) => u.trim() !== ''));
        }
        if (what === 'merge') onMerged?.(target);
        if (what === 'merge' || what === 'discard') onDone();
      })
      // Every refusal here is one that would otherwise lose work quietly —
      // uncommitted changes, the wrong branch checked out — so it is shown
      // in full rather than summarised.
      .catch((e) => setProblem(String(e)))
      .finally(() => setBusy(null));
  };

  /* Friction proportional to consequence: merge mutates the base branch —
     the one thing every hint promises an attempt cannot touch — so it arms
     exactly like discard does. Both name what the second click will do. */
  // Seven seconds, not the default four: the armed label names a branch,
  // and disarming while the reader checks that name punishes hesitation —
  // the one response an armed merge should invite.
  const merge = useArmed(run('merge', () => api.mergeAttempt(attempt.id)), 7000);
  const discard = useArmed(run('discard', () => api.finishAttempt(attempt.id, 'discarded')), 7000);

  return (
    <footer className="inspector-foot">
      {/* The refusal before the click: the same checks merge runs, worn as
          a banner while there is still time to act on them. */}
      {next !== null && (
        <p className={`next-banner ${next}`} data-testid="next-banner">
          {t(NEXT_KEY[next], { branch: baseBranch, n: behind })}
        </p>
      )}
      {problem && <FriendlyError text={problem} testid="finish-error" />}
      {/* The PR is the whole product of this path; its URL cannot be dead
          text in a 460px drawer. A real link that opens the browser, and a
          copy for wherever the review conversation actually happens.
          One row per repository the card spans — a pull request belongs to a
          repository, so these are several links and never one joined string.
          The copy button takes all of them, because the message somebody is
          about to write mentions both. */}
      {prUrls.map((url, i) => (
        <p
          className="mono small pr-url"
          key={url}
          data-testid={i === 0 ? 'pr-url' : `pr-url-${i + 1}`}
        >
          <a
            href={url}
            onClick={(e) => {
              e.preventDefault();
              void api.openExternal(url);
            }}
          >
            {url}
          </a>
          {i === 0 && (
            <button
              className="chip"
              data-testid="pr-copy"
              onClick={() => {
                void navigator.clipboard?.writeText(prUrls.join('\n'));
                setCopied(true);
              }}
            >
              {copied ? t('attempt.copied') : t('inspector.copyUrl')}
            </button>
          )}
        </p>
      ))}
      <div className="row">
        <button
          className={merge.armed ? 'confirm-arm' : 'primary'}
          disabled={busy !== null}
          data-testid={merge.armed ? 'confirm-merge' : 'merge-attempt'}
          onClick={merge.fire}
        >
          {busy === 'merge'
            ? t('inspector.working')
            : merge.armed
              ? t('inspector.confirmMerge', { branch: target })
              : t('inspector.mergeInto', { branch: target })}
        </button>
        <button
          disabled={busy !== null}
          data-testid="open-pr"
          onClick={run('pr', () => api.openPr(attempt.id))}
        >
          {busy === 'pr' ? t('inspector.working') : t('inspector.openPr')}
        </button>
        <span className="spacer" />
        <button
          className={discard.armed ? 'confirm-delete' : 'danger'}
          disabled={busy !== null}
          data-testid={discard.armed ? 'confirm-discard' : 'discard-attempt'}
          title={t('inspector.discardHint')}
          onClick={discard.fire}
        >
          {discard.armed ? t('inspector.confirmDiscard') : t('inspector.discard')}
        </button>
      </div>
    </footer>
  );
}

/** One file's slice of the diff: its lines, its counts, and the raw header
 *  lines the display no longer spends four rows of a 460px drawer on. */
interface FileSection {
  file: string | null;
  meta: string[];
  lines: { l: DiffLine; i: number }[];
  adds: number;
  dels: number;
}

/** The header lines every diff viewer folds away. Only what is recognized
 *  is folded — unrecognized text outside a hunk stays on screen, because a
 *  quietly hidden line is worse than an ugly one. */
const PLUMBING =
  /^(diff |index |--- |\+\+\+ |old mode|new mode|deleted file|new file|similarity |rename |copy |\\)/;

function groupByFile(lines: DiffLine[]): FileSection[] {
  const out: FileSection[] = [];
  let cur: FileSection | null = null;
  lines.forEach((l, i) => {
    if (l.kind === 'meta' && (PLUMBING.test(l.text) || l.text.trim() === '')) {
      if (l.text.startsWith('diff ') || cur === null) {
        cur = { file: null, meta: [], lines: [], adds: 0, dels: 0 };
        out.push(cur);
      }
      if (l.text.trim() !== '') cur.meta.push(l.text);
      return;
    }
    if (cur === null) {
      cur = { file: null, meta: [], lines: [], adds: 0, dels: 0 };
      out.push(cur);
    }
    cur.file ??= l.file;
    if (l.kind === 'add') cur.adds += 1;
    if (l.kind === 'del') cur.dels += 1;
    cur.lines.push({ l, i });
  });
  return out.filter((s) => s.lines.length > 0);
}

/** Wrapping long lines is a reading preference, not a per-diff choice. */
const WRAP_KEY = 'marol.diffWrap';

function DiffPane({
  attemptId,
  diff,
  fetchedAt,
  comments,
  viewed,
  onViewed,
  onPick,
  canEdit,
  editing,
  onEditOpen,
  onEditClose,
  onEditDirty,
  onEditSaved,
  editTell,
}: {
  attemptId: string;
  diff: string | null;
  fetchedAt: number | null;
  comments: readonly ReviewComment[];
  viewed: string[];
  onViewed: (files: string[]) => void;
  onPick: (p: Picked) => void;
  /** Whether edit chips are offered at all — the park button's family:
      open, settled, not parked. Binary files never qualify: their diff
      carries no `+++` header, so their section has no file name. */
  canEdit: boolean;
  /** The file whose section is an editor right now, or null. */
  editing: string | null;
  onEditOpen: (file: string) => void;
  /** A request, not an act — the parent holds the dirty guard. */
  onEditClose: () => void;
  onEditDirty: (dirty: boolean) => void;
  onEditSaved: (file: string) => void;
  /** Send the tell-agent note for a file, or null when this session
      cannot be told. */
  editTell: ((file: string) => void) | null;
}) {
  const t = useT();
  const lines = useMemo(() => (diff === null ? [] : parseDiff(diff)), [diff]);
  const sections = useMemo(() => groupByFile(lines), [lines]);

  /** Explicit opens and closes, per file, overriding the starting policy.
   *  The policy folds what nobody reads linearly — deletions, walls past
   *  800 lines, files already marked viewed — and a click reverses any of
   *  it; the click is remembered, the policy is not fought. */
  const [open, setOpen] = useState<Record<string, boolean>>({});
  /** Where the j/k (or n/p) walk last stood, so focus that wandered into
      the compose box and back resumes there instead of at the top. */
  const lastStop = useRef<HTMLElement | null>(null);
  const [wrap, setWrap] = useState(() => localStorage.getItem(WRAP_KEY) === '1');

  const fileKey = (s: FileSection, si: number) => s.file ?? `#${si}`;
  const isOpen = (s: FileSection, si: number) => {
    const explicit = open[fileKey(s, si)];
    if (explicit !== undefined) return explicit;
    return !(
      autoCollapse(s.lines.length, s.meta) ||
      (s.file !== null && viewed.includes(s.file))
    );
  };

  /** Marking viewed folds the file; unmarking reopens it. Either way the
   *  explicit override is cleared, so the policy speaks again. */
  const toggleViewed = (file: string) => {
    onViewed(
      viewed.includes(file) ? viewed.filter((f) => f !== file) : [...viewed, file],
    );
    setOpen((o) => {
      if (!(file in o)) return o;
      const next = { ...o };
      delete next[file];
      return next;
    });
  };

  if (diff === null) return <p className="muted small pad">{t('common.loading')}</p>;
  if (diff.trim() === '') {
    return (
      <p className="muted small pad" data-testid="diff-empty">
        {t('inspector.noChanges')}
      </p>
    );
  }

  const adds = sections.reduce((n, s) => n + s.adds, 0);
  const dels = sections.reduce((n, s) => n + s.dels, 0);

  const noted = (l: DiffLine) =>
    comments.some((c) => c.file === l.file && c.line === l.line && c.excerpt === l.text);

  /**
   * j/k walk the commentable lines, n/p the file headers; Enter acts on
   * whichever is focused — a comment on a line, a fold on a header.
   * Plain letters are safe here: the diff is not a text field, and the
   * review textarea lives outside this element, so typing never collides.
   */
  const onDiffKeys = (e: React.KeyboardEvent<HTMLPreElement>) => {
    // Keystrokes inside the in-place editor are text, not navigation —
    // they bubble up here, and stealing j/k from someone typing would
    // break the editor at its first vowelless word.
    if ((e.target as HTMLElement).closest('.file-editor') !== null) return;
    const walk = (selector: string, forward: boolean) => {
      const stops = [...e.currentTarget.querySelectorAll<HTMLElement>(selector)];
      if (stops.length === 0) return;
      e.preventDefault();
      const at = stops.indexOf(document.activeElement as HTMLElement);
      // Focus fell off the walk (adding a comment moves it into the
      // compose box and back out again): resume where the walk left off
      // instead of marching a 12-comment review back to the top each time.
      const remembered = lastStop.current;
      const next =
        at < 0
          ? remembered !== null && stops.includes(remembered)
            ? remembered
            : stops[0]
          : stops[Math.min(stops.length - 1, Math.max(0, at + (forward ? 1 : -1)))];
      lastStop.current = next;
      next.focus();
      next.scrollIntoView({ block: 'nearest' });
    };
    if (e.key === 'j' || e.key === 'k') {
      walk('.diff-line.commentable', e.key === 'j');
    } else if (e.key === 'n' || e.key === 'p') {
      walk('.diff-file-name', e.key === 'n');
    } else if (e.key === 'e' || e.key === 'v') {
      // On a file header (n/p's stops), e edits and v toggles viewed —
      // the header's own chips, reachable without leaving the walk. The
      // chips themselves stay tabIndex=-1; these keys are their keyboard.
      const head = (document.activeElement as HTMLElement | null)?.closest('.diff-file');
      if (head === null || head === undefined) return;
      const si = Number(head.id.replace('diff-file-', ''));
      const s = sections[si];
      if (!s || s.file === null) return;
      e.preventDefault();
      if (e.key === 'v') {
        toggleViewed(s.file);
      } else if (canEdit) {
        if (editing === s.file) onEditClose();
        else if (editing === null) onEditOpen(s.file);
      }
    }
  };

  const seen = sections.filter((s) => s.file !== null && viewed.includes(s.file)).length;

  return (
    <>
      {/* The whole diff in one line, and when it was read. The reload sits
          in the header; this is the honesty that makes it worth pressing —
          a diff with no timestamp reads as current long after it is not. */}
      <div className="diff-summary mono small muted" data-testid="diff-summary">
        {/* The file count is also the jump: picking a file scrolls to it,
            reopening it if the fold policy had it away. */}
        <select
          className="diff-jump"
          value=""
          aria-label={t('inspector.jumpLabel')}
          data-testid="diff-jump"
          onChange={(e) => {
            const si = Number(e.target.value);
            if (!Number.isFinite(si) || !sections[si]) return;
            setOpen((o) => ({ ...o, [fileKey(sections[si], si)]: true }));
            // After the section has rendered open.
            requestAnimationFrame(() => {
              document
                .getElementById(`diff-file-${si}`)
                ?.scrollIntoView({ block: 'start' });
            });
          }}
        >
          <option value="" disabled>
            {t('inspector.diffSummary', { files: sections.length })}
          </option>
          {sections.map((s, si) => (
            <option key={si} value={si}>
              {s.file ?? '—'} +{s.adds} −{s.dels}
            </option>
          ))}
        </select>
        {adds > 0 && <span className="diff-count add">+{adds}</span>}
        {dels > 0 && <span className="diff-count del">−{dels}</span>}
        {seen > 0 && (
          <span data-testid="viewed-count">
            {t('inspector.viewedCount', { seen, files: sections.length })}
          </span>
        )}
        <span className="spacer" />
        <button
          className={`diff-wrap-toggle${wrap ? ' active' : ''}`}
          aria-pressed={wrap}
          data-testid="diff-wrap"
          title={t('inspector.wrap')}
          onClick={() => {
            setWrap(!wrap);
            localStorage.setItem(WRAP_KEY, wrap ? '0' : '1');
          }}
        >
          <Icon name="wrap" />
        </button>
        {fetchedAt !== null && (
          <span className="diff-fetched">{t('inspector.readAt', { time: clock(fetchedAt) })}</span>
        )}
      </div>
      <pre
        className={`diff mono${wrap ? ' wrap' : ''}`}
        data-testid="diff-body"
        tabIndex={0}
        title={t('inspector.diffKeys')}
        aria-keyshortcuts="j k n p e v Enter"
        onKeyDown={onDiffKeys}
      >
      {sections.map((s, si) => {
        const opened = isOpen(s, si);
        const isViewed = s.file !== null && viewed.includes(s.file);
        return (
        <span key={si} className="diff-section">
          {/* The plumbing (`index 1111111..`, `---/+++`) said nothing a
              reviewer acts on and cost four rows per file in a 460px
              drawer. The filename and its weight say it all; the raw
              header is a hover away. Roving focus like the lines: n/p
              land here, Enter folds. */}
          <span className="diff-file" id={`diff-file-${si}`}>
            <button
              className="diff-file-name"
              tabIndex={-1}
              aria-expanded={opened}
              title={s.meta.join('\n') || undefined}
              data-testid={`diff-fold-${si}`}
              // Folding the file that is being edited is a close in
              // disguise — routed through the same dirty guard.
              onClick={() =>
                editing !== null && editing === s.file
                  ? onEditClose()
                  : setOpen((o) => ({ ...o, [fileKey(s, si)]: !opened }))
              }
            >
              <span className="diff-caret" aria-hidden="true">
                {opened ? '▾' : '▸'}
              </span>
              {s.file ?? '—'}
            </button>
            {s.adds > 0 && <span className="diff-count add">+{s.adds}</span>}
            {s.dels > 0 && <span className="diff-count del">−{s.dels}</span>}
            <span className="spacer" />
            {canEdit && s.file !== null && (
              <button
                className={`diff-edit${editing === s.file ? ' on' : ''}`}
                tabIndex={-1}
                aria-pressed={editing === s.file}
                data-testid={`diff-edit-${si}`}
                disabled={editing !== null && editing !== s.file}
                title={
                  editing !== null && editing !== s.file
                    ? t('edit.oneAtATime')
                    : t('edit.hint')
                }
                aria-label={t('edit.hint')}
                onClick={() =>
                  editing === s.file ? onEditClose() : onEditOpen(s.file as string)
                }
              >
                <Icon name="pencil" />
              </button>
            )}
            {s.file !== null && (
              <button
                className={`diff-viewed${isViewed ? ' on' : ''}`}
                tabIndex={-1}
                aria-pressed={isViewed}
                data-testid={`diff-viewed-${si}`}
                aria-label={t(isViewed ? 'inspector.unmarkViewed' : 'inspector.markViewed')}
                title={t(isViewed ? 'inspector.unmarkViewed' : 'inspector.markViewed')}
                onClick={() => toggleViewed(s.file as string)}
              >
                ✓
              </button>
            )}
          </span>
          {editing !== null && editing === s.file ? (
            <FileEditor
              attemptId={attemptId}
              file={s.file as string}
              onTell={
                editTell === null ? null : () => editTell(s.file as string)
              }
              onSaved={() => onEditSaved(s.file as string)}
              onRequestClose={onEditClose}
              onDirtyChange={onEditDirty}
            />
          ) : opened && s.lines.map(({ l, i }) => (
            <span
              key={i}
              className={[
                'diff-line',
                classOf(l),
                commentable(l) ? 'commentable' : '',
                noted(l) ? 'noted' : '',
              ]
                .filter(Boolean)
                .join(' ')}
                            // The review loop is the flagship; it cannot be mouse-only.
              // Roving focus: the <pre> is the single tab stop and j/k move
              // within it — per-line tabstops made a 300-line diff a
              // 300-stop wall between the header and the merge button.
              role={commentable(l) ? 'button' : undefined}
              // A noted line's highlight is silence to a screen reader;
              // pressed is the nearest honest word for "already carries
              // feedback" on a line that acts as a button.
              aria-pressed={commentable(l) ? noted(l) : undefined}
              tabIndex={commentable(l) ? -1 : undefined}
              onKeyDown={
                commentable(l)
                  ? (e) => {
                      if (e.key === 'Enter') {
                        e.preventDefault();
                        onPick({ file: l.file, line: l.line, excerpt: l.text });
                      }
                    }
                  : undefined
              }
              onClick={
                commentable(l)
                  ? () => onPick({ file: l.file, line: l.line, excerpt: l.text })
                  : undefined
              }
            >
              {/* Texture, not a parser's claim: strings and comments only,
                  tinted from whatever color the line already wears. The
                  runs concatenate back to l.text exactly — excerpts and
                  matching elsewhere compare against the raw line. */}
              {l.kind === 'add' || l.kind === 'del' || l.kind === 'context'
                ? tint(l.text).map((run, ri) =>
                    run.cls === null ? (
                      run.text
                    ) : (
                      <span key={ri} className={`tk-${run.cls}`}>
                        {run.text}
                      </span>
                    ),
                  )
                : l.text}
              {'\n'}
            </span>
          ))}
        </span>
        );
      })}
      </pre>
    </>
  );
}

/**
 * Which colour a diff line takes. The parser has already told the file
 * headers apart from added and removed lines — colouring `+++` as an addition
 * is exactly the mistake it exists to prevent.
 */
function classOf(l: DiffLine): string {
  switch (l.kind) {
    case 'add':
      return 'add';
    case 'del':
      return 'del';
    case 'hunk':
      return 'hunk';
    case 'meta':
      return 'meta';
    default:
      return '';
  }
}

function Timeline({
  events,
  error,
  checkpoints,
  onRestore,
  blocked,
}: {
  events: AttemptEvent[];
  error: string | null;
  /** The attempt's snapshots, oldest first, for the ↩ anchors. */
  checkpoints: Checkpoint[];
  /** Null when the attempt is finished — nothing left to restore into. */
  onRestore: ((n: number) => void) | null;
  /** Why restoring is off the table right now (mid-turn, parked), or
      null when it is open. The buttons stay, disabled, wearing the reason. */
  blocked: string | null;
}) {
  const t = useT();
  const rows = useMemo(() => rollup(events), [events]);
  /** Which row's ↩ is armed — the two-click contract, one row at a time. */
  const [armed, setArmed] = useState<number | null>(null);
  useEffect(() => {
    if (armed === null) return;
    const timer = setTimeout(() => setArmed(null), 4000);
    return () => clearTimeout(timer);
  }, [armed]);
  /** "Before this turn" = the last snapshot taken before its prompt — or
      the attempt's base, the free zeroth checkpoint. */
  const targetOf = (promptAt: number): number => {
    let n = 0;
    for (const c of checkpoints) {
      if (c.at * 1000 <= promptAt) n = c.n;
    }
    return n;
  };
  // A failed read is not an empty history. "No activity yet" over a dead
  // fetch would clear an agent that was never audited.
  if (error !== null && events.length === 0) {
    return (
      <p className="dialog-error pad" role="alert" data-testid="timeline-error">
        {t('inspector.eventsFailed', { err: error })}
      </p>
    );
  }
  if (events.length === 0) {
    return (
      <p className="muted small pad" data-testid="timeline-empty">
        {t('inspector.noActivity')}
      </p>
    );
  }
  return (
    <>
      <ol className="timeline" data-testid="timeline">
      {rows.map((e, i) => (
        <li
          key={`${e.at}-${i}`}
          className={`tl-row tl-${e.kind}${
            e.tool === 'SendMessage' || e.kind === 'message' ? ' tl-send' : ''
          }`}
          data-kind={e.kind}
        >
          <span className="tl-time mono small muted">{clock(e.at)}</span>
          {e.kind === 'tool' ? (
            <>
              <span className="tl-tool mono">
                {/* A cross-session message is an act between cards, not a
                    tool grinding — it wears the arrow the README writes. */}
                {e.tool === 'SendMessage' && <span aria-hidden="true">→ </span>}
                {e.tool}
                {/* A run of the same tool is one act, not N lines between
                    the reader and the next real event. Every detail rides
                    the tooltip; the row shows the latest. */}
                {e.count > 1 && <span className="tl-count">×{e.count}</span>}
              </span>
              <span
                className="tl-detail mono small muted"
                title={e.details.length > 1 ? e.details.join('\n') : (e.detail ?? undefined)}
              >
                {e.detail}
              </span>
            </>
          ) : e.kind === 'status' ? (
            <span className="tl-status">
              {STATUS_KEY[e.detail as never] ? t(STATUS_KEY[e.detail as never]) : e.detail}
              {/* What the wait cost — the number the record alone cannot
                  show, measured to whatever happened next. */}
              {e.heldMs !== null && e.heldMs >= 1000 && (
                <span className="tl-held muted">
                  {' '}
                  {t('timeline.waited', { for: elapsed(e.at, e.at + e.heldMs) })}
                </span>
              )}
            </span>
          ) : e.kind === 'message' ? (
            /* Not a prompt. Somebody else's agent said this, and the row
               says so — the same honesty the envelope carries into the
               terminal, kept for whoever reads the record afterwards. No
               ↩ beside it: a restore is anchored to a turn the person
               started, and this is not one of those. */
            <>
              <span className="tl-tool mono">
                <span aria-hidden="true">← </span>
                {e.tool}
              </span>
              <span className="tl-detail mono small muted" title={e.detail ?? undefined}>
                {e.detail}
              </span>
            </>
          ) : (
            <>
              <span className="tl-prompt">{e.detail}</span>
              {/* The retreat, anchored where the turn began. Disabled — not
                  hidden — while the agent is mid-turn, so the reason is a
                  hover away instead of the button being a mystery. */}
              {onRestore !== null && (
                <button
                  className={`tl-restore${armed === i ? ' armed' : ''}`}
                  data-testid={`restore-${i}`}
                  disabled={blocked !== null}
                  title={blocked ?? t('ckpt.restoreHint')}
                  onClick={() => {
                    if (armed === i) {
                      setArmed(null);
                      onRestore(targetOf(e.at));
                    } else {
                      setArmed(i);
                    }
                  }}
                >
                  {armed === i ? t('ckpt.restoreArm') : '↩'}
                </button>
              )}
            </>
          )}
        </li>
      ))}
      </ol>
    </>
  );
}

function clock(ms: number): string {
  const d = new Date(ms);
  const two = (n: number) => String(n).padStart(2, '0');
  return `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`;
}

/**
 * What the agent working here already knows, before anyone types.
 *
 * Slots, not discoveries: a rules file that is missing is still listed, with
 * its path, marked absent. The question people actually have is "where do the
 * conventions go", and a list of only what exists answers that with silence —
 * the same reason a session with no status signal wears a disclaimer rather
 * than a blank.
 *
 * Every supported CLI's convention appears, not only the one this attempt
 * runs — a checkout's `AGENTS.md` matters whichever agent is reading it.
 * Narrowing the list to the running agent would answer a smaller question
 * than the one people open this tab with.
 */
function Knows({ cwd }: { cwd: string }) {
  const t = useT();
  const [docs, setDocs] = useState<AgentDoc[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setDocs(null);
    setError(null);
    void api
      .agentDocs(cwd)
      .then((d) => live && setDocs(d))
      // Blank and broken must not look alike: a failed read says so.
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
  }, [cwd]);

  if (error !== null) return <FriendlyError text={error} testid="knows-error" />;
  if (docs === null) return <p className="muted small">{t('common.loading')}</p>;

  const groups: { scope: string; label: MessageKey }[] = [
    { scope: 'project', label: 'knows.project' },
    { scope: 'global', label: 'knows.global' },
  ];

  return (
    <div className="knows" data-testid="knows">
      {groups.map(({ scope, label }) => {
        const rows = docs.filter((d) => d.scope === scope);
        if (rows.length === 0) return null;
        return (
          <div key={scope}>
            <h4 className="knows-head">{t(label)}</h4>
            {/* A card spanning two repos has two `CLAUDE.md`, and they are
                two different files. The checkout each belongs to rides in
                front of the name, so the rows read as what they are rather
                than as a duplicate — and so the test ids stay distinct. */}
            {rows.map((d) => (
              <div className={`knows-row${d.exists ? '' : ' absent'}`} key={d.path}>
                <button
                  className="knows-name mono"
                  data-testid={`knows-${d.dir === '' ? d.name : `${d.dir}/${d.name}`}`}
                  // Only what is there can be opened; the rest is a path to
                  // write to, and offering to open nothing would be a lie
                  // dressed as a button.
                  disabled={!d.exists}
                  title={d.path}
                  onClick={() => void api.openPath(d.path)}
                >
                  {d.dir === '' ? d.name : `${d.dir}/${d.name}`}
                </button>
                <span className="knows-agent">{d.agent === 'shared' ? t('knows.shared') : d.agent}</span>
                {!d.exists && <span className="knows-absent">{t('knows.absent')}</span>}
              </div>
            ))}
          </div>
        );
      })}
    </div>
  );
}
