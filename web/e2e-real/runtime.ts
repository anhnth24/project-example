// Pure env/JSON parsers for the real-deployment Playwright suite.
// Fail closed: no fixed-seed account or "POC Library" fallback.
import { readFileSync } from 'node:fs';

export type RuntimeCredentials = {
  runId: string;
  adminEmail: string;
  adminPassword: string;
  viewerEmail: string;
  viewerPassword: string;
};

export type RuntimeFixture = {
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
};

type EnvLike = Record<string, string | undefined>;

function requireEnvPath(env: EnvLike, key: string): string {
  const value = env[key]?.trim();
  if (!value) {
    throw new Error(`${key} is required (no fixed-seed fallback)`);
  }
  return value;
}

function readJsonObject(path: string): Record<string, unknown> {
  let raw: string;
  try {
    raw = readFileSync(path, 'utf8');
  } catch (error) {
    throw new Error(`failed to read runtime file at ${path}: ${String(error)}`);
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    throw new Error(`invalid JSON in runtime file at ${path}: ${String(error)}`);
  }
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error(`runtime file at ${path} must be a JSON object`);
  }
  return parsed as Record<string, unknown>;
}

function requireNonEmptyString(payload: Record<string, unknown>, field: string): string {
  const value = payload[field];
  if (typeof value !== 'string' || value.trim() === '') {
    throw new Error(`missing required field: ${field}`);
  }
  return value;
}

function requireStringArray(payload: Record<string, unknown>, field: string): string[] {
  const value = payload[field];
  if (!Array.isArray(value) || value.some((item) => typeof item !== 'string')) {
    throw new Error(`missing required field: ${field}`);
  }
  return value as string[];
}

/** Load runtime credentials from `MARKHAND_E2E_REAL_CREDENTIALS_FILE`. */
export function loadRuntimeCredentials(env: EnvLike): RuntimeCredentials {
  const path = requireEnvPath(env, 'MARKHAND_E2E_REAL_CREDENTIALS_FILE');
  const payload = readJsonObject(path);
  return {
    runId: requireNonEmptyString(payload, 'runId'),
    adminEmail: requireNonEmptyString(payload, 'adminEmail'),
    adminPassword: requireNonEmptyString(payload, 'adminPassword'),
    viewerEmail: requireNonEmptyString(payload, 'viewerEmail'),
    viewerPassword: requireNonEmptyString(payload, 'viewerPassword'),
  };
}

/** Load run-scoped fixture IDs/names from `MARKHAND_E2E_REAL_FIXTURE_FILE`. */
export function loadRuntimeFixture(env: EnvLike): RuntimeFixture {
  const path = requireEnvPath(env, 'MARKHAND_E2E_REAL_FIXTURE_FILE');
  const payload = readJsonObject(path);
  return {
    runId: requireNonEmptyString(payload, 'runId'),
    orgId: requireNonEmptyString(payload, 'orgId'),
    adminUserId: requireNonEmptyString(payload, 'adminUserId'),
    viewerUserId: requireNonEmptyString(payload, 'viewerUserId'),
    collectionId: requireNonEmptyString(payload, 'collectionId'),
    collectionName: requireNonEmptyString(payload, 'collectionName'),
    failedDocumentId: requireNonEmptyString(payload, 'failedDocumentId'),
    failedVersionId: requireNonEmptyString(payload, 'failedVersionId'),
    objectIds: requireStringArray(payload, 'objectIds'),
    vectorPointIds: requireStringArray(payload, 'vectorPointIds'),
    checksum: requireNonEmptyString(payload, 'checksum'),
  };
}
