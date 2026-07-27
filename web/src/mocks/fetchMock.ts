/**
 * Installs a fetch-level mock: overrides `globalThis.fetch` so any code using
 * the standard `fetch()` API (the SPA's real `api/client.ts`, or a test) gets
 * routed to the handlers in `handlers/*` instead of hitting a network. See the
 * P2-02 report for why this shape was chosen over a standalone mock server.
 */
import './handlers'; // registers every operation as a side effect
import { DELIBERATELY_UNMOCKED_OPERATIONS, getHandler, getRegisteredOperations } from './registry';
import type { MockRequestContext } from './registry';
import { getSpecIndex, type OperationDef } from './spec/openApiSpec';
import {
  assertJsonBodyMatchesSpec,
  assertOperationsExistInSpec,
  assertStatusDeclared,
} from './spec/driftGuard';
import { mockControl } from './control';
import { authContextForHeader, resetMockStore } from './fixtures';
import { resetRequestIdCounter } from './ids';
import { unauthorized } from './apiError';

/** Matches the app's own convention (see `src/api/health.ts`): API routes live under `/api/v1`. */
const API_PREFIX = '/api/v1';

interface CompiledRoute {
  operationId: string;
  method: string;
  regex: RegExp;
  paramNames: string[];
}

function pathToRegex(path: string): { regex: RegExp; paramNames: string[] } {
  const paramNames: string[] = [];
  const escaped = path
    .split(/(\{[^}]+\})/)
    .map((segment) => {
      const paramMatch = /^\{([^}]+)\}$/.exec(segment);
      if (paramMatch) {
        paramNames.push(paramMatch[1]);
        return '([^/]+)';
      }
      return segment.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    })
    .join('');
  return { regex: new RegExp(`^${escaped}$`), paramNames };
}

let compiledRoutes: CompiledRoute[] | undefined;

function getRoutes(): CompiledRoute[] {
  if (compiledRoutes) return compiledRoutes;
  const registered = getRegisteredOperations();
  assertOperationsExistInSpec(registered.map((r) => r.operationId));
  const spec = getSpecIndex();
  compiledRoutes = registered.map(({ operationId }) => {
    const op = spec.operations[operationId];
    const { regex, paramNames } = pathToRegex(op.path);
    return { operationId, method: op.method, regex, paramNames };
  });
  return compiledRoutes;
}

/** Forces route recompilation on next request — used by tests that mutate the registry. */
export function invalidateRouteCache(): void {
  compiledRoutes = undefined;
}

function buildResponse(
  status: number,
  body: unknown,
  headers?: Record<string, string>,
  rawBody?: { text: string; contentType: string },
): Response {
  const responseHeaders = new Headers(headers);
  if (rawBody) {
    responseHeaders.set('content-type', rawBody.contentType);
    return new Response(rawBody.text, { status, headers: responseHeaders });
  }
  if (body === undefined) {
    return new Response(null, { status, headers: responseHeaders });
  }
  responseHeaders.set('content-type', 'application/json');
  return new Response(JSON.stringify(body), { status, headers: responseHeaders });
}

function emit(
  operationId: string,
  status: number,
  body: unknown,
  headers?: Record<string, string>,
  rawBody?: { text: string; contentType: string },
): Response {
  assertStatusDeclared(operationId, status);
  if (!rawBody && body !== undefined) {
    assertJsonBodyMatchesSpec(operationId, status, body);
  }
  return buildResponse(status, body, headers, rawBody);
}

function findDeclaredButUnmocked(method: string, pathname: string): OperationDef | undefined {
  const spec = getSpecIndex();
  for (const operationId of DELIBERATELY_UNMOCKED_OPERATIONS) {
    const op = spec.operations[operationId as string];
    if (!op || op.method !== method.toLowerCase()) continue;
    const { regex } = pathToRegex(op.path);
    if (regex.test(pathname)) return op;
  }
  return undefined;
}

/**
 * A fallback base for resolving relative URLs (`fetch('/api/v1/...')`).
 * Node's native `fetch`/`Request`/`URL` (what `vitest`'s "jsdom" environment
 * actually uses under the hood — jsdom itself ships no fetch implementation)
 * has no notion of "the page's origin" the way a real browser does, so a bare
 * `fetch('/api/v1/health/live')` throws `ERR_INVALID_URL` unless resolved
 * against *some* base first. The base's value is never observable by
 * handlers (they only see `pathname`/`searchParams`).
 */
const RELATIVE_URL_BASE = 'http://mock.local/';

async function mockFetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  // If `input` is already a Request/absolute-URL, its URL was already valid
  // when *it* was constructed, so only bare-string/URL-object input can be
  // relative here.
  const absoluteUrl =
    input instanceof Request ? input.url : new URL(input.toString(), RELATIVE_URL_BASE).toString();
  const request =
    input instanceof Request && init === undefined ? input : new Request(absoluteUrl, init);
  const url = new URL(request.url);
  const pathname = url.pathname.startsWith(API_PREFIX)
    ? url.pathname.slice(API_PREFIX.length) || '/'
    : url.pathname;
  const method = request.method.toUpperCase();

  const routes = getRoutes();
  const match = routes.find((r) => r.method === method.toLowerCase() && r.regex.test(pathname));

  if (!match) {
    const unmocked = findDeclaredButUnmocked(method, pathname);
    if (unmocked) {
      throw new Error(
        `mocks/fetchMock: ${method} ${pathname} ("${unmocked.operationId}") is declared in the spec but ` +
          `deliberately not mocked here — it's an SSE streaming endpoint. See the P2-02 report for the ` +
          `rationale (web/src/mocks is fetch-level; streaming needs a real ReadableStream mock, out of scope).`,
      );
    }
    throw new Error(`mocks/fetchMock: no mock handler matches ${method} ${pathname}`);
  }

  const op = getSpecIndex().operations[match.operationId];
  const captured = match.regex.exec(pathname)?.slice(1) ?? [];
  const params: Record<string, string> = {};
  match.paramNames.forEach((name, i) => {
    params[name] = decodeURIComponent(captured[i] ?? '');
  });

  if (op.requiresAuth && !authContextForHeader(request.headers.get('authorization'))) {
    // Deliberately bypasses `emit`'s assertStatusDeclared: the spec applies
    // root-level `bearerAuth` security to almost every operation, but only a
    // handful of them (authMe, search, ask, askStream, jobEvents) actually list
    // "401" in their per-operation `responses` — the rest just don't repeat it.
    // A real server still answers 401 uniformly for a missing/invalid bearer
    // token, so the mock does too; enforcing assertStatusDeclared here would
    // make almost every protected endpoint impossible to exercise unauthenticated
    // in a test, which is far more useful than pedantic spec literalism. This is
    // the one intentional exception to "every status this mock returns must be
    // spec-declared for that operation" — see the P2-02 report.
    const failure = unauthorized();
    return buildResponse(failure.status, failure.body, failure.headers);
  }

  const forced = mockControl.consume(match.operationId);
  if (forced) {
    return emit(match.operationId, forced.status, forced.body, forced.headers);
  }

  const handler = getHandler(match.operationId);
  if (!handler) {
    throw new Error(
      `mocks/fetchMock: route matched "${match.operationId}" but no handler is registered for it`,
    );
  }

  const ctx: MockRequestContext = {
    method: op.method,
    params,
    query: url.searchParams,
    headers: request.headers,
    json: async <T>() => (await request.clone().json()) as T,
    formData: () => request.clone().formData(),
    request,
  };

  const result = await handler(ctx);
  return emit(match.operationId, result.status, result.body, result.headers, result.rawBody);
}

let originalFetch: typeof globalThis.fetch | undefined;

/** Installs the mock over `globalThis.fetch`. Idempotent — calling it twice is a no-op. */
export function installMockFetch(): void {
  if (originalFetch) return;
  originalFetch = globalThis.fetch;
  globalThis.fetch = mockFetch as typeof globalThis.fetch;
  invalidateRouteCache();
}

/** Restores the real `globalThis.fetch`. */
export function uninstallMockFetch(): void {
  if (originalFetch) {
    globalThis.fetch = originalFetch;
    originalFetch = undefined;
  }
}

/** Resets seed data, request-id counter, and any forced failures. Call between tests. */
export function resetMockState(): void {
  resetMockStore();
  resetRequestIdCounter();
  mockControl.reset();
}
