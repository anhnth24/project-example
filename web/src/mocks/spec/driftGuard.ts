import { getSpecIndex } from './openApiSpec';
import { validateAgainstSchema } from '../schema/validate';

/**
 * The spec-drift guard the P2-02 task asks for: every operationId this mock
 * claims to implement must actually exist in `openapi.yaml`, and every status
 * code / JSON body it emits for that operation must be one the spec declares.
 * If the spec drops an operation, a status code, or tightens a schema, these
 * throw instead of the mock quietly answering for an API that no longer
 * exists — see `specDrift.test.ts` for a live demonstration.
 */

export function assertOperationsExistInSpec(operationIds: readonly string[]): void {
  const spec = getSpecIndex();
  for (const operationId of operationIds) {
    if (!spec.operations[operationId]) {
      throw new Error(
        `mocks drift-guard: operationId "${operationId}" is registered as a mock handler but ` +
          `does not exist in crates/server/openapi/openapi.yaml. The spec removed/renamed this ` +
          `operation — remove or rename the corresponding handler in web/src/mocks/handlers/.`,
      );
    }
  }
}

export function assertStatusDeclared(operationId: string, status: number): void {
  const op = getSpecIndex().operations[operationId];
  if (!op) {
    throw new Error(`mocks drift-guard: operationId "${operationId}" does not exist in the spec.`);
  }
  if (!(String(status) in op.responses)) {
    throw new Error(
      `mocks drift-guard: operation "${operationId}" (${op.method.toUpperCase()} ${op.path}) returned ` +
        `status ${status}, but the spec only declares [${Object.keys(op.responses).join(', ')}] for it.`,
    );
  }
}

export function assertJsonBodyMatchesSpec(
  operationId: string,
  status: number,
  body: unknown,
): void {
  const op = getSpecIndex().operations[operationId];
  const schema = op?.responses[String(status)]?.content['application/json'];
  if (!schema) return; // no JSON schema declared for this status (e.g. 204, or a non-JSON media type)
  const errors = validateAgainstSchema(schema, body);
  if (errors.length > 0) {
    throw new Error(
      `mocks drift-guard: response body for ${operationId} (${status}) does not match its spec schema:\n  ${errors.join('\n  ')}`,
    );
  }
}
