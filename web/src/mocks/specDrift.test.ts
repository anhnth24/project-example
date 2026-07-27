import { describe, expect, it } from 'vitest';
import {
  assertOperationsExistInSpec,
  assertStatusDeclared,
  assertJsonBodyMatchesSpec,
} from './spec/driftGuard';
import { getRegisteredOperations, registerOperation } from './registry';
import { installMockFetch, uninstallMockFetch } from './fetchMock';

/**
 * Demonstrates the spec-drift guard the P2-02 task asks for: "at minimum, a
 * check that every mocked operation exists in the spec with the status code
 * you return, so an endpoint that disappears from the spec breaks the mock
 * instead of lingering."
 */
describe('spec-drift guard', () => {
  it('passes for every operation this mock actually implements today', () => {
    const operationIds = getRegisteredOperations().map((r) => r.operationId);
    expect(operationIds.length).toBeGreaterThan(0);
    expect(() => assertOperationsExistInSpec(operationIds)).not.toThrow();
  });

  it('FAILS when pointed at an operationId the spec does not declare', () => {
    expect(() =>
      assertOperationsExistInSpec(['listCollections', 'thisOperationWasRemovedFromTheSpec']),
    ).toThrow(
      /operationId "thisOperationWasRemovedFromTheSpec".*does not exist in crates\/server\/openapi\/openapi\.yaml/s,
    );
  });

  it('FAILS when a handler returns a status code its operation does not declare', () => {
    // listCollections only declares 200 and 429 (see openapi.yaml) — 500 isn't one of them.
    expect(() => assertStatusDeclared('listCollections', 500)).toThrow(
      /operation "listCollections".*returned status 500.*only declares \[200, 429\]/s,
    );
  });

  it('FAILS when a response body is missing a field the spec requires', () => {
    expect(() =>
      assertJsonBodyMatchesSpec('healthLive', 200, { status: 'ok' /* missing requestId */ }),
    ).toThrow(/does not match its spec schema/);
    expect(() =>
      assertJsonBodyMatchesSpec('healthLive', 200, {
        status: 'ok',
        requestId: '11111111-1111-1111-1111-111111111111',
      }),
    ).not.toThrow();
  });

  it('FAILS end-to-end through the real mock pipeline for an operation the registry has but the spec dropped', async () => {
    // Simulate exactly the scenario the task describes: a handler that still
    // exists in web/src/mocks/handlers/** for an operation the spec no longer
    // has (renamed/removed upstream). Register it directly against the live
    // registry (this test file's own isolated module instance) so the mock's
    // real route-compilation path — the one every other test in this package
    // goes through via `installMockFetch()` — is what actually throws, not a
    // hand-picked assertion.
    registerOperation('anOperationThatIsNotInOpenapiYaml', () => ({
      status: 200,
      body: { ok: true },
    }));

    installMockFetch();
    try {
      await expect(fetch('/api/v1/anything')).rejects.toThrow(
        /operationId "anOperationThatIsNotInOpenapiYaml".*is registered as a mock handler but.*does not exist/s,
      );
    } finally {
      uninstallMockFetch();
    }
  });
});
