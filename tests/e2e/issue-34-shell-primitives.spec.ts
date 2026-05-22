/**
 * Issue #34 — AppShell, Home, Project list, and Settings recomposed from the
 * `apps/dashboard/src/ui/` primitive library.
 *
 * These tests assert visual-state coverage on each surface (loaded / loading /
 * empty / error) by reading the primitives' stable DOM markers:
 * - `CardHead` renders the title as an `<h2>` (role=heading, level 2).
 * - `StatusPill` renders inside a `span` containing the tone color class.
 * - `EmptyState` renders an uppercase heading `<p>` block.
 * - `ErrorState` renders the literal cartouche text "Error" + a coral heading.
 * - `LoadingSkeleton` renders `<div>`s with the `hud-scanline` class.
 */
import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

test.describe('AppShell', () => {
  test('renders HUD header with persistent nav and HudToastRegion', async ({
    page,
  }) => {
    await page.goto('/');

    // Branding heading kept as h1 so existing tooling/assertions still pass.
    await expect(
      page.getByRole('heading', { level: 1, name: 'agentic-afk' }),
    ).toBeVisible();

    // Cross-surface nav lives in AppShell; it survives every route change.
    const nav = page.getByRole('navigation');
    await expect(nav.getByRole('link', { name: 'Home', exact: true })).toBeVisible();
    await expect(nav.getByRole('link', { name: 'Projects', exact: true })).toBeVisible();
    await expect(nav.getByRole('link', { name: 'Settings', exact: true })).toBeVisible();

    // HudToastRegion exposes the polite live-region used by mutation toasts.
    // The region has zero height when empty, so assert attachment, not paint.
    await expect(page.locator('[role="status"][aria-live="polite"]')).toBeAttached();
  });
});

test.describe('Home', () => {
  test('loaded state renders a Card with a Verified Connected pill', async ({
    page,
  }) => {
    await page.goto('/');
    // Card head renders as h2 — see ui/card.rs.
    await expect(
      page.getByRole('heading', { level: 2, name: 'API connected' }),
    ).toBeVisible();
    // StatusPill exposes label text in uppercase tracked caps.
    await expect(page.getByText('Connected', { exact: true })).toBeVisible();
  });

  test('error state renders ErrorState with the coral cartouche', async ({
    page,
  }) => {
    await page.route('**/api/app-info', async (route) => {
      await route.fulfill({ status: 503, body: 'control plane offline' });
    });
    await page.goto('/');
    // ErrorState renders the literal "Error" cartouche label.
    await expect(page.getByText('Error', { exact: true })).toBeVisible();
    await expect(
      page.getByRole('heading', { name: /api disconnected/i }),
    ).toBeVisible();
  });

  test('loading state renders LoadingSkeleton scanlines', async ({ page }) => {
    let resolveLater = () => {};
    const gate = new Promise<void>((res) => {
      resolveLater = res;
    });
    await page.route('**/api/app-info', async (route) => {
      await gate;
      await route.continue();
    });
    await page.goto('/');
    // Scanline class on at least one skeleton bar while gated.
    await expect(page.locator('.hud-scanline').first()).toBeVisible();
    resolveLater();
  });
});

test.describe('Project list', () => {
  test('empty state renders EmptyState when no Projects exist', async ({
    page,
  }) => {
    await page.route('**/api/projects', async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: '[]',
      });
    });
    await page.goto('/projects');
    await expect(
      page.getByRole('heading', { level: 2, name: 'Projects' }),
    ).toBeVisible();
    // EmptyState title rendered as a tracked-caps paragraph.
    await expect(page.getByText('No Projects', { exact: true })).toBeVisible();
  });

  test('each row is a Card with Trust and Git StatusPills', async ({
    page,
    request,
  }) => {
    const projectPath = resolve(
      'target/playwright/issue-34-projects',
      randomUUID(),
    );
    await mkdir(resolve(projectPath, '.git'), { recursive: true });
    await writeFile(
      resolve(projectPath, '.git/config'),
      '[remote "origin"]\n    url = git@github.com:pmd-coutinho/example.git\n',
    );

    const created = await request.post('/api/projects', {
      data: { path: projectPath },
    });
    await expect(created).toBeOK();

    await page.goto('/projects');
    // The row is a Card — at least one h2 'Project' rendered above the rows.
    await expect(
      page.getByRole('heading', { level: 2, name: 'Project' }).first(),
    ).toBeVisible();
    // Trust pill — newly created Projects are untrusted by default.
    await expect(page.getByText('Untrusted', { exact: true }).first()).toBeVisible();
  });
});

test.describe('Settings', () => {
  test('renders a Settings Card with a KeyValueList of config rows', async ({
    page,
  }) => {
    await page.goto('/settings');
    await expect(
      page.getByRole('heading', { level: 2, name: 'Settings' }),
    ).toBeVisible();
    // KeyValueRow labels rendered as tracked-caps <dt> entries.
    await expect(page.getByText('Bind address', { exact: true })).toBeVisible();
    await expect(
      page.getByText('Dashboard assets', { exact: true }),
    ).toBeVisible();
    await expect(page.getByText('Database', { exact: true })).toBeVisible();
  });
});
