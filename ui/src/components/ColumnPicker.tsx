import { useT } from '../i18n';
import type { Layout } from '../layout';

/**
 * Column count for auto mode, and the way back out of a hand-built layout.
 *
 * `自訂` is not something you can pick — it appears once a drag has given the
 * tab an explicit split tree, and exists so the control still describes what
 * you are looking at. Choosing anything else discards the tree, which is the
 * only undo a hand-built layout has.
 *
 * The list runs to however many panes are on the wall, rather than to a fixed
 * three. Three was a number with nothing behind it: `autoCols` already
 * refuses to draw more columns than there are panes, so one column per pane
 * — a single row, everything side by side — is the widest arrangement that
 * can differ from any other, and it was unreachable for anybody with four
 * terminals open. Past that the options would be identical to each other,
 * which is not more choice, only a longer list.
 */
export function ColumnPicker({
  layout,
  panes,
  onPick,
}: {
  layout: Layout;
  /** How many panes the tab is showing — the point past which another
   *  column changes nothing. */
  panes: number;
  onPick: (value: 'auto' | number) => void;
}) {
  const t = useT();
  const manual = layout.mode === 'manual';
  const value = manual ? 'manual' : String(layout.cols);
  // At least three, so an empty or nearly empty wall keeps the choices it has
  // always had; and never fewer than the count already chosen, or picking 8
  // and then closing a session would leave the select describing a value it
  // no longer offers.
  const most = Math.max(3, panes, manual || layout.cols === 'auto' ? 0 : layout.cols);
  const COUNTS = Array.from({ length: most }, (_, i) => i + 1);

  return (
    <label className="col-picker">
      <span className="muted small">{t('cols.label')}</span>
      <select
        data-testid="col-picker"
        value={value}
        title={manual ? t('cols.manualHint') : undefined}
        onChange={(e) => {
          const v = e.target.value;
          onPick(v === 'auto' ? 'auto' : Number(v));
        }}
      >
        {manual && (
          <option value="manual" disabled>
            {t('cols.custom')}
          </option>
        )}
        <option value="auto">{t('cols.auto')}</option>
        {COUNTS.map((n) => (
          <option key={n} value={String(n)}>
            {/* English needs the singular; zh-TW's 欄 never inflects. */}
            {n === 1 ? t('cols.one') : t('cols.n', { n })}
          </option>
        ))}
      </select>
    </label>
  );
}
