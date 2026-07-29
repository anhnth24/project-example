// Shared test-only plumbing for `components/library/**`'s P2-18 coverage.
// Not imported by any production file — same convention as
// `components/admin/testSupport.ts`'s `grantMemberManage()`.
import { getStore } from '../../mocks/fixtures';

/**
 * Grants the seeded demo user `doc.upload` in place, so a test can drive
 * `ProjectsPanel` (create project / assign collection) as a permitted
 * caller. The demo user does not have this permission by default — see
 * `mocks/fixtures.ts`'s own note on `DEMO_USER` — so this exists rather than
 * changing that shared default. Idempotent; call after `resetMockState()`.
 */
export function grantDocUpload(): void {
  const [user] = getStore().users;
  if (!user.permissions.includes('doc.upload')) {
    user.permissions.push('doc.upload');
  }
}
