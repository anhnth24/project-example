// Shared helpers + fixture constants for the P2-15 mock-based E2E suite.
//
// SCOPE OF THIS SUITE (read this first):
//   - This is the *mock-based* half of P2-15: a real Chromium driving the SPA
//     against the in-browser fetch mock (`VITE_MARKHAND_MOCK=1`). The
//     real-deployment E2E half (against Postgres/Qdrant/MinIO) is out of
//     scope here — see playwright.config.ts's own header.
//   - `ask -> citation` is deliberately ABSENT. The Q&A page (`QaPage.tsx`) is
//     a placeholder — "Chưa kết nối tới API hỏi đáp." — because P2-10 is
//     blocked on R02/R03/R05. There is no Q&A UI to drive, so this suite does
//     NOT contain an ask/citation spec (a skipped-but-named test would falsely
//     imply the flow exists). Its absence is intentional and noted here.
//
// FIXTURE GROUND TRUTH (src/mocks/fixtures.ts): every assertion below is
// against these seeded values, not invented ones. ids come from `mockUuid(n)`
// (src/mocks/ids.ts): a fixed uuid whose last 12 hex digits are `n`.
import { expect, type Page } from '@playwright/test';

/** Deterministic seeded ids (`mockUuid(n)` — last 12 hex digits are `n` in hex). */
export const IDS = {
  /** Demo user / signed-in caller — seeded `owner`, `active`. */
  demoUser: '00000000-0000-4000-8000-000000000001',
  /** The demo org the whole session is bound to. */
  org: '00000000-0000-4000-8000-000000000002',
  /** Second seeded member — `admin`, `active` (a non-owner promotion/demotion target). */
  secondMember: '00000000-0000-4000-8000-00000000001e',
  /** Third seeded member — `viewer`, `suspended` (the non-active row). */
  thirdMember: '00000000-0000-4000-8000-00000000001f',
} as const;

export const DEMO = {
  email: 'demo@markhand.test',
  password: 'demo-password',
} as const;

/** Statuses `mockControl.forceStatus` accepts (src/mocks/control.ts). */
type ForcedStatus = 401 | 403 | 404 | 409 | 429 | 503;

declare global {
  interface Window {
    __markhandMockReset?: () => void;
    __markhandMockControl?: {
      forceStatus: (
        operationId: string,
        kind: ForcedStatus,
        opts?: { times?: number; quota?: unknown },
      ) => void;
      clear: (operationId: string) => void;
      reset: () => void;
    };
  }
}

/**
 * Logs in with the demo user via the real /login form and waits until the
 * in-app shell (the "Thư viện" rail link) is mounted. Every test starts here:
 * `page.goto('/login')` is a full load, which re-runs the mock bootstrap and
 * re-seeds the store, so each test gets isolated fixtures. In-app navigation
 * afterwards must go through rail links / route links (NOT `page.goto`): a
 * full reload re-seeds the store and drops the seeded refresh token, which
 * logs the session out.
 */
export async function login(page: Page): Promise<void> {
  await page.goto('/login');
  await page.getByLabel('Email').fill(DEMO.email);
  await page.getByLabel('Mật khẩu').fill(DEMO.password);
  await page.getByRole('button', { name: 'Đăng nhập' }).click();
  await expect(page.getByRole('link', { name: 'Thư viện' })).toBeVisible();
  await expect(page).not.toHaveURL(/\/login/);
}

/**
 * Forces the next `times` call(s) of `operationId` to answer `status`, via the
 * mock control exposed on `window`. Set this BEFORE the request it targets
 * fires (e.g. before `setInputFiles` for an upload, before confirming a
 * delete).
 */
export async function forceStatus(
  page: Page,
  operationId: string,
  status: ForcedStatus,
  times = 1,
): Promise<void> {
  // The mock control is installed by `main.tsx`'s async bootstrap (a dynamic
  // `import('./mocks/browser')` awaited before render). A test that calls this
  // immediately after `goto` — before any UI it would otherwise wait on has
  // rendered — can race that install; on a fast CI run `__markhandMockControl`
  // was still undefined here (flaky `forceStatus` TypeError). Wait for it so
  // the helper is deterministic regardless of when it's called.
  await page.waitForFunction(() => window.__markhandMockControl !== undefined);
  await page.evaluate(
    ([op, kind, n]) => {
      window.__markhandMockControl!.forceStatus(op as string, kind as ForcedStatus, {
        times: n as number,
      });
    },
    [operationId, status, times] as const,
  );
}

/** Opens the seeded "Employee Handbook" collection from the library's collection nav. */
export async function openEmployeeHandbook(page: Page): Promise<void> {
  await page.getByRole('link', { name: 'Thư viện' }).click();
  await page.getByRole('link', { name: 'Employee Handbook' }).click();
  // The upload panel only renders once a collection is open — a reliable
  // "collection is loaded" signal.
  await expect(page.getByLabel('Chọn tệp để tải lên')).toBeVisible();
}
