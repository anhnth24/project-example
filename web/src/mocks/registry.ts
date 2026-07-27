import type { HttpMethod } from './spec/openApiSpec';

export interface MockRequestContext {
  method: HttpMethod;
  /** Path params extracted from the matched `{param}` segments of the operation's path template. */
  params: Record<string, string>;
  query: URLSearchParams;
  headers: Headers;
  /** Parses the request body as JSON. Throws if the body isn't valid JSON. */
  json: <T = unknown>() => Promise<T>;
  /** Available for the one multipart operation (`createUpload`). */
  formData: () => Promise<FormData>;
  request: Request;
}

export interface MockHandlerResult {
  status: number;
  /** JSON-serializable response body. Omit for 204/empty responses. */
  body?: unknown;
  /** Use for non-JSON bodies (redeemDownload's markdown/binary, openapiYaml's raw text). */
  rawBody?: { text: string; contentType: string };
  headers?: Record<string, string>;
}

export type MockHandler = (
  ctx: MockRequestContext,
) => Promise<MockHandlerResult> | MockHandlerResult;

export interface RegisteredOperation {
  operationId: string;
  handle: MockHandler;
}

/**
 * Central registry of every operation the mock implements, keyed by
 * operationId. Route templates, methods, and declared status codes are never
 * duplicated here — they're looked up from the parsed spec
 * (`spec/openApiSpec.ts`) by `fetchMock.ts` and by `specDrift.test.ts`, using
 * this map's keys as the only "which operations do we mock" source of truth.
 */
const registry = new Map<string, MockHandler>();

export function registerOperation(operationId: string, handle: MockHandler): void {
  if (registry.has(operationId)) {
    throw new Error(`mocks/registry: operationId "${operationId}" is already registered`);
  }
  registry.set(operationId, handle);
}

export function getRegisteredOperations(): RegisteredOperation[] {
  return [...registry.entries()].map(([operationId, handle]) => ({ operationId, handle }));
}

export function getHandler(operationId: string): MockHandler | undefined {
  return registry.get(operationId);
}

/** Operations the spec declares that this mock deliberately does not implement (see README/report: SSE streaming). */
export const DELIBERATELY_UNMOCKED_OPERATIONS = ['jobEvents', 'askStream'] as const;
