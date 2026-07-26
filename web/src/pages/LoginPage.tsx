import { useAuth } from '../auth/AuthContext';
import { Button } from '../components/ui';

export function LoginPage() {
  const { session } = useAuth();

  return (
    <section className="page" aria-labelledby="login-heading">
      <p className="eyebrow">Đăng nhập</p>
      <h1 id="login-heading">Đăng nhập vào Markhand</h1>
      <p className="lede">
        {session.status === 'authenticated'
          ? `Đã đăng nhập với vai trò ${session.role}.`
          : 'Nhập thông tin đăng nhập để truy cập bộ sưu tập và trợ lý tài liệu.'}
      </p>
      <Button variant="primary" disabled>
        Đăng nhập (đang chờ client xác thực)
      </Button>
    </section>
  );
}
