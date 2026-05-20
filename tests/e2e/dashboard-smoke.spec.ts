import { expect, test } from '@playwright/test';

test('Dashboard loads the seeded Project and API health from the Local Control Plane', async ({
  page,
  request,
}) => {
  await page.goto('/');

  await expect(page.getByRole('heading', { name: 'agentic-afk' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'API connected' })).toBeVisible();
  await expect(page.getByText('Loading Projects')).toBeHidden();

  const projectLink = page.getByRole('link', { name: /dioxus-agentic-afk/ });
  await expect(projectLink).toBeVisible();
  await projectLink.click();

  await expect(page).toHaveURL(/\/projects\/[^/]+$/);
  await expect(page.getByRole('heading', { name: 'Project detail' })).toBeVisible();
  await expect(page.getByText('Project ID')).toBeVisible();

  const health = await request.get('/health');
  await expect(health).toBeOK();
  await expect(await health.json()).toEqual({ status: 'ok' });
});
