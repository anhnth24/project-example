import { useCallback, useEffect, useRef, useState } from 'react';
import { fetchReadiness, type ConnectionState } from './api/health';
import { AuthProvider } from './auth/AuthContext';
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
import type { RouteName } from './types/routes';

const navItems: Array<{ route: RouteName; to: string; label: string }> = [
  { route: 'login', to: '/login', label: 'Đăng nhập' },
  { route: 'library', to: '/library', label: 'Thư viện' },
  { route: 'qa', to: '/qa', label: 'Hỏi đáp' },
  { route: 'adminMembers', to: '/admin/members', label: 'Thành viên' },
  { route: 'adminUsage', to: '/admin/usage', label: 'Sử dụng' },
  { route: 'help', to: '/help', label: 'Trợ giúp' },
];

function PrimaryNav() {
  const { match } = useRouter();
  return (
    <nav className="primary-nav" aria-label="Điều hướng chính">
      {navItems.map((item) => (
        <RouteLink
          key={item.route}
          to={item.to}
          aria-current={match.name === item.route ? 'page' : undefined}
        >
          {item.label}
        </RouteLink>
      ))}
    </nav>
  );
}

function RouteOutlet() {
  const { match } = useRouter();
  switch (match.name) {
    case 'login':
      return <LoginPage />;
    case 'library':
      return <LibraryPage collectionId={match.params.collectionId} />;
    case 'qa':
      return <QaPage collectionId={match.params.collectionId} />;
    case 'adminMembers':
      return <AdminMembersPage />;
    case 'adminUsage':
      return <AdminUsagePage />;
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
      <AuthProvider>
        <AppShell />
      </AuthProvider>
    </RouterProvider>
  );
}

function AppShell() {
  const [connection, setConnection] = useState<ConnectionState>({ kind: 'checking' });
  const controllerRef = useRef<AbortController | null>(null);
  const { match } = useRouter();

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
    <div className="app-shell">
      <a className="skip-link" href="#main-content">
        Bỏ qua để đến nội dung chính
      </a>
      <header className="topbar">
        <RouteLink className="brand" to="/" aria-label="Trang chủ Markhand">
          <span aria-hidden="true" className="brand-mark">
            M
          </span>
          <span>Markhand</span>
        </RouteLink>
        <PrimaryNav />
        <span className={`connection-dot ${connection.kind}`} aria-hidden="true" />
        <span className="connection-label" role="status">
          {connection.kind === 'checking' && 'Đang kiểm tra máy chủ'}
          {connection.kind === 'ready' && 'Đã kết nối máy chủ'}
          {connection.kind === 'unavailable' && 'Máy chủ chưa sẵn sàng'}
        </span>
      </header>

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
  );
}
