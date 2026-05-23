import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

/**
 * Issue #58: each Phase Output row renders collapsed by default and toggles
 * to an inline expanded body on click. The Failed variant carries an error
 * string and (optionally) the RFC-7807 problem-type URN of the originating
 * CoordinatorError, both of which must be visible in the expanded view.
 * Multiple rows may be open at once.
 */
test('Failed Phase Output row expands inline to show the error string', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/phase-output-expand', randomUUID());
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

  // Publish a blocked Issue Assignment carrying two Failed Phase Output
  // rows: one for an unparseable implementation, one for a failed review.
  // Both use the typed `Failed` body shape (`phase=failed` tag plus
  // `error` + optional `problem_type`).
  await request.post(`/api/_test/projects/${project.id}/project-event`, {
    data: {
      type: 'assignment_status_changed',
      id: 'assignment-58',
      project_id: project.id,
      source_id: '58',
      source_title: 'Failure expand UX',
      branch: 'agent/issue-58',
      worktree_path: '/tmp/worktrees/agent-issue-58',
      status: 'blocked',
      status_detail: 'Implementation failed',
      latest_attempt: null,
      plan_run_id: planRunId,
      selection_summary: 'baseline ready',
      phase_outputs: [
        {
          phase: 'implementation',
          outcome: 'failed',
          body_json: {
            phase: 'failed',
            error: 'codex exited with status 137',
            problem_type: 'urn:agentic-afk:implementation-phase-failed',
          },
          recorded_at: 'unix:2',
          assignment_id: 'assignment-58',
        },
        {
          phase: 'review',
          outcome: 'failed',
          body_json: {
            phase: 'failed',
            error: 'reviewer JSON could not be parsed at line 3',
          },
          recorded_at: 'unix:3',
          assignment_id: 'assignment-58',
        },
      ],
      review_rejection_count: 0,
      block_reason: {
        kind: 'merge_phase_failed',
        detail: 'implementation phase failed',
      },
    },
  });

  // Collapsed by default: pill is visible, body view is not yet rendered.
  const rows = page.getByTestId('assignment-phase-output-row');
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0)).toContainText('implementation');
  await expect(rows.nth(0)).toContainText('failed');
  await expect(rows.nth(0)).toHaveAttribute('data-expanded', 'false');
  await expect(page.getByTestId('phase-output-body')).toHaveCount(0);

  // Click expands the first row and reveals the error + problem-type.
  await rows.nth(0).getByTestId('phase-output-toggle').click();
  await expect(rows.nth(0)).toHaveAttribute('data-expanded', 'true');
  const firstBody = rows.nth(0).getByTestId('phase-output-body');
  await expect(firstBody).toBeVisible();
  await expect(firstBody.getByTestId('phase-output-error')).toContainText(
    'codex exited with status 137',
  );
  await expect(firstBody.getByTestId('phase-output-problem-type')).toContainText(
    'urn:agentic-afk:implementation-phase-failed',
  );

  // Multiple rows may be open at once.
  await rows.nth(1).getByTestId('phase-output-toggle').click();
  await expect(rows.nth(1)).toHaveAttribute('data-expanded', 'true');
  await expect(rows.nth(0)).toHaveAttribute('data-expanded', 'true');
  await expect(rows.nth(1).getByTestId('phase-output-error')).toContainText(
    'reviewer JSON could not be parsed',
  );

  // Toggling collapses the row back without affecting siblings.
  await rows.nth(0).getByTestId('phase-output-toggle').click();
  await expect(rows.nth(0)).toHaveAttribute('data-expanded', 'false');
  await expect(rows.nth(1)).toHaveAttribute('data-expanded', 'true');
});
