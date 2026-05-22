/**
 * Issue #29: Assignment mutations migrated to ProjectStore.mutate().
 *
 * Each lifecycle button (Start, Abandon, Recover, Refresh Proposal State)
 * must:
 *   - disable while pending (data-mutation-pending="true")
 *   - surface RFC 7807 validation errors inline, not via toast
 *   - refetch dependent panels without a full page reload (a marker survives)
 *   - never duplicate POSTs when clicked repeatedly while pending
 *
 * The Local Control Plane only seeds a bare Project, so we mock the
 * assignment-state / planning-snapshot endpoints from Playwright to drive
 * each branch of the UI deterministically.
 */
import { expect, test, type Page, type Route } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import {
  mockProjectSnapshot,
  publishProjectEvent,
} from './helpers/project-snapshot';

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

function assignmentResponse(id: string, status: string, projectId: string) {
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

function eligibleIssue() {
  return {
    source_id: 'issue-A',
    title: 'Sample Issue',
    readiness: 'ready',
    lifecycle_status: 'todo',
    parent_issue: null,
    issue_dependencies: [],
    source_order: 1,
    raw_text: 'Sample Issue',
  };
}

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

async function setSpaMarker(page: Page) {
  await page.evaluate(() => {
    (window as unknown as { __spaMark?: string }).__spaMark = 'before-mutation';
  });
}

async function expectNoReload(page: Page) {
  const mark = await page.evaluate(
    () => (window as unknown as { __spaMark?: string }).__spaMark,
  );
  expect(mark).toBe('before-mutation');
}

test('Start Assignment: pending state disables button, success refetches, no reload', async ({
  page,
  request,
}) => {
  const projectId = await createTrustedProject(request, 'start-ok');
  let postCalls = 0;
  let planning = { ...EMPTY_PLANNING, eligible: [eligibleIssue()] };

  await mockProjectSnapshot(page, projectId, () => ({
    planning_snapshot: planning,
  }));

  await page.route(
    `**/api/projects/${projectId}/source-issues/issue-A/assignment`,
    async (route) => {
      postCalls += 1;
      await new Promise((r) => setTimeout(r, 400));
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(assignmentResponse('assn-1', 'proposal_pending', projectId)),
      });
    },
  );

  await page.goto(`/projects/${projectId}/planning`);
  const button = page.getByTestId('start-assignment-button').first();
  await expect(button).toBeVisible();

  await setSpaMarker(page);

  // Rapid clicks while pending must not duplicate POSTs.
  await button.click();
  await expect(button).toBeDisabled();
  await expect(button).toHaveAttribute('data-mutation-pending', 'true');
  await button.click({ force: true }).catch(() => {});
  await button.click({ force: true }).catch(() => {});

  // Simulate the live SSE delta that the real backend would emit after
  // start_assignment succeeds: the eligible issue moves into `active`.
  planning = { ...EMPTY_PLANNING, active: [eligibleIssue()] };
  await publishProjectEvent(request, projectId, {
    type: 'planning_snapshot_changed',
    snapshot: planning,
  });
  // The Dashboard's live store applies the delta and the panel re-renders.
  await expect(page.getByRole('heading', { name: 'Active Issues' })).toBeVisible();
  const eligibleGroup = page.getByRole('heading', { name: 'Eligible Ready Issues' }).locator('..');
  await expect(eligibleGroup.getByText('None')).toBeVisible();

  expect(postCalls).toBe(1);
  await expectNoReload(page);
});

test('Start Assignment: 422 validation error renders inline, no reload', async ({
  page,
  request,
}) => {
  const projectId = await createTrustedProject(request, 'start-err');

  await mockProjectSnapshot(page, projectId, () => ({
    planning_snapshot: { ...EMPTY_PLANNING, eligible: [eligibleIssue()] },
  }));

  await page.route(
    `**/api/projects/${projectId}/source-issues/issue-A/assignment`,
    async (route) => {
      await route.fulfill({
        status: 422,
        contentType: 'application/problem+json',
        body: JSON.stringify({
          type: 'about:blank',
          title: 'Untrusted Project',
          status: 422,
          detail: 'Project must be trusted',
        }),
      });
    },
  );

  await page.goto(`/projects/${projectId}/planning`);
  await setSpaMarker(page);

  const button = page.getByTestId('start-assignment-button').first();
  await button.click();

  const inlineError = page.locator('[data-error-marker="start-assignment"]').first();
  await expect(inlineError).toBeVisible();
  await expect(inlineError).toContainText('Untrusted Project');

  // No toast for validation errors.
  await expect(page.locator('[data-toast-kind="error"]')).toHaveCount(0);
  await expectNoReload(page);
});

test('Abandon Assignment: pending state, success refetches Assignment panel, no reload', async ({
  page,
  request,
}) => {
  const projectId = await createTrustedProject(request, 'abandon-ok');
  let postCalls = 0;

  await mockProjectSnapshot(page, projectId, () => ({
    assignment_state: {
      active_assignment: assignmentResponse('assn-1', 'blocked', projectId),
      waiting_ready_issue_count: 0,
    },
  }));

  await page.route(
    `**/api/projects/${projectId}/assignments/assn-1/abandon`,
    async (route) => {
      postCalls += 1;
      await new Promise((r) => setTimeout(r, 400));
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(assignmentResponse('assn-1', 'abandoned', projectId)),
      });
    },
  );

  await page.goto(`/projects/${projectId}/assignment`);
  const button = page.getByTestId('abandon-assignment-button');
  await expect(button).toBeVisible();

  await setSpaMarker(page);

  await button.click();
  await expect(button).toBeDisabled();
  await expect(button).toHaveAttribute('data-mutation-pending', 'true');

  // Live SSE delta: assignment transitions to terminal `abandoned` status,
  // which clears the active_assignment slot in the store.
  await publishProjectEvent(request, projectId, {
    type: 'assignment_status_changed',
    ...assignmentResponse('assn-1', 'abandoned', projectId),
  });

  await expect(page.getByText('No active Assignment')).toBeVisible();
  expect(postCalls).toBe(1);
  await expectNoReload(page);
});

test('Abandon Assignment: 422 validation error renders inline, no reload', async ({
  page,
  request,
}) => {
  const projectId = await createTrustedProject(request, 'abandon-err');

  await mockProjectSnapshot(page, projectId, () => ({
    assignment_state: {
      active_assignment: assignmentResponse('assn-1', 'blocked', projectId),
      waiting_ready_issue_count: 0,
    },
  }));

  await page.route(
    `**/api/projects/${projectId}/assignments/assn-1/abandon`,
    async (route) => {
      await route.fulfill({
        status: 422,
        contentType: 'application/problem+json',
        body: JSON.stringify({
          type: 'about:blank',
          title: 'Not abandonable',
          status: 422,
          detail: 'Assignment not in blocked state',
        }),
      });
    },
  );

  await page.goto(`/projects/${projectId}/assignment`);
  await setSpaMarker(page);

  await page.getByTestId('abandon-assignment-button').click();

  const inlineError = page.locator('[data-error-marker="abandon-assignment"]');
  await expect(inlineError).toBeVisible();
  await expect(inlineError).toContainText('Not abandonable');

  await expect(page.locator('[data-toast-kind="error"]')).toHaveCount(0);
  await expectNoReload(page);
});

test('Recover Assignment: pending state, success refetches, no reload', async ({
  page,
  request,
}) => {
  const projectId = await createTrustedProject(request, 'recover-ok');
  let postCalls = 0;

  await mockProjectSnapshot(page, projectId, () => ({
    assignment_state: {
      active_assignment: assignmentResponse('assn-1', 'blocked', projectId),
      waiting_ready_issue_count: 0,
    },
  }));

  await page.route(
    `**/api/projects/${projectId}/assignments/assn-1/recover`,
    async (route) => {
      postCalls += 1;
      await new Promise((r) => setTimeout(r, 400));
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(assignmentResponse('assn-1', 'proposal_pending', projectId)),
      });
    },
  );

  await page.goto(`/projects/${projectId}/assignment`);
  const button = page.getByTestId('recover-assignment-button');
  await expect(button).toBeVisible();

  await setSpaMarker(page);

  await button.click();
  await expect(button).toBeDisabled();
  await expect(button).toHaveAttribute('data-mutation-pending', 'true');

  // Live SSE delta: recover transitions assignment to proposal_pending.
  await publishProjectEvent(request, projectId, {
    type: 'assignment_status_changed',
    ...assignmentResponse('assn-1', 'proposal_pending', projectId),
  });

  await expect(page.getByText('Awaiting checks')).toBeVisible();
  await expect(page.getByTestId('recover-assignment-button')).toHaveCount(0);
  expect(postCalls).toBe(1);
  await expectNoReload(page);
});

test('Recover Assignment: 500 transient error keeps button enabled, no reload', async ({
  page,
  request,
}) => {
  const projectId = await createTrustedProject(request, 'recover-err');

  await mockProjectSnapshot(page, projectId, () => ({
    assignment_state: {
      active_assignment: assignmentResponse('assn-1', 'blocked', projectId),
      waiting_ready_issue_count: 0,
    },
  }));

  await page.route(
    `**/api/projects/${projectId}/assignments/assn-1/recover`,
    async (route) => {
      await route.fulfill({
        status: 500,
        contentType: 'application/problem+json',
        body: JSON.stringify({
          type: 'about:blank',
          title: 'Internal error',
          status: 500,
          detail: 'db unavailable',
        }),
      });
    },
  );

  await page.goto(`/projects/${projectId}/assignment`);
  await setSpaMarker(page);

  const button = page.getByTestId('recover-assignment-button');
  await button.click();

  // Transient -> toast (not inline).
  await expect(
    page.locator('[data-toast-kind="error"]', { hasText: 'Internal error' }),
  ).toBeVisible();
  // Button re-enabled after settle.
  await expect(button).toBeEnabled();
  await expectNoReload(page);
});

test('Refresh Proposal State: pending state, success refetches, no reload', async ({
  page,
  request,
}) => {
  const projectId = await createTrustedProject(request, 'refresh-ok');
  let postCalls = 0;

  await mockProjectSnapshot(page, projectId, () => ({
    assignment_state: {
      active_assignment: assignmentResponse('assn-1', 'proposal_pending', projectId),
      waiting_ready_issue_count: 0,
    },
  }));

  await page.route(
    `**/api/projects/${projectId}/assignments/assn-1/refresh-proposal-state`,
    async (route) => {
      postCalls += 1;
      await new Promise((r) => setTimeout(r, 400));
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(assignmentResponse('assn-1', 'proposal_verified', projectId)),
      });
    },
  );

  await page.goto(`/projects/${projectId}/assignment`);
  const button = page.getByTestId('refresh-proposal-state-button');
  await expect(button).toBeVisible();

  await setSpaMarker(page);

  await button.click();
  await expect(button).toBeDisabled();
  await expect(button).toHaveAttribute('data-mutation-pending', 'true');

  // Live SSE delta: proposal moves to verified status.
  await publishProjectEvent(request, projectId, {
    type: 'assignment_status_changed',
    ...assignmentResponse('assn-1', 'proposal_verified', projectId),
  });

  await expect(page.getByText('Verified')).toBeVisible();
  expect(postCalls).toBe(1);
  await expectNoReload(page);
});

test('Refresh Proposal State: 422 validation error renders inline, no reload', async ({
  page,
  request,
}) => {
  const projectId = await createTrustedProject(request, 'refresh-err');

  await mockProjectSnapshot(page, projectId, () => ({
    assignment_state: {
      active_assignment: assignmentResponse('assn-1', 'proposal_pending', projectId),
      waiting_ready_issue_count: 0,
    },
  }));

  await page.route(
    `**/api/projects/${projectId}/assignments/assn-1/refresh-proposal-state`,
    async (route: Route) => {
      await route.fulfill({
        status: 422,
        contentType: 'application/problem+json',
        body: JSON.stringify({
          type: 'about:blank',
          title: 'No Change Proposal',
          status: 422,
          detail: 'Assignment has no proposal to refresh',
        }),
      });
    },
  );

  await page.goto(`/projects/${projectId}/assignment`);
  await setSpaMarker(page);

  await page.getByTestId('refresh-proposal-state-button').click();

  const inlineError = page.locator('[data-error-marker="refresh-proposal"]');
  await expect(inlineError).toBeVisible();
  await expect(inlineError).toContainText('No Change Proposal');

  await expect(page.locator('[data-toast-kind="error"]')).toHaveCount(0);
  await expectNoReload(page);
});
