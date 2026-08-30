/**
 * A stand-in for the Tauri IPC bridge, injected before the app's modules load.
 *
 * macOS ships WKWebView with no WebDriver endpoint, so the packaged window
 * cannot be driven directly. Running the same React tree in Chromium against
 * this mock still exercises everything above the IPC boundary — the session
 * list, the new-session flow, and xterm's decoding and rendering of real PTY
 * bytes — which is where both reported bugs live.
 */

export interface MockSession {
  id: string;
  cwd: string;
  title: string;
  agent: string;
  status: string;
  created_at: number;
  last_active_at: number;
  live: boolean;
  reports_status: boolean;
  hooks_wired: boolean;
  /** Who has a message waiting for this session's turn to end. Absent is the
   *  ordinary case; a person's own note leaves it empty. */
  pending_from?: string[];
  /** The $MAROL_PORT a run script was handed, when reachable. */
  preview_port: number | null;
  /** The conversation's token account — tests seed it, the core computes it. */
  usage?: {
    input: number;
    output: number;
    cache_read: number;
    cache_write: number;
    context: number;
  } | null;
  activity: { tool: string; detail: string } | null;
  activity_since: number;
  completed: boolean;
  attempt_id: string | null;
  has_followup?: boolean;
}

export interface MockAttempt {
  id: string;
  task_id: string;
  seq: number;
  agent: string;
  worktree_path: string;
  branch: string;
  base_sha: string;
  mode: string;
  outcome: string | null;
  frozen_diff: string | null;
  created_at: number;
  /** Set while parked — worktree and slot given back, resumable. */
  parked_at: number | null;
  session_id: string | null;
}

export interface MockEvent {
  id: number;
  attempt_id: string;
  at: number;
  kind: string;
  tool: string | null;
  detail: string | null;
}

export interface MockTask {
  id: string;
  title: string;
  prompt: string;
  repo_path: string;
  base_branch: string;
  /** Absent is a card with the one repository — which is nearly all of
      them, and what every fixture written before this meant. */
  extra_repos?: Array<{ repo_path: string; base_branch: string }>;
  lifecycle: string;
  position: number;
  created_at: number;
  attempts: MockAttempt[];
  queued_at: number | null;
}

export interface MockTab {
  id: string;
  name: string;
  layout: string;
  slots: Array<string | null>;
  position: number;
}

declare global {
  interface Window {
    __mock: {
      sessions: MockSession[];
      tabs: MockTab[];
      tasks: MockTask[];
      /** Which repositories exist, and what branches they have. The core
          refuses a card it cannot open a worktree for, so the mock must too. */
      repos: Record<string, string[]>;
      /** What each attempt's worktree currently shows as changed. */
      diffs: Map<string, string>;
      /** Per-file full text, both sides, keyed `<attemptId>:<path>` —
          what attempt_file answers and write_attempt_file mutates. */
      files: Map<string, { base: string | null; work: string | null }>;
      /** Each open attempt's numstat footprint, as the core measures it. */
      stats: Map<
        string,
        { files: number; adds: number; dels: number; ahead: number; behind: number; dirty: boolean }
      >;
      events: Map<string, MockEvent[]>;
      /** Cards waiting for a slot, in order. */
      queue: string[];
      pendingStarts: Map<string, { agent: string; prompt: string; mode: string }>;
      /** Attempts whose worktree has uncommitted work, so merge must refuse. */
      dirtyWorktrees: Set<string>;
      /** The repo's `.marol/config.json` run script names. */
      runScripts: string[];
      /** The checkouts the Knows tab's project rows belong to, for a session
          standing in a workspace that holds several. Empty is one checkout —
          the ordinary session, whose rows carry no folder. */
      knowsDirs: string[];
      /** Each attempt's worktree shell, while one is live — the core's cache. */
      shells: Map<string, string>;
      /** One message per session, held for the end of its turn. */
      queuedFollowups: Map<string, string>;
      /** Named launch profiles, as the settings table holds them. */
      profiles: Array<{ name: string; agent: string; args: string[] }>;
      /** Which notifications the desk raises, as the core defaults them. */
      notifyPrefs: { permission: boolean; input: boolean; done: boolean };
      /** Whether the end of a turn snapshots the worktree — default on. */
      checkpointsOn: boolean;
      agentUpdatesOn: boolean;
      /** Each attempt's checkpoints, as the refs would hold them. */
      checkpoints: Map<string, Array<{ n: number; sha: string; at: number }>>;
      /** Makes the next manual checkpoint answer "nothing new". */
      checkpointQuiet: boolean;
      /** Seeded by tests: what resume_attempt reports as restore_error. */
      resumeRestoreError: string | null;
      /** Monotonic session id source — ids are never reissued. */
      sessionSeq: number;
      /** What probe_port answers — false plays an unreachable server. */
      portListening: boolean;
      /** Measured CLIs whose version is too old to be wired for status, so a
       *  session of theirs launches with no hooks at all. */
      unwiredAgents: string[];
      /** What the world switch enumerates. */
      worlds: { wsl: string[]; ssh: string[] };
      /** Seeded by tests: per-world probe answers. */
      worldProbes: Map<
        string,
        { claude: string | null; codex?: string | null; error: string | null }
      >;
      maxConcurrent: number;
      /** How many attempts hold a terminal right now. */
      running(): number;
      drainQueue(): void;
      /** Stand in for a hook landing on an attempt's timeline. */
      record(attemptId: string, kind: string, tool: string | null, detail: string | null): void;
      persist(): void;
      pushSessions(): void;
      pushTasks(): void;
      sorted(): MockSession[];
      calls: Array<{ cmd: string; args: unknown }>;
      listeners: Map<string, number[]>;
      cbSeq: number;
      snapshots: Map<string, { data: string; seq: number }>;
      emit(event: string, payload: unknown): void;
      /** Push a base64 chunk to a session's terminal, as the PTY would. */
      feed(id: string, data: string, seq: number): void;
      /** Stand in for a hook report: change status and optionally activity. */
      report(id: string, status: string, activity?: { tool: string; detail: string }): void;
    };
  }
}

/**
 * Runs inside the page before any app code. Kept as one self-contained
 * function so Playwright can pass it straight to `addInitScript`.
 */
export function installMock(): void {
  // Pin the language. Without this the suite would render in whatever locale
  // the CI browser happens to report, and every assertion below that names a
  // button would pass or fail by accident. The switcher itself is covered in
  // i18n.spec.ts, which overrides this deliberately.
  try {
    // Only when nothing has been chosen: this script re-runs on every load,
    // including reloads, so setting it unconditionally would overwrite a
    // language the test just switched to and make persistence untestable.
    if (localStorage.getItem('marol.locale') === null) {
      localStorage.setItem('marol.locale', 'zh-TW');
    }
    // Pre-answer the one-shot surfaces the same way: a suite about the
    // board must not fight a welcome dialog or a coaching card. The specs
    // about those surfaces remove these keys in their own init script.
    if (localStorage.getItem('marol.welcomed') === null) {
      localStorage.setItem('marol.welcomed', '1');
    }
    if (localStorage.getItem('marol.coach') === null) {
      localStorage.setItem(
        'marol.coach',
        JSON.stringify({ attempt: true, mode: true, finish: true, terminal: true, waiting: true }),
      );
    }
  } catch {
    /* storage unavailable; detection falls back to the browser locale */
  }

  const mock = {
    sessions: JSON.parse(
      sessionStorage.getItem('__mockSessions') ?? '[]',
    ) as MockSession[],
    tabs: JSON.parse(
      sessionStorage.getItem('__mockTabs') ??
        '[{"id":"t1","name":"工作區","layout":"{\\"mode\\":\\"auto\\",\\"cols\\":\\"auto\\"}","slots":[],"position":0}]',
    ) as MockTab[],
    tasks: JSON.parse(sessionStorage.getItem('__mockTasks') ?? '[]') as MockTask[],
    repos: { '/Users/test/picked-repo': ['main', 'develop'] } as Record<string, string[]>,
    diffs: new Map<string, string>(),
    files: new Map<string, { base: string | null; work: string | null }>(),
    stats: new Map<
      string,
      { files: number; adds: number; dels: number; ahead: number; behind: number; dirty: boolean }
    >(),
    events: new Map<string, MockEvent[]>(),
    queue: [] as string[],
    maxConcurrent: 3,
    pendingStarts: new Map<string, { agent: string; prompt: string; mode: string }>(),
    dirtyWorktrees: new Set<string>(),
    runScripts: [] as string[],
    knowsDirs: [] as string[],
    shells: new Map<string, string>(),
    queuedFollowups: new Map<string, string>(),
    profiles: [] as Array<{ name: string; agent: string; args: string[] }>,
    notifyPrefs: { permission: true, input: true, done: false },
    checkpointsOn: true,
    agentUpdatesOn: true,
    checkpoints: new Map<string, Array<{ n: number; sha: string; at: number }>>(),
    checkpointQuiet: false,
    /** One directory tree per world, keyed by the world's own prefix. The
     *  local and WSL trees deliberately differ under the same path names:
     *  `/home/you` exists in both, and only what is inside says which
     *  machine the picker is actually reading. */
    dirs: {
      '': {
        '/home/you': ['code', 'Downloads', '.config'],
        '/home/you/code': ['picked-repo'],
        '/home/you/code/picked-repo': [],
        '/home/you/Downloads': [],
        '/home/you/.config': [],
        '/home': ['you'],
        // The path the journeys walk to. It lives outside home on purpose:
        // typing a path you already know is the picker's fast lane, and a
        // journey that only ever clicked would never exercise it.
        '/Users/test': ['picked-repo'],
        '/Users/test/picked-repo': [],
        '/Users': ['test'],
        '/': ['home', 'Users'],
      },
      'wsl://Ubuntu': {
        '/home/you': ['service', 'client'],
        '/home/you/service': [],
        '/home/you/client': [],
        '/home': ['you'],
        '/': ['home'],
      },
      'ssh://devbox': {
        '/home/you': ['deploy'],
        '/home/you/deploy': [],
        '/home': ['you'],
        '/': ['home'],
      },
    } as Record<string, Record<string, string[]>>,
    /** Where each world's picker opens when asked for no path. */
    dirHome: {
      '': '/home/you',
      'wsl://Ubuntu': '/home/you',
      'ssh://devbox': '/home/you',
    } as Record<string, string>,
    /** Which paths answer "yes, a git checkout". */
    dirRepos: [
      '/home/you/code/picked-repo',
      '/home/you/service',
      '/Users/test/picked-repo',
    ] as string[],

    /** The updater's whole surface. Defaults describe the build a person
     *  actually has: a real version, a key in place, self-contained, and no
     *  newer release waiting — so every existing test paints the "you are on
     *  the newest" branch and only the update tests move off it. */
    update: {
      version: '0.6.0',
      configured: true,
      enabled: true,
      selfContained: true,
      /** What a restart would do to the agents running now, split the way
       *  core.rs splits it: `held` come back, `lost` do not. Seeded by
       *  tests; zero is the desk with nothing running. */
      held: 0,
      lost: 0,
      lastCheck: null as number | null,
      due: true,
      releases: 'https://github.com/KCL1104/marol/releases/latest',
      /** Seeded by tests: what a check finds. Null is "you are on it". */
      available: null as { version: string; notes: string | null; date: string | null } | null,
      /** Seeded by tests: what applying fails with, if it should. */
      applyError: null as string | null,
      /** Recorded so a test can assert the snapshot happened before the
       *  swap, and that acknowledgement was carried rather than assumed. */
      applied: [] as boolean[],
    },
    resumeRestoreError: null as string | null,
    sessionSeq: 0,
    portListening: true,
    unwiredAgents: [] as string[],
    /** What the world switch enumerates — a Windows machine's shape. */
    worlds: { wsl: ['Ubuntu'], ssh: ['devbox'] } as { wsl: string[]; ssh: string[] },
    /** Seeded by tests: what probing a world answers. */
    worldProbes: new Map<
      string,
      { claude: string | null; codex?: string | null; error: string | null }
    >(),
    calls: [] as Array<{ cmd: string; args: unknown }>,
    listeners: new Map<string, number[]>(),
    cbSeq: 0,
    snapshots: new Map<string, { data: string; seq: number }>(),

    /** The core sorts by last activity, newest first. */
    sorted() {
      return [...mock.sessions].sort((a, b) => b.last_active_at - a.last_active_at);
    },

    emit(event: string, payload: unknown) {
      // Deep-copy the way Tauri's IPC does. Emitting a live reference would
      // let React's identity check skip work that the real app always does,
      // and the mock would report bugs the product does not have.
      const frozen = JSON.parse(JSON.stringify(payload)) as unknown;
      for (const id of mock.listeners.get(event) ?? []) {
        const cb = (window as unknown as Record<string, unknown>)[`_${id}`];
        if (typeof cb === 'function') {
          (cb as (m: unknown) => void)({ event, id: 0, payload: frozen });
        }
      }
    },

    /**
     * Stand in for the core surviving a reload.
     *
     * Reloading the webview does not restart the Rust side, so the sessions
     * are still running and still live when the page comes back. Dropping
     * them here instead would empty every tab on reload and make the layout
     * look as though it had not been saved.
     */
    persist() {
      sessionStorage.setItem('__mockTabs', JSON.stringify(mock.tabs));
      sessionStorage.setItem('__mockSessions', JSON.stringify(mock.sessions));
      sessionStorage.setItem('__mockTasks', JSON.stringify(mock.tasks));
    },

    /** Save, then broadcast — the order the real core writes and emits in. */
    pushSessions() {
      mock.persist();
      queueMicrotask(() => mock.emit('sessions:changed', mock.sorted()));
    },

    pushTasks() {
      // The core recomputes each card's queue position on every broadcast.
      for (const t of mock.tasks) {
        const at = mock.queue.indexOf(t.id);
        t.queued_at = at < 0 ? null : at + 1;
      }
      mock.persist();
      queueMicrotask(() => mock.emit('tasks:changed', mock.tasks));
    },

    feed(id: string, data: string, seq: number) {
      mock.emit('term:output', { id, data, seq });
    },

    report(id: string, status: string, activity?: { tool: string; detail: string }) {
      const s = mock.sessions.find((x) => x.id === id);
      if (!s) return;
      s.status = status;
      s.reports_status = true;
      if (activity) {
        s.activity = activity;
        s.activity_since = Date.now();
      }
      // The Stop hook's half: the turn ended, so what waited for it goes
      // in as the next one — recorded like any follow-up.
      if (status === 'idle') {
        const queued = mock.queuedFollowups.get(id);
        if (queued !== undefined) {
          mock.queuedFollowups.delete(id);
          s.has_followup = false;
          if (s.attempt_id) mock.record(s.attempt_id, 'prompt', null, queued);
        }
      }
      mock.emit('sessions:changed', mock.sorted());
    },

    record(attemptId: string, kind: string, tool: string | null, detail: string | null) {
      const rows = mock.events.get(attemptId) ?? [];
      rows.push({
        id: rows.length + 1,
        attempt_id: attemptId,
        at: Date.now(),
        kind,
        tool,
        detail,
      });
      mock.events.set(attemptId, rows);
    },

    running() {
      return mock.sessions.filter((s) => s.live && s.attempt_id !== null).length;
    },

    /** Start whatever the freed slots can take, as the core does. */
    drainQueue() {
      while (mock.queue.length > 0 && mock.running() < mock.maxConcurrent) {
        const taskId = mock.queue.shift()!;
        const pending = mock.pendingStarts.get(taskId);
        mock.pendingStarts.delete(taskId);
        if (pending) startAttempt(taskId, pending.agent, pending.prompt, pending.mode);
      }
    },

    /** The core renumbers both affected columns on every move. */
    renumber() {
      for (const life of ['backlog', 'running', 'review', 'done', 'abandoned']) {
        mock.tasks
          .filter((t) => t.lifecycle === life)
          .sort((a, b) => a.position - b.position)
          .forEach((t, i) => {
            t.position = i;
          });
      }
    },
  };

  window.__mock = mock;

  const now = () => Date.now();

  /** A profile name resolves to its CLI; anything else is a binary name —
      the core's own semantics. */
  const resolveAgent = (name: string) =>
    mock.profiles.find((p) => p.name === name)?.agent ?? name;

  /** The CLIs whose conventions the core knows — the prompt is sent for
      them, a follow-up can go in through the terminal, and a permission
      mode means something on their command line. Mirrors
      `src-tauri/src/agent.rs`; spelled out here rather than imported
      because this script is serialised into the page. */
  const measured = (agent: string) => agent === 'claude' || agent === 'codex';

  const makeSession = (cwd: string, agent: string): MockSession => {
    // A counter, not the array length: parking removes rows, and a freed
    // id must never be reissued to a different terminal.
    mock.sessionSeq += 1;
    const id = `s${mock.sessionSeq}`;
    // The core counts a repeated directory name up rather than handing out
    // the same row name twice — see Core::unique_title. Without it here the
    // frontend would be tested against a list the app never produces.
    const base = cwd.split('/').filter(Boolean).pop() ?? cwd;
    const taken = new Set(mock.sessions.map((s) => s.title));
    let title = base;
    for (let n = 2; taken.has(title); n += 1) title = `${base} ${n}`;
    return {
      id,
      cwd,
      title,
      agent,
      status: 'starting',
      created_at: now(),
      last_active_at: now(),
      live: true,
      reports_status: false,
      // Mirrors the real gate at core.rs launch(): a session is wired only
      // when this desk knows the CLI's conventions and the version in that
      // world is new enough. `unwiredAgents` is how a test plays the second
      // half — a codex too old for its own hooks engine.
      hooks_wired:
        ['claude', 'codex'].includes(agent) && !mock.unwiredAgents.includes(agent),
      preview_port: null,
      activity: null,
      activity_since: 0,
      completed: false,
      attempt_id: null,
    };
  };

  const handlers: Record<string, (args: Record<string, unknown>) => unknown> = {
    boot_status: () => ({
      ready: true,
      shell: '/bin/zsh',
      envResolved: true,
      envSource: 'login',
      envVarCount: 42,
      path: '/usr/local/bin:/usr/bin:/bin',
      claude: '/usr/local/bin/claude',
      claudeVersion: '2.1.226',
      codex: null,
      codexVersion: null,
      // The detection report the first-run panel renders: claude found,
      // the rest absent — the commonest real machine. Seedable per test
      // (__mockAgents),讀在呼叫當下 —— 「重新偵測」按下去重跑的就是
      // 這一份,測試改了種子,重跑就看得到新發現。
      agents: (JSON.parse(
        sessionStorage.getItem('__mockAgents') ?? 'null',
      ) as Array<{ name: string; path: string | null }> | null) ?? [
        { name: 'claude', path: '/usr/local/bin/claude', version: '2.1.226', reports: true },
        { name: 'codex', path: null, version: null, reports: false },
        { name: 'gemini', path: null },
        { name: 'aider', path: null },
      ],
      messaging: true,
      db: '/tmp/marol.db',
      hookUrl: 'http://127.0.0.1:1/h/tok',
      promptTemplate: '/tmp/marol/prompt-template.md',
    }),

    list_sessions: () => mock.sorted(),

    list_tabs: () => mock.tabs,

    create_tab: (args) => {
      const id = `t${mock.tabs.length + 1}`;
      mock.tabs.push({
        id,
        name: String(args.name),
        layout: '{"mode":"auto","cols":"auto"}',
        slots: [],
        position: mock.tabs.length,
      });
      mock.persist();
      queueMicrotask(() => mock.emit('tabs:changed', mock.tabs));
      return id;
    },

    rename_tab: (args) => {
      const t = mock.tabs.find((x) => x.id === args.id);
      if (t) t.name = String(args.name);
      mock.persist();
      queueMicrotask(() => mock.emit('tabs:changed', mock.tabs));
      return null;
    },

    close_tab: (args) => {
      if (mock.tabs.length <= 1) throw new Error('the last tab cannot be closed');
      mock.tabs = mock.tabs.filter((x) => x.id !== args.id);
      mock.persist();
      queueMicrotask(() => mock.emit('tabs:changed', mock.tabs));
      return null;
    },

    update_tab: (args) => {
      const slots = args.slots as Array<string | null>;
      // The core enforces one-session-per-tab; the mock must too, or the
      // frontend would be tested against rules the real app does not have.
      const claimed = new Set(slots.filter((s): s is string => s !== null));
      for (const t of mock.tabs) {
        if (t.id === args.id) {
          t.layout = String(args.layout);
          t.slots = slots;
        } else {
          // The core closes the gap rather than blanking a position, because
          // a blank one cannot be told apart from one emptied on purpose.
          t.slots = t.slots.filter((s) => s === null || !claimed.has(s));
        }
      }
      mock.persist();
      queueMicrotask(() => mock.emit('tabs:changed', mock.tabs));
      return null;
    },

    new_session: (args) => {
      const s = makeSession(String(args.cwd), resolveAgent(String(args.agent ?? 'claude')));
      mock.sessions.push(s);
      if (!mock.snapshots.has(s.id)) mock.snapshots.set(s.id, { data: '', seq: 0 });
      // The real core broadcasts the new list; the pane mounts on the render
      // that follows.
      mock.pushSessions();
      return s.id;
    },

    reopen_session: (args) => {
      const s = mock.sessions.find((x) => x.id === args.id);
      if (s) {
        // Attaching is not starting, and the mock has to agree with the core
        // about that: `new-session -A -D` reattaches to a held agent and drops
        // the argv, so no SessionStart fires and 啟動中 would never correct
        // itself. See Core::reopen_session.
        s.status = s.status === 'detached' ? 'detached' : 'starting';
        s.live = true;
      }
      mock.pushSessions();
      return null;
    },

    close_session: (args) => {
      const s = mock.sessions.find((x) => x.id === args.id);
      if (s) {
        s.live = false;
        s.status = 'saved';
      }
      mock.pushSessions();
      // An attempt's terminal ending is the commonest way a slot frees.
      if (s?.attempt_id) {
        mock.drainQueue();
        mock.pushTasks();
      }
      return null;
    },

    archive_session: (args) => {
      mock.sessions = mock.sessions.filter((x) => x.id !== args.id);
      mock.pushSessions();
      return null;
    },

    set_completed: (args) => {
      const s = mock.sessions.find((x) => x.id === args.id);
      if (s) s.completed = Boolean(args.completed);
      mock.pushSessions();
      return null;
    },

    // The core cleans a name to one line and refuses an empty one; the mock
    // must too, or the frontend is tested against rules the app does not have.
    rename_session: (args) => {
      const title = String(args.title).split(/\s+/).filter(Boolean).join(' ');
      if (!title) throw new Error("a session's name cannot be empty");
      const s = mock.sessions.find((x) => x.id === args.id);
      if (s) s.title = title.slice(0, 80);
      mock.pushSessions();
      return null;
    },

    set_locale: () => null,

    term_snapshot: (args) => mock.snapshots.get(String(args.id)) ?? { data: '', seq: 0 },
    term_write: () => null,
    term_resize: () => null,

    /* ---------------------------- board ---------------------------- */

    list_tasks: () => mock.tasks,

    create_task: (args) => {
      const repo = String(args.repoPath);
      const branch = String(args.baseBranch);
      const extra = (args.extraRepos ?? []) as Array<{
        repo_path: string;
        base_branch: string;
      }>;
      // The core checks every repository when the card is made, not when it
      // is first run, so a card that could never produce an attempt cannot be
      // created. The mock refuses the same things for the same reasons: one
      // that was more generous than the product would turn a real refusal
      // into a test that never sees it.
      const world = (p: string) => /^(wsl|ssh):\/\/[^/]+/.exec(p)?.[0] ?? '';
      const seen = new Set<string>();
      for (const r of [{ repo_path: repo, base_branch: branch }, ...extra]) {
        if (world(r.repo_path) !== world(repo)) {
          throw new Error(
            `同一張卡的 repo 必須在同一台主機：${repo} 和 ${r.repo_path} 不是。`,
          );
        }
        if (seen.has(r.repo_path)) throw new Error(`${r.repo_path} 在這張卡上出現了兩次`);
        seen.add(r.repo_path);
        const branches = mock.repos[r.repo_path];
        if (!branches) throw new Error(`${r.repo_path} is not a git repository`);
        if (!branches.includes(r.base_branch)) {
          throw new Error(`${r.repo_path} has no branch \`${r.base_branch}\``);
        }
      }

      const id = `k${mock.tasks.length + 1}`;
      mock.tasks.push({
        id,
        title: String(args.title),
        prompt: String(args.prompt),
        repo_path: repo,
        base_branch: branch,
        extra_repos: extra,
        lifecycle: 'backlog',
        position: mock.tasks.filter((t) => t.lifecycle === 'backlog').length,
        created_at: now(),
        attempts: [],
        queued_at: null,
      });
      mock.pushTasks();
      return id;
    },

    move_task: (args) => {
      const t = mock.tasks.find((x) => x.id === args.id);
      if (!t) throw new Error(`no such task: ${String(args.id)}`);
      const to = String(args.lifecycle);
      const at = Number(args.position);
      const column = mock.tasks
        .filter((x) => x.lifecycle === to && x.id !== t.id)
        .sort((a, b) => a.position - b.position);
      t.lifecycle = to;
      // Insert at `at`, then renumber both columns from scratch — exactly what
      // the core does, because a position only means anything relative to its
      // neighbours.
      column.splice(Math.max(0, Math.min(at, column.length)), 0, t);
      column.forEach((x, i) => {
        x.position = i;
      });
      mock.renumber();
      mock.pushTasks();
      return null;
    },

    delete_task: (args) => {
      const t = mock.tasks.find((x) => x.id === args.id);
      // Attempts still holding a worktree give it back with the card.
      const ids = new Set((t?.attempts ?? []).map((a) => a.session_id));
      mock.sessions = mock.sessions.filter((s) => !ids.has(s.id));
      mock.tasks = mock.tasks.filter((x) => x.id !== args.id);
      mock.renumber();
      mock.pushSessions();
      mock.pushTasks();
      return null;
    },

    preview_prompt: (args) => {
      const t = mock.tasks.find((x) => x.id === args.taskId);
      const seq = (t?.attempts.length ?? 0) + 1;
      return {
        prompt:
          `[Marol 任務] ${t?.title ?? ''}\n\n` +
          `你在一個專為這張卡開的 git worktree：分支 marol/card-${seq}，` +
          `從 ${t?.base_branch ?? 'main'} @ abcd1234 開出。\n\n---\n\n${t?.prompt ?? ''}\n`,
        // Only the CLIs in the conventions table are sent a prompt. A
        // profile resolves to the CLI underneath before the question is
        // asked.
        willSend: measured(resolveAgent(String(args.agent))),
      };
    },

    open_attempt: (args) => {
      const taskId = String(args.taskId);
      const t = mock.tasks.find((x) => x.id === taskId);
      if (!t) throw new Error(`no such task: ${taskId}`);
      const agent = String(args.agent ?? 'claude');
      const prompt = String(args.prompt ?? '');
      const mode = String(args.mode ?? 'normal');
      // Over the limit it waits its turn rather than being refused.
      if (mock.running() >= mock.maxConcurrent) {
        if (!mock.queue.includes(taskId)) mock.queue.push(taskId);
        mock.pendingStarts.set(taskId, { agent, prompt, mode });
        mock.pushTasks();
        return { attempt: null, queuedAt: mock.queue.indexOf(taskId) + 1 };
      }
      return { attempt: startAttempt(taskId, agent, prompt, mode), queuedAt: null };
    },

    cancel_queued: (args) => {
      const taskId = String(args.taskId);
      mock.queue = mock.queue.filter((x) => x !== taskId);
      mock.pendingStarts.delete(taskId);
      mock.pushTasks();
      return null;
    },

    /* -------------------------- folder picker ------------------------- */

    list_dir: (args) => {
      const world = String(args.world ?? '');
      // A tiny filesystem per world, so a test can prove the picker is
      // browsing the machine the card runs on rather than this one. The
      // shapes differ on purpose: /home/you exists in both, and only the
      // contents say which you are looking at.
      const tree = mock.dirs[world];
      if (!tree) throw new Error(`no such world: ${world}`);
      const path = args.path == null || String(args.path).trim() === ''
        ? mock.dirHome[world]
        : String(args.path);
      const dirs = tree[path];
      if (!dirs) throw new Error(`${path} cannot be opened`);
      const cut = path.replace(/\/+$/, '').lastIndexOf('/');
      return {
        path,
        parent: path === '/' ? null : cut <= 0 ? '/' : path.slice(0, cut),
        dirs,
        is_repo: mock.dirRepos.includes(path),
      };
    },

    /* ---------------------------- updates ---------------------------- */

    update_status: () => ({
      version: mock.update.version,
      configured: mock.update.configured,
      enabled: mock.update.enabled,
      selfContained: mock.update.selfContained,
      held: mock.update.held,
      lost: mock.update.lost,
      lastCheck: mock.update.lastCheck,
      due: mock.update.due,
      releases: mock.update.releases,
    }),

    update_check: () => {
      if (!mock.update.configured || !mock.update.enabled) return null;
      mock.update.lastCheck = Math.floor(Date.now() / 1000);
      return mock.update.available;
    },

    update_apply: (args) => {
      if (mock.update.applyError) throw new Error(mock.update.applyError);
      mock.update.applied.push(Boolean(args.acknowledged));
      return null;
    },

    set_update_enabled: (args) => {
      mock.update.enabled = Boolean(args.on);
      return null;
    },

    concurrency: () => ({
      max: mock.maxConcurrent,
      running: mock.running(),
      queued: mock.queue.length,
    }),

    set_concurrency: (args) => {
      mock.maxConcurrent = Math.max(1, Number(args.max));
      // Raising the limit is a way of saying "go now".
      mock.drainQueue();
      mock.pushTasks();
      return null;
    },

    merge_attempt: (args) => {
      const attempt = mock.tasks
        .flatMap((x) => x.attempts)
        .find((a) => a.id === args.attemptId);
      if (!attempt) throw new Error(`no such attempt: ${String(args.attemptId)}`);
      // The core refuses rather than producing a merge without the work in it.
      if (mock.dirtyWorktrees.has(attempt.id)) {
        throw new Error(`${attempt.branch} 有未提交的變更，合併不會包含。`);
      }
      finishAttempt(attempt.id, 'merged');
      return 'deadbeefcafe';
    },

    open_pr: (args) => {
      const attempt = mock.tasks
        .flatMap((x) => x.attempts)
        .find((a) => a.id === args.attemptId);
      if (!attempt) throw new Error(`no such attempt: ${String(args.attemptId)}`);
      if (mock.dirtyWorktrees.has(attempt.id)) {
        throw new Error(`${attempt.branch} 有未提交的變更，推送不會包含。`);
      }
      // One per repository the card spans, newline-separated in the order
      // the card names them — a pull request belongs to a repository, so a
      // card spanning two produces two. The attempt deliberately stays open:
      // review is when there is still something to change.
      const task = mock.tasks.find((x) => x.attempts.some((a) => a.id === attempt.id))!;
      return [
        `https://github.com/test/repo/pull/${attempt.seq}`,
        ...(task.extra_repos ?? []).map((r) => {
          const name = r.repo_path.split('/').filter(Boolean).slice(-1)[0];
          return `https://github.com/test/${name}/pull/${attempt.seq}`;
        }),
      ].join('\n');
    },
  };

  /** Open an attempt now. Shared by the button and the queue, as in the core.
      The launcher name resolves here — a queued start carries the name. */
  function startAttempt(taskId: string, launcher: string, prompt: string, mode = 'normal') {
      const agent = resolveAgent(launcher);
      const t = mock.tasks.find((x) => x.id === taskId)!;
      const seq = t.attempts.length + 1;
      const attemptId = `${t.id}-a${seq}`;
      // The worktree lives in the repo's own world, as the core insists:
      // an ssh:// repo gets an ssh:// worktree, and everything keying off
      // the path's world (preview reachability) sees the truth.
      const world = t.repo_path.match(/^(wsl|ssh):\/\/[^/]+/)?.[0] ?? '';
      const session = makeSession(`${world}/Users/test/worktrees/card-${seq}`, agent);
      session.title = `${t.title} #${seq}`;
      session.attempt_id = attemptId;
      // A brand-new worktree always opens on the folder-trust prompt, and no
      // hook reports it — the core sets this directly.
      session.status = measured(agent) ? 'awaiting_trust' : 'starting';
      mock.sessions.push(session);
      mock.snapshots.set(session.id, { data: '', seq: 0 });

      t.attempts.push({
        id: attemptId,
        task_id: t.id,
        seq,
        agent,
        worktree_path: session.cwd,
        branch: `marol/card-${seq}`,
        base_sha: 'abcd1234deadbeef',
        mode,
        outcome: null,
        frozen_diff: null,
        created_at: now(),
        parked_at: null,
        session_id: session.id,
      });
      t.lifecycle = 'running';
      // The core writes the prompt as sent onto the timeline.
      mock.record(attemptId, 'prompt', null, prompt);
      mock.renumber();
      mock.pushSessions();
      mock.pushTasks();
      return {
        attempt_id: attemptId,
        session_id: session.id,
        branch: `marol/card-${seq}`,
        worktree_path: session.cwd,
        prompt,
        prompt_sent: measured(agent),
      };
  }

  function finishAttempt(attemptId: string, outcome: string) {
    const attempt = mock.tasks.flatMap((x) => x.attempts).find((a) => a.id === attemptId);
    if (!attempt) return;
    attempt.outcome = outcome;
    attempt.frozen_diff =
      mock.diffs.get(attemptId) ?? 'diff --git a/app.txt b/app.txt\n+fixed\n';
    // The worktree goes, and the session with it — which frees a slot.
    mock.sessions = mock.sessions.filter((s) => s.id !== attempt.session_id);
    attempt.session_id = null;
    mock.drainQueue();
    mock.pushSessions();
    mock.pushTasks();
  }

  const rest: Record<string, (args: Record<string, unknown>) => unknown> = {

    reopen_attempt: (args) => {
      const attempt = mock.tasks
        .flatMap((t) => t.attempts)
        .find((a) => a.id === args.attemptId);
      if (!attempt) throw new Error(`no such attempt: ${String(args.attemptId)}`);
      if (attempt.outcome !== null) throw new Error('attempt is finished');
      const s = mock.sessions.find((x) => x.id === attempt.session_id);
      if (s) {
        s.live = true;
        s.status = 'starting';
      }
      mock.pushSessions();
      mock.pushTasks();
      return attempt.session_id;
    },

    park_attempt: (args) => {
      const attempt = mock.tasks
        .flatMap((t) => t.attempts)
        .find((a) => a.id === args.attemptId);
      if (!attempt) throw new Error(`no such attempt: ${String(args.attemptId)}`);
      if (attempt.outcome !== null) throw new Error('this attempt is finished');
      if (attempt.parked_at !== null) throw new Error('this attempt is already parked');
      const session = mock.sessions.find((s) => s.attempt_id === attempt.id);
      if (session && session.live && !['idle', 'saved', 'exited'].includes(session.status)) {
        throw new Error('the agent is mid-turn in this worktree');
      }
      // The shelf checkpoint the core always keeps first.
      const list = mock.checkpoints.get(attempt.id) ?? [];
      list.push({
        n: (list[list.length - 1]?.n ?? 0) + 1,
        sha: `shelf${list.length + 1}00`,
        at: Math.floor(Date.now() / 1000),
      });
      mock.checkpoints.set(attempt.id, list);
      // Sessions living in the worktree go with it — shell included.
      mock.sessions = mock.sessions.filter(
        (s) => s.attempt_id !== attempt.id && !s.cwd.startsWith(attempt.worktree_path),
      );
      attempt.parked_at = Date.now();
      attempt.session_id = null;
      mock.drainQueue();
      mock.pushSessions();
      mock.pushTasks();
      return attempt.branch;
    },

    resume_attempt: (args) => {
      const attempt = mock.tasks
        .flatMap((t) => t.attempts)
        .find((a) => a.id === args.attemptId);
      if (!attempt) throw new Error(`no such attempt: ${String(args.attemptId)}`);
      if (attempt.parked_at === null) throw new Error('this attempt is not parked');
      attempt.parked_at = null;
      const session = makeSession(attempt.worktree_path, attempt.agent);
      session.attempt_id = attempt.id;
      attempt.session_id = session.id;
      mock.sessions.push(session);
      if (!mock.snapshots.has(session.id)) mock.snapshots.set(session.id, { data: '', seq: 0 });
      mock.pushSessions();
      mock.pushTasks();
      return { session_id: session.id, restore_error: mock.resumeRestoreError };
    },

    finish_attempt: (args) => {
      finishAttempt(String(args.attemptId), String(args.outcome));
      return null;
    },

    attempt_diff: (args) => {
      const attempt = mock.tasks
        .flatMap((t) => t.attempts)
        .find((a) => a.id === args.attemptId);
      // A finished attempt reads the copy frozen before its worktree went.
      if (attempt?.frozen_diff) return attempt.frozen_diff;
      // Compared against a checkpoint: tests seed `<id>@<n>` entries.
      if (args.n != null && Number(args.n) !== 0) {
        return mock.diffs.get(`${String(args.attemptId)}@${Number(args.n)}`) ?? '';
      }
      return mock.diffs.get(String(args.attemptId)) ?? '';
    },

    attempt_stats: (args) => {
      const attempt = mock.tasks
        .flatMap((t) => t.attempts)
        .find((a) => a.id === args.attemptId);
      // The core refuses a finished attempt — no worktree, nothing to measure.
      if (!attempt) throw new Error(`no such attempt: ${String(args.attemptId)}`);
      if (attempt.outcome !== null) throw new Error('attempt is finished');
      return (
        mock.stats.get(String(args.attemptId)) ?? {
          files: 0,
          adds: 0,
          dels: 0,
          ahead: 0,
          behind: 0,
        }
      );
    },

    attempt_events: (args) => mock.events.get(String(args.attemptId)) ?? [],

    list_launchers: () =>
      [
        ...['claude', 'codex', 'gemini', 'aider'].map((a) => ({
          name: a,
          agent: a,
          profile: false,
        })),
        ...mock.profiles.map((p) => ({ name: p.name, agent: p.agent, profile: true })),
      ],

    list_profiles: () => mock.profiles,

    save_profiles: (args) => {
      const profiles = args.profiles as Array<{ name: string; agent: string; args: string[] }>;
      // The core's validation, mirrored: every name says something, no two
      // say the same thing, none shadows an agent's own name.
      const seen = new Set<string>();
      for (const p of profiles) {
        if (p.name.trim() === '') throw new Error('a profile needs a name');
        if (['claude', 'codex', 'gemini', 'aider'].includes(p.name.trim())) {
          throw new Error(`\`${p.name}\` is an agent's own name; a profile may not shadow it`);
        }
        if (seen.has(p.name.trim())) throw new Error(`two profiles are both called \`${p.name}\``);
        seen.add(p.name.trim());
      }
      mock.profiles = profiles;
      return null;
    },

    notify_prefs: () => mock.notifyPrefs,

    set_notify_prefs: (args) => {
      mock.notifyPrefs = args.prefs as { permission: boolean; input: boolean; done: boolean };
      return null;
    },

    test_notification: () => null,

    probe_port: () => mock.portListening,

    list_worlds: () => mock.worlds,

    probe_world: (args) => {
      const world = String(args.world ?? '');
      return (
        mock.worldProbes.get(world) ?? { claude: '2.1.226', codex: null, error: null }
      );
    },

    checkpoints_enabled: () => mock.checkpointsOn,

    agent_updates_enabled: () => mock.agentUpdatesOn,

    set_agent_updates_enabled: (args) => {
      mock.agentUpdatesOn = Boolean(args.on);
      return null;
    },

    set_checkpoints_enabled: (args) => {
      mock.checkpointsOn = Boolean(args.on);
      return null;
    },

    checkpoint_now: (args) => {
      if (mock.checkpointQuiet) return null;
      const id = String(args.attemptId);
      const list = mock.checkpoints.get(id) ?? [];
      const cp = {
        n: (list[list.length - 1]?.n ?? 0) + 1,
        sha: `cafe${list.length + 1}00`,
        at: Math.floor(Date.now() / 1000),
      };
      list.push(cp);
      mock.checkpoints.set(id, list);
      return cp;
    },

    list_checkpoints: (args) => mock.checkpoints.get(String(args.attemptId)) ?? [],

    restore_checkpoint: (args) => {
      const id = String(args.attemptId);
      const n = Number(args.n);
      // The core's refusal, mirrored: a turn in flight blocks the restore.
      const session = mock.sessions.find((s) => s.attempt_id === id);
      if (session && session.live && !['idle', 'saved', 'exited'].includes(session.status)) {
        throw new Error('the agent is mid-turn in this worktree');
      }
      const list = mock.checkpoints.get(id) ?? [];
      const target = n === 0 ? { sha: 'abcd1234deadbeef' } : list.find((c) => c.n === n);
      if (!target) throw new Error(`this attempt has no checkpoint #${n}`);
      // The automatic pre-restore snapshot the core always keeps first.
      const saved = {
        n: (list[list.length - 1]?.n ?? 0) + 1,
        sha: `feed${list.length + 1}00`,
        at: Math.floor(Date.now() / 1000),
      };
      list.push(saved);
      mock.checkpoints.set(id, list);
      return { to_n: n, to_sha: target.sha, saved };
    },

    attempt_file: (args) => {
      const attempt = mock.tasks
        .flatMap((t) => t.attempts)
        .find((a) => a.id === args.attemptId);
      if (!attempt) throw new Error(`no such attempt: ${String(args.attemptId)}`);
      // The core's refusals, mirrored: a record is not a document.
      if (attempt.outcome !== null) {
        throw new Error('this attempt is finished — its frozen diff is a record, not a document');
      }
      if (attempt.parked_at !== null) {
        throw new Error('this attempt is parked — there is no worktree to read');
      }
      return (
        mock.files.get(`${String(args.attemptId)}:${String(args.path)}`) ?? {
          base: null,
          work: null,
        }
      );
    },

    write_attempt_file: (args) => {
      const attemptId = String(args.attemptId);
      const path = String(args.path);
      const contents = String(args.contents);
      const attempt = mock.tasks
        .flatMap((t) => t.attempts)
        .find((a) => a.id === attemptId);
      if (!attempt) throw new Error(`no such attempt: ${attemptId}`);
      if (attempt.outcome !== null) {
        throw new Error('this attempt is finished — its frozen diff is a record, not a document');
      }
      if (attempt.parked_at !== null) {
        throw new Error('this attempt is parked — resume it first, then edit');
      }
      // The settled guard, verbatim in spirit: the UI hiding its button is
      // not the guard, this refusal is.
      const session = mock.sessions.find((s) => s.attempt_id === attemptId);
      if (session && session.live && !['idle', 'saved', 'exited'].includes(session.status)) {
        throw new Error(
          'the agent is mid-turn in this worktree. Saving now would change files under ' +
            'its feet while it is still writing its own. Wait for the turn to end — or ' +
            'close the session — and save then',
        );
      }
      const key = `${attemptId}:${path}`;
      const entry = mock.files.get(key) ?? { base: null, work: null };
      // The freshness contract, mirrored: a disk that moved since the
      // editor read it refuses the save.
      if (args.expected != null && (entry.work ?? '') !== String(args.expected)) {
        throw new Error(
          `${path} changed on disk after the editor read it — a shell, a script, or ` +
            'another turn wrote here. Close the editor and reopen it to see the current ' +
            'text; saving now would overwrite that work unseen',
        );
      }
      entry.work = contents;
      mock.files.set(key, entry);
      // The worktree changed, so the next diff read must say so: this
      // file's section is re-rendered from its two sides, the rest of the
      // seeded diff stays.
      const section =
        `diff --git a/${path} b/${path}\n--- a/${path}\n+++ b/${path}\n@@ -1 +1 @@\n` +
        (entry.base ?? '').split('\n').filter((l) => l !== '').map((l) => `-${l}\n`).join('') +
        contents.split('\n').filter((l) => l !== '').map((l) => `+${l}\n`).join('');
      const current = mock.diffs.get(attemptId) ?? '';
      const parts = current.split(/^(?=diff --git )/m).filter((p) => p !== '');
      const at = parts.findIndex((p) => p.startsWith(`diff --git a/${path} `));
      if (at >= 0) parts[at] = section;
      else parts.push(section);
      mock.diffs.set(attemptId, parts.join(''));
      return null;
    },

    list_run_scripts: () => mock.runScripts,

    run_script: (args) => {
      const attempt = mock.tasks
        .flatMap((t) => t.attempts)
        .find((a) => a.id === args.attemptId);
      if (!attempt) throw new Error(`no such attempt: ${String(args.attemptId)}`);
      if (attempt.outcome !== null) throw new Error('attempt is finished');
      const name = String(args.name);
      if (!mock.runScripts.includes(name)) throw new Error(`no run script named \`${name}\``);
      // An ad-hoc session in the attempt's worktree, exactly as the core
      // makes it: no card, no slot.
      const s = makeSession(attempt.worktree_path, 'sh');
      s.title = `▶ ${name}`;
      s.preview_port = attempt.worktree_path.startsWith('ssh://') ? null : 4173;
      mock.sessions.push(s);
      mock.snapshots.set(s.id, { data: '', seq: 0 });
      mock.pushSessions();
      return s.id;
    },

    queue_followup: (args) => {
      const s = mock.sessions.find((x) => x.id === args.id);
      if (!s) throw new Error(`no such session: ${String(args.id)}`);
      if (!s.live) throw new Error(`no terminal for session ${s.id}`);
      if (!measured(s.agent)) {
        throw new Error(`\`${s.agent}\`'s input conventions have not been measured`);
      }
      mock.queuedFollowups.set(s.id, String(args.text));
      s.has_followup = true;
      mock.pushSessions();
      return null;
    },

    cancel_followup: (args) => {
      const s = mock.sessions.find((x) => x.id === args.id);
      mock.queuedFollowups.delete(String(args.id));
      if (s) s.has_followup = false;
      mock.pushSessions();
      return null;
    },

    list_branches: (args) => {
      const branches = mock.repos[String(args.repoPath)];
      if (!branches) throw new Error(`${String(args.repoPath)} is not a git repository`);
      return branches;
    },

    open_shell: (args) => {
      const attemptId = String(args.attemptId);
      const attempt = mock.tasks
        .flatMap((t) => t.attempts)
        .find((a) => a.id === attemptId);
      if (!attempt) throw new Error(`no such attempt: ${attemptId}`);
      if (attempt.outcome !== null) throw new Error('attempt is finished');
      // One shell per attempt: while it lives, the button returns it.
      const existing = mock.shells.get(attemptId);
      if (existing && mock.sessions.some((s) => s.id === existing && s.live)) {
        return existing;
      }
      const task = mock.tasks.find((t) => t.id === attempt.task_id)!;
      const s = makeSession(attempt.worktree_path, 'zsh');
      s.title = `$ ${task.title} #${attempt.seq}`;
      mock.sessions.push(s);
      mock.snapshots.set(s.id, { data: '', seq: 0 });
      mock.shells.set(attemptId, s.id);
      mock.pushSessions();
      return s.id;
    },

    send_followup: (args) => {
      const s = mock.sessions.find((x) => x.id === args.id);
      if (!s) throw new Error(`no such session: ${String(args.id)}`);
      // The core only sends into CLIs whose input conventions are measured,
      // and only through a live terminal — the mock must refuse the same way.
      if (!measured(s.agent)) {
        throw new Error(`\`${s.agent}\`'s input conventions have not been measured`);
      }
      if (!s.live) throw new Error(`no terminal for session ${s.id}`);
      if (s.attempt_id) mock.record(s.attempt_id, 'prompt', null, String(args.text));
      return null;
    },

    'plugin:event|listen': (args) => {
      const event = String(args.event);
      const handler = Number(args.handler);
      const ids = mock.listeners.get(event) ?? [];
      ids.push(handler);
      mock.listeners.set(event, ids);
      return handler;
    },
    'plugin:event|unlisten': () => null,
    'plugin:opener|open_url': () => null,
    'plugin:opener|open_path': () => null,

    /** Slots and discoveries together, the way the core returns them: two
     *  rules files present, one absent, plus a skill read off disk. `dir` is
     *  empty throughout — one checkout, which is what every fixture here has;
     *  `mock.knowsDir` makes the project rows wear a checkout instead, for
     *  the card that spans two. */
    agent_docs: () =>
      [
        { scope: 'project', agent: 'claude', kind: 'rules', name: 'CLAUDE.md', path: '/wt/CLAUDE.md', exists: true },
        { scope: 'project', agent: 'shared', kind: 'rules', name: 'AGENTS.md', path: '/wt/AGENTS.md', exists: false },
        { scope: 'project', agent: 'gemini', kind: 'rules', name: 'GEMINI.md', path: '/wt/GEMINI.md', exists: false },
        { scope: 'project', agent: 'claude', kind: 'skill', name: 'release', path: '/wt/.claude/skills/release/SKILL.md', exists: true },
        { scope: 'global', agent: 'claude', kind: 'rules', name: 'CLAUDE.md', path: '/home/me/.claude/CLAUDE.md', exists: true },
        { scope: 'global', agent: 'codex', kind: 'rules', name: 'AGENTS.md', path: '/home/me/.codex/AGENTS.md', exists: false },
      ].flatMap((d) =>
        d.scope === 'project' && mock.knowsDirs.length > 0
          ? mock.knowsDirs.map((dir) => ({
              ...d,
              dir,
              path: `/wt/${dir}/${d.path.slice('/wt/'.length)}`,
            }))
          : [{ ...d, dir: '' }],
      ),
    'plugin:notification|is_permission_granted': () => true,
    'plugin:notification|notify': () => null,
  };
  Object.assign(handlers, rest);

  // Tauri's unlisten path goes through this, not through invoke. Without it
  // every effect cleanup throws and the noise buries real failures.
  (window as unknown as Record<string, unknown>).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: (event: string, id: number) => {
      const ids = (mock.listeners.get(event) ?? []).filter((x) => x !== id);
      mock.listeners.set(event, ids);
      return Promise.resolve();
    },
  };

  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke: (cmd: string, args: Record<string, unknown> = {}) => {
      mock.calls.push({ cmd, args });
      const fn = handlers[cmd];
      if (!fn) return Promise.reject(new Error(`unmocked command: ${cmd}`));
      return Promise.resolve(fn(args));
    },
    transformCallback: (cb: unknown) => {
      const id = ++mock.cbSeq;
      (window as unknown as Record<string, unknown>)[`_${id}`] = cb;
      return id;
    },
    metadata: {
      currentWindow: { label: 'main' },
      currentWebview: { label: 'main', windowLabel: 'main' },
    },
  };
}
