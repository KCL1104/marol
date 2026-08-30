/**
 * Whether the sidebar is folded down to its rail.
 *
 * A per-machine preference, stored the way the world and the locale are.
 * "Folded" rather than "collapsed" on purpose: the sections *inside* the
 * sidebar already collapse, and one word for two different things is how a
 * later reader ends up toggling the wrong one.
 */
const KEY = 'marol.sidebar';

export function storedFolded(): boolean {
  return localStorage.getItem(KEY) === 'folded';
}

export function rememberFolded(folded: boolean) {
  // Absent is the resting state, the same shape `rememberWorld` keeps: a
  // fresh desk has never folded anything, and should read as such.
  if (folded) localStorage.setItem(KEY, 'folded');
  else localStorage.removeItem(KEY);
}
