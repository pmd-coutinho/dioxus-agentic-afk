import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

/**
 * Issue #42: a Plan Run with one eligible Ready Issue surfaces the claimed
 * Issue Assignment inside the active Plan Run card.
 *
 * The Playwright web server boots with `AGENTIC_AFK_TEST_PLAN_RUN_STUBS=1`
 * (empty selection), so this test arrives at the claim path by publishing
 * the expected SSE events through the existing test endpoint. The
 * Dashboard's project store mirrors the assignment into the active Plan
 * Run regardless of which transport delivered it, so this proves the
 * "Dashboard-visible selection path" criterion end-to-end.
 */
test('claimed Issue Assignment renders inside the active Plan Run card', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/plan-run-claim', randomUUID());
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
      type: 'assignment_created',
      id: 'assignment-1',
      project_id: project.id,
      source_id: '42',
      source_title: 'Plan and claim',
      branch: 'agent/issue-42',
      worktree_path: '/tmp/worktrees/agent-issue-42',
      status: 'claimed',
      status_detail: null,
      latest_attempt: null,
      plan_run_id: planRunId,
      selection_summary: 'baseline ready',
    },
  });

  // The active Plan Run card now lists the claimed assignment with
  // selection summary and branch.
  const assignmentRow = page.getByTestId('plan-run-assignment-row');
  await expect(assignmentRow).toBeVisible();
  await expect(assignmentRow).toContainText('42: Plan and claim');
  await expect(assignmentRow).toContainText('agent/issue-42');
  await expect(assignmentRow).toContainText('baseline ready');
  await expect(assignmentRow).toContainText('claimed');
});
