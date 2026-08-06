// Contract tests for pure real-E2E runtime credential/fixture parsers
// (`web/e2e-real/runtime.ts`). Fail closed: missing paths, bad JSON, and
// missing required fields must throw — never fall back to the old fixed POC
// seed account / "POC Library" collection.
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { loadRuntimeCredentials, loadRuntimeFixture } from '../../e2e-real/runtime';

const FIXED_SEED_EMAIL = 'admin@poc.example';
const FIXED_SEED_COLLECTION = 'POC Library';

let tempDir: string | undefined;

afterEach(() => {
  if (tempDir) {
    rmSync(tempDir, { recursive: true, force: true });
    tempDir = undefined;
  }
});

function writeJson(name: string, value: unknown, mode = 0o600): string {
  tempDir ??= mkdtempSync(join(tmpdir(), 'e2e-real-runtime-'));
  const path = join(tempDir, name);
  writeFileSync(path, JSON.stringify(value), { encoding: 'utf8', mode });
  chmodSync(path, mode);
  return path;
}

const validCredentials = {
  runId: 'e2e-abcdef012345-1',
  adminEmail: 'admin+e2e-abcdef012345-1@example.test',
  adminPassword: 'runtime-admin-secret',
  viewerEmail: 'viewer+e2e-abcdef012345-1@example.test',
  viewerPassword: 'runtime-viewer-secret',
};

const validFixture = {
  runId: 'e2e-abcdef012345-1',
  orgId: 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaa1',
  adminUserId: 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbb1',
  viewerUserId: 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbb2',
  collectionId: 'cccccccc-cccc-cccc-cccc-ccccccccccc1',
  collectionName: 'E2E Library e2e-abcdef012345-1',
  failedDocumentId: 'dddddddd-dddd-dddd-dddd-ddddddddddd1',
  failedVersionId: 'dddddddd-dddd-dddd-dddd-ddddddddddd2',
  objectIds: ['eeeeeeee-eeee-eeee-eeee-eeeeeeeeeee1'],
  vectorPointIds: ['ffffffff-ffff-ffff-ffff-fffffffffff1'],
  checksum: 'deadbeef',
};

describe('loadRuntimeCredentials', () => {
  it('rejects a missing MARKHAND_E2E_REAL_CREDENTIALS_FILE path', () => {
    expect(() => loadRuntimeCredentials({})).toThrow(/MARKHAND_E2E_REAL_CREDENTIALS_FILE/);
    expect(() =>
      loadRuntimeCredentials({
        MARKHAND_E2E_REAL_CREDENTIALS_FILE: '/tmp/does-not-exist-credentials.json',
      }),
    ).toThrow();
  });

  it('rejects malformed JSON', () => {
    tempDir ??= mkdtempSync(join(tmpdir(), 'e2e-real-runtime-'));
    const path = join(tempDir, 'bad-credentials.json');
    writeFileSync(path, '{not-json', { encoding: 'utf8', mode: 0o600 });
    chmodSync(path, 0o600);

    expect(() => loadRuntimeCredentials({ MARKHAND_E2E_REAL_CREDENTIALS_FILE: path })).toThrow(
      /json/i,
    );
  });

  it('rejects credentials missing required fields', () => {
    const path = writeJson('incomplete-credentials.json', {
      runId: 'e2e-abcdef012345-1',
      adminEmail: 'admin+e2e@example.test',
      // adminPassword missing
      viewerEmail: 'viewer+e2e@example.test',
      viewerPassword: 'viewer-secret',
    });

    expect(() => loadRuntimeCredentials({ MARKHAND_E2E_REAL_CREDENTIALS_FILE: path })).toThrow(
      /adminPassword/,
    );
  });

  it('rejects fixed-seed fallback when the credentials file env is unset', () => {
    let thrown: unknown;
    try {
      loadRuntimeCredentials({});
    } catch (error) {
      thrown = error;
    }
    expect(thrown).toBeInstanceOf(Error);
    const message = String(thrown);
    expect(message).not.toMatch(new RegExp(FIXED_SEED_EMAIL.replace('.', '\\.')));
    expect(() => loadRuntimeCredentials({})).toThrow(/MARKHAND_E2E_REAL_CREDENTIALS_FILE/);
  });

  it('loads runtime credentials from a mode-0600 JSON file', () => {
    const path = writeJson('credentials.json', validCredentials, 0o600);
    const loaded = loadRuntimeCredentials({
      MARKHAND_E2E_REAL_CREDENTIALS_FILE: path,
    });
    expect(loaded.adminEmail).toBe(validCredentials.adminEmail);
    expect(loaded.adminPassword).toBe(validCredentials.adminPassword);
    expect(loaded.viewerEmail).toBe(validCredentials.viewerEmail);
    expect(loaded.viewerPassword).toBe(validCredentials.viewerPassword);
    expect(loaded.runId).toBe(validCredentials.runId);
    expect(loaded.adminEmail).not.toBe(FIXED_SEED_EMAIL);
  });
});

describe('loadRuntimeFixture', () => {
  it('rejects a missing MARKHAND_E2E_REAL_FIXTURE_FILE path', () => {
    expect(() => loadRuntimeFixture({})).toThrow(/MARKHAND_E2E_REAL_FIXTURE_FILE/);
    expect(() =>
      loadRuntimeFixture({
        MARKHAND_E2E_REAL_FIXTURE_FILE: '/tmp/does-not-exist-fixture.json',
      }),
    ).toThrow();
  });

  it('rejects malformed JSON', () => {
    tempDir ??= mkdtempSync(join(tmpdir(), 'e2e-real-runtime-'));
    const path = join(tempDir, 'bad-fixture.json');
    writeFileSync(path, 'not-json-at-all', { encoding: 'utf8' });

    expect(() => loadRuntimeFixture({ MARKHAND_E2E_REAL_FIXTURE_FILE: path })).toThrow(/json/i);
  });

  it('rejects fixtures missing required fields', () => {
    const path = writeJson('incomplete-fixture.json', {
      ...validFixture,
      collectionName: '',
    });

    expect(() => loadRuntimeFixture({ MARKHAND_E2E_REAL_FIXTURE_FILE: path })).toThrow(
      /collectionName/,
    );
  });

  it('rejects fixed-seed fallback when the fixture file env is unset', () => {
    expect(() => loadRuntimeFixture({})).toThrow(/MARKHAND_E2E_REAL_FIXTURE_FILE/);
    let thrown: unknown;
    try {
      loadRuntimeFixture({});
    } catch (error) {
      thrown = error;
    }
    expect(String(thrown)).not.toContain(FIXED_SEED_COLLECTION);
  });

  it('loads run-scoped fixture names and ids', () => {
    const path = writeJson('fixture.json', validFixture, 0o644);
    const loaded = loadRuntimeFixture({
      MARKHAND_E2E_REAL_FIXTURE_FILE: path,
    });
    expect(loaded.collectionName).toBe(validFixture.collectionName);
    expect(loaded.collectionId).toBe(validFixture.collectionId);
    expect(loaded.runId).toBe(validFixture.runId);
    expect(loaded.collectionName).not.toBe(FIXED_SEED_COLLECTION);
  });
});
