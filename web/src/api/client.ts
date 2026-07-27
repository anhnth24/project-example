// Typed HTTP client for the Markhand API. Every request/response shape comes
// from `generated/contract.ts` (regenerated from the server's OpenAPI doc) —
// nothing here hand-declares a type that file already exports.
//
// Responsibilities (see plans/markhand-web/phase-2-web-spa.md §P2.2):
//   - typed request/response against `paths`/`components`;
//   - bearer injection + single-flight refresh-on-401 (delegated to `session.ts`);
//   - normalized `{code,message,requestId,details?}` errors (delegated to `errors.ts`);
//   - 429 quota metadata surfaced, not swallowed;
//   - request cancellation via `AbortSignal`.
//
// Business routes are served under `/api/v1` even though the paths in
// `contract.ts` (and the OpenAPI doc itself, `servers: [{url: /api/v1}]`)
// omit that prefix; the server mounts every route in this file with it
// (verified in crates/server/src/routes/*.rs, e.g. `/api/v1/auth/login`).
import type { components, paths } from './generated/contract';
import { NetworkError, isAbortError, normalizeErrorResponse } from './errors';
import {
  createSessionManager,
  type SessionManager,
  type SessionTokens,
  type TokenProvider,
} from './session';

const API_PREFIX = '/api/v1';

type HttpMethod = 'get' | 'put' | 'post' | 'delete' | 'options' | 'head' | 'patch' | 'trace';

/** Paths declared in the OpenAPI contract. */
export type ApiPath = keyof paths;
/**
 * HTTP methods the contract actually declares for a given path. Every method
 * key is always present on `paths[P]` (e.g. `get?: never`), so filtering by
 * key name alone (`keyof paths[P]`) would accept every method for every
 * path; the unsupported ones are excluded here by checking that the value
 * isn't the `undefined` an unset optional method resolves to.
 */
export type MethodOf<P extends ApiPath> = {
  [M in HttpMethod]: paths[P][M] extends undefined ? never : M;
}[HttpMethod];
type OperationOf<P extends ApiPath, M extends MethodOf<P>> = paths[P][M];

type JsonRequestBodyOf<Op> = Op extends {
  requestBody?: { content: { 'application/json': infer B } };
}
  ? B
  : never;

// Response status codes are numeric literal keys in the generated contract
// (`200: {...}`), not string keys, so the "2xx" test coerces to a template
// literal (`${K}`) before pattern-matching against `2${string}`.
type SuccessResponseOf<Op> = Op extends { responses: infer R }
  ? {
      [K in keyof R]: K extends number
        ? `${K}` extends `2${string}`
          ? R[K] extends { content: { 'application/json': infer J } }
            ? J
            : void
          : never
        : never;
    }[keyof R]
  : never;

type PathParamsOf<P extends ApiPath, M extends MethodOf<P>> =
  OperationOf<P, M> extends { parameters: { path: infer PP } } ? PP : undefined;

type QueryParamsOf<P extends ApiPath, M extends MethodOf<P>> =
  OperationOf<P, M> extends { parameters: { query?: infer QP } } ? QP : undefined;

// `params` (and, transitively, `params.path`) is only required at the call
// site when the operation actually declares a path parameter — e.g.
// `request('get', '/collections/{collectionId}', ...)` must pass
// `params.path.collectionId`, while `request('get', '/auth/me')` needs no
// third argument at all. Body requiredness is not similarly enforced (see
// `JsonRequestBodyOf`) — a documented gap, not an oversight.
type ParamsFieldFor<P extends ApiPath, M extends MethodOf<P>> =
  PathParamsOf<P, M> extends undefined
    ? { path?: undefined; query?: QueryParamsOf<P, M> }
    : { path: PathParamsOf<P, M>; query?: QueryParamsOf<P, M> };

type BaseRequestOptions<P extends ApiPath, M extends MethodOf<P>> = {
  body?: JsonRequestBodyOf<OperationOf<P, M>>;
  signal?: AbortSignal;
};

export type RequestOptions<P extends ApiPath, M extends MethodOf<P>> =
  PathParamsOf<P, M> extends undefined
    ? BaseRequestOptions<P, M> & { params?: ParamsFieldFor<P, M> }
    : BaseRequestOptions<P, M> & { params: ParamsFieldFor<P, M> };

/** Whether `request()`'s third argument can be omitted for a given path/method (no required path params). */
type RequestOptionsArg<P extends ApiPath, M extends MethodOf<P>> =
  PathParamsOf<P, M> extends undefined
    ? [options?: RequestOptions<P, M>]
    : [options: RequestOptions<P, M>];

function substitutePath(template: string, pathParams: Record<string, unknown> | undefined): string {
  if (!pathParams) return template;
  return template.replace(/\{([^}]+)\}/g, (match, name: string) => {
    if (!(name in pathParams)) return match;
    return encodeURIComponent(String(pathParams[name]));
  });
}

function buildQueryString(query: Record<string, unknown> | undefined): string {
  if (!query) return '';
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value === undefined || value === null) continue;
    search.set(key, String(value));
  }
  const asString = search.toString();
  return asString.length > 0 ? `?${asString}` : '';
}

async function parseJsonResponse<T>(response: Response): Promise<T> {
  const text = await response.text();
  return (text.length > 0 ? JSON.parse(text) : undefined) as T;
}

export interface ApiClientOptions {
  /** Overrides `VITE_MARKHAND_API_BASE_URL`; mainly for tests. */
  baseUrl?: string;
  /** Overrides the default expiry skew passed to the session manager; mainly for tests. */
  expirySkewMs?: number;
}

export interface ApiClient {
  /** Typed request against any path/method the OpenAPI contract declares. Requires a valid session. */
  request<P extends ApiPath, M extends MethodOf<P>>(
    method: M,
    path: P,
    ...options: RequestOptionsArg<P, M>
  ): Promise<SuccessResponseOf<OperationOf<P, M>>>;
  /** `POST /auth/login`. On success, installs the returned tokens into the session. */
  login(
    credentials: components['schemas']['LoginRequest'],
    signal?: AbortSignal,
  ): Promise<SessionTokens>;
  /**
   * `POST /auth/logout`. Best-effort: the local session is always cleared,
   * even if the server call fails (network error or the refresh token was
   * already invalid) — logout should always take effect client-side. Any
   * such failure is still thrown so the caller can surface it if it wants.
   */
  logout(signal?: AbortSignal): Promise<void>;
  /** `GET /auth/me`. Authenticated; participates in refresh-on-401 like any other request. */
  me(signal?: AbortSignal): Promise<components['schemas']['MeResponse']>;
  /** The `TokenProvider` seam handed to the SSE agent's code and to auth UI. */
  tokenProvider: TokenProvider;
  /** Escape hatch for auth UI/tests that need session control beyond the `TokenProvider` seam. */
  sessionManager: SessionManager;
}

export function createApiClient(options: ApiClientOptions = {}): ApiClient {
  const baseUrl =
    (options.baseUrl ?? import.meta.env.VITE_MARKHAND_API_BASE_URL)?.replace(/\/$/, '') ?? '';

  async function rawFetch(path: string, init: RequestInit): Promise<Response> {
    try {
      return await fetch(`${baseUrl}${API_PREFIX}${path}`, init);
    } catch (cause) {
      if (isAbortError(cause)) throw cause;
      throw new NetworkError('Network request failed', { cause });
    }
  }

  async function refreshViaHttp(refreshToken: string): Promise<SessionTokens> {
    const body: components['schemas']['RefreshTokenRequest'] = { refreshToken };
    const response = await rawFetch('/auth/refresh', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
      body: JSON.stringify(body),
    });
    if (!response.ok) throw await normalizeErrorResponse(response);
    return parseJsonResponse<SessionTokens>(response);
  }

  const sessionManager = createSessionManager(refreshViaHttp, {
    expirySkewMs: options.expirySkewMs,
  });

  /**
   * Runs one attempt of an authenticated request, retrying exactly once
   * after a single `refreshNow()` if the first attempt came back 401 — never
   * more than once, so a persistently-rejecting server surfaces as a normal
   * 401 `HttpApiError` instead of looping.
   */
  async function authedFetch(
    method: string,
    path: string,
    init: { body?: unknown; signal?: AbortSignal },
  ): Promise<Response> {
    const attempt = async (): Promise<Response> => {
      const token = await sessionManager.getAccessToken();
      const headers: Record<string, string> = {
        Accept: 'application/json',
        Authorization: `Bearer ${token}`,
      };
      if (init.body !== undefined) headers['Content-Type'] = 'application/json';
      return rawFetch(path, {
        method,
        headers,
        body: init.body !== undefined ? JSON.stringify(init.body) : undefined,
        signal: init.signal,
      });
    };

    const first = await attempt();
    if (first.status !== 401) return first;
    await sessionManager.refreshNow();
    return attempt();
  }

  async function request<P extends ApiPath, M extends MethodOf<P>>(
    method: M,
    path: P,
    ...rest: RequestOptionsArg<P, M>
  ): Promise<SuccessResponseOf<OperationOf<P, M>>> {
    const reqOptions: RequestOptions<P, M> = rest[0] ?? ({} as RequestOptions<P, M>);
    const url =
      substitutePath(
        path as string,
        reqOptions.params?.path as Record<string, unknown> | undefined,
      ) + buildQueryString(reqOptions.params?.query as Record<string, unknown> | undefined);
    const response = await authedFetch(method.toUpperCase(), url, {
      body: reqOptions.body,
      signal: reqOptions.signal,
    });
    if (!response.ok) throw await normalizeErrorResponse(response);
    if (response.status === 204 || response.status === 205) {
      return undefined as SuccessResponseOf<OperationOf<P, M>>;
    }
    return parseJsonResponse<SuccessResponseOf<OperationOf<P, M>>>(response);
  }

  async function login(
    credentials: components['schemas']['LoginRequest'],
    signal?: AbortSignal,
  ): Promise<SessionTokens> {
    const response = await rawFetch('/auth/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
      body: JSON.stringify(credentials),
      signal,
    });
    if (!response.ok) throw await normalizeErrorResponse(response);
    const tokens = await parseJsonResponse<SessionTokens>(response);
    sessionManager.setTokens(tokens);
    return tokens;
  }

  async function logout(signal?: AbortSignal): Promise<void> {
    const refreshToken = sessionManager.getRefreshTokenForLogout();
    if (!refreshToken) {
      sessionManager.clear();
      return;
    }
    try {
      const body: components['schemas']['RefreshTokenRequest'] = { refreshToken };
      const response = await rawFetch('/auth/logout', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
        body: JSON.stringify(body),
        signal,
      });
      if (!response.ok) throw await normalizeErrorResponse(response);
    } finally {
      sessionManager.clear();
    }
  }

  async function me(signal?: AbortSignal): Promise<components['schemas']['MeResponse']> {
    return request('get', '/auth/me', { signal });
  }

  return {
    request,
    login,
    logout,
    me,
    tokenProvider: sessionManager,
    sessionManager,
  };
}

/** Ready-to-use singleton for app code; tests should prefer `createApiClient()` for isolation. */
export const apiClient = createApiClient();

export { HttpApiError, NetworkError, isAbortError } from './errors';
export type { ApiErrorBody, RateLimitInfo } from './errors';
export type { TokenProvider, SessionTokens } from './session';
