import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

/**
 * Issue #61: the typed `Push` Phase Output body is recorded once per
 * `git push` attempt as a Plan-Run-scoped row (`assignment_id = None`)
 * and surfaces in the PlanRunCard header section. A push failure on a
 * staged assignment must NOT duplicate per-assignment merge/failed
 * rows — the failure manifests only on the Plan-Run-scoped Push row.
 *
 * Append-only invariant: two consecutive push attempts (initial failure
 * + later attempt) appear as two distinct Push rows in chronological
 * order on the header.
 */
test('Push Phase Output renders at PlanRunCard header without per-assignment duplication', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/issue-61-push-phase-output', randomUUID());
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

  // Plan Run starts carrying one failed Plan-Run-scoped Push Phase
  // Output. This mirrors the orchestrator's emission on a non-fast-
  // forward push at the Merge Phase push boundary.
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
      phase_outputs: [
        {
          phase: 'push',
          outcome: 'failed',
          body_json: {
            phase: 'push',
            stderr: 'remote rejected: non-fast-forward',
            fast_forward: false,
            attempt: 1,
          },
          recorded_at: 'unix:2',
          assignment_id: null,
        },
      ],
      assignments: [],
    },
  });

  // The merged assignment lives at `merge_staged` and carries ONLY its
  // own `merge`/`merged` row — the push failure must NOT be duplicated
  // here as a per-assignment row.
  await request.post(`/api/_test/projects/${project.id}/project-event`, {
    data: {
      type: 'assignment_status_changed',
      id: 'assignment-61',
      project_id: project.id,
      source_id: '61',
      source_title: 'Push body renders',
      branch: 'agent/issue-61',
      worktree_path: '/tmp/worktrees/agent-issue-61',
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
            merged_source_ids: ['61'],
            verification: ['cargo test --workspace'],
            gaps: [],
            summary: 'integrated cleanly',
          },
          recorded_at: 'unix:3',
          assignment_id: 'assignment-61',
        },
      ],
      review_rejection_count: 0,
      block_reason: null,
    },
  });

  // Header section renders the Plan-Run-scoped Push row.
  const header = page.getByTestId('plan-run-card-header-phase-outputs');
  await expect(header).toBeVisible();

  let headerRows = header.getByTestId('plan-run-phase-output-row');
  await expect(headerRows).toHaveCount(1);
  await expect(headerRows.nth(0)).toContainText('push');
  await expect(headerRows.nth(0)).toContainText('failed');

  // No duplicated push row on the merged assignment card.
  const assignmentRows = page.getByTestId('assignment-phase-output-row');
  await expect(assignmentRows).toHaveCount(1);
  await expect(assignmentRows.nth(0)).toContainText('merge');
  await expect(assignmentRows.nth(0)).toContainText('merged');

  // Expand the Push row and verify the typed body fields are visible.
  await headerRows.nth(0).getByTestId('phase-output-toggle').click();
  await expect(headerRows.nth(0)).toHaveAttribute('data-expanded', 'true');
  const pushBody = headerRows.nth(0).getByTestId('phase-output-push');
  await expect(pushBody).toBeVisible();
  await expect(headerRows.nth(0).getByTestId('phase-output-push-stderr')).toContainText(
    'non-fast-forward',
  );
  await expect(headerRows.nth(0).getByTestId('phase-output-push-attempt')).toContainText(
    'attempt 1',
  );

  // Append-only invariant: a second push attempt arrives as a brand new
  // Plan-Run-scoped row, chronologically after the first.
  await request.post(`/api/_test/projects/${project.id}/project-event`, {
    data: {
      type: 'plan_run_phase_completed',
      plan_run_id: planRunId,
      phase_output: {
        phase: 'push',
        outcome: 'failed',
        body_json: {
          phase: 'push',
          stderr: 'remote rejected: still non-fast-forward',
          fast_forward: false,
          attempt: 2,
        },
        recorded_at: 'unix:4',
        assignment_id: null,
      },
    },
  });

  headerRows = header.getByTestId('plan-run-phase-output-row');
  await expect(headerRows).toHaveCount(2);
  // Order is chronological (recorded_at).
  await expect(headerRows.nth(0).getByTestId('phase-output-push-attempt')).toContainText(
    'attempt 1',
  );
  // The second row collapsed by default — expand to verify its body.
  await headerRows.nth(1).getByTestId('phase-output-toggle').click();
  await expect(headerRows.nth(1).getByTestId('phase-output-push-attempt')).toContainText(
    'attempt 2',
  );

  // Assignment card still shows ONLY its merge row — no per-assignment
  // duplicate on either push attempt.
  await expect(page.getByTestId('assignment-phase-output-row')).toHaveCount(1);
});
