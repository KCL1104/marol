import { test, expect, type Page } from '@playwright/test';
import { installMock } from './mock-tauri';

const REPO = '/Users/test/picked-repo';

async function boot(page: Page) {
  await page.addInitScript(installMock);
  await page.goto('/');
  await expect(page.locator('.sidebar')).toBeVisible();
  await expect(page.locator('.tab')).toHaveCount(1);
}

async function toBoard(page: Page) {
  await page.getByTestId('view-board').click();
  await expect(page.getByTestId('board')).toBeVisible();
}

async function newCard(page: Page, title: string) {
  await page.getByRole('button', { name: '新增卡片', exact: true }).click();
  await expect(page.locator('.modal')).toBeVisible();
  await page.getByTestId('task-title').fill(title);
  await page.getByTestId('task-prompt').fill('把它修好');
  await page.getByTestId('task-repo').fill(REPO);
  await page.getByTestId('task-branch').fill('main');
  await page.getByTestId('task-create').click();
}

async function start(page: Page, taskId: string) {
  await page.locator(`[data-testid="task-${taskId}"] button.primary`).click();
  await expect(page.getByTestId('attempt-prompt')).toBeVisible();
  await page.getByTestId('attempt-start').click();
}

test.describe('the payoff click', () => {
  test('the waiting banner lands you in the terminal from any view', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await toBoard(page);
    await page.evaluate(() => window.__mock.report('s1', 'waiting_permission'));

    // The moment of highest urgency: the click has to visibly answer.
    await page.locator('.waiting-banner').click();
    await expect(page.getByTestId('board')).toHaveCount(0);
    await expect(page.locator('.pane:visible')).toHaveCount(1);
  });

  test('a sidebar row opens the terminal view too', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await toBoard(page);

    await page.locator('[data-testid="session-s1"]').click();
    await expect(page.getByTestId('board')).toHaveCount(0);
    await expect(page.locator('.pane:visible')).toHaveCount(1);
  });
});

test.describe('dialogs behave like dialogs', () => {
  test('Escape closes a clean dialog', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await page.getByRole('button', { name: '新增卡片', exact: true }).click();
    await expect(page.locator('.modal')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.locator('.modal')).toHaveCount(0);
  });

  test('a stray backdrop click cannot discard typed content', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await page.getByRole('button', { name: '新增卡片', exact: true }).click();
    await page.getByTestId('task-prompt').fill('好幾分鐘打出來的 prompt');

    // Dirty: the backdrop refuses.
    await page.locator('.modal-backdrop').click({ position: { x: 5, y: 5 } });
    await expect(page.locator('.modal')).toBeVisible();

    // Escape is deliberate in a way a mis-aimed click is not: it still works.
    await page.keyboard.press('Escape');
    await expect(page.locator('.modal')).toHaveCount(0);
  });

  /**
   * The new-card dialog opens fully visible, at the size the app's own window
   * opens at. It is the one dialog everybody meets first, and one that opens
   * already scrolled hides its own primary button behind a gesture.
   *
   * Pinned because it has been lost once: giving the card its second
   * repository added a button on its own row plus a paragraph explaining the
   * feature, and those two together pushed a dialog that had never scrolled
   * 96px past its own ceiling. The fix was to move the affordance onto the
   * label of the thing it repeats and let the README carry the explanation —
   * but nothing would have said so, because scrolling is not an error.
   */
  test('the new-card dialog opens without scrolling', async ({ page }) => {
    // The app's own default window, which is what a first run gets.
    await page.setViewportSize({ width: 1280, height: 820 });
    await boot(page);
    await toBoard(page);
    await page.getByRole('button', { name: '新增卡片', exact: true }).click();
    await expect(page.getByTestId('task-prompt')).toBeVisible();

    const modal = page.locator('.modal');
    const over = await modal.evaluate((el) => el.scrollHeight - el.clientHeight);
    expect(over, 'the new-card dialog opens already scrolled').toBe(0);
    // Including the button the whole dialog exists to reach.
    await expect(page.getByTestId('task-create')).toBeInViewport();
  });

  test('a clean backdrop click still closes', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await page.getByRole('button', { name: '新增卡片', exact: true }).click();
    await page.locator('.modal-backdrop').click({ position: { x: 5, y: 5 } });
    await expect(page.locator('.modal')).toHaveCount(0);
  });

  test('Tab stays inside an open dialog', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await page.getByRole('button', { name: '新增卡片', exact: true }).click();
    // Walk far enough to have wrapped at least once.
    for (let i = 0; i < 40; i++) {
      await page.keyboard.press('Tab');
      const inside = await page.evaluate(() =>
        document.querySelector('.modal')?.contains(document.activeElement),
      );
      expect(inside).toBe(true);
    }
  });
});

test.describe('the keyboard can drive', () => {
  test('a session row is focusable and Enter opens it', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await toBoard(page);

    // The door is the row's one honest tab stop.
    await page.locator('[data-testid="session-s1"] .row-door').focus();
    await page.keyboard.press('Enter');
    await expect(page.getByTestId('board')).toHaveCount(0);
    await expect(page.locator('.pane:visible')).toHaveCount(1);
  });

  test('row actions become visible when focus reaches them', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await toBoard(page);

    const actions = page.locator('[data-testid="session-s1"] .row-actions');
    await expect(actions).toHaveCSS('opacity', '0');
    await page.locator('[data-testid="session-s1"] .row-action').first().focus();
    await expect(actions).toHaveCSS('opacity', '1');
  });

  test('⌘/Ctrl+1/2/3 switch views', async ({ page }) => {
    await boot(page);
    await page.keyboard.press('ControlOrMeta+2');
    await expect(page.getByTestId('board')).toBeVisible();
    await page.keyboard.press('ControlOrMeta+3');
    await expect(page.locator('.ov-grid, .overview, [data-testid="overview"]').first()).toBeVisible();
    await page.keyboard.press('ControlOrMeta+1');
    await expect(page.getByTestId('board')).toHaveCount(0);
  });

  test('⌘/Ctrl+E jumps to the session that is waiting', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await toBoard(page);
    await page.evaluate(() => window.__mock.report('s1', 'waiting_permission'));

    await page.keyboard.press('ControlOrMeta+e');
    await expect(page.getByTestId('board')).toHaveCount(0);
    await expect(page.locator('.pane:visible')).toHaveCount(1);
  });

  test('⌘/Ctrl+Alt+arrows cycle the focused pane', async ({ page }) => {
    await boot(page);
    // Two ad-hoc sessions straight into the wall.
    for (const dir of ['/Users/test/repo-one', '/Users/test/repo-two']) {
      await page.locator('.sidebar-head button.icon').click();
      await page.locator('.modal input.mono').first().fill(dir);
      await page.locator('.modal button.primary').click();
    }
    await expect(page.locator('.pane:visible')).toHaveCount(2);
    await expect(page.locator('.pane.focused')).toHaveAttribute('data-session-id', 's2');

    await page.keyboard.press('ControlOrMeta+Alt+ArrowRight');
    await expect(page.locator('.pane.focused')).toHaveAttribute('data-session-id', 's1');
    await page.keyboard.press('ControlOrMeta+Alt+ArrowLeft');
    await expect(page.locator('.pane.focused')).toHaveAttribute('data-session-id', 's2');
  });

  test('Ctrl+PgDn / PgUp cycle tabs', async ({ page }) => {
    await boot(page);
    await page.locator('.tab-add').click();
    await expect(page.locator('.tab')).toHaveCount(2);
    // A fresh tab opens in rename mode; keep the offered name and move on.
    await page.keyboard.press('Escape');
    await expect(page.locator('.tab.active')).toContainText('工作區 2');

    await page.keyboard.press('Control+PageDown');
    await expect(page.locator('.tab.active')).toContainText('工作區');
    await page.keyboard.press('Control+PageUp');
    await expect(page.locator('.tab.active')).toContainText('工作區 2');
  });

  test('⌘/Ctrl+I toggles the inspector beside the terminal', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await expect(page.locator('.pane:visible')).toHaveCount(1);
    await page.evaluate(() => {
      window.__mock.diffs.set(
        'k1-a1',
        ['diff --git a/a.py b/a.py', '--- a/a.py', '+++ b/a.py', '@@ -1 +1,2 @@', ' x', '+y'].join(
          '\n',
        ),
      );
    });

    // From outside the terminal the plain chord works; inside it, readline
    // owns Ctrl+I (it is Tab), so the Shift variant is the one that fires.
    await page.locator('.topbar').click();
    await page.keyboard.press('ControlOrMeta+i');
    await expect(page.getByTestId('inspector')).toBeVisible();
    // The chord finishes its own journey: J/K walk the diff immediately,
    // with no mouse trip to earn the focus first.
    await expect(page.getByTestId('diff-body')).toBeFocused();
    await page.keyboard.press('ControlOrMeta+i');
    await expect(page.getByTestId('inspector')).toHaveCount(0);

    await page.locator('.term-host').first().click();
    await page.keyboard.press('ControlOrMeta+Shift+I');
    await expect(page.getByTestId('inspector')).toBeVisible();
  });

  test('⌘/Ctrl+/ shows the cheat sheet and Escape puts it away', async ({ page }) => {
    await boot(page);
    await page.keyboard.press('ControlOrMeta+/');
    await expect(page.getByTestId('shortcuts')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('shortcuts')).toHaveCount(0);
  });

  /**
   * 滾輪修好之後仍然成立的那件事:在 agent 的整頁畫面裡,捲動是那個
   * CLI 的事,不是 Marol 的。所以表上把它們分開列 —— 同一張表混在一起
   * 會讓人以為這個 app 改得動 Ctrl+T。
   */
  test('the sheet names the agent’s own keys, apart from Marol’s own', async ({ page }) => {
    await boot(page);
    await page.keyboard.press('ControlOrMeta+/');
    await expect(page.getByTestId('shortcuts')).toBeVisible();

    const agent = page.getByTestId('agent-keys');
    await expect(agent).toBeVisible();
    await expect(agent).toContainText('Ctrl + T');
    await expect(agent).toContainText('codex');
    // 分開的兩張表:agent 的鍵不能出現在 Marol 自己那張上。
    await expect(page.getByTestId('shortcuts')).not.toContainText('Ctrl + T');
  });

  test('j and k walk the commentable diff lines', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await toBoard(page);
    await page.evaluate(() => {
      window.__mock.diffs.set('k1-a1', [
        'diff --git a/src/auth.py b/src/auth.py',
        '--- a/src/auth.py',
        '+++ b/src/auth.py',
        '@@ -10,3 +10,4 @@',
        ' def login(request):',
        '+    session = make_session(user)',
        '+    return session',
      ].join('\n'));
    });
    await page.getByTestId('inspect-k1').click();
    await expect(page.getByTestId('diff-body')).toBeVisible();

    await page.getByTestId('diff-body').focus();
    await page.keyboard.press('j');
    await expect(page.locator('.diff-line.commentable').first()).toBeFocused();
    await page.keyboard.press('j');
    await expect(page.locator('.diff-line.commentable').nth(1)).toBeFocused();
    await page.keyboard.press('k');
    await expect(page.locator('.diff-line.commentable').first()).toBeFocused();

    // Enter on the focused line opens the composer for exactly that line.
    await page.keyboard.press('Enter');
    await expect(page.getByTestId('review-note')).toBeVisible();
  });

  test('a board card is enterable with Enter', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await toBoard(page);

    // The card's title is the door — a real button, not a clickable article.
    await page.locator('[data-testid="task-k1"] .card-door').focus();
    await page.keyboard.press('Enter');
    await expect(page.getByTestId('board')).toHaveCount(0);
    await expect(page.locator('.pane:visible')).toHaveCount(1);
  });
});

test.describe('signals reach everyone', () => {
  test('an agent starting to wait is announced to assistive tech', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await toBoard(page);

    // Starting an attempt already announces once: the folder-trust prompt is
    // itself a human being waited on.
    await expect(page.getByTestId('live-announce')).toContainText('等你確認資料夾');
    await page.evaluate(() => window.__mock.report('s1', 'running'));
    await page.evaluate(() => window.__mock.report('s1', 'waiting_permission'));
    await expect(page.getByTestId('live-announce')).toContainText('等你授權');
    await expect(page.getByTestId('live-announce')).toHaveAttribute('aria-live', 'polite');
  });

  test('⌘/Ctrl+E cycles through everything that waits', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '第一張');
    await newCard(page, '第二張');
    await start(page, 'k1');
    await toBoard(page);
    await start(page, 'k2');
    await toBoard(page);
    await page.evaluate(() => {
      window.__mock.report('s1', 'waiting_permission');
      window.__mock.report('s2', 'waiting_permission');
    });

    await page.keyboard.press('ControlOrMeta+e');
    await expect(page.locator('.pane.focused')).toHaveAttribute('data-session-id', 's1');
    // Focus is now inside s1's terminal, where plain Ctrl+E belongs to
    // readline — the documented in-terminal variant carries Shift.
    await page.keyboard.press('ControlOrMeta+Shift+E');
    await expect(page.locator('.pane.focused')).toHaveAttribute('data-session-id', 's2');
    await page.keyboard.press('ControlOrMeta+Shift+E');
    await expect(page.locator('.pane.focused')).toHaveAttribute('data-session-id', 's1');
  });

  /**
   * ⌘E 的另一半。E 去該去的地方,L 回剛才那個 —— 答完一個插隊的 agent
   * 之後,回去繼續原本在做的事,是這個迴圈少掉的那一步。
   */
  test('⌘/Ctrl+L goes back to the session you were on before', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '第一張');
    await newCard(page, '第二張');
    await start(page, 'k1');
    await toBoard(page);
    await start(page, 'k2');
    await expect(page.locator('.pane.focused')).toHaveAttribute('data-session-id', 's2');

    // 在終端機裡,所以走加了 Shift 的那一顆 —— 無 Shift 的 Ctrl+L 屬於 shell。
    await page.keyboard.press('ControlOrMeta+Shift+L');
    await expect(page.locator('.pane.focused')).toHaveAttribute('data-session-id', 's1');
    // 回頭路本身也有回頭路:再按一次回到 s2。
    await page.keyboard.press('ControlOrMeta+Shift+L');
    await expect(page.locator('.pane.focused')).toHaveAttribute('data-session-id', 's2');
  });

  test('workspace tabs answer the keyboard', async ({ page }) => {
    await boot(page);
    await page.locator('.tab-add').click();
    await page.keyboard.press('Escape');
    await expect(page.locator('.tab.active')).toContainText('工作區 2');

    // The strip is one tab stop; arrows move the selection.
    await page.locator('[data-testid="tab-t2"]').focus();
    await page.keyboard.press('ArrowRight');
    await expect(page.locator('.tab.active')).toContainText('工作區');

    // Enter on the focused tab opens rename.
    await page.keyboard.press('Enter');
    await expect(page.locator('.tab-rename')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.locator('.tab-rename')).toHaveCount(0);
  });

  test('the PATH renders as separate chips, not one fused path', async ({ page }) => {
    await boot(page);
    await page.locator('.sidebar-foot').click();
    await page.getByTestId('sec-diagnostics').click();
    await expect(page.locator('.modal .chips .chip')).toHaveCount(3);
    await expect(page.locator('.modal .chips .chip').first()).toHaveCSS(
      'border-radius',
      '5px',
    );
  });
});

test.describe('the accessibility tree tells the truth', () => {
  test('dialogs carry their own names', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await page.getByRole('button', { name: '新增卡片', exact: true }).click();
    const dialog = page.locator('[role="dialog"]');
    // aria-labelledby resolves to the h2, so the dialog announces as its
    // title, not as an anonymous "dialog".
    await expect(dialog).toHaveAttribute('aria-labelledby', /.+/);
    const labelled = await page.evaluate(() => {
      const d = document.querySelector('[role="dialog"]')!;
      const id = d.getAttribute('aria-labelledby')!;
      return document.getElementById(id)?.textContent ?? '';
    });
    expect(labelled).toContain('新增卡片');
  });

  test('rows and cards are groups holding one door, not nested buttons', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await toBoard(page);

    await expect(page.getByTestId('task-k1')).toHaveAttribute('role', 'group');
    await expect(page.locator('[data-testid="task-k1"] .card-door')).toHaveCount(1);
    await expect(page.getByTestId('session-s1')).toHaveAttribute('role', 'group');
    // Click-anywhere survives the restructuring: the door stretches.
    await page.locator('[data-testid="task-k1"]').click({ position: { x: 10, y: 60 } });
    await expect(page.getByTestId('board')).toHaveCount(0);
  });

  test('every tablist keeps the one-stop contract', async ({ page }) => {
    await boot(page);
    // Topbar view switcher: only the active tab is in the tab order.
    await expect(page.getByRole('tab', { name: /終端機/ })).toHaveAttribute('tabindex', '0');
    await expect(page.getByTestId('view-board')).toHaveAttribute('tabindex', '-1');

    // Inspector tabs: roving plus arrows.
    await toBoard(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await toBoard(page);
    await page.getByTestId('inspect-k1').click();
    await expect(page.getByTestId('inspector-diff-tab')).toHaveAttribute('tabindex', '0');
    await expect(page.getByTestId('inspector-timeline-tab')).toHaveAttribute('tabindex', '-1');
    await page.getByTestId('inspector-diff-tab').focus();
    await page.keyboard.press('ArrowRight');
    await expect(page.getByTestId('inspector-timeline-tab')).toHaveAttribute(
      'aria-selected',
      'true',
    );
  });

  test('the permission mode wears a visible label', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await page.locator('[data-testid="task-k1"] button.primary').click();
    await expect(page.getByText('權限模式')).toBeVisible();
    await expect(page.getByLabel('權限模式')).toHaveValue('normal');
  });
});

test.describe('outcomes say so', () => {
  test('merging an attempt gets its confirmation toast', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await toBoard(page);
    await page.getByTestId('inspect-k1').click();
    await expect(page.getByTestId('inspector')).toBeVisible();

    // Merge arms first — it mutates the base branch, the heaviest act here.
    await page.getByTestId('merge-attempt').click();
    await expect(page.getByTestId('confirm-merge')).toContainText('確定合併回 main');
    await page.getByTestId('confirm-merge').click();
    await expect(page.locator('.toast.ok')).toContainText('已合併回 main');
  });

  test('the delete arm disarms by itself', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');

    await page.getByRole('button', { name: '刪除卡片' }).click();
    await expect(page.getByTestId('confirm-delete-k1')).toBeVisible();
    // Not clicking again: the armed state must give up on its own.
    await expect(page.getByRole('button', { name: '刪除卡片' })).toBeVisible({
      timeout: 6000,
    });
    await expect(page.locator('.board-card')).toHaveCount(1);
  });

  test('a card moves between columns from the keyboard', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await expect(page.locator('[data-testid="col-backlog"] .board-card')).toHaveCount(1);

    await page.locator('[data-testid="task-k1"]').focus();
    await page.keyboard.press('ControlOrMeta+ArrowRight');
    await expect(page.locator('[data-testid="col-running"] .board-card')).toHaveCount(1);
    // The move is spoken, and focus follows the card into its new column.
    await expect(page.getByTestId('live-announce')).toContainText('移到');
    await expect(page.locator('[data-testid="task-k1"]')).toBeFocused();

    await page.keyboard.press('ControlOrMeta+ArrowLeft');
    await expect(page.locator('[data-testid="col-backlog"] .board-card')).toHaveCount(1);
  });

  test('a card moves within its column from the keyboard', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await newCard(page, '寫測試');
    const inBacklog = page.locator('[data-testid="col-backlog"] .board-card');
    await expect(inBacklog).toHaveCount(2);
    await expect(inBacklog.first()).toHaveAttribute('data-testid', 'task-k1');

    // The order the drag could always say, sayable by the keyboard too.
    await page.locator('[data-testid="task-k2"]').focus();
    await page.keyboard.press('ControlOrMeta+ArrowUp');
    await expect(inBacklog.first()).toHaveAttribute('data-testid', 'task-k2');
    await expect(page.getByTestId('live-announce')).toContainText('移到第 1 位');
    await expect(page.locator('[data-testid="task-k2"]')).toBeFocused();

    // The edge is a wall, not a wrap: first place stays first.
    await page.keyboard.press('ControlOrMeta+ArrowUp');
    await expect(inBacklog.first()).toHaveAttribute('data-testid', 'task-k2');

    await page.keyboard.press('ControlOrMeta+ArrowDown');
    await expect(inBacklog.first()).toHaveAttribute('data-testid', 'task-k1');
  });

  test('two agents blocking at once are both announced', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '第一張');
    await newCard(page, '第二張');
    await start(page, 'k1');
    await toBoard(page);
    await start(page, 'k2');
    await toBoard(page);

    // One update, two sessions entering a blocked state: both are spoken.
    await page.evaluate(() => {
      for (const id of ['s1', 's2']) {
        const s = window.__mock.sessions.find((x) => x.id === id);
        if (s) s.status = 'waiting_permission';
      }
      window.__mock.emit('sessions:changed', window.__mock.sorted());
    });
    await expect(page.getByTestId('live-announce')).toContainText('2 個 session 等你');
  });

  test('overview cards wear the card title and open on one click', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await page.getByRole('tab', { name: '總覽' }).click();

    const card = page.getByTestId('card-s1');
    await expect(card.locator('.ov-title')).toHaveText('修好登入 #1');
    await card.click();
    await expect(page.locator('.pane:visible')).toHaveCount(1);
  });

  test('diff lines are one tab stop, and the state reaches AT labels', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await toBoard(page);
    await page.evaluate(() => {
      window.__mock.report('s1', 'waiting_permission');
      window.__mock.diffs.set('k1-a1', [
        'diff --git a/src/auth.py b/src/auth.py',
        'index 1111111..2222222 100644',
        '--- a/src/auth.py',
        '+++ b/src/auth.py',
        '@@ -10,3 +10,4 @@',
        ' def login(request):',
        '+    session = make_session(user)',
        '+    return session',
      ].join('\n'));
    });

    // The card's accessible name carries its blocked state — the one thing
    // the breathing card shouts must not be silent to AT.
    await expect(page.getByTestId('task-k1')).toHaveAttribute('aria-label', /等你授權/);
    // And the number triage runs on is on the card itself.
    await expect(page.getByTestId('state-k1')).toContainText('·');

    await page.getByTestId('inspect-k1').click();
    await expect(page.getByTestId('diff-body')).toBeVisible();
    // Roving focus: lines are reachable by j/k, never by Tab — a 300-line
    // diff must not be 300 tab stops in front of the merge button.
    await expect(page.locator('.diff-line.commentable').first()).toHaveAttribute(
      'tabindex',
      '-1',
    );
    // The plumbing is a chip now: filename + weights, sticky over its hunks.
    await expect(page.locator('.diff-file')).toContainText('src/auth.py');
    await expect(page.locator('.diff-file .diff-count.add')).toHaveText('+2');
    await expect(page.getByTestId('diff-body')).not.toContainText('index 1111111');
  });

  test('a closed session does not sit in 等待輸入', async ({ page }) => {
    await boot(page);
    await toBoard(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await toBoard(page);
    await page.evaluate(() => window.__mock.report('s1', 'waiting_permission'));
    await expect(page.locator('[data-section="waiting"] .session-row')).toHaveCount(1);

    await page.evaluate(() => {
      const s = window.__mock.sessions.find((x) => x.id === 's1');
      if (s) {
        s.live = false;
        s.status = 'saved';
      }
      window.__mock.emit('sessions:changed', window.__mock.sorted());
    });
    // 已完成 starts collapsed, so count the section's own badge, and the
    // now-empty waiting section unrenders entirely.
    await expect(page.locator('[data-section="done"] .section-count')).toHaveText('1');
    await expect(page.locator('[data-section="waiting"]')).toHaveCount(0);
  });
});
