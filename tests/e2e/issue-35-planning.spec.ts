/**
 * Issue #35 — Project Planning sub-route recomposed from primitives.
 *
 * Planning is a Card wrapping the five group sections (Eligible Ready
 * Issues, Active Issues, Blocked Ready Issues, Completed Issues, Non-ready
 * Source Issues). After issue #47 retired the proposal-era execution path,
 * Plan Run owns kicking off issue work — Planning groups are now read-only.
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
  const projectPath = resolve(`target/playwright/planning-${label}`, randomUUID());
  await mkdir(resolve(projectPath, '.git'), { recursive: true });
  const created = await request.post('/api/projects', { data: { path: projectPath } });
  await expect(created).toBeOK();
  const project = await created.json();
  const trusted = await request.put(`/api/projects/${project.id}/trust`);
  await expect(trusted).toBeOK();
  return project.id as string;
}

const EMPTY_PLANNING = {
  source: { kind: 'local-fs', locator: 'issues/' },
  last_successful_sync_at: null,
  last_failure: null,
  eligible: [],
  active: [],
  blocked: [],
  completed: [],
  non_ready: [],
};

function eligibleIssue(suffix: string) {
  return {
    source_id: `issue-${suffix}`,
    title: `Ready issue ${suffix}`,
    readiness: 'ready',
    lifecycle_status: 'todo',
    parent_issue: null,
    issue_dependencies: [],
    source_order: 1,
    raw_text: '',
  };
}

test.describe('Planning', () => {
  test('loaded state renders the Planning Card with five groups', async ({
    page,
    request,
  }) => {
    const projectId = await createTrustedProject(request, 'loaded');
    await mockProjectSnapshot(page, projectId, () => ({
      planning_snapshot: { ...EMPTY_PLANNING, eligible: [eligibleIssue('A')] },
    }));

    await page.goto(`/projects/${projectId}/planning`);

    await expect(
      page.getByRole('heading', { level: 2, name: 'Planning snapshot' }),
    ).toBeVisible();
    for (const group of [
      'Eligible Ready Issues',
      'Active Issues',
      'Blocked Ready Issues',
      'Completed Issues',
      'Non-ready Source Issues',
    ]) {
      await expect(page.getByRole('heading', { level: 3, name: group })).toBeVisible();
    }
    await expect(page.getByText('Ready issue A', { exact: true })).toBeVisible();
    // Eligible Ready Issues are read-only after the proposal-era Start
    // Assignment surface was retired (issue #47).
    await expect(page.getByTestId('start-assignment-button')).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Start Assignment' })).toHaveCount(0);
  });

  test('empty groups render the "None" sentinel inside the group section', async ({
    page,
    request,
  }) => {
    const projectId = await createTrustedProject(request, 'empty');
    await mockProjectSnapshot(page, projectId, () => ({
      planning_snapshot: EMPTY_PLANNING,
    }));
    await page.goto(`/projects/${projectId}/planning`);

    // All five groups present and each carries the "None" empty marker.
    const noneMarkers = page.getByText('None', { exact: true });
    await expect(noneMarkers).toHaveCount(5);
  });

  test('sync failure renders ErrorState above the groups', async ({
    page,
    request,
  }) => {
    const projectId = await createTrustedProject(request, 'sync-failed');
    await mockProjectSnapshot(page, projectId, () => ({
      planning_snapshot: {
        ...EMPTY_PLANNING,
        last_failure: 'failed to read local markdown Issue Source: no such file',
      },
    }));
    await page.goto(`/projects/${projectId}/planning`);

    await expect(page.getByText('Error', { exact: true })).toBeVisible();
    await expect(
      page.getByRole('heading', { name: /issue source sync failed/i }),
    ).toBeVisible();
  });
});
