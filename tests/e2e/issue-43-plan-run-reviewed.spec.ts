import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

/**
 * Issue #43: a reviewed Issue Assignment surfaces its implementation and
 * approving review Phase Outputs inside the active Plan Run.
 *
 * The Playwright web server boots with empty-plan stubs, so this test
 * publishes the expected SSE events through the test endpoint and proves
 * the Dashboard's project store + Plan Run card honour the new phase
 * output evidence end-to-end.
 */
test('reviewed assignment shows implementation and review phase outputs', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/plan-run-reviewed', randomUUID());
  await mkdir(resolve(projectPath, '.git'), { recursive: true });

  const created = await request.post('/api/projects', {
    data: { path: projectPath },
  });
  await expect(created).toBeOK();
  const project = await created.json();

  await request.put(`/api/projects/${project.id}/trust`);
  await page.goto(`/projects/${project.id}`);

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
      source_title: 'Plan and claim',
      branch: 'agent/issue-42',
      worktree_path: '/tmp/worktrees/agent-issue-42',
      status: 'reviewed',
      status_detail: null,
      latest_attempt: null,
      plan_run_id: planRunId,
      selection_summary: 'baseline ready',
      phase_outputs: [
        {
          phase: 'implementation',
          outcome: 'ready_for_review',
          body_json: { summary: 'feature complete' },
          recorded_at: 'unix:2',
          assignment_id: 'assignment-1',
        },
        {
          phase: 'review',
          outcome: 'approved',
          body_json: { summary: 'looks good' },
          recorded_at: 'unix:3',
          assignment_id: 'assignment-1',
        },
      ],
    },
  });

  const assignmentRow = page.getByTestId('plan-run-assignment-row');
  await expect(assignmentRow).toContainText('reviewed');
  await expect(assignmentRow).toContainText('42: Plan and claim');

  const phaseRows = page.getByTestId('assignment-phase-output-row');
  await expect(phaseRows).toHaveCount(2);
  await expect(phaseRows.nth(0)).toContainText('implementation');
  await expect(phaseRows.nth(0)).toContainText('feature complete');
  await expect(phaseRows.nth(1)).toContainText('review');
  await expect(phaseRows.nth(1)).toContainText('looks good');
});
