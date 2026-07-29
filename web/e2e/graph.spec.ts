// P2-17 Document Graph MVP e2e (owner request 2026-07-29): login -> "Đồ thị"
// -> see communities + sidebar counts, toggle a cluster off (its nodes
// disappear), click a node -> lands in that node's own collection's library
// (P2-07's existing route) and reaches a real document preview from there.
//
// Node click does NOT deep-link straight into the graph node's own document
// preview: `LibraryPage`'s selected-document state is local component state,
// not a URL param, so there is no document-level route to navigate to (see
// `GraphPage.tsx`'s module doc). The graph's own node catalog
// (`mocks/handlers/graph.ts`) is a separate, disjoint fixture set from
// `mocks/fixtures.ts`'s document store for the same reason `QA_COMPARE_*`
// picked its own numeric id range there — so the document actually opened
// below ("Onboarding Guide.pdf") is the real seeded Employee Handbook
// document, not the graph node's own synthetic title.
import { expect, test } from '@playwright/test';
import { login } from './support';

test('opens the graph and shows communities with per-cluster counts', async ({ page }) => {
  await login(page);
  await page.getByRole('link', { name: 'Đồ thị' }).click();

  await expect(page.getByRole('heading', { name: 'Đồ thị tài liệu' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Sổ tay nhân viên 2024' })).toBeVisible();
  await expect(page.getByRole('button', { name: /Đặc tả sản phẩm v1/ })).toBeVisible();
  await expect(page.getByRole('button', { name: /Biên bản họp liên phòng/ })).toBeVisible();

  await expect(page.getByRole('heading', { name: 'Cộng đồng' })).toBeVisible();
  await expect(page.getByLabel(/Employee Handbook/)).toBeVisible();
  await expect(page.getByLabel(/Product Specs/)).toBeVisible();
  await expect(page.getByLabel(/Liên phòng ban/)).toBeVisible();
});

test('turning off a community hides its nodes; "Chọn tất cả" restores them', async ({ page }) => {
  await login(page);
  await page.getByRole('link', { name: 'Đồ thị' }).click();
  await expect(page.getByRole('button', { name: 'Sổ tay nhân viên 2024' })).toBeVisible();

  await page.getByLabel(/Employee Handbook/).click();
  await expect(page.getByRole('button', { name: 'Sổ tay nhân viên 2024' })).toHaveCount(0);
  // A different cluster is unaffected.
  await expect(page.getByRole('button', { name: /Đặc tả sản phẩm v1/ })).toBeVisible();

  await page.getByRole('button', { name: 'Chọn tất cả' }).click();
  await expect(page.getByRole('button', { name: 'Sổ tay nhân viên 2024' })).toBeVisible();
});

test("clicking a node navigates into that document's own collection, reaching a real preview", async ({
  page,
}) => {
  await login(page);
  await page.getByRole('link', { name: 'Đồ thị' }).click();
  await expect(page.getByRole('button', { name: 'Sổ tay nhân viên 2024' })).toBeVisible();

  // "Sổ tay nhân viên 2024" belongs to the Employee Handbook collection.
  await page.getByRole('button', { name: /Sổ tay nhân viên 2024/ }).click();

  await expect(page).toHaveURL(/\/library\//);
  const table = page.getByRole('table', { name: 'Danh sách tài liệu' });
  await expect(table.getByRole('button', { name: /Onboarding Guide\.pdf/ })).toBeVisible();

  await table.getByRole('button', { name: /Onboarding Guide\.pdf/ }).click();
  await expect(page.getByTestId('document-preview-markdown')).toContainText(
    'Mock preview content for version 1',
  );
});
