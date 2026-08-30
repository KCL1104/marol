/**
 * How far one wheel event should move a full-screen TUI.
 *
 * This exists because of a gap in xterm.js, not a preference. On the
 * alternate buffer there is no scrollback to move, so xterm converts a wheel
 * event into cursor keys and lets the program decide — the right idea, with
 * two costs this codebase cannot absorb:
 *
 *   * it sends **one** arrow per event, discarding the line count it just
 *     computed (CoreBrowserTerminal.ts:826-835). 5.5.0 sent the full count;
 *     6.0.0 does not, so a wheel notch moves a fifth of what it used to.
 *   * it damps pixel deltas under 50px by 0.3 as "likely trackpad" and floors
 *     the accumulator to whole cells (CoreMouseService.ts:255-263). With a
 *     ~17px cell and the ~4px deltas a trackpad actually emits, that is
 *     ~0.07 cells per event: roughly thirteen out of every fourteen events
 *     send nothing at all. On a laptop that is not an edge case, it is the
 *     normal way of scrolling, and it reads as "the wheel is dead".
 *
 * So the arithmetic is ours. The carry is the whole reason this is a pure
 * function with state passed through it: sub-cell deltas have to accumulate
 * across events or a trackpad can never reach one line, and a test has to be
 * able to feed it a stream of small deltas and watch a line eventually come
 * out.
 */

/** `WheelEvent.deltaMode`, named. The DOM constants are on the event class,
 *  which does not exist in a plain unit-test environment. */
export const DELTA_PIXEL = 0;
export const DELTA_LINE = 1;
export const DELTA_PAGE = 2;

export interface WheelStep {
  /** Whole lines to send now. Negative is up, positive is down. */
  lines: number;
  /** Sub-line remainder to feed back into the next call. */
  carry: number;
}

/**
 * One wheel event, in lines.
 *
 * `cellHeight` and `rows` come from the live terminal; a terminal that has
 * not been laid out yet reports zero, and zero lines is the honest answer
 * there — better a notch that does nothing than one that scrolls by a
 * nonsense amount computed from a nonsense cell.
 *
 * The cap is per event and deliberate: a flick on a high-resolution trackpad
 * can carry hundreds of pixels, and every line here becomes a keystroke the
 * agent has to process. One screen per event is as much as any reader wants
 * and far more than the PTY should be asked to swallow at once.
 */
export function wheelStep(
  delta: number,
  deltaMode: number,
  cellHeight: number,
  rows: number,
  carry: number,
): WheelStep {
  if (delta === 0 || !Number.isFinite(delta)) return { lines: 0, carry };
  if (!(rows > 0)) return { lines: 0, carry };

  let inLines: number;
  if (deltaMode === DELTA_PAGE) {
    inLines = delta * rows;
  } else if (deltaMode === DELTA_LINE) {
    inLines = delta;
  } else {
    // Pixels. No trackpad damping: the whole point is that a small delta is
    // still a real delta and must be allowed to accumulate into a line.
    if (!(cellHeight > 0)) return { lines: 0, carry };
    inLines = delta / cellHeight;
  }

  // A direction change abandons the carry rather than fighting it — half a
  // line of downward intent must not eat the first line of an upward one.
  const total = (carry !== 0 && Math.sign(carry) !== Math.sign(inLines) ? 0 : carry) + inLines;
  const whole = Math.trunc(total);
  const capped = Math.max(-rows, Math.min(rows, whole));
  return { lines: capped, carry: total - whole };
}

/**
 * The bytes that move a TUI by `lines`, or '' for no movement.
 *
 * Cursor keys, because that is the only vocabulary a program on the
 * alternate buffer has agreed to hear for this — the same sequence xterm
 * sends, sent the right number of times. `applicationCursorKeys` (DECCKM)
 * picks the SS3 form; tmux turns it on at attach, so getting this wrong
 * would send `ESC [ A` into a session expecting `ESC O A`.
 */
export function wheelSequence(lines: number, applicationCursorKeys: boolean): string {
  if (lines === 0) return '';
  const intro = applicationCursorKeys ? '\x1bO' : '\x1b[';
  return (intro + (lines < 0 ? 'A' : 'B')).repeat(Math.abs(lines));
}
