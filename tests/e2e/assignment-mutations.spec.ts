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

const EMPTY_ASSIGNMENT_STATE = {
  active_assignment: null,
  waiting_ready_issue_count: 0,
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
  let planningCalls = 0;
  let postCalls = 0;

  await page.route(
    `**/api/projects/${projectId}/planning-snapshot`,
    async (route) => {
      planningCalls += 1;
      // First load: one eligible issue. After Start Assignment succeeds, the
      // panel refetches; report it as active so we can prove the refetch ran.
      const snapshot =
        planningCalls === 1
          ? { ...EMPTY_PLANNING, eligible: [eligibleIssue()] }
          : { ...EMPTY_PLANNING, active: [eligibleIssue()] };
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(snapshot),
      });
    },
  );

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

  // After settle: panel refetched and now shows the issue under Active.
  await expect(page.getByRole('heading', { name: 'Active Issues' })).toBeVisible();
  // Eligible Ready Issues group should now show "None".
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

  await page.route(
    `**/api/projects/${projectId}/planning-snapshot`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ ...EMPTY_PLANNING, eligible: [eligibleIssue()] }),
      });
    },
  );

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

  const inlineError = page.locator('[data-start-assignment-error="true"]').first();
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
  let stateCalls = 0;
  let postCalls = 0;

  await page.route(
    `**/api/projects/${projectId}/assignment-state`,
    async (route) => {
      stateCalls += 1;
      if (stateCalls === 1) {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            active_assignment: assignmentResponse('assn-1', 'blocked', projectId),
            waiting_ready_issue_count: 0,
          }),
        });
      } else {
        // Post-mutation refetch: assignment cleared.
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify(EMPTY_ASSIGNMENT_STATE),
        });
      }
    },
  );

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

  await expect(page.getByText('No active Issue Assignment')).toBeVisible();
  expect(postCalls).toBe(1);
  expect(stateCalls).toBeGreaterThanOrEqual(2);
  await expectNoReload(page);
});

test('Abandon Assignment: 422 validation error renders inline, no reload', async ({
  page,
  request,
}) => {
  const projectId = await createTrustedProject(request, 'abandon-err');

  await page.route(
    `**/api/projects/${projectId}/assignment-state`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          active_assignment: assignmentResponse('assn-1', 'blocked', projectId),
          waiting_ready_issue_count: 0,
        }),
      });
    },
  );

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

  const inlineError = page.locator('[data-lifecycle-error="abandon-assignment"]');
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
  let stateCalls = 0;
  let postCalls = 0;

  await page.route(
    `**/api/projects/${projectId}/assignment-state`,
    async (route) => {
      stateCalls += 1;
      const status = stateCalls === 1 ? 'blocked' : 'proposal_pending';
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          active_assignment: assignmentResponse('assn-1', status, projectId),
          waiting_ready_issue_count: 0,
        }),
      });
    },
  );

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

  // After settle: status transitions and Recover button disappears.
  await expect(page.getByText('Change Proposal awaiting checks')).toBeVisible();
  await expect(page.getByTestId('recover-assignment-button')).toHaveCount(0);
  expect(postCalls).toBe(1);
  await expectNoReload(page);
});

test('Recover Assignment: 500 transient error keeps button enabled, no reload', async ({
  page,
  request,
}) => {
  const projectId = await createTrustedProject(request, 'recover-err');

  await page.route(
    `**/api/projects/${projectId}/assignment-state`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          active_assignment: assignmentResponse('assn-1', 'blocked', projectId),
          waiting_ready_issue_count: 0,
        }),
      });
    },
  );

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
  let stateCalls = 0;
  let postCalls = 0;

  await page.route(
    `**/api/projects/${projectId}/assignment-state`,
    async (route) => {
      stateCalls += 1;
      const status = stateCalls === 1 ? 'proposal_pending' : 'proposal_verified';
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          active_assignment: assignmentResponse('assn-1', status, projectId),
          waiting_ready_issue_count: 0,
        }),
      });
    },
  );

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

  await expect(page.getByText('Verified — awaiting human merge')).toBeVisible();
  expect(postCalls).toBe(1);
  await expectNoReload(page);
});

test('Refresh Proposal State: 422 validation error renders inline, no reload', async ({
  page,
  request,
}) => {
  const projectId = await createTrustedProject(request, 'refresh-err');

  await page.route(
    `**/api/projects/${projectId}/assignment-state`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          active_assignment: assignmentResponse('assn-1', 'proposal_pending', projectId),
          waiting_ready_issue_count: 0,
        }),
      });
    },
  );

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

  const inlineError = page.locator('[data-lifecycle-error="refresh-proposal"]');
  await expect(inlineError).toBeVisible();
  await expect(inlineError).toContainText('No Change Proposal');

  await expect(page.locator('[data-toast-kind="error"]')).toHaveCount(0);
  await expectNoReload(page);
});
