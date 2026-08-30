import { test, expect, type Page } from '@playwright/test';
import { installMock } from './mock-tauri';

const REPO = '/Users/test/picked-repo';

/** `goto`, and then wait for the app to actually be there.
 *
 * Almost every test below presses a key or clicks something the moment the
 * page loads, and each of those is handled by a listener React installs on
 * mount. Pressing before that is a race the test wins on a fast machine and
 * loses on a slow one — which is what it did on the macOS runner, on a
 * different test each run. From outside it looks like flakiness; from
 * inside it is a missing wait.
 */
async function land(page: Page) {
  await page.goto('/');
  await expect(page.locator('.tab')).toHaveCount(1);
}

/** Boot with the one-shot surfaces re-armed — the mock normally
 *  pre-answers them so the rest of the suite never fights them. Re-armed
 *  once per tab, not per load: a reload must exercise the real
 *  persistence, not have the harness wiping it again. */
async function bootFresh(page: Page) {
  await page.addInitScript(installMock);
  await page.addInitScript(() => {
    if (sessionStorage.getItem('__rearmedOnce') === null) {
      sessionStorage.setItem('__rearmedOnce', '1');
      localStorage.removeItem('marol.welcomed');
      localStorage.removeItem('marol.coach');
    }
  });
  await land(page);
}

async function newCard(page: Page, title: string) {
  await page.getByRole('button', { name: '新增卡片', exact: true }).click();
  await page.getByTestId('task-title').fill(title);
  await page.getByTestId('task-prompt').fill('把它修好');
  await page.getByTestId('task-repo').fill(REPO);
  await page.getByTestId('task-branch').fill('main');
  await page.getByTestId('task-create').click();
}

test.describe('the first-run panel', () => {
  test('an empty desk is greeted with what the probe found', async ({ page }) => {
    await bootFresh(page);

    const modal = page.locator('.modal');
    await expect(modal).toContainText('歡迎使用 Marol');
    // The detection report is the probe the app already ran, shown.
    await expect(page.getByTestId('welcome-claude')).toContainText('✓ 2.1.226');
    await expect(page.getByTestId('welcome-codex')).toContainText('找不到');
  });

  test('the primary way out is the first card', async ({ page }) => {
    await bootFresh(page);
    await page.getByTestId('welcome-card').click();

    // Straight into the board with the card dialog open — no dead end.
    await expect(page.getByTestId('board')).toBeVisible();
    await expect(page.getByTestId('task-title')).toBeVisible();

    // And never again.
    await page.reload();
    await expect(page.getByTestId('task-title')).toHaveCount(0);
    await expect(page.locator('.modal')).toHaveCount(0);
  });

  test('a true first run lands on the board, on the door to the first card', async ({
    page,
  }) => {
    await bootFresh(page);
    // 歡迎面板浮在看板上;關掉它,腳下已經是看板 —— 不是空的終端牆。
    await page.locator('.modal button', { hasText: '關閉' }).click();
    await expect(page.getByTestId('board')).toBeVisible();
    // 空的待辦欄本身就是那扇門,第一分鐘與第一百次說同一句話。
    await expect(page.getByTestId('board-cta')).toHaveText('新增一張卡片');
  });

  test('the mental model wears the board’s dot vocabulary, statically', async ({ page }) => {
    await bootFresh(page);
    const rail = page.locator('.welcome-rail-row');
    await expect(rail).toHaveCount(3);
    await expect(rail.nth(0)).toContainText('一張卡片');
    await expect(rail.nth(2)).toContainText('合');
  });

  test('a machine with no agent CLI wears the amber banner, and probe again repaints', async ({
    page,
  }) => {
    await page.addInitScript(() => {
      sessionStorage.setItem(
        '__mockAgents',
        JSON.stringify([
          { name: 'claude', path: null },
          { name: 'codex', path: null },
          { name: 'gemini', path: null },
          { name: 'aider', path: null },
        ]),
      );
    });
    await bootFresh(page);
    await expect(page.getByTestId('welcome-no-agents')).toBeVisible();
    await expect(page.getByTestId('welcome-no-agents')).toContainText('找不到 agent CLI');

    // 裝好 CLI 之後按「重新偵測」:真的重跑 boot_status,發現就換新。
    await page.evaluate(() =>
      sessionStorage.setItem(
        '__mockAgents',
        JSON.stringify([
          { name: 'claude', path: '/usr/local/bin/claude' },
          { name: 'codex', path: null },
          { name: 'gemini', path: null },
          { name: 'aider', path: null },
        ]),
      ),
    );
    await page.getByTestId('welcome-reprobe').click();
    await expect(page.getByTestId('welcome-claude')).toContainText('✓');
    await expect(page.getByTestId('welcome-no-agents')).toHaveCount(0);
  });

  test('the welcome panel reopens from the environment panel, flags untouched', async ({
    page,
  }) => {
    await page.addInitScript(installMock);
    await land(page);
    await page.getByRole('button', { name: '設定' }).click();
    await page.getByTestId('show-welcome').click();
    await expect(page.locator('.modal')).toContainText('歡迎使用 Marol');

    // 重看不是重來:旗標留著,重新整理不會再被招呼。
    await page.locator('.modal button', { hasText: '關閉' }).click();
    const flag = await page.evaluate(() => localStorage.getItem('marol.welcomed'));
    expect(flag).toBe('1');
    await page.reload();
    await expect(page.locator('.modal')).toHaveCount(0);
  });

  /**
   * 介面上的字被刻意收短了,教學搬進了歡迎面板與 README —— 那就必須有一條
   * 從 app 走得到 README 的路,否則不是搬家,是把知識丟掉。
   * 而讀中文的人不該被丟到英文那份,所以連結跟著介面語言走。
   */
  test('the environment panel opens the documentation, in the interface language', async ({
    page,
  }) => {
    await page.addInitScript(installMock);
    await land(page);
    await page.getByRole('button', { name: '設定' }).click();
    await page.getByTestId('open-docs').click();
    const zh = await page.evaluate(
      () => window.__mock.calls.find((c) => c.cmd === 'plugin:opener|open_url')?.args,
    );
    expect((zh as { url: string }).url).toContain('README.zh-TW.md');
  });

  /** The one mark left still deserves a way back to it. */
  test('the first-run tip can be shown again from the settings panel', async ({ page }) => {
    await page.addInitScript(installMock);
    await page.addInitScript(() =>
      localStorage.setItem('marol.coach', JSON.stringify({ terminal: true })),
    );
    await land(page);
    await page.getByRole('button', { name: '設定' }).click();
    await page.getByTestId('replay-coach').click();
    await expect(page.locator('.toast')).toContainText('已重設');
    const marks = await page.evaluate(() => localStorage.getItem('marol.coach'));
    expect(marks).toBeNull();
  });

  test('the welcome panel reopens from the palette too', async ({ page }) => {
    await page.addInitScript(installMock);
    await land(page);
    await page.keyboard.press('ControlOrMeta+K');
    await page.getByTestId('palette-input').fill('歡迎');
    await page.getByTestId('pal-action-show-welcome').click();
    await expect(page.locator('.modal')).toContainText('歡迎使用 Marol');
  });

  test('a desk already in use is never greeted', async ({ page }) => {
    // One surviving session marks the desk as lived-in. Seeded before the
    // mock installs, because it reads this storage as it loads.
    await page.addInitScript(() => {
      sessionStorage.setItem(
        '__mockSessions',
        JSON.stringify([
          {
            id: 's9', cwd: '/Users/test/app', title: 'app', agent: 'claude',
            status: 'saved', created_at: 1, last_active_at: 1, live: false,
            reports_status: false, hooks_wired: true, activity: null, activity_since: 0,
            completed: false, attempt_id: null,
          },
        ]),
      );
    });
    await page.addInitScript(installMock);
    // Re-armed after the mock installs — the mock pre-answers it, and a
    // pre-answered welcome would make this test prove nothing.
    await page.addInitScript(() => {
      localStorage.removeItem('marol.welcomed');
    });
    await land(page);

    // A closed session files under 已完成, which starts collapsed — the
    // count on the section head is the proof the desk is lived-in.
    const done = page.locator('.section[data-section="done"]');
    await expect(done.locator('.section-count')).toHaveText('1');
    await expect(page.locator('.modal')).toHaveCount(0);
  });
});

test.describe('one-shot coaching', () => {
  /**
   * Four of the five marks taught what the screen was already saying at the
   * moment they fired — the worktree (the welcome panel's three lines), the
   * permission mode (the label on the select just used), that finishing is
   * final (the buttons arm), that an agent is waiting (the sidebar counts it,
   * the card wears it, the pane pulses, the tab badges it). Starting an
   * attempt now teaches nothing, because there is nothing left to teach.
   */
  test('starting an attempt no longer interrupts to explain itself', async ({ page }) => {
    await bootFresh(page);
    await page.locator('.modal button', { hasText: '關閉' }).click();
    await page.getByTestId('view-board').click();
    await newCard(page, '修好登入');

    await page.locator('[data-testid="task-k1"] button.primary').click();
    await page.getByTestId('attempt-start').click();
    await expect(page.locator('.pane:visible')).toHaveCount(1);

    // Landing in the pane raises the one mark that survives — the keyboard
    // trap — and nothing about worktrees.
    await expect(page.getByTestId('coach-terminal')).toBeVisible();
    await expect(page.getByTestId('coach-attempt')).toHaveCount(0);
    await expect(page.locator('.coach')).toHaveCount(1);
  });

  /** Nor does opening the drawer, nor a turn going into 等你. */
  test('neither the drawer nor a blocked turn raises a mark', async ({ page }) => {
    await bootFresh(page);
    await page.locator('.modal button', { hasText: '關閉' }).click();
    await page.getByTestId('view-board').click();
    await newCard(page, '修好登入');
    await page.locator('[data-testid="task-k1"] button.primary').click();
    await page.getByTestId('attempt-start').click();
    // Retire the terminal mark so anything left on screen is a new one.
    await page.getByTestId('coach-dismiss').click();

    await page.getByTestId('view-board').click();
    await page.getByTestId('inspect-k1').click();
    await expect(page.locator('.coach')).toHaveCount(0);

    await page.evaluate(() => window.__mock.report('s1', 'running'));
    await expect(page.locator('.dot.running').first()).toBeVisible();
    await page.evaluate(() => window.__mock.report('s1', 'waiting_permission'));
    await expect(page.locator('.dot.waiting_permission').first()).toBeVisible();
    await expect(page.locator('.coach')).toHaveCount(0);
  });
});

test.describe('the first-run terminal wall', () => {
  test('the empty wall teaches three keys, and retires once any session exists', async ({
    page,
  }) => {
    await bootFresh(page);
    await page.locator('.modal button', { hasText: '關閉' }).click();
    // 第一次落在看板;去看終端牆。
    await page.keyboard.press('ControlOrMeta+1');
    await expect(page.getByTestId('term-keymap')).toBeVisible();
    await expect(page.getByTestId('term-keymap')).toContainText('終端機 · 看板 · 總覽');
    await expect(page.getByTestId('term-keymap')).toContainText('快捷鍵');

    // 開過 session 之後讓位:退出佈局後的空網格說的是老話,不再上課。
    await page.locator('.sidebar-head button.icon').click();
    await page.locator('.modal input.mono').first().fill('/Users/test/repo-one');
    await page.locator('.modal button.primary').click();
    await expect(page.locator('.pane:visible')).toHaveCount(1);
    await page.getByTestId('eject-s1').click();
    await expect(page.getByTestId('empty-grid')).toBeVisible();
    await expect(page.getByTestId('term-keymap')).toHaveCount(0);
  });
});

/**
 * 這個面板原本是一條卷軸,把這張桌子所有能被交代的事疊在一起,藏在側欄
 * 底部一顆 11px 灰字鈕後 —— 審查判定的最弱項。分區給了它名字,搜尋讓人
 * 用「畫面上叫什麼」就找得到,⌘, 讓它有一扇平台自己的門。
 */
test.describe('settings', () => {
  const open = async (page: Page) => {
    await page.addInitScript(installMock);
    await land(page);
    await page.keyboard.press('ControlOrMeta+,');
    await expect(page.getByTestId('settings-body')).toBeVisible();
  };

  test('⌘/Ctrl+, opens it, and the rail names what is in here', async ({ page }) => {
    await open(page);
    await expect(page.getByRole('heading', { name: '設定' })).toBeVisible();
    await expect(page.getByTestId('sec-general')).toBeVisible();
    await expect(page.getByTestId('sec-diagnostics')).toBeVisible();
  });

  test('search finds a setting by the name it is called on screen', async ({ page }) => {
    await open(page);
    // 「檢查點」住在 Session 分區,而不在預設看得到的那一頁。
    await page.getByTestId('settings-search').fill('檢查點');
    await expect(page.getByTestId('sec-general')).toHaveCount(0);
    await expect(page.getByTestId('sec-sessions')).toBeVisible();
    await page.getByTestId('sec-sessions').click();
    await expect(page.getByTestId('ckpt-toggle')).toBeVisible();
  });

  /**
   * 這個設定的標籤原本寫「（Claude Code session）」,而那是錯的:快照掛
   * 在 Stop hook 上,Codex 也發 Stop。一個 codex 使用者讀了那句話,會
   * 關掉一個本來對他有效的功能 —— 或者從來不打開它。
   */
  test('the turn-end snapshot does not claim to be one CLI’s', async ({ page }) => {
    await open(page);
    await page.getByTestId('sec-sessions').click();
    const body = page.getByTestId('settings-body');
    await expect(body).toContainText('回合結束時自動快照');
    await expect(body).not.toContainText('Claude Code session）');
    // 說出真正的條件:會回報狀態的 agent,兩個都算。
    await expect(body).toContainText('Codex');
  });

  test('a search that matches nothing says so rather than showing everything', async ({ page }) => {
    await open(page);
    await page.getByTestId('settings-search').fill('zzzz');
    await expect(page.getByTestId('sec-general')).toHaveCount(0);
    await expect(page.getByTestId('settings-body')).toBeVisible();
  });

  /**
   * 每個拒絕都附上完整理由 —— 但理由原本活在 README 與決策文件裡,
   * 就是不在「使用者打開設定、找不到那個開關」的那一刻。
   */
  test('the refusals answer where the search for them ends', async ({ page }) => {
    await open(page);
    await page.getByTestId('sec-agents').click();
    await expect(page.getByTestId('note-agents')).toContainText('憑證');
    await page.getByTestId('sec-terminal').click();
    await expect(page.getByTestId('note-scrollback')).toContainText('不留副本');
    await page.getByTestId('sec-advanced').click();
    await expect(page.getByTestId('note-telemetry')).toContainText('不收集任何資料');
    // Apache-2.0 要求的那份清單,也在這裡。
    await expect(page.getByTestId('licenses')).toContainText('xterm.js');
  });

  /**
   * 開場 prompt 一直都看得到、也一直都能就地改 —— 缺的只是「改給往後每一次」
   * 的那個入口。
   */
  test('the opening prompt template has a door', async ({ page }) => {
    await open(page);
    await page.getByTestId('sec-sessions').click();
    await page.getByTestId('open-template').click();
    const opened = await page.evaluate(
      () => window.__mock.calls.find((c) => c.cmd === 'plugin:opener|open_path')?.args,
    );
    expect((opened as { path: string }).path).toContain('prompt-template.md');
  });
});

test.describe('notification preferences', () => {
  test('toggles persist and the test button fires one', async ({ page }) => {
    await page.addInitScript(installMock);
    await land(page);
    await page.getByRole('button', { name: '設定' }).click();
    await page.getByTestId('sec-notifications').click();

    // The defaults the core ships: blocked states on, turn endings off.
    await expect(page.getByTestId('notify-permission')).toBeChecked();
    await expect(page.getByTestId('notify-input')).toBeChecked();
    await expect(page.getByTestId('notify-done')).not.toBeChecked();

    // A preference is not a form: the click is the save.
    await page.getByTestId('notify-done').click();
    const stored = await page.evaluate(() => window.__mock.notifyPrefs);
    expect(stored.done).toBe(true);

    await page.getByTestId('notify-test').click();
    await expect(page.getByTestId('notify-test')).toHaveText('已送出 ✓');
    const fired = await page.evaluate(
      () => window.__mock.calls.filter((c) => c.cmd === 'test_notification').length,
    );
    expect(fired).toBe(1);
  });
});

test.describe('checkpoints', () => {
  test('the environment panel owns the switch, default on', async ({ page }) => {
    await page.addInitScript(installMock);
    await land(page);
    await page.getByRole('button', { name: '設定' }).click();
    await page.getByTestId('sec-sessions').click();

    // On by default — the retreat is the point; opting out is the choice.
    await expect(page.getByTestId('ckpt-toggle')).toBeChecked();

    await page.getByTestId('ckpt-toggle').click();
    const stored = await page.evaluate(() => window.__mock.checkpointsOn);
    expect(stored).toBe(false);
  });
});
