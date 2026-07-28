// Shared test-only plumbing for `components/admin/**` and the two admin
// pages' tests. Not imported by any production file — same convention as
// `components/actions/testSupport.ts`.
import { getStore } from '../../mocks/fixtures';

/**
 * Grants the seeded demo user `member.manage` in place (mutating the live
 * `MockUser` object `authContextForHeader` resolves requests against), so a
 * test can drive the admin members/usage pages as a permitted caller. The
 * demo user does NOT have this permission by default — see
 * `mocks/fixtures.ts`'s own note on `DEMO_USER` — because an existing
 * `App.test.tsx` scenario specifically covers a signed-in user *without*
 * member.manage on `/admin/members`; this function exists so P2-11/P2-12
 * tests don't have to change that shared default to get their own coverage.
 * Idempotent; call after `resetMockState()`.
 */
export function grantMemberManage(): void {
  const [user] = getStore().users;
  if (!user.permissions.includes('member.manage')) {
    user.permissions.push('member.manage');
  }
}
