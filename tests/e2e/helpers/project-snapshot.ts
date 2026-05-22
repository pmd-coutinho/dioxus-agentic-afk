/**
 * Playwright helpers for driving the Dashboard's live `ProjectStore` from
 * mocked snapshots and synthetic SSE events.
 *
 * Issue #32 removed `use_resource` REST fetches from the Assignment, Planning,
 * and Issue Source panels — they now read from `ProjectStore`. Tests that
 * previously mocked `/assignment-state`, `/planning-snapshot`, and friends
 * must now mock the combined `/api/projects/:id/snapshot` endpoint and inject
 * post-mutation state changes via the test-only `_test/project-event`
 * endpoint (gated by `AGENTIC_AFK_TEST_ENDPOINTS=1`).
 */
import type { APIRequestContext, Page } from '@playwright/test';

export interface MockSnapshotInput {
  project?: Record<string, unknown>;
  planning_snapshot?: Record<string, unknown> | null;
  assignment_state?: Record<string, unknown>;
  activity?: Array<Record<string, unknown>>;
  issue_source_candidates?: Array<Record<string, unknown>>;
}

const EMPTY_ASSIGNMENT_STATE = {
  active_assignment: null,
  waiting_ready_issue_count: 0,
};

/**
 * Intercept `/api/projects/:id/snapshot` so the Dashboard hydrates from the
 * caller-supplied `getSnapshot()` result. `getSnapshot` is a callback so tests
 * can mutate the returned shape between renders (e.g. before and after a
 * mutation).
 */
export async function mockProjectSnapshot(
  page: Page,
  projectId: string,
  getSnapshot: () => MockSnapshotInput,
): Promise<void> {
  await page.route(
    `**/api/projects/${projectId}/snapshot**`,
    async (route) => {
      const input = getSnapshot();
      const snapshot = {
        project: input.project ?? {
          id: projectId,
          path: '/tmp/playwright',
          trusted: true,
          git_summary: null,
          enabled_issue_source: null,
        },
        planning_snapshot: input.planning_snapshot ?? null,
        assignment_state: input.assignment_state ?? EMPTY_ASSIGNMENT_STATE,
        activity: input.activity ?? [],
        issue_source_candidates: input.issue_source_candidates ?? [],
      };
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ snapshot, sequence: 0 }),
      });
    },
  );
  // The SSE `/events` stream is intentionally NOT mocked. Tests that need a
  // post-mutation state change publish a real `ProjectEvent` through
  // `publishProjectEvent`, which hits the real server EventBus and arrives
  // through the live `/events` subscription the Dashboard opens on mount.
}

/**
 * Publish a `ProjectEvent` for `projectId` via the test-only endpoint so the
 * live Dashboard reflects a post-mutation lifecycle transition.
 */
export async function publishProjectEvent(
  request: APIRequestContext,
  projectId: string,
  event: Record<string, unknown>,
): Promise<void> {
  const response = await request.post(
    `/api/_test/projects/${projectId}/project-event`,
    { data: event },
  );
  if (!response.ok()) {
    throw new Error(
      `publishProjectEvent failed: ${response.status()} ${await response.text()}`,
    );
  }
}
