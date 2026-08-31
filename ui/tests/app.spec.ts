import { test, expect, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { installMock } from './mock-tauri';

const here = dirname(fileURLToPath(import.meta.url));

/** Real Claude Code TUI output, captured from a PTY and split mid-character. */
const tui = JSON.parse(
  readFileSync(join(here, 'fixtures/claude-tui.json'), 'utf8'),
) as { totalBytes: number; splitAt: number; chunks: string[] };

async function boot(page: Page) {
  await page.addInitScript(installMock);
  await page.goto('/');
  await expect(page.locator('.sidebar')).toBeVisible();
  // The arrangement lives on a tab, so nothing renders until one exists.
  await expect(page.locator('.tab')).toHaveCount(1);
}

async function newSession(page: Page, cwd: string) {
  await page.locator('.sidebar-head button.icon').click();
  await expect(page.locator('.modal')).toBeVisible();
  await page.locator('.modal input.mono').first().fill(cwd);
  await page.locator('.modal button.primary').click();
  await expect(page.locator('.modal')).toHaveCount(0);
}

/**
 * What is actually on screen, read from xterm's buffer.
 *
 * Not the DOM: with the WebGL renderer loaded, xterm paints to a canvas and
 * leaves the row elements empty, so scraping them would assert nothing.
 */
async function screenText(page: Page, index = 0): Promise<string> {
  return page.locator('.pane .term-host').nth(index).evaluate((el) => {
    const term = (el as HTMLElement & { __term?: any }).__term;
    if (!term) return '';
    const buf = term.buffer.active;
    const lines: string[] = [];
    for (let i = 0; i < buf.length; i++) {
      lines.push(buf.getLine(i)?.translateToString(true) ?? '');
    }
    return lines.join('\n');
  });
}

/** Wait until a pane has painted something. */
async function waitForPaint(page: Page, index = 0) {
  await expect
    .poll(async () => (await screenText(page, index)).trim().length, { timeout: 5000 })
    .toBeGreaterThan(0);
}

test.describe('sessions', () => {
  test('a second session can be opened while the first is running', async ({ page }) => {
    await boot(page);

    await newSession(page, '/Users/test/repo-one');
    await expect(page.locator('.session-row')).toHaveCount(1);
    await expect(page.locator('.pane')).toHaveCount(1);

    // The reported bug: with one session already open, the second never
    // appeared.
    await newSession(page, '/Users/test/repo-two');
    await expect(page.locator('.session-row')).toHaveCount(2);
    await expect(page.locator('.pane')).toHaveCount(2);

    // Both directories are listed, newest first, and both are on screen: the
    // layout grows to hold what you open rather than hiding all but one.
    await expect(page.locator('.row-title')).toContainText(['repo-two', 'repo-one']);
    await expect(page.locator('.term-host:visible')).toHaveCount(2);
  });

  test('a session already on screen is focused rather than added twice', async ({ page }) => {
    await boot(page);
    await newSession(page, '/Users/test/repo-one');
    await newSession(page, '/Users/test/repo-two');
    await expect(page.locator('.term-host:visible')).toHaveCount(2);

    // It has one PTY and therefore one size; two panes showing it would
    // resize it against itself.
    await page.locator('[data-testid="session-s1"]').click();
    await expect(page.locator('.pane')).toHaveCount(2);
    await expect(page.locator('.term-host:visible')).toHaveCount(2);
    await expect(page.locator('.topbar strong')).toHaveText('repo-one');
  });

  test('the new-session dialog opens above a running terminal', async ({ page }) => {
    await boot(page);
    await newSession(page, '/Users/test/repo-one');

    await page.locator('.sidebar-head button.icon').click();
    const modal = page.locator('.modal');
    await expect(modal).toBeVisible();
    // A terminal canvas painting over the dialog would make it unclickable.
    await expect(page.locator('.modal button.primary')).toBeEnabled();
  });
});

test.describe('failure surfacing', () => {
  test('a spawn that fails says so instead of silently doing nothing', async ({ page }) => {
    await boot(page);

    // Make the next spawn fail the way the core would when the agent CLI is
    // missing from the login-shell PATH.
    await page.evaluate(() => {
      const internals = (window as unknown as Record<string, any>).__TAURI_INTERNALS__;
      const real = internals.invoke;
      internals.invoke = (cmd: string, args: Record<string, unknown>) =>
        cmd === 'new_session'
          ? Promise.reject('`claude` not found on the login-shell PATH')
          : real(cmd, args);
    });

    await newSession(page, '/Users/test/repo-one');

    // The dialog closing with no session and no message is the failure mode
    // that reads as a dead button.
    await expect(page.locator('.toast.error')).toBeVisible();
    await expect(page.locator('.toast.error')).toContainText('not found');
    await expect(page.locator('.session-row')).toHaveCount(0);
  });
});

test.describe('terminal rendering', () => {
  test('a chunk boundary inside a character does not corrupt the frame', async ({ page }) => {
    await boot(page);
    await newSession(page, '/Users/test/repo-one');
    await expect(page.locator('.pane')).toHaveCount(1);

    // Feed the real capture the way the PTY does: two chunks, the boundary
    // falling inside a multi-byte box-drawing character.
    await page.evaluate(
      ([a, b]) => {
        window.__mock.feed('s1', a, 1);
        window.__mock.feed('s1', b, 2);
      },
      tui.chunks,
    );

    await waitForPaint(page);
    const text = await screenText(page);

    // U+FFFD is what a per-chunk UTF-8 decode leaves behind when a character
    // straddles the boundary. Its absence is the whole point of passing bytes.
    expect(text).not.toContain('�');
    // And the frame really is drawn from the multi-byte characters at risk.
    expect(text).toMatch(/[─│╭╮╰╯]/);
  });

  test('the fixture really is a trap: decoding per chunk would corrupt it', async ({ page }) => {
    await boot(page);
    await newSession(page, '/Users/test/repo-one');

    // Guard on the guard. If the capture ever stopped splitting a character,
    // the test above would pass for the wrong reason and quietly stop
    // protecting anything. Decode each chunk in isolation — what the Rust
    // side used to do — and confirm that path really does produce U+FFFD.
    const corrupted = await page.evaluate(([a, b]) => {
      const dec = new TextDecoder('utf-8');        // no stream option: per-chunk
      const bytes = (s: string) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
      return dec.decode(bytes(a)) + dec.decode(bytes(b));
    }, tui.chunks);

    expect(corrupted).toContain('\uFFFD');
  });

  test('rows sit flush so box drawing joins up', async ({ page }) => {
    await boot(page);
    await newSession(page, '/Users/test/repo-one');
    await page.evaluate(
      ([a, b]) => {
        window.__mock.feed('s1', a, 1);
        window.__mock.feed('s1', b, 2);
      },
      tui.chunks,
    );
    await waitForPaint(page);

    // Two things make a box-drawn frame come apart between rows: extra
    // leading from a line-height above 1, and a fractional cell height, which
    // leaves a sub-pixel seam on every row boundary.
    const metrics = await page.locator('.pane .term-host').first().evaluate((el) => {
      const term = (el as HTMLElement & { __term?: any }).__term;
      const screen = el.querySelector('.xterm-screen') as HTMLElement;
      return {
        lineHeight: term.options.lineHeight,
        rows: term.rows,
        screenHeight: screen.getBoundingClientRect().height,
      };
    });

    expect(metrics.lineHeight).toBe(1);
    expect(metrics.rows).toBeGreaterThan(0);

    const cellHeight = metrics.screenHeight / metrics.rows;
    expect(cellHeight).toBeGreaterThan(0);
    expect(Math.abs(cellHeight - Math.round(cellHeight))).toBeLessThan(0.01);
  });

  test('output produced before the pane mounts is replayed, not lost', async ({ page }) => {
    await boot(page);

    // Arm a snapshot for the session that is about to be created, standing in
    // for a PTY that painted before React mounted its pane.
    await page.evaluate((chunk) => {
      const orig = window.__mock.snapshots;
      // The mock assigns ids in order, so the next session is s1.
      orig.set('s1', { data: chunk, seq: 5 });
    }, tui.chunks[0]);

    await newSession(page, '/Users/test/repo-one');
    await waitForPaint(page);

    // A live chunk the snapshot already covers must not be written twice.
    const before = (await screenText(page)).length;
    await page.evaluate((chunk) => window.__mock.feed('s1', chunk, 3), tui.chunks[0]);
    await page.waitForTimeout(200);
    expect((await screenText(page)).length).toBe(before);
  });
});

/**
 * How wide the wall may be.
 *
 * The count used to stop at three, which was a constant with nothing behind
 * it: the model has always drawn as many columns as it was given, and the
 * renderer bounds that by the number of panes rather than by any number of
 * ours. So the list runs to one column per pane — the widest arrangement
 * that can differ from any other — and the person decides where in it to sit.
 */
test.describe('how many columns', () => {
  const options = (page: Page) =>
    page.getByTestId('col-picker').locator('option').allTextContents();

  test('the choices grow with the wall, past the old three', async ({ page }) => {
    await boot(page);
    for (const n of ['one', 'two', 'three', 'four', 'five']) {
      await newSession(page, `/Users/test/repo-${n}`);
    }
    await expect(page.locator('.pane')).toHaveCount(5);

    const offered = await options(page);
    expect(offered).toContain('5 欄');
    expect(offered).toContain('4 欄');
    // And nothing beyond the wall: a sixth column with five panes draws the
    // same five, so offering it would be a longer list, not more choice.
    expect(offered).not.toContain('6 欄');
  });

  test('a wall of five can actually be laid out five across', async ({ page }) => {
    await boot(page);
    for (const n of ['one', 'two', 'three', 'four', 'five']) {
      await newSession(page, `/Users/test/repo-${n}`);
    }
    await page.getByTestId('col-picker').selectOption('5');

    // The desk's own declaration of the count...
    await expect(page.locator('.term-stack')).toHaveAttribute('data-cols', '5');
    // ...and the grid it actually built from it, five tracks across one row.
    const tracks = await page
      .locator('.term-grid')
      .evaluate((el) => getComputedStyle(el).gridTemplateColumns.split(' ').length);
    expect(tracks).toBe(5);
  });

  test('the choice survives a reload, and the picker still offers it', async ({ page }) => {
    await boot(page);
    for (const n of ['one', 'two', 'three', 'four']) {
      await newSession(page, `/Users/test/repo-${n}`);
    }
    await page.getByTestId('col-picker').selectOption('4');
    await page.reload();
    await expect(page.locator('.pane')).toHaveCount(4);
    await expect(page.getByTestId('col-picker')).toHaveValue('4');
  });

  /** An empty desk keeps the choices it always had rather than collapsing to
      one, so the control does not change shape underneath somebody. */
  test('an empty wall still offers three', async ({ page }) => {
    await boot(page);
    expect(await options(page)).toContain('3 欄');
  });
});
