// Org scope safety (P2-15 flow 4, P2-06/P2-15 org switch un-deferred by 1C-01).
//
// The old version of this spec documented an honest gap: the generated
// contract had no org-list/org-switch endpoint, so the rail's org control
// could only show the single bound org and state that switching wasn't
// wired. 1C-01 (`GET /orgs`, `GET /orgs/{orgId}`, `POST /orgs/switch`) closed
// that gap — this spec now covers the switcher's own popover mechanics
// (listing every org the caller belongs to, marking the current one,
// a11y). The full cross-org "switch, and the previous org's data is
// completely gone" acceptance flow — the actual "no old-org render" gate —
// lives in `org-switch.spec.ts`, not duplicated here.
import { expect, test } from '@playwright/test';
import { login } from './support';

test('the org switcher lists every org the caller belongs to and marks the current one', async ({
  page,
}) => {
  await login(page);

  await page.getByRole('button', { name: 'Đơn vị hiện tại' }).click();
  const dialog = page.getByRole('dialog', { name: 'Đơn vị hiện tại' });
  await expect(dialog).toBeVisible();

  // The demo user is seeded as an active member of both org A ("Acme Co",
  // the org the session boots into) and org B ("Globex Labs").
  const currentOption = dialog.getByRole('button', { name: /Acme Co/ });
  await expect(currentOption).toBeVisible();
  await expect(currentOption).toHaveAttribute('aria-current', 'true');
  await expect(currentOption).toBeDisabled();

  const otherOption = dialog.getByRole('button', { name: /Globex Labs/ });
  await expect(otherOption).toBeVisible();
  await expect(otherOption).toBeEnabled();
  await expect(otherOption).not.toHaveAttribute('aria-current');
});

test('Escape closes the org popover and returns focus to its trigger', async ({ page }) => {
  await login(page);

  const trigger = page.getByRole('button', { name: 'Đơn vị hiện tại' });
  await trigger.click();
  await expect(page.getByRole('dialog', { name: 'Đơn vị hiện tại' })).toBeVisible();

  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog', { name: 'Đơn vị hiện tại' })).not.toBeVisible();
  await expect(trigger).toBeFocused();
});
