/**
 * PRD override surface — operator marks a Source Issue as a Parent-Issue-style
 * PRD so it disappears from every active Planning Snapshot bucket. The
 * "N PRDs hidden" footer surfaces the unmark affordance.
 */
import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

async function createProjectWithMarkdownIssues(
  request: import('@playwright/test').APIRequestContext,
  label: string,
) {
  const projectPath = resolve(`target/playwright/prd-${label}`, randomUUID());
  const issuesDir = resolve(projectPath, '.scratch/issues');
  await mkdir(issuesDir, { recursive: true });
  await mkdir(resolve(projectPath, '.git'), { recursive: true });
  await writeFile(
    resolve(issuesDir, '001-prd.md'),
    '# Big PRD\n\nReadiness: ready\nSource Order: 1\n',
  );
  await writeFile(
    resolve(issuesDir, '002-work.md'),
    '# Real work\n\nReadiness: ready\nSource Order: 2\n',
  );

  const created = await request.post('/api/projects', { data: { path: projectPath } });
  await expect(created).toBeOK();
  const project = await created.json();

  await request.put(`/api/projects/${project.id}/trust`);
  await request.put(`/api/projects/${project.id}/issue-source`, {
    data: { kind: 'local_markdown', locator: '.scratch/issues' },
  });
  await request.post(`/api/projects/${project.id}/issue-source/sync`);
  return project.id as string;
}

test('Mark as PRD hides the Source Issue and unmark restores it', async ({
  page,
  request,
}) => {
  const projectId = await createProjectWithMarkdownIssues(request, 'mark-flow');
  await page.goto(`/projects/${projectId}/planning`);

  await expect(page.getByText('Big PRD', { exact: true })).toBeVisible();
  await expect(page.getByText('Real work', { exact: true })).toBeVisible();
  // No PRDs marked yet — footer hidden.
  await expect(page.getByTestId('prd-overrides-footer')).toHaveCount(0);

  // Mark the PRD. Source id is the local_markdown stem.
  await page.getByTestId('mark-prd-001-prd').click();

  // Big PRD vanishes from active buckets; footer surfaces it.
  await expect(page.getByText('Big PRD', { exact: true })).toBeHidden();
  await expect(page.getByText('Real work', { exact: true })).toBeVisible();
  const footer = page.getByTestId('prd-overrides-footer');
  await expect(footer).toBeVisible();
  await expect(footer.getByTestId('prd-overrides-toggle')).toContainText(
    '1 PRDs hidden',
  );

  // Expand footer to unmark.
  await footer.getByTestId('prd-overrides-toggle').click();
  await footer.getByTestId('unmark-prd-001-prd').click();

  // PRD restored to Eligible Ready Issues; footer collapses away.
  await expect(page.getByText('Big PRD', { exact: true })).toBeVisible();
  await expect(page.getByTestId('prd-overrides-footer')).toHaveCount(0);
});
