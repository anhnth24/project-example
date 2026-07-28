// Shared test-only plumbing for `components/shell/**` (org switch) tests. Not
// imported by any production file — same convention as
// `components/admin/testSupport.ts`.
import { getStore, DEMO_USER_ID, ORG_A_ID } from '../../mocks/fixtures';
import { mockUuid, mockTimestamp } from '../../mocks/ids';

/** A third org the demo user is also an active member of — role `viewer`, no seeded collection of its own. Exists purely so a test can drive a race between switching to two *different* target orgs (the `OrgSwitch` component only ever has one alternate org to click against without this). Idempotent; call after `resetMockState()`. */
export function seedThirdOrgMembership(): { orgId: string; orgName: string } {
  const orgId = mockUuid(4);
  const store = getStore();
  if (!store.orgs.some((o) => o.id === orgId)) {
    store.orgs.push({ id: orgId, slug: 'initech', name: 'Initech', createdAt: mockTimestamp(0) });
  }
  if (!store.orgMemberships.some((m) => m.userId === DEMO_USER_ID && m.orgId === orgId)) {
    store.orgMemberships.push({ userId: DEMO_USER_ID, orgId, role: 'viewer', state: 'active' });
  }
  if (!store.orgProfiles.has(orgId)) {
    const orgAProfile = store.orgProfiles.get(ORG_A_ID);
    store.orgProfiles.set(orgId, {
      permissions: orgAProfile?.permissions ?? [],
      allowedCollectionIds: [],
    });
  }
  return { orgId, orgName: 'Initech' };
}
