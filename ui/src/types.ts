/** Mirrors src-tauri/src/core.rs. */

import type { MessageKey } from './i18n/messages';

export type Status =
  | 'saved'
  /** Still running, with nobody watching: the app was closed, tmux kept the
      agent, and this start found it alive. The opposite fact from `saved`,
      and worth its own word — a card saying "closed" over a working agent
      invites a second attempt onto the same worktree. */
  | 'detached'
  | 'starting'
  /** Sitting on Claude Code's folder-trust prompt, which every new worktree
      opens on. No hook reports this — nothing runs until it is answered — so
      the core sets it directly. See core.rs. */
  | 'awaiting_trust'
  | 'running'
  /** Blocked on a permission decision — cannot continue without you. */
  | 'waiting_permission'
  /** Idle long enough that Claude Code raised an idle prompt. */
  | 'waiting_input'
  /** Finished its turn; your move. */
  | 'idle'
  | 'exited';

/** The states where an agent is actually blocked on a human. */
export const NEEDS_YOU: readonly Status[] = [
  'waiting_permission',
  'waiting_input',
  'awaiting_trust',
];

/** Where the agents' environment came from — the field first, with the
    boolean kept as the fallback for cores that predate it. */
export function envSource(boot: BootStatus): 'login' | 'system' | 'process' {
  return boot.envSource ?? (boot.envResolved ? 'login' : 'process');
}

export const ENV_SOURCE_KEY: Record<'login' | 'system' | 'process', MessageKey> = {
  login: 'env.sourceLogin',
  system: 'env.sourceSystem',
  process: 'env.sourceProcess',
};

export function needsYou(s: Status): boolean {
  return NEEDS_YOU.includes(s);
}

/** Where a card sits on the board. Moved by hand, only ever by hand. */
export type Lifecycle = 'backlog' | 'running' | 'review' | 'done' | 'abandoned';

/** How an attempt ended. Setting one removes its worktree. */
export type Outcome = 'merged' | 'discarded' | 'superseded';

/** How much the agent may do without asking, chosen per attempt. The
    worktree is the safety case: the attempt can only spend its own branch. */
export type PermissionMode = 'normal' | 'accept_edits' | 'yolo';

/** Mirrors core.rs AttemptView. */
export interface Attempt {
  id: string;
  task_id: string;
  /** Which try this is, for `<slug>-<n>`. */
  seq: number;
  agent: string;
  worktree_path: string;
  branch: string;
  base_sha: string;
  /** How much the agent may do without asking. Approved at start, kept for
      every resume, worn as a badge while it runs. */
  mode: PermissionMode;
  /** `null` while it is still going. */
  outcome: Outcome | null;
  /** The diff, captured before the worktree was removed. */
  frozen_diff: string | null;
  created_at: number;
  /** Set while parked: worktree and slot given back, branch and
      conversation kept, resumable. Never set alongside an outcome. */
  parked_at: number | null;
  /** `null` once the attempt's session has been archived out from under it. */
  session_id: string | null;
}

/** One moment on an attempt's timeline. Mirrors store.rs AttemptEvent. */
export interface AttemptEvent {
  id: number;
  attempt_id: string;
  at: number;
  /** `prompt` — what it was asked. `tool` — what it reached for.
      `status` — when it started waiting on you, or stopped.
      `message` — what another session on this desk sent it, `tool` naming
      which one. */
  kind: 'prompt' | 'tool' | 'status' | 'message' | string;
  tool: string | null;
  detail: string | null;
}

/** One repository a card spans. Mirrors store.rs TaskRepo. */
export interface TaskRepo {
  repo_path: string;
  base_branch: string;
}

/** Mirrors core.rs TaskView. */
export interface Task {
  id: string;
  title: string;
  prompt: string;
  repo_path: string;
  base_branch: string;
  /** The repositories beside the first one. Absent on a card written by a
      build that predates cards spanning more than one — which is every card
      that has only ever had the one. */
  extra_repos?: TaskRepo[];
  lifecycle: Lifecycle;
  position: number;
  created_at: number;
  attempts: Attempt[];
  /** Where this card sits in the start queue, counting from 1, when every
      slot was taken at the moment 開始 was pressed. */
  queued_at: number | null;
}

/** Every repository a card spans, first one first — the shape the board and
    the dialogs actually use. Mirrors `StoredTask::repos`. */
export function taskRepos(task: Task): TaskRepo[] {
  return [
    { repo_path: task.repo_path, base_branch: task.base_branch },
    ...(task.extra_repos ?? []),
  ];
}

export interface SessionMeta {
  id: string;
  cwd: string;
  title: string;
  /** Which agent CLI this session runs: `claude`, `codex`, ... */
  agent: string;
  status: Status;
  created_at: number;
  last_active_at: number;
  live: boolean;
  /** What the agent is doing right now, from its last PreToolUse report. */
  activity: { tool: string; detail: string } | null;
  /** When that activity started, for the elapsed counter. */
  activity_since: number;
  /** Marked done by the user. Never inferred — `Stop` means the turn ended,
      not that the work is finished. */
  completed: boolean;
  /** True once the status plugin has reported, so the UI can tell "idle" from
      "this CLI does not report status". */
  reports_status: boolean;
  /** Whether this session's CLI was wired for status at launch. `reports_status`
      says it has spoken; this says it was given a mouth. False means it never
      will, so a card can say so at once rather than waiting out a silence with
      no end. Per session because the answer is per world — see core.rs. */
  hooks_wired: boolean;
  /** The attempt this session runs, or `null` for an ad-hoc session that
      lives outside the board. */
  attempt_id: string | null;
  /** A message is queued to go in when this turn ends. Transient — never
      stored, absent from restores. */
  has_followup?: boolean;
  /** Who has a message waiting for this session's turn to end. Empty when the
      only thing queued is the person's own note — "you left a note here" and
      "two other agents are waiting on this one" are different facts. */
  pending_from?: string[];
  /** How far what this session was last told sits from the last thing a
      person said, counted in agent-to-agent relays. Zero is a person —
      typing into the terminal puts it back there. Transient, like the queue
      it is counted from. */
  relay_hops?: number;
  /** The $MAROL_PORT a run script was handed, when reachable from the
      app (local and WSL; an SSH host's port lives on the remote). Transient
      — the server dies with the PTY. */
  preview_port?: number | null;
  /** The conversation's token account, read off its transcript at each
      turn's end. Mirrors core.rs Usage. Absent until a claude session's
      first Stop — and forever, honestly, for agents with no transcript. */
  usage?: {
    input: number;
    output: number;
    cache_read: number;
    cache_write: number;
    /** The last request's prompt size — where the next turn starts from. */
    context: number;
  } | null;
}

/** Which notifications the desk raises. Mirrors core.rs NotifyPrefs —
    blocked states default on, a finished turn defaults off. */
export interface NotifyPrefs {
  permission: boolean;
  input: boolean;
  done: boolean;
}

/** A named way to launch an agent. Mirrors store.rs Profile. */
export interface Profile {
  name: string;
  /** The CLI it launches — `claude`, `codex`, anything on the PATH. */
  agent: string;
  /** Options this profile always passes, before anything else. */
  args: string[];
}

/** One entry in a launch dialog's list. Mirrors core.rs Launcher. */
export interface Launcher {
  /** What the person picks — a bare agent's name, or a profile's. */
  name: string;
  /** The CLI it resolves to, so the dialog knows which conventions apply. */
  agent: string;
  /** True for a profile, so the list can say which entries are yours. */
  profile: boolean;
}

export interface BootStatus {
  ready: boolean;
  error?: string | null;
  shell?: string;
  envResolved?: boolean;
  /** Where the environment came from: a probed login shell, Windows' own
      process environment (the real thing there), or the degraded fallback. */
  envSource?: 'login' | 'system' | 'process';
  envVarCount?: number;
  path?: string | null;
  claude?: string | null;
  /** The installed Claude Code's version, measured at startup. */
  claudeVersion?: string | null;
  codex?: string | null;
  /** The installed Codex's version, measured at startup alongside it. */
  codexVersion?: string | null;
  /** Every agent CLI the resolved environment can see — the first-run
      panel's detection report, from the same PATH the sessions get.
      `version` and `reports` are filled in for the CLIs whose conventions
      this app knows; `reports` is whether the installed one is new enough
      to be wired for status, which is a different fact from being found. */
  agents?: Array<{
    name: string;
    path: string | null;
    version?: string | null;
    reports?: boolean;
  }>;
  /** Whether this desk's claude sessions can name themselves and, with
      that, message each other across cards. */
  messaging?: boolean;
  db?: string;
  hookUrl?: string | null;
  /** The opening-prompt template on disk — the one text this desk adds to a
      session by itself, named so the settings can open it. */
  promptTemplate?: string;
}

/** A world's path prefix: '' is local, 'wsl://Ubuntu' and 'ssh://devbox'
 *  name the others. Re-exported from worlds.ts so a component can take one
 *  without importing the helpers too. */
export type { World } from './worlds';

/** One directory inside a world. Mirrors core.rs DirListing. */
export interface DirListing {
  /** Where it really is — absolute and symlink-resolved by the world, not
      an echo of what was asked for. A picker that echoed would build its
      next path on a guess. */
  path: string;
  /** Where `..` goes, or null at a root. */
  parent: string | null;
  /** Subdirectory names, sorted, dotfiles last. Names only: the caller
      joins them, because only the world knows its own separator. */
  dirs: string[];
  /** Whether this directory is itself a git checkout. */
  is_repo: boolean;
}

/** What this build is and what updating it would take. Mirrors the
 *  update_status command in main.rs; every field is free to answer, so the
 *  panel paints without waiting on GitHub. */
export interface UpdateStatus {
  /** The running build's own version — the one number the app could not
      previously tell you about itself. */
  version: string;
  /** Whether this build carries a public key to verify a download against.
      False in a build made before the key existed, and the reason the panel
      says so rather than offering a button that could only fail. */
  configured: boolean;
  /** The off switch. On by default. */
  enabled: boolean;
  /** Whether this copy owns the file it runs from. False for a deb or rpm,
      whose package manager keeps its own record of the files it owns. */
  selfContained: boolean;
  /** Live agents a tmux in their own world would hand back after a restart. */
  held: number;
  /** Live agents a restart would end, because nothing is holding them. */
  lost: number;
  /** When the last check happened, epoch seconds, or null for never. */
  lastCheck: number | null;
  /** Whether enough time has passed to ask again. */
  due: boolean;
  /** Where to send someone this app will not update itself. */
  releases: string;
}

/** A release worth offering. Mirrors update.rs Available. */
export interface UpdateAvailable {
  version: string;
  notes: string | null;
  date: string | null;
}

/** A named working arrangement. Mirrors src-tauri/src/store.rs StoredTab. */
export interface Tab {
  id: string;
  name: string;
  layout: string;
  slots: Array<string | null>;
  position: number;
}
