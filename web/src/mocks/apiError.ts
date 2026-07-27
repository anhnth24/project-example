import type { components } from '../api/generated/contract';
import { nextRequestId } from './ids';

export type ApiErrorBody = components['schemas']['ApiError'];

export interface QuotaDetails {
  limit: number;
  remaining: number;
  resetAt: string;
}

/** Builds the canonical `{ code, message, requestId, details? }` error envelope. */
export function apiError(code: string, message: string, details?: unknown): ApiErrorBody {
  return { code, message, requestId: nextRequestId(), details };
}

export interface MockErrorResponse {
  status: number;
  body: ApiErrorBody;
  headers?: Record<string, string>;
}

export function unauthorized(
  message = 'Authentication required or token invalid/expired.',
): MockErrorResponse {
  return { status: 401, body: apiError('unauthorized', message) };
}

export function forbidden(
  message = 'Caller lacks the permission required for this action.',
): MockErrorResponse {
  return { status: 403, body: apiError('forbidden', message) };
}

export function notFound(
  message = 'The requested resource does not exist or is not visible to the caller.',
): MockErrorResponse {
  return { status: 404, body: apiError('not_found', message) };
}

export function conflict(
  message = 'The request conflicts with the current state of the resource.',
): MockErrorResponse {
  return { status: 409, body: apiError('conflict', message) };
}

export function serviceUnavailable(
  message = 'A required dependency is not currently reachable.',
): MockErrorResponse {
  return { status: 503, body: apiError('service_unavailable', message) };
}

/** 429 with the quota metadata the spec's "Ground truth" calls out, carried in `details`. */
export function rateLimited(quota: Partial<QuotaDetails> = {}): MockErrorResponse {
  const details: QuotaDetails = {
    limit: quota.limit ?? 60,
    remaining: quota.remaining ?? 0,
    resetAt: quota.resetAt ?? new Date(Date.now() + 30_000).toISOString(),
  };
  const retryAfterSeconds = Math.max(
    1,
    Math.round((Date.parse(details.resetAt) - Date.now()) / 1000),
  );
  return {
    status: 429,
    body: apiError('rate_limited', 'Too many requests; quota exhausted.', details),
    headers: { 'Retry-After': String(retryAfterSeconds) },
  };
}
