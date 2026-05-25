import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

/**
 * Issue #85: end-to-end Dashboard coverage for Plan Run Stage and
 * Outcome labels. Proves the Dashboard displays derived stage for
 * active runs and outcome-specific labels for finished runs.
 *
 * The web server boots with empty-plan stubs, so we drive state
 * through the SSE test endpoint and assert on rendered labels.
 */

test('mixed active assignments show least-advanced Plan Run Stage', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/stage-outcome', randomUUID());
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
  // Start a Plan Run with two assignments: one implementing,
  // one reviewed. The least advanced is "implementing", so the
  // stage pill should show "Implementing".
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
      assignments: [
        {
          id: 'assn-impl',
          project_id: project.id,
          source_id: '1',
          source_title: 'Implement feature A',
          branch: 'agent/issue-1',
          worktree_path: '/tmp/worktrees/a',
          status: 'implementing',
          status_detail: null,
          latest_attempt: null,
          plan_run_id: planRunId,
          selection_summary: 'baseline ready',
          phase_outputs: [],
          review_rejection_count: 0,
          block_reason: null,
        },
        {
          id: 'assn-review',
          project_id: project.id,
          source_id: '2',
          source_title: 'Review feature B',
          branch: 'agent/issue-2',
          worktree_path: '/tmp/worktrees/b',
          status: 'reviewed',
          status_detail: null,
          latest_attempt: null,
          plan_run_id: planRunId,
          selection_summary: 'baseline ready',
          phase_outputs: [],
          review_rejection_count: 0,
          block_reason: null,
        },
      ],
    },
  });

  const activeCard = page.getByTestId('plan-run-active');
  await expect(activeCard).toBeVisible();
  // Stage label should be "Implementing" (least-advanced wins).
  // The stage is derived by the backend from the assignments in the
  // plan_run_started event data; the backend's classify_stage picks
  // "implementing" when any assignment has that status.
  await expect(activeCard).toContainText('Implementing');
});

test('finished empty backlog displays Empty backlog in history', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/stage-outcome', randomUUID());
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

  await request.post(`/api/_test/projects/${project.id}/project-event`, {
    data: {
      type: 'plan_run_completed',
      id: planRunId,
      project_id: project.id,
      integration_branch: 'main',
      baseline_commit: 'baseline-sha',
      state: 'finished',
      started_at: 'unix:1',
      finished_at: 'unix:2',
      phase_outputs: [
        {
          phase: 'planning',
          outcome: 'succeeded_empty',
          body_json: { issues: [], summary: 'no eligible work' },
          recorded_at: 'unix:1',
          assignment_id: null,
        },
      ],
      assignments: [],
      outcome: 'empty_backlog',
    },
  });

  await expect(page.getByTestId('plan-run-history')).toBeVisible();
  const historyRow = page.getByTestId('plan-run-history-row').first();
  await expect(historyRow).toContainText('Empty backlog');
});

test('finished blocked assignment displays Assignment blocked in history', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/stage-outcome', randomUUID());
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
      assignments: [
        {
          id: 'assn-blocked',
          project_id: project.id,
          source_id: '3',
          source_title: 'Blocked feature',
          branch: 'agent/issue-3',
          worktree_path: '/tmp/worktrees/c',
          status: 'blocked',
          status_detail: null,
          latest_attempt: null,
          plan_run_id: planRunId,
          selection_summary: 'baseline ready',
          phase_outputs: [],
          review_rejection_count: 3,
          block_reason: {
            kind: 'review_retry_limit_exhausted',
            detail: '3 rejections exhausted retry limit',
          },
        },
      ],
    },
  });

  await request.post(`/api/_test/projects/${project.id}/project-event`, {
    data: {
      type: 'plan_run_completed',
      id: planRunId,
      project_id: project.id,
      integration_branch: 'main',
      baseline_commit: 'baseline-sha',
      state: 'finished',
      started_at: 'unix:1',
      finished_at: 'unix:3',
      phase_outputs: [],
      assignments: [],
      outcome: 'assignment_blocked',
    },
  });

  await expect(page.getByTestId('plan-run-history')).toBeVisible();
  const historyRow = page.getByTestId('plan-run-history-row').first();
  await expect(historyRow).toContainText('Assignment blocked');
});

test('merge staged assignment shows Pushing stage in active run', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/stage-outcome', randomUUID());
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
      assignments: [
        {
          id: 'assn-staged',
          project_id: project.id,
          source_id: '4',
          source_title: 'Merge staged feature',
          branch: 'agent/issue-4',
          worktree_path: '/tmp/worktrees/d',
          status: 'merge_staged',
          status_detail: null,
          latest_attempt: null,
          plan_run_id: planRunId,
          selection_summary: 'baseline ready',
          phase_outputs: [],
          review_rejection_count: 0,
          block_reason: null,
        },
      ],
    },
  });

  const activeCard = page.getByTestId('plan-run-active');
  await expect(activeCard).toBeVisible();
  // A merge_staged assignment triggers the Pushing stage because
  // the backend classifies merge_staged as part of the push
  // boundary (ADR-0038).
  await expect(activeCard).toContainText('Pushing');
});
