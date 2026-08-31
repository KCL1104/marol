import type * as React from 'react';
import type { SessionMeta } from './types';

/**
 * How a tab arranges its panes.
 *
 * There are two modes and they are rendered by completely different means,
 * which is deliberate.
 *
 * `auto` is a uniform CSS grid whose column count falls out of the container
 * width. It costs nothing to maintain and it is right on every monitor, which
 * a stored "3x2" never is: three columns is a comfortable wall on a 27" screen
 * and an unreadable one on a laptop.
 *
 * `manual` is a split tree, and it is positioned from rectangles this module
 * computes rather than by nesting DOM. Nesting would be the obvious way to
 * draw a tree, but moving a pane between branches would re-parent its
 * terminal — and re-parenting disposes the xterm instance along with its
 * scrollback and its WebGL context. Absolute rectangles over a flat DOM mean a
 * pane never changes parent no matter how the tree is rearranged, and they
 * make splitter positions fall out of the same computation.
 */
export type Layout =
  | { mode: 'auto'; cols: number | 'auto' }
  | { mode: 'manual'; root: Node | null };

/** A leaf is a session; a split divides its box among its children. */
export type Node = { id: string } | Split;

export interface Split {
  /** `row` lays children out side by side; `col` stacks them. */
  dir: 'row' | 'col';
  kids: Node[];
  /** One share per child, summing to 1. */
  fr: number[];
}

export const isSplit = (n: Node): n is Split => 'kids' in n;

/**
 * The width below which Claude Code's TUI stops being readable.
 *
 * The TUI reflows to the terminal width and its box drawing comes apart below
 * roughly 60 columns; at 13px in this font that is about 490px including the
 * pane's own chrome. Auto mode never chooses a column count that would go
 * under this — which is the whole reason it beats a stored grid size.
 */
export const MIN_PANE_W = 490;

/** Roughly 20 rows plus the pane header: less and there is nothing to read. */
export const MIN_PANE_H = 350;

/** How small a splitter drag may make a track. Smaller and the header goes. */
export const MIN_TRACK = 160;

/** Gap between panes, in px. Splitter handles are centred on it. */
export const GAP = 5;

/** Hit area of a splitter, which is wider than the gap it sits in. */
export const HANDLE = 9;

export const AUTO: Layout = { mode: 'auto', cols: 'auto' };

/* ------------------------------------------------------------------ */
/* Persistence                                                         */
/* ------------------------------------------------------------------ */

/**
 * Read a stored layout, falling back to auto rather than throwing.
 *
 * Layouts written before this model was a tree were fixed "CxR" grids. Only
 * `1x1` carries intent worth keeping — one pane at a time is a choice auto
 * cannot express. Everything else was the user fitting N things on screen,
 * which is exactly what auto now does, and better, because it accounts for the
 * window they are actually looking at.
 */
export function parseLayout(raw: string | undefined | null): Layout {
  if (!raw) return AUTO;
  if (raw === 'single' || raw === '1x1') return { mode: 'auto', cols: 1 };
  if (raw[0] !== '{') return AUTO;
  try {
    const p = JSON.parse(raw) as Layout;
    if (p?.mode === 'auto') {
      const c = p.cols;
      return { mode: 'auto', cols: c === 'auto' ? 'auto' : clampCols(c) };
    }
    if (p?.mode === 'manual') return { mode: 'manual', root: sanitize(p.root) };
  } catch {
    /* not ours */
  }
  return AUTO;
}

export function formatLayout(l: Layout): string {
  return JSON.stringify(l);
}

export function sameLayout(a: Layout, b: Layout): boolean {
  return formatLayout(a) === formatLayout(b);
}

/**
 * A stored column count, made sane.
 *
 * The ceiling is storage hygiene and nothing else: what actually gets drawn
 * is `autoCols`, which never returns more columns than there are panes, so a
 * choice larger than the wall is already indistinguishable from the wall's
 * own width. The number here only has to reject a corrupt row — a negative, a
 * NaN, a value no desk could have chosen — without ever being the thing
 * somebody bumps into. Six used to be that number and was the wrong kind of
 * limit: it was smaller than a wall of terminals can plausibly be, so it
 * capped a real choice rather than a bad row.
 */
const clampCols = (n: unknown) => Math.min(64, Math.max(1, Math.floor(Number(n)) || 1));

/** Reject anything that would make the renderer misbehave. */
function sanitize(n: unknown): Node | null {
  if (!n || typeof n !== 'object') return null;
  const node = n as Partial<Split> & { id?: unknown };
  if (typeof node.id === 'string') return { id: node.id };
  if (!Array.isArray(node.kids)) return null;
  const kids = node.kids.map(sanitize).filter((k): k is Node => k !== null);
  if (kids.length === 0) return null;
  if (kids.length === 1) return kids[0];
  return {
    dir: node.dir === 'col' ? 'col' : 'row',
    kids,
    fr: normalize(Array.isArray(node.fr) ? node.fr : [], kids.length),
  };
}

/* ------------------------------------------------------------------ */
/* Fractions                                                           */
/* ------------------------------------------------------------------ */

/** Force `fr` to `n` positive shares summing to 1. */
export function normalize(fr: readonly number[], n: number): number[] {
  if (n <= 0) return [];
  // A stored array of the wrong length has lost track of which number went
  // with which pane, so there is nothing to preserve — equal shares beat
  // guessing. Every caller inside this module keeps the two in step, so this
  // only fires on a layout that was edited or corrupted outside the app.
  if (fr.length !== n) return equalFr(n);
  const vals: number[] = [];
  for (let i = 0; i < n; i++) {
    const v = Number(fr[i]);
    vals.push(Number.isFinite(v) && v > 0 ? v : 1 / n);
  }
  const total = vals.reduce((a, b) => a + b, 0);
  return total > 0 ? vals.map((v) => v / total) : vals.map(() => 1 / n);
}

export const equalFr = (n: number): number[] => Array.from({ length: n }, () => 1 / n);

/* ------------------------------------------------------------------ */
/* Tree reads                                                          */
/* ------------------------------------------------------------------ */

/** Every session in the tree, in reading order. */
export function leaves(n: Node | null): string[] {
  if (!n) return [];
  if (!isSplit(n)) return [n.id];
  return n.kids.flatMap(leaves);
}

/** The split holding `id`, as a path of child indices from the root. */
function pathTo(n: Node | null, id: string, acc: number[] = []): number[] | null {
  if (!n) return null;
  if (!isSplit(n)) return n.id === id ? acc : null;
  for (let i = 0; i < n.kids.length; i++) {
    const hit = pathTo(n.kids[i], id, [...acc, i]);
    if (hit) return hit;
  }
  return null;
}

export function nodeAt(root: Node | null, path: readonly number[]): Node | null {
  let cur = root;
  for (const i of path) {
    if (!cur || !isSplit(cur)) return null;
    cur = cur.kids[i] ?? null;
  }
  return cur;
}

/** Rebuild `root` with `fn` applied to the node at `path`. */
function replaceAt(root: Node, path: readonly number[], fn: (n: Node) => Node | null): Node | null {
  if (path.length === 0) return fn(root);
  if (!isSplit(root)) return root;
  const [i, ...rest] = path;
  const child = root.kids[i];
  if (!child) return root;
  const next = replaceAt(child, rest, fn);
  const kids = root.kids.slice();
  const fr = root.fr.slice();
  if (next === null) {
    kids.splice(i, 1);
    fr.splice(i, 1);
  } else {
    kids[i] = next;
  }
  return collapse({ dir: root.dir, kids, fr: normalize(fr, kids.length) });
}

/** A split with one child is not a split; a split with none is nothing. */
function collapse(s: Split): Node | null {
  if (s.kids.length === 0) return null;
  if (s.kids.length === 1) return s.kids[0];
  return s;
}

/* ------------------------------------------------------------------ */
/* Tree writes                                                         */
/* ------------------------------------------------------------------ */

export function removeLeaf(root: Node | null, id: string): Node | null {
  if (!root) return null;
  if (!isSplit(root)) return root.id === id ? null : root;
  const path = pathTo(root, id);
  if (!path) return root;
  return replaceAt(root, path, () => null);
}

/** Build the tree an auto layout is currently drawing, so switching to manual
 *  keeps the arrangement the user is looking at instead of reshuffling it. */
export function materialise(ids: readonly string[], cols: number): Node | null {
  if (ids.length === 0) return null;
  const rows: Node[] = [];
  for (let i = 0; i < ids.length; i += cols) {
    const slice = ids.slice(i, i + cols).map((id) => ({ id }));
    rows.push(slice.length === 1 ? slice[0] : { dir: 'row', kids: slice, fr: equalFr(slice.length) });
  }
  if (rows.length === 1) return rows[0];
  return { dir: 'col', kids: rows, fr: equalFr(rows.length) };
}

/** Where a pane was dropped, relative to the pane it was dropped on. */
export type Zone = 'center' | 'left' | 'right' | 'top' | 'bottom';

const ZONE_DIR: Record<Exclude<Zone, 'center'>, 'row' | 'col'> = {
  left: 'row',
  right: 'row',
  top: 'col',
  bottom: 'col',
};
const ZONE_AFTER: Record<Exclude<Zone, 'center'>, boolean> = {
  left: false,
  right: true,
  top: false,
  bottom: true,
};

/**
 * Drop `movingId` onto `targetId`.
 *
 * The edge zones split the pane you dropped on, not the row it happens to sit
 * in — dropping below a pane in a row of four gives you that one pane cut in
 * half, and doing it to a second pane turns the row of four into a 2x2. That
 * is the same rule i3 and VS Code use, and it is what makes two drags enough
 * to reshape a layout.
 *
 * Fractions of the split that gained a child reset to equal. Restructuring
 * means a new shape, and carrying old proportions into it looks like a bug;
 * the splitters are there for fine-tuning afterwards.
 */
export function dropOn(
  root: Node | null,
  movingId: string,
  targetId: string,
  zone: Zone,
): Node | null {
  if (movingId === targetId) return root;
  if (!root) return { id: movingId };

  if (zone === 'center') return swapOrReplace(root, movingId, targetId);

  const pruned = removeLeaf(root, movingId);
  if (!pruned) return { id: movingId };

  const path = pathTo(pruned, targetId);
  // The target went away with the move (it was the moving pane's only
  // sibling, so the split collapsed). Nothing sensible to attach to.
  if (!path) return pruned;

  const dir = ZONE_DIR[zone];
  const after = ZONE_AFTER[zone];
  const parentPath = path.slice(0, -1);
  const parent = nodeAt(pruned, parentPath);

  // Same direction as the parent: become a sibling rather than nesting, so
  // repeated drops in one direction stay flat instead of growing a spine.
  if (parent && isSplit(parent) && parent.dir === dir) {
    const at = path[path.length - 1] + (after ? 1 : 0);
    return replaceAt(pruned, parentPath, (n) => {
      const s = n as Split;
      const kids = s.kids.slice();
      kids.splice(at, 0, { id: movingId });
      return { dir: s.dir, kids, fr: equalFr(kids.length) };
    });
  }

  return replaceAt(pruned, path, (target) => ({
    dir,
    kids: after ? [target, { id: movingId }] : [{ id: movingId }, target],
    fr: equalFr(2),
  }));
}

/**
 * Drop on the layout's own edge rather than on a pane: split the whole thing.
 *
 * Without this the tree can hold shapes no gesture can build. Dropping on a
 * pane's bottom edge halves *that pane*, so in a 2x2 there is no way to ask
 * for a strip running under all four — the split would have to happen above
 * the columns, and every pane-relative drop happens below them.
 */
export function dropOnRoot(root: Node | null, movingId: string, zone: Zone): Node | null {
  if (zone === 'center' || !root) return root ?? { id: movingId };
  const pruned = removeLeaf(root, movingId);
  if (!pruned) return { id: movingId };

  const dir = ZONE_DIR[zone];
  const after = ZONE_AFTER[zone];
  // Already split this way at the top: join the row rather than wrapping it,
  // so dropping twice on the same edge does not build a lopsided spine.
  if (isSplit(pruned) && pruned.dir === dir) {
    const kids = pruned.kids.slice();
    kids.splice(after ? kids.length : 0, 0, { id: movingId });
    return { dir, kids, fr: equalFr(kids.length) };
  }
  return {
    dir,
    kids: after ? [pruned, { id: movingId }] : [{ id: movingId }, pruned],
    fr: equalFr(2),
  };
}

/**
 * Centre drop. Two panes on screen trade places; a session arriving from the
 * sidebar takes the pane's place and evicts it back to the sidebar.
 *
 * Swapping rather than displacing matters: dragging one pane onto another is
 * how you rearrange, and losing the other session in the process would be a
 * surprise.
 */
function swapOrReplace(root: Node, movingId: string, targetId: string): Node {
  const movingPath = pathTo(root, movingId);
  const setLeaf = (n: Node, id: string): Node => (isSplit(n) ? n : { id });

  const targetPath = pathTo(root, targetId);
  if (!targetPath) return root;

  let next = replaceAt(root, targetPath, (n) => setLeaf(n, movingId)) ?? root;
  if (movingPath) next = replaceAt(next, movingPath, (n) => setLeaf(n, targetId)) ?? next;
  return next;
}

/**
 * Bring the tree back in line with the sessions the tab actually holds.
 *
 * The core owns the rule that a session lives in one tab at a time, so it can
 * take one away from under us; sessions also die. Leaves that are no longer
 * members go, and members with no leaf are appended, so the two can never
 * drift apart.
 */
export function reconcileTree(root: Node | null, members: readonly string[]): Node | null {
  const want = new Set(members);
  let next = root;
  for (const id of leaves(root)) {
    if (!want.has(id)) next = removeLeaf(next, id);
  }
  const have = new Set(leaves(next));
  for (const id of members) {
    if (have.has(id)) continue;
    next = next ? appendTo(next, id) : { id };
  }
  return next;
}

/** Append at the end of the outermost split, so new panes land predictably. */
function appendTo(root: Node, id: string): Node {
  if (!isSplit(root)) return { dir: 'row', kids: [root, { id }], fr: equalFr(2) };
  const kids = [...root.kids, { id }];
  return { dir: root.dir, kids, fr: equalFr(kids.length) };
}

/** Adjust the boundary between kids `index` and `index + 1` of one split. */
export function setFractions(root: Node | null, path: readonly number[], fr: number[]): Node | null {
  if (!root) return root;
  return replaceAt(root, path, (n) =>
    isSplit(n) ? { dir: n.dir, kids: n.kids, fr: normalize(fr, n.kids.length) } : n,
  );
}

export function resetFractions(root: Node | null, path: readonly number[]): Node | null {
  if (!root) return root;
  return replaceAt(root, path, (n) =>
    isSplit(n) ? { dir: n.dir, kids: n.kids, fr: equalFr(n.kids.length) } : n,
  );
}

/* ------------------------------------------------------------------ */
/* Geometry                                                            */
/* ------------------------------------------------------------------ */

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Handle {
  /** The split this handle divides. */
  path: number[];
  /** The boundary between kids `index` and `index + 1`. */
  index: number;
  dir: 'row' | 'col';
  rect: Rect;
  /** Size of the two tracks the drag moves between, in px. */
  span: number;
}

export interface Geometry {
  panes: Map<string, Rect>;
  handles: Handle[];
}

/** Lay the tree out inside `box`, in px. */
export function geometry(root: Node | null, box: Rect, gap = GAP): Geometry {
  const panes = new Map<string, Rect>();
  const handles: Handle[] = [];
  if (root) walk(root, box, [], panes, handles, gap);
  return { panes, handles };
}

function walk(
  n: Node,
  box: Rect,
  path: number[],
  panes: Map<string, Rect>,
  handles: Handle[],
  gap: number,
) {
  if (!isSplit(n)) {
    panes.set(n.id, box);
    return;
  }
  const horizontal = n.dir === 'row';
  const total = horizontal ? box.w : box.h;
  const avail = Math.max(0, total - gap * (n.kids.length - 1));
  const fr = normalize(n.fr, n.kids.length);

  let offset = horizontal ? box.x : box.y;
  for (let i = 0; i < n.kids.length; i++) {
    const size = avail * fr[i];
    const kidBox: Rect = horizontal
      ? { x: offset, y: box.y, w: size, h: box.h }
      : { x: box.x, y: offset, w: box.w, h: size };
    walk(n.kids[i], kidBox, [...path, i], panes, handles, gap);

    if (i < n.kids.length - 1) {
      const mid = offset + size + gap / 2;
      handles.push({
        path,
        index: i,
        dir: n.dir,
        span: avail,
        rect: horizontal
          ? { x: mid - HANDLE / 2, y: box.y, w: HANDLE, h: box.h }
          : { x: box.x, y: mid - HANDLE / 2, w: box.w, h: HANDLE },
      });
    }
    offset += size + gap;
  }
}

/**
 * Move one boundary by `deltaPx`, taking from one neighbour and giving to the
 * other so the rest of the split does not move. Both are clamped so a drag
 * cannot crush a pane out of existence.
 */
export function dragHandle(
  fr: readonly number[],
  index: number,
  deltaPx: number,
  span: number,
): number[] {
  const norm = normalize(fr, fr.length);
  if (span <= 0 || index < 0 || index + 1 >= norm.length) return norm;

  const a = norm[index] * span;
  const b = norm[index + 1] * span;
  const min = Math.min(MIN_TRACK, (a + b) / 2);
  const moved = Math.max(min - a, Math.min(b - min, deltaPx));

  const next = norm.slice();
  next[index] = (a + moved) / span;
  next[index + 1] = (b - moved) / span;
  return normalize(next, next.length);
}

/* ------------------------------------------------------------------ */
/* Auto mode                                                           */
/* ------------------------------------------------------------------ */

/**
 * How many columns auto mode should use.
 *
 * Width decides, not session count: the point of auto is that no pane is ever
 * narrower than the TUI needs. Capping at the session count stops two sessions
 * on a wide monitor from being squeezed into a quarter of it each with two
 * empty tracks alongside.
 */
export function autoCols(layout: Layout, width: number, count: number): number {
  const most = Math.max(1, count);
  if (layout.mode === 'auto' && layout.cols !== 'auto') return Math.min(layout.cols, most);
  const fits = Math.floor((width + GAP) / (MIN_PANE_W + GAP));
  return Math.max(1, Math.min(fits, most));
}

export function autoRows(count: number, cols: number): number {
  return Math.max(1, Math.ceil(Math.max(1, count) / Math.max(1, cols)));
}

/**
 * The grid auto mode draws.
 *
 * Rows carry a minimum so the grid grows past the viewport and scrolls instead
 * of squeezing panes below what a TUI can use. Columns do not: their count was
 * already chosen to satisfy the minimum, and a floor there would produce
 * horizontal scrolling, which is miserable in a wall of terminals.
 */
export function gridStyle(cols: number, rows: number): React.CSSProperties {
  return {
    gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`,
    gridTemplateRows: `repeat(${rows}, minmax(${MIN_PANE_H}px, 1fr))`,
    gap: `${GAP}px`,
  };
}

/* ------------------------------------------------------------------ */
/* Membership                                                          */
/* ------------------------------------------------------------------ */

/**
 * The sessions a tab shows, in order, with no holes.
 *
 * Holes used to be the model, and they cost more than they were worth: an
 * empty cell is indistinguishable from one the user deliberately emptied, so
 * every rule about filling them had to guess at intent, and two separate bugs
 * came out of guessing wrong.
 */
export function members(slots: ReadonlyArray<string | null> | undefined): string[] {
  return (slots ?? []).filter((s): s is string => typeof s === 'string' && s.length > 0);
}

export function addMember(ids: readonly string[], id: string): string[] {
  return ids.includes(id) ? [...ids] : [...ids, id];
}

export function removeMember(ids: readonly string[], id: string): string[] {
  return ids.filter((x) => x !== id);
}

/** Drop whatever is no longer running. Nothing is pulled in to take its
 *  place — refilling a pane the user just ejected makes eject look broken. */
export function reconcile(ids: readonly string[], sessions: readonly SessionMeta[]): string[] {
  const live = new Set(sessions.filter((s) => s.live).map((s) => s.id));
  return ids.filter((id) => live.has(id));
}

export const sameIds = (a: readonly string[], b: readonly string[]) =>
  a.length === b.length && a.every((v, i) => v === b[i]);

/* ------------------------------------------------------------------ */
/* Drag and drop                                                       */
/* ------------------------------------------------------------------ */

/** A session dragged from the sidebar, or a pane dragged from the grid. */
export type DragPayload = { kind: 'session' | 'pane'; id: string };

export const DRAG_MIME = 'application/x-marol';

export function encodeDrag(p: DragPayload): string {
  return JSON.stringify(p);
}

export function decodeDrag(raw: string): DragPayload | null {
  try {
    const p = JSON.parse(raw) as DragPayload;
    if ((p.kind === 'session' || p.kind === 'pane') && typeof p.id === 'string') return p;
  } catch {
    /* not ours */
  }
  return null;
}

/**
 * Which zone of a pane the pointer is in.
 *
 * The edges are a fixed fraction rather than a fixed number of pixels so the
 * split gestures stay reachable in a small pane, and the centre stays big
 * enough that a swap is never hard to hit.
 */
export const EDGE = 0.26;

export function zoneAt(x: number, y: number, w: number, h: number): Zone {
  if (w <= 0 || h <= 0) return 'center';
  const fx = x / w;
  const fy = y / h;
  // Compare distance-to-edge as a fraction so the nearer edge wins in a
  // corner instead of one axis always taking priority.
  const near = Math.min(fx, 1 - fx, fy, 1 - fy);
  if (near >= EDGE) return 'center';
  if (near === fx) return 'left';
  if (near === 1 - fx) return 'right';
  if (near === fy) return 'top';
  return 'bottom';
}

/**
 * Where to draw the drop preview inside the target pane.
 *
 * Percentages rather than pixels, so the same helper serves a grid cell in
 * auto mode and an absolutely positioned rectangle in manual mode.
 */
export function previewInset(zone: Zone): React.CSSProperties {
  switch (zone) {
    case 'left':
      return { inset: '0 50% 0 0' };
    case 'right':
      return { inset: '0 0 0 50%' };
    case 'top':
      return { inset: '0 0 50% 0' };
    case 'bottom':
      return { inset: '50% 0 0 0' };
    default:
      return { inset: 0 };
  }
}
