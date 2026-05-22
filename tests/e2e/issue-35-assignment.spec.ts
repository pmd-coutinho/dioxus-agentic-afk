/**
 * Issue #35 — Project Assignment sub-route recomposed from primitives.
 *
 * The active Issue Assignment surface is a Card whose Lifecycle Status is a
 * `StatusPill` and whose Refresh Proposal State / Recover Assignment /
 * Abandon Assignment buttons are `ActionButton`s bound to their respective
 * `MutationKey`. The empty state is rendered through `EmptyState`.
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
  const projectPath = resolve(`target/playwright/assignment-${label}`, randomUUID());
  await mkdir(resolve(projectPath, '.git'), { recursive: true });
  const created = await request.post('/api/projects', { data: { path: projectPath } });
  await expect(created).toBeOK();
  const project = await created.json();
  const trusted = await request.put(`/api/projects/${project.id}/trust`);
  await expect(trusted).toBeOK();
  return project.id as string;
}

function assignment(id: string, status: string, projectId: string, extras: Partial<Record<string, unknown>> = {}) {
  return {
    id,
    project_id: projectId,
    source_id: 'issue-A',
    source_title: 'Sample Issue',
    branch: 'agent/issue-a',
    worktree_path: '/tmp/wt',
    status,
    status_detail: null,
    change_proposal: null,
    latest_attempt: null,
    repair_budget: null,
    ...extras,
  };
}

test.describe('Assignment', () => {
  test('blocked assignment renders Failed pill plus Recover and Abandon ActionButtons', async ({
    page,
    request,
  }) => {
    const projectId = await createTrustedProject(request, 'blocked');
    await mockProjectSnapshot(page, projectId, () => ({
      assignment_state: {
        active_assignment: assignment('assn-1', 'blocked', projectId),
        waiting_ready_issue_count: 0,
      },
    }));

    await page.goto(`/projects/${projectId}/assignment`);
    await expect(
      page.getByRole('heading', { level: 2, name: 'Issue Assignment' }),
    ).toBeVisible();
    // derive_assignment_lifecycle_pill("blocked") → (Failed, "Blocked").
    await expect(page.getByText('Blocked', { exact: true })).toBeVisible();
    await expect(page.getByTestId('recover-assignment-button')).toBeVisible();
    await expect(page.getByTestId('abandon-assignment-button')).toBeVisible();
    await expect(page.getByTestId('refresh-proposal-state-button')).toHaveCount(0);
  });

  test('proposal_pending assignment renders Running pill and Refresh Proposal State ActionButton', async ({
    page,
    request,
  }) => {
    const projectId = await createTrustedProject(request, 'proposal-pending');
    await mockProjectSnapshot(page, projectId, () => ({
      assignment_state: {
        active_assignment: assignment('assn-1', 'proposal_pending', projectId),
        waiting_ready_issue_count: 2,
      },
    }));

    await page.goto(`/projects/${projectId}/assignment`);
    await expect(page.getByText('Awaiting checks', { exact: true })).toBeVisible();
    await expect(page.getByTestId('refresh-proposal-state-button')).toBeVisible();
    await expect(page.getByText(/2 eligible Ready Issue waiting/i)).toBeVisible();
  });

  test('repair budget renders its own StatusPill alongside lifecycle', async ({
    page,
    request,
  }) => {
    const projectId = await createTrustedProject(request, 'repair');
    await mockProjectSnapshot(page, projectId, () => ({
      assignment_state: {
        active_assignment: assignment('assn-1', 'proposal_pending', projectId, {
          repair_budget: { attempt_count: 1, max_attempts: 3, window_seconds: 600 },
        }),
        waiting_ready_issue_count: 0,
      },
    }));

    await page.goto(`/projects/${projectId}/assignment`);
    await expect(page.getByText('Repair 1/3', { exact: true })).toBeVisible();
  });

  test('no active Assignment renders EmptyState', async ({ page, request }) => {
    const projectId = await createTrustedProject(request, 'no-active');
    await mockProjectSnapshot(page, projectId, () => ({}));

    await page.goto(`/projects/${projectId}/assignment`);
    await expect(
      page.getByText('No active Assignment', { exact: true }),
    ).toBeVisible();
  });
});
