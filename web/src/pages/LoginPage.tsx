import { useId, useState, type FormEvent } from 'react';
import { HttpApiError, NetworkError } from '../api/errors';
import { useAuth } from '../auth/AuthContext';
import { BrandMark } from '../components/BrandMark';
import { Button, Notice } from '../components/ui';

function messageFor(cause: unknown): string {
  if (cause instanceof HttpApiError && cause.status === 401) {
    return 'Email hoặc mật khẩu không đúng.';
  }
  if (cause instanceof HttpApiError && cause.status === 429) {
    return 'Quá nhiều lần thử. Vui lòng thử lại sau ít phút.';
  }
  if (cause instanceof NetworkError) {
    return 'Không thể kết nối máy chủ. Kiểm tra kết nối mạng và thử lại.';
  }
  return 'Không thể đăng nhập lúc này. Vui lòng thử lại.';
}

export function LoginPage() {
  const { login } = useAuth();
  const emailId = useId();
  const passwordId = useId();
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setPending(true);
    setError(null);
    try {
      await login(email, password);
      // Success leaves this page mounted only for an instant: `session`
      // becomes 'authenticated' and `PublicOnlyRoute` (in App.tsx) is the one
      // place that decides where an authenticated visitor belongs and
      // navigates there — this component does not duplicate that decision.
    } catch (cause) {
      setError(messageFor(cause));
      setPending(false);
    }
  }

  return (
    <section className="page auth-card" aria-labelledby="login-heading">
      <BrandMark className="rail-brand-mark auth-card-brand" />
      <p className="eyebrow">Đăng nhập</p>
      <h1 id="login-heading">Đăng nhập vào Folyvo</h1>
      <p className="lede">Nhập thông tin đăng nhập để truy cập bộ sưu tập và trợ lý tài liệu.</p>

      {error && <Notice tone="error">{error}</Notice>}

      <form className="auth-form" onSubmit={(event) => void handleSubmit(event)} noValidate>
        <div className="field">
          <label htmlFor={emailId}>Email</label>
          <input
            id={emailId}
            name="email"
            type="email"
            autoComplete="username"
            required
            disabled={pending}
            value={email}
            onChange={(event) => setEmail(event.target.value)}
          />
        </div>

        <div className="field">
          <label htmlFor={passwordId}>Mật khẩu</label>
          <input
            id={passwordId}
            name="password"
            type="password"
            autoComplete="current-password"
            required
            disabled={pending}
            value={password}
            onChange={(event) => setPassword(event.target.value)}
          />
        </div>

        <Button type="submit" variant="primary" loading={pending} disabled={pending}>
          Đăng nhập
        </Button>
      </form>
    </section>
  );
}
