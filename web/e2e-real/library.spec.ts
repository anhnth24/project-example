// Real-deployment smoke (P2-15 / P2-20): after logging in against the real
// backend with runtime credentials, the library shell renders and lists the
// run-scoped collection from a real `GET /collections` round-trip.
import { expect, test } from '@playwright/test';
import { login, runtimeFixture } from './support';

test('logging in shows the library shell with the run-scoped collection', async ({ page }) => {
  const fixture = runtimeFixture();
  await login(page);

  await page.getByRole('link', { name: 'Thư viện' }).click();
  await expect(page.getByRole('heading', { name: 'Tất cả bộ sưu tập' })).toBeVisible();

  await expect(
    page.getByRole('navigation', { name: 'Điều hướng bộ sưu tập' }).getByRole('link', {
      name: fixture.collectionName,
    }),
  ).toBeVisible();
});
