// Real-deployment smoke (P2-15, real-deployment half): after logging in
// against the real backend, the library shell renders and lists the seeded
// collection from a real `GET /collections` round-trip. Upload→indexed and
// per-document actions are a later pass — see `support.ts`'s scope note.
import { expect, test } from '@playwright/test';
import { login, SEEDED_COLLECTION_NAME } from './support';

test('logging in shows the library shell with the seeded collection', async ({ page }) => {
  await login(page);

  await page.getByRole('link', { name: 'Thư viện' }).click();
  await expect(page.getByRole('heading', { name: 'Tất cả bộ sưu tập' })).toBeVisible();

  await expect(
    page.getByRole('navigation', { name: 'Điều hướng bộ sưu tập' }).getByRole('link', {
      name: SEEDED_COLLECTION_NAME,
    }),
  ).toBeVisible();
});
