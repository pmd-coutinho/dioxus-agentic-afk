import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

test('Auto-Replan empty and blocked pauses render banner copy with Resume', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/auto-replan', randomUUID());
  await mkdir(resolve(projectPath, '.git'), { recursive: true });
  await mkdir(resolve(projectPath, 'issues'), { recursive: true });

  const created = await request.post('/api/projects', {
    data: { path: projectPath },
  });
  await expect(created).toBeOK();
  const project = await created.json();

  await request.put(`/api/projects/${project.id}/trust`);
  await request.put(`/api/projects/${project.id}/issue-source`, {
    data: { kind: 'local_markdown', locator: 'issues' },
  });

  await page.goto(`/projects/${project.id}`);
  const autoReplan = page.getByTestId('auto-replan-action');
  await expect(autoReplan).toContainText('Arm');
  await autoReplan.click();
  await expect(page.getByText('Armed')).toBeVisible();

  const tick = await request.post('/api/_test/auto-replan/tick');
  await expect(tick).toBeOK();

  const banner = page.getByTestId('auto-replan-paused-banner');
  await expect(banner).toBeVisible();
  await expect(banner).toContainText(
    'Empty backlog: no Ready Issues left to plan.',
  );
  await expect(page.getByTestId('auto-replan-banner-resume')).toBeVisible();
  await expect(page.getByTestId('auto-replan-banner-disarm')).toBeVisible();

  await page.getByTestId('auto-replan-banner-resume').click();
  await expect(page.getByText('Armed')).toBeVisible();

  const blocked = await request.post(
    `/api/_test/projects/${project.id}/auto-replan/pause`,
    { data: { reason: 'assignment_blocked' } },
  );
  await expect(blocked).toBeOK();
  await expect(banner).toContainText(
    'Assignment blocked: resolve via Re-Enable.',
  );
});
