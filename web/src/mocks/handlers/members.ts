// P2-11/P2-12: member/role admin + usage mock handlers. Mirrors the real
// domain rules in `crates/server/src/services/members.rs` and
// `crates/server/src/routes/members.rs` closely enough for the SPA's tests to
// exercise the same decisions the real server makes — in particular:
//
//   - Org scoping (P2-06/P2-15 org switch): every operation below (except
//     `acceptMemberInvite`, see its own note) reads/writes through
//     `getOrgMemberships(auth.orgId)`/`getOrgInvites(auth.orgId)`/
//     `getOrgUsage(auth.orgId)` — `auth.orgId` from `authContextForHeader(...)`,
//     i.e. whichever org the caller's *current* access token is scoped to, not
//     any fixed org. Same pattern `handlers/library.ts`'s `listCollections`
//     already established for collections; this file previously stayed pinned
//     to one org's roster regardless of `switchOrg` (a known gap, now closed).
//   - `member.manage` gates every operation here except `acceptMemberInvite`
//     (auth-only by design, matching the real route).
//   - "admin không quản owner": granting the owner role, or mutating a
//     membership that is *currently* owner (role change / suspend / reactivate
//     / remove), requires the caller to themselves be an active owner —
//     `guardOwnerTier` below, mirroring `services::members::guard_owner_tier`.
//     Both this and a plain missing-`member.manage` 403 map to the same
//     generic `forbidden` code/message the real server sends (see
//     `RouteError::into_response`'s `Self::Denied` arm) — this mock does not
//     invent a more specific code the real API doesn't have.
//   - The last-owner invariant (`guardLastOwner`) mirrors
//     `check_last_owner_invariant`: an operation that would leave zero active
//     owners in the org fails with 409 `last_owner`.
//   - Invite lifecycle codes (`already_member`, `invite_terminal`,
//     `invite_expired`) mirror `RouteError::from_member`'s exact `code` strings
//     so the SPA's error-code mapping is tested against the real vocabulary.
import { registerOperation } from '../registry';
import { apiError, forbidden, notFound } from '../apiError';
import { mockTimestamp } from '../ids';
import {
  authContextForHeader,
  findInviteAcrossOrgs,
  getOrgInvites,
  getOrgMemberships,
  getOrgUsage,
  nextId,
  type InviteRecord,
  type MockUser,
} from '../fixtures';
import type { components } from '../../api/generated/contract';

type Membership = components['schemas']['Membership'];
type MembershipRole = Membership['role'];
type Invite = components['schemas']['Invite'];
type PatchMemberRequest = components['schemas']['PatchMemberRequest'];
type CreateInviteRequest = components['schemas']['CreateInviteRequest'];
type AcceptInviteRequest = components['schemas']['AcceptInviteRequest'];

const PERMISSION_MEMBER_MANAGE = 'member.manage';
const MIN_INVITE_TTL_SECS = 60;
const MAX_INVITE_TTL_SECS = 30 * 24 * 3600;
const DEFAULT_INVITE_TTL_SECS = 7 * 24 * 3600;

/** Same generic message the real route sends for every 403 here (`RouteError::into_response`'s `Self::Denied` arm: `"Permission denied"`), regardless of *why* — see this file's module doc. */
function permissionDenied() {
  return forbidden('Permission denied');
}

/** Resolves the caller's `MockUser` + their own membership row in `orgId`, or `undefined` if unauthenticated/not on that org's roster. `fetchMock.ts` already 401s a missing/invalid bearer before any handler runs, so `undefined` here means "authenticated but not in `getOrgMemberships(orgId)`" — treated as no `member.manage`, fail-closed. */
function callerMembership(
  authorizationHeader: string | null,
  orgId: string,
): Membership | undefined {
  const auth = authContextForHeader(authorizationHeader);
  if (!auth) return undefined;
  return getOrgMemberships(orgId).find((m) => m.userId === auth.user.userId);
}

/** Resolves the caller if authenticated AND holding `member.manage`, `undefined` otherwise — one call gets both the permission check and the `orgId` every operation below scopes its store access to. */
function authWithMemberManage(
  authorizationHeader: string | null,
): { user: MockUser; sessionId: string; orgId: string } | undefined {
  const auth = authContextForHeader(authorizationHeader);
  if (!auth || !auth.user.permissions.includes(PERMISSION_MEMBER_MANAGE)) return undefined;
  return auth;
}

/** Mirrors `services::members::operation_manages_owner`. */
function operationManagesOwner(targetCurrentRole: MembershipRole, grantsOwner: boolean): boolean {
  return grantsOwner || targetCurrentRole === 'owner';
}

/** Mirrors `services::members::guard_owner_tier`: true (allowed) unless the operation touches the owner tier and the caller isn't an active owner in `orgId`. */
function guardOwnerTier(
  authorizationHeader: string | null,
  orgId: string,
  targetCurrentRole: MembershipRole,
  grantsOwner: boolean,
): boolean {
  if (!operationManagesOwner(targetCurrentRole, grantsOwner)) return true;
  const caller = callerMembership(authorizationHeader, orgId);
  return !!caller && caller.role === 'owner' && caller.state === 'active';
}

/** Mirrors `services::members::check_last_owner_invariant`, scoped to `orgId`'s own roster. */
function guardLastOwner(
  orgId: string,
  targetUserId: string,
  targetWillBeActiveOwner: boolean,
): boolean {
  const othersActive = getOrgMemberships(orgId).filter(
    (m) => m.userId !== targetUserId && m.role === 'owner' && m.state === 'active',
  ).length;
  const remaining = othersActive + (targetWillBeActiveOwner ? 1 : 0);
  return remaining > 0;
}

function lastOwnerConflict() {
  return {
    status: 409,
    body: apiError('last_owner', 'Operation would leave the organization with zero active owners'),
  };
}

function inviteStatus(invite: InviteRecord, now: number): Invite['status'] {
  if (invite.revokedAt) return 'revoked';
  if (invite.acceptedAt) return 'accepted';
  if (Date.parse(invite.expiresAt) <= now) return 'expired';
  return 'pending';
}

function inviteDto(invite: InviteRecord): Invite {
  return {
    id: invite.id,
    email: invite.email,
    role: invite.role,
    status: inviteStatus(invite, Date.now()),
    expiresAt: invite.expiresAt,
    acceptedAt: invite.acceptedAt,
    revokedAt: invite.revokedAt,
    createdAt: invite.createdAt,
  };
}

// ---------------------------------------------------------------------------
// E1 — GET /members
// ---------------------------------------------------------------------------

registerOperation('listMembers', (ctx) => {
  const auth = authWithMemberManage(ctx.headers.get('authorization'));
  if (!auth) return permissionDenied();
  return {
    status: 200,
    body: { items: getOrgMemberships(auth.orgId), page: { hasMore: false, nextCursor: null } },
  };
});

// ---------------------------------------------------------------------------
// E2 — GET /members/invites
// ---------------------------------------------------------------------------

registerOperation('listMemberInvites', (ctx) => {
  const auth = authWithMemberManage(ctx.headers.get('authorization'));
  if (!auth) return permissionDenied();
  return {
    status: 200,
    body: {
      items: getOrgInvites(auth.orgId).map(inviteDto),
      page: { hasMore: false, nextCursor: null },
    },
  };
});

// ---------------------------------------------------------------------------
// E3 — POST /members/invites
// ---------------------------------------------------------------------------

registerOperation('createMemberInvite', async (ctx) => {
  const authorization = ctx.headers.get('authorization');
  const auth = authWithMemberManage(authorization);
  if (!auth) return permissionDenied();
  const body = await ctx.json<CreateInviteRequest>();

  const email = body.email.trim();
  if (!email || !email.includes('@')) {
    return { status: 400, body: apiError('validation_failed', 'Invalid email') };
  }
  if (
    body.ttlSecs !== undefined &&
    (body.ttlSecs < MIN_INVITE_TTL_SECS || body.ttlSecs > MAX_INVITE_TTL_SECS)
  ) {
    return { status: 400, body: apiError('validation_failed', 'Invalid ttlSecs') };
  }

  // Inviting a new owner is itself owner-tier: mirrors
  // `create_invite`'s `OwnerRequiredForOwnerInvite` check (there is no
  // "current role" for an invite target, so this only ever checks the
  // grants-owner half of `guardOwnerTier`).
  if (body.role === 'owner' && !guardOwnerTier(authorization, auth.orgId, 'viewer', true)) {
    return permissionDenied();
  }

  const ttlSecs = body.ttlSecs ?? DEFAULT_INVITE_TTL_SECS;
  const token = `mock-invite-token.${nextId()}`;
  const record: InviteRecord = {
    id: nextId(),
    email: email.toLowerCase(),
    role: body.role,
    tokenHash: `hash-of-${token}`,
    expiresAt: new Date(Date.now() + ttlSecs * 1000).toISOString(),
    acceptedAt: null,
    revokedAt: null,
    createdAt: mockTimestamp(0),
  };
  getOrgInvites(auth.orgId).push(record);

  return {
    status: 201,
    body: { invite: inviteDto(record), token },
  };
});

// ---------------------------------------------------------------------------
// E4 — POST /members/invites/{inviteId}/revoke
// ---------------------------------------------------------------------------

registerOperation('revokeMemberInvite', (ctx) => {
  const auth = authWithMemberManage(ctx.headers.get('authorization'));
  if (!auth) return permissionDenied();
  const invite = getOrgInvites(auth.orgId).find((i) => i.id === ctx.params.inviteId);
  if (!invite) return notFound(`Invite ${ctx.params.inviteId} does not exist.`);
  if (invite.acceptedAt || invite.revokedAt) {
    return {
      status: 409,
      body: apiError('invite_terminal', 'Invite has already been accepted or revoked'),
    };
  }
  invite.revokedAt = mockTimestamp(0);
  return { status: 200, body: inviteDto(invite) };
});

// ---------------------------------------------------------------------------
// E5 — POST /members/invites/accept (auth-only — see module doc; the admin
// pages this task builds never call this, it's the invitee's own flow, but
// it's mocked for completeness/future use).
//
// Deliberately NOT scoped by `auth.orgId` like every other operation above:
// the invite being accepted belongs to whichever org issued it, which is not
// necessarily the org the accepting caller's *current* access token is scoped
// to (they may not even be a member of it yet — that is the whole point of an
// invite), so `findInviteAcrossOrgs` searches every org's invite list by
// token hash instead.
// ---------------------------------------------------------------------------

registerOperation('acceptMemberInvite', async (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return { status: 401, body: apiError('unauthorized', 'Authentication required') };
  const body = await ctx.json<AcceptInviteRequest>();
  const token = body.token.trim();
  const found = findInviteAcrossOrgs((i) => `hash-of-${token}` === i.tokenHash);
  if (!found) return notFound('Invite token is unknown.');
  const { orgId, invite } = found;
  if (invite.acceptedAt || invite.revokedAt) {
    return {
      status: 409,
      body: apiError('invite_terminal', 'Invite has already been accepted or revoked'),
    };
  }
  if (Date.parse(invite.expiresAt) <= Date.now()) {
    return { status: 409, body: apiError('invite_expired', 'Invite has expired') };
  }
  if (getOrgMemberships(orgId).some((m) => m.userId === auth.user.userId)) {
    return {
      status: 409,
      body: apiError('already_member', 'User is already a member of this organization'),
    };
  }
  const membership: Membership = {
    userId: auth.user.userId,
    role: invite.role,
    state: 'active',
    createdAt: mockTimestamp(0),
  };
  getOrgMemberships(orgId).push(membership);
  invite.acceptedAt = mockTimestamp(0);
  return { status: 201, body: membership };
});

// ---------------------------------------------------------------------------
// E6 — PATCH /members/{userId}
// ---------------------------------------------------------------------------

registerOperation('patchMember', async (ctx) => {
  const authorization = ctx.headers.get('authorization');
  const auth = authWithMemberManage(authorization);
  if (!auth) return permissionDenied();
  const body = await ctx.json<PatchMemberRequest>();
  if (body.role === undefined && body.state === undefined) {
    return { status: 400, body: apiError('validation_failed', 'role or state is required') };
  }

  const userId = ctx.params.userId;
  const membership = getOrgMemberships(auth.orgId).find((m) => m.userId === userId);
  if (!membership) return notFound(`Member ${userId} does not exist.`);

  // Two separate owner-tier gates, not one shared at the top: mirrors
  // `patch_member` calling `change_role` (owner-tier check against the
  // *pre*-change role) and then `suspend_member`/`reactivate_member` (a
  // fresh check against whatever role is current *after* that role change
  // already committed) as two sequential steps — so `{role: "viewer", state:
  // "suspended"}` against a currently-owner target checks owner-tier once
  // for the demotion and, since the target is no longer an owner afterwards,
  // does not require owner-tier again for the suspend.
  if (body.role !== undefined) {
    if (!guardOwnerTier(authorization, auth.orgId, membership.role, body.role === 'owner')) {
      return permissionDenied();
    }
    if (body.role !== membership.role) {
      const willBeActiveOwner = body.role === 'owner' && membership.state === 'active';
      if (!guardLastOwner(auth.orgId, userId, willBeActiveOwner)) return lastOwnerConflict();
      membership.role = body.role;
    }
  }

  if (body.state !== undefined) {
    if (!guardOwnerTier(authorization, auth.orgId, membership.role, false)) {
      return permissionDenied();
    }
    if (body.state !== membership.state) {
      if (body.state === 'suspended' && !guardLastOwner(auth.orgId, userId, false)) {
        return lastOwnerConflict();
      }
      membership.state = body.state;
    }
  }

  return { status: 200, body: membership };
});

// ---------------------------------------------------------------------------
// E7 — DELETE /members/{userId}
// ---------------------------------------------------------------------------

registerOperation('deleteMember', (ctx) => {
  const authorization = ctx.headers.get('authorization');
  const auth = authWithMemberManage(authorization);
  if (!auth) return permissionDenied();

  const userId = ctx.params.userId;
  const roster = getOrgMemberships(auth.orgId);
  const idx = roster.findIndex((m) => m.userId === userId);
  if (idx === -1) return notFound(`Member ${userId} does not exist.`);
  const membership = roster[idx];

  if (!guardOwnerTier(authorization, auth.orgId, membership.role, false)) return permissionDenied();
  if (!guardLastOwner(auth.orgId, userId, false)) return lastOwnerConflict();

  roster.splice(idx, 1);
  return { status: 204 };
});

// ---------------------------------------------------------------------------
// E8 — GET /usage
// ---------------------------------------------------------------------------

registerOperation('getUsage', (ctx) => {
  const auth = authWithMemberManage(ctx.headers.get('authorization'));
  if (!auth) return permissionDenied();
  return { status: 200, body: { items: getOrgUsage(auth.orgId) } };
});
