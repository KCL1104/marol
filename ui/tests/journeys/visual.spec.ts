import { test, expect } from '@playwright/test';
import { chord, coldStart, driveStatus, expectAnnounce, expectFocusNeutral, expectFocusWithin } from './helpers';

/**
 * V —— 六個關鍵畫面的視覺基準線，走成一條不換手的線：
 * 冷啟被招呼（歡迎面板＋探測發現＋三點軌）→ 第一次的空看板（長句 CTA）→
 * 空的終端牆（認鍵卡）→ 種一張三態看板（微光／呼吸／已合併並排）→
 * 走進檢視器（diff、一則行內意見、finish footer）→ 指令面板當注意力
 * 收件匣（兩個等你）。
 *
 * 視覺契約的紀律：
 * - 基準線是像素，像素跟著平台的字型光柵走。CI 與這台開發機都是
 *   linux，基準就只在 linux 鑄造 —— 其他平台跳過，不產生第二套真相。
 * - reducedMotion: 'reduce' 讓呼吸與微光落在 styles.css 說好的靜態終點，
 *   toHaveScreenshot 再補 animations: 'disabled'，兩層一起把「動」凍住。
 * - 會走的字（等候計秒、diff 的讀取時鐘）用 mask 蓋掉 —— 蓋的是時間，
 *   不是版面：被蓋的元素仍在流裡佔位，位移照樣會被看見。
 * - 關鍵節點照 journey 的三重奏驗過（焦點落點、朗讀通道、可見狀態）
 *   再按快門：截圖凍結的必須是一個被證實過的狀態，不是碰巧的一幀。
 */

// 基準線只在 linux 上鑄造（見檔頭）：其他平台直接跳過整個檔案。
test.skip(process.platform !== 'linux', 'visual baselines are minted on linux only');

// 1280×820 是 app 預設視窗的大小 —— 基準線量的就是出廠那一扇窗。
// reducedMotion 在這一版 Playwright 住在 contextOptions 底下。
test.use({
  viewport: { width: 1280, height: 820 },
  contextOptions: { reducedMotion: 'reduce' },
});

/** 每一張快門共用的比較參數。0.02 的容差吃掉抗鋸齒的呼吸，吃不掉版面位移。 */
const SHOT = { animations: 'disabled' as const, maxDiffPixelRatio: 0.02 };

/** k1 的 diff：與 j1 同一格式的最小真實形狀 —— 兩個可評論的加行、
 *  一個刪行，行內意見有地方落。 */
const DIFF = [
  'diff --git a/src/auth/session.py b/src/auth/session.py',
  '--- a/src/auth/session.py',
  '+++ b/src/auth/session.py',
  '@@ -12,7 +12,8 @@ def login(request):',
  '     user = authenticate(request)',
  '     if user is None:',
  '-        return redirect("/login")',
  '+        clear_session_cookie(request)',
  '+        return redirect("/login?expired=1")',
  '     session = Session.create(user)',
].join('\n');

test('V · six key screens, one continuous line', async ({ page }) => {
  // 六幕、六張快門；120s 是餘裕不是等待 —— 每一步仍靠 expect 輪詢。
  test.setTimeout(120_000);

  const live = page.getByTestId('live-announce');
  // 會走的字，全部蓋掉：卡片的等候計秒、側欄列的計秒、diff 的讀取時鐘。
  // 沒出現的場景裡這些 locator 配不到東西，mask 就是無為 —— 一份清單
  // 六張快門共用，哪一張都不必重新想一次「這裡有沒有時間」。
  const masks = [
    page.locator('.card-elapsed'),
    page.locator('.row-elapsed'),
    page.locator('.diff-fetched'),
  ];

  await test.step('1. cold start — the welcome panel, probe findings, three-dot rail', async () => {
    await coldStart(page);
    await expect(page.locator('.sidebar')).toBeVisible();

    // (c) 歡迎面板端出探測的發現與三點軌 —— 快門前先證實內容都到齊。
    const modal = page.locator('.modal');
    await expect(modal).toContainText('歡迎使用 Marol');
    await expect(page.getByTestId('welcome-claude')).toContainText('✓ 2.1.226');
    await expect(page.getByTestId('welcome-codex')).toContainText('找不到');
    await expect(page.locator('.welcome-rail-row')).toHaveCount(3);
    // (a) 焦點真的在對話框裡；(b) 朗讀通道 polite 且此刻無話可說。
    await expectFocusWithin(page, '.modal');
    await expect(live).toHaveAttribute('aria-live', 'polite');
    await expect(live).toHaveText('');

    // 字型換裝完成才按快門：截圖的穩定迴圈會等,但等在明處更誠實。
    await page.evaluate(() => document.fonts.ready.then(() => undefined));
    await expect(page).toHaveScreenshot('1-welcome.png', { ...SHOT, mask: masks });
  });

  await test.step('2. close the greeting — the first-run board wears the long CTA', async () => {
    await page.locator('.modal button', { hasText: '關閉' }).click();
    // 滑鼠停回原點：按鈕消失後游標懸在看板上，:hover 的殘影不准進基準線。
    await page.mouse.move(0, 0);

    // (c) 對話框收起，腳下已是看板，空的待辦欄本身就是那扇門。
    await expect(page.locator('.modal')).toHaveCount(0);
    await expect(page.getByTestId('board')).toBeVisible();
    await expect(page.getByTestId('board-cta')).toHaveText('新增一張卡片');
    // (a) 自動浮起的面板沒有召喚者可還，焦點退回 <body>；(b) 通道安靜。
    await expectFocusNeutral(page);
    await expect(live).toHaveText('');

    await expect(page).toHaveScreenshot('2-first-run-board.png', { ...SHOT, mask: masks });
  });

  await test.step('6th scene early — the empty terminal wall still teaches its keys', async () => {
    // 認鍵卡只活在「一個 session 都還沒開過」的桌子上,所以這一幕必須
    // 搶在種桌子之前拍 —— 場景編號 6,拍攝順序第三。
    await chord(page, '1');

    // (c) 空網格戴著三行認鍵卡。
    await expect(page.getByTestId('empty-grid')).toBeVisible();
    await expect(page.getByTestId('term-keymap')).toBeVisible();
    await expect(page.locator('.term-keymap-row')).toHaveCount(3);
    await expect(page.getByTestId('term-keymap')).toContainText('終端機 · 看板 · 總覽');
    // (a) 視圖和弦不搬焦點；(b) 換視圖不是要朗讀的事。
    await expectFocusNeutral(page);
    await expect(live).toHaveText('');

    await expect(page).toHaveScreenshot('6-terminal-wall.png', { ...SHOT, mask: masks });
  });

  await test.step('3. seed the desk — running/astir, waiting/breathing, merged, side by side', async () => {
    // 之後的每一幕都會踩到 coach 的觸發點(等你、進 pane、finish footer)。
    // coach 的課在 onboarding.spec 與 j1 教過了 —— 這條線量的是像素,
    // 先把五堂課都答掉,快門裡才不會有一張隨機的卡片。coachSeen 每次
    // 都現讀 localStorage,所以中途改旗標就生效。
    await page.evaluate(() =>
      localStorage.setItem(
        'marol.coach',
        JSON.stringify({ attempt: true, mode: true, finish: true, terminal: true, waiting: true }),
      ),
    );

    // 直接把三態桌子寫進 mock(shots.spec 的同一扇門):三張卡、兩個
    // session、腳印與 diff,一次 evaluate 種完。時間欄位以當下為錨 ——
    // 等候計秒會被 mask,但被蓋的字寬要兩次跑之間一致,版面才可比。
    await page.evaluate((diff) => {
      const m = window.__mock;
      const now = Date.now();
      m.sessions.push(
        {
          id: 's101',
          cwd: '/Users/test/worktrees/card-1',
          title: '修好登入轉圈圈 #1',
          agent: 'claude',
          status: 'running',
          created_at: now - 3600e3,
          last_active_at: now - 45e3,
          live: true,
          reports_status: true,
          hooks_wired: true,
          preview_port: null,
          activity: { tool: 'Bash', detail: 'pytest tests/auth -x' },
          activity_since: 0,
          completed: false,
          attempt_id: 'k1-a1',
          usage: {
            input: 48_213,
            output: 612_400,
            cache_read: 96_420_113,
            cache_write: 5_204_887,
            context: 74_310,
          },
        },
        {
          id: 's102',
          cwd: '/Users/test/worktrees/card-2',
          title: '公開 API 加上限流 #1',
          agent: 'claude',
          status: 'waiting_permission',
          created_at: now - 3600e3,
          last_active_at: now - 134e3,
          live: true,
          reports_status: true,
          hooks_wired: true,
          preview_port: null,
          activity: { tool: 'Edit', detail: 'src/api/limiter.ts' },
          activity_since: 0,
          completed: false,
          attempt_id: 'k2-a1',
        },
      );
      const mkAttempt = (id: string, taskId: string, sessionId: string | null) => ({
        id,
        task_id: taskId,
        seq: 1,
        agent: 'claude',
        worktree_path: `/Users/test/worktrees/${taskId}`,
        branch: `marol/${taskId}-1`,
        base_sha: 'abcd1234deadbeef',
        mode: 'normal',
        outcome: null as string | null,
        frozen_diff: null as string | null,
        created_at: now - 3000e3,
        parked_at: null,
        session_id: sessionId,
      });
      m.tasks.push(
        {
          id: 'k1',
          title: '修好登入轉圈圈',
          prompt: '修好登入轉圈圈',
          repo_path: '/Users/test/picked-repo',
          base_branch: 'main',
          lifecycle: 'running',
          position: 0,
          created_at: now - 86400e3,
          attempts: [mkAttempt('k1-a1', 'k1', 's101')],
          queued_at: null,
        },
        {
          id: 'k2',
          title: '公開 API 加上限流',
          prompt: '公開 API 加上限流',
          repo_path: '/Users/test/picked-repo',
          base_branch: 'main',
          lifecycle: 'running',
          position: 1,
          created_at: now - 86400e3,
          attempts: [{ ...mkAttempt('k2-a1', 'k2', 's102'), mode: 'accept_edits' }],
          queued_at: null,
        },
        {
          id: 'k3',
          title: '編輯器深色主題',
          prompt: '編輯器深色主題',
          repo_path: '/Users/test/picked-repo',
          base_branch: 'main',
          lifecycle: 'done',
          position: 0,
          created_at: now - 172800e3,
          attempts: [
            {
              ...mkAttempt('k3-a1', 'k3', null),
              outcome: 'merged',
              frozen_diff: 'diff --git a/theme.css b/theme.css\n+dark\n',
            },
          ],
          queued_at: null,
        },
      );
      m.stats.set('k1-a1', { files: 2, adds: 11, dels: 2, ahead: 2, behind: 0, dirty: false });
      m.stats.set('k2-a1', { files: 5, adds: 342, dels: 57, ahead: 0, behind: 0, dirty: false });
      m.diffs.set('k1-a1', diff);
      m.pushSessions();
      m.pushTasks();
    }, DIFF);

    // (b) 一個 session 帶著「等你」出生,朗讀鏈立刻說出是誰、等什麼。
    await expectAnnounce(page, '公開 API 加上限流 #1 等你授權');

    await chord(page, '2');
    // (c-1) 微光的卡:agent 在做,狀態行說「執行中」,腳印上了狀態行。
    await expect(page.getByTestId('board')).toBeVisible();
    await expect(page.getByTestId('task-k1')).toHaveClass(/astir/);
    await expect(page.getByTestId('state-k1')).toContainText('執行中');
    await expect(page.getByTestId('stat-k1')).toContainText('+11');
    // (c-2) 呼吸的卡:等你授權,計秒在場(等下被 mask 的就是它)。
    await expect(page.getByTestId('task-k2')).toHaveClass(/needs-you/);
    await expect(page.getByTestId('state-k2')).toContainText('等你授權');
    await expect(page.locator('[data-testid="task-k2"] .card-elapsed')).toBeVisible();
    await expect(page.getByTestId('mode-k2')).toBeVisible();
    // (c-3) 合併的卡:完成欄裡戴著紫紅的邊。
    await expect(page.getByTestId('task-k3')).toHaveAttribute('data-outcome', 'merged');
    await expect(page.getByTestId('state-k3')).toContainText('已合併');
    // (c-4) 側欄同一份事實:琥珀橫幅計 1,等你分區列著 s102。
    await expect(page.locator('.waiting-banner')).toHaveText('1 個等你');
    await expect(
      page.locator('[data-section="waiting"] [data-testid="session-s102"]'),
    ).toBeVisible();
    // (a) 種桌子與換視圖都不搬焦點。
    await expectFocusNeutral(page);

    await expect(page).toHaveScreenshot('3-board-three-states.png', { ...SHOT, mask: masks });
  });

  await test.step('4. walk into the inspector — a diff, one line comment, the finish footer', async () => {
    // k1 的回合在看板前結束 —— 未讀文法亮起,這正是「進門檢視」的起點。
    await driveStatus(page, 's101', 'idle');
    await expect(page.getByTestId('unseen-card-k1')).toBeVisible();
    await expect(page.getByTestId('unseen-s101')).toBeVisible();
    await expectAnnounce(page, '「修好登入轉圈圈 #1」回合結束');

    // 進門看它:看見即已讀。用鍵盤進門(門聚焦、Enter)—— 滑鼠靠近
    // 卡片會讓 peek 滑進來、整欄左移,mousedown 與 mouseup 之間門就
    // 搬了家,click 事件落不到門上;焦點不吃幾何,Enter 永遠算數。
    const door = page.locator('[data-testid="task-k1"] .card-door');
    await door.focus();
    await page.keyboard.press('Enter');
    // (c) 真的進門了:看板讓位給終端視圖,pane 上了牆。
    await expect(page.getByTestId('board')).toHaveCount(0);
    await expect(page.locator('.pane[data-session-id="s101"]')).toBeVisible();
    // (a) 插入點真的落進終端。
    await expectFocusWithin(page, '.pane[data-session-id="s101"] .term-host');
    await expect(page.getByTestId('unseen-card-k1')).toHaveCount(0);
    await expect(page.getByTestId('unseen-s101')).toHaveCount(0);

    // 插入點在終端裡,所以是 Shift 變體(Ctrl+字母屬於 shell)。
    await chord(page, 'I', { shift: true });
    // (c) 檢視器開在終端旁,diff 以檔名籌碼領頭。
    await expect(page.getByTestId('inspector')).toBeVisible();
    await expect(page.locator('.diff-file')).toContainText('src/auth/session.py');
    // (a) 和弦自己走完旅程:焦點落在 diff 本體。
    await expect(page.getByTestId('diff-body')).toBeFocused();
    // (b) 開檢視器不是要朗讀的事。
    await expect(live).not.toContainText('檢視');

    // j 走到第一個可評論的行,Enter 對準它留話,⌘/Ctrl+Enter 收進批次。
    await page.keyboard.press('j');
    await expect(page.locator('.diff-line.commentable').first()).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(page.getByTestId('review-note')).toBeFocused();
    await page.getByTestId('review-note').fill('先清掉 cookie 再轉址，不然還是會轉圈圈。');
    await chord(page, 'Enter');
    await expect(page.getByTestId('review-pending')).toContainText('先清掉 cookie');
    // 撰寫框借走的插入點還給 diff。
    await expect(page.getByTestId('diff-body')).toBeFocused();

    // finish footer 全員到齊:合併武裝制的第一個名字、PR、放棄。
    await expect(page.getByTestId('merge-attempt')).toHaveText('合併回 main');
    await expect(page.getByTestId('open-pr')).toBeVisible();
    await expect(page.getByTestId('discard-attempt')).toBeVisible();

    // 快門前把游標停回原點:剛剛的點擊讓它懸在抽屜上。
    await page.mouse.move(0, 0);
    await expect(page).toHaveScreenshot('4-inspector-review.png', { ...SHOT, mask: masks });
  });

  await test.step('5. the palette as attention inbox — two waiting, first row selected', async () => {
    // 回看板;檢視器連著終端視圖一起卸下,焦點退回 <body>。
    await chord(page, '2');
    await expect(page.getByTestId('board')).toBeVisible();
    await expectFocusNeutral(page);

    // k1 也停在授權門上 —— 桌上現在兩個等你。
    await driveStatus(page, 's101', 'waiting_permission');
    await expectAnnounce(page, '修好登入轉圈圈 #1 等你授權');
    // 側欄橫幅說全桌的真相(2 個);分頁徽章只數自己牆上的 pane ——
    // s102 從沒上過牆,所以徽章是 1。兩個數字各說各的事實,都對。
    await expect(page.locator('.waiting-banner')).toHaveText('2 個等你');
    await expect(page.locator('.tab-badge.waiting')).toHaveText('1');

    // 朗讀通道說完 5 秒自清 —— 等它清空(輪詢,不 sleep),下一個斷言
    // 「開面板不朗讀」才有乾淨的地板可證。
    await expect(live).toHaveText('', { timeout: 9_000 });

    await chord(page, 'K');
    // (c) 面板開著,第一組就是收件匣:兩列等你,各自戴著狀態。
    await expect(page.getByTestId('palette')).toBeVisible();
    await expect(page.getByTestId('palette')).toContainText('等你');
    await expect(page.getByTestId('pal-session-s101')).toContainText('等你授權');
    await expect(page.getByTestId('pal-session-s102')).toContainText('等你授權');
    // 第一列已選好 —— ⌘K、Enter 就是整趟旅程。
    await expect(page.locator('.palette-item').first()).toHaveAttribute('aria-selected', 'true');
    // (a) 焦點在輸入框,一落地就能打字。
    await expect(page.getByTestId('palette-input')).toBeFocused();
    // (b) 開面板不是要朗讀的事:通道維持剛剛證過的安靜。
    await expect(live).toHaveText('');

    await expect(page).toHaveScreenshot('5-palette-inbox.png', { ...SHOT, mask: masks });
  });
});
