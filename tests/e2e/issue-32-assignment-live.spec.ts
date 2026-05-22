/**
 * Issue #32: Assignment, Planning, and Issue Source panels live via SSE.
 *
 * Drives the full Start Assignment → Assignment Attempt → Change Proposal
 * pending → Change Proposal verified flow by publishing `ProjectEvent`
 * deltas via the test-only `POST /api/_test/projects/{id}/project-event`
 * endpoint (gated by `AGENTIC_AFK_TEST_ENDPOINTS=1`). The Dashboard's
 * Assignment panel must reflect each transition without any user click or
 * page reload.
 */
import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

test('Assignment panel updates live across Start → Attempt → Proposal verified', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/issue-32-assignment', randomUUID());
  await mkdir(resolve(projectPath, '.git'), { recursive: true });

  const created = await request.post('/api/projects', {
    data: { path: projectPath },
  });
  await expect(created).toBeOK();
  const project = await created.json();

  await page.goto(`/projects/${project.id}/assignment`);

  // Marker proves no full reload happens.
  await page.evaluate(() => {
    (window as unknown as { __issue32Mark?: string }).__issue32Mark = 'pre-mutation';
  });

  await expect(page.getByRole('heading', { name: 'Issue Assignment' })).toBeVisible();
  await expect(page.getByText('No active Assignment')).toBeVisible();

  // --- Start Assignment ----------------------------------------------------
  const created1 = await request.post(
    `/api/_test/projects/${project.id}/project-event`,
    {
      data: {
        type: 'assignment_created',
        id: 'assn-1',
        project_id: project.id,
        source_id: 'issue-A',
        source_title: 'Live Test Source Title',
        branch: 'agentic-afk/issue-A',
        worktree_path: '/tmp/agentic-afk/issue-A',
        status: 'running',
        status_detail: null,
        change_proposal: null,
        latest_attempt: null,
      },
    },
  );
  await expect(created1).toBeOK();

  await expect(page.getByText('Live Test Source Title')).toBeVisible();
  // Unknown lifecycle status renders the raw value inside a StatusPill.
  await expect(page.getByText('running', { exact: true })).toBeVisible();
  await expect(page.getByText('No active Assignment')).toBeHidden();

  // --- Assignment Attempt added -------------------------------------------
  const attemptAdded = await request.post(
    `/api/_test/projects/${project.id}/project-event`,
    {
      data: {
        type: 'assignment_attempt_added',
        assignment_id: 'assn-1',
        attempt: {
          id: 'attempt-1',
          kind: 'initial',
          process_id: null,
          process_identity: null,
          terminal_outcome: {
            outcome: 'ReadyForProposal',
            summary: 'Codex finished',
          },
        },
      },
    },
  );
  await expect(attemptAdded).toBeOK();

  // --- Change Proposal pending --------------------------------------------
  const proposalRefreshed = await request.post(
    `/api/_test/projects/${project.id}/project-event`,
    {
      data: {
        type: 'change_proposal_refreshed',
        assignment_id: 'assn-1',
        change_proposal: {
          status: 'pending',
          url: 'https://github.com/example/repo/pull/123',
        },
      },
    },
  );
  await expect(proposalRefreshed).toBeOK();
  await expect(page.getByText('Change Proposal pending')).toBeVisible();

  // Transition assignment.status to proposal_pending to match.
  const statusPending = await request.post(
    `/api/_test/projects/${project.id}/project-event`,
    {
      data: {
        type: 'assignment_status_changed',
        id: 'assn-1',
        project_id: project.id,
        source_id: 'issue-A',
        source_title: 'Live Test Source Title',
        branch: 'agentic-afk/issue-A',
        worktree_path: '/tmp/agentic-afk/issue-A',
        status: 'proposal_pending',
        status_detail: null,
        change_proposal: {
          status: 'pending',
          url: 'https://github.com/example/repo/pull/123',
        },
        latest_attempt: null,
      },
    },
  );
  await expect(statusPending).toBeOK();
  await expect(page.getByText('Awaiting checks')).toBeVisible();

  // --- Change Proposal verified -------------------------------------------
  const proposalVerified = await request.post(
    `/api/_test/projects/${project.id}/project-event`,
    {
      data: {
        type: 'change_proposal_verified',
        assignment_id: 'assn-1',
        change_proposal: {
          status: 'verified',
          url: 'https://github.com/example/repo/pull/123',
        },
      },
    },
  );
  await expect(proposalVerified).toBeOK();

  const statusVerified = await request.post(
    `/api/_test/projects/${project.id}/project-event`,
    {
      data: {
        type: 'assignment_status_changed',
        id: 'assn-1',
        project_id: project.id,
        source_id: 'issue-A',
        source_title: 'Live Test Source Title',
        branch: 'agentic-afk/issue-A',
        worktree_path: '/tmp/agentic-afk/issue-A',
        status: 'proposal_verified',
        status_detail: null,
        change_proposal: {
          status: 'verified',
          url: 'https://github.com/example/repo/pull/123',
        },
        latest_attempt: null,
      },
    },
  );
  await expect(statusVerified).toBeOK();
  await expect(page.getByText('Verified', { exact: true })).toBeVisible();
  await expect(page.getByText('Change Proposal verified')).toBeVisible();

  // No reload happened.
  const mark = await page.evaluate(
    () => (window as unknown as { __issue32Mark?: string }).__issue32Mark,
  );
  expect(mark).toBe('pre-mutation');
});
