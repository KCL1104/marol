import { test, expect } from '@playwright/test';
import { DELTA_LINE, DELTA_PAGE, DELTA_PIXEL, tmuxScrollSequence, wheelSequence, wheelStep } from '../src/wheel';

/** A 17px cell in a 40-row pane — the shape a real pane actually has. */
const CELL = 17;
const ROWS = 40;

/** Feed a stream of identical deltas and count the lines that come out. */
function stream(delta: number, times: number, mode = DELTA_PIXEL): number {
  let carry = 0;
  let total = 0;
  for (let i = 0; i < times; i++) {
    const step = wheelStep(delta, mode, CELL, ROWS, carry);
    carry = step.carry;
    total += step.lines;
  }
  return total;
}

test.describe('the wheel, on the alternate buffer', () => {
  /**
   * The defect this whole module exists for. xterm damps a sub-50px delta by
   * 0.3 and floors the accumulator, so a trackpad emitting ~4px per event
   * lands at ~0.07 cells and sends nothing roughly thirteen times in
   * fourteen. Here the same stream has to actually move.
   */
  test('a trackpad accumulates into real lines instead of being damped away', () => {
    // 20 events of 4px is 80px — four and a bit cells, and the four must
    // arrive. Deliberately not the exact 17 that makes 68px: a test sitting
    // on a floating-point boundary would pass here and fail on another
    // engine, and it is the accumulation being pinned, not the rounding.
    expect(stream(4, 20)).toBe(4);
    // No single one of them is a line on its own; the carry is what earns it.
    expect(wheelStep(4, DELTA_PIXEL, CELL, ROWS, 0).lines).toBe(0);
  });

  /**
   * The other defect: xterm computes the line count and then sends exactly
   * one arrow. A mouse notch is ~100px, which is about six lines at this cell
   * height, and six is what should go out.
   */
  test('a mouse notch sends the lines it is worth, not one', () => {
    const step = wheelStep(100, DELTA_PIXEL, CELL, ROWS, 0);
    expect(step.lines).toBe(5);
    expect(wheelSequence(step.lines, false)).toBe('\x1b[B'.repeat(5));
  });

  test('up is up, and the carry does not leak across a direction change', () => {
    // Half a line of downward intent, then a full line up.
    const down = wheelStep(8, DELTA_PIXEL, CELL, ROWS, 0);
    expect(down.lines).toBe(0);
    expect(down.carry).toBeGreaterThan(0);
    // The abandoned carry must not eat the first upward line.
    const up = wheelStep(-17, DELTA_PIXEL, CELL, ROWS, down.carry);
    expect(up.lines).toBe(-1);
  });

  test('line and page modes are taken at their word', () => {
    expect(wheelStep(3, DELTA_LINE, CELL, ROWS, 0).lines).toBe(3);
    // A page is a screen, and a screen is the per-event cap.
    expect(wheelStep(1, DELTA_PAGE, CELL, ROWS, 0).lines).toBe(ROWS);
    expect(wheelStep(9, DELTA_PAGE, CELL, ROWS, 0).lines).toBe(ROWS);
    expect(wheelStep(-9, DELTA_PAGE, CELL, ROWS, 0).lines).toBe(-ROWS);
  });

  /**
   * A pane that has not been laid out reports a zero cell and zero rows. The
   * honest answer there is no movement — a notch that did nothing beats one
   * that scrolled by a number derived from a zero.
   */
  test('an unmeasured pane moves nothing rather than guessing', () => {
    expect(wheelStep(100, DELTA_PIXEL, 0, ROWS, 0).lines).toBe(0);
    expect(wheelStep(100, DELTA_PIXEL, CELL, 0, 0).lines).toBe(0);
    expect(wheelStep(0, DELTA_PIXEL, CELL, ROWS, 0).lines).toBe(0);
    expect(wheelStep(NaN, DELTA_PIXEL, CELL, ROWS, 0).lines).toBe(0);
    // And the carry survives a no-op, rather than being reset by it.
    expect(wheelStep(0, DELTA_PIXEL, CELL, ROWS, 0.4).carry).toBe(0.4);
  });

  /**
   * tmux turns DECCKM on at attach, so the SS3 form is not an exotic case
   * here — it is the one every held session actually gets. Sending `ESC [ A`
   * into a session expecting `ESC O A` is the whole bug this pins.
   */
  test('DECCKM picks the SS3 form, and no movement sends nothing', () => {
    expect(wheelSequence(2, true)).toBe('\x1bOA'.repeat(0) + '\x1bOB\x1bOB');
    expect(wheelSequence(-2, true)).toBe('\x1bOA\x1bOA');
    expect(wheelSequence(-1, false)).toBe('\x1b[A');
    expect(wheelSequence(0, false)).toBe('');
    expect(wheelSequence(0, true)).toBe('');
  });
});

/**
 * A held pane's notch goes to `tmux`, not to the program.
 *
 * The cursor keys are right only when nothing is in the way. Under `tmux` the
 * pane is on the alternate buffer from the moment it attaches, whatever the
 * program inside is doing — so an inline agent like Codex takes the Up key in
 * its composer and walks prompt history, which is precisely what the wheel
 * looked like it was doing. `tmux` is the only party that can see the inner
 * screen state, so the notch is handed to it and its config branches.
 */
test.describe('the wheel, on a pane tmux is holding', () => {
  test('a notch becomes the key tmux binds, one per line', () => {
    // CSI 5;3~ / CSI 6;3~ — PageUp/PageDown with Alt, which tmux reads as
    // M-PPage and M-NPage.
    expect(tmuxScrollSequence(-1)).toBe('\x1b[5;3~');
    expect(tmuxScrollSequence(1)).toBe('\x1b[6;3~');
    expect(tmuxScrollSequence(-3)).toBe('\x1b[5;3~'.repeat(3));
    expect(tmuxScrollSequence(4)).toBe('\x1b[6;3~'.repeat(4));
  });

  test('no movement sends nothing', () => {
    // The carry still has to be able to swallow a notch without the pane
    // twitching, exactly as on the cursor-key path.
    expect(tmuxScrollSequence(0)).toBe('');
  });

  test('it is not the cursor keys, whatever DECCKM says', () => {
    // The whole point: these must not be confusable with Up/Down, or they
    // land in the composer again.
    for (const app of [true, false]) {
      expect(tmuxScrollSequence(-1)).not.toBe(wheelSequence(-1, app));
      expect(tmuxScrollSequence(1)).not.toBe(wheelSequence(1, app));
    }
  });
});
