// Org scope safety (P2-15 flow 4, P2-06).
//
// HONEST GAP — the cross-org "switch org, assert the previous org's documents
// are gone" flow CANNOT be driven through the UI as it exists today, and this
// spec does not fake it. Reason, from the app itself (OrgSwitch.tsx's module
// doc): the session's `Scope` carries exactly one `orgId`, and the generated
// contract (api/generated/contract.ts) exposes no org-list or org-switch
// endpoint — only /auth/{login,logout,me,refresh}. There is literally nothing
// to switch *to*, so the rail's "org switch" control deliberately shows the
// current org identity and states that switching isn't wired yet, rather than
// fabricating a second org.
//
// The scope-safety machinery that flow 4 is meant to exercise (the epoch guard
// in useScopeSafeRequest / state/scope.ts that discards a previous org's
// in-flight responses after a switch) is real and unit-tested at the module
// level, but has no UI affordance to trigger a second org here. So this spec
// verifies the switcher's actual, honest behavior; the cross-tenant assertion
// is reported as an untestable gap for this mock-based suite.
import { expect, test } from '@playwright/test';
import { IDS, login } from './support';

test('the org switcher shows the single bound org and states no cross-org switch is wired', async ({
  page,
}) => {
  await login(page);

  await page.getByRole('button', { name: 'Đơn vị hiện tại' }).click();

  const dialog = page.getByRole('dialog', { name: 'Đơn vị hiện tại' });
  await expect(dialog).toBeVisible();
  // Real, current scope identity — the seeded org id, straight from useScope().
  await expect(dialog).toContainText(`org ${IDS.org}`);
  // …and the honest statement that switching orgs isn't available.
  await expect(dialog).toContainText('chưa có API để liệt kê hoặc chuyển sang đơn vị khác');
});
