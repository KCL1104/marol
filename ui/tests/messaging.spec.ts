import { test, expect } from '@playwright/test';
import { installMock } from './mock-tauri';

test.describe('cross-session messaging surfaces', () => {
  /** The environment panel answers "can my sessions message each other"
      without the person opening a terminal to find out. */
  test('the environment panel says whether messaging is available', async ({ page }) => {
    await page.addInitScript(installMock);
    await page.goto('/');
    await expect(page.locator('.tab')).toHaveCount(1);

    await page.locator('.sidebar-foot').click();
    await page.getByTestId('sec-diagnostics').click();
    const panel = page.locator('.modal');
    await expect(panel).toContainText('跨 session 互傳訊息');
    await expect(panel).toContainText('✓ · claude 2.1.226');
  });

  /** An older CLI reads as "not yet", with the version that would fix it. */
  test('an older claude shows what is missing rather than a bare no', async ({ page }) => {
    await page.addInitScript(installMock);
    await page.addInitScript(() => {
      const internals = (
        window as unknown as {
          __TAURI_INTERNALS__: { invoke: (c: string, a?: unknown) => Promise<unknown> };
        }
      ).__TAURI_INTERNALS__;
      const original = internals.invoke.bind(internals);
      internals.invoke = (cmd: string, args?: unknown) =>
        cmd === 'boot_status'
          ? original(cmd, args).then((b) => ({
              ...(b as object),
              claudeVersion: '2.0.14',
              messaging: false,
            }))
          : original(cmd, args);
    });
    await page.goto('/');
    await expect(page.locator('.tab')).toHaveCount(1);

    await page.locator('.sidebar-foot').click();
    await page.getByTestId('sec-diagnostics').click();
    await expect(page.locator('.modal')).toContainText('需要 Claude Code ≥ 2.1.224（目前 2.0.14）');
  });

  /**
   * Keeping the agent CLIs current, and saying what it did.
   *
   * The report is per world because the CLIs are per world: a WSL distro has
   * its own claude at its own version, and updating the one on the Windows
   * side would leave the one that actually runs untouched. And it reports
   * what changed rather than what was attempted — "already current" is a
   * measured version, not the CLI's own wording, which has changed before.
   */
  test('the agent updates say what happened, per world', async ({ page }) => {
    await page.addInitScript(installMock);
    await page.addInitScript(() => {
      const internals = (
        window as unknown as {
          __TAURI_INTERNALS__: { invoke: (c: string, a?: unknown) => Promise<unknown> };
        }
      ).__TAURI_INTERNALS__;
      const original = internals.invoke.bind(internals);
      internals.invoke = (cmd: string, args?: unknown) =>
        cmd === 'boot_status'
          ? original(cmd, args).then((b) => ({
              ...(b as object),
              agentUpdates: [
                {
                  world: 'wsl:Ubuntu',
                  agent: 'codex',
                  from: '0.145.0',
                  to: '0.151.0',
                  outcome: 'updated',
                  detail: '',
                },
                {
                  world: 'wsl:Ubuntu',
                  agent: 'claude',
                  from: '2.1.226',
                  to: '2.1.226',
                  outcome: 'current',
                  detail: '',
                },
                {
                  world: 'ssh:build',
                  agent: 'codex',
                  from: '0.140.0',
                  to: '0.140.0',
                  outcome: 'failed',
                  detail: 'unrecognized subcommand',
                },
              ],
            }))
          : original(cmd, args);
    });
    await page.goto('/');
    await expect(page.locator('.tab')).toHaveCount(1);

    await page.locator('.sidebar-foot').click();
    await page.getByTestId('sec-updates').click();
    const report = page.getByTestId('ag-report');
    await expect(report).toContainText('wsl:Ubuntu 的 codex：0.145.0 → 0.151.0');
    // Already current is worth a row too: it is the difference between "the
    // pass ran and there was nothing to do" and "the pass never reached here".
    await expect(report).toContainText('wsl:Ubuntu 的 claude：已經是 2.1.226');
    // A refusal names itself rather than being folded into silence.
    await expect(report).toContainText('ssh:build 的 codex：更新不了（unrecognized subcommand）');
  });

  /** Off means off: no rows, and the desk asks nothing of anybody's CLI. */
  test('the agent update pass can be switched off', async ({ page }) => {
    await page.addInitScript(installMock);
    await page.goto('/');
    await expect(page.locator('.tab')).toHaveCount(1);

    await page.locator('.sidebar-foot').click();
    await page.getByTestId('sec-updates').click();
    const toggle = page.getByTestId('ag-toggle');
    await expect(toggle).toBeChecked();
    await expect(page.getByTestId('ag-report')).toBeVisible();

    await toggle.uncheck();
    await expect(toggle).not.toBeChecked();
    // The report goes with it: it describes a pass that is no longer run,
    // and leaving it up would read as one that still is.
    await expect(page.getByTestId('ag-report')).toHaveCount(0);
  });

  /**
   * The held shells are worth a row precisely because declining is silent.
   *
   * A world with no `sh` on the far side, or a pool that is always
   * contended, falls back to spawning every command — which is correct, and
   * is also indistinguishable from the channel working, except that the
   * window feels the way it did before any of this. The count is the only
   * place that difference is visible, so it says both halves: how many
   * answers cost no process, and out of how many.
   */
  test('the diagnostics say what the held shells actually saved', async ({ page }) => {
    await page.addInitScript(installMock);
    await page.addInitScript(() => {
      const internals = (
        window as unknown as {
          __TAURI_INTERNALS__: { invoke: (c: string, a?: unknown) => Promise<unknown> };
        }
      ).__TAURI_INTERNALS__;
      const original = internals.invoke.bind(internals);
      internals.invoke = (cmd: string, args?: unknown) =>
        cmd === 'boot_status'
          ? original(cmd, args).then((b) => ({
              ...(b as object),
              channels: [
                { world: 'wsl:Ubuntu', held: 118, spawned: 2, lost: 0 },
                { world: 'ssh:build', held: 4, spawned: 1, lost: 3 },
              ],
            }))
          : original(cmd, args);
    });
    await page.goto('/');
    await expect(page.locator('.tab')).toHaveCount(1);

    await page.locator('.sidebar-foot').click();
    await page.getByTestId('sec-diagnostics').click();
    const panel = page.locator('.modal');
    // One row per world, named the way a card's badge names it.
    await expect(panel).toContainText('wsl:Ubuntu');
    await expect(panel).toContainText('120 個命令裡有 118 個不必開行程');
    // A lost command is a command whose fate is unknown, which is a
    // different fact from a slow one and is not folded into the ratio.
    await expect(panel).toContainText('ssh:build');
    await expect(panel).toContainText('3 個沒有回音');
    // The world with none stays quiet: a clean channel has nothing to add.
    await expect(panel).not.toContainText('118 個沒有回音');
  });

  /**
   * The diagnostics list both CLIs this desk knows how to drive, and for
   * each one the two facts that decide what a card can do: whether it is
   * installed, and whether the installed version reports status. Those are
   * different answers — a Codex too old for its hooks engine runs a session
   * perfectly and tells the desk nothing — and a panel that only said
   * "found" would leave the commonest blank card unexplained.
   */
  test('the diagnostics name both CLIs, and say which of them reports status', async ({
    page,
  }) => {
    await page.addInitScript(installMock);
    await page.addInitScript(() => {
      const internals = (
        window as unknown as {
          __TAURI_INTERNALS__: { invoke: (c: string, a?: unknown) => Promise<unknown> };
        }
      ).__TAURI_INTERNALS__;
      const original = internals.invoke.bind(internals);
      internals.invoke = (cmd: string, args?: unknown) =>
        cmd === 'boot_status'
          ? original(cmd, args).then((b) => ({
              ...(b as object),
              codex: '/usr/local/bin/codex',
              codexVersion: '0.100.0',
              agents: [
                { name: 'claude', path: '/usr/local/bin/claude', version: '2.1.226', reports: true },
                // Installed, and older than the hooks engine this desk
                // wires up: found, but quiet.
                { name: 'codex', path: '/usr/local/bin/codex', version: '0.100.0', reports: false },
              ],
            }))
          : original(cmd, args);
    });
    await page.goto('/');
    await expect(page.locator('.tab')).toHaveCount(1);

    await page.locator('.sidebar-foot').click();
    await page.getByTestId('sec-diagnostics').click();
    const panel = page.locator('.modal');
    await expect(panel).toContainText('/usr/local/bin/claude · 2.1.226 · 狀態回報 ✓');
    await expect(panel).toContainText('/usr/local/bin/codex · 0.100.0 · 沒有狀態回報');
  });
});

/**
 * 一張只跑 codex 的桌子。
 *
 * 「跨 session 互傳訊息」是 Claude Code 自己的功能,marol 只是替 session
 * 取名字讓訊息有地方去。所以對一個從沒裝過 claude 的人,「需要 Claude
 * Code ≥ 2.1.224（目前 —）」是一件他沒要求過的雜事 —— 那句話是給有
 * claude、只是版本太舊的人看的。兩種讀者,兩句話。
 */
test.describe('a desk with no claude on it', () => {
  const codexOnly = async (page: import('@playwright/test').Page) => {
    await page.addInitScript(installMock);
    await page.addInitScript(() => {
      const internals = (
        window as unknown as {
          __TAURI_INTERNALS__: { invoke: (c: string, a?: unknown) => Promise<unknown> };
        }
      ).__TAURI_INTERNALS__;
      const original = internals.invoke.bind(internals);
      internals.invoke = (cmd: string, args?: unknown) =>
        cmd === 'boot_status'
          ? original(cmd, args).then((b) => ({
              ...(b as object),
              claude: null,
              claudeVersion: null,
              codex: '/usr/local/bin/codex',
              codexVersion: '0.150.0',
              messaging: false,
            }))
          : original(cmd, args);
    });
    await page.goto('/');
    await expect(page.locator('.tab')).toHaveCount(1);
    await page.locator('.sidebar-foot').click();
    await page.getByTestId('sec-diagnostics').click();
  };

  test('names whose feature it is instead of nagging about a version', async ({ page }) => {
    await codexOnly(page);
    const panel = page.locator('.modal');
    await expect(panel).toContainText('這是 Claude Code 自己的功能');
    // The chore that used to be handed to somebody who never asked for it.
    await expect(panel).not.toContainText('需要 Claude Code');
  });

  /**
   * 兩件 Codex 的事實,寫下來而不是繞過去 —— 兩件都是 Codex 的性質,
   * 不是這張桌子的缺陷,所以正確的處理是說清楚,不是發明一個變通。
   */
  test('the diagnostics write down the two things Codex does differently', async ({ page }) => {
    await codexOnly(page);
    await expect(page.getByTestId('note-codex')).toContainText('/hooks');
    await expect(page.getByTestId('note-codex-idle')).toContainText('待命');
  });
});
