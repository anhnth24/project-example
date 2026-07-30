// Shared test-only plumbing for `components/library/**`'s P2-18/P2-08
// coverage. Not imported by any production file — same convention as
// `components/admin/testSupport.ts`'s `grantMemberManage()`.
import { getStore } from '../../mocks/fixtures';
import type { DocumentState } from './types';

/**
 * Grants the seeded demo user `doc.upload` in place, so a test can drive
 * `AdminProjectsPage`/the "Dự án" rail item (create project / assign
 * collection) as a permitted caller. The demo user does not have this
 * permission by default — see `mocks/fixtures.ts`'s own note on `DEMO_USER`
 * — so this exists rather than changing that shared default. Idempotent;
 * call after `resetMockState()`.
 */
export function grantDocUpload(): void {
  const [user] = getStore().users;
  if (!user.permissions.includes('doc.upload')) {
    user.permissions.push('doc.upload');
  }
}

/**
 * The inverse of `grantDocUpload()` — removes it if present. E2E's rail
 * "guard quyền" spec needs a signed-in caller who genuinely lacks
 * `doc.upload` (the E2E bootstrap, `mocks/browser.ts`'s
 * `installBrowserMocks()`, grants it to the demo user by default so every
 * *other* P2-18 flow is reachable — see that file's own doc), so this must
 * be called before login's `GET /auth/me` is issued (i.e. before submitting
 * the login form), not after: permissions are read once into `AuthContext`'s
 * session at login/`refreshSession()` time, not live off the store.
 *
 * Mutates `user.permissions` IN PLACE (`.splice`, never a reassigning
 * `.filter()`) — same reason `grantDocUpload()`'s `.push()` above does:
 * `seedOrgProfiles()` (`mocks/fixtures.ts`) seeds each org's `OrgProfile`
 * with a direct reference to this same array (`permissions:
 * DEMO_USER.permissions`), and `/auth/me` reads permissions off that
 * `OrgProfile`, not off `user.permissions` directly — reassigning here would
 * silently stop mutating the array `/auth/me` actually reads, and the
 * "revoke" would never take effect.
 */
export function revokeDocUpload(): void {
  const [user] = getStore().users;
  const index = user.permissions.indexOf('doc.upload');
  if (index !== -1) user.permissions.splice(index, 1);
}

/**
 * P2-08 gap-close mock seam ("trạng thái document chưa đúng giai đoạn xử lý
 * khi load lại trang hoặc mở chức năng khác rồi quay lại"): nothing in the
 * mock ever advances a document's own `state` past `uploaded`/`converting`
 * on its own — the same "no lifecycle progression" gap
 * `components/upload/testSupport.ts`'s `succeedJob` closes for *job*
 * status — so this exists to let a test/demo nudge the document itself
 * through the forward path one step at a time, exactly like that file's own
 * seam does for jobs. Convention: `__markhandMock*` on `window`
 * (`mocks/browser.ts`). No-op if `documentId` isn't a document the store
 * actually has, or if it's already at a state this seam doesn't advance
 * further (terminal `indexed`/`failed`, or a soft-delete state) — advancing
 * *into* `failed` is a distinct real-world outcome this forward-only seam
 * does not fabricate.
 */
const NEXT_DOCUMENT_STATE: Partial<Record<DocumentState, DocumentState>> = {
  uploaded: 'converting',
  converting: 'converted',
  converted: 'indexing',
  indexing: 'indexed',
};

export function advanceDocumentState(documentId: string): void {
  for (const docs of getStore().documents.values()) {
    const document = docs.find((d) => d.id === documentId);
    if (!document) continue;
    const next = NEXT_DOCUMENT_STATE[document.state];
    if (next) {
      document.state = next;
      document.updatedAt = new Date().toISOString();
    }
    return;
  }
}
