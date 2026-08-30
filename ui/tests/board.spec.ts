import { test, expect, type Page } from '@playwright/test';
import { installMock } from './mock-tauri';

const REPO = '/Users/test/picked-repo';

async function boot(page: Page) {
  await page.addInitScript(installMock);
  await page.goto('/');
  await expect(page.locator('.sidebar')).toBeVisible();
  await expect(page.locator('.tab')).toHaveCount(1);
  await page.getByTestId('view-board').click();
  await expect(page.getByTestId('board')).toBeVisible();
}

async function newCard(page: Page, title: string, repo = REPO, branch = 'main') {
  await page.getByRole('button', { name: '新增卡片', exact: true }).click();
  await expect(page.locator('.modal')).toBeVisible();
  await page.getByTestId('task-title').fill(title);
  await page.getByTestId('task-prompt').fill('把它修好');
  await page.getByTestId('task-repo').fill(repo);
  await page.getByTestId('task-branch').fill(branch);
  await page.getByTestId('task-create').click();
}

/** Start an attempt on the only card, accepting the prompt as offered. */
async function start(page: Page, taskId: string, agent = 'claude') {
  await page.locator(`[data-testid="task-${taskId}"] button.primary`).click();
  await expect(page.getByTestId('attempt-prompt')).toBeVisible();
  if (agent !== 'claude') await page.getByTestId('attempt-agent').selectOption(agent);
  await page.getByTestId('attempt-start').click();
}

/**
 * Drag a card onto a column, or onto another card to insert before it.
 *
 * Synthetic HTML5 drag events, dispatched in one tick. That is stricter than
 * a real drag, where React has many frames to re-render between `dragstart`
 * and `drop` — so a drop that only works because state had settled in between
 * fails here, which is the point.
 */
async function dragCardTo(page: Page, taskId: string, target: string) {
  await page.evaluate(
    ({ taskId, target }) => {
      const card = document.querySelector(`[data-testid="task-${taskId}"]`)!;
      const onto = document.querySelector(`[data-testid="${target}"]`)!;
      const dt = new DataTransfer();
      card.dispatchEvent(new DragEvent('dragstart', { dataTransfer: dt, bubbles: true }));
      onto.dispatchEvent(new DragEvent('dragover', { dataTransfer: dt, bubbles: true }));
      onto.dispatchEvent(new DragEvent('drop', { dataTransfer: dt, bubbles: true }));
      card.dispatchEvent(new DragEvent('dragend', { dataTransfer: dt, bubbles: true }));
    },
    { taskId, target },
  );
}

test.describe('one dialog, one act', () => {
  test('建立並開始 makes the card, runs it, and lands in the terminal', async ({ page }) => {
    await boot(page);
    await page.getByRole('button', { name: '新增卡片', exact: true }).click();
    await expect(page.locator('.modal')).toBeVisible();
    await page.getByTestId('task-title').fill('修好登入');
    await page.getByTestId('task-prompt').fill('把它修好');
    await page.getByTestId('task-repo').fill(REPO);
    await page.getByTestId('task-branch').fill('main');

    // The agent and the permission mode are in this dialog now: there is no
    // second one to answer.
    await expect(page.getByTestId('task-agent')).toBeVisible();
    await expect(page.getByTestId('task-mode')).toBeVisible();

    // Enter inside the prompt is a newline; ⌘/Ctrl+Enter is the primary
    // action, which is the one printed on the button.
    await page.getByTestId('task-prompt').click();
    await page.keyboard.press('Enter');
    await expect(page.locator('.modal')).toHaveCount(1);
    await page.keyboard.press('ControlOrMeta+Enter');

    // Straight into the TUI — no board stop, no start dialog.
    await expect(page.getByTestId('attempt-prompt')).toHaveCount(0);
    await expect(page.locator('.pane[data-session-id="s1"]')).toHaveClass(/focused/);

    // And the card is real, in 進行中, with the attempt behind it.
    await page.getByTestId('view-board').click();
    await expect(page.getByTestId('col-running').getByTestId('task-k1')).toBeVisible();
  });

  test('放進待辦 still files a card without running anything', async ({ page }) => {
    await boot(page);
    await newCard(page, '晚點再說');
    await expect(page.getByTestId('col-backlog').getByTestId('task-k1')).toBeVisible();
    await expect(page.locator('.pane[data-session-id]')).toHaveCount(0);
    await expect(page.getByTestId('state-k1')).toHaveText(/尚未開始/);
  });

  test('an unmeasured CLI gets the session and not the prompt, with no second dialog', async ({
    page,
  }) => {
    await boot(page);
    await page.getByRole('button', { name: '新增卡片', exact: true }).click();
    await page.getByTestId('task-prompt').fill('把它修好');
    await page.getByTestId('task-repo').fill(REPO);
    await page.getByTestId('task-branch').fill('main');
    await page.getByTestId('task-agent').selectOption('gemini');
    // The mode picker belongs to measured CLIs only, and withdraws with them.
    await expect(page.getByTestId('task-mode')).toHaveCount(0);
    await page.getByTestId('task-start').click();

    await expect(page.locator('.pane[data-session-id="s1"]')).toHaveClass(/focused/);
    const sent = await page.evaluate(() =>
      window.__mock.calls.filter((c) => c.cmd === 'open_attempt').map((c) => c.args),
    );
    expect(sent).toHaveLength(1);
    expect((sent[0] as { prompt: string | null }).prompt).toBeNull();
  });
});

test.describe('board', () => {
  test('a new card lands in 待辦 and nothing is running behind it', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入');

    const card = page.getByTestId('task-k1');
    await expect(page.locator('[data-testid="col-backlog"] .board-card')).toHaveCount(1);
    await expect(card).toContainText('修好登入');
    await expect(page.getByTestId('state-k1')).toHaveText(/尚未開始/);
  });

  /**
   * The acceptance criterion for the whole two-axis idea: a card sitting in a
   * column lights up by itself when its agent is blocked, without anyone
   * opening anything.
   */
  test('a card in 進行中 lights up by itself when its agent needs you', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');

    // Starting an attempt moves the card, and the very first thing a fresh
    // worktree does is ask whether you trust the folder.
    await page.getByTestId('view-board').click();
    await expect(page.locator('[data-testid="col-running"] .board-card')).toHaveCount(1);
    await expect(page.getByTestId('state-k1')).toHaveText(/等你確認資料夾/);
    await expect(page.getByTestId('task-k1')).toHaveClass(/needs-you/);

    // Trust answered: the hook takes over and the card calms down.
    await page.evaluate(() => window.__mock.report('s1', 'running', { tool: 'Bash', detail: 'pytest -v' }));
    await expect(page.getByTestId('state-k1')).toHaveText(/執行中/);
    await expect(page.getByTestId('task-k1')).not.toHaveClass(/needs-you/);
    await expect(page.getByTestId('task-k1')).toContainText('pytest -v');

    // And it lights up again the moment a permission prompt appears — the
    // card is still in 進行中 throughout. Nothing moved it.
    await page.evaluate(() => window.__mock.report('s1', 'waiting_permission'));
    await expect(page.getByTestId('state-k1')).toHaveText(/等你授權/);
    await expect(page.getByTestId('state-k1').locator('.icon-glyph')).toBeVisible();
    await expect(page.getByTestId('task-k1')).toHaveClass(/needs-you/);
    await expect(page.locator('[data-testid="col-running"] .board-card')).toHaveCount(1);
  });

  /**
   * Hooks belong to the CLIs that have them. For an agent without one the
   * card's calm face is unverified, and 「安靜」 read as 「沒事」 is exactly
   * the lie the two-axis board exists to avoid — so the card says it cannot
   * tell.
   */
  test('a hookless agent’s card admits it has no status signal', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1', 'gemini');
    await page.getByTestId('view-board').click();

    await expect(page.getByTestId('nosignal-k1')).toHaveText('沒有狀態回報');

    // The first real report retires the disclaimer for good.
    await page.evaluate(() => window.__mock.report('s1', 'running'));
    await expect(page.getByTestId('nosignal-k1')).toHaveCount(0);
  });

  /**
   * The gap this closes: being a CLI Marol *knows* is not the same as being
   * one it *wired*. A codex older than its own hooks engine runs a session
   * perfectly and never says a word — and the card used to withhold the
   * disclaimer precisely because codex is measured, so a card that would
   * never report was indistinguishable from one working quietly. On the one
   * surface whose job is to be believed at a glance, that is the worst
   * possible failure.
   */
  test('a measured CLI that was never wired for status says so too', async ({ page }) => {
    await boot(page);
    await page.evaluate(() => {
      window.__mock.unwiredAgents = ['codex'];
    });
    await newCard(page, '修好登入');
    await start(page, 'k1', 'codex');
    await page.getByTestId('view-board').click();

    await expect(page.getByTestId('nosignal-k1')).toHaveText('沒有狀態回報');
    // And it names the reason, which is a version rather than the CLI.
    await expect(page.getByTestId('nosignal-k1')).toHaveAttribute('title', /codex/);
  });

  // One test per CLI rather than a loop inside one: the board keeps its
  // state across a reload, so a second card in the same page is `k2` and
  // the assertions would quietly move off the card they were written for.
  for (const agent of ['claude', 'codex']) {
    test(`a ${agent} card never wears the no-signal chip`, async ({ page }) => {
      await boot(page);
      await newCard(page, '修好登入');
      await start(page, 'k1', agent);
      await page.getByTestId('view-board').click();

      // Fresh and silent, but its silence is trustworthy: hooks will speak.
      // A chip that appeared here and withdrew itself on the first report
      // would be a flicker on the surface whose whole job is to be believed
      // at a glance.
      await expect(page.locator('[data-testid="col-running"] .board-card')).toHaveCount(1);
      await expect(page.getByTestId('nosignal-k1')).toHaveCount(0);
    });
  }

  /**
   * The other half of the same criterion: the card is a way *into* the live
   * terminal, not a summary of it. Clicking has to leave the caret in the
   * TUI, because the next thing you do is answer it.
   */
  test('clicking a waiting card lands in its live TUI with the caret in it', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await page.getByTestId('view-board').click();
    await page.evaluate(() => window.__mock.report('s1', 'waiting_permission'));

    await page.getByTestId('task-k1').click();

    // We are on the terminal view, showing that session's pane, focused.
    await expect(page.locator('.pane[data-session-id="s1"]')).toBeVisible();
    await expect(page.locator('.pane[data-session-id="s1"]')).toHaveClass(/focused/);

    // And the real caret is inside it, so the answer can just be typed.
    await expect
      .poll(() =>
        page.evaluate(() => {
          const pane = document.querySelector('.pane[data-session-id="s1"]');
          return !!pane && !!document.activeElement && pane.contains(document.activeElement);
        }),
      )
      .toBe(true);
  });

  test('dragging a card to another column moves it, and only a drag does', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入');

    await dragCardTo(page, 'k1', 'col-review');
    await expect(page.locator('[data-testid="col-review"] .board-card')).toHaveCount(1);
    await expect(page.locator('[data-testid="col-backlog"] .board-card')).toHaveCount(0);

    // A hook report never moves a card. `Stop` means the turn ended, not that
    // the work is done, and that distance is the reason for two axes.
    await start(page, 'k1');
    await page.getByTestId('view-board').click();
    await page.evaluate(() => window.__mock.report('s1', 'idle'));
    await expect(page.getByTestId('state-k1')).toHaveText(/待命/);
    await expect(page.locator('[data-testid="col-running"] .board-card')).toHaveCount(1);
    await expect(page.locator('[data-testid="col-done"] .board-card')).toHaveCount(0);
  });

  test('reordering within a column sticks, and survives a reload', async ({ page }) => {
    await boot(page);
    await newCard(page, '第一張');
    await newCard(page, '第二張');
    await newCard(page, '第三張');

    const titles = () =>
      page.locator('[data-testid="col-backlog"] .board-card-title').allTextContents();
    expect(await titles()).toEqual(['第一張', '第二張', '第三張']);

    // Dropped onto the first card, so it goes in front of it.
    await dragCardTo(page, 'k3', 'task-k1');
    expect(await titles()).toEqual(['第三張', '第一張', '第二張']);

    await page.reload();
    await page.getByTestId('view-board').click();
    expect(await titles()).toEqual(['第三張', '第一張', '第二張']);
  });

  /**
   * After every restart this is what the board looks like, so it has to be a
   * first-class state rather than something that reads as broken.
   */
  test('an attempt with no terminal says so and offers to continue', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await page.getByTestId('view-board').click();

    await page.evaluate(() => void window.__mock);
    await page.locator('.pane[data-session-id="s1"] [data-testid="eject-s1"]').count();
    // The PTY dies, as it would on quit.
    await page.evaluate(() => {
      const s = window.__mock.sessions.find((x) => x.id === 's1')!;
      s.live = false;
      s.status = 'saved';
      window.__mock.pushSessions();
    });

    await expect(page.getByTestId('state-k1')).toHaveText(/未執行/);
    await expect(page.getByTestId('task-k1')).not.toHaveClass(/needs-you/);

    // One click puts a terminal back on it and takes you there.
    await page.getByTestId('resume-k1').click();
    await expect(page.locator('.pane[data-session-id="s1"]')).toHaveClass(/focused/);
    await page.getByTestId('view-board').click();
    await expect(page.getByTestId('state-k1')).toHaveText(/啟動中/);
  });

  test('a session with no card sits in the board columns and gets you into its TUI', async ({
    page,
  }) => {
    await boot(page);

    // Opened from the sidebar, with no card behind it.
    await page.locator('.sidebar-head button.icon').click();
    await page.locator('.modal input.mono').first().fill('/Users/test/scratch');
    await page.locator('.modal button.primary').click();

    await page.getByTestId('view-board').click();
    // Live, so it is in 進行中 — beside the cards, not in a strip of its own.
    await expect(page.getByTestId('col-running').getByTestId('loose-s1')).toBeVisible();
    await expect(page.getByTestId('col-running').locator('.section-count')).toHaveText('1');

    await page.evaluate(() => window.__mock.report('s1', 'waiting_input'));
    await expect(page.getByTestId('loose-s1')).toHaveClass(/needs-you/);

    await page.getByTestId('loose-s1').getByRole('button', { name: /scratch/ }).click();
    await expect(page.locator('.pane[data-session-id="s1"]')).toHaveClass(/focused/);
  });

  test('the waiting badge counts board attempts and ad-hoc sessions alike', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');

    await page.locator('.sidebar-head button.icon').click();
    await page.locator('.modal input.mono').first().fill('/Users/test/scratch');
    await page.locator('.modal button.primary').click();

    // The attempt is on its trust prompt; put the ad-hoc one on a permission
    // prompt. Both are a person being waited on, and the badge is one number.
    await page.evaluate(() => window.__mock.report('s2', 'waiting_permission'));
    await expect(page.locator('.waiting-banner')).toHaveText(/2 個等你/);
  });

  test('a card whose repository is not one is refused in the dialog', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入', '/Users/test/not-a-repo');

    // A known refusal arrives translated and actionable; the backend's raw
    // words wait one disclosure behind it.
    await expect(page.getByTestId('task-error')).toContainText('不是 git repository');
    await page.getByTestId('task-error').locator('summary').click();
    await expect(page.getByTestId('task-error')).toContainText('not a git repository');
    // The dialog stays open with the typing intact, and no card was made.
    await expect(page.getByTestId('task-title')).toHaveValue('修好登入');
    await expect(page.locator('.board-card')).toHaveCount(0);
  });

  test('a base branch that does not exist is refused in the dialog', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入', REPO, 'no-such-branch');
    await expect(page.getByTestId('task-error')).toContainText('沒有名為「no-such-branch」的分支');
    await expect(page.locator('.board-card')).toHaveCount(0);
  });

  /**
   * Honest degradation. Guessing at another CLI's argument conventions is
   * worse than not trying: the flag that means "here is your prompt" in one
   * means "print this and exit" in another.
   */
  test('an agent we have not measured says it will not send the prompt', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入');

    await page.locator('[data-testid="task-k1"] button.primary').click();
    await expect(page.getByTestId('attempt-prompt')).toBeVisible();
    await page.getByTestId('attempt-agent').selectOption('gemini');

    await expect(page.getByTestId('attempt-manual')).toBeVisible();
    await expect(page.getByTestId('attempt-start')).toHaveText(/不送 prompt/);
    // The prompt is still built and still there to copy.
    await expect(page.getByTestId('attempt-prompt')).not.toHaveValue('');
    await expect(page.getByTestId('attempt-copy')).toBeVisible();
  });

  test('the prompt is editable, and what you edit is what starts the attempt', async ({
    page,
  }) => {
    await boot(page);
    await newCard(page, '修好登入');

    await page.locator('[data-testid="task-k1"] button.primary').click();
    await expect(page.getByTestId('attempt-prompt')).toContainText('[Marol 任務]');
    await page.getByTestId('attempt-prompt').fill('我自己寫的 prompt');
    await page.getByTestId('attempt-start').click();

    const sent = await page.evaluate(
      () =>
        (
          window.__mock.calls.find((c) => c.cmd === 'open_attempt')?.args as {
            prompt: string;
          }
        ).prompt,
    );
    expect(sent).toBe('我自己寫的 prompt');
  });

  /**
   * Switching agent means another attempt, not a restart of this one. The
   * first is left alone: two agents on one card, each in its own worktree, is
   * a thing worth being able to do, and comparing their diffs is the point.
   */
  test('換 agent opens a second attempt and leaves the first running', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await page.getByTestId('view-board').click();
    await page.evaluate(() => window.__mock.report('s1', 'running'));

    // Park and switch-agent live behind aiming at the card, so the clip
    // that reaches them starts the way a hand does.
    await page.getByTestId('task-k1').hover();
    await page.getByTestId('retry-k1').click();
    await page.getByTestId('attempt-agent').selectOption('codex');
    await page.getByTestId('attempt-start').click();
    await page.getByTestId('view-board').click();

    // Still one card, now with two live sessions behind it.
    await expect(page.locator('.board-card')).toHaveCount(1);
    await expect(page.locator('.session-row')).toHaveCount(2);
    // The card follows the newest attempt.
    await expect(page.getByTestId('task-k1')).toContainText('codex');
    await expect(page.getByTestId('state-k1')).toContainText('#2');

    // And the first attempt is untouched — nothing was superseded behind
    // your back.
    const first = await page.evaluate(
      () => window.__mock.tasks[0].attempts[0].outcome,
    );
    expect(first).toBeNull();
  });

  test('deleting a card takes its attempt session with it — once the agent settles', async ({
    page,
  }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await page.getByTestId('view-board').click();
    await expect(page.locator('.session-row')).toHaveCount(1);

    // Mid-turn the ✕ is a wall wearing words: deleting would take the
    // live session and its worktree with it — same guard park uses.
    const del = page.locator('[data-testid="task-k1"] [aria-label="刪除卡片"]');
    await expect(del).toBeDisabled();
    await page.evaluate(() => window.__mock.report('s1', 'idle'));
    await expect(del).toBeEnabled();

    // The first click only arms: a stray click on a 12px ✕ must not be able
    // to take a task's history with it.
    await del.click();
    await expect(page.locator('.board-card')).toHaveCount(1);
    await page.getByTestId('confirm-delete-k1').click();
    await expect(page.locator('.board-card')).toHaveCount(0);
    await expect(page.locator('.session-row')).toHaveCount(0);
  });
});

/**
 * The card's footprint badges: numstat counts and where the branch stands
 * against its base — read from git, never from the terminal.
 */
test.describe('attempt footprint on the card', () => {
  test('a card wears +N −M and ahead/behind', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await page.evaluate(() => {
      window.__mock.stats.set('k1-a1', { files: 2, adds: 12, dels: 3, ahead: 2, behind: 1, dirty: false });
    });

    await page.getByTestId('view-board').click();
    const stat = page.getByTestId('stat-k1');
    await expect(stat).toContainText('+12');
    await expect(stat).toContainText('−3');
    await expect(stat).toContainText('↑2');
    // Behind is the merge refusal not yet hit — the one count in warn.
    await expect(stat).toContainText('↓1');
  });

  test('an untouched worktree wears no badge at all', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await page.getByTestId('view-board').click();
    await expect(page.getByTestId('state-k1')).toBeVisible();
    await expect(page.getByTestId('stat-k1')).toHaveCount(0);
  });

  test('the drawer meta shows where the branch stands', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await page.evaluate(() => {
      window.__mock.stats.set('k1-a1', { files: 1, adds: 5, dels: 0, ahead: 1, behind: 2, dirty: false });
    });
    await page.getByTestId('view-board').click();
    await page.getByTestId('inspect-k1').click();
    await expect(page.getByTestId('inspector-behind')).toHaveText('↓2');
  });
});

/**
 * The base branch, offered instead of guessed: the repository's own
 * branches, most recently committed first, under the field as you type.
 */
test.describe('the base branch picker', () => {
  test('the repository corrects a default it does not have', async ({ page }) => {
    await boot(page);
    await page.evaluate(() => {
      window.__mock.repos['/Users/test/legacy-repo'] = ['develop', 'feature/x'];
    });
    await page.getByRole('button', { name: '新增卡片', exact: true }).click();
    await page.getByTestId('task-repo').fill('/Users/test/legacy-repo');

    // The suggestions arrive in recency order, and the untouched 'main'
    // guess becomes the branch the repo actually leads with.
    await expect(page.locator('#branch-options option')).toHaveCount(2);
    await expect(page.getByTestId('task-branch')).toHaveValue('develop');
  });

  test('a typed base is never overwritten', async ({ page }) => {
    await boot(page);
    await page.evaluate(() => {
      window.__mock.repos['/Users/test/legacy-repo'] = ['develop'];
    });
    await page.getByRole('button', { name: '新增卡片', exact: true }).click();
    await page.getByTestId('task-branch').fill('release');
    await page.getByTestId('task-repo').fill('/Users/test/legacy-repo');

    await expect(page.locator('#branch-options option')).toHaveCount(1);
    await expect(page.getByTestId('task-branch')).toHaveValue('release');
  });

  test('a path that is not a repository suggests nothing', async ({ page }) => {
    await boot(page);
    await page.getByRole('button', { name: '新增卡片', exact: true }).click();
    await page.getByTestId('task-repo').fill('/nowhere/at/all');
    await page.getByTestId('task-title').fill('等一下');
    await expect(page.locator('#branch-options option')).toHaveCount(0);
  });
});

test.describe('the visual system speaks', () => {
  test('an empty backlog is a door to the first card', async ({ page }) => {
    await boot(page);
    // The placeholder is a real button, and it opens the same dialog ＋ does.
    await page.getByTestId('board-cta').click();
    await expect(page.locator('.modal')).toBeVisible();
    await expect(page.getByTestId('task-title')).toBeVisible();
  });

  test('a mid-turn card shimmers; a blocked card breathes instead', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await page.getByTestId('view-board').click();

    await page.evaluate(() => window.__mock.report('s1', 'running'));
    await expect(page.getByTestId('task-k1')).toHaveClass(/astir/);

    // Blocked outranks busy: the breath is the only motion at attention
    // scale, so the shimmer yields the moment the card needs a human.
    await page.evaluate(() => window.__mock.report('s1', 'waiting_permission'));
    await expect(page.getByTestId('task-k1')).toHaveClass(/needs-you/);
    await expect(page.getByTestId('task-k1')).not.toHaveClass(/astir/);
  });

  test('a merged card says so on its edge — the one ending that is a win', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await page.getByTestId('view-board').click();
    await page.evaluate(async () => {
      await (
        window as unknown as {
          __TAURI_INTERNALS__: { invoke: (c: string, a: unknown) => Promise<unknown> };
        }
      ).__TAURI_INTERNALS__.invoke('finish_attempt', { attemptId: 'k1-a1', outcome: 'merged' });
    });
    await expect(page.getByTestId('task-k1')).toHaveAttribute('data-outcome', 'merged');
  });
});

test.describe('parked — work kept, ground given back', () => {
  test('a settled card parks: session gone, card asleep, branch in the toast', async ({
    page,
  }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await page.getByTestId('view-board').click();
    await page.evaluate(() => window.__mock.report('s1', 'idle'));

    await page.getByTestId('task-k1').hover();
    await page.getByTestId('park-k1').click();

    // The card is asleep, resumable — and its session left the sidebar:
    // parked things stop paying the attention tax.
    await expect(page.getByTestId('task-k1')).toHaveAttribute('data-live', 'parked');
    await expect(page.getByTestId('state-k1')).toHaveText(/已擱置/);
    await expect(page.getByTestId('resume-k1')).toBeVisible();
    await expect(page.locator('[data-testid="session-s1"]')).toHaveCount(0);
    await expect(page.locator('.toast.ok')).toContainText('marol/card-1');

    // The shelf checkpoint was kept before the ground went.
    const kept = await page.evaluate(() => window.__mock.checkpoints.get('k1-a1'));
    expect(kept?.length).toBe(1);
  });

  test('mid-turn there is no park button — the guard is ahead of the click', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await page.getByTestId('view-board').click();

    await page.evaluate(() => window.__mock.report('s1', 'running'));
    await expect(page.getByTestId('park-k1')).toHaveCount(0);
    await page.evaluate(() => window.__mock.report('s1', 'idle'));
    // Present again once it settles — and, like every secondary action on a
    // card, only once the card is aimed at.
    await page.getByTestId('task-k1').hover();
    await expect(page.getByTestId('park-k1')).toBeVisible();
  });

  test('resume wakes it into a terminal, and a restore failure is said, not hidden', async ({
    page,
  }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await page.getByTestId('view-board').click();
    await page.evaluate(() => window.__mock.report('s1', 'idle'));
    await page.getByTestId('task-k1').hover();
    await page.getByTestId('park-k1').click();
    await expect(page.getByTestId('task-k1')).toHaveAttribute('data-live', 'parked');

    await page.getByTestId('resume-k1').click();
    // Landed in the terminal, conversation continued on a fresh session.
    await expect(page.locator('.pane[data-session-id="s2"]')).toBeVisible();

    // Park again; this time the shelf refuses to come down cleanly.
    await page.getByTestId('view-board').click();
    await page.evaluate(() => {
      window.__mock.report('s2', 'idle');
      window.__mock.resumeRestoreError = 'restore blew up';
    });
    await page.getByTestId('task-k1').hover();
    await page.getByTestId('park-k1').click();
    await page.getByTestId('resume-k1').click();
    await expect(page.locator('.toast.error')).toContainText('restore blew up');
    // Honestly half-done: the terminal is still there to work in.
    await expect(page.locator('.pane[data-session-id="s3"]')).toBeVisible();
  });

  test('a parked drawer offers no worktree acts, and restore says resume first', async ({
    page,
  }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await page.getByTestId('view-board').click();
    await page.evaluate(() => window.__mock.report('s1', 'idle'));
    await page.getByTestId('task-k1').hover();
    await page.getByTestId('park-k1').click();

    await page.getByTestId('inspect-k1').click();
    // No shell, no ▶, no ⚑ — every chip in that row needs ground.
    await expect(page.getByTestId('run-scripts')).toHaveCount(0);
    await page.getByTestId('inspector-timeline-tab').click();
    const restore = page.getByTestId('restore-0');
    await expect(restore).toBeDisabled();
    await expect(restore).toHaveAttribute('title', /先繼續/);
  });
});

/**
 * 被 tmux 扛住的卡片是一扇門。
 *
 * `board.ts` 的註解一直寫著「打開卡片就接回還在跑的那個」,但卡片上從來沒有
 * 那個入口:`enter` 只認 `session`,detached 既不可點、footer 也沒有按鈕。
 * 一張寫著「執行中」卻打不開的卡,比一張寫著「未執行」的更糟 —— 那正是真的
 * 有東西在跑的那一格。
 */
test.describe('a card tmux is still holding', () => {
  test('is a door: clicking it reattaches instead of doing nothing', async ({ page }) => {
    await boot(page);
    await newCard(page, '修好登入');
    await start(page, 'k1');
    await page.getByTestId('view-board').click();

    // 這就是重啟後那一格:session 還在清單上,狀態是 detached,但這個行程裡
    // 沒有 pty 扛著它。
    await page.evaluate(() => {
      const s = window.__mock.sessions.find((x) => x.id === 's1');
      if (s) {
        s.live = false;
        s.status = 'detached';
      }
      window.__mock.pushSessions();
    });

    await expect(page.getByTestId('state-k1')).toContainText('執行中，無回報');
    const door = page.locator('[data-testid="task-k1"] .card-door');
    await expect(door).toHaveCount(1);

    await door.click();
    // 接回去了:離開看板、終端機在眼前,而且它在版面上真的佔了一格 ——
    // slots 是空的話,終端機牆會切過去然後什麼都不顯示。
    await expect(page.getByTestId('board')).toHaveCount(0);
    await expect(page.locator('.pane[data-session-id="s1"]')).toBeVisible();
    await expect
      .poll(() => page.evaluate(() => window.__mock.tabs[0].slots.filter(Boolean).length))
      .toBe(1);
  });
});
