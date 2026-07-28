// 1C-01 org lifecycle mocks: `GET /orgs`, `GET /orgs/{orgId}`, `POST
// /orgs/switch`. Mirrors `crates/server/src/routes/orgs.rs` /
// `crates/server/src/services/orgs.rs` closely enough for the SPA's org
// switcher and its tests to exercise the same decisions the real server
// makes:
//
//   - Auth-only (a valid bearer access token), never gated by the token's
//     *own* org claim — same "auth-only" trust level `acceptMemberInvite`
//     already uses, and the whole reason these three routes exist: a caller
//     scoped to org A must be able to discover/switch to org B.
//   - `listOrgs`/`getOrg` only ever surface orgs the caller is a CURRENT
//     active member of (`OrgMembershipRecord.state === 'active'`) — a
//     revoked/suspended membership silently disappears from the list rather
//     than 403ing, matching `list_user_orgs`/`get_org_detail` folding every
//     denial into "not present" server-side.
//   - `getOrg` answers 404 (not 403) for both "no such org" and "caller not
//     a member" — no existence oracle for non-members, per the real route's
//     own doc comment.
//   - `switchOrg` re-checks membership against `orgMemberships` (the mock's
//     stand-in for a fresh PostgreSQL read) and denies with the real
//     server's exact code, `membership_missing` (see
//     `crates/server/src/auth/middleware.rs` / `services/audit.rs`), not the
//     mock's generic `forbidden()` helper — this is the one status/code this
//     file deliberately does NOT reuse `apiError.ts`'s shared `forbidden()`
//     for. On success it mints an independent token pair scoped to the
//     target org via `mintTokenPair(user, orgId)`; the caller's original org
//     session is left untouched, exactly like the real endpoint.
import { registerOperation } from '../registry';
import { apiError, notFound, unauthorized } from '../apiError';
import { authContextForHeader, getStore, mintTokenPair } from '../fixtures';
import type { components } from '../../api/generated/contract';

type Org = components['schemas']['Org'];
type OrgPage = components['schemas']['OrgPage'];
type SwitchOrgRequest = components['schemas']['SwitchOrgRequest'];
type TokenResponse = components['schemas']['TokenResponse'];

function orgDto(orgId: string, role: Org['role']): Org | undefined {
  const org = getStore().orgs.find((o) => o.id === orgId);
  if (!org) return undefined;
  return { id: org.id, slug: org.slug, name: org.name, role, createdAt: org.createdAt };
}

registerOperation('listOrgs', (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const items: Org[] = getStore()
    .orgMemberships.filter((m) => m.userId === auth.user.userId && m.state === 'active')
    .map((m) => orgDto(m.orgId, m.role))
    .filter((org): org is Org => org !== undefined);
  const body: OrgPage = { items, page: { hasMore: false, nextCursor: null } };
  return { status: 200, body };
});

registerOperation('getOrg', (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const orgId = ctx.params.orgId;
  const membership = getStore().orgMemberships.find(
    (m) => m.userId === auth.user.userId && m.orgId === orgId && m.state === 'active',
  );
  // Identical response whether the org doesn't exist or the caller just
  // isn't an active member of it — see module doc.
  const org = membership ? orgDto(membership.orgId, membership.role) : undefined;
  if (!org) return notFound(`Organization ${orgId} does not exist.`);
  return { status: 200, body: org };
});

registerOperation('switchOrg', async (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const body = await ctx.json<SwitchOrgRequest>();
  const membership = getStore().orgMemberships.find(
    (m) => m.userId === auth.user.userId && m.orgId === body.orgId && m.state === 'active',
  );
  if (!membership) {
    return {
      status: 403,
      body: apiError(
        'membership_missing',
        'Caller is not an active member of the requested organization.',
      ),
    };
  }
  const pair = mintTokenPair(auth.user, body.orgId);
  const response: TokenResponse = {
    accessToken: pair.accessToken,
    refreshToken: pair.refreshToken,
    tokenType: 'Bearer',
    expiresIn: 3600,
    orgId: body.orgId,
    userId: auth.user.userId,
  };
  return { status: 200, body: response };
});
