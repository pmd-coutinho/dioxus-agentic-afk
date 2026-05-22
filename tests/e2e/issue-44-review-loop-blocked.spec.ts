import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

/**
 * Issue #44: a blocked Issue Assignment (Review Loop exhausted) surfaces
 * the block reason, the review rejection count, and a Re-enable button
 * inside the active Plan Run card. The Playwright web server boots with
 * empty-plan stubs, so this test drives the blocked state through the
 * existing SSE test endpoint and proves the Dashboard renders the new
 * review-retry state end-to-end.
 */
test('blocked assignment exposes rejection count, block reason, and Re-enable button', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/review-loop-blocked', randomUUID());
  await mkdir(resolve(projectPath, '.git'), { recursive: true });

  const created = await request.post('/api/projects', {
    data: { path: projectPath },
  });
  await expect(created).toBeOK();
  const project = await created.json();

  await request.put(`/api/projects/${project.id}/trust`);
  await page.goto(`/projects/${project.id}`);

  // Wait for the Plan Run card to mount so the SSE subscription is live
  // before we publish events through the test endpoint.
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

  await request.post(`/api/_test/projects/${project.id}/project-event`, {
    data: {
      type: 'assignment_status_changed',
      id: 'assignment-1',
      project_id: project.id,
      source_id: '42',
      source_title: 'Loop rejected review findings',
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
            findings: ['missing tests', 'unhandled error'],
          },
          recorded_at: 'unix:3',
          assignment_id: 'assignment-1',
        },
      ],
      review_rejection_count: 2,
      block_reason: 'Review Loop exhausted: 2 rejection(s) reached the Project Review Retry Limit (2).',
    },
  });

  const assignmentRow = page.getByTestId('plan-run-assignment-row');
  await expect(assignmentRow).toContainText('blocked');
  await expect(assignmentRow).toContainText('42: Loop rejected review findings');

  // Review rejection count + block reason rendered for the developer.
  await expect(page.getByTestId('assignment-review-rejection-count')).toContainText('2');
  await expect(page.getByTestId('assignment-block-reason')).toContainText(
    'Review Loop exhausted',
  );

  // Rejected review Phase Output is preserved as durable evidence under
  // the blocked assignment, so review-loop findings stay visible after
  // worktree cleanup.
  const phaseRows = page.getByTestId('assignment-phase-output-row');
  await expect(phaseRows).toHaveCount(2);
  await expect(phaseRows.nth(1)).toContainText('review');
  await expect(phaseRows.nth(1)).toContainText('rejected');

  // Re-enable button is wired to the new endpoint; pressing it transitions
  // the assignment out of the blocked state via an SSE follow-up.
  await expect(page.getByTestId('re-enable-assignment-button')).toBeVisible();
});
