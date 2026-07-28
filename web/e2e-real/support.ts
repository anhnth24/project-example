// Shared helpers + fixture constants for the real-deployment half of P2-15.
//
// SCOPE OF THIS SUITE (read this first):
//   - This drives a real Chromium against a *real* fileconv-server — the
//     compose dev stack (Postgres/Qdrant/MinIO/embedding) plus a built
//     `web/dist` served by the server itself (see `deploy/README.md`'s "Web
//     SPA static serving" section and `crates/server/src/spa.rs`). There is
//     no fetch mock here: every request is a real HTTP round-trip.
//   - Only `deploy/scripts/web-e2e-real.sh` is meant to run this suite: it
//     brings the stack up, migrates, seeds, builds the SPA, starts the
//     server, and only then runs `playwright test --project=real`. Running
//     this suite any other way requires reproducing all of that by hand.
//   - Smoke scope only, deliberately small: login against a seeded real
//     account, then confirm the library shell renders with its seeded
//     collection. Broader flows (upload → indexed, actions, org switch)
//     belong to a later pass once this harness is proven — see P2-15's
//     status note.
//
// FIXTURE GROUND TRUTH: the admin account and its password come from the
// same seed the dev stack always uses — `crates/server/migrations/
// 0011_expand_poc_seed.sql` (account + org + collection) and
// `deploy/scripts/seed-dev-password.sh` (sets the password hash). The
// collection name asserted below ("POC Library") is that migration's seeded
// row, not an invented value. Override via env only if a caller changes
// `MARKHAND_DEV_PASSWORD` from its default.
import { expect, type Page } from '@playwright/test';

export const REAL_ADMIN = {
  email: process.env.MARKHAND_E2E_ADMIN_EMAIL ?? 'admin@poc.example',
  password: process.env.MARKHAND_E2E_ADMIN_PASSWORD ?? 'markhand-dev',
} as const;

/** Seeded collection name (migration 0011_expand_poc_seed.sql). */
export const SEEDED_COLLECTION_NAME = 'POC Library';

/**
 * Logs in with the seeded admin account via the real `/login` form and
 * waits until the in-app shell (the "Thư viện" rail link) is mounted.
 */
export async function login(page: Page): Promise<void> {
  await page.goto('/login');
  await page.getByLabel('Email').fill(REAL_ADMIN.email);
  await page.getByLabel('Mật khẩu').fill(REAL_ADMIN.password);
  await page.getByRole('button', { name: 'Đăng nhập' }).click();
  await expect(page.getByRole('link', { name: 'Thư viện' })).toBeVisible();
  await expect(page).not.toHaveURL(/\/login/);
}
