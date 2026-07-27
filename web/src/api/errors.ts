// Normalized error types for every HTTP call the SPA makes. Everything the
// server can return is typed against `generated/contract.ts` — nothing here
// re-declares a shape the OpenAPI contract already owns.
import type { components } from './generated/contract';

/** Canonical error envelope every non-2xx JSON response carries. */
export type ApiErrorBody = components['schemas']['ApiError'];

/**
 * Quota metadata surfaced from a 429. `retryAfterSeconds` is read from the
 * `Retry-After` response header (the only header `rate_limit_guard.rs`
 * actually sets); `scope`/`quota` come from `ApiError.details`, which the
 * guard populates as `{ retryAfterSeconds, scope, quota }` but which the
 * OpenAPI contract types only as `unknown`, so both are read defensively.
 */
export interface RateLimitInfo {
  retryAfterSeconds: number;
  scope?: string;
  quota?: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

/** Runtime guard for the canonical error envelope — the wire body is `unknown` until checked. */
export function isApiErrorBody(value: unknown): value is ApiErrorBody {
  return (
    isRecord(value) &&
    typeof value.code === 'string' &&
    typeof value.message === 'string' &&
    typeof value.requestId === 'string'
  );
}

function readRateLimitDetails(details: unknown): { scope?: string; quota?: string } {
  if (!isRecord(details)) return {};
  const scope = typeof details.scope === 'string' ? details.scope : undefined;
  const quota = typeof details.quota === 'string' ? details.quota : undefined;
  return { scope, quota };
}

/** Parses `Retry-After` as whole seconds; the guard always sends an integer, but we guard anyway. */
export function parseRetryAfterHeader(response: Response): number | undefined {
  const raw = response.headers.get('Retry-After');
  if (raw === null) return undefined;
  const seconds = Number.parseInt(raw, 10);
  return Number.isFinite(seconds) && seconds >= 0 ? seconds : undefined;
}

export function extractRateLimitInfo(
  response: Response,
  body: ApiErrorBody | null,
): RateLimitInfo | undefined {
  if (response.status !== 429) return undefined;
  const headerSeconds = parseRetryAfterHeader(response);
  const { scope, quota } = readRateLimitDetails(body?.details);
  if (headerSeconds === undefined && scope === undefined && quota === undefined) {
    return undefined;
  }
  return { retryAfterSeconds: headerSeconds ?? 0, scope, quota };
}

/**
 * A non-2xx response the server answered with, normalized to
 * `{code,message,requestId,details?}` plus HTTP status and (for 429) quota
 * metadata. Thrown by every request path in `client.ts`, including the
 * refresh call itself.
 */
export class HttpApiError extends Error {
  readonly name = 'HttpApiError';
  readonly status: number;
  readonly code: string;
  readonly requestId: string;
  readonly details?: unknown;
  readonly rateLimit?: RateLimitInfo;

  constructor(params: {
    status: number;
    code: string;
    message: string;
    requestId: string;
    details?: unknown;
    rateLimit?: RateLimitInfo;
  }) {
    super(params.message);
    this.status = params.status;
    this.code = params.code;
    this.requestId = params.requestId;
    this.details = params.details;
    this.rateLimit = params.rateLimit;
  }
}

/** `fetch` itself rejected (offline, DNS, CORS, connection reset — no HTTP response to normalize). */
export class NetworkError extends Error {
  readonly name = 'NetworkError';
  readonly cause?: unknown;

  constructor(message: string, options?: { cause?: unknown }) {
    super(message);
    this.cause = options?.cause;
  }
}

/** True for the `DOMException` `fetch`/`AbortController` produce on cancellation. */
export function isAbortError(value: unknown): value is DOMException {
  return value instanceof DOMException && value.name === 'AbortError';
}

async function readJsonBodyOrNull(response: Response): Promise<unknown> {
  try {
    const text = await response.text();
    return text.length > 0 ? JSON.parse(text) : null;
  } catch {
    return null;
  }
}

/**
 * Builds a `HttpApiError` from a non-ok `Response`. Reads the body defensively:
 * a body that isn't the documented `ApiError` JSON shape (proxy error pages,
 * empty 5xx bodies, etc.) still produces a usable error instead of throwing
 * out of the error path itself.
 */
export async function normalizeErrorResponse(response: Response): Promise<HttpApiError> {
  const parsed = await readJsonBodyOrNull(response);
  const body = isApiErrorBody(parsed) ? parsed : null;
  const rateLimit = extractRateLimitInfo(response, body);
  return new HttpApiError({
    status: response.status,
    code: body?.code ?? 'unknown_error',
    message: body?.message ?? response.statusText ?? `HTTP ${response.status}`,
    requestId: body?.requestId ?? '',
    details: body?.details,
    rateLimit,
  });
}
