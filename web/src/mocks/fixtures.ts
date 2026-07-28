/**
 * In-memory seed data + mutable "store" the handlers read/write. Not derived
 * from the spec (there's no ground-truth *data* to derive — only shape, which
 * `schema/validate.ts` checks separately) but every record's required fields
 * are exercised by `handlers/*.test.ts` against `spec.schemas`, so a fixture
 * missing a field the spec requires fails a test rather than lingering.
 */
import type { components } from '../api/generated/contract';
import { encodeCursor, decodeCursor, mockTimestamp, mockUuid } from './ids';

type Collection = components['schemas']['Collection'];
type Document = components['schemas']['Document'];
type DocumentVersion = components['schemas']['DocumentVersion'];
type Job = components['schemas']['Job'];
type MeResponse = components['schemas']['MeResponse'];
type Membership = components['schemas']['Membership'];
type Org = components['schemas']['Org'];
type OrgRole = Org['role'];
type OrgState = Membership['state'];

export interface MockUser {
  userId: string;
  orgId: string;
  email: string;
  password: string;
  displayName: string;
  permissions: string[];
  allowedCollectionIds: string[];
}

export interface ConflictRecord {
  id: string;
  status: string;
  severity: string;
  conflictType: string;
  claimAId: string;
  claimBId: string;
  collectionAId: string;
  collectionBId: string;
  firstDetectedAt: string;
  resolvedAt: string | null;
  resolutionNote: string | null;
}

export interface DownloadCapabilityRecord {
  capability: string;
  documentId: string;
  versionId: string;
  purpose: 'markdown' | 'original';
  expiresAt: number;
  redeemed: boolean;
}

/**
 * Server-side invite record: every wire `Invite` field plus the token hash,
 * which `Invite` (the response schema) never carries — mirrors
 * `DownloadCapabilityRecord`'s split between "what's stored" and "what's on
 * the wire". `status` is intentionally absent here too: like the real
 * `org_invites` table, it's derived at read time from
 * `acceptedAt`/`revokedAt`/`expiresAt` (see `handlers/members.ts`'s
 * `inviteStatus`), never stored as its own field.
 */
export interface InviteRecord {
  id: string;
  email: string;
  role: Membership['role'];
  tokenHash: string;
  expiresAt: string;
  acceptedAt: string | null;
  revokedAt: string | null;
  createdAt: string;
}

/**
 * A token is scoped to exactly one org — `orgId` here is what `switchOrg`
 * mints a *new* access token for, independent of `user.orgId` (the user's
 * "home"/original org). A session in org A stays alive and usable after a
 * switch mints a fresh, separate token pair for org B (1C-01's "the caller's
 * session in their current org is untouched").
 */
export interface AccessTokenRecord {
  userId: string;
  sessionId: string;
  orgId: string;
}

/** Catalog entry for an org — 1C-01's `Org`/`OrgPage`, minus the caller-specific `role` (that's per-membership, see `OrgMembershipRecord`). */
export interface OrgRecord {
  id: string;
  slug: string;
  name: string;
  createdAt: string;
}

/** One user's membership in one org — the thing `listOrgs`/`getOrg`/`switchOrg` all re-check against, mirroring `crates/server/src/services/orgs.rs`'s PostgreSQL membership re-check. */
export interface OrgMembershipRecord {
  userId: string;
  orgId: string;
  role: OrgRole;
  state: OrgState;
}

/** Per-org `permissions`/`allowedCollectionIds` for `GET /auth/me` after a switch — the same user can carry a different collection allowlist per org even though this fixture set keeps `permissions` identical across the demo user's two orgs. */
export interface OrgProfile {
  permissions: string[];
  allowedCollectionIds: string[];
}

interface Store {
  users: MockUser[];
  refreshTokens: Map<string, { userId: string; orgId: string }>; // refreshToken -> {userId, orgId}
  accessTokens: Map<string, AccessTokenRecord>; // accessToken -> {userId, sessionId, orgId}
  collections: Collection[];
  /** collectionId -> the org it belongs to. Not part of the wire `Collection` shape (the real schema has no `orgId` field — org isolation is enforced server-side from the bearer token, never a response field), kept here purely so mock handlers can filter. */
  collectionOrgId: Map<string, string>;
  documents: Map<string, Document[]>; // collectionId -> documents
  versions: Map<string, DocumentVersion[]>; // documentId -> versions, oldest first
  jobs: Map<string, Job>;
  conflicts: ConflictRecord[];
  downloadCapabilities: Map<string, DownloadCapabilityRecord>;
  /** Memberships of the demo org (`DEMO_USER.orgId`) — every seeded/mock user's role+state. Unaffected by org switch: the admin member/invite/usage pages are out of scope for the org-switch task and stay pinned to this one org's roster. */
  memberships: Membership[];
  invites: InviteRecord[];
  /** Org catalog for `listOrgs`/`getOrg`/`switchOrg` (1C-01). */
  orgs: OrgRecord[];
  /** Every user's org memberships, across every org — not just the demo org's roster above. */
  orgMemberships: OrgMembershipRecord[];
  /** orgId -> permissions/allowedCollectionIds `GET /auth/me` reports once a session is scoped to that org. */
  orgProfiles: Map<string, OrgProfile>;
}

const DEMO_USER: MockUser = {
  userId: mockUuid(1),
  orgId: mockUuid(2),
  email: 'demo@markhand.test',
  password: 'demo-password',
  displayName: 'Demo User',
  // Deliberately WITHOUT `member.manage` — `App.test.tsx` already has a
  // scenario ("renders an in-shell notice... for a signed-in user without
  // member.manage") that depends on this exact user lacking it. P2-11/P2-12
  // tests that need a member.manage caller grant it on this same seeded user
  // via `components/admin/testSupport.ts`'s `grantMemberManage()` instead of
  // changing the shared default here.
  permissions: ['doc.quarantine.review', 'qa.history'],
  allowedCollectionIds: [mockUuid(10), mockUuid(11)],
};

/** A second seeded org member — active admin, non-owner — so member-list tests have more than one row and a non-owner promotion/demotion target. */
export const SECOND_MEMBER_USER_ID = mockUuid(30);
/** A third seeded org member — suspended viewer — so the list shows a non-active row too. */
export const THIRD_MEMBER_USER_ID = mockUuid(31);

/**
 * Org switch (1C-01 + P2-06/P2-15). `DEMO_USER.orgId` (`mockUuid(2)`) is
 * "org A" — the org every other fixture/test in this file predates this
 * feature and already assumes. `ORG_B_ID` is a second org the same demo user
 * is also an active member of, seeded with its own collection/document (see
 * `ORG_B_COLLECTION_ID` below) so a switch has something visibly different to
 * render — the whole point being to prove the previous org's data is gone,
 * not just that a new orgId string appears somewhere.
 */
export const ORG_A_ID = DEMO_USER.orgId;
/** The seeded demo user's own id — exported so test-only helpers (e.g. `components/shell/testSupport.ts`'s `seedThirdOrgMembership`) can add memberships/profiles for them without reaching into `getStore().users` themselves. */
export const DEMO_USER_ID = DEMO_USER.userId;
export const ORG_B_ID = mockUuid(3);
export const ORG_B_COLLECTION_ID = mockUuid(12);
export const ORG_B_DOCUMENT_ID = mockUuid(120);
const ORG_B_VERSION_ID = mockUuid(1200);

function seedOrgs(): OrgRecord[] {
  return [
    { id: ORG_A_ID, slug: 'acme-co', name: 'Acme Co', createdAt: mockTimestamp(0) },
    { id: ORG_B_ID, slug: 'globex-labs', name: 'Globex Labs', createdAt: mockTimestamp(0) },
  ];
}

function seedOrgMemberships(): OrgMembershipRecord[] {
  return [
    { userId: DEMO_USER.userId, orgId: ORG_A_ID, role: 'owner', state: 'active' },
    { userId: DEMO_USER.userId, orgId: ORG_B_ID, role: 'editor', state: 'active' },
  ];
}

function seedOrgProfiles(): Map<string, OrgProfile> {
  return new Map([
    [
      ORG_A_ID,
      { permissions: DEMO_USER.permissions, allowedCollectionIds: [mockUuid(10), mockUuid(11)] },
    ],
    [ORG_B_ID, { permissions: DEMO_USER.permissions, allowedCollectionIds: [ORG_B_COLLECTION_ID] }],
  ]);
}

function seedMemberships(): Membership[] {
  return [
    { userId: DEMO_USER.userId, role: 'owner', state: 'active', createdAt: mockTimestamp(0) },
    {
      userId: SECOND_MEMBER_USER_ID,
      role: 'admin',
      state: 'active',
      createdAt: mockTimestamp(10),
    },
    {
      userId: THIRD_MEMBER_USER_ID,
      role: 'viewer',
      state: 'suspended',
      createdAt: mockTimestamp(20),
    },
  ];
}

function seedInvites(): InviteRecord[] {
  return [
    {
      id: mockUuid(9100),
      email: 'moi-nguoi-moi@example.com',
      role: 'editor',
      tokenHash: 'seed-invite-token-hash-unused',
      expiresAt: mockTimestamp(60 * 24 * 7),
      acceptedAt: null,
      revokedAt: null,
      createdAt: mockTimestamp(0),
    },
  ];
}

function seedCollections(): Collection[] {
  return [
    {
      id: mockUuid(10),
      name: 'Employee Handbook',
      slug: 'employee-handbook',
      description: 'Company policies and onboarding material.',
      visibility: 'org',
      createdAt: mockTimestamp(0),
    },
    {
      id: mockUuid(11),
      name: 'Product Specs',
      slug: 'product-specs',
      description: null,
      visibility: 'private',
      createdAt: mockTimestamp(60),
    },
    // Org B's own collection — deliberately distinct name/content from every
    // org A fixture above, so an org-switch test can assert its *absence*
    // while on org A and its presence (with org A's fully gone) after
    // switching, rather than merely asserting "some org id changed".
    {
      id: ORG_B_COLLECTION_ID,
      name: 'Globex Roadmap',
      slug: 'globex-roadmap',
      description: 'Globex Labs planning material — org B only.',
      visibility: 'org',
      createdAt: mockTimestamp(90),
    },
  ];
}

function seedDocuments(): Map<string, Document[]> {
  const map = new Map<string, Document[]>();
  map.set(mockUuid(10), [
    {
      id: mockUuid(100),
      collectionId: mockUuid(10),
      title: 'Onboarding Guide.pdf',
      state: 'indexed',
      currentVersionId: mockUuid(1000),
      createdAt: mockTimestamp(1),
      updatedAt: mockTimestamp(5),
    },
    {
      id: mockUuid(101),
      collectionId: mockUuid(10),
      title: 'Leave Policy.docx',
      state: 'converting',
      currentVersionId: null,
      createdAt: mockTimestamp(2),
      updatedAt: mockTimestamp(2),
    },
  ]);
  map.set(mockUuid(11), [
    {
      id: mockUuid(110),
      collectionId: mockUuid(11),
      title: 'Roadmap.xlsx',
      state: 'indexed',
      currentVersionId: mockUuid(1100),
      createdAt: mockTimestamp(3),
      updatedAt: mockTimestamp(7),
    },
  ]);
  map.set(ORG_B_COLLECTION_ID, [
    {
      id: ORG_B_DOCUMENT_ID,
      collectionId: ORG_B_COLLECTION_ID,
      title: 'Globex Master Plan.pdf',
      state: 'indexed',
      currentVersionId: ORG_B_VERSION_ID,
      createdAt: mockTimestamp(91),
      updatedAt: mockTimestamp(92),
    },
  ]);
  return map;
}

function seedVersions(): Map<string, DocumentVersion[]> {
  const map = new Map<string, DocumentVersion[]>();
  map.set(mockUuid(100), [
    {
      id: mockUuid(1000),
      documentId: mockUuid(100),
      versionNumber: 1,
      isCurrent: true,
      sourceContentSha256: 'a'.repeat(64),
      effectiveFrom: mockTimestamp(1),
      effectiveTo: null,
      changeSummary: 'Initial upload.',
      createdAt: mockTimestamp(1),
    },
  ]);
  map.set(mockUuid(110), [
    {
      id: mockUuid(1100),
      documentId: mockUuid(110),
      versionNumber: 1,
      isCurrent: true,
      sourceContentSha256: 'b'.repeat(64),
      effectiveFrom: mockTimestamp(3),
      effectiveTo: null,
      changeSummary: null,
      createdAt: mockTimestamp(3),
    },
  ]);
  map.set(ORG_B_DOCUMENT_ID, [
    {
      id: ORG_B_VERSION_ID,
      documentId: ORG_B_DOCUMENT_ID,
      versionNumber: 1,
      isCurrent: true,
      sourceContentSha256: 'c'.repeat(64),
      effectiveFrom: mockTimestamp(91),
      effectiveTo: null,
      changeSummary: null,
      createdAt: mockTimestamp(91),
    },
  ]);
  return map;
}

function seedCollectionOrgId(): Map<string, string> {
  return new Map([
    [mockUuid(10), ORG_A_ID],
    [mockUuid(11), ORG_A_ID],
    [ORG_B_COLLECTION_ID, ORG_B_ID],
  ]);
}

function seedJobs(): Map<string, Job> {
  const map = new Map<string, Job>();
  const job: Job = {
    id: mockUuid(9000),
    jobType: 'convert',
    status: 'succeeded',
    attempts: 1,
    documentId: mockUuid(100),
    versionId: mockUuid(1000),
    createdAt: mockTimestamp(1),
    updatedAt: mockTimestamp(2),
    finishedAt: mockTimestamp(2),
  };
  map.set(job.id, job);
  return map;
}

function seedConflicts(): ConflictRecord[] {
  return [
    {
      id: mockUuid(8000),
      status: 'open',
      severity: 'high',
      conflictType: 'contradiction',
      claimAId: mockUuid(8001),
      claimBId: mockUuid(8002),
      collectionAId: mockUuid(10),
      collectionBId: mockUuid(11),
      firstDetectedAt: mockTimestamp(30),
      resolvedAt: null,
      resolutionNote: null,
    },
  ];
}

function freshStore(): Store {
  return {
    users: [{ ...DEMO_USER }],
    refreshTokens: new Map(),
    accessTokens: new Map(),
    collections: seedCollections(),
    collectionOrgId: seedCollectionOrgId(),
    documents: seedDocuments(),
    versions: seedVersions(),
    jobs: seedJobs(),
    conflicts: seedConflicts(),
    downloadCapabilities: new Map(),
    memberships: seedMemberships(),
    invites: seedInvites(),
    orgs: seedOrgs(),
    orgMemberships: seedOrgMemberships(),
    orgProfiles: seedOrgProfiles(),
  };
}

let store = freshStore();

/** Restores all mock state to the initial seed. Call between tests for isolation. */
export function resetMockStore(): void {
  store = freshStore();
}

export function getStore(): Store {
  return store;
}

let runtimeIdSeed = 20_000; // seeded fixtures all use ids below this; keeps runtime-created ids collision-free

/** A fresh id for entities created during a test/dev session (uploads, new collections, jobs...). */
export function nextId(): string {
  runtimeIdSeed += 1;
  return mockUuid(runtimeIdSeed);
}

export interface TokenPair {
  accessToken: string;
  refreshToken: string;
  sessionId: string;
}

/**
 * Mints and registers a fresh access/refresh token pair for `user`, scoped to
 * `orgId` (defaults to the user's home org — the plain login/refresh case).
 * `switchOrg` is the one caller that passes a *different* org: the resulting
 * pair is an independent family scoped to that org, and the org-A family
 * this call didn't touch stays valid (mirrors the real server's "caller's
 * session in their current org is untouched").
 */
export function mintTokenPair(user: MockUser, orgId: string = user.orgId): TokenPair {
  const accessToken = `mock-access.${nextId()}`;
  const refreshToken = `mock-refresh.${nextId()}`;
  const sessionId = `mock-session.${nextId()}`;
  store.accessTokens.set(accessToken, { userId: user.userId, sessionId, orgId });
  store.refreshTokens.set(refreshToken, { userId: user.userId, orgId });
  return { accessToken, refreshToken, sessionId };
}

/** Looks up the caller (and their session + the org this specific token is scoped to) from an `Authorization: Bearer <token>` header value. */
export function authContextForHeader(
  authorizationHeader: string | null,
): { user: MockUser; sessionId: string; orgId: string } | undefined {
  const match = authorizationHeader ? /^Bearer\s+(.+)$/i.exec(authorizationHeader.trim()) : null;
  if (!match) return undefined;
  const record = store.accessTokens.get(match[1]);
  if (!record) return undefined;
  const user = store.users.find((u) => u.userId === record.userId);
  if (!user) return undefined;
  return { user, sessionId: record.sessionId, orgId: record.orgId };
}

/** `GET /auth/me`'s shape for `user`, scoped to `orgId` (defaults to the user's home org) — `permissions`/`allowedCollectionIds` come from that org's `OrgProfile`, not a single fixed value on `user`, so a post-switch `/auth/me` reflects the org actually active for this token. */
export function toMeResponse(
  user: MockUser,
  sessionId: string,
  orgId: string = user.orgId,
): MeResponse {
  const profile = store.orgProfiles.get(orgId);
  return {
    userId: user.userId,
    orgId,
    email: user.email,
    displayName: user.displayName,
    permissions: profile?.permissions ?? user.permissions,
    allowedCollectionIds: profile?.allowedCollectionIds ?? user.allowedCollectionIds,
    sessionId,
  };
}

export { encodeCursor, decodeCursor };
