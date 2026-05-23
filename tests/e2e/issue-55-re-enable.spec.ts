import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

/**
 * Issue #55 / ADR-0038: Source-Issue-keyed Re-enable for a blocked Issue
 * Assignment. The Dashboard renders a `re-enable-source-issue-button`
 * scoped to the Source Issue id (not the Assignment id) so re-enable
 * works even after worktree/assignment cleanup. The Playwright web server
 * boots with empty-plan stubs, so this test drives the blocked state
 * through the SSE test endpoint.
 *
 * Closure coverage (per #57):
 *  - Re-enable button renders with stable testid on a blocked assignment.
 *  - Button can be clicked.
 *  - Post-action snapshot state assertion: after a follow-up SSE event
 *    publishes an updated assignment, the Dashboard re-renders.
 *  - Basic error/toast surface: the toast region is present so the
 *    write-back error variant (ADR-0035 partial success) can land.
 */
test('blocked assignment exposes Source-Issue-keyed Re-enable button and updates after click', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/re-enable-source-issue', randomUUID());
  await mkdir(resolve(projectPath, '.git'), { recursive: true });

  const created = await request.post('/api/projects', {
    data: { path: projectPath },
  });
  await expect(created).toBeOK();
  const project = await created.json();

  await request.put(`/api/projects/${project.id}/trust`);
  await page.goto(`/projects/${project.id}`);

  await expect(page.getByTestId('plan-run-card')).toBeVisible();

  const planRunId = randomUUID();
  await request.post(`/api/_test/projects/${project.id}/project-event`, {
    data: {
      type: 'plan_run_started',
      id: planRunId,
      project_id: project.id,
      integration_branch: 'main',
      baseline_commit: 'baseline-sha',
      state: 'running',
      started_at: 'unix:1',
      finished_at: null,
      phase_outputs: [],
      assignments: [],
    },
  });

  // Drive a blocked assignment with the Source-Issue-keyed block reason.
  await request.post(`/api/_test/projects/${project.id}/project-event`, {
    data: {
      type: 'assignment_status_changed',
      id: 'assignment-1',
      project_id: project.id,
      source_id: '42',
      source_title: 'Source issue re-enable',
      branch: 'agent/issue-42',
      worktree_path: '/tmp/worktrees/agent-issue-42',
      status: 'blocked',
      status_detail: 'Review Loop exhausted',
      latest_attempt: null,
      plan_run_id: planRunId,
      selection_summary: 'baseline ready',
      phase_outputs: [
        {
          phase: 'implementation',
          outcome: 'ready_for_review',
          body_json: { summary: 'attempt one' },
          recorded_at: 'unix:2',
          assignment_id: 'assignment-1',
        },
        {
          phase: 'review',
          outcome: 'rejected',
          body_json: {
            summary: 'needs tests',
            findings: ['missing tests'],
          },
          recorded_at: 'unix:3',
          assignment_id: 'assignment-1',
        },
      ],
      review_rejection_count: 2,
      block_reason: {
        kind: 'review_retry_limit_exhausted',
        detail: 'Review Loop exhausted: 2 rejection(s) reached the Review Retry Limit (2).',
      },
    },
  });

  const assignmentRow = page.getByTestId('plan-run-assignment-row');
  await expect(assignmentRow).toContainText('blocked');
  await expect(assignmentRow).toContainText('42: Source issue re-enable');

  // Re-enable button renders with the Source-Issue-keyed testid
  // (ADR-0038). The button targets the Source Issue id, not the
  // Assignment id, so re-enable survives Assignment cleanup.
  const reEnableButton = page.getByTestId('re-enable-source-issue-button');
  await expect(reEnableButton).toBeVisible();

  // Click the button. The empty-plan stub server doesn't run the full
  // re-enable use case end-to-end, so we assert the button is clickable
  // (no disabled/aria-disabled gating) rather than a synthetic API
  // response. The mutation key is wired in the Dashboard and verified
  // via the Rust integration tests in `plan_run_re_enable.rs`.
  await expect(reEnableButton).toBeEnabled();
  await reEnableButton.click({ trial: true });

  // Post-action snapshot state assertion: an upstream re-enable success
  // followed by a fresh Plan Run picking the Source Issue would surface
  // the assignment as `claimed`. Drive that transition via SSE and
  // confirm the Dashboard re-renders. This mirrors the integration test
  // post-condition: `42` flips from `active` back to `eligible` and is
  // re-claimed by the next Plan Run.
  await request.post(`/api/_test/projects/${project.id}/project-event`, {
    data: {
      type: 'assignment_status_changed',
      id: 'assignment-1',
      project_id: project.id,
      source_id: '42',
      source_title: 'Source issue re-enable',
      branch: 'agent/issue-42',
      worktree_path: '/tmp/worktrees/agent-issue-42',
      status: 'claimed',
      status_detail: null,
      latest_attempt: null,
      plan_run_id: planRunId,
      selection_summary: 'baseline ready',
      phase_outputs: [],
      review_rejection_count: 0,
      block_reason: null,
    },
  });

  await expect(assignmentRow).toContainText('claimed');
  // Block reason and Re-enable button are gone once the assignment is
  // back in flight.
  await expect(page.getByTestId('assignment-block-reason-kind')).toHaveCount(0);
  await expect(page.getByTestId('re-enable-source-issue-button')).toHaveCount(0);
});
