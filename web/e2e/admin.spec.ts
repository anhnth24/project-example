// Member/role admin (P2-15 flow 7, P2-11) and usage (flow 8, P2-12). The demo
// user holds `member.manage` in mock mode and is seeded as an active owner, so
// the admin pages are reachable and owner-tier controls are enabled.
import { expect, test } from '@playwright/test';
import { forceStatus, IDS, login } from './support';

async function openMembers(page: import('@playwright/test').Page): Promise<void> {
  await page.getByRole('link', { name: 'Thành viên' }).click();
  await expect(page.getByRole('heading', { name: 'Thành viên và vai trò' })).toBeVisible();
  // The seeded owner row is enough to know the list has loaded.
  await expect(page.getByRole('row').filter({ hasText: IDS.demoUser })).toBeVisible();
}

test('inviting a member shows the one-time token', async ({ page }) => {
  await login(page);
  await openMembers(page);

  await page.getByLabel('Email người được mời').fill('nguoi-moi@example.com');
  await page.getByRole('button', { name: 'Gửi lời mời' }).click();

  const dialog = page.getByRole('dialog', { name: 'Đã tạo lời mời' });
  await expect(dialog).toBeVisible();
  // The plaintext invite token is shown exactly once, here.
  await expect(dialog.getByLabel('Mã mời (một lần)')).toHaveValue(/^mock-invite-token\./);
  await expect(dialog.getByText('nguoi-moi@example.com')).toBeVisible();
});

test("changing a member's role persists after refetch", async ({ page }) => {
  await login(page);
  await openMembers(page);

  // The second member is seeded `admin`; demote to editor.
  const roleSelect = page.getByRole('combobox', { name: `Vai trò của ${IDS.secondMember}` });
  await expect(roleSelect).toContainText('Quản trị viên');
  await roleSelect.click();
  await page.getByRole('option', { name: 'Biên tập viên' }).click();

  // After the PATCH + members refetch, the row reflects the new role.
  await expect(roleSelect).toContainText('Biên tập viên');
});

test('suspending then reactivating a member toggles their state', async ({ page }) => {
  await login(page);
  await openMembers(page);

  // Second member is seeded active — suspend, then bring back.
  const secondRow = page.getByRole('row').filter({ hasText: IDS.secondMember });
  await secondRow.getByRole('button', { name: 'Tạm ngưng' }).click();
  await expect(secondRow.getByText('Đã tạm ngưng')).toBeVisible();

  await secondRow.getByRole('button', { name: 'Kích hoạt lại' }).click();
  await expect(secondRow.getByText('Đang hoạt động')).toBeVisible();
});

test('the seeded suspended member can be reactivated', async ({ page }) => {
  await login(page);
  await openMembers(page);

  // Third member is seeded `viewer` / `suspended`.
  const thirdRow = page.getByRole('row').filter({ hasText: IDS.thirdMember });
  await expect(thirdRow.getByText('Đã tạm ngưng')).toBeVisible();
  await thirdRow.getByRole('button', { name: 'Kích hoạt lại' }).click();
  await expect(thirdRow.getByText('Đang hoạt động')).toBeVisible();
});

test('suspending the sole active owner is blocked by the last-owner invariant (409)', async ({
  page,
}) => {
  await login(page);
  await openMembers(page);

  // The demo user is the only active owner; the server's last-owner invariant
  // rejects suspending them with a 409, surfaced as its specific message.
  const ownerRow = page.getByRole('row').filter({ hasText: IDS.demoUser });
  await ownerRow.getByRole('button', { name: 'Tạm ngưng' }).click();

  await expect(
    page.getByText(
      'Không thể thực hiện: tổ chức phải luôn còn ít nhất một chủ sở hữu (owner) đang hoạt động.',
    ),
  ).toBeVisible();
});

test('an owner-tier mutation that 403s shows the owner-tier denial message', async ({ page }) => {
  await login(page);
  await openMembers(page);

  // Promoting to owner is an owner-tier action. Force it to 403 (a race the UI
  // handles honestly rather than assuming impossible) and assert the specific
  // owner-tier copy — not the generic permission message.
  await forceStatus(page, 'patchMember', 403, 1);
  const roleSelect = page.getByRole('combobox', { name: `Vai trò của ${IDS.secondMember}` });
  await roleSelect.click();
  await page.getByRole('option', { name: 'Chủ sở hữu' }).click();

  // Scoped to the members table: the same sentence also appears in the page's
  // intro `lede`, so an unscoped match would be ambiguous. The row's error
  // notice is the owner-tier denial copy (not the generic permission message).
  await expect(
    page
      .getByRole('table', { name: 'Danh sách thành viên' })
      .getByText(
        'Chỉ chủ sở hữu (owner) đang hoạt động mới có thể cấp hoặc quản lý vai trò chủ sở hữu.',
      ),
  ).toBeVisible();
});

test('the usage page renders cards from GET /usage', async ({ page }) => {
  await login(page);
  await page.getByRole('link', { name: 'Sử dụng' }).click();

  await expect(page.getByRole('heading', { name: 'Sử dụng và hạn mức' })).toBeVisible();
  // One card per seeded usage resource (handlers/members.ts getUsage).
  await expect(page.getByText('Dung lượng lưu trữ')).toBeVisible();
  await expect(page.getByText('Số tài liệu')).toBeVisible();
  await expect(page.getByText('Tác vụ đồng thời')).toBeVisible();
  await expect(page.getByText('Token (LLM)')).toBeVisible();
});
