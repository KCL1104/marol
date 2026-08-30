/**
 * The world the README is photographed and filmed in.
 *
 * Shared by `shots.spec.ts` (stills) and `clips.spec.ts` (per-feature
 * recordings) so the two never drift into showing different products. The
 * frames are the actual React tree, the actual stylesheet, and xterm
 * rendering a real captured Claude Code TUI — only the backend is the same
 * mock every test trusts. Staged data, true pixels.
 */
import { expect, type Browser, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { installMock } from './mock-tauri';

const here = dirname(fileURLToPath(import.meta.url));

export const MEDIA = join(here, '..', '..', 'docs', 'media');

export const tui = JSON.parse(
  readFileSync(join(here, 'fixtures/claude-tui.json'), 'utf8'),
) as { chunks: string[] };

/** A second terminal's worth of honest-looking output: a vitest run with
    ANSI color, so the wall does not show the same frame twice. */
export const TEST_LOG = [
  '\x1b[36m$ npx vitest run --reporter=verbose\x1b[0m',
  '',
  '\x1b[32m✓\x1b[0m src/api/limiter.test.ts \x1b[90m(9 tests)\x1b[0m \x1b[33m412ms\x1b[0m',
  '\x1b[32m✓\x1b[0m src/api/session.test.ts \x1b[90m(14 tests)\x1b[0m \x1b[33m230ms\x1b[0m',
  '\x1b[32m✓\x1b[0m src/auth/tokens.test.ts \x1b[90m(6 tests)\x1b[0m \x1b[33m88ms\x1b[0m',
  '',
  '\x1b[1mTest Files\x1b[0m  \x1b[32m3 passed\x1b[0m \x1b[90m(3)\x1b[0m',
  '\x1b[1m     Tests\x1b[0m  \x1b[32m29 passed\x1b[0m \x1b[90m(29)\x1b[0m',
  '\x1b[90m  Duration\x1b[0m  1.42s',
  '',
].join('\r\n');

export const DIFF = [
  'diff --git a/src/auth/session.py b/src/auth/session.py',
  'index 3f1c2aa..9d04b71 100644',
  '--- a/src/auth/session.py',
  '+++ b/src/auth/session.py',
  '@@ -12,9 +12,11 @@ def login(request):',
  '     user = authenticate(request)',
  '     if user is None:',
  '-        return redirect("/login")  # loops when the session cookie is stale',
  '+        clear_session_cookie(request)',
  '+        return redirect("/login?expired=1")',
  '     session = Session.create(user)',
  '-    session.ttl = 3600',
  '+    # TTL follows the "remember me" checkbox, not a constant',
  '+    session.ttl = 30 * 86400 if request.POST.get("remember") else 3600',
  '     return respond(request, session)',
  'diff --git a/src/auth/cookies.py b/src/auth/cookies.py',
  'new file mode 100644',
  'index 0000000..b7ad433',
  '--- /dev/null',
  '+++ b/src/auth/cookies.py',
  '@@ -0,0 +1,7 @@',
  '+"""Session-cookie helpers shared by login and logout."""',
  '+',
  '+def clear_session_cookie(request):',
  '+    """Expire the cookie the stale-redirect loop was feeding on."""',
  '+    request.cookies.pop("sid", None)',
  '+    request.response.delete_cookie("sid")',
  '+',
].join('\n');

export const SESSION_BASE = [
  'def login(request):',
  '    user = authenticate(request)',
  '    if user is None:',
  '        return redirect("/login")  # loops when the session cookie is stale',
  '    session = Session.create(user)',
  '    session.ttl = 3600',
  '    return respond(request, session)',
  '',
].join('\n');

export const SESSION_WORK = [
  'def login(request):',
  '    user = authenticate(request)',
  '    if user is None:',
  '        clear_session_cookie(request)',
  '        return redirect("/login?expired=1")',
  '    session = Session.create(user)',
  '    # TTL follows the "remember me" checkbox, not a constant',
  '    session.ttl = 30 * 86400 if request.POST.get("remember") else 3600',
  '    return respond(request, session)',
  '',
].join('\n');

export type MediaLocale = 'en' | 'zh-TW';

/** Card titles, per locale — the one thing screenshots must localize. */
export const TITLES: Record<MediaLocale, string[]> = {
  en: [
    'Fix the login redirect loop',
    'Rate-limit the public API',
    'Migrate settings to SQLite',
    'Dark theme for the editor',
    'Spike: import from Linear',
    'Polish the onboarding empty state',
    'Workspace',
  ],
  'zh-TW': [
    '修好登入轉圈圈',
    '公開 API 加上限流',
    '設定搬進 SQLite',
    '編輯器深色主題',
    '試作：從 Linear 匯入',
    '打磨初次上手的空狀態',
    '工作區',
  ],
};

/** The lines typed on camera, per locale. Stills never type; clips do. */
export const LINES: Record<MediaLocale, Record<string, string>> = {
  en: {
    review: 'Clear the cookie before redirecting.',
    compose: 'Ship the empty states for onboarding',
    edit: '# checked by hand, right here in the diff',
    settingsSearch: 'checkpoint',
  },
  'zh-TW': {
    review: '先清掉 cookie 再轉址，不然還是會轉圈圈。',
    compose: '把 onboarding 的空狀態補完整',
    edit: '# 在 diff 裡直接手改的',
    settingsSearch: '檢查點',
  },
};

/** Pin the locale (and the drawer width) before the app boots. */
export function localeScript(locale: MediaLocale, drawer = 600): string {
  return `
    localStorage.setItem('marol.locale', ${JSON.stringify(locale)});
    localStorage.setItem('marol.inspectorWidth', '${drawer}');
  `;
}

/** Welcome answered and every coach mark spent. Each has its own frame;
    turning up uninvited in another one is noise. */
export const QUIET_FIRST_RUN = `
  localStorage.setItem('marol.welcomed', '1');
  localStorage.setItem(
    'marol.coach',
    JSON.stringify({ attempt: true, mode: true, finish: true, terminal: true, waiting: true }),
  );
`;

/** Build the whole desk in one evaluate: five cards across the lifecycle,
    their sessions, stats, checkpoints, diffs — the world the README shows. */
export async function seedWorld(page: Page, locale: MediaLocale, k1Status: string) {
  await page.evaluate(
    ({ titles, diff, base, work, k1Status }) => {
      const m = window.__mock;
      const now = Date.now();
      const mkSession = (
        id: string,
        cwd: string,
        title: string,
        status: string,
        attemptId: string | null,
        extra: Record<string, unknown> = {},
      ) => ({
        id,
        cwd,
        title,
        agent: 'claude',
        status,
        created_at: now - 3600e3,
        last_active_at: now - 45e3,
        live: true,
        reports_status: true,
        hooks_wired: true,
        preview_port: null,
        activity: null,
        activity_since: now - 90e3,
        completed: false,
        attempt_id: attemptId,
        usage: null,
        ...extra,
      });
      const mkAttempt = (
        id: string,
        taskId: string,
        sessionId: string | null,
        extra: Record<string, unknown> = {},
      ) => ({
        id,
        task_id: taskId,
        seq: 1,
        agent: 'claude',
        worktree_path: `/Users/dev/worktrees/${taskId}`,
        branch: `marol/${taskId}-1`,
        base_sha: 'abcd1234deadbeef',
        mode: 'normal',
        outcome: null,
        frozen_diff: null,
        created_at: now - 3000e3,
        parked_at: null,
        session_id: sessionId,
        ...extra,
      });
      const mkTask = (
        id: string,
        title: string,
        lifecycle: string,
        position: number,
        attempts: unknown[],
      ) => ({
        id,
        title,
        prompt: title,
        repo_path: '/Users/dev/webapp',
        base_branch: 'main',
        lifecycle,
        position,
        created_at: now - 86400e3,
        attempts,
        queued_at: null,
      });

      // k1 — the star: the redirect-loop fix, settled or mid-turn per scene.
      const s1 = mkSession('s101', '/Users/dev/worktrees/k1', `${titles[0]} #1`, k1Status, 'k1-a1', {
        activity:
          k1Status === 'running' ? { tool: 'Bash', detail: 'pytest tests/auth -x' } : null,
        usage: {
          input: 48_213,
          output: 612_400,
          cache_read: 96_420_113,
          cache_write: 5_204_887,
          context: 74_310,
        },
      });
      // k2 — blocked on a human: the breathing card.
      const s2 = mkSession('s102', '/Users/dev/worktrees/k2', `${titles[1]} #1`, 'waiting_permission', 'k2-a1', {
        activity: { tool: 'Edit', detail: 'src/api/limiter.ts' },
      });
      // k3 — reviewing, agent idle.
      const s3 = mkSession('s103', '/Users/dev/worktrees/k3', `${titles[2]} #1`, 'idle', 'k3-a1');

      m.sessions.push(s1, s2, s3);
      m.tasks.push(
        mkTask('k1', titles[0], 'running', 0, [mkAttempt('k1-a1', 'k1', 's101')]),
        mkTask('k2', titles[1], 'running', 1, [
          mkAttempt('k2-a1', 'k2', 's102', { mode: 'accept_edits' }),
        ]),
        mkTask('k3', titles[2], 'review', 0, [mkAttempt('k3-a1', 'k3', 's103')]),
        mkTask('k4', titles[3], 'done', 0, [
          mkAttempt('k4-a1', 'k4', null, {
            outcome: 'merged',
            frozen_diff: 'diff --git a/theme.css b/theme.css\n+dark\n',
          }),
        ]),
        mkTask('k5', titles[4], 'running', 2, [
          mkAttempt('k5-a1', 'k5', null, { parked_at: now - 7200e3 }),
        ]),
        mkTask('k6', titles[5], 'backlog', 0, []),
      );

      m.stats.set('k1-a1', { files: 2, adds: 11, dels: 2, ahead: 2, behind: 0, dirty: true });
      m.stats.set('k2-a1', { files: 5, adds: 342, dels: 57, ahead: 0, behind: 3, dirty: false });
      m.stats.set('k3-a1', { files: 1, adds: 22, dels: 4, ahead: 1, behind: 0, dirty: false });
      m.diffs.set('k1-a1', diff);
      m.files.set('k1-a1:src/auth/session.py', { base, work });
      m.checkpoints.set('k1-a1', [
        { n: 1, sha: 'cafe100', at: Math.floor(now / 1000) - 2400 },
        { n: 2, sha: 'cafe200', at: Math.floor(now / 1000) - 600 },
      ]);
      m.record('k1-a1', 'prompt', null, titles[0]);
      m.record('k1-a1', 'tool', 'Bash', 'pytest tests/auth -x');

      // The default tab name ships in zh; the en frames should not wear it.
      m.tabs[0].name = titles[6];
      m.emit('tabs:changed', m.tabs);

      m.pushSessions();
      m.pushTasks();
    },
    { titles: TITLES[locale], diff: DIFF, base: SESSION_BASE, work: SESSION_WORK, k1Status },
  );
}

/** The real captured TUI, into whichever session the shot needs alive.
 *
 *  Only for panes at ~88 columns or wider. The captured frame draws its box
 *  at the width it was recorded on, and xterm reflows it to whatever the
 *  pane actually has — under the threshold the borders break apart, which is
 *  the app being libelled by its own screenshot. A narrower pane gets
 *  `feedLog` instead. */
export async function feedTui(page: Page, sessionId: string) {
  await page.evaluate(
    ({ chunks, id }) => {
      window.__mock.feed(id, chunks[0], 1);
      window.__mock.feed(id, chunks[1], 2);
    },
    { chunks: tui.chunks, id: sessionId },
  );
}

/** Plain wrapping ANSI output, for a pane too narrow to hold a TUI frame:
    the terminal beside a full drawer. Lines, not boxes, so any width is
    honest. */
export async function feedLog(page: Page, sessionId: string) {
  await page.evaluate(
    ({ log, id }) => window.__mock.feed(id, log, 1),
    // Encoded here, not with btoa in the page: the log carries ✓ and box
    // glyphs, beyond btoa's Latin-1 and exactly the UTF-8 bytes xterm's
    // decoder is built to eat.
    { log: Buffer.from(TEST_LOG).toString('base64'), id: sessionId },
  );
}

/** Headless capture races the GPU: a keyed re-sort reparents the xterm
    canvas and can shed its WebGL context between paint and capture. A forced
    full refresh right before the frame settles what the pixels say to what
    the buffer holds. */
export async function repaintTerminals(page: Page) {
  await page.evaluate(() => {
    document.querySelectorAll<HTMLElement & { __term?: any }>('.term-host').forEach((el) => {
      const t = el.__term;
      if (t) t.refresh(0, t.rows - 1);
    });
  });
  await page.waitForTimeout(600);
}

/** A fresh context with the mock and locale pinned. */
export async function mediaPage(
  browser: Browser,
  locale: MediaLocale,
  viewport: { width: number; height: number },
  // Scenes with live terminals shoot at 1x: headless WebGL paints nothing
  // onto @2x canvases here, and a blank terminal is worse than a 1x one.
  // DOM-only scenes keep the crisper 2x.
  dpr = 2,
  extra: { drawer?: number; quietFirstRun?: boolean; recordVideo?: { dir: string; size: { width: number; height: number } } } = {},
): Promise<Page> {
  const context = await browser.newContext({
    viewport,
    deviceScaleFactor: dpr,
    ...(extra.recordVideo ? { recordVideo: extra.recordVideo } : {}),
  });
  await context.addInitScript(localeScript(locale, extra.drawer ?? 600));
  if (extra.quietFirstRun) await context.addInitScript(QUIET_FIRST_RUN);
  await context.addInitScript(installMock);
  const page = await context.newPage();
  await page.goto('http://localhost:5174/');
  await expect(page.locator('.tab')).toHaveCount(1);
  return page;
}
