// Org switch (P2-15 flow 4 acceptance, P2-06 gate: "Không render dữ liệu từ
// scope cũ"). The demo user is seeded as an active member of two orgs: org A
// ("Acme Co", the org the session boots into — Employee Handbook / Product
// Specs collections) and org B ("Globex Labs" — its own, entirely distinct
// "Globex Roadmap" collection and "Globex Master Plan.pdf" document, see
// `src/mocks/fixtures.ts`). This is the actual "switch org, and the previous
// org's data is completely gone — not just that a new orgId string appears
// somewhere" flow the old `org-scope.spec.ts` could not drive before 1C-01
// shipped `GET /orgs` / `POST /orgs/switch`.
import { expect, test } from '@playwright/test';
import { forceStatus, IDS, login, openEmployeeHandbook } from './support';

test('switching org replaces org A data with org B data — no stale org A render survives', async ({
  page,
}) => {
  await login(page);

  // See org A's data first.
  await openEmployeeHandbook(page);
  const table = page.getByRole('table', { name: 'Danh sách tài liệu' });
  await expect(table.getByRole('button', { name: /Onboarding Guide\.pdf/ })).toBeVisible();

  // Switch to org B.
  await page.getByRole('button', { name: 'Đơn vị hiện tại' }).click();
  const dialog = page.getByRole('dialog', { name: 'Đơn vị hiện tại' });
  await dialog.getByRole('button', { name: /Globex Labs/ }).click();
  // Success closes the popover and navigates home — the app-level "fresh
  // start under the new org" signal, not left on org A's collection route.
  await expect(dialog).not.toBeVisible();
  await expect(page).toHaveURL(/\/$/);

  // Re-open the library: only org B's collection is offered — org A's two
  // collections ("Employee Handbook", "Product Specs") are gone entirely,
  // not merely unselected.
  await page.getByRole('link', { name: 'Thư viện' }).click();
  await expect(page.getByRole('link', { name: 'Globex Roadmap' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Employee Handbook' })).not.toBeVisible();
  await expect(page.getByRole('link', { name: 'Product Specs' })).not.toBeVisible();

  // Open org B's own collection: its own document renders, and nothing from
  // org A — right document title, and none of org A's document titles
  // anywhere on the page.
  await page.getByRole('link', { name: 'Globex Roadmap' }).click();
  const orgBTable = page.getByRole('table', { name: 'Danh sách tài liệu' });
  await expect(orgBTable.getByRole('button', { name: /Globex Master Plan\.pdf/ })).toBeVisible();
  await expect(page.getByText('Onboarding Guide.pdf')).not.toBeVisible();
  await expect(page.getByText('Leave Policy.docx')).not.toBeVisible();
  await expect(page.getByText('Roadmap.xlsx')).not.toBeVisible();

  // The switcher itself now shows Globex Labs as current, Acme Co as the
  // switch-back target — proof the scope, not just the visible list, moved.
  await page.getByRole('button', { name: 'Đơn vị hiện tại' }).click();
  await expect(dialog.getByRole('button', { name: /Globex Labs/ })).toHaveAttribute(
    'aria-current',
    'true',
  );
  await expect(dialog.getByRole('button', { name: /Acme Co/ })).toBeEnabled();
  await page.keyboard.press('Escape');

  // The admin members page (`handlers/members.ts`'s org-scoping gap, now
  // closed) also moved with the switch: org B's own roster renders — the
  // demo user's row plus its distinct second member — and neither of org A's
  // member ids (`secondMember`/`thirdMember`) appears anywhere.
  await page.getByRole('link', { name: 'Thành viên' }).click();
  await expect(page.getByRole('heading', { name: 'Thành viên và vai trò' })).toBeVisible();
  await expect(page.getByRole('row').filter({ hasText: IDS.demoUser })).toBeVisible();
  await expect(page.getByRole('row').filter({ hasText: IDS.globexMember })).toBeVisible();
  await expect(page.getByRole('row').filter({ hasText: IDS.secondMember })).toHaveCount(0);
  await expect(page.getByRole('row').filter({ hasText: IDS.thirdMember })).toHaveCount(0);
});

test('a denied switch (membership_missing) leaves org A active and shows an accessible error', async ({
  page,
}) => {
  await login(page);
  await openEmployeeHandbook(page);

  await page.getByRole('button', { name: 'Đơn vị hiện tại' }).click();
  const dialog = page.getByRole('dialog', { name: 'Đơn vị hiện tại' });

  // Force the very next `switchOrg` call to be denied, then attempt one.
  await forceStatus(page, 'switchOrg', 403, 1);
  await dialog.getByRole('button', { name: /Globex Labs/ }).click();

  await expect(page.getByRole('alert')).toBeVisible();
  // Still on org A: the popover stays open (no silent close on failure) and
  // org A's collection/data are still what's rendered underneath.
  await expect(dialog).toBeVisible();
  const table = page.getByRole('table', { name: 'Danh sách tài liệu' });
  await expect(table.getByRole('button', { name: /Onboarding Guide\.pdf/ })).toBeVisible();
});
