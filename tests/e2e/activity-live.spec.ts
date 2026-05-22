/**
 * Issue #31: Activity panel is fed live by SSE.
 *
 * ProjectLayout hydrates from `/snapshot` and subscribes to `/events`.
 * Triggering an Activity-emitting mutation via the API in another process
 * must update the Activity panel without any user click or page reload.
 *
 * The Activity-emitting mutations in production all require the worktrunk +
 * codex binaries (assignment lifecycle). For e2e we instead drive a test
 * endpoint gated by `AGENTIC_AFK_TEST_ENDPOINTS=1` that goes through the
 * production `activity_publisher`, so the SSE wire format and the audit log
 * exercise the same code path.
 */
import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';

test('Activity panel updates live from SSE without a page reload', async ({
  page,
  request,
}) => {
  const projectPath = resolve('target/playwright/activity-live', randomUUID());
  await mkdir(resolve(projectPath, '.git'), { recursive: true });

  const created = await request.post('/api/projects', {
    data: { path: projectPath },
  });
  await expect(created).toBeOK();
  const project = await created.json();

  await page.goto(`/projects/${project.id}/activity`);

  // Drop a marker to prove no full reload happens.
  await page.evaluate(() => {
    (window as unknown as { __activityLiveMark?: string }).__activityLiveMark =
      'pre-mutation';
  });

  // Wait for the Activity heading to be visible (panel hydrated from snapshot).
  await expect(page.getByRole('heading', { name: 'Activity' })).toBeVisible();
  await expect(page.getByText('No Activity recorded yet.')).toBeVisible();

  // Trigger a backend mutation via the test endpoint.
  const recorded = await request.post(
    `/api/_test/projects/${project.id}/activity`,
    {
      data: { kind: 'assignment_started', detail: 'live-sse-smoke' },
    },
  );
  await expect(recorded).toBeOK();

  // The Activity panel updates without any user action.
  await expect(page.getByText('assignment_started')).toBeVisible();
  await expect(page.getByText('live-sse-smoke')).toBeVisible();
  await expect(page.getByText('No Activity recorded yet.')).toBeHidden();

  // Confirm no page reload happened.
  const mark = await page.evaluate(
    () =>
      (window as unknown as { __activityLiveMark?: string })
        .__activityLiveMark,
  );
  expect(mark).toBe('pre-mutation');

  // A second mutation appears live and prepended above the first.
  const second = await request.post(
    `/api/_test/projects/${project.id}/activity`,
    {
      data: { kind: 'plan_run_phase_completed', detail: 'second-event' },
    },
  );
  await expect(second).toBeOK();
  await expect(page.getByText('plan_run_phase_completed')).toBeVisible();
  await expect(page.getByText('second-event')).toBeVisible();
});
