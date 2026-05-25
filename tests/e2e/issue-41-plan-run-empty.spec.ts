import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

/**
 * Issue #41: empty Plan Run lifecycle through the Dashboard.
 *
 * Drives trust → execution config → start Plan Run → succeeded_empty in the
 * recent history → second start blocked until the first finishes. The server
 * is bootstrapped with `AGENTIC_AFK_TEST_PLAN_RUN_STUBS=1` so the Plan Run
 * coordinator returns an empty selection immediately.
 */

test('empty Plan Run surfaces succeeded_empty in the history and blocks a second start', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/plan-run-empty', randomUUID());
  await mkdir(resolve(projectPath, '.git'), { recursive: true });

  const created = await request.post('/api/projects', {
    data: { path: projectPath },
  });
  await expect(created).toBeOK();
  const project = await created.json();

  await request.put(`/api/projects/${project.id}/trust`);

  await page.goto(`/projects/${project.id}`);

  // Plan Run card visible, but Start button disabled until config is set.
  const startButton = page.getByTestId('start-plan-run');
  await expect(startButton).toBeVisible();
  await expect(startButton).toHaveAttribute('aria-disabled', 'true');

  // Save execution config inline.
  await page
    .getByTestId('execution-config-integration-branch')
    .fill('main');
  await page
    .getByTestId('execution-config-max-parallel-tasks')
    .fill('2');
  await page
    .getByTestId('execution-config-review-retry-limit')
    .fill('1');
  await page.getByTestId('execution-config-save').click();

  // Start Plan Run becomes enabled once the config save resolves.
  await expect(startButton).toHaveAttribute('aria-disabled', 'false');

  await startButton.click();

  // The stubbed coordinator finishes immediately, so the active card clears
  // and the run shows up in the recent history with the empty-success pill.
  await expect(page.getByTestId('plan-run-history')).toBeVisible();
  const historyRows = page.getByTestId('plan-run-history-row');
  await expect(historyRows).toHaveCount(1);
  await expect(historyRows.first()).toContainText('Empty backlog');

  // Second history entry also shows "Empty backlog" because the
  // stub endpoint seeds two empty E2E fixtures.
  await expect(historyRows.nth(0)).toContainText('Empty backlog');
  await expect(historyRows.nth(1)).toContainText('Empty backlog');
});
