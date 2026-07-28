import { expect, test } from '@playwright/test';

// Foundation smoke: proves the mock-mode app boots in a real browser and the
// in-browser mock backend answers a full login round-trip. The broad flow
// coverage (upload, actions, org switch, member admin, quota/permission
// denials) builds on this once the harness is proven.
test('the mock-mode app boots and logs in with the demo user', async ({ page }) => {
  await page.goto('/login');

  await expect(page.getByRole('heading', { name: 'Đăng nhập vào Markhand' })).toBeVisible();

  await page.getByLabel('Email').fill('demo@markhand.test');
  await page.getByLabel('Mật khẩu').fill('demo-password');
  await page.getByRole('button', { name: 'Đăng nhập' }).click();

  // Successful login leaves /login and mounts the application rail; the
  // "Thư viện" destination is present on every in-app route.
  await expect(page.getByRole('link', { name: 'Thư viện' })).toBeVisible();
  await expect(page).not.toHaveURL(/\/login/);
});
