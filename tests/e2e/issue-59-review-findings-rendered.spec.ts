import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

/**
 * Issue #59: the typed Review Phase Output body renders review findings as
 * an ordered list with per-finding `location` + `message`, plus a
 * preformatted `verification` block, inside the expanded Phase Output row.
 * This exercises the Review Loop block path (assignment ends `blocked` with
 * a rejected review preserved as durable evidence) so the renderer is
 * validated end-to-end through SSE.
 */
test('Review Phase Output expanded row renders findings with location and message', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/issue-59-review-findings', randomUUID());
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

  // Blocked Issue Assignment carrying a typed Implementation phase output
  // (ready_for_review) and a typed Review phase output (rejected) — the
  // Review body uses the `{location, message}` finding shape introduced
  // in issue #59.
  await request.post(`/api/_test/projects/${project.id}/project-event`, {
    data: {
      type: 'assignment_status_changed',
      id: 'assignment-59',
      project_id: project.id,
      source_id: '59',
      source_title: 'Review findings render',
      branch: 'agent/issue-59',
      worktree_path: '/tmp/worktrees/agent-issue-59',
      status: 'blocked',
      status_detail: 'Review Loop exhausted',
      latest_attempt: null,
      plan_run_id: planRunId,
      selection_summary: 'baseline ready',
      phase_outputs: [
        {
          phase: 'implementation',
          outcome: 'ready_for_review',
          body_json: {
            phase: 'implementation',
            commits: ['abc123', 'def456'],
            verification: ['cargo test --workspace'],
            gaps: [],
            summary: 'shipped impl',
          },
          recorded_at: 'unix:2',
          assignment_id: 'assignment-59',
        },
        {
          phase: 'review',
          outcome: 'rejected',
          body_json: {
            phase: 'review',
            findings: [
              { location: 'src/lib.rs:42', message: 'missing null check' },
              { location: 'tests/e2e/foo.spec.ts:7', message: 'flaky selector' },
            ],
            verification: ['cargo test', 'cargo clippy'],
            gaps: ['no e2e covers retry'],
            summary: 'needs more',
          },
          recorded_at: 'unix:3',
          assignment_id: 'assignment-59',
        },
      ],
      review_rejection_count: 1,
      block_reason: {
        kind: 'review_retry_limit_exhausted',
        detail: 'Review Loop exhausted',
      },
    },
  });

  const rows = page.getByTestId('assignment-phase-output-row');
  await expect(rows).toHaveCount(2);

  // Expand the Review row (index 1).
  const reviewRow = rows.nth(1);
  await expect(reviewRow).toContainText('review');
  await expect(reviewRow).toContainText('rejected');
  await reviewRow.getByTestId('phase-output-toggle').click();
  await expect(reviewRow).toHaveAttribute('data-expanded', 'true');

  // Findings rendered as ordered list with location + message per finding.
  const findings = reviewRow.getByTestId('phase-output-review-finding');
  await expect(findings).toHaveCount(2);
  await expect(findings.nth(0).getByTestId('phase-output-review-finding-location')).toContainText(
    'src/lib.rs:42',
  );
  await expect(findings.nth(0).getByTestId('phase-output-review-finding-message')).toContainText(
    'missing null check',
  );
  await expect(findings.nth(1).getByTestId('phase-output-review-finding-location')).toContainText(
    'tests/e2e/foo.spec.ts:7',
  );
  await expect(findings.nth(1).getByTestId('phase-output-review-finding-message')).toContainText(
    'flaky selector',
  );

  // Verification log rendered as preformatted block.
  await expect(reviewRow.getByTestId('phase-output-verification')).toContainText('cargo test');
  await expect(reviewRow.getByTestId('phase-output-verification')).toContainText('cargo clippy');

  // Collapsed sibling Implementation row keeps its pill + summary.
  const implRow = rows.nth(0);
  await expect(implRow).toContainText('implementation');
  await expect(implRow).toContainText('ready_for_review');
  await expect(implRow).toContainText('shipped impl');
  await expect(implRow).toHaveAttribute('data-expanded', 'false');

  // Expand Implementation row to confirm typed renderer fields.
  await implRow.getByTestId('phase-output-toggle').click();
  await expect(implRow.getByTestId('phase-output-implementation')).toBeVisible();
  await expect(implRow.getByTestId('phase-output-commits')).toContainText('abc123');
  await expect(implRow.getByTestId('phase-output-commits')).toContainText('def456');
  await expect(implRow.getByTestId('phase-output-verification')).toContainText(
    'cargo test --workspace',
  );
});
