import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

test('Trust Project mutation runs without a page reload and announces success', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/trust-project', randomUUID());
  await mkdir(resolve(projectPath, '.git'), { recursive: true });

  const created = await request.post('/api/projects', {
    data: { path: projectPath },
  });
  await expect(created).toBeOK();
  const project = await created.json();

  // Delay the trust PUT so we can observe the pending UI state.
  await page.route(`**/api/projects/${project.id}/trust`, async (route) => {
    await new Promise((r) => setTimeout(r, 400));
    await route.continue();
  });

  await page.goto(`/projects/${project.id}`);
  await expect(page.getByText('Not trusted')).toBeVisible();

  // Drop a marker on the window; a full reload would clear it.
  await page.evaluate(() => {
    (window as unknown as { __trustProjectMark?: string }).__trustProjectMark =
      'pre-click';
  });

  const button = page.getByTestId('trust-project-button');
  await expect(button).toHaveText('Trust Project');
  await button.click();

  // Mid-flight: button disabled and labeled pending.
  await expect(button).toBeDisabled();
  await expect(button).toHaveAttribute('data-mutation-pending', 'true');

  // Settled: project is trusted, button gone, success toast visible.
  await expect(
    page.getByText('Trusted for agent execution'),
  ).toBeVisible();
  await expect(
    page.locator('[data-toast-kind="success"]', {
      hasText: 'Project trusted',
    }),
  ).toBeVisible();

  // No full page reload happened.
  const mark = await page.evaluate(
    () =>
      (window as unknown as { __trustProjectMark?: string })
        .__trustProjectMark,
  );
  expect(mark).toBe('pre-click');
});
