// Real-deployment smoke (P2-15, real-deployment half): proves the built SPA,
// served by a real fileconv-server, completes a genuine login round-trip
// against Postgres — no fetch mock involved.
import { expect, test } from '@playwright/test';
import { login } from './support';

test('the built SPA boots against the real backend and logs in with the seeded admin', async ({
  page,
}) => {
  await page.goto('/login');
  await expect(page.getByRole('heading', { name: 'Đăng nhập vào Folyvo' })).toBeVisible();

  await login(page);
});
