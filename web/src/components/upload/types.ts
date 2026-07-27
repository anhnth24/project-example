// Shared types for the upload panel (P2-08, plans/markhand-web/phase-2-web-spa.md
// §P2.4). Kept free of React so `uploadTransport.ts`/`jobLifecycle.ts` stay
// unit-testable in isolation.
import type { components } from '../../api/generated/contract';

export type Job = components['schemas']['Job'];
export type ApiErrorBody = components['schemas']['ApiError'];

/**
 * The 201 body `POST /uploads` returns — see `createUpload` in
 * `api/generated/contract.ts`. Hand-declared (not imported from the
 * generated `operations["createUpload"]["responses"][201]` type) only
 * because that type is an inline object literal with no exported name to
 * reference; the fields below are copied verbatim from it.
 */
export interface UploadSuccessBody {
  disposition: string;
  objectId: string;
  documentId?: string;
  versionId?: string;
  jobId?: string;
  collectionId?: string;
  sha256: string;
  sizeBytes: number;
  canonicalFormat: string;
  requestId: string;
}

/**
 * Quota metadata for a 429 on `POST /uploads`. The contract types
 * `ApiError.details` as `unknown`, and this codebase's own mocks
 * (`mocks/apiError.ts`'s `rateLimited()`) and its runtime error reader
 * (`api/errors.ts`'s `readRateLimitDetails`) don't even agree with each
 * other on `details`' shape ({limit,remaining,resetAt} vs {scope,quota}).
 * [Unverified] which shape the real server sends. `details` is therefore
 * kept as `unknown` here too and only ever read defensively.
 */
export interface UploadRateLimitInfo {
  retryAfterSeconds: number | undefined;
  details: unknown;
}

/** Every way an upload attempt can conclude. Exactly one variant per settle. */
export type UploadOutcome =
  | { kind: 'success'; body: UploadSuccessBody }
  | { kind: 'conflict'; error: ApiErrorBody | null }
  | { kind: 'too-large'; error: ApiErrorBody | null }
  | { kind: 'quota'; error: ApiErrorBody | null; rateLimit: UploadRateLimitInfo }
  | { kind: 'forbidden'; error: ApiErrorBody | null }
  | { kind: 'session-lost' }
  | { kind: 'network-error' }
  | { kind: 'aborted' }
  | { kind: 'http-error'; status: number; error: ApiErrorBody | null };

export interface UploadProgress {
  loaded: number;
  /** `undefined` means the browser could not compute a length (`lengthComputable` was false) — render an indeterminate state, never a fabricated percentage. */
  total: number | undefined;
}
