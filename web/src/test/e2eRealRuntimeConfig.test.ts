import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  loadRuntimeCredentials,
  loadRuntimeFixture,
  RuntimeConfigError,
} from '../../e2e-real/runtime';

function tempDir(): string {
  return mkdtempSync(join(tmpdir(), 'markhand-e2e-runtime-'));
}

function writeJson(path: string, payload: unknown): void {
  writeFileSync(path, JSON.stringify(payload), 'utf8');
}

const validCredentials = {
  runId: 'e2e-run-1',
  adminEmail: 'admin+e2e-run-1@example.test',
  adminPassword: 'runtime-admin-secret',
  viewerEmail: 'viewer+e2e-run-1@example.test',
  viewerPassword: 'runtime-viewer-secret',
  objectKeys: ['org/object-key'],
  vectorPointIds: ['vector-point-1'],
};

const validFixture = {
  runId: 'e2e-run-1',
  orgId: 'org-uuid',
  adminUserId: 'admin-uuid',
  viewerUserId: 'viewer-uuid',
  collectionId: 'collection-uuid',
  collectionName: 'E2E Library e2e-run-1',
  failedDocumentId: 'failed-doc-uuid',
  failedVersionId: 'failed-ver-uuid',
  objectIds: ['object-uuid'],
  vectorPointIds: ['vector-point-1'],
  checksum: 'a'.repeat(64),
};

describe('loadRuntimeCredentials', () => {
  it('fails closed when MARKHAND_E2E_REAL_CREDENTIALS_FILE is unset', () => {
    expect(() => loadRuntimeCredentials({})).toThrow(RuntimeConfigError);
    expect(() => loadRuntimeCredentials({})).toThrow(/MARKHAND_E2E_REAL_CREDENTIALS_FILE/);
  });

  it('fails closed when credentials path does not exist', () => {
    const path = join(tempDir(), 'missing-credentials.json');
    expect(() => loadRuntimeCredentials({ MARKHAND_E2E_REAL_CREDENTIALS_FILE: path })).toThrow(
      RuntimeConfigError,
    );
    expect(() => loadRuntimeCredentials({ MARKHAND_E2E_REAL_CREDENTIALS_FILE: path })).toThrow(
      /cannot read/,
    );
  });

  it('fails on malformed JSON', () => {
    const dir = tempDir();
    const path = join(dir, 'credentials.json');
    writeFileSync(path, '{not json', 'utf8');

    expect(() => loadRuntimeCredentials({ MARKHAND_E2E_REAL_CREDENTIALS_FILE: path })).toThrow(
      RuntimeConfigError,
    );
    expect(() => loadRuntimeCredentials({ MARKHAND_E2E_REAL_CREDENTIALS_FILE: path })).toThrow(
      /malformed JSON/,
    );
  });

  it('fails when required credential fields are missing', () => {
    const dir = tempDir();
    const path = join(dir, 'credentials.json');
    writeJson(path, { runId: 'e2e-run-1', adminEmail: 'admin@example.test' });

    expect(() => loadRuntimeCredentials({ MARKHAND_E2E_REAL_CREDENTIALS_FILE: path })).toThrow(
      RuntimeConfigError,
    );
    expect(() => loadRuntimeCredentials({ MARKHAND_E2E_REAL_CREDENTIALS_FILE: path })).toThrow(
      /adminPassword/,
    );
  });

  it('rejects fixed POC seed admin email instead of inventing a fallback', () => {
    const dir = tempDir();
    const path = join(dir, 'credentials.json');
    writeJson(path, {
      ...validCredentials,
      adminEmail: 'admin@poc.example',
    });

    expect(() => loadRuntimeCredentials({ MARKHAND_E2E_REAL_CREDENTIALS_FILE: path })).toThrow(
      RuntimeConfigError,
    );
    expect(() => loadRuntimeCredentials({ MARKHAND_E2E_REAL_CREDENTIALS_FILE: path })).toThrow(
      /fixed POC seed/,
    );
  });

  it('rejects fixed POC seed admin password instead of inventing a fallback', () => {
    const dir = tempDir();
    const path = join(dir, 'credentials.json');
    writeJson(path, {
      ...validCredentials,
      adminPassword: 'markhand-dev',
    });

    expect(() => loadRuntimeCredentials({ MARKHAND_E2E_REAL_CREDENTIALS_FILE: path })).toThrow(
      RuntimeConfigError,
    );
    expect(() => loadRuntimeCredentials({ MARKHAND_E2E_REAL_CREDENTIALS_FILE: path })).toThrow(
      /fixed POC seed/,
    );
  });

  it('loads valid runtime credentials from the configured path', () => {
    const dir = tempDir();
    const path = join(dir, 'credentials.json');
    writeJson(path, validCredentials);

    expect(loadRuntimeCredentials({ MARKHAND_E2E_REAL_CREDENTIALS_FILE: path })).toEqual(
      validCredentials,
    );
  });
});

describe('loadRuntimeFixture', () => {
  it('fails closed when MARKHAND_E2E_REAL_FIXTURE_FILE is unset', () => {
    expect(() => loadRuntimeFixture({})).toThrow(RuntimeConfigError);
    expect(() => loadRuntimeFixture({})).toThrow(/MARKHAND_E2E_REAL_FIXTURE_FILE/);
  });

  it('fails closed when fixture path does not exist', () => {
    const path = join(tempDir(), 'missing-fixture.json');
    expect(() => loadRuntimeFixture({ MARKHAND_E2E_REAL_FIXTURE_FILE: path })).toThrow(
      RuntimeConfigError,
    );
    expect(() => loadRuntimeFixture({ MARKHAND_E2E_REAL_FIXTURE_FILE: path })).toThrow(
      /cannot read/,
    );
  });

  it('fails on malformed JSON', () => {
    const dir = tempDir();
    const path = join(dir, 'fixture.json');
    writeFileSync(path, '[]', 'utf8');

    expect(() => loadRuntimeFixture({ MARKHAND_E2E_REAL_FIXTURE_FILE: path })).toThrow(
      RuntimeConfigError,
    );
  });

  it('fails when required fixture fields are missing', () => {
    const dir = tempDir();
    const path = join(dir, 'fixture.json');
    writeJson(path, { runId: 'e2e-run-1', orgId: 'org-uuid' });

    expect(() => loadRuntimeFixture({ MARKHAND_E2E_REAL_FIXTURE_FILE: path })).toThrow(
      RuntimeConfigError,
    );
    expect(() => loadRuntimeFixture({ MARKHAND_E2E_REAL_FIXTURE_FILE: path })).toThrow(
      /missing or invalid required field/,
    );
  });

  it('rejects fixed POC seed collection name instead of inventing a fallback', () => {
    const dir = tempDir();
    const path = join(dir, 'fixture.json');
    writeJson(path, {
      ...validFixture,
      collectionName: 'POC Library',
    });

    expect(() => loadRuntimeFixture({ MARKHAND_E2E_REAL_FIXTURE_FILE: path })).toThrow(
      RuntimeConfigError,
    );
    expect(() => loadRuntimeFixture({ MARKHAND_E2E_REAL_FIXTURE_FILE: path })).toThrow(
      /fixed POC seed/,
    );
  });

  it('loads valid runtime fixture from the configured path', () => {
    const dir = tempDir();
    const path = join(dir, 'fixture.json');
    writeJson(path, validFixture);

    expect(loadRuntimeFixture({ MARKHAND_E2E_REAL_FIXTURE_FILE: path })).toEqual(validFixture);
  });
});
