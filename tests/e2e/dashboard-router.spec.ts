import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

async function createBareProject(
  request: import('@playwright/test').APIRequestContext,
  label: string,
): Promise<string> {
  const projectPath = resolve(`target/playwright/router-${label}`, randomUUID());
  await mkdir(resolve(projectPath, '.git'), { recursive: true });
  const created = await request.post('/api/projects', {
    data: { path: projectPath },
  });
  await expect(created).toBeOK();
  const project = await created.json();
  return project.id as string;
}

test('Dashboard deep-links to each Project sub-route without flashing the Overview', async ({
  page,
  request,
}) => {
  const projectId = await createBareProject(request, 'deeplink');

  // Activity sub-route: should render the Activity panel directly.
  await page.goto(`/projects/${projectId}/activity`);
  await expect(page.getByRole('heading', { name: 'Activity' })).toBeVisible();
  // Overview-only sections (Issue Source candidates / Planning snapshot) must
  // not appear on the Activity sub-route, proving no fallback to Overview.
  await expect(
    page.getByRole('heading', { name: 'Issue Source candidates' }),
  ).toBeHidden();
  await expect(
    page.getByRole('heading', { name: 'Planning snapshot' }),
  ).toBeHidden();

  // Planning sub-route.
  await page.goto(`/projects/${projectId}/planning`);
  await expect(page.getByRole('heading', { name: 'Planning snapshot' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Activity' })).toBeHidden();

  // Source sub-route.
  await page.goto(`/projects/${projectId}/source`);
  await expect(
    page.getByRole('heading', { name: 'Issue Source candidates' }),
  ).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Activity' })).toBeHidden();

  // Settings route.
  await page.goto('/settings');
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();
});

test('Dashboard navigates between Project sub-routes without a full page reload', async ({
  page,
  request,
}) => {
  const projectId = await createBareProject(request, 'spa-nav');

  await page.goto(`/projects/${projectId}`);
  // CardHead renders the Overview metadata card title as <h2>Project</h2>.
  await expect(
    page.getByRole('heading', { level: 2, name: 'Project' }).first(),
  ).toBeVisible();

  // Tag the document so we can detect a full reload (would clear the marker).
  await page.evaluate(() => {
    (window as unknown as { __routerSpaMarker: number }).__routerSpaMarker = 1;
  });

  await page.getByRole('link', { name: 'Activity', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/projects/${projectId}/activity$`));
  await expect(page.getByRole('heading', { name: 'Activity' })).toBeVisible();

  await page.getByRole('link', { name: 'Planning', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/projects/${projectId}/planning$`));
  await expect(page.getByRole('heading', { name: 'Planning snapshot' })).toBeVisible();

  const marker = await page.evaluate(
    () => (window as unknown as { __routerSpaMarker?: number }).__routerSpaMarker,
  );
  expect(marker).toBe(1);
});
