import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

async function createProjectWithLocalCandidate(
  request: import('@playwright/test').APIRequestContext,
  label: string,
): Promise<{ id: string; testid: string }> {
  const projectPath = resolve(`target/playwright/enable-source-${label}`, randomUUID());
  await mkdir(resolve(projectPath, '.git'), { recursive: true });
  await mkdir(resolve(projectPath, '.scratch/issues'), { recursive: true });
  const created = await request.post('/api/projects', {
    data: { path: projectPath },
  });
  await expect(created).toBeOK();
  const project = await created.json();
  return {
    id: project.id as string,
    testid: 'enable-issue-source-local_markdown-.scratch/issues',
  };
}

test('Enable Issue Source shows pending, transitions row to Enabled in place, no page reload', async ({
  page,
  request,
}) => {
  const { id: projectId, testid } = await createProjectWithLocalCandidate(
    request,
    'success',
  );

  await page.route(
    `**/api/projects/${projectId}/issue-source`,
    async (route) => {
      if (route.request().method() === 'PUT') {
        await new Promise((r) => setTimeout(r, 400));
        await route.continue();
      } else {
        await route.continue();
      }
    },
  );

  await page.goto(`/projects/${projectId}/source`);
  await expect(
    page.getByRole('heading', { name: 'Issue Source candidates' }),
  ).toBeVisible();

  await page.evaluate(() => {
    (window as unknown as { __enableMark?: string }).__enableMark = 'pre-click';
  });

  const button = page.getByTestId(testid);
  await expect(button).toContainText('Enable local_markdown');
  await button.click();

  // Mid-flight: pending state.
  await expect(button).toHaveAttribute('data-mutation-pending', 'true');
  await expect(button).toBeDisabled();

  // Settled: row swaps the ActionButton for a StatusPill labelled "Enabled".
  await expect(page.getByText('Enabled', { exact: true })).toBeVisible();
  await expect(button).toHaveCount(0);

  // No full page reload happened.
  const mark = await page.evaluate(
    () => (window as unknown as { __enableMark?: string }).__enableMark,
  );
  expect(mark).toBe('pre-click');
});

test('Enable Issue Source surfaces inline error on validation failure', async ({
  page,
  request,
}) => {
  const { id: projectId, testid } = await createProjectWithLocalCandidate(
    request,
    'error',
  );

  await page.route(
    `**/api/projects/${projectId}/issue-source`,
    async (route) => {
      if (route.request().method() === 'PUT') {
        await route.fulfill({
          status: 422,
          contentType: 'application/json',
          body: JSON.stringify({
            type: 'urn:agentic-afk:invalid-issue-source',
            title: 'Invalid Issue Source',
            status: 422,
            detail: 'Locator could not be validated',
          }),
        });
      } else {
        await route.continue();
      }
    },
  );

  await page.goto(`/projects/${projectId}/source`);
  await page.evaluate(() => {
    (window as unknown as { __enableErrMark?: string }).__enableErrMark =
      'pre-click';
  });

  const button = page.getByTestId(testid);
  await button.click();

  await expect(page.locator('[data-error-marker^="enable-issue-source-"]')).toContainText(
    'Invalid Issue Source',
  );

  // Button remains (row was not transitioned to Enabled), no reload.
  await expect(button).toBeVisible();
  const mark = await page.evaluate(
    () => (window as unknown as { __enableErrMark?: string }).__enableErrMark,
  );
  expect(mark).toBe('pre-click');
});
