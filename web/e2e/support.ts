// Shared helpers + fixture constants for the P2-15 mock-based E2E suite.
//
// SCOPE OF THIS SUITE (read this first):
//   - This is the *mock-based* half of P2-15: a real Chromium driving the SPA
//     against the in-browser fetch mock (`VITE_MARKHAND_MOCK=1`). The
//     real-deployment E2E half (against Postgres/Qdrant/MinIO) is out of
//     scope here — see playwright.config.ts's own header.
//   - `ask -> citation` (`qa.spec.ts`, P2-10): the owner lowered P2-10's gate
//     2026-07-29 (`plans/markhand-web/backlog/phase-2/issues/README.md`) to
//     build on the OpenAPI contract + mock server, same as every other P2-0x
//     flow, rather than waiting for full R02/R03/R05 live-evidence. `QaPage`
//     is real now (search/ask/stream/citations/revoke/fallback) — see
//     `qa.spec.ts` for the covered scenarios, and `mocks/handlers/qa.ts`'s
//     module doc for how `/ask/stream` is mocked deterministically.
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
  /** The seeded "Employee Handbook" collection `openEmployeeHandbook` opens. */
  employeeHandbookCollection: '00000000-0000-4000-8000-00000000000a',
  /** Second org the demo user is also an active member of (`editor`) — org switch (P2-06/P2-15). */
  orgB: '00000000-0000-4000-8000-000000000003',
  /** Org B's own collection — seeded with content distinct from every org A fixture, so a switch has something visibly different to render. */
  orgBCollection: '00000000-0000-4000-8000-00000000000c',
  /** Org B's own second member (`editor`, active) on its admin members roster — distinct from every org A member id, so a post-switch members page reads as genuinely different data. */
  globexMember: '00000000-0000-4000-8000-000000000020',
} as const;

export const DEMO = {
  email: 'demo@markhand.test',
  password: 'demo-password',
} as const;

/**
 * Display names the admin members table actually renders (`MembersTable.tsx`
 * shows a name/email — never the raw `user_id` from `IDS` above, see the
 * owner-reported UI gap this closed). Row lookups in `admin.spec.ts` filter
 * by these, not by `IDS`, since a UUID no longer appears in that table's
 * text at all.
 */
export const NAMES = {
  demoUser: 'Demo User',
  secondMember: 'Bao Tran',
  thirdMember: 'Chi Vo',
  globexMember: 'Duc Nguyen',
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
    /** Advances a job already registered in the mock store past `pending` (src/components/upload/testSupport.ts). */
    __markhandMockJobs?: {
      succeed: (jobId: string) => void;
    };
    /** Advances a document's own `state` one step forward (src/components/library/testSupport.ts's `advanceDocumentState`). */
    __markhandMockDocs?: {
      advance: (documentId: string) => void;
    };
    /** Revokes `doc.upload` from the demo user (src/components/library/testSupport.ts's `revokeDocUpload`). */
    __markhandMockPermissions?: {
      revokeDocUpload: () => void;
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

/**
 * Marks `jobId` (a job the real `createUpload` handler already registered in
 * the mock store) `succeeded`, via the mock jobs control exposed on
 * `window`. Nothing in the mock ever advances a job's status past `pending`
 * on its own (see `components/upload/testSupport.ts`), so a job-lifecycle
 * test that needs to observe the "converted" stage calls this once it has
 * seen the "converting" one.
 */
export async function succeedUploadJob(page: Page, jobId: string): Promise<void> {
  await page.waitForFunction(() => window.__markhandMockJobs !== undefined);
  await page.evaluate((id) => window.__markhandMockJobs!.succeed(id), jobId);
}

/**
 * Advances `documentId`'s own server-side `state` one step forward
 * (uploaded->converting->converted->indexing->indexed) via the mock seam —
 * the P2-08 live-status-polling spec's stand-in for "the worker finished the
 * next pipeline stage", since nothing in the mock advances a document's
 * state on its own (see `components/library/testSupport.ts`'s own doc).
 */
export async function advanceDocument(page: Page, documentId: string): Promise<void> {
  await page.waitForFunction(() => window.__markhandMockDocs !== undefined);
  await page.evaluate((id) => window.__markhandMockDocs!.advance(id), documentId);
}

/**
 * Revokes `doc.upload` from the demo user in the mock store. Must be called
 * BEFORE `login()` submits the form — permissions are read once into
 * `AuthContext`'s session at `GET /auth/me` time, not live off the store, so
 * revoking after login would not retroactively hide anything gated on it.
 */
export async function revokeDocUpload(page: Page): Promise<void> {
  await page.waitForFunction(() => window.__markhandMockPermissions !== undefined);
  await page.evaluate(() => window.__markhandMockPermissions!.revokeDocUpload());
}

/** Opens the seeded "Employee Handbook" collection from the library's collection nav. */
export async function openEmployeeHandbook(page: Page): Promise<void> {
  await page.getByRole('link', { name: 'Thư viện' }).click();
  await page.getByRole('link', { name: 'Employee Handbook' }).click();
  // The upload panel only renders once a collection is open — a reliable
  // "collection is loaded" signal.
  await expect(page.getByLabel('Chọn tệp để tải lên')).toBeVisible();
}
