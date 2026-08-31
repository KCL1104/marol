import { test, expect } from '@playwright/test';
import {
  autoCols,
  decodeDrag,
  dragHandle,
  dropOn,
  dropOnRoot,
  encodeDrag,
  formatLayout,
  geometry,
  isSplit,
  leaves,
  materialise,
  MIN_TRACK,
  normalize,
  parseLayout,
  reconcileTree,
  removeLeaf,
  zoneAt,
  type Node,
  type Rect,
} from '../src/layout';

/** A 1000x1000 box makes every fraction read as a percentage. */
const BOX: Rect = { x: 0, y: 0, w: 1000, h: 1000 };

/**
 * Where each pane sits, as `x,y wxh` rounded to whole pixels.
 *
 * Asserting on rectangles rather than on the tree is deliberate: the tree is
 * an implementation of an arrangement, and what the user agreed to is the
 * arrangement. A refactor that reshapes the tree without moving anything on
 * screen should not fail these.
 */
function boxes(root: Node | null) {
  const out: Record<string, string> = {};
  for (const [id, r] of geometry(root, BOX, 0).panes) {
    out[id] = `${Math.round(r.x)},${Math.round(r.y)} ${Math.round(r.w)}x${Math.round(r.h)}`;
  }
  return out;
}

test.describe('dropping a pane', () => {
  test('the centre swaps two panes', () => {
    const root = materialise(['a', 'b'], 2);
    // Rearranging must not cost you the session you dropped onto.
    expect(leaves(dropOn(root, 'a', 'b', 'center'))).toEqual(['b', 'a']);
  });

  test('a session from the sidebar takes the pane it lands on', () => {
    const root = materialise(['a', 'b'], 2);
    // The evicted session is not lost — it goes back to being merely unshown,
    // and the sidebar still lists it.
    expect(leaves(dropOn(root, 'c', 'b', 'center'))).toEqual(['a', 'c']);
  });

  test('an edge splits the pane it lands on, not the row around it', () => {
    const root = materialise(['a', 'b'], 2);
    const next = dropOn(root, 'c', 'a', 'bottom');

    // `a` gives up its own half to make room, so `b` is untouched. Splitting
    // the whole row instead would move a pane the user never dragged.
    expect(boxes(next)).toEqual({
      a: '0,0 500x500',
      c: '0,500 500x500',
      b: '500,0 500x1000',
    });
  });

  test('two drags turn a row of four into a 2x2', () => {
    // The gesture this whole model exists to serve.
    let root = materialise(['a', 'b', 'c', 'd'], 4);
    expect(boxes(root).a).toBe('0,0 250x1000');

    root = dropOn(root, 'c', 'a', 'bottom');
    root = dropOn(root, 'd', 'b', 'bottom');

    expect(boxes(root)).toEqual({
      a: '0,0 500x500',
      b: '500,0 500x500',
      c: '0,500 500x500',
      d: '500,500 500x500',
    });
  });

  test('dropping alongside a same-direction split stays flat', () => {
    const root = materialise(['a', 'b'], 2);
    const next = dropOn(root, 'c', 'b', 'right');

    // Nesting here would grow a spine of two-way splits that behaves subtly
    // differently under a later drag, for no visible difference now.
    expect(next && isSplit(next) ? next.kids.length : 0).toBe(3);
    expect(boxes(next)).toEqual({
      a: '0,0 333x1000',
      b: '333,0 333x1000',
      c: '667,0 333x1000',
    });
  });

  test('restructuring resets the proportions of the split it changed', () => {
    // A new shape with the old proportions carried into it reads as a bug,
    // and the splitters are right there for fine-tuning afterwards.
    const root: Node = { dir: 'row', kids: [{ id: 'a' }, { id: 'b' }], fr: [0.9, 0.1] };
    expect(boxes(root).a).toBe('0,0 900x1000');
    expect(boxes(dropOn(root, 'c', 'b', 'right')).a).toBe('0,0 333x1000');
  });

  test('the layout edge splits above everything, not inside it', () => {
    const root = materialise(['a', 'b'], 2);
    expect(boxes(dropOnRoot(root, 'c', 'bottom'))).toEqual({
      a: '0,0 500x500',
      b: '500,0 500x500',
      c: '0,500 1000x500',
    });
  });

  test('a second drop on the same edge joins it rather than nesting', () => {
    let root = dropOnRoot(materialise(['a', 'b'], 2), 'c', 'bottom');
    root = dropOnRoot(root, 'd', 'bottom');
    // Nesting here would build a lopsided spine: c at half the height and d at
    // a quarter, from two identical gestures.
    expect(boxes(root).c).toBe('0,333 1000x333');
    expect(boxes(root).d).toBe('0,667 1000x333');
  });

  test('dropping a pane on itself changes nothing', () => {
    const root = materialise(['a', 'b'], 2);
    expect(dropOn(root, 'a', 'a', 'bottom')).toBe(root);
  });
});

test.describe('removing a pane', () => {
  test('a split with one child left stops being a split', () => {
    const root = dropOn(materialise(['a', 'b'], 2), 'c', 'a', 'bottom');
    const next = removeLeaf(root, 'c');

    // Without the collapse, `a` would stay boxed into the half-height its
    // vanished neighbour left behind.
    expect(boxes(next)).toEqual({ a: '0,0 500x1000', b: '500,0 500x1000' });
  });

  test('the last pane leaving empties the layout', () => {
    expect(removeLeaf(materialise(['a'], 1), 'a')).toBeNull();
  });

  test('what is left shares out the space', () => {
    const root = materialise(['a', 'b', 'c'], 3);
    expect(boxes(removeLeaf(root, 'b'))).toEqual({
      a: '0,0 500x1000',
      c: '500,0 500x1000',
    });
  });
});

test.describe('keeping the tree and the membership list in step', () => {
  test('a session the tab no longer holds leaves the tree', () => {
    // The core can take a session away at any time: it belongs to one tab, so
    // claiming it elsewhere removes it from under us.
    const root = materialise(['a', 'b'], 2);
    expect(leaves(reconcileTree(root, ['a']))).toEqual(['a']);
  });

  test('a session with no pane yet is appended rather than dropped', () => {
    const root = materialise(['a'], 1);
    expect(leaves(reconcileTree(root, ['a', 'b']))).toEqual(['a', 'b']);
  });

  test('an empty tab reconciles to nothing rather than throwing', () => {
    expect(reconcileTree(null, [])).toBeNull();
    expect(leaves(reconcileTree(null, ['a']))).toEqual(['a']);
  });
});

test.describe('splitter drags', () => {
  test('moving a boundary takes from one side and gives to the other', () => {
    // The third track must stay exactly where it is: a splitter moves one
    // boundary, not everything to the right of it.
    const fr = dragHandle([0.25, 0.25, 0.5], 0, 100, 2000);
    expect(fr.map((f) => Math.round(f * 2000))).toEqual([600, 400, 1000]);
  });

  test('a drag cannot crush a pane out of existence', () => {
    const fr = dragHandle([0.5, 0.5], 0, 10_000, 1000);
    expect(Math.round(fr[1] * 1000)).toBe(MIN_TRACK);
  });

  test('fractions are always renormalised, however they arrive', () => {
    expect(normalize([2, 2], 2)).toEqual([0.5, 0.5]);
    // A layout edited outside the app can be the wrong length; equal shares
    // beat guessing which pane the missing number belonged to.
    expect(normalize([1], 2)).toEqual([0.5, 0.5]);
    expect(normalize([], 3)).toEqual([1 / 3, 1 / 3, 1 / 3]);
    // Junk must not reach the renderer as a NaN width.
    const fixed = normalize([Number.NaN, 1], 2);
    expect(fixed.every(Number.isFinite)).toBe(true);
    expect(fixed.reduce((a, b) => a + b, 0)).toBeCloseTo(1);
  });
});

test.describe('drop zones', () => {
  test('the middle swaps and the edges split', () => {
    expect(zoneAt(200, 150, 400, 300)).toBe('center');
    expect(zoneAt(10, 150, 400, 300)).toBe('left');
    expect(zoneAt(390, 150, 400, 300)).toBe('right');
    expect(zoneAt(200, 10, 400, 300)).toBe('top');
    expect(zoneAt(200, 290, 400, 300)).toBe('bottom');
  });

  test('in a corner the nearer edge wins', () => {
    // A fixed axis priority would make one of the two corners unreachable.
    expect(zoneAt(5, 40, 400, 300)).toBe('left');
    expect(zoneAt(40, 5, 400, 300)).toBe('top');
  });

  test('a payload from elsewhere is rejected rather than guessed at', () => {
    // Files and text dropped on the grid must not be read as session moves.
    expect(decodeDrag('not json')).toBeNull();
    expect(decodeDrag('{"kind":"session"}')).toBeNull();
    expect(decodeDrag('{"kind":"nonsense","id":"a"}')).toBeNull();
    expect(decodeDrag(encodeDrag({ kind: 'pane', id: 'a' }))).toEqual({ kind: 'pane', id: 'a' });
  });
});

test.describe('auto columns', () => {
  const auto = { mode: 'auto', cols: 'auto' } as const;

  test('width decides, so the same tab is right on either monitor', () => {
    expect(autoCols(auto, 1010, 6)).toBe(2);
    expect(autoCols(auto, 2300, 6)).toBe(4);
    // Narrower than one readable pane still has to draw one.
    expect(autoCols(auto, 300, 6)).toBe(1);
  });

  test('never more columns than there are sessions to put in them', () => {
    // Otherwise two sessions on a wide monitor get a quarter of it each with
    // two empty tracks alongside.
    expect(autoCols(auto, 2300, 2)).toBe(2);
    expect(autoCols(auto, 2300, 0)).toBe(1);
  });

  test('an explicit column count overrides the width', () => {
    expect(autoCols({ mode: 'auto', cols: 3 }, 1010, 6)).toBe(3);
  });

  /**
   * A wall wider than three.
   *
   * Three was a picker constant with nothing behind it — the model has always
   * been able to draw more, and `autoCols` bounds the answer by the number of
   * panes rather than by any number of ours. So the only real ceiling is one
   * column per pane, and asking for more than that is asking for the same
   * arrangement by another name.
   */
  test('a wall can be as wide as it has panes', () => {
    expect(autoCols({ mode: 'auto', cols: 8 }, 1010, 8)).toBe(8);
    expect(autoCols({ mode: 'auto', cols: 12 }, 1010, 12)).toBe(12);
    // And past that it saturates rather than drawing empty tracks.
    expect(autoCols({ mode: 'auto', cols: 12 }, 4000, 5)).toBe(5);
  });

  /** A stored count survives the round trip it used to be clipped by. */
  test('a stored column count past the old ceiling is kept', () => {
    expect(parseLayout(formatLayout({ mode: 'auto', cols: 10 }))).toEqual({
      mode: 'auto',
      cols: 10,
    });
    // Still not a place to put nonsense: a corrupt row reads as one column,
    // which is the arrangement that can always be drawn.
    expect(parseLayout('{"mode":"auto","cols":-4}')).toEqual({ mode: 'auto', cols: 1 });
    expect(parseLayout('{"mode":"auto","cols":"lots"}')).toEqual({ mode: 'auto', cols: 1 });
  });
});
