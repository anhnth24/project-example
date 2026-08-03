// P2-17 Document Graph MVP e2e (owner request 2026-07-29): login -> "Đồ thị"
// -> see communities + sidebar counts, toggle a cluster off (its nodes
// disappear), click a node -> deep-links straight to `/library/:collectionId
// ?doc=:documentId` (P2-17 gap close) and its own document preview loads
// immediately, no separate row click needed.
//
// `GraphNode.id` doubles as a document id on the real API
// (`crates/server/src/db/graph.rs::list_visible_documents` selects `d.id`
// off the `documents` table directly). The mock's own node catalog
// (`mocks/handlers/graph.ts`) otherwise uses a disjoint id range from
// `mocks/fixtures.ts`'s document store (same reason `QA_COMPARE_*` picked
// its own numeric id range there) — except "Sổ tay nhân viên 2024", which
// deliberately reuses the real seeded "Onboarding Guide.pdf" document id so
// this deep-link has a real, previewable document behind it.
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

test('click node → preview: node click deep-links to ?doc= and its document preview loads immediately', async ({
  page,
}) => {
  await login(page);
  await page.getByRole('link', { name: 'Đồ thị' }).click();
  await expect(page.getByRole('button', { name: 'Sổ tay nhân viên 2024' })).toBeVisible();

  // "Sổ tay nhân viên 2024" belongs to the Employee Handbook collection and
  // (mock-only) reuses the real "Onboarding Guide.pdf" document id.
  await page.getByRole('button', { name: /Sổ tay nhân viên 2024/ }).click();

  await expect(page).toHaveURL(/\/library\/[^/]+\?doc=[^&]+/);

  // The preview loads straight off the node click — no extra row click.
  await expect(page.getByTestId('document-preview-markdown')).toContainText(
    'Mock preview content for version 1',
  );
});
