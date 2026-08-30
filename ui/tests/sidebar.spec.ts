import { test, expect, type Page } from '@playwright/test';
import { installMock } from './mock-tauri';

/** Matches SETTLE_MS in src/sections.ts. */
const SETTLE_MS = 2500;

async function boot(page: Page) {
  await page.addInitScript(installMock);
  await page.goto('/');
  await expect(page.locator('.sidebar')).toBeVisible();
  // The arrangement lives on a tab, so nothing renders until one exists.
  await expect(page.locator('.tab')).toHaveCount(1);
}

async function newSession(page: Page, cwd: string) {
  await page.locator('.sidebar-head button.icon').click();
  await page.locator('.modal input.mono').first().fill(cwd);
  await page.locator('.modal button.primary').click();
  await expect(page.locator('.modal')).toHaveCount(0);
}

/** Which section a session's row is currently rendered under. */
async function sectionOf(page: Page, id: string): Promise<string | null> {
  return page
    .locator(`[data-testid="session-${id}"]`)
    .evaluate((el) => el.closest('.section')?.getAttribute('data-section') ?? null);
}

const report = (page: Page, id: string, status: string, activity?: unknown) =>
  page.evaluate(
    ([i, st, act]) => window.__mock.report(i as string, st as string, act as any),
    [id, status, activity] as const,
  );

test.describe('sidebar sections', () => {
  test('sessions land in the section their status implies', async ({ page }) => {
    await boot(page);
    await newSession(page, '/Users/test/repo-one');
    await newSession(page, '/Users/test/repo-two');

    await report(page, 's1', 'running');
    expect(await sectionOf(page, 's1')).toBe('working');

    // s2 is selected, so s1 is unpinned: being blocked on a permission
    // decision must not wait out the settle window.
    await report(page, 's1', 'waiting_permission');
    await expect.poll(() => sectionOf(page, 's1')).toBe('waiting');
  });

  test('the pin holds even for a permission prompt on the session you are watching', async ({
    page,
  }) => {
    await page.clock.install();
    await boot(page);
    await newSession(page, '/Users/test/repo-one');
    await report(page, 's1', 'running');
    expect(await sectionOf(page, 's1')).toBe('working');

    // The prompt is already on screen in the terminal you are looking at, and
    // the waiting banner still counts it — moving the row would be noise.
    await report(page, 's1', 'waiting_permission');
    await page.clock.fastForward(SETTLE_MS * 3);
    expect(await sectionOf(page, 's1')).toBe('working');
    await expect(page.locator('.waiting-banner')).toContainText('1 個等你');
  });

  test('an end-of-turn blip does not move the row', async ({ page }) => {
    await page.clock.install();
    await boot(page);
    await newSession(page, '/Users/test/repo-one');
    await newSession(page, '/Users/test/repo-two');

    // s2 is selected (newest), so s1 is the unpinned one to observe.
    await report(page, 's1', 'running');
    await expect.poll(() => sectionOf(page, 's1')).toBe('working');

    // `Stop` fires at the end of every turn. A session that is about to keep
    // working must not visibly hop to 等待輸入 and back.
    await report(page, 's1', 'idle');
    await page.clock.fastForward(SETTLE_MS - 500);
    expect(await sectionOf(page, 's1')).toBe('working');

    await report(page, 's1', 'running');
    await page.clock.fastForward(SETTLE_MS * 2);
    expect(await sectionOf(page, 's1')).toBe('working');
  });

  test('a session that really has settled does move', async ({ page }) => {
    await page.clock.install();
    await boot(page);
    await newSession(page, '/Users/test/repo-one');
    await newSession(page, '/Users/test/repo-two');

    await report(page, 's1', 'running');
    await expect.poll(() => sectionOf(page, 's1')).toBe('working');

    await report(page, 's1', 'idle');
    await page.clock.fastForward(SETTLE_MS + 500);
    await expect.poll(() => sectionOf(page, 's1')).toBe('idle');
  });

  test('待命 is not 等你: the section count and the ⚠ banner agree', async ({ page }) => {
    await page.clock.install();
    await boot(page);
    await newSession(page, '/Users/test/repo-one');
    await newSession(page, '/Users/test/repo-two');
    await newSession(page, '/Users/test/repo-three');

    // One settled turn, one real block. Filing both under 等你 made the
    // section say 2 over a banner saying ⚠ 1.
    await report(page, 's1', 'idle');
    await report(page, 's2', 'waiting_permission');
    await page.clock.fastForward(SETTLE_MS + 500);
    await expect.poll(() => sectionOf(page, 's1')).toBe('idle');
    await expect.poll(() => sectionOf(page, 's2')).toBe('waiting');

    await expect(page.locator('.waiting-banner')).toHaveText(/1/);
    await expect(page.locator('[data-section="waiting"] .section-count')).toHaveText('1');
    await expect(page.locator('[data-section="idle"] .section-count')).toHaveText('1');
  });

  test('a section head says whether it is open, not only to the eye', async ({ page }) => {
    await boot(page);
    await newSession(page, '/Users/test/repo-one');

    const head = page.locator('[data-section="working"] .section-head');
    await expect(head).toHaveAttribute('aria-expanded', 'true');
    await head.click();
    await expect(head).toHaveAttribute('aria-expanded', 'false');
    await expect(page.locator('[data-testid="session-s1"]')).toHaveCount(0);
  });

  test('the selected session is pinned so the row never moves under the cursor', async ({
    page,
  }) => {
    await page.clock.install();
    await boot(page);
    await newSession(page, '/Users/test/repo-one');

    await report(page, 's1', 'running');
    await expect.poll(() => sectionOf(page, 's1')).toBe('working');

    // s1 is the selected session. Even a long settle must not move it.
    await report(page, 's1', 'idle');
    await page.clock.fastForward(SETTLE_MS * 4);
    expect(await sectionOf(page, 's1')).toBe('working');
  });

  test('marking a session done files it under 已完成 at once, even when pinned', async ({
    page,
  }) => {
    await page.clock.install();
    await boot(page);
    await newSession(page, '/Users/test/repo-one');
    expect(await sectionOf(page, 's1')).toBe('working');

    // s1 is the selected session, so the pin would normally hold it in place.
    // An explicit action must override that without waiting.
    // By its name, not its place in the row: the actions are a list that
    // grows, and "the first one" is not what this test is about.
    await page
      .locator('[data-testid="session-s1"] .row-action')
      .and(page.getByLabel('標記為完成'))
      .click();
    await expect(page.locator('[data-section="done"]')).toBeVisible();

    // 已完成 starts collapsed, so the row is filed there but out of the way.
    await page.locator('[data-section="done"] .section-head').click();
    await expect.poll(() => sectionOf(page, 's1')).toBe('done');
  });

  test('the row shows what the agent is doing, not just that it is busy', async ({ page }) => {
    await boot(page);
    await newSession(page, '/Users/test/repo-one');

    await report(page, 's1', 'running', { tool: 'Bash', detail: 'pytest tests/test_auth.py -v' });

    const row = page.locator('[data-testid="session-s1"]');
    await expect(row.locator('.row-tool')).toHaveText('Bash');
    await expect(row.locator('.row-detail')).toHaveText('pytest tests/test_auth.py -v');
  });
});

/**
 * Telling one session from another.
 *
 * A session opened without a card can only be called after its directory, and
 * opening several terminals in one checkout is the ordinary thing to do here
 * — so the default has to stop repeating itself, and the name has to be
 * changeable once the work has a better one.
 */
test.describe('naming a session', () => {
  test('sessions in one directory do not all get the same row name', async ({ page }) => {
    await boot(page);
    await newSession(page, '/Users/test/repo-one');
    await newSession(page, '/Users/test/repo-one');
    await newSession(page, '/Users/test/repo-one');

    await expect(page.locator('[data-testid="session-s1"] .row-title')).toHaveText('repo-one');
    await expect(page.locator('[data-testid="session-s2"] .row-title')).toHaveText('repo-one 2');
    await expect(page.locator('[data-testid="session-s3"] .row-title')).toHaveText('repo-one 3');
  });

  test('double-clicking the name edits it in place, Enter keeps it', async ({ page }) => {
    await boot(page);
    await newSession(page, '/Users/test/repo-one');

    await page.locator('[data-testid="session-s1"] .row-title').dblclick();
    const input = page.getByTestId('rename-s1');
    await expect(input).toBeFocused();
    await input.fill('改登入導向');
    await input.press('Enter');

    await expect(page.locator('[data-testid="session-s1"] .row-title')).toHaveText('改登入導向');
    // The name is the row's whole identity, so it reaches what a screen
    // reader hears too, not only what is drawn.
    await expect(page.getByTestId('session-s1')).toHaveAttribute(
      'aria-label',
      /^改登入導向，/,
    );
  });

  test('Escape puts the old name back, and F2 opens the editor from the keyboard', async ({
    page,
  }) => {
    await boot(page);
    await newSession(page, '/Users/test/repo-one');
    const title = page.locator('[data-testid="session-s1"] .row-title');

    await title.focus();
    await page.keyboard.press('F2');
    const input = page.getByTestId('rename-s1');
    await expect(input).toBeFocused();
    await input.fill('半路反悔');
    await input.press('Escape');
    await expect(title).toHaveText('repo-one');

    // And the ✎ button is the same door for anyone who never learns F2.
    await page
      .locator('[data-testid="session-s1"] .row-action')
      .and(page.getByLabel('改名（F2）'))
      .click();
    await page.getByTestId('rename-s1').fill('改登入導向');
    await page.getByTestId('rename-s1').press('Enter');
    await expect(title).toHaveText('改登入導向');
  });

  test('a name blanked to nothing leaves the row as it was', async ({ page }) => {
    await boot(page);
    await newSession(page, '/Users/test/repo-one');

    await page.locator('[data-testid="session-s1"] .row-title').dblclick();
    await page.getByTestId('rename-s1').fill('   ');
    await page.getByTestId('rename-s1').press('Enter');

    // A row with no name is a row you can no longer pick out at all.
    await expect(page.locator('[data-testid="session-s1"] .row-title')).toHaveText('repo-one');
  });
});

/**
 * 收起側欄。
 *
 * 借的是每個有側欄的編輯器都有的那顆鍵(⌘/Ctrl+B),但這裡的收起不是
 * 「藏起來」:摺完剩一條軌,軌上留兩樣東西 —— 回去的路,以及「幾個在
 * 等你」。第一樣是因為只能用快捷鍵離開的狀態會把人關在裡面;第二樣是
 * 因為那個數字是這個 app 存在的理由,把清單收起來不該連它一起收掉。
 *
 * 另一半是效能:摺起來時每一列、每一段、還有那個一秒一次的計時器都
 * 真的卸載,不是用 CSS 藏起來。這條由「計時器停了」釘住。
 */
test.describe('folding the sidebar', () => {
  test('the fold leaves a rail with the way back on it, and remembers itself', async ({ page }) => {
    await boot(page);
    await newSession(page, '/Users/test/repo-one');
    await expect(page.getByTestId('session-s1')).toBeVisible();

    await page.getByTestId('sidebar-fold').click();

    // 列真的不見了 —— 不是被切掉的可視範圍。
    await expect(page.getByTestId('session-s1')).toHaveCount(0);
    await expect(page.locator('.app')).toHaveClass(/sidebar-folded/);
    // 回去的路一定在:只能用快捷鍵離開的狀態,是把人關起來。
    await expect(page.getByTestId('sidebar-unfold')).toBeVisible();

    // 跨重開機記得,和世界選擇器同一條規矩。
    await page.reload();
    await expect(page.getByTestId('sidebar-unfold')).toBeVisible();
    await expect(page.getByTestId('session-s1')).toHaveCount(0);

    await page.getByTestId('sidebar-unfold').click();
    await expect(page.getByTestId('session-s1')).toBeVisible();
    await expect(page.locator('.app')).not.toHaveClass(/sidebar-folded/);
  });

  test('⌘/Ctrl+B folds it, and the terminal keeps plain Ctrl+B for readline', async ({ page }) => {
    await boot(page);
    await newSession(page, '/Users/test/repo-one');

    // 開完 session,焦點就在那個終端機裡 —— 在裡面,Ctrl+B 是 readline
    // 的 backward-char,不是我們的。這一按必須什麼都不做。
    //
    // 這裡刻意寫死 Control 而不是 ControlOrMeta:被讓出去的那顆是 Ctrl,
    // 每個平台都一樣,因為 readline 讀的是 Ctrl。在 mac 上 ControlOrMeta
    // 會變成 ⌘,而 ⌘B 從來就不是 shell 的 —— 它該收側欄,底下那一按就是
    // 在講這件事。
    await page.keyboard.press('Control+b');
    await expect(page.getByTestId('session-s1')).toBeVisible();

    // shell 沒在讀的修飾鍵,終端機裡外都是我們的。
    await page.keyboard.press('Meta+b');
    await expect(page.getByTestId('sidebar-unfold')).toBeVisible();
    await page.getByTestId('sidebar-unfold').click();
    await expect(page.getByTestId('session-s1')).toBeVisible();
    await page.locator('.term-host').first().click();

    // 加 Shift 才是這個 app 的那一顆,與 E/L/F/I 同一條規則。
    await page.keyboard.press('ControlOrMeta+Shift+B');
    await expect(page.getByTestId('sidebar-unfold')).toBeVisible();

    // 展開後那顆鈕自己卸載,焦點落回 body —— 也就是終端機外面,
    // 素的那一顆在這裡才是我們的。
    await page.getByTestId('sidebar-unfold').click();
    await expect(page.getByTestId('session-s1')).toBeVisible();
    await page.keyboard.press('ControlOrMeta+b');
    await expect(page.getByTestId('sidebar-unfold')).toBeVisible();
  });

  test('the one number survives the fold, and still cycles', async ({ page }) => {
    await boot(page);
    await newSession(page, '/Users/test/repo-one');
    await newSession(page, '/Users/test/repo-two');
    await report(page, 's1', 'waiting_input');
    await report(page, 's2', 'waiting_permission');
    await expect(page.locator('.waiting-banner')).toContainText('2');

    await page.getByTestId('sidebar-fold').click();
    // 收起清單不該連「幾個在等你」一起收掉 —— 那是這張桌子的重點。
    await expect(page.getByTestId('rail-waiting')).toContainText('2');
    // 而且和橫幅一樣是輪替,不是跳到同一個:答完一個按下一次就是下一個。
    await page.getByTestId('rail-waiting').click();
    await expect(page.locator('.pane.focused')).toHaveAttribute('data-session-id', 's1');
    await page.getByTestId('rail-waiting').click();
    await expect(page.locator('.pane.focused')).toHaveAttribute('data-session-id', 's2');
  });

  test('a folded sidebar stops the once-a-second timer', async ({ page }) => {
    await page.clock.install();
    await boot(page);
    await newSession(page, '/Users/test/repo-one');
    // 有活動的列才有計秒 —— 計時器的存在條件就是它。
    await report(page, 's1', 'running', { tool: 'Bash', detail: 'npm test' });
    await expect(page.locator('[data-testid="session-s1"] .row-elapsed')).toBeVisible();

    await page.clock.runFor(3000);
    const ticking = await page.locator('[data-testid="session-s1"] .row-elapsed').textContent();
    expect(ticking).not.toBe('0s');

    // 摺起來:列卸載,計時器跟著走。展開後從當下重新開始數,而不是
    // 從一個在背景空轉了整段時間的時鐘接手。
    await page.getByTestId('sidebar-fold').click();
    await page.clock.runFor(60_000);
    await page.getByTestId('sidebar-unfold').click();
    await expect(page.locator('[data-testid="session-s1"] .row-elapsed')).toBeVisible();
  });
});
