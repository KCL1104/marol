import { test, expect, type Page } from '@playwright/test';
import { installMock } from './mock-tauri';

const REPO = '/Users/test/picked-repo';

const DIFF = [
  'diff --git a/src/auth.py b/src/auth.py',
  '--- a/src/auth.py',
  '+++ b/src/auth.py',
  '@@ -1,3 +1,3 @@',
  ' def login():',
  '-    return None',
  '+    return session',
].join('\n');

async function reviewMidTurn(page: Page) {
  await page.addInitScript(installMock);
  await page.goto('/');
  await expect(page.locator('.tab')).toHaveCount(1);
  await page.getByTestId('view-board').click();

  await page.getByRole('button', { name: '新增卡片', exact: true }).click();
  await page.getByTestId('task-title').fill('修好登入');
  await page.getByTestId('task-prompt').fill('把它修好');
  await page.getByTestId('task-repo').fill(REPO);
  await page.getByTestId('task-branch').fill('main');
  await page.getByTestId('task-create').click();
  await page.locator('[data-testid="task-k1"] button.primary').click();
  await page.getByTestId('attempt-start').click();
  await expect(page.locator('.pane:visible')).toHaveCount(1);

  // Mid-turn: the agent is working while the review is being written.
  await page.evaluate((d) => {
    window.__mock.report('s1', 'running');
    window.__mock.diffs.set('k1-a1', d);
  }, DIFF);
  await page.getByTestId('view-board').click();
  await page.getByTestId('inspect-k1').click();
  await expect(page.getByTestId('diff-body')).toBeVisible();

  await page.locator('.diff-line.add').click();
  await page.getByTestId('review-note').fill('session 可能是 undefined');
  await page.getByTestId('review-add').click();
}

/**
 * VK's queue-a-follow-up, Marol-shaped: mid-turn, the review batch
 * holds for Stop instead of steering the turn it reviews — and arrives as
 * the next one, about a diff that has stopped moving.
 */
test.describe('the queued follow-up', () => {
  test('mid-turn, the batch queues; Stop spends it onto the timeline', async ({ page }) => {
    await reviewMidTurn(page);

    // The button says what will actually happen.
    await expect(page.getByTestId('review-send')).toHaveText(/這輪結束後送出 1 則/);
    await page.getByTestId('review-send').click();

    // Held, visibly, with a way to change your mind.
    await expect(page.getByTestId('queued-followup')).toBeVisible();
    await expect(page.getByTestId('review-pending')).toHaveCount(0);

    // Stop lands: the message goes in and the banner goes with it.
    await page.evaluate(() => window.__mock.report('s1', 'idle'));
    await expect(page.getByTestId('queued-followup')).toHaveCount(0);
    await page.getByTestId('inspector-timeline-tab').click();
    await expect(page.locator('.tl-row.tl-prompt').last()).toContainText(
      'session 可能是 undefined',
    );
  });

  test('cancelling takes it back before Stop can spend it', async ({ page }) => {
    await reviewMidTurn(page);
    await page.getByTestId('review-send').click();
    await expect(page.getByTestId('queued-followup')).toBeVisible();

    await page.getByTestId('cancel-followup').click();
    await expect(page.getByTestId('queued-followup')).toHaveCount(0);

    await page.evaluate(() => window.__mock.report('s1', 'idle'));
    await page.getByTestId('inspector-timeline-tab').click();
    // Only the opening prompt — nothing was sent behind anyone's back.
    await expect(page.locator('.tl-row.tl-prompt')).toHaveCount(1);
  });

  test('an idle session sends now, exactly as before', async ({ page }) => {
    await reviewMidTurn(page);
    await page.evaluate(() => window.__mock.report('s1', 'idle'));
    await expect(page.getByTestId('review-send')).toHaveText(/送出 1 則意見給 agent/);
  });
});

/**
 * 另一個 session 送來的訊息,在人這一側。
 *
 * 兩件事要分開說。抽屜的橫幅原本一句話同時描述「你自己留了張紙條」和
 * 「兩個 agent 在等這個回合結束」—— 那是兩種處境。而時間線原本把中繼
 * 來的訊息記成 prompt,等於對事後讀紀錄的人說「這句話是你講的」,正是
 * 信封在終端機裡擋掉的那個謊,換一個對象再說一次。
 */
test.describe('a message from another session, on the person’s side', () => {
  test('the timeline says who actually spoke, and offers no restore for it', async ({ page }) => {
    // 事件要在抽屜打開之前種好:`refresh` 綁的是 attempt,不是分頁 ——
    // 一份在你讀的時候自己重排的紀錄比一份要重開才更新的更糟。
    await page.addInitScript(installMock);
    await page.goto('/');
    await expect(page.locator('.tab')).toHaveCount(1);
    await page.getByTestId('view-board').click();
    await page.getByRole('button', { name: '新增卡片', exact: true }).click();
    await page.getByTestId('task-title').fill('修好登入');
    await page.getByTestId('task-prompt').fill('把它修好');
    await page.getByTestId('task-repo').fill(REPO);
    await page.getByTestId('task-branch').fill('main');
    await page.getByTestId('task-create').click();
    await page.locator('[data-testid="task-k1"] button.primary').click();
    await page.getByTestId('attempt-start').click();
    await page.evaluate(() => {
      window.__mock.events.set('k1-a1', [
        { id: 1, attempt_id: 'k1-a1', at: Date.now() - 5000, kind: 'prompt', tool: null, detail: '把它修好' },
        {
          id: 2,
          attempt_id: 'k1-a1',
          at: Date.now(),
          kind: 'message',
          tool: '移植測試 #1',
          detail: 'auth.py 我在動，先別碰',
        },
      ]);
    });
    await page.getByTestId('view-board').click();
    await page.getByTestId('inspect-k1').click();
    await page.getByTestId('inspector-timeline-tab').click();

    const row = page.locator('.tl-row.tl-message');
    await expect(row).toHaveCount(1);
    await expect(row).toContainText('移植測試 #1');
    await expect(row).toContainText('auth.py 我在動，先別碰');
    // 它不是 prompt,所以不戴 prompt 的外觀,也沒有 ↩ ——「回到這個回合
    // 之前」錨定的是人開始的回合,而這不是其中之一。
    await expect(page.locator('.tl-row.tl-prompt')).toHaveCount(1);
    await expect(row.locator('button')).toHaveCount(0);
  });

  test('the banner names who is waiting, instead of calling it your own note', async ({ page }) => {
    await reviewMidTurn(page);
    await page.evaluate(() => {
      const s = window.__mock.sessions.find((x) => x.id === 's1');
      if (!s) throw new Error('no session s1');
      s.has_followup = true;
      s.pending_from = ['移植測試 #1', '修文件 #2'];
      window.__mock.emit('sessions:changed', window.__mock.sessions);
    });

    const banner = page.getByTestId('queued-followup');
    await expect(banner).toContainText('移植測試 #1');
    await expect(banner).toContainText('修文件 #2');
    await expect(banner).toContainText('正在等這個回合結束');
    // 你自己那張紙條的句子不該同時出現 —— 那是另一種處境。
    await expect(banner).not.toContainText('一則訊息會在這個回合結束後送出');
    // 改變心意的那條路還在:訊息是誰送的都一樣,擋下它的是這個人。
    await expect(page.getByTestId('cancel-followup')).toBeVisible();
  });
});
