// P2-18 (org -> project -> collection -> document grouping) + the "Khu Quản
// trị" move (owner critique, 2026-07-29: "quản lý project/document/người
// dùng đang thiết kế UIUX chưa hợp lý"): project management (create/rename/
// assign) now lives at `/admin/projects` (`AdminProjectsPage.tsx`), not
// inside the Library page — `ProjectsPanel.tsx` is gone. `LibraryPage` keeps
// only the read side: `CollectionNav`'s project grouping, plus a "Quản lý dự
// án" shortcut link into the new page.
//
// Fixture ground truth (src/mocks/fixtures.ts): org A seeds two projects —
// "Nhân sự" (assigned to the Employee Handbook collection only) and "Sản
// phẩm" (no collections yet) — and one always-unassigned collection,
// "Product Specs". Demo user has `doc.upload` in mock/E2E mode only
// (`mocks/browser.ts`), so the admin projects UI + the rail's "Dự án" item
// are reachable by default here.
import { expect, test } from '@playwright/test';
import { login, revokeDocUpload } from './support';

/** Scoped to the rail's own nav landmark: an unscoped `getByRole('link',
 * {name: 'Dự án'})` is a substring match (Playwright's default) that also
 * matches `LibraryPage`'s own "Quản lý dự án" shortcut link. */
function railLink(page: import('@playwright/test').Page, name: string) {
  return page.getByRole('navigation', { name: 'Điều hướng chính' }).getByRole('link', { name });
}

async function openAdminProjects(page: import('@playwright/test').Page): Promise<void> {
  await railLink(page, 'Dự án').click();
  // `exact: true`: an unscoped substring match also hits this same page's
  // own "Danh sách dự án"/"Tạo dự án mới"/"Chưa thuộc dự án" card headings.
  await expect(page.getByRole('heading', { name: 'Dự án', exact: true })).toBeVisible();
  // The seeded "Nhân sự" project row (with its Employee Handbook chip) is
  // enough to know the table has loaded.
  await expect(page.getByRole('table', { name: 'Danh sách dự án' })).toBeVisible();
}

test('create a project and assign a collection to it from /admin/projects, then see the Library nav regroup', async ({
  page,
}) => {
  await login(page);
  await openAdminProjects(page);

  // Seeded grouping is visible before any mutation.
  const table = page.getByRole('table', { name: 'Danh sách dự án' });
  const hrRow = table.getByRole('row').filter({ hasText: 'Nhân sự' });
  await expect(hrRow.getByText('Employee Handbook')).toBeVisible();
  await expect(page.getByText('Product Specs')).toBeVisible();

  // Create a new project.
  await page.getByLabel('Tên dự án', { exact: true }).fill('Marketing');
  await page.getByRole('button', { name: 'Tạo dự án' }).click();
  await expect(page.getByLabel('Tên dự án', { exact: true })).toHaveValue('');
  await expect(table.getByRole('row').filter({ hasText: 'Marketing' })).toBeVisible();

  // Assign the always-unassigned "Product Specs" collection to it, from the
  // "Chưa thuộc dự án" section.
  const assignSelect = page.getByRole('combobox', {
    name: 'Gán dự án cho bộ sưu tập Product Specs',
  });
  await assignSelect.click();
  await page.getByRole('option', { name: 'Marketing' }).click();

  // It moves into the Marketing row's own chip list, and out of "Chưa thuộc dự án".
  const marketingRow = table.getByRole('row').filter({ hasText: 'Marketing' });
  await expect(marketingRow.getByText('Product Specs')).toBeVisible();
  await expect(
    page.getByRole('combobox', { name: 'Gán dự án cho bộ sưu tập Product Specs' }),
  ).toHaveCount(0);

  // Back in the Library, the collection nav regroups it under "Marketing".
  await page.getByRole('link', { name: 'Thư viện' }).click();
  const nav = page.getByRole('navigation', { name: 'Điều hướng bộ sưu tập' });
  const marketingGroup = nav.locator('p.eyebrow', { hasText: 'Marketing' }).locator('..');
  await expect(marketingGroup.getByRole('link', { name: 'Product Specs' })).toBeVisible();
});

test('renaming a project inline persists, and unassigning a collection moves it back to "Chưa thuộc dự án"', async ({
  page,
}) => {
  await login(page);
  await openAdminProjects(page);

  const table = page.getByRole('table', { name: 'Danh sách dự án' });
  const hrRow = table.getByRole('row').filter({ hasText: 'Nhân sự' });

  await hrRow.getByRole('button', { name: 'Sửa tên dự án Nhân sự' }).click();
  await page.getByLabel('Tên mới cho dự án Nhân sự').fill('Nhân sự & Văn hóa');
  await page.getByRole('button', { name: 'Lưu' }).click();

  const renamedRow = table.getByRole('row').filter({ hasText: 'Nhân sự & Văn hóa' });
  await expect(renamedRow).toBeVisible();
  await expect(renamedRow.getByText('Employee Handbook')).toBeVisible();

  // Unassign the Employee Handbook chip from it.
  await renamedRow
    .getByRole('button', {
      name: 'Bỏ gán bộ sưu tập Employee Handbook khỏi dự án Nhân sự & Văn hóa',
    })
    .click();
  await expect(renamedRow.getByText('Chưa có bộ sưu tập nào')).toBeVisible();
  await expect(
    page.getByRole('combobox', { name: 'Gán dự án cho bộ sưu tập Employee Handbook' }),
  ).toBeVisible();
});

test('creating a project with a name already in use in this org shows the 409 inline, without navigating away', async ({
  page,
}) => {
  await login(page);
  await openAdminProjects(page);

  // "Nhân sự" is already seeded in this org.
  await page.getByLabel('Tên dự án', { exact: true }).fill('Nhân sự');
  await page.getByRole('button', { name: 'Tạo dự án' }).click();

  await expect(
    page.getByText('Tên dự án này đã được dùng trong tổ chức — hãy chọn một tên khác.'),
  ).toBeVisible();
  // Still on the create form, nothing was cleared/navigated away.
  await expect(page.getByLabel('Tên dự án', { exact: true })).toHaveValue('Nhân sự');
  await expect(page.getByRole('heading', { name: 'Dự án', exact: true })).toBeVisible();
});

test('a caller without doc.upload does not see the rail\'s "Dự án" item or reach /admin/projects', async ({
  page,
}) => {
  await page.goto('/login');
  await revokeDocUpload(page);
  await page.getByLabel('Email').fill('demo@markhand.test');
  await page.getByLabel('Mật khẩu').fill('demo-password');
  await page.getByRole('button', { name: 'Đăng nhập' }).click();
  await expect(page.getByRole('link', { name: 'Thư viện' })).toBeVisible();

  // Members/Usage stay visible — this task only gates "Dự án" — while "Dự
  // án" itself is gone from the rail. The Library page's own "Quản lý dự án"
  // shortcut link is gated on the same permission too (see `LibraryPage.tsx`),
  // so it is also absent — asserted separately from the rail scope below.
  await expect(railLink(page, 'Thành viên')).toBeVisible();
  await expect(railLink(page, 'Sử dụng')).toBeVisible();
  await expect(railLink(page, 'Dự án')).toHaveCount(0);
  await expect(page.getByRole('link', { name: 'Quản lý dự án' })).toHaveCount(0);

  // The route itself is equally gated (`ProtectedRoute
  // permission="doc.upload"` in App.tsx) — covered at the unit level
  // (`App.test.tsx`'s "renders an in-shell notice ... without doc.upload")
  // rather than here: reaching `/admin/projects` with no rail link to click
  // would need a real navigation, which — per this suite's own module doc —
  // re-seeds the mock store and drops the session, defeating the point of
  // this specific test (proving the rail hides the item for this session).
});
