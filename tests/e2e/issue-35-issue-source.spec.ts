/**
 * Issue #35 — Project Issue Source sub-route recomposed from primitives.
 *
 * Sync status is a Card with a `StatusPill` for the lifecycle state plus a
 * Refresh `ActionButton`. The candidate list is a Card whose empty state
 * uses `EmptyState`. Each candidate row renders either an Enable
 * `ActionButton` or, once enabled, a Verified `StatusPill`.
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
  const projectPath = resolve(`target/playwright/source-${label}`, randomUUID());
  await mkdir(resolve(projectPath, '.git'), { recursive: true });
  const created = await request.post('/api/projects', { data: { path: projectPath } });
  await expect(created).toBeOK();
  const project = await created.json();
  const trusted = await request.put(`/api/projects/${project.id}/trust`);
  await expect(trusted).toBeOK();
  return project.id as string;
}

const ENABLED_PROJECT = (projectId: string) => ({
  id: projectId,
  path: '/tmp/playwright',
  trusted: true,
  git_summary: null,
  enabled_issue_source: { kind: 'local_markdown', locator: 'issues' },
});

test.describe('Issue Source', () => {
  test('idle state (Issue Source enabled, never synced) renders Idle pill and Refresh ActionButton', async ({
    page,
    request,
  }) => {
    const projectId = await createTrustedProject(request, 'idle');
    // Enable Issue Source via the real endpoint so the server's SSE replay
    // does not overwrite a mocked snapshot during the page's initial sync.
    const enabled = await request.put(
      `/api/projects/${projectId}/issue-source`,
      { data: { kind: 'local_markdown', locator: 'issues' } },
    );
    await expect(enabled).toBeOK();

    await page.goto(`/projects/${projectId}/source`);
    await expect(
      page.getByRole('heading', { level: 2, name: 'Last sync status' }),
    ).toBeVisible();
    // No successful sync yet, no failure → Idle pill labelled "Never synced".
    await expect(page.getByText('Never synced', { exact: true }).first()).toBeVisible();
    const refreshBtn = page.getByTestId('refresh-issue-source-button');
    await expect(refreshBtn).toBeVisible();
    await expect(refreshBtn).toHaveAttribute('data-mutation-pending', 'false');
  });

  test('empty candidate list renders EmptyState card', async ({
    page,
    request,
  }) => {
    const projectId = await createTrustedProject(request, 'empty-candidates');
    await mockProjectSnapshot(page, projectId, () => ({
      issue_source_candidates: [],
    }));

    await page.goto(`/projects/${projectId}/source`);
    await expect(
      page.getByRole('heading', { level: 2, name: 'Issue Source candidates' }),
    ).toBeVisible();
    await expect(page.getByText('No candidates', { exact: true })).toBeVisible();
  });

  test('candidate row renders Enable ActionButton with a stable testid', async ({
    page,
    request,
  }) => {
    const projectId = await createTrustedProject(request, 'candidates');
    await mockProjectSnapshot(page, projectId, () => ({
      issue_source_candidates: [
        { kind: 'local_markdown', locator: '.scratch/issues', enabled: false },
        { kind: 'github', locator: 'pmd-coutinho/example', enabled: true },
      ],
    }));

    await page.goto(`/projects/${projectId}/source`);
    await expect(
      page.getByTestId('enable-issue-source-local_markdown-.scratch/issues'),
    ).toBeVisible();
    // Already-enabled candidates render a Verified StatusPill in place of
    // the ActionButton.
    await expect(page.getByText('Enabled', { exact: true })).toBeVisible();
  });
});
