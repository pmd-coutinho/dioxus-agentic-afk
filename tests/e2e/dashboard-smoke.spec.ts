import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

test('Dashboard loads the seeded Project and API health from the Local Control Plane', async ({
  page,
  request,
}) => {
  await page.goto('/');

  await expect(page.getByRole('heading', { name: 'agentic-afk' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'API connected' })).toBeVisible();
  await expect(page.getByText('Loading Projects')).toBeHidden();

  const projectLink = page.getByRole('link', {
    name: resolve('.'),
    exact: true,
  });
  await expect(projectLink).toBeVisible();
  await projectLink.click();

  await expect(page).toHaveURL(/\/projects\/[^/]+$/);
  await expect(page.getByRole('heading', { name: 'Project detail' })).toBeVisible();
  await expect(page.getByText('Project ID')).toBeVisible();

  const health = await request.get('/health');
  await expect(health).toBeOK();
  await expect(await health.json()).toEqual({ status: 'ok' });
});

test('Project detail enables a discovered Issue Source candidate', async ({
  page,
  request,
}) => {
  const projectPath = resolve(
    'target/playwright/issue-source-candidate',
    randomUUID(),
  );
  await mkdir(resolve(projectPath, '.git'), { recursive: true });
  await mkdir(resolve(projectPath, '.scratch/issues'), { recursive: true });
  await writeFile(
    resolve(projectPath, '.git/config'),
    '[remote "origin"]\n    url = git@github.com:pmd-coutinho/dioxus-agentic-afk.git\n',
  );
  await writeFile(
    resolve(projectPath, '.scratch/issues/001-parent.md'),
    '# Parent planning issue\n\nReadiness: not-ready\nSource Order: 1\n',
  );
  await writeFile(
    resolve(projectPath, '.scratch/issues/002-blocked.md'),
    '# Blocked ready issue\n\nReadiness: ready\nParent Issue: 001-parent\nIssue Dependencies: 003-eligible\nSource Order: 2\n',
  );
  await writeFile(
    resolve(projectPath, '.scratch/issues/003-eligible.md'),
    '# Eligible ready issue\n\nReadiness: ready\nParent Issue: 001-parent\nSource Order: 3\n',
  );

  const createdProject = await request.post('/api/projects', {
    data: { path: projectPath },
  });
  await expect(createdProject).toBeOK();
  const project = await createdProject.json();

  await page.goto(`/projects/${project.id}`);

  await expect(page.getByRole('heading', { name: 'Project detail' })).toBeVisible();
  await expect(page.getByText('Not enabled')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Issue Source candidates' })).toBeVisible();
  await expect(
    page.getByText('github pmd-coutinho/dioxus-agentic-afk', { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText('local_markdown .scratch/issues', { exact: true }),
  ).toBeVisible();

  await page
    .getByRole('button', { name: 'Enable local_markdown .scratch/issues' })
    .click();

  await expect(
    page.getByText('local_markdown .scratch/issues', { exact: true }),
  ).toBeVisible();
  await expect(page.getByText('Not enabled')).toBeHidden();

  await page.getByRole('button', { name: 'Refresh Issue Source' }).click();

  await expect(page.getByRole('heading', { name: 'Last sync status' })).toBeVisible();
  await expect(page.getByText('Never synced')).toBeHidden();
  await expect(page.getByRole('heading', { name: 'Eligible Ready Issues' })).toBeVisible();
  await expect(
    page.getByText('Eligible ready issue', { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Start Assignment' }),
  ).toBeHidden();
  await page.getByRole('button', { name: 'Trust Project' }).click();
  await expect(
    page.getByRole('button', { name: 'Start Assignment' }),
  ).toBeVisible();
  const blockedIssues = page
    .getByRole('heading', { name: 'Blocked Ready Issues' })
    .locator('..');
  await expect(blockedIssues).toBeVisible();
  await expect(
    blockedIssues.getByText('Blocked ready issue', { exact: true }),
  ).toBeVisible();
  await expect(blockedIssues.getByText('003-eligible', { exact: true })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Non-ready Source Issues' })).toBeVisible();
  await expect(page.getByText('Parent planning issue')).toBeVisible();

  await rm(resolve(projectPath, '.scratch/issues'), { recursive: true });
  await page.getByRole('button', { name: 'Refresh Issue Source' }).click();

  await expect(
    page.getByText(/failed to read local markdown Issue Source/).first(),
  ).toBeVisible();
  await expect(
    page.getByText('Eligible ready issue', { exact: true }),
  ).toBeVisible();
});
