import { useCallback, useEffect, useRef, useState } from 'react';
import { fetchReadiness, type ConnectionState } from './api/health';
import { AuthProvider } from './auth/AuthContext';
import { ProtectedRoute, PublicOnlyRoute } from './auth/RouteGuard';
import { Rail } from './components/shell';
import { RouteLink } from './components/RouteLink';
import {
  AdminMembersPage,
  AdminUsagePage,
  HelpPage,
  LibraryPage,
  LoginPage,
  QaPage,
} from './pages';
import { RouterProvider, useRouter } from './state/RouterProvider';
import { ScopeProvider } from './state/ScopeProvider';

/**
 * Real permission constants from plans/markhand-web/phase-1c-multi-org-security.md
 * §P1C.2 — `member.manage` gates the members admin page. There is no
 * documented constant yet for the usage/quota admin page (P2-12/1C haven't
 * shipped one), so that route only requires "signed in" below rather than
 * guessing a permission name the server may not agree with.
 */
const MEMBER_MANAGE_PERMISSION = 'member.manage';

function RouteOutlet() {
  const { match } = useRouter();
  switch (match.name) {
    case 'login':
      return (
        <PublicOnlyRoute>
          <LoginPage />
        </PublicOnlyRoute>
      );
    case 'library':
      return (
        <ProtectedRoute>
          <LibraryPage collectionId={match.params.collectionId} />
        </ProtectedRoute>
      );
    case 'qa':
      return (
        <ProtectedRoute>
          <QaPage collectionId={match.params.collectionId} />
        </ProtectedRoute>
      );
    case 'adminMembers':
      return (
        <ProtectedRoute permission={MEMBER_MANAGE_PERMISSION}>
          <AdminMembersPage />
        </ProtectedRoute>
      );
    case 'adminUsage':
      return (
        <ProtectedRoute>
          <AdminUsagePage />
        </ProtectedRoute>
      );
    case 'help':
      return <HelpPage />;
    case 'notFound':
      return (
        <section className="page" aria-labelledby="not-found-heading">
          <h1 id="not-found-heading">Không tìm thấy trang</h1>
          <p className="lede">
            <RouteLink to="/">Về trang chính</RouteLink>
          </p>
        </section>
      );
    case 'home':
      return null;
  }
}

export function App() {
  return (
    <RouterProvider>
      <ScopeProvider>
        <AuthProvider>
          <AppShell />
        </AuthProvider>
      </ScopeProvider>
    </RouterProvider>
  );
}

function AppShell() {
  const [connection, setConnection] = useState<ConnectionState>({ kind: 'checking' });
  const controllerRef = useRef<AbortController | null>(null);
  const { match } = useRouter();
  // The rail is the app's chrome for every *inside-the-app* route. `/login`
  // (both design sources render it as a distinct, chrome-free full-bleed
  // screen — see the report) is the one route that opts out, matching
  // `PublicOnlyRoute`'s own framing: a visitor there isn't "in the app" yet.
  const showRail = match.name !== 'login';

  const loadConnection = useCallback(async () => {
    controllerRef.current?.abort();
    const controller = new AbortController();
    controllerRef.current = controller;
    setConnection({ kind: 'checking' });
    try {
      const health = await fetchReadiness(controller.signal);
      if (controllerRef.current === controller) {
        setConnection({ kind: 'ready', requestId: health.requestId });
      }
    } catch (error) {
      if (
        controllerRef.current === controller &&
        !(error instanceof DOMException && error.name === 'AbortError')
      ) {
        setConnection({ kind: 'unavailable' });
      }
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    queueMicrotask(() => {
      if (!disposed) {
        void loadConnection();
      }
    });
    return () => {
      disposed = true;
      controllerRef.current?.abort();
    };
  }, [loadConnection]);

  const checkConnection = () => void loadConnection();

  const isReady = connection.kind === 'ready';

  return (
    <div className={`app-shell ${showRail ? 'shell-with-rail' : ''}`}>
      <a className="skip-link" href="#main-content">
        Bỏ qua để đến nội dung chính
      </a>
      {showRail && <Rail />}

      <div className="app-shell-main">
        <div className="shell-statusline">
          <span className={`connection-dot ${connection.kind}`} aria-hidden="true" />
          <span className="connection-label" role="status">
            {connection.kind === 'checking' && 'Đang kiểm tra máy chủ'}
            {connection.kind === 'ready' && 'Đã kết nối máy chủ'}
            {connection.kind === 'unavailable' && 'Máy chủ chưa sẵn sàng'}
          </span>
        </div>

        <main id="main-content" className="welcome" tabIndex={-1}>
          {match.name === 'home' ? (
            <>
              <p className="eyebrow">Không gian tri thức</p>
              <h1>Không gian làm việc đã sẵn sàng để kết nối.</h1>
              <p className="lede">
                Markhand quản lý chuyển đổi tài liệu, lập chỉ mục và câu trả lời có trích dẫn trong
                một không gian được kiểm soát.
              </p>

              <section className="connection-card" aria-labelledby="connection-heading">
                <div>
                  <p className="card-label">Kết nối dịch vụ</p>
                  <h2 id="connection-heading">
                    {isReady ? 'Máy chủ đã sẵn sàng' : 'Đang chờ máy chủ'}
                  </h2>
                  <p className="card-copy">
                    {isReady
                      ? 'Không gian tài liệu có thể tải dữ liệu thật khi các API nghiệp vụ sẵn sàng.'
                      : 'Khởi động máy chủ Markhand, sau đó kiểm tra lại kết nối.'}
                  </p>
                </div>
                <button
                  type="button"
                  disabled={connection.kind === 'checking'}
                  onClick={checkConnection}
                >
                  {connection.kind === 'checking' ? 'Đang kiểm tra…' : 'Kiểm tra kết nối'}
                </button>
                {isReady && (
                  <p className="request-id">
                    Mã yêu cầu đã kết nối <code>{connection.requestId}</code>
                  </p>
                )}
              </section>

              <section className="next-steps" aria-labelledby="next-steps-heading">
                <h2 id="next-steps-heading">Sắp có</h2>
                <ul>
                  <li>Bộ sưu tập và tải tài liệu an toàn</li>
                  <li>Tiến trình chuyển đổi và lập chỉ mục</li>
                  <li>Tìm kiếm và câu trả lời có trích dẫn</li>
                </ul>
              </section>
            </>
          ) : (
            <RouteOutlet />
          )}
        </main>
      </div>
    </div>
  );
}
