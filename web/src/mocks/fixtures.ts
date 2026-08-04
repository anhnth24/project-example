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
type UsageEntry = components['schemas']['UsageEntry'];
type Project = components['schemas']['Project'];
type CitationPin = components['schemas']['CitationPin'];

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

/**
 * P2-19 — one stored Q&A turn inside a chat session. Mirrors
 * `db::chat_sessions::ChatTurn`'s wire fields exactly; `citations`/`warnings`
 * are stored (and returned) opaque/verbatim, same "never re-validated on
 * read" contract `routes::chat_sessions`'s module doc documents — this mock
 * does not re-check a citation's hash/span/ACL any more than the real server
 * does.
 */
export interface ChatTurnRecord {
  id: string;
  seq: number;
  question: string;
  answer: string;
  answerMode: string;
  citations: CitationPin[];
  warnings: string[];
  createdAt: string;
}

/**
 * P2-19 — a private, per-user chat session. Scoped by BOTH `orgId` and
 * `userId` (never just one) — mirrors the real `qa_chat_sessions` table's
 * "RLS org isolation + `user_id = caller` application filter" so a session
 * belonging to another user in the *same* org is exactly as invisible as one
 * in a different org (`handlers/chatSessions.ts`'s own doc has the full
 * rationale, same as `crates/server/src/db/chat_sessions.rs`'s).
 */
export interface ChatSessionRecord {
  id: string;
  orgId: string;
  userId: string;
  title: string;
  createdAt: string;
  updatedAt: string;
  /**
   * Monotonic "most recently active" rank, bumped on create/rename/append-
   * turn. Every timestamp this fixture set hands out is deterministic
   * (`mockTimestamp`), so two sessions/mutations can easily land on the same
   * instant — this counter is what actually breaks ties for
   * `listChatSessions`'s "most recently active first" ordering, the same
   * role `nextId()`'s counter plays for id uniqueness.
   */
  activityRank: number;
  turns: ChatTurnRecord[];
}

interface Store {
  users: MockUser[];
  refreshTokens: Map<string, { userId: string; orgId: string }>; // refreshToken -> {userId, orgId}
  accessTokens: Map<string, AccessTokenRecord>; // accessToken -> {userId, sessionId, orgId}
  collections: Collection[];
  /** collectionId -> the org it belongs to. Not part of the wire `Collection` shape (the real schema has no `orgId` field — org isolation is enforced server-side from the bearer token, never a response field), kept here purely so mock handlers can filter. */
  collectionOrgId: Map<string, string>;
  /** P2-18 — orgId -> that org's projects. Same per-org-roster shape as `membershipsByOrg`. */
  projectsByOrg: Map<string, Project[]>;
  /** P2-18 — collectionId -> assigned projectId, or `null` when unassigned. Not part of the wire `Collection` shape here either (that lives on the DTO's own `projectId`/`projectName` fields, computed at read time in `handlers/library.ts` — this map is the mutable "what's assigned" source of truth those fields are derived from). */
  collectionProjectId: Map<string, string | null>;
  documents: Map<string, Document[]>; // collectionId -> documents
  versions: Map<string, DocumentVersion[]>; // documentId -> versions, oldest first
  jobs: Map<string, Job>;
  conflicts: ConflictRecord[];
  downloadCapabilities: Map<string, DownloadCapabilityRecord>;
  /**
   * orgId -> that org's member roster (role/state per user) for the admin
   * members page (`handlers/members.ts` E1/E6/E7). Each org gets its own
   * roster — a member row seeded for org A says nothing about org B, mirroring
   * `collectionOrgId`'s "org isolation lives in the mock store, not on the
   * wire shape" pattern above. Read/written through `getOrgMemberships()`,
   * never indexed directly, so a lookup for an org with nothing seeded gets an
   * (auto-vivified) empty roster instead of `undefined`.
   */
  membershipsByOrg: Map<string, Membership[]>;
  /** orgId -> that org's invites (E2-E5), same per-org shape as `membershipsByOrg`. Read/written through `getOrgInvites()`/`findInviteAcrossOrgs()`. */
  invitesByOrg: Map<string, InviteRecord[]>;
  /** Org catalog for `listOrgs`/`getOrg`/`switchOrg` (1C-01). */
  orgs: OrgRecord[];
  /** Every user's org memberships, across every org — not just the demo org's roster above. */
  orgMemberships: OrgMembershipRecord[];
  /** orgId -> permissions/allowedCollectionIds `GET /auth/me` reports once a session is scoped to that org. */
  orgProfiles: Map<string, OrgProfile>;
  /** orgId -> `GET /usage` snapshot (E8), read through `getOrgUsage()`. */
  usageByOrg: Map<string, UsageEntry[]>;
  /**
   * P2-19 — chat history, keyed by `chatSessionKey(orgId, userId)` (never by
   * `orgId` alone — see `ChatSessionRecord`'s own doc). Read/written through
   * `getUserChatSessions()`/`findUserChatSession()`, same auto-vivifying
   * "always an array to push/splice against" convention `membershipsByOrg`
   * already established.
   */
  chatSessionsByOrgUser: Map<string, ChatSessionRecord[]>;
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
export const SECOND_MEMBER_DISPLAY_NAME = 'Bao Tran';
export const SECOND_MEMBER_EMAIL = 'bao-tran@example.com';
/** A third seeded org member — suspended viewer — so the list shows a non-active row too. */
export const THIRD_MEMBER_USER_ID = mockUuid(31);
export const THIRD_MEMBER_DISPLAY_NAME = 'Chi Vo';
export const THIRD_MEMBER_EMAIL = 'chi-vo@example.com';

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
/** Org B's second seeded member (`editor`, active) — so its admin members roster has more than the demo user's own row, same reason `SECOND_MEMBER_USER_ID` exists for org A. */
export const GLOBEX_MEMBER_USER_ID = mockUuid(32);
export const GLOBEX_MEMBER_DISPLAY_NAME = 'Duc Nguyen';
export const GLOBEX_MEMBER_EMAIL = 'duc-nguyen@example.com';

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
    {
      userId: DEMO_USER.userId,
      email: DEMO_USER.email,
      displayName: DEMO_USER.displayName,
      role: 'owner',
      state: 'active',
      createdAt: mockTimestamp(0),
    },
    {
      userId: SECOND_MEMBER_USER_ID,
      email: SECOND_MEMBER_EMAIL,
      displayName: SECOND_MEMBER_DISPLAY_NAME,
      role: 'admin',
      state: 'active',
      createdAt: mockTimestamp(10),
    },
    {
      userId: THIRD_MEMBER_USER_ID,
      email: THIRD_MEMBER_EMAIL,
      displayName: THIRD_MEMBER_DISPLAY_NAME,
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

/**
 * Org B's own admin members roster — deliberately small and deliberately
 * different from org A's (`seedMemberships()`): the demo user is `owner` here
 * (not `admin`, its org A role) and the second row is a distinct user id
 * (`GLOBEX_MEMBER_USER_ID`, not `SECOND_MEMBER_USER_ID`) with a different
 * role/state combination, so a test asserting "org B's list, not org A's" has
 * more than an orgId to go on.
 */
function seedOrgBMemberships(): Membership[] {
  return [
    {
      userId: DEMO_USER.userId,
      email: DEMO_USER.email,
      displayName: DEMO_USER.displayName,
      role: 'owner',
      state: 'active',
      createdAt: mockTimestamp(90),
    },
    {
      userId: GLOBEX_MEMBER_USER_ID,
      email: GLOBEX_MEMBER_EMAIL,
      displayName: GLOBEX_MEMBER_DISPLAY_NAME,
      role: 'editor',
      state: 'active',
      createdAt: mockTimestamp(91),
    },
  ];
}

/** Org B's own invite — different email/role from org A's seeded invite (`seedInvites()`), so the two lists are never accidentally interchangeable in an assertion. */
function seedOrgBInvites(): InviteRecord[] {
  return [
    {
      id: mockUuid(9200),
      email: 'khach-moi-globex@example.com',
      role: 'viewer',
      tokenHash: 'seed-invite-token-hash-unused-org-b',
      expiresAt: mockTimestamp(60 * 24 * 7),
      acceptedAt: null,
      revokedAt: null,
      createdAt: mockTimestamp(90),
    },
  ];
}

/** Per-org `GET /usage` snapshots (E8) — org A keeps the original hand-picked values `AdminUsagePage.test.tsx` asserts exact numbers against; org B's are deliberately smaller/different so a post-switch usage page reads as genuinely different data, not the same numbers under a new orgId. */
function seedUsageByOrg(): Map<string, UsageEntry[]> {
  return new Map([
    [
      ORG_A_ID,
      [
        {
          resource: 'storage_bytes',
          limit: 10_737_418_240,
          committed: 3_221_225_472,
          reserved: 104_857_600,
          remaining: 7_411_335_168,
        },
        { resource: 'documents', limit: 5000, committed: 812, reserved: 3, remaining: 4185 },
        { resource: 'concurrent_jobs', limit: 8, committed: 1, reserved: 0, remaining: 7 },
        {
          resource: 'tokens',
          limit: 2_000_000,
          committed: 415_000,
          reserved: 0,
          remaining: 1_585_000,
        },
      ],
    ],
    [
      ORG_B_ID,
      [
        {
          resource: 'storage_bytes',
          limit: 2_147_483_648,
          committed: 268_435_456,
          reserved: 0,
          remaining: 1_879_048_192,
        },
        { resource: 'documents', limit: 500, committed: 42, reserved: 0, remaining: 458 },
        { resource: 'concurrent_jobs', limit: 2, committed: 0, reserved: 0, remaining: 2 },
        { resource: 'tokens', limit: 200_000, committed: 12_000, reserved: 0, remaining: 188_000 },
      ],
    ],
  ]);
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

// ---------------------------------------------------------------------------
// P2-18 projects — org -> project -> collection -> document grouping.
// ---------------------------------------------------------------------------

/** Org A: two projects. `PROJECT_A_HR_ID` is assigned to Employee Handbook
 * (`mockUuid(10)`); `PROJECT_A_PRODUCT_ID` starts with zero collections
 * assigned, so Product Specs (`mockUuid(11)`) stays under "Chưa thuộc dự án"
 * — the "org A: 2 projects + 1 unassigned collection" fixture the E2E spec
 * needs, without inventing a third org A collection. */
export const PROJECT_A_HR_ID = mockUuid(200);
export const PROJECT_A_PRODUCT_ID = mockUuid(201);
/** Org B's own project, separate id space from org A's — same "distinct
 * enough that a cross-org assertion can't pass by accident" convention as
 * `ORG_B_COLLECTION_ID`. */
export const PROJECT_B_ID = mockUuid(202);

function seedProjects(): Map<string, Project[]> {
  return new Map([
    [
      ORG_A_ID,
      [
        { id: PROJECT_A_HR_ID, name: 'Nhân sự', createdAt: mockTimestamp(0) },
        { id: PROJECT_A_PRODUCT_ID, name: 'Sản phẩm', createdAt: mockTimestamp(1) },
      ],
    ],
    [ORG_B_ID, [{ id: PROJECT_B_ID, name: 'Globex Ops', createdAt: mockTimestamp(90) }]],
  ]);
}

function seedCollectionProjectId(): Map<string, string | null> {
  return new Map([
    [mockUuid(10), PROJECT_A_HR_ID],
    [mockUuid(11), null],
    [ORG_B_COLLECTION_ID, PROJECT_B_ID],
  ]);
}

/**
 * P2-10 (Q&A) demo document — the one seeded document with **two** published
 * versions, so `mode: 'compare'`/`'history'` ask requests (and the version
 * picker that drives them, `components/qa/ChatPanel.tsx`) have something real
 * to compare instead of every other seeded document's single version. Added
 * into the existing "Product Specs" collection (`mockUuid(11)`, already in
 * `DEMO_USER.allowedCollectionIds`) rather than a new collection, and purely
 * additive to `seedDocuments()`/`seedVersions()` above — no existing id's data
 * changes, so `LibraryPage.test.tsx`/`DocumentRowActions.test.tsx` (which key
 * off specific pre-existing titles/ids, not "how many documents total") stay
 * unaffected. Numeric id range (150/1500/1501) is disjoint from every id used
 * elsewhere in this file.
 */
export const QA_COMPARE_DOCUMENT_ID = mockUuid(150);
export const QA_COMPARE_VERSION_A_ID = mockUuid(1500);
export const QA_COMPARE_VERSION_B_ID = mockUuid(1501);

function seedQaCompareDocument(
  documents: Map<string, Document[]>,
  versions: Map<string, DocumentVersion[]>,
): void {
  const collectionId = mockUuid(11);
  const docs = documents.get(collectionId) ?? [];
  docs.push({
    id: QA_COMPARE_DOCUMENT_ID,
    collectionId,
    title: 'Chính sách ngân sách vận hành.pdf',
    state: 'indexed',
    currentVersionId: QA_COMPARE_VERSION_B_ID,
    createdAt: mockTimestamp(50),
    updatedAt: mockTimestamp(95),
  });
  documents.set(collectionId, docs);
  versions.set(QA_COMPARE_DOCUMENT_ID, [
    {
      id: QA_COMPARE_VERSION_A_ID,
      documentId: QA_COMPARE_DOCUMENT_ID,
      versionNumber: 1,
      isCurrent: false,
      sourceContentSha256: 'd'.repeat(64),
      effectiveFrom: mockTimestamp(50),
      effectiveTo: mockTimestamp(95),
      // P2-10 conflict-warning demo: BA's original claim (10 triệu/quý) —
      // superseded by v2's design-driven figure below. Kept as the concrete
      // "BA 10m vs design 15m" conflict `mocks/handlers/qa.ts`'s
      // as-of/compare/history warnings demonstrate.
      changeSummary: 'BA đề xuất ngân sách vận hành 10 triệu đồng mỗi quý.',
      createdAt: mockTimestamp(50),
    },
    {
      id: QA_COMPARE_VERSION_B_ID,
      documentId: QA_COMPARE_DOCUMENT_ID,
      versionNumber: 2,
      isCurrent: true,
      sourceContentSha256: 'e'.repeat(64),
      effectiveFrom: mockTimestamp(95),
      effectiveTo: null,
      // The resolution side of the same conflict — design overrides BA's
      // figure, and this is the version `currentModeWarnings`/the new
      // as-of/compare/history warnings point callers back to as "resolved".
      changeSummary:
        'Bộ phận thiết kế điều chỉnh ngân sách vận hành lên 15 triệu đồng mỗi quý, giải quyết xung đột với đề xuất ban đầu của BA.',
      createdAt: mockTimestamp(95),
    },
  ]);
}

// ---------------------------------------------------------------------------
// P2-19 — private per-user chat history seed. Self-contained (own citation
// literals below, not derived from `mocks/handlers/qa.ts`'s live passage
// catalog — that module already imports from this one, so the reverse import
// would be circular), same "own fixed data, not derived from the spec"
// convention every other seed function in this file follows. Two sessions for
// the demo user in org A: one with a single-document citation, one with a
// second turn citing two different documents (so the footnote UI's "Tổng hợp
// từ N tài liệu" note and a `page`-bearing citation both have something real
// to render in tests/demo without waiting on a live `/ask/stream` round trip).
// ---------------------------------------------------------------------------

export const DEMO_CHAT_SESSION_ROADMAP_ID = mockUuid(9300);
export const DEMO_CHAT_SESSION_ONBOARDING_ID = mockUuid(9301);

function chatSessionKey(orgId: string, userId: string): string {
  return `${orgId} ${userId}`;
}

function seedCitation(
  citeId: string,
  documentId: string,
  versionId: string,
  collectionId: string,
  documentTitle: string,
  quote: string,
  page?: number,
): CitationPin {
  return {
    citeId,
    logicalDocumentId: documentId,
    versionId,
    collectionId,
    documentTitle,
    sourceContentSha256: `src-${versionId}`,
    canonicalMarkdownSha256: `md-${versionId}`,
    quoteSha256: `quote-${versionId}`,
    chunkIdentitySha256: `chunk-${versionId}`,
    quote,
    sourceSpanStart: 0,
    sourceSpanEnd: quote.length,
    quoteLocalStart: 0,
    quoteLocalEnd: quote.length,
    page,
    isCurrent: true,
    anchor: `mhcite-1.${documentId.slice(-4)}`,
  };
}

function seedChatSessions(): Map<string, ChatSessionRecord[]> {
  const roadmapCitation = seedCitation(
    'CITE-0001',
    mockUuid(110),
    mockUuid(1100),
    mockUuid(11),
    'Roadmap.xlsx',
    'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
  );
  const onboardingCitation = seedCitation(
    'CITE-0001',
    mockUuid(100),
    mockUuid(1000),
    mockUuid(10),
    'Onboarding Guide.pdf',
    'Nhân viên mới cần hoàn thành khóa đào tạo hội nhập trong 30 ngày đầu tiên.',
    3,
  );
  // A second, DIFFERENT document (not "Onboarding Guide.pdf" above) so the
  // second turn's two citations exercise the footnote block's "Tổng hợp từ N
  // tài liệu" note — reuses the already-seeded QA compare document (a real,
  // `isCurrent` version, so its deep-link/preview genuinely resolves) rather
  // than inventing a third document just for this.
  const budgetCitation = seedCitation(
    'CITE-0002',
    QA_COMPARE_DOCUMENT_ID,
    QA_COMPARE_VERSION_B_ID,
    mockUuid(11),
    'Chính sách ngân sách vận hành.pdf',
    'Ngân sách vận hành được thiết kế điều chỉnh thành 15 triệu đồng mỗi quý, giải quyết xung đột với đề xuất ban đầu của BA.',
  );

  const roadmapSession: ChatSessionRecord = {
    id: DEMO_CHAT_SESSION_ROADMAP_ID,
    orgId: ORG_A_ID,
    userId: DEMO_USER.userId,
    title: 'Lộ trình quý 3 tập trung vào việc gì?',
    createdAt: mockTimestamp(200),
    updatedAt: mockTimestamp(201),
    activityRank: 2,
    turns: [
      {
        id: mockUuid(9310),
        seq: 1,
        question: 'Lộ trình quý 3 tập trung vào việc gì?',
        answer:
          'Dựa trên tài liệu đã lập chỉ mục: Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục. [CITE-0001]',
        answerMode: 'offline_extractive',
        citations: [roadmapCitation],
        warnings: [],
        createdAt: mockTimestamp(200),
      },
    ],
  };

  const onboardingSession: ChatSessionRecord = {
    id: DEMO_CHAT_SESSION_ONBOARDING_ID,
    orgId: ORG_A_ID,
    userId: DEMO_USER.userId,
    title: 'Nhân viên mới cần hoàn thành gì trong 30 ngày đầu?',
    createdAt: mockTimestamp(100),
    updatedAt: mockTimestamp(103),
    activityRank: 1,
    turns: [
      {
        id: mockUuid(9320),
        seq: 1,
        question: 'Nhân viên mới cần hoàn thành gì trong 30 ngày đầu?',
        answer:
          'Dựa trên tài liệu đã lập chỉ mục: Nhân viên mới cần hoàn thành khóa đào tạo hội nhập trong 30 ngày đầu tiên. [CITE-0001]',
        answerMode: 'offline_extractive',
        citations: [onboardingCitation],
        warnings: [],
        createdAt: mockTimestamp(100),
      },
      {
        id: mockUuid(9321),
        seq: 2,
        question: 'Còn ngân sách vận hành thì sao?',
        answer:
          'Dựa trên tài liệu đã lập chỉ mục: Nhân viên mới cần hoàn thành khóa đào tạo hội nhập trong 30 ngày đầu tiên. [CITE-0001] Ngân sách vận hành được thiết kế điều chỉnh thành 15 triệu đồng mỗi quý, giải quyết xung đột với đề xuất ban đầu của BA. [CITE-0002]',
        answerMode: 'offline_extractive',
        citations: [onboardingCitation, budgetCitation],
        warnings: [],
        createdAt: mockTimestamp(103),
      },
    ],
  };

  return new Map([
    [chatSessionKey(ORG_A_ID, DEMO_USER.userId), [roadmapSession, onboardingSession]],
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
  const documents = seedDocuments();
  const versions = seedVersions();
  seedQaCompareDocument(documents, versions);
  return {
    users: [{ ...DEMO_USER }],
    refreshTokens: new Map(),
    accessTokens: new Map(),
    collections: seedCollections(),
    collectionOrgId: seedCollectionOrgId(),
    projectsByOrg: seedProjects(),
    collectionProjectId: seedCollectionProjectId(),
    documents,
    versions,
    jobs: seedJobs(),
    conflicts: seedConflicts(),
    downloadCapabilities: new Map(),
    membershipsByOrg: new Map([
      [ORG_A_ID, seedMemberships()],
      [ORG_B_ID, seedOrgBMemberships()],
    ]),
    invitesByOrg: new Map([
      [ORG_A_ID, seedInvites()],
      [ORG_B_ID, seedOrgBInvites()],
    ]),
    orgs: seedOrgs(),
    orgMemberships: seedOrgMemberships(),
    orgProfiles: seedOrgProfiles(),
    usageByOrg: seedUsageByOrg(),
    chatSessionsByOrgUser: seedChatSessions(),
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

/** `orgId`'s admin members roster, auto-vivifying an empty one for an org with nothing seeded (rather than returning `undefined`) — the same "always an array to push/splice against" convention `documents`/`versions` already rely on via `?? []`/`.set(...)` at their call sites. */
export function getOrgMemberships(orgId: string): Membership[] {
  let roster = store.membershipsByOrg.get(orgId);
  if (!roster) {
    roster = [];
    store.membershipsByOrg.set(orgId, roster);
  }
  return roster;
}

/** `orgId`'s invites, same auto-vivifying shape as `getOrgMemberships`. */
export function getOrgInvites(orgId: string): InviteRecord[] {
  let invites = store.invitesByOrg.get(orgId);
  if (!invites) {
    invites = [];
    store.invitesByOrg.set(orgId, invites);
  }
  return invites;
}

/**
 * Searches every org's invite list for one matching `predicate`. Accepting an
 * invite (`acceptMemberInvite`) is the one flow in `handlers/members.ts` that
 * cannot scope by the caller's *current* org (`authContextForHeader(...).orgId`)
 * the way every other operation here does — the invite may belong to an org
 * the caller isn't a member of yet (that's the entire point of the invite),
 * so their bearer token's org tells you nothing about which org's invite list
 * to search.
 */
export function findInviteAcrossOrgs(
  predicate: (invite: InviteRecord) => boolean,
): { orgId: string; invite: InviteRecord } | undefined {
  for (const [orgId, invites] of store.invitesByOrg) {
    const invite = invites.find(predicate);
    if (invite) return { orgId, invite };
  }
  return undefined;
}

/** `orgId`'s `GET /usage` snapshot — `[]` for an org with none seeded rather than throwing, matching every other per-org accessor's "empty, not an error" default. */
export function getOrgUsage(orgId: string): UsageEntry[] {
  return store.usageByOrg.get(orgId) ?? [];
}

/** P2-18 — `orgId`'s projects, auto-vivifying an empty roster like `getOrgMemberships`. */
export function getOrgProjects(orgId: string): Project[] {
  let projects = store.projectsByOrg.get(orgId);
  if (!projects) {
    projects = [];
    store.projectsByOrg.set(orgId, projects);
  }
  return projects;
}

/** P2-18 — `collectionId`'s assigned project id, or `undefined` for a collection this store has never seen (distinct from `null`, which means "seen, explicitly unassigned"). */
export function getCollectionProjectId(collectionId: string): string | null | undefined {
  return store.collectionProjectId.get(collectionId);
}

/** P2-18 — assigns/unassigns `collectionId`'s project (`null` unassigns), same mutation shape `POST /collections/{id}/assign-project` performs server-side. */
export function setCollectionProjectId(collectionId: string, projectId: string | null): void {
  store.collectionProjectId.set(collectionId, projectId);
}

/** P2-18 — collection ids currently assigned to `projectId`, scoped to `orgId` via `collectionOrgId` (mirrors `db::projects::collection_ids_for_project`'s org-scoped filter). */
export function collectionIdsForProject(orgId: string, projectId: string): string[] {
  const ids: string[] = [];
  for (const [collectionId, assigned] of store.collectionProjectId) {
    if (assigned === projectId && store.collectionOrgId.get(collectionId) === orgId) {
      ids.push(collectionId);
    }
  }
  return ids;
}

/**
 * P2-19 — `orgId`+`userId`'s own chat sessions, auto-vivifying an empty list
 * like `getOrgMemberships`/`getOrgProjects`. Never scoped by `orgId` alone —
 * see `ChatSessionRecord`'s own doc for why the composite key is load-bearing,
 * not incidental.
 */
export function getUserChatSessions(orgId: string, userId: string): ChatSessionRecord[] {
  const key = chatSessionKey(orgId, userId);
  let sessions = store.chatSessionsByOrgUser.get(key);
  if (!sessions) {
    sessions = [];
    store.chatSessionsByOrgUser.set(key, sessions);
  }
  return sessions;
}

/** One of `orgId`+`userId`'s own sessions by id, or `undefined` for a missing/foreign one — callers must map a miss to 404, never 403 (no existence oracle, same convention `handlers/chatSessions.ts` follows throughout). */
export function findUserChatSession(
  orgId: string,
  userId: string,
  sessionId: string,
): ChatSessionRecord | undefined {
  return getUserChatSessions(orgId, userId).find((s) => s.id === sessionId);
}

let chatSessionActivityCounter = 100; // above every seeded session's own activityRank

/** Next monotonic `activityRank` for a create/rename/append-turn mutation — see `ChatSessionRecord`'s own doc. */
export function nextChatSessionActivityRank(): number {
  chatSessionActivityCounter += 1;
  return chatSessionActivityCounter;
}

export { encodeCursor, decodeCursor };
