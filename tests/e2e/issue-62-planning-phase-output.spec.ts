import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

/**
 * Issue #62: typed `Planning` PhaseOutputBody variant + PlanRunCard header
 * renderer. The Planning row lands as a Plan-Run-scoped Phase Output
 * (`assignment_id = None`) in the PlanRunCard header section. The
 * expanded body renders one row per Planned Claim (Source Issue identity
 * + derived issue branch), the planner rationale/summary, and any
 * explicitly rejected candidates with reason. An empty Plan Run shows
 * the empty-selection state explicitly (not blank).
 */
test('Planning Phase Output with selections renders Source Issue identity + branch', async ({
  page,
  request,
}) => {
  const projectPath = resolve(
    'target/playwright/issue-62-planning-phase-output-selection',
    randomUUID(),
  );
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

  // Plan Run carries one typed Planning Phase Output with a non-empty
  // selections list + a rejected candidate.
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
          phase: 'planning',
          outcome: 'succeeded',
          body_json: {
            phase: 'planning',
            selections: [
              {
                source_issue_id: '62',
                title: 'Typed Planning variant',
                branch: 'agent/issue-62',
                selection_summary: 'baseline ready',
              },
            ],
            summary: 'one ready issue selected for this Plan Run',
            rejected_candidates: [
              {
                source_issue_id: '63',
                reason: 'depends on #62',
              },
            ],
          },
          recorded_at: 'unix:2',
          assignment_id: null,
        },
      ],
      assignments: [],
    },
  });

  // Header section renders the Plan-Run-scoped Planning row.
  const header = page.getByTestId('plan-run-card-header-phase-outputs');
  await expect(header).toBeVisible();

  const headerRows = header.getByTestId('plan-run-phase-output-row');
  await expect(headerRows).toHaveCount(1);
  await expect(headerRows.nth(0)).toContainText('planning');
  await expect(headerRows.nth(0)).toContainText('succeeded');

  // Expand the Planning row and verify the typed body fields.
  await headerRows.nth(0).getByTestId('phase-output-toggle').click();
  await expect(headerRows.nth(0)).toHaveAttribute('data-expanded', 'true');

  const planningBody = headerRows.nth(0).getByTestId('phase-output-planning');
  await expect(planningBody).toBeVisible();

  // Rationale (summary) row.
  await expect(
    headerRows.nth(0).getByTestId('phase-output-planning-rationale'),
  ).toContainText('one ready issue selected');

  // Exactly one selection row carrying Source Issue identity + derived
  // issue branch.
  const selections = headerRows.nth(0).getByTestId('phase-output-planning-selection');
  await expect(selections).toHaveCount(1);
  await expect(selections.nth(0)).toContainText('62');
  await expect(selections.nth(0)).toContainText('Typed Planning variant');
  await expect(
    selections.nth(0).getByTestId('phase-output-planning-selection-branch'),
  ).toContainText('agent/issue-62');

  // Rejected candidate with reason.
  const rejected = headerRows.nth(0).getByTestId('phase-output-planning-rejected');
  await expect(rejected).toHaveCount(1);
  await expect(rejected.nth(0)).toContainText('63');
  await expect(
    rejected.nth(0).getByTestId('phase-output-planning-rejected-reason'),
  ).toContainText('depends on #62');

  // Empty-state marker must NOT render when selections are present.
  await expect(
    headerRows.nth(0).getByTestId('phase-output-planning-empty'),
  ).toHaveCount(0);
});

test('empty Plan Run renders Planning row with explicit empty-selection state', async ({
  page,
  request,
}) => {
  const projectPath = resolve(
    'target/playwright/issue-62-planning-phase-output-empty',
    randomUUID(),
  );
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
      state: 'succeeded_empty',
      started_at: 'unix:1',
      finished_at: 'unix:2',
      phase_outputs: [
        {
          phase: 'planning',
          outcome: 'succeeded_empty',
          body_json: {
            phase: 'planning',
            selections: [],
            summary: '',
            rejected_candidates: [],
          },
          recorded_at: 'unix:2',
          assignment_id: null,
        },
      ],
      assignments: [],
    },
  });

  const header = page.getByTestId('plan-run-card-header-phase-outputs');
  await expect(header).toBeVisible();

  const headerRows = header.getByTestId('plan-run-phase-output-row');
  await expect(headerRows).toHaveCount(1);
  await expect(headerRows.nth(0)).toContainText('planning');
  await expect(headerRows.nth(0)).toContainText('succeeded_empty');

  // Expand the Planning row.
  await headerRows.nth(0).getByTestId('phase-output-toggle').click();
  await expect(headerRows.nth(0)).toHaveAttribute('data-expanded', 'true');

  // The empty-selection state renders explicitly (not blank).
  await expect(
    headerRows.nth(0).getByTestId('phase-output-planning'),
  ).toBeVisible();
  await expect(
    headerRows.nth(0).getByTestId('phase-output-planning-empty'),
  ).toBeVisible();

  // No selection or rejected rows on an empty Plan Run.
  await expect(
    headerRows.nth(0).getByTestId('phase-output-planning-selection'),
  ).toHaveCount(0);
  await expect(
    headerRows.nth(0).getByTestId('phase-output-planning-rejected'),
  ).toHaveCount(0);
});
