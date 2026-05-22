/**
 * Issue #35 — Project Overview sub-route recomposed from primitives.
 *
 * Overview renders three Cards: Project metadata, Issue Assignment summary,
 * and Git Summary. Each card has explicit loading / empty / error states
 * built from the primitive library.
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
  const projectPath = resolve(`target/playwright/overview-${label}`, randomUUID());
  await mkdir(resolve(projectPath, '.git'), { recursive: true });
  const created = await request.post('/api/projects', { data: { path: projectPath } });
  await expect(created).toBeOK();
  const project = await created.json();
  const trusted = await request.put(`/api/projects/${project.id}/trust`);
  await expect(trusted).toBeOK();
  return project.id as string;
}

function assignment(id: string, status: string, projectId: string) {
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
  };
}

test.describe('Overview', () => {
  test('loaded state renders Project, Assignment, and Git Summary cards', async ({
    page,
    request,
  }) => {
    const projectId = await createTrustedProject(request, 'loaded');
    await mockProjectSnapshot(page, projectId, () => ({
      project: {
        id: projectId,
        path: '/tmp/playwright',
        trusted: true,
        git_summary: { branch: 'master', head: 'deadbeefcafe', dirty: false },
        enabled_issue_source: null,
      },
      assignment_state: {
        active_assignment: assignment('assn-1', 'proposal_pending', projectId),
        waiting_ready_issue_count: 0,
      },
    }));

    await page.goto(`/projects/${projectId}`);

    // All three Cards render as h2 headings via CardHead.
    await expect(
      page.getByRole('heading', { level: 2, name: 'Project' }),
    ).toBeVisible();
    await expect(
      page.getByRole('heading', { level: 2, name: 'Issue Assignment' }),
    ).toBeVisible();
    await expect(
      page.getByRole('heading', { level: 2, name: 'Git Summary' }),
    ).toBeVisible();

    // Trust StatusPill — Verified/Trusted when trusted.
    await expect(page.getByText('Trusted', { exact: true })).toBeVisible();

    // Assignment Lifecycle StatusPill — derive_assignment_lifecycle_pill maps
    // proposal_pending to "Awaiting checks".
    await expect(
      page.getByText('Awaiting checks', { exact: true }),
    ).toBeVisible();

    // Git Summary KeyValueList rows. Card scoped to disambiguate the
    // master branch text from the GitSummary StatusPill label.
    const gitCard = page
      .getByRole('heading', { level: 2, name: 'Git Summary' })
      .locator('xpath=ancestor::article');
    await expect(gitCard.getByText('Branch', { exact: true })).toBeVisible();
    await expect(gitCard.getByRole('definition').filter({ hasText: 'master' })).toBeVisible();
  });

  test('empty Assignment card renders EmptyState when no active assignment', async ({
    page,
    request,
  }) => {
    const projectId = await createTrustedProject(request, 'empty-assn');
    await mockProjectSnapshot(page, projectId, () => ({
      project: {
        id: projectId,
        path: '/tmp/playwright',
        trusted: true,
        git_summary: null,
        enabled_issue_source: null,
      },
    }));

    await page.goto(`/projects/${projectId}`);
    await expect(
      page.getByText('No active Assignment', { exact: true }),
    ).toBeVisible();
  });

  test('error state renders ErrorState when project fetch fails', async ({
    page,
  }) => {
    // Unknown Project IDs return 404 from the bare `/api/projects/:id`
    // endpoint, which routes the Layout into its `ErrorState` branch.
    await page.goto(`/projects/does-not-exist`);
    await expect(page.getByText('Error', { exact: true })).toBeVisible();
    await expect(
      page.getByRole('heading', { name: /project unavailable/i }),
    ).toBeVisible();
  });

  test('loading state renders skeleton scanlines', async ({ page, request }) => {
    const projectId = await createTrustedProject(request, 'loading');
    let resolveLater = () => {};
    const gate = new Promise<void>((res) => {
      resolveLater = res;
    });
    await page.route(`**/api/projects/${projectId}`, async (route) => {
      await gate;
      await route.continue();
    });
    await page.goto(`/projects/${projectId}`);
    // LoadingSkeleton scanline class while gated.
    await expect(page.locator('.hud-scanline').first()).toBeVisible();
    resolveLater();
  });
});
