import { readFileSync } from 'node:fs';

const FIXED_SEED_ADMIN_EMAIL = 'admin@poc.example';
const FIXED_SEED_ADMIN_PASSWORD = 'markhand-dev';
const FIXED_SEED_COLLECTION_NAME = 'POC Library';

export interface RuntimeCredentials {
  runId: string;
  adminEmail: string;
  adminPassword: string;
  viewerEmail: string;
  viewerPassword: string;
  objectKeys: string[];
  vectorPointIds: string[];
}

export interface RuntimeFixture {
  runId: string;
  orgId: string;
  adminUserId: string;
  viewerUserId: string;
  collectionId: string;
  collectionName: string;
  failedDocumentId: string;
  failedVersionId: string;
  objectIds: string[];
  vectorPointIds: string[];
  checksum: string;
}

export class RuntimeConfigError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'RuntimeConfigError';
  }
}

type EnvLike = Record<string, string | undefined>;

function requireEnvPath(env: EnvLike, key: string): string {
  const path = env[key]?.trim();
  if (!path) {
    throw new RuntimeConfigError(`${key} is required`);
  }
  return path;
}

function parseJsonFile(path: string): unknown {
  try {
    const raw = readFileSync(path, 'utf8');
    return JSON.parse(raw);
  } catch (error) {
    if (error instanceof SyntaxError) {
      throw new RuntimeConfigError(`malformed JSON in ${path}: ${error.message}`);
    }
    const message = error instanceof Error ? error.message : String(error);
    throw new RuntimeConfigError(`cannot read ${path}: ${message}`);
  }
}

function asRecord(value: unknown, context: string): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new RuntimeConfigError(`${context} file must contain a JSON object`);
  }
  return value as Record<string, unknown>;
}

function requireString(
  obj: Record<string, unknown>,
  field: string,
  context: string,
): string {
  const value = obj[field];
  if (typeof value !== 'string' || value.trim() === '') {
    throw new RuntimeConfigError(
      `${context}: missing or invalid required field "${field}"`,
    );
  }
  return value;
}

function requireStringArray(
  obj: Record<string, unknown>,
  field: string,
  context: string,
): string[] {
  const value = obj[field];
  if (
    !Array.isArray(value) ||
    value.length === 0 ||
    value.some((entry) => typeof entry !== 'string' || entry.trim() === '')
  ) {
    throw new RuntimeConfigError(
      `${context}: missing or invalid required field "${field}"`,
    );
  }
  return value;
}

function rejectFixedSeedCredentials(credentials: RuntimeCredentials): void {
  if (
    credentials.adminEmail === FIXED_SEED_ADMIN_EMAIL ||
    credentials.adminPassword === FIXED_SEED_ADMIN_PASSWORD
  ) {
    throw new RuntimeConfigError(
      'credentials use fixed POC seed values; real E2E requires runtime-generated credentials',
    );
  }
}

function rejectFixedSeedFixture(fixture: RuntimeFixture): void {
  if (fixture.collectionName === FIXED_SEED_COLLECTION_NAME) {
    throw new RuntimeConfigError(
      'fixture uses fixed POC seed collection name; real E2E requires run-scoped fixture',
    );
  }
}

export function loadRuntimeCredentials(env: EnvLike): RuntimeCredentials {
  const path = requireEnvPath(env, 'MARKHAND_E2E_REAL_CREDENTIALS_FILE');
  const obj = asRecord(parseJsonFile(path), 'credentials');
  const credentials: RuntimeCredentials = {
    runId: requireString(obj, 'runId', 'credentials'),
    adminEmail: requireString(obj, 'adminEmail', 'credentials'),
    adminPassword: requireString(obj, 'adminPassword', 'credentials'),
    viewerEmail: requireString(obj, 'viewerEmail', 'credentials'),
    viewerPassword: requireString(obj, 'viewerPassword', 'credentials'),
    objectKeys: requireStringArray(obj, 'objectKeys', 'credentials'),
    vectorPointIds: requireStringArray(obj, 'vectorPointIds', 'credentials'),
  };
  rejectFixedSeedCredentials(credentials);
  return credentials;
}

export function loadRuntimeFixture(env: EnvLike): RuntimeFixture {
  const path = requireEnvPath(env, 'MARKHAND_E2E_REAL_FIXTURE_FILE');
  const obj = asRecord(parseJsonFile(path), 'fixture');
  const fixture: RuntimeFixture = {
    runId: requireString(obj, 'runId', 'fixture'),
    orgId: requireString(obj, 'orgId', 'fixture'),
    adminUserId: requireString(obj, 'adminUserId', 'fixture'),
    viewerUserId: requireString(obj, 'viewerUserId', 'fixture'),
    collectionId: requireString(obj, 'collectionId', 'fixture'),
    collectionName: requireString(obj, 'collectionName', 'fixture'),
    failedDocumentId: requireString(obj, 'failedDocumentId', 'fixture'),
    failedVersionId: requireString(obj, 'failedVersionId', 'fixture'),
    objectIds: requireStringArray(obj, 'objectIds', 'fixture'),
    vectorPointIds: requireStringArray(obj, 'vectorPointIds', 'fixture'),
    checksum: requireString(obj, 'checksum', 'fixture'),
  };
  if (!/^[0-9a-f]{64}$/.test(fixture.checksum)) {
    throw new RuntimeConfigError(
      'fixture: missing or invalid required field "checksum"',
    );
  }
  rejectFixedSeedFixture(fixture);
  return fixture;
}
