import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

/**
 * Issue #46: a bounded parallel Plan Run with partial success surfaces
 * the parallel batch, the merge set, blocked exclusions, and cleanup-safe
 * outputs in the Dashboard. The Playwright web server boots with
 * empty-plan stubs, so the test drives the parallel tranche through the
 * SSE test endpoint and proves the Dashboard renders both the merged
 * assignment and the blocked assignment under the same active Plan Run.
 */
test('parallel partial-success Plan Run shows merged + blocked assignments together', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/parallel-partial-success', randomUUID());
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

  // Merged assignment for source 42.
  await request.post(`/api/_test/projects/${project.id}/project-event`, {
    data: {
      type: 'assignment_status_changed',
      id: 'assignment-42',
      project_id: project.id,
      source_id: '42',
      source_title: 'First parallel issue',
      branch: 'agent/issue-42',
      worktree_path: '/tmp/worktrees/agent-issue-42',
      status: 'merged',
      status_detail: null,
      latest_attempt: null,
      plan_run_id: planRunId,
      selection_summary: 'ready 42',
      phase_outputs: [
        {
          phase: 'implementation',
          outcome: 'ready_for_review',
          body_json: { summary: 'shipped 42' },
          recorded_at: 'unix:2',
          assignment_id: 'assignment-42',
        },
        {
          phase: 'review',
          outcome: 'approved',
          body_json: { summary: 'lgtm 42' },
          recorded_at: 'unix:3',
          assignment_id: 'assignment-42',
        },
        {
          phase: 'merge',
          outcome: 'merged',
          body_json: {
            summary: 'integrated 42',
            verification: ['cargo test --workspace'],
            merged_source_ids: ['42'],
          },
          recorded_at: 'unix:4',
          assignment_id: 'assignment-42',
        },
      ],
      review_rejection_count: 0,
      block_reason: null,
    },
  });

  // Blocked assignment for source 43 (review loop exhausted).
  await request.post(`/api/_test/projects/${project.id}/project-event`, {
    data: {
      type: 'assignment_status_changed',
      id: 'assignment-43',
      project_id: project.id,
      source_id: '43',
      source_title: 'Second parallel issue',
      branch: 'agent/issue-43',
      worktree_path: '/tmp/worktrees/agent-issue-43',
      status: 'blocked',
      status_detail: 'Review Loop exhausted',
      latest_attempt: null,
      plan_run_id: planRunId,
      selection_summary: 'ready 43',
      phase_outputs: [
        {
          phase: 'implementation',
          outcome: 'ready_for_review',
          body_json: { summary: 'shipped 43' },
          recorded_at: 'unix:2',
          assignment_id: 'assignment-43',
        },
        {
          phase: 'review',
          outcome: 'rejected',
          body_json: {
            summary: 'needs more',
            findings: ['missing tests'],
          },
          recorded_at: 'unix:3',
          assignment_id: 'assignment-43',
        },
      ],
      review_rejection_count: 1,
      block_reason: {
        kind: 'review_retry_limit_exhausted',
        detail:
          'Review Loop exhausted: 1 rejection(s) reached the Project Review Retry Limit (1).',
      },
    },
  });

  // Both assignments render under the same Plan Run.
  const assignmentRows = page.getByTestId('plan-run-assignment-row');
  await expect(assignmentRows).toHaveCount(2);

  // Merged assignment exposes the merge Phase Output as durable
  // evidence so cleanup-safe outputs remain visible after the Plan Run
  // finishes.
  const mergedRow = assignmentRows.filter({ hasText: '42: First parallel issue' });
  await expect(mergedRow).toContainText('merged');
  await expect(mergedRow.getByTestId('assignment-phase-output-row')).toHaveCount(3);

  // Blocked assignment shows the block reason and the Re-enable button
  // — the exclusion from the merge set is visible inline.
  const blockedRow = assignmentRows.filter({ hasText: '43: Second parallel issue' });
  await expect(blockedRow).toContainText('blocked');
  await expect(blockedRow.getByTestId('assignment-block-reason')).toContainText(
    'Review Loop exhausted',
  );
  await expect(blockedRow.getByTestId('re-enable-assignment-button')).toBeVisible();

  // Plan Run finishes as succeeded (partial-success: merged work merged,
  // blocked work stayed outside the merge).
  await request.post(`/api/_test/projects/${project.id}/project-event`, {
    data: {
      type: 'plan_run_completed',
      id: planRunId,
      project_id: project.id,
      integration_branch: 'main',
      baseline_commit: 'baseline-sha',
      state: 'succeeded',
      started_at: 'unix:1',
      finished_at: 'unix:5',
      phase_outputs: [],
      assignments: [],
    },
  });

  await expect(page.getByTestId('plan-run-history')).toBeVisible();
  const historyRow = page.getByTestId('plan-run-history-row').first();
  await expect(historyRow).toContainText('Succeeded');
});
