import type { MessageKey } from './i18n';

/**
 * Every named thing the palette can do, in one table.
 *
 * The table is the point, more than any row in it: the palette, the ⌘/
 * cheat sheet, and whatever menus come later all render from here, so an
 * action's name, its chord, and its documentation cannot drift apart —
 * the same discipline `STATUS_KEY` holds for status names, applied to
 * verbs. Adding an action here makes it searchable and documented in the
 * same keystroke; the App supplies what each id actually does, because
 * running one needs the App's own state and this module needs none of it.
 */
export type ActionId =
  | 'jump-waiting'
  | 'last-session'
  | 'new-card'
  | 'new-session'
  | 'toggle-sidebar'
  | 'toggle-inspector'
  | 'view-terminal'
  | 'view-board'
  | 'view-overview'
  | 'open-settings'
  | 'open-keys'
  | 'show-welcome'
  | 'replay-coach';

/** What an action's `when` may ask about. Kept to judgements the palette
 *  needs — this is visibility, not enablement: an action that cannot
 *  apply right now is simply not offered, never greyed. */
export interface ActionCtx {
  /** Any session currently blocked on a human. */
  hasWaiting: boolean;
  /** There is an attempt to inspect — a drawer up, or a focused pane
   *  that belongs to one. */
  canInspect: boolean;
  /** Somewhere to go back to: a session visited before this one, still
   *  alive. Offering "back" with no back is worse than not offering it. */
  hasPrevious: boolean;
}

export interface ActionDef {
  id: ActionId;
  title: MessageKey;
  /** The chord exactly as the cheat sheet prints it; null for actions
   *  that have buttons instead of keys. */
  keys: string | null;
  /** Absent means always offered. */
  when?: (ctx: ActionCtx) => boolean;
}

/** In the order the palette offers them: triage first, making second,
 *  navigation third, the app's own surfaces last. */
export const ACTIONS: readonly ActionDef[] = [
  { id: 'jump-waiting', title: 'keys.jump', keys: '⌘/Ctrl + E', when: (c) => c.hasWaiting },
  // ⌘E 的另一半。E 是「去該去的地方」(注意力),L 是「回到剛才那個」
  // (記憶)——答完一個插隊的 agent 之後,回去繼續原本在做的事。
  // 選 L 是因為 K 進不來:終端機內 Ctrl+Shift+K 已經是叫出面板的寫法。
  { id: 'last-session', title: 'keys.last', keys: '⌘/Ctrl + L', when: (c) => c.hasPrevious },
  // Shift 生在和弦裡,所以終端機內外都是同一顆 —— shell 擁有的是
  // 無 Shift 的 Ctrl+字母。N 沒人用過;K 不行(終端機內 Ctrl+Shift+K
  // 已經是叫出面板的寫法),C 是複製,E/F/I 各有原主。
  { id: 'new-card', title: 'board.newCard', keys: '⌘/Ctrl + Shift + N' },
  { id: 'new-session', title: 'sidebar.newSession', keys: null },
  // B 是每個有側欄的編輯器都用的那顆,所以它不必被學。Ctrl+B 是
  // readline 的 backward-char,終端機內因此要加 Shift —— 與 E/L/F/I
  // 同一條規則,不是這顆的例外。
  { id: 'toggle-sidebar', title: 'keys.sidebar', keys: '⌘/Ctrl + B' },
  { id: 'toggle-inspector', title: 'keys.inspector', keys: '⌘/Ctrl + I', when: (c) => c.canInspect },
  { id: 'view-terminal', title: 'view.terminal', keys: '⌘/Ctrl + 1' },
  { id: 'view-board', title: 'view.board', keys: '⌘/Ctrl + 2' },
  { id: 'view-overview', title: 'view.overview', keys: '⌘/Ctrl + 3' },
  { id: 'open-settings', title: 'common.env', keys: '⌘/Ctrl + ,' },
  { id: 'open-keys', title: 'keys.title', keys: '⌘/Ctrl + /' },
  // 重看歡迎面板:偵測重跑、旗標不動 —— 給剛裝好 CLI 的人一條回門口的路。
  { id: 'show-welcome', title: 'welcome.reopen', keys: null },
  // 面板是門口,導覽是課。介面的字收短之後,這五個時刻才是概念的家 ——
  // 一堂只上得了一次的課,值得給一條重修的路。
  { id: 'replay-coach', title: 'coach.replay', keys: null },
];

/** Keyboard that is not an action — movement and editing chords the sheet
 *  documents but nobody would run from a palette. */
export const KEY_DOCS: readonly { combo: string; what: MessageKey }[] = [
  { combo: '⌘/Ctrl + K', what: 'keys.palette' },
  { combo: '⌘/Ctrl + F', what: 'keys.find' },
  { combo: '⌘/Ctrl + ⌥/Alt + ← · →', what: 'keys.cyclePanes' },
  { combo: '⌘/Ctrl + ← → · ↑ ↓', what: 'keys.moveCard' },
  { combo: 'Ctrl + PgDn · PgUp', what: 'keys.cycleTabs' },
  { combo: 'J · K', what: 'inspector.diffKeys' },
  { combo: 'Esc', what: 'keys.escape' },
];

/**
 * Keys that belong to the agent, not to this desk.
 *
 * Listed apart from `KEY_DOCS` on purpose: everything in that table is a
 * chord Marol binds and could change, and everything here is one the CLI
 * owns and Marol merely knows about. Folding them together would tell a
 * reader this app is responsible for a key it cannot alter.
 *
 * They earn a place at all because of what the wheel can and cannot do. On
 * the alternate buffer — which is every agent pane in a world that has tmux
 * — a wheel notch is cursor keys sent to the program, so it walks whatever
 * that program calls scrolling. Codex's own transcript view is the thing
 * that actually holds the conversation, and nothing on screen said so.
 *
 * Codex only, because Codex is the CLI whose table was measured
 * (`keymap.rs`, the pager keymap). An entry here for a CLI nobody checked
 * would be a guess wearing a shortcut's clothes.
 */
export const AGENT_KEYS: readonly { agent: string; combo: string; what: MessageKey }[] = [
  { agent: 'codex', combo: 'Ctrl + T', what: 'keys.codexTranscript' },
  { agent: 'codex', combo: 'PgUp · PgDn · Ctrl+U · Ctrl+D · j · k', what: 'keys.codexPager' },
];

/**
 * The pointer's side of the sheet. These lived only in tooltips, which the
 * sheet's own comment calls the way gestures go unfound — so the sheet
 * lists them beside the keys, one row per surface.
 */
export const GESTURES: readonly { where: MessageKey; what: MessageKey }[] = [
  { where: 'gesture.pane', what: 'pane.dragHint' },
  { where: 'gesture.tab', what: 'gesture.tabWhat' },
  { where: 'gesture.splitter', what: 'splitter.hint' },
  { where: 'gesture.row', what: 'gesture.rowWhat' },
];
