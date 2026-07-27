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

export interface AccessTokenRecord {
  userId: string;
  sessionId: string;
}

interface Store {
  users: MockUser[];
  refreshTokens: Map<string, string>; // refreshToken -> userId
  accessTokens: Map<string, AccessTokenRecord>; // accessToken -> {userId, sessionId}
  collections: Collection[];
  documents: Map<string, Document[]>; // collectionId -> documents
  versions: Map<string, DocumentVersion[]>; // documentId -> versions, oldest first
  jobs: Map<string, Job>;
  conflicts: ConflictRecord[];
  downloadCapabilities: Map<string, DownloadCapabilityRecord>;
}

const DEMO_USER: MockUser = {
  userId: mockUuid(1),
  orgId: mockUuid(2),
  email: 'demo@markhand.test',
  password: 'demo-password',
  displayName: 'Demo User',
  permissions: ['doc.quarantine.review', 'qa.history'],
  allowedCollectionIds: [mockUuid(10), mockUuid(11)],
};

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
  return map;
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
    documents: seedDocuments(),
    versions: seedVersions(),
    jobs: seedJobs(),
    conflicts: seedConflicts(),
    downloadCapabilities: new Map(),
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

/** Mints and registers a fresh access/refresh token pair for `user`. */
export function mintTokenPair(user: MockUser): TokenPair {
  const accessToken = `mock-access.${nextId()}`;
  const refreshToken = `mock-refresh.${nextId()}`;
  const sessionId = `mock-session.${nextId()}`;
  store.accessTokens.set(accessToken, { userId: user.userId, sessionId });
  store.refreshTokens.set(refreshToken, user.userId);
  return { accessToken, refreshToken, sessionId };
}

/** Looks up the caller (and their session) from an `Authorization: Bearer <token>` header value. */
export function authContextForHeader(
  authorizationHeader: string | null,
): { user: MockUser; sessionId: string } | undefined {
  const match = authorizationHeader ? /^Bearer\s+(.+)$/i.exec(authorizationHeader.trim()) : null;
  if (!match) return undefined;
  const record = store.accessTokens.get(match[1]);
  if (!record) return undefined;
  const user = store.users.find((u) => u.userId === record.userId);
  if (!user) return undefined;
  return { user, sessionId: record.sessionId };
}

export function toMeResponse(user: MockUser, sessionId: string): MeResponse {
  return {
    userId: user.userId,
    orgId: user.orgId,
    email: user.email,
    displayName: user.displayName,
    permissions: user.permissions,
    allowedCollectionIds: user.allowedCollectionIds,
    sessionId,
  };
}

export { encodeCursor, decodeCursor };
