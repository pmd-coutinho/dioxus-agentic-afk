import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

/**
 * Issue #60: the typed Merge Phase Output body renders the merger's
 * verification log as a preformatted block, the integration summary, and
 * the merged Source Issue ids on the expanded row. The collapsed row
 * keeps pill + summary. Covers the local-integration `merged` outcome
 * only; Integration Branch push diagnostics ship as a separate `Push`
 * variant in slice 4 (#61).
 */
test('Merge Phase Output expanded row renders verification log and summary', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/issue-60-merge-rendered', randomUUID());
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

  // Merge-staged Issue Assignment carrying a typed Merge phase output
  // (merged) so the renderer is validated end-to-end through SSE.
  await request.post(`/api/_test/projects/${project.id}/project-event`, {
    data: {
      type: 'assignment_status_changed',
      id: 'assignment-60',
      project_id: project.id,
      source_id: '60',
      source_title: 'Merge body renders',
      branch: 'agent/issue-60',
      worktree_path: '/tmp/worktrees/agent-issue-60',
      status: 'merge_staged',
      status_detail: 'merge staged awaiting push',
      latest_attempt: null,
      plan_run_id: planRunId,
      selection_summary: 'baseline ready',
      phase_outputs: [
        {
          phase: 'merge',
          outcome: 'merged',
          body_json: {
            phase: 'merge',
            merged_source_ids: ['60'],
            verification: ['cargo test --workspace', 'cargo clippy'],
            gaps: [],
            summary: 'integrated cleanly',
          },
          recorded_at: 'unix:2',
          assignment_id: 'assignment-60',
        },
      ],
      review_rejection_count: 0,
      block_reason: null,
    },
  });

  const rows = page.getByTestId('assignment-phase-output-row');
  await expect(rows).toHaveCount(1);

  const mergeRow = rows.nth(0);
  // Collapsed row keeps pill + summary line.
  await expect(mergeRow).toContainText('merge');
  await expect(mergeRow).toContainText('merged');
  await expect(mergeRow).toContainText('integrated cleanly');
  await expect(mergeRow).toHaveAttribute('data-expanded', 'false');

  // Expand the Merge row.
  await mergeRow.getByTestId('phase-output-toggle').click();
  await expect(mergeRow).toHaveAttribute('data-expanded', 'true');

  // Typed Merge renderer shape.
  await expect(mergeRow.getByTestId('phase-output-merge')).toBeVisible();
  await expect(mergeRow.getByTestId('phase-output-merge-summary')).toContainText(
    'integrated cleanly',
  );
  await expect(mergeRow.getByTestId('phase-output-merge-commits')).toContainText('60');
  await expect(mergeRow.getByTestId('phase-output-merge-verification')).toContainText(
    'cargo test --workspace',
  );
  await expect(mergeRow.getByTestId('phase-output-merge-verification')).toContainText(
    'cargo clippy',
  );
});
