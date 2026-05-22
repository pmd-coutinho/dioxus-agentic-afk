import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

async function createProjectWithEnabledLocalSource(
  request: import('@playwright/test').APIRequestContext,
  label: string,
): Promise<string> {
  const projectPath = resolve(`target/playwright/refresh-source-${label}`, randomUUID());
  await mkdir(resolve(projectPath, '.git'), { recursive: true });
  await mkdir(resolve(projectPath, '.scratch/issues'), { recursive: true });

  const created = await request.post('/api/projects', {
    data: { path: projectPath },
  });
  await expect(created).toBeOK();
  const project = await created.json();

  const enabled = await request.put(
    `/api/projects/${project.id}/issue-source`,
    { data: { kind: 'local_markdown', locator: '.scratch/issues' } },
  );
  await expect(enabled).toBeOK();
  return project.id as string;
}

test('Refresh Issue Source shows pending, announces success via toast, no page reload', async ({
  page,
  request,
}) => {
  const projectId = await createProjectWithEnabledLocalSource(request, 'success');

  // Delay the sync POST so we can observe the pending UI state.
  await page.route(
    `**/api/projects/${projectId}/issue-source/sync`,
    async (route) => {
      await new Promise((r) => setTimeout(r, 400));
      await route.continue();
    },
  );

  await page.goto(`/projects/${projectId}/source`);
  await expect(page.getByRole('heading', { name: 'Last sync status' })).toBeVisible();

  // Marker survives only if there is no full page reload.
  await page.evaluate(() => {
    (window as unknown as { __syncMark?: string }).__syncMark = 'pre-click';
  });

  const button = page.getByTestId('refresh-issue-source-button');
  await expect(button).toHaveText('Refresh Issue Source');
  await button.click();

  // Mid-flight: ActionButton swaps to its Pending render-state (disabled
  // attribute + `data-mutation-pending="true"`). Label text stays stable —
  // the pending visual is the shimmer treatment on the same element.
  await expect(button).toHaveAttribute('data-mutation-pending', 'true');
  await expect(button).toBeDisabled();

  // Settled: success toast appears.
  await expect(
    page.locator('[data-toast-kind="success"]', {
      hasText: 'Issue Source synced',
    }),
  ).toBeVisible();

  // Button returns to idle state.
  await expect(button).toHaveAttribute('data-mutation-pending', 'false');
  await expect(button).toBeEnabled();

  // No full page reload happened.
  const mark = await page.evaluate(
    () => (window as unknown as { __syncMark?: string }).__syncMark,
  );
  expect(mark).toBe('pre-click');
});

test('Refresh Issue Source surfaces inline error on validation failure', async ({
  page,
  request,
}) => {
  const projectId = await createProjectWithEnabledLocalSource(request, 'error');

  await page.route(
    `**/api/projects/${projectId}/issue-source/sync`,
    async (route) => {
      await route.fulfill({
        status: 422,
        contentType: 'application/json',
        body: JSON.stringify({
          type: 'urn:agentic-afk:issue-source-sync-failed',
          title: 'Sync failed',
          status: 422,
          detail: 'Locator unreachable',
        }),
      });
    },
  );

  await page.goto(`/projects/${projectId}/source`);
  await page.evaluate(() => {
    (window as unknown as { __syncErrMark?: string }).__syncErrMark = 'pre-click';
  });

  const button = page.getByTestId('refresh-issue-source-button');
  await button.click();

  // Inline error shows next to the button.
  await expect(page.locator('[data-error-marker="refresh-issue-source"]')).toContainText(
    'Sync failed',
  );

  // No reload happened.
  const mark = await page.evaluate(
    () => (window as unknown as { __syncErrMark?: string }).__syncErrMark,
  );
  expect(mark).toBe('pre-click');
});

test('Refresh Issue Source preserves scroll position and focus through mutation', async ({
  page,
  request,
}) => {
  const projectId = await createProjectWithEnabledLocalSource(request, 'scroll');

  await page.route(
    `**/api/projects/${projectId}/issue-source/sync`,
    async (route) => {
      await new Promise((r) => setTimeout(r, 200));
      await route.continue();
    },
  );

  await page.goto(`/projects/${projectId}/source`);
  const button = page.getByTestId('refresh-issue-source-button');
  await expect(button).toBeVisible();

  // Force the page taller so scroll is possible.
  await page.evaluate(() => {
    const filler = document.createElement('div');
    filler.style.height = '2000px';
    filler.id = '__scroll_filler__';
    document.body.appendChild(filler);
    window.scrollTo(0, 600);
  });

  const scrollBefore = await page.evaluate(() => window.scrollY);
  expect(scrollBefore).toBeGreaterThan(0);

  await button.focus();
  await button.click();

  await expect(
    page.locator('[data-toast-kind="success"]', {
      hasText: 'Issue Source synced',
    }),
  ).toBeVisible();

  // Scroll position preserved (a page reload would reset to 0).
  const scrollAfter = await page.evaluate(() => window.scrollY);
  expect(scrollAfter).toBeGreaterThan(0);
  expect(Math.abs(scrollAfter - scrollBefore)).toBeLessThan(200);

  // Focus is preserved (a page reload would drop it to <body>). The button
  // stays interactive afterwards because Dioxus diffs DOM nodes rather than
  // re-mounting them.
  const focused = await page.evaluate(() => ({
    tag: document.activeElement?.tagName ?? null,
    testid: document.activeElement?.getAttribute('data-testid') ?? null,
  }));
  expect(focused.tag).toBe('BUTTON');
  expect(focused.testid).toBe('refresh-issue-source-button');
});
