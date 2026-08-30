import { test, expect } from '@playwright/test';
import {
  columnOf,
  currentAttempt,
  dropIndex,
  liveLabel,
  liveStateOf,
  liveTone,
  repoName,
} from '../src/board';
import { translator } from '../src/i18n/messages';
import type { Attempt, Lifecycle, SessionMeta, Status, Task } from '../src/types';

/** These assertions name the Chinese labels, so the model is asked in Chinese. */
const zh = translator('zh-TW');

function attempt(over: Partial<Attempt> & { id: string; seq: number }): Attempt {
  return {
    task_id: 'k1',
    agent: 'claude',
    worktree_path: `/wt/${over.id}`,
    branch: `marol/card-${over.seq}`,
    base_sha: 'abcd1234',
    outcome: null,
    frozen_diff: null,
    created_at: 1000 + over.seq,
    session_id: null,
    ...over,
  };
}

function task(over: Partial<Task> = {}): Task {
  return {
    id: 'k1',
    title: '修好登入',
    prompt: 'p',
    repo_path: '/repo',
    base_branch: 'main',
    lifecycle: 'running',
    position: 0,
    created_at: 1000,
    attempts: [],
    queued_at: null,
    ...over,
  };
}

function session(over: Partial<SessionMeta> & { id: string }): SessionMeta {
  return {
    cwd: '/wt/a1',
    title: 't',
    agent: 'claude',
    status: 'running' as Status,
    created_at: 1000,
    last_active_at: 1000,
    live: true,
    reports_status: true,
    hooks_wired: true,
    activity: null,
    activity_since: 0,
    completed: false,
    attempt_id: null,
    ...over,
  };
}

test.describe('which attempt a card is about', () => {
  test('the newest open one, because opening another is how you switch agent', () => {
    const t = task({
      attempts: [
        attempt({ id: 'a1', seq: 1, outcome: 'superseded' }),
        attempt({ id: 'a2', seq: 2 }),
      ],
    });
    expect(currentAttempt(t)?.id).toBe('a2');
  });

  test('order comes from the attempt number, not from the array', () => {
    const t = task({
      attempts: [attempt({ id: 'a2', seq: 2 }), attempt({ id: 'a1', seq: 1 })],
    });
    expect(currentAttempt(t)?.id).toBe('a2');
  });

  /// Once everything is finished the card still has to show something, or a
  /// finished card would look as though it had never been started.
  test('falls back to the newest finished one when none are open', () => {
    const t = task({
      attempts: [
        attempt({ id: 'a1', seq: 1, outcome: 'discarded' }),
        attempt({ id: 'a2', seq: 2, outcome: 'merged' }),
      ],
    });
    expect(currentAttempt(t)?.id).toBe('a2');
  });

  test('a card with no attempts has none', () => {
    expect(currentAttempt(task())).toBeNull();
  });
});

test.describe('the second axis', () => {
  test('a live session reports its own status', () => {
    const t = task({ attempts: [attempt({ id: 'a1', seq: 1, session_id: 's1' })] });
    const live = liveStateOf(t, [session({ id: 's1', status: 'waiting_permission' })]);
    expect(live.kind).toBe('session');
    expect(liveLabel(live, zh)).toBe('等你授權');
    expect(liveTone(live)).toBe('waiting_permission');
  });

  /**
   * The state every attempt is in after a restart: the app kills its PTYs on
   * the way out, so a card left in 進行中 has no agent behind it. Showing it
   * as running would be a lie, and it is the common case rather than an edge
   * one.
   */
  test('an attempt whose session is not live reads as stopped, not running', () => {
    const t = task({ attempts: [attempt({ id: 'a1', seq: 1, session_id: 's1' })] });
    const live = liveStateOf(t, [session({ id: 's1', live: false, status: 'saved' })]);
    expect(live.kind).toBe('stopped');
    expect(liveLabel(live, zh)).toBe('未執行');
  });

  test('an attempt whose session row is gone is still resumable', () => {
    const t = task({ attempts: [attempt({ id: 'a1', seq: 1, session_id: null })] });
    expect(liveStateOf(t, []).kind).toBe('stopped');
  });

  test('a finished attempt names how it ended', () => {
    const t = task({ attempts: [attempt({ id: 'a1', seq: 1, outcome: 'merged' })] });
    const live = liveStateOf(t, []);
    expect(live.kind).toBe('finished');
    expect(liveLabel(live, zh)).toBe('已合併');
  });

  test('a card nobody has started says so', () => {
    expect(liveLabel(liveStateOf(task(), []), zh)).toBe('尚未開始');
  });

  /**
   * Waiting for a slot is its own state. It has no attempt yet — the worktree
   * is not made until its turn comes — so it must not read as "not started",
   * which is the one thing that would make someone press 開始 again.
   */
  test('a card waiting for a slot says where it is in the queue', () => {
    const live = liveStateOf(task({ queued_at: 2 }), []);
    expect(live.kind).toBe('queued');
    expect(liveLabel(live, zh)).toBe('排隊中 · 第 2 個');
  });

  /// Once it starts, the attempt is what the card is about.
  test('a card that got its turn stops reading as queued', () => {
    const live = liveStateOf(
      task({ queued_at: null, attempts: [attempt({ id: 'a1', seq: 1, session_id: 's1' })] }),
      [session({ id: 's1', status: 'running' })],
    );
    expect(live.kind).toBe('session');
  });

  /// A card that is merely stopped must not look like a warning, or the
  /// warning colour stops meaning anything.
  test('only a session that is really blocked takes the warning tone', () => {
    const stopped = liveStateOf(
      task({ attempts: [attempt({ id: 'a1', seq: 1, session_id: null })] }),
      [],
    );
    expect(liveTone(stopped)).toBe('saved');
  });
});

test.describe('dropping a card', () => {
  const cards = [
    task({ id: 'a', position: 0 }),
    task({ id: 'b', position: 1 }),
    task({ id: 'c', position: 2 }),
  ];

  test('onto a card inserts before it', () => {
    expect(dropIndex(cards, 'x', 'b')).toBe(1);
  });

  test('onto empty space appends', () => {
    expect(dropIndex(cards, 'x', null)).toBe(3);
  });

  /**
   * Dragging within a column: the card being moved is not in the running
   * order any more, so counting it would make every move past itself land one
   * place short.
   */
  test('the card being dragged does not count towards its own destination', () => {
    expect(dropIndex(cards, 'a', 'c')).toBe(1);
    expect(dropIndex(cards, 'a', null)).toBe(2);
  });

  test('a card dropped on nothing recognisable goes to the end', () => {
    expect(dropIndex(cards, 'x', 'nonexistent')).toBe(3);
  });
});

test('a column is ordered by position, not by arrival', () => {
  const tasks = [
    task({ id: 'late', lifecycle: 'running', position: 2 }),
    task({ id: 'first', lifecycle: 'running', position: 0 }),
    task({ id: 'elsewhere', lifecycle: 'done' as Lifecycle, position: 0 }),
    task({ id: 'mid', lifecycle: 'running', position: 1 }),
  ];
  expect(columnOf(tasks, 'running').map((t) => t.id)).toEqual(['first', 'mid', 'late']);
});

/** Cards from different repositories share one board, so each must be able
    to say which codebase it is about. */
test('a card names its repository by basename', () => {
  expect(repoName('/Users/me/code/marol')).toBe('marol');
  expect(repoName('/Users/me/code/marol/')).toBe('marol');
  expect(repoName('weird')).toBe('weird');
});

/** A repo in another world wears its host ahead of its name; a local one
    wears nothing. Mirrors host.rs. */
test('a wsl repository is labelled with its distro', async () => {
  const { hostLabel, repoName } = await import('../src/board');
  expect(hostLabel('wsl://Ubuntu/home/me/code/app')).toBe('wsl:Ubuntu');
  expect(hostLabel('ssh://devbox/home/me/app')).toBe('ssh:devbox');
  expect(hostLabel('/Users/me/code/app')).toBeNull();
  // The name still reads from the path's last segment, URL or not.
  expect(repoName('wsl://Ubuntu/home/me/code/app')).toBe('app');
});

/**
 * 「app 關掉了,agent 沒有」——tmux 撐住的 session 在重開後不能讀成未執行。
 *
 * 這個謊言原本會講兩次:狀態標籤講一次(sidebar / overview / 面板),
 * 卡片再用 `live.stopped` 講一次。兩處都要說實話。
 */
test.describe('a session tmux kept running', () => {
  test('reads as 執行中，無回報 on the card, not as 未執行', () => {
    const t = task({ attempts: [attempt({ id: 'a1', seq: 1, session_id: 's1' })] });
    const live = liveStateOf(t, [session({ id: 's1', live: false, status: 'detached' })]);
    expect(live.kind).toBe('detached');
    expect(liveLabel(live, zh)).toBe('執行中，無回報');
  });

  test('wears the neutral dot — running is known, what it is doing is not', () => {
    const t = task({ attempts: [attempt({ id: 'a1', seq: 1, session_id: 's1' })] });
    const live = liveStateOf(t, [session({ id: 's1', live: false, status: 'detached' })]);
    // 不是 accent:這一刻知道的就只有「它在跑」。hook 的 endpoint 現在跨重啟
    // 是同一個,所以 agent 的下一個事件會把真狀態放回來 —— 但那是之後的事,
    // 而這個點畫的是現在。
    expect(liveTone(live)).toBe('detached');
  });

  // 接回去之後,session 變成 live,但狀態仍是 detached —— 因為 attach 不是
  // 啟動,不會有新的 SessionStart。卡片這時走 `session` 這一格,而標籤必須
  // 還是那句「執行中,無回報」,不能變成「啟動中」。
  test('after reattaching it is live and still not reporting', () => {
    const t = task({ attempts: [attempt({ id: 'a1', seq: 1, session_id: 's1' })] });
    const live = liveStateOf(t, [session({ id: 's1', live: true, status: 'detached' })]);
    expect(live.kind).toBe('session');
    expect(liveLabel(live, zh)).toBe('執行中，無回報');
  });

  test('a session that really did end still reads as 未執行', () => {
    const t = task({ attempts: [attempt({ id: 'a1', seq: 1, session_id: 's1' })] });
    const live = liveStateOf(t, [session({ id: 's1', live: false, status: 'saved' })]);
    expect(live.kind).toBe('stopped');
    expect(liveLabel(live, zh)).toBe('未執行');
  });
});
