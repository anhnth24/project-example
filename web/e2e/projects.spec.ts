// P2-18 (org -> project -> collection -> document grouping): create a
// project, assign a collection to it from the Library page, then use the
// "Phạm vi" (scope) dropdown in the Q&A composer to narrow search to that
// project's collections and back out to "Tất cả dự án".
//
// Fixture ground truth (src/mocks/fixtures.ts): org A seeds two projects —
// "Nhân sự" (assigned to the Employee Handbook collection only) and "Sản
// phẩm" (no collections yet) — and one always-unassigned collection,
// "Product Specs" ("Roadmap.xlsx" lives there, matched by the "lộ trình quý
// 3" query `qa.spec.ts` already exercises unscoped). Demo user has
// `doc.upload` in mock/E2E mode only (`mocks/browser.ts`), so the project
// create/assign UI is reachable here.
import { expect, test } from '@playwright/test';
import { login } from './support';

test('create a project, assign a collection, then scope Q&A search to it and back to "Tất cả dự án"', async ({
  page,
}) => {
  await login(page);
  await page.getByRole('link', { name: 'Thư viện' }).click();

  // Seeded grouping is visible before any mutation. Scoped to the nav
  // itself: "Nhân sự"/"Chưa thuộc dự án" also appear as the current value
  // inside each collection's own assign-project `<select>` trigger below.
  const nav = page.getByRole('navigation', { name: 'Điều hướng bộ sưu tập' });
  await expect(nav.getByText('Nhân sự')).toBeVisible();
  await expect(nav.getByText('Chưa thuộc dự án')).toBeVisible();

  // Create a new project.
  await page.getByLabel('Tên dự án mới').fill('Marketing');
  await page.getByRole('button', { name: 'Tạo dự án' }).click();
  await expect(page.getByLabel('Tên dự án mới')).toHaveValue('');

  // Assign the always-unassigned "Product Specs" collection to it.
  const assignSelect = page.getByRole('combobox', {
    name: 'Dự án cho bộ sưu tập Product Specs',
  });
  await expect(assignSelect).toContainText('Chưa thuộc dự án');
  await assignSelect.click();
  await page.getByRole('option', { name: 'Marketing' }).click();
  await expect(assignSelect).toContainText('Marketing');

  // The nav re-groups it under the new project heading: "Product Specs" now
  // renders inside the same group container as the "Marketing" heading.
  const marketingGroup = page.locator('p.eyebrow', { hasText: 'Marketing' }).locator('..');
  await expect(marketingGroup.getByRole('link', { name: 'Product Specs' })).toBeVisible();

  // Q&A: scope to "Nhân sự" (Employee Handbook only) and confirm the search
  // narrows correctly in both directions.
  await page.getByRole('link', { name: 'Hỏi đáp' }).click();
  const scopeSelect = page.getByRole('combobox', { name: 'Phạm vi dự án' });
  await expect(scopeSelect).toContainText('Tất cả dự án');
  await scopeSelect.click();
  await page.getByRole('option', { name: 'Nhân sự' }).click();
  await expect(scopeSelect).toContainText('Nhân sự');

  // A query matching this project's own document succeeds.
  await page.getByLabel('Từ khóa').fill('hội nhập');
  await page.getByRole('button', { name: 'Tìm kiếm' }).click();
  await expect(page.getByText('Onboarding Guide.pdf')).toBeVisible();

  // A query matching only Product Specs' document (outside this project's
  // scope) finds nothing while scoped to "Nhân sự".
  await page.getByLabel('Từ khóa').fill('lộ trình quý 3');
  await page.getByRole('button', { name: 'Tìm kiếm' }).click();
  await expect(page.getByText(/Không tìm thấy kết quả phù hợp/)).toBeVisible();

  // Back to "Tất cả dự án" (no filter) — the same query now finds it.
  await scopeSelect.click();
  await page.getByRole('option', { name: 'Tất cả dự án' }).click();
  await expect(scopeSelect).toContainText('Tất cả dự án');
  await page.getByRole('button', { name: 'Tìm kiếm' }).click();
  await expect(page.getByText('Roadmap.xlsx')).toBeVisible();
});
