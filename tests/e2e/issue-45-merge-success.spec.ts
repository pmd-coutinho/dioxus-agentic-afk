import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

/**
 * Issue #45: a successful Merge Phase surfaces the merged Issue
 * Assignment, the merge Phase Output (with verification evidence), and
 * the completed Plan Run in recent history. The Playwright web server
 * boots with empty-plan stubs, so this test drives the merged state
 * through the existing SSE test endpoint and proves the Dashboard
 * renders the post-merge state end-to-end.
 */
test('successful merge surfaces merged assignment, merge phase output, and succeeded plan run', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/merge-success', randomUUID());
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

  // Drive the assignment through the merge phase. The Dashboard reads
  // assignment status + phase outputs from AssignmentStatusChanged
  // events.
  await request.post(`/api/_test/projects/${project.id}/project-event`, {
    data: {
      type: 'assignment_status_changed',
      id: 'assignment-1',
      project_id: project.id,
      source_id: '42',
      source_title: 'Merge one reviewed assignment',
      branch: 'agent/issue-42',
      worktree_path: '/tmp/worktrees/agent-issue-42',
      status: 'merged',
      status_detail: null,
      latest_attempt: null,
      plan_run_id: planRunId,
      selection_summary: 'baseline ready',
      phase_outputs: [
        {
          phase: 'implementation',
          outcome: 'ready_for_review',
          body_json: { summary: 'shipped impl' },
          recorded_at: 'unix:2',
          assignment_id: 'assignment-1',
        },
        {
          phase: 'review',
          outcome: 'approved',
          body_json: { summary: 'lgtm' },
          recorded_at: 'unix:3',
          assignment_id: 'assignment-1',
        },
        {
          phase: 'merge',
          outcome: 'merged',
          body_json: {
            summary: 'integrated cleanly',
            verification: ['cargo test --workspace'],
            merged_source_ids: ['42'],
          },
          recorded_at: 'unix:4',
          assignment_id: 'assignment-1',
        },
      ],
      review_rejection_count: 0,
      block_reason: null,
    },
  });

  const assignmentRow = page.getByTestId('plan-run-assignment-row');
  await expect(assignmentRow).toContainText('merged');
  await expect(assignmentRow).toContainText('42: Merge one reviewed assignment');

  // Merge Phase Output is visible as durable evidence under the merged
  // assignment alongside implementation and review outputs.
  const phaseRows = page.getByTestId('assignment-phase-output-row');
  await expect(phaseRows).toHaveCount(3);
  await expect(phaseRows.nth(2)).toContainText('merge');
  await expect(phaseRows.nth(2)).toContainText('merged');
  await expect(phaseRows.nth(2)).toContainText('integrated cleanly');

  // No block reason, no re-enable button on the merged happy path.
  await expect(page.getByTestId('assignment-block-reason')).toHaveCount(0);
  await expect(page.getByTestId('re-enable-assignment-button')).toHaveCount(0);

  // Plan Run completes as succeeded and lands in recent history.
  const succeededPlanRun = {
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
  };
  await request.post(`/api/_test/projects/${project.id}/project-event`, {
    data: succeededPlanRun,
  });

  await expect(page.getByTestId('plan-run-history')).toBeVisible();
  const historyRow = page.getByTestId('plan-run-history-row').first();
  await expect(historyRow).toContainText('Succeeded');
});
