/**
 * Failure-on-demand controls for the mock. Tests (or a dev-mode UI toggle,
 * once someone wires that up) call `mockControl.forceStatus(...)` before
 * making a request to exercise the SPA's 401/403/404/409/429/503 paths without
 * needing a real backend that can be coerced into those states.
 */
import type { MockErrorResponse } from './apiError';
import {
  conflict,
  forbidden,
  notFound,
  rateLimited,
  serviceUnavailable,
  unauthorized,
} from './apiError';
import type { QuotaDetails } from './apiError';

export type ForcedFailureKind = 401 | 403 | 404 | 409 | 429 | 503;

interface ForcedFailure {
  kind: ForcedFailureKind;
  quota?: Partial<QuotaDetails>;
  /** If set, only the next N matching requests fail; omitted/undefined means "until reset". */
  remainingUses?: number;
}

// Keyed by operationId; "*" applies to every operation.
const forced = new Map<string, ForcedFailure>();

export const mockControl = {
  /**
   * Force the next call(s) to `operationId` (or every operation, via "*") to
   * fail with `kind`. `times` limits how many calls fail before it clears
   * itself; omit for "fails until `reset()`/`clear()`".
   */
  forceStatus(
    operationId: string | '*',
    kind: ForcedFailureKind,
    opts: { times?: number; quota?: Partial<QuotaDetails> } = {},
  ): void {
    forced.set(operationId, { kind, quota: opts.quota, remainingUses: opts.times });
  },

  clear(operationId: string | '*'): void {
    forced.delete(operationId);
  },

  reset(): void {
    forced.clear();
  },

  /** Looks up (and consumes one use of) a forced failure for this operationId, if any. */
  consume(operationId: string): MockErrorResponse | undefined {
    const entry = forced.get(operationId) ?? forced.get('*');
    if (!entry) return undefined;
    const key = forced.has(operationId) ? operationId : '*';
    if (entry.remainingUses !== undefined) {
      entry.remainingUses -= 1;
      if (entry.remainingUses <= 0) forced.delete(key);
    }
    switch (entry.kind) {
      case 401:
        return unauthorized();
      case 403:
        return forbidden();
      case 404:
        return notFound();
      case 409:
        return conflict();
      case 429:
        return rateLimited(entry.quota);
      case 503:
        return serviceUnavailable();
    }
  },
};
