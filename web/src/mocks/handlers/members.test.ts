// Org scoping for the admin members/invites/usage mock handlers (gap noted in
// `fixtures.ts`'s old `memberships`/`invites` doc comment: these previously
// stayed pinned to one fixed roster regardless of which org the caller's
// access token was actually scoped to — switching org never changed what
// `/members`, `/members/invites`, or `/usage` returned). Exercised through a
// real `ApiClient` against `installMockFetch()`, the same "no hand-stubbed
// handler" convention `AdminMembersPage.test.tsx`/`OrgSwitch.test.tsx` use —
// just at the handler level (no page/component in the way) so each operation
// in `handlers/members.ts` is checked directly.
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createApiClient, type ApiClient } from '../../api/client';
import { grantMemberManage } from '../../components/admin/testSupport';
import { installMockFetch, resetMockState, uninstallMockFetch } from '../index';
import { mockUuid } from '../ids';
import {
  getOrgInvites,
  getOrgMemberships,
  getStore,
  GLOBEX_MEMBER_USER_ID,
  mintTokenPair,
  ORG_A_ID,
  ORG_B_ID,
  SECOND_MEMBER_USER_ID,
  type MockUser,
} from '../fixtures';

/** A live, ready-to-use client whose bearer token is scoped to `orgId` — no login/refresh round trip needed since these tests only exercise `client.request`. */
function clientScopedTo(orgId: string): ApiClient {
  const client = createApiClient({ baseUrl: '' });
  const [user] = getStore().users;
  const { accessToken, refreshToken } = mintTokenPair(user, orgId);
  client.sessionManager.setTokens({
    accessToken,
    refreshToken,
    tokenType: 'Bearer',
    expiresIn: 3600,
    orgId,
    userId: user.userId,
  });
  return client;
}

beforeEach(() => {
  installMockFetch();
  resetMockState();
  grantMemberManage(); // same permissions array backs every org's profile — see fixtures.ts's DEMO_USER doc.
});

afterEach(() => {
  uninstallMockFetch();
});

describe('GET /members — org scoping', () => {
  it("returns each org's own roster, not a shared/fixed one", async () => {
    const orgAMembers = await clientScopedTo(ORG_A_ID).request('get', '/members');
    const orgBMembers = await clientScopedTo(ORG_B_ID).request('get', '/members');

    expect(orgAMembers.items.some((m) => m.userId === SECOND_MEMBER_USER_ID)).toBe(true);
    expect(orgBMembers.items.some((m) => m.userId === SECOND_MEMBER_USER_ID)).toBe(false);
    expect(orgBMembers.items.some((m) => m.userId === GLOBEX_MEMBER_USER_ID)).toBe(true);
    expect(orgAMembers.items.some((m) => m.userId === GLOBEX_MEMBER_USER_ID)).toBe(false);
  });
});

describe('GET /members/invites — org scoping', () => {
  it("returns each org's own invites", async () => {
    const orgAInvites = await clientScopedTo(ORG_A_ID).request('get', '/members/invites');
    const orgBInvites = await clientScopedTo(ORG_B_ID).request('get', '/members/invites');

    expect(orgAInvites.items.some((i) => i.email === 'moi-nguoi-moi@example.com')).toBe(true);
    expect(orgAInvites.items.some((i) => i.email === 'khach-moi-globex@example.com')).toBe(false);
    expect(orgBInvites.items.some((i) => i.email === 'khach-moi-globex@example.com')).toBe(true);
    expect(orgBInvites.items.some((i) => i.email === 'moi-nguoi-moi@example.com')).toBe(false);
  });
});

describe('POST /members/invites — org scoping', () => {
  it('creates the invite only in the caller org, invisible from the other org', async () => {
    await clientScopedTo(ORG_B_ID).request('post', '/members/invites', {
      body: { email: 'chi-moi-org-b@example.com', role: 'viewer' },
    });

    expect(getOrgInvites(ORG_B_ID).some((i) => i.email === 'chi-moi-org-b@example.com')).toBe(true);
    expect(getOrgInvites(ORG_A_ID).some((i) => i.email === 'chi-moi-org-b@example.com')).toBe(
      false,
    );
  });
});

describe('POST /members/invites/{inviteId}/revoke — org scoping', () => {
  it("404s revoking org A's invite id through an org B token", async () => {
    const [orgAInvite] = getOrgInvites(ORG_A_ID);
    await expect(
      clientScopedTo(ORG_B_ID).request('post', '/members/invites/{inviteId}/revoke', {
        params: { path: { inviteId: orgAInvite.id } },
      }),
    ).rejects.toMatchObject({ status: 404 });
    // Untouched: a 404 from the wrong org must not revoke it either.
    expect(getOrgInvites(ORG_A_ID).find((i) => i.id === orgAInvite.id)?.revokedAt).toBeNull();
  });

  it("revokes org B's own invite through an org B token", async () => {
    const [orgBInvite] = getOrgInvites(ORG_B_ID);
    await clientScopedTo(ORG_B_ID).request('post', '/members/invites/{inviteId}/revoke', {
      params: { path: { inviteId: orgBInvite.id } },
    });
    expect(getOrgInvites(ORG_B_ID).find((i) => i.id === orgBInvite.id)?.revokedAt).not.toBeNull();
  });
});

describe('PATCH /members/{userId} — org scoping', () => {
  it("404s patching org A's member id through an org B token", async () => {
    await expect(
      clientScopedTo(ORG_B_ID).request('patch', '/members/{userId}', {
        params: { path: { userId: SECOND_MEMBER_USER_ID } },
        body: { role: 'viewer' },
      }),
    ).rejects.toMatchObject({ status: 404 });
    // Org A's own copy of that member is untouched.
    expect(getOrgMemberships(ORG_A_ID).find((m) => m.userId === SECOND_MEMBER_USER_ID)?.role).toBe(
      'admin',
    );
  });

  it("patches org B's own member through an org B token", async () => {
    await clientScopedTo(ORG_B_ID).request('patch', '/members/{userId}', {
      params: { path: { userId: GLOBEX_MEMBER_USER_ID } },
      body: { role: 'viewer' },
    });
    expect(getOrgMemberships(ORG_B_ID).find((m) => m.userId === GLOBEX_MEMBER_USER_ID)?.role).toBe(
      'viewer',
    );
  });
});

describe('DELETE /members/{userId} — org scoping', () => {
  it("404s removing org A's member id through an org B token, leaving org A untouched", async () => {
    await expect(
      clientScopedTo(ORG_B_ID).request('delete', '/members/{userId}', {
        params: { path: { userId: SECOND_MEMBER_USER_ID } },
      }),
    ).rejects.toMatchObject({ status: 404 });
    expect(getOrgMemberships(ORG_A_ID).some((m) => m.userId === SECOND_MEMBER_USER_ID)).toBe(true);
  });
});

describe('GET /usage — org scoping', () => {
  it('returns different snapshots per org', async () => {
    const orgAUsage = await clientScopedTo(ORG_A_ID).request('get', '/usage');
    const orgBUsage = await clientScopedTo(ORG_B_ID).request('get', '/usage');

    const orgADocs = orgAUsage.items.find((i) => i.resource === 'documents');
    const orgBDocs = orgBUsage.items.find((i) => i.resource === 'documents');
    expect(orgADocs?.limit).toBe(5000);
    expect(orgBDocs?.limit).toBe(500);
    expect(orgADocs).not.toEqual(orgBDocs);
  });
});

describe('POST /members/invites/accept — cross-org lookup', () => {
  it("adds the membership to the invite's own org, not the caller's current-token org", async () => {
    const inviteResponse = await clientScopedTo(ORG_A_ID).request('post', '/members/invites', {
      body: { email: 'nguoi-duoc-moi-b@example.com', role: 'viewer' },
    });
    // Move it to org B's list to simulate "invited into org B" without a
    // second create endpoint per org — same effect, less duplication.
    const orgAInvites = getOrgInvites(ORG_A_ID);
    const idx = orgAInvites.findIndex((i) => i.id === inviteResponse.invite.id);
    const [moved] = orgAInvites.splice(idx, 1);
    getOrgInvites(ORG_B_ID).push(moved);

    // A brand-new user, currently a member of org A only (not yet seeded on
    // `store.users` at all — `mintTokenPair`/`authContextForHeader` both
    // resolve a token back to a real entry there), accepts an org B invite.
    // A real `mockUuid` id (not an arbitrary string): the response is checked
    // against the spec schema, which requires `Membership.userId` to be a uuid.
    const newUserId = mockUuid(33);
    const newUser: MockUser = {
      userId: newUserId,
      orgId: ORG_A_ID,
      email: 'nguoi-duoc-moi-b@example.com',
      password: 'unused',
      displayName: 'Người được mời B',
      permissions: [],
      allowedCollectionIds: [],
    };
    getStore().users.push(newUser);
    const acceptingClient = createApiClient({ baseUrl: '' });
    const { accessToken, refreshToken } = mintTokenPair(newUser, ORG_A_ID);
    acceptingClient.sessionManager.setTokens({
      accessToken,
      refreshToken,
      tokenType: 'Bearer',
      expiresIn: 3600,
      orgId: ORG_A_ID,
      userId: newUserId,
    });

    const membership = await acceptingClient.request('post', '/members/invites/accept', {
      body: { token: inviteResponse.token },
    });

    expect(membership.userId).toBe(newUserId);
    expect(getOrgMemberships(ORG_B_ID).some((m) => m.userId === newUserId)).toBe(true);
    expect(getOrgMemberships(ORG_A_ID).some((m) => m.userId === newUserId)).toBe(false);
  });
});
