/**
 * Issue #35 — Project Activity sub-route recomposed from primitives.
 *
 * Activity is a Card whose body renders an `EmptyState` when no entries
 * exist and a `LoadingSkeleton` while the snapshot is still hydrating.
 */
import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { mockProjectSnapshot } from './helpers/project-snapshot';

async function createTrustedProject(
  request: import('@playwright/test').APIRequestContext,
  label: string,
): Promise<string> {
  const projectPath = resolve(`target/playwright/activity-${label}`, randomUUID());
  await mkdir(resolve(projectPath, '.git'), { recursive: true });
  const created = await request.post('/api/projects', { data: { path: projectPath } });
  await expect(created).toBeOK();
  const project = await created.json();
  const trusted = await request.put(`/api/projects/${project.id}/trust`);
  await expect(trusted).toBeOK();
  return project.id as string;
}

test.describe('Activity', () => {
  test('empty state renders EmptyState card', async ({ page, request }) => {
    const projectId = await createTrustedProject(request, 'empty');
    await page.goto(`/projects/${projectId}/activity`);
    await expect(
      page.getByRole('heading', { level: 2, name: 'Activity' }),
    ).toBeVisible();
    await expect(
      page.getByText('No Activity recorded yet.', { exact: true }),
    ).toBeVisible();
  });

  test('loaded state renders activity entries', async ({ page, request }) => {
    const projectId = await createTrustedProject(request, 'loaded');
    await mockProjectSnapshot(page, projectId, () => ({
      activity: [
        {
          id: 'a1',
          project_id: projectId,
          kind: 'AssignmentCreated',
          recorded_at: '2026-05-22T12:00:00Z',
          detail: 'Sample detail',
          assignment_id: 'assn-1',
        },
      ],
    }));

    await page.goto(`/projects/${projectId}/activity`);
    await expect(page.getByText('AssignmentCreated', { exact: true })).toBeVisible();
    await expect(page.getByText('Sample detail', { exact: true })).toBeVisible();
    await expect(page.getByText('assignment assn-1', { exact: true })).toBeVisible();
  });

  test('loading state renders LoadingSkeleton scanlines', async ({ page, request }) => {
    const projectId = await createTrustedProject(request, 'loading');
    let resolveLater = () => {};
    const gate = new Promise<void>((res) => {
      resolveLater = res;
    });
    await page.route(
      `**/api/projects/${projectId}/snapshot**`,
      async (route) => {
        await gate;
        await route.continue();
      },
    );

    await page.goto(`/projects/${projectId}/activity`);
    // Pre-hydration: ActivitySection renders SkeletonHeading + SkeletonLines.
    await expect(page.locator('.hud-scanline').first()).toBeVisible();
    resolveLater();
  });
});
