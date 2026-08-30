import { expect, type Page } from '@playwright/test';
import {
  installMock,
  type MockAttempt,
  type MockSession,
  type MockTask,
} from '../mock-tauri';

/** The one repository the mock's chooser and repo table both know. */
export const REPO = '/Users/test/picked-repo';

/**
 * 冷啟：真正的第一次。
 *
 * mock 安裝時會預答 welcomed/coach（讓其他 suite 不用跟歡迎面板打架），
 * 所以第一次進場前把兩把旗標拆掉 —— 但只拆一次（__rearmedOnce 守著）：
 * reload 必須走真的持久化，不能被 harness 每次都重新抹掉，否則
 * 「重新整理不再被招呼」這條契約永遠測不到。與 onboarding.spec 的
 * bootFresh 同一份寫法，journeys 只是把它立為地基。
 */
export async function coldStart(page: Page): Promise<void> {
  await page.addInitScript(installMock);
  await page.addInitScript(() => {
    if (sessionStorage.getItem('__rearmedOnce') === null) {
      sessionStorage.setItem('__rearmedOnce', '1');
      localStorage.removeItem('marol.welcomed');
      localStorage.removeItem('marol.coach');
    }
  });
  await page.goto('/');
}

/**
 * 平常的開機：一張已經用過的桌子。
 *
 * mock 預答過的一次性介面（歡迎、coach）維持已答 —— 之後的 journey
 * 從這裡出發，不必每一條都重演第一次。斷言 sidebar 出現＝BootGate
 * 已經放行，後面的步驟才有地板可站。
 */
export async function boot(page: Page): Promise<void> {
  await page.addInitScript(installMock);
  await page.goto('/');
  await expect(page.locator('.sidebar')).toBeVisible();
}

/* ------------------------------ shapes ------------------------------ */
/* 形狀工廠只補欄位不加語意：mock-tauri 的介面要什麼就給什麼，讓
   journey 用兩三行說出「N 張卡」「一個等你的 session」這種常見開場。 */

/** 一張卡的最小合法形狀。id 依 mock 的慣例編成 k1、k2…，之後從 UI
 *  再建的卡會接著編號，不會相撞。 */
export function cardShape(n: number, over: Partial<MockTask> = {}): MockTask {
  return {
    id: `k${n}`,
    title: `卡片 ${n}`,
    prompt: '把它修好',
    repo_path: REPO,
    base_branch: 'main',
    lifecycle: 'backlog',
    position: n - 1,
    created_at: 1000 + n,
    attempts: [],
    queued_at: null,
    ...over,
  };
}

/** 一個 attempt 的最小合法形狀，id 走 mock 的 `<taskId>-a<seq>` 慣例。 */
export function attemptShape(
  taskId: string,
  seq: number,
  over: Partial<MockAttempt> = {},
): MockAttempt {
  return {
    id: `${taskId}-a${seq}`,
    task_id: taskId,
    seq,
    agent: 'claude',
    worktree_path: `/Users/test/worktrees/card-${seq}`,
    branch: `marol/card-${seq}`,
    base_sha: 'abcd1234deadbeef',
    mode: 'normal',
    outcome: null,
    frozen_diff: null,
    created_at: 1000,
    parked_at: null,
    session_id: null,
    ...over,
  };
}

/** 一個 session 的最小合法形狀。id 用 s9x 一類的高位編號，避開
 *  makeSession 從 s1 起跳的計數器。 */
export function sessionShape(id: string, over: Partial<MockSession> = {}): MockSession {
  return {
    id,
    cwd: '/Users/test/app',
    title: 'app',
    agent: 'claude',
    status: 'running',
    created_at: 1000,
    last_active_at: 1000,
    live: true,
    reports_status: true,
    hooks_wired: true,
    preview_port: null,
    activity: null,
    activity_since: 0,
    completed: false,
    attempt_id: null,
    ...over,
  };
}

/* ------------------------------ seeding ----------------------------- */

/**
 * 把形狀寫進 mock 讀種子的 sessionStorage 鍵。
 *
 * 必須在 coldStart/boot **之前**呼叫：installMock 是 init script，
 * 建構當下就讀 __mockTasks/__mockSessions —— 之後才加的種子它看不見。
 * 與 onboarding.spec 種 __mockAgents 的順序同一條規矩。
 */
export async function seedDesk(
  page: Page,
  desk: { tasks?: MockTask[]; sessions?: MockSession[] },
): Promise<void> {
  await page.addInitScript((d: { tasks?: MockTask[]; sessions?: MockSession[] }) => {
    if (d.tasks) sessionStorage.setItem('__mockTasks', JSON.stringify(d.tasks));
    if (d.sessions) sessionStorage.setItem('__mockSessions', JSON.stringify(d.sessions));
  }, desk);
}

/** N 張卡，同一欄 —— 看板批量開場的一行寫法。 */
export async function seedCards(
  page: Page,
  n: number,
  lifecycle: MockTask['lifecycle'] = 'backlog',
): Promise<void> {
  await seedDesk(page, {
    tasks: Array.from({ length: n }, (_, i) => cardShape(i + 1, { lifecycle, position: i })),
  });
}

/** 一個正在等你的 session（臨時、不掛卡）：整條「等你」訊號鏈 ——
 *  琥珀橫幅、等你分區、⌘E —— 的最小點火材料。id 固定 s91。 */
export async function seedWaitingSession(page: Page): Promise<void> {
  await seedDesk(page, {
    sessions: [
      sessionShape('s91', { status: 'waiting_permission', title: '等你的 session' }),
    ],
  });
}

/** 一張停了的桌子：重啟後每個 attempt 的樣子 —— 卡在進行中、attempt
 *  還開著、session 沒有終端機（live:false），看板該端出「繼續」。 */
export async function seedStoppedDesk(page: Page): Promise<void> {
  await seedDesk(page, {
    tasks: [
      cardShape(1, {
        lifecycle: 'running',
        attempts: [attemptShape('k1', 1, { session_id: 's91' })],
      }),
    ],
    sessions: [
      sessionShape('s91', {
        live: false,
        status: 'saved',
        attempt_id: 'k1-a1',
        title: '卡片 1 #1',
        cwd: '/Users/test/worktrees/card-1',
      }),
    ],
  });
}

/* ----------------------------- contracts ---------------------------- */

/**
 * 朗讀通道說了什麼。
 *
 * 契約：畫面上每個要人動作的訊號，都要同時經過 aria-live 的
 * .visually-hidden 區（data-testid="live-announce"）。說完 5 秒會自清，
 * 所以斷言靠 Playwright 的輪詢在窗口內接住，不 sleep。
 */
export async function expectAnnounce(page: Page, text: string | RegExp): Promise<void> {
  await expect(page.getByTestId('live-announce')).toContainText(text);
}

/**
 * 焦點真正的落點。
 *
 * 契約：對話框開合、視圖切換之後，焦點必須落在說好的容器裡 ——
 * 看 document.activeElement 本人，不看 CSS 的樣子。傳 testid，或以
 * . / # / [ 開頭的原生 selector（modal、pane 這類沒有 testid 的容器）。
 */
export async function expectFocusWithin(page: Page, target: string): Promise<void> {
  const selector = /^[.#[]/.test(target) ? target : `[data-testid="${target}"]`;
  await expect
    .poll(
      () =>
        page.evaluate((sel) => {
          const el = document.querySelector(sel);
          return el !== null && el.contains(document.activeElement);
        }, selector),
      { message: `focus should be inside ${selector}` },
    )
    .toBe(true);
}

/**
 * 焦點退回中性起點。
 *
 * `expectFocusWithin` 的另一半，而且同樣要等 —— 這是這個檔案裡唯一
 * 一個曾經寫成「不等」的斷言，然後在 macOS 上紅了。焦點的落點會等
 * 一格畫面，焦點的*離開*一樣：持有焦點的元素被卸載時，瀏覽器把
 * activeElement 收回 <body>，那是它自己的排程，不是斷言的。
 *
 * 契約沒有變鬆：仍然要退回 <body>，只是不再要求它在某一格之內做完。
 */
export async function expectFocusNeutral(page: Page): Promise<void> {
  await expect
    .poll(() => page.evaluate(() => document.activeElement === document.body), {
      message: 'focus should have gone back to <body>',
    })
    .toBe(true);
}

/**
 * 平台修飾鍵的和弦，existing specs 的同一顆：ControlOrMeta —— mac 上是
 * ⌘、其他平台是 Ctrl，與 App 鍵盤表聽的 metaKey/ctrlKey 同一份事實。
 * 終端機裡的變體加 Shift（Ctrl+字母屬於 shell），opts.shift 說這件事。
 */
export async function chord(
  page: Page,
  key: string,
  opts: { shift?: boolean } = {},
): Promise<void> {
  await page.keyboard.press(`ControlOrMeta+${opts.shift ? 'Shift+' : ''}${key}`);
}

/**
 * 讓 mock 代打一個 hook 回報：狀態轉變（working→waiting、回合結束）
 * 都從這裡驅動 —— window.__mock.report 是 suite 全體共用的那扇門，
 * 這裡只是把 evaluate 的樣板收起來。
 */
export async function driveStatus(
  page: Page,
  id: string,
  status: string,
  activity?: { tool: string; detail: string },
): Promise<void> {
  await page.evaluate(
    (args: { id: string; status: string; activity?: { tool: string; detail: string } }) =>
      window.__mock.report(args.id, args.status, args.activity),
    { id, status, activity },
  );
}
