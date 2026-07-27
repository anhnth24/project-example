// Route/control guards for P2-05. These are UI convenience only — never
// authorization. `ProtectedRoute`/`PublicOnlyRoute` decide whether to render
// a page or bounce to `/login`; they cannot and do not stop an API call from
// answering 403. The server is always the authority (see
// plans/markhand-web/phase-2-web-spa.md §P2.3 and §P2.6: "Chỉ render theo
// permission nhưng server vẫn là nguồn authorization").
import { useEffect, type ReactNode } from 'react';
import { useRouter } from '../state/RouterProvider';
import { Notice } from '../components/ui';
import { useAuth } from './AuthContext';

function currentPathWithSearch(): string {
  if (typeof window === 'undefined') return '/';
  return window.location.pathname + window.location.search;
}

/**
 * Validates a `next` redirect target: it must resolve to a same-origin,
 * root-relative path. Anything else (a foreign absolute URL, a
 * protocol-relative `//evil.example`, an unparsable value) falls back to
 * `fallback` — this is the open-redirect guard for the `?next=` query param.
 */
export function sanitizeNextPath(next: string | null, fallback = '/library'): string {
  if (!next) return fallback;
  try {
    const resolved = new URL(next, window.location.origin);
    if (resolved.origin !== window.location.origin) return fallback;
    const path = `${resolved.pathname}${resolved.search}`;
    return path.startsWith('/') ? path : fallback;
  } catch {
    return fallback;
  }
}

function loginPathWithNext(): string {
  return `/login?next=${encodeURIComponent(currentPathWithSearch())}`;
}

function AuthChecking() {
  return (
    <p className="auth-loading" role="status">
      Đang kiểm tra phiên đăng nhập…
    </p>
  );
}

/**
 * Wraps a route's page. Renders `children` only once the session is
 * `authenticated`; while `checking` it shows a neutral loading state instead
 * of guessing; once resolved `anonymous`, it redirects to `/login` with the
 * current location preserved as `?next=` so login can return the visitor to
 * where they were headed. Redirecting only ever runs from an effect keyed on
 * `session.status`, and the redirect itself changes which route renders —
 * this component unmounts once the pathname moves to `/login`, so the effect
 * cannot re-fire and loop.
 *
 * `permission`, when given, is checked only for *rendering*: a signed-in
 * visitor without it sees an in-shell notice instead of the page, never a
 * silent redirect (they are still signed in; nothing here implies the server
 * would actually deny them — the real answer comes from the API call itself).
 */
export function ProtectedRoute({
  children,
  permission,
}: {
  children: ReactNode;
  permission?: string;
}) {
  const { session, hasPermission } = useAuth();
  const { navigate } = useRouter();

  useEffect(() => {
    if (session.status === 'anonymous') {
      navigate(loginPathWithNext());
    }
  }, [session.status, navigate]);

  if (session.status === 'checking') return <AuthChecking />;
  if (session.status === 'anonymous') return null;
  if (permission && !hasPermission(permission)) {
    return (
      <Notice tone="warning">
        Tài khoản của bạn không có quyền truy cập mục này. Đây chỉ là gợi ý hiển thị — máy chủ luôn
        là nơi quyết định cuối cùng.
      </Notice>
    );
  }
  return <>{children}</>;
}

/**
 * Wraps `/login` itself: an already-authenticated visitor is sent to their
 * intended page (`?next=`, sanitized) instead of seeing the form again.
 */
export function PublicOnlyRoute({ children }: { children: ReactNode }) {
  const { session } = useAuth();
  const { navigate } = useRouter();

  useEffect(() => {
    if (session.status === 'authenticated') {
      const params = new URLSearchParams(window.location.search);
      navigate(sanitizeNextPath(params.get('next')));
    }
  }, [session.status, navigate]);

  if (session.status === 'checking') return <AuthChecking />;
  if (session.status === 'authenticated') return null;
  return <>{children}</>;
}
