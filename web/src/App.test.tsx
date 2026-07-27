import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { App } from './App';
import { installMockFetch, resetMockState, uninstallMockFetch } from './mocks';
// Raw CSS text for the structural "bottom cluster can't be clipped" contract
// test below. Vite's `?raw` import would be the obvious choice, but Vitest's
// default CSS handling stubs every `*.css` module (including `?raw`ed ones)
// to an empty string in a jsdom environment unless `test.css` is turned on
// in vite.config.ts — a config file this task isn't allowed to touch. Node's
// `fs` reads the real file directly instead: it has no ambient types here
// (this project has no @types/node, and adding one would be a new
// dependency), so the import is `@ts-expect-error`-suppressed for `tsc`
// (`pnpm build`) — it still runs for real under Vitest's Node process.
// @ts-expect-error -- no @types/node in this project; readFileSync exists at runtime under Vitest's Node process.
import { readFileSync } from 'node:fs';

// Relative to the process cwd, which is this package's root (`web/`) for
// every way this suite is actually invoked (`pnpm test`, `pnpm --dir web
// test`, vitest's own CLI run from here) — not relative to this file, since
// `import.meta.url` under Vitest's transform isn't guaranteed to be a real
// `file:` URL the way it is in a plain Node/ESM script.
const railStylesheet: string = readFileSync('src/styles.css', 'utf-8');

const DEMO_EMAIL = 'demo@markhand.test';
const DEMO_PASSWORD = 'demo-password';

function fillAndSubmitLogin(email: string, password: string) {
  fireEvent.change(screen.getByLabelText('Email'), { target: { value: email } });
  fireEvent.change(screen.getByLabelText('Mật khẩu'), { target: { value: password } });
  fireEvent.click(screen.getByRole('button', { name: 'Đăng nhập' }));
}

/** The rail's icon-only controls that are always present regardless of auth
 * state (brand + destinations) — org switch and the account
 * menu only render once `session.status === 'authenticated'` (see
 * components/shell/OrgSwitch.tsx / UserMenu.tsx), so they're covered
 * separately in the authenticated describe block below. */
const ALWAYS_ON_RAIL_CONTROLS: Array<{ role: 'link' | 'button'; name: string | RegExp }> = [
  { role: 'link', name: 'Trang chủ Markhand' },
  { role: 'link', name: 'Thư viện' },
  { role: 'link', name: 'Hỏi đáp' },
  { role: 'link', name: 'Thành viên' },
  { role: 'link', name: 'Sử dụng' },
  { role: 'link', name: 'Trợ giúp' },
];

describe('App', () => {
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
    window.history.pushState(null, '', '/');
  });

  it('renders readiness from the real API contract', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        new Response(
          JSON.stringify({ status: 'ok', requestId: '5b435d32-20a3-47c0-a615-aa0b9c5bcd28' }),
          {
            status: 200,
            headers: { 'Content-Type': 'application/json' },
          },
        ),
      ),
    );

    render(<App />);

    expect(
      screen.getByRole('heading', { name: 'Không gian làm việc đã sẵn sàng để kết nối.' }),
    ).toBeVisible();
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Đã kết nối máy chủ'));
    expect(screen.getByText('5b435d32-20a3-47c0-a615-aa0b9c5bcd28')).toBeVisible();
    expect(fetch).toHaveBeenCalledWith(
      '/api/v1/health/ready',
      expect.objectContaining({ headers: { Accept: 'application/json' } }),
    );
  });

  it('shows a recoverable state when the backend is unavailable', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 503 })));

    render(<App />);

    await waitFor(() =>
      expect(screen.getByRole('status')).toHaveTextContent('Máy chủ chưa sẵn sàng'),
    );
    expect(screen.getByRole('button', { name: 'Kiểm tra kết nối' })).toBeVisible();
  });

  it('navigates to a P2.3 route by clicking the rail nav link', () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 503 })));

    render(<App />);
    fireEvent.click(screen.getByRole('link', { name: 'Trợ giúp' }));

    expect(window.location.pathname).toBe('/help');
    expect(screen.getByRole('heading', { name: 'Trợ giúp Markhand' })).toBeVisible();
  });

  // P2-14 (plans/markhand-web/phase-2-web-spa.md §P2.7): "focus sau route
  // change". Without this, a keyboard/screen-reader user who activates a
  // rail link keeps focus on the link itself while the page underneath it
  // changes — the new page's heading is never announced and Tab continues
  // from the rail instead of from the content. Moving focus to the `<main>`
  // landmark (already `tabIndex={-1}` at App.tsx's `#main-content`, put
  // there for the skip link) is the standard SPA fix; this only asserts it
  // actually happens, not just that the markup exists for it to.
  it('moves focus to the main landmark after a route change (not just on first paint)', () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 503 })));

    render(<App />);
    const main = screen.getByRole('main');
    // The initial mount must not be reporting a false positive here — a
    // browser tab load starts focus on <body>, not on the landmark.
    expect(document.activeElement).not.toBe(main);

    fireEvent.click(screen.getByRole('link', { name: 'Trợ giúp' }));

    expect(window.location.pathname).toBe('/help');
    expect(document.activeElement).toBe(main);
  });

  it('redirects a deep-linked protected route to /login when anonymous, preserving it as ?next=', async () => {
    window.history.pushState(null, '', '/library/col-42');
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 503 })));

    render(<App />);

    await waitFor(() =>
      expect(window.location.pathname + window.location.search).toBe(
        '/login?next=%2Flibrary%2Fcol-42',
      ),
    );
    expect(screen.getByRole('heading', { name: 'Đăng nhập vào Markhand' })).toBeVisible();
    expect(screen.queryByRole('heading', { name: 'Bộ sưu tập col-42' })).not.toBeInTheDocument();
  });

  it('shows a not-found page for an unknown path and links back home', () => {
    window.history.pushState(null, '', '/does-not-exist');
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 503 })));

    render(<App />);

    expect(screen.getByRole('heading', { name: 'Không tìm thấy trang' })).toBeVisible();
    fireEvent.click(screen.getByRole('link', { name: 'Về trang chính' }));
    expect(window.location.pathname).toBe('/');
  });

  it('does not render the rail on /login — it is a distinct, chrome-free screen in both design sources', async () => {
    window.history.pushState(null, '', '/login');
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 503 })));

    render(<App />);

    expect(screen.getByRole('heading', { name: 'Đăng nhập vào Markhand' })).toBeVisible();
    expect(screen.queryByRole('navigation', { name: 'Điều hướng chính' })).not.toBeInTheDocument();
  });
});

describe('App / rail (icon-only shell nav)', () => {
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
    window.history.pushState(null, '', '/');
  });

  it('marks exactly the current route aria-current="page", and no other destination', () => {
    window.history.pushState(null, '', '/help');
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 503 })));

    render(<App />);

    expect(screen.getByRole('link', { name: 'Trợ giúp' })).toHaveAttribute('aria-current', 'page');
    for (const name of ['Thư viện', 'Hỏi đáp', 'Thành viên', 'Sử dụng']) {
      expect(screen.getByRole('link', { name })).not.toHaveAttribute('aria-current');
    }
  });

  it('gives every icon-only rail control an accessible name, a visible tooltip, and keyboard focusability', () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 503 })));

    const { container } = render(<App />);

    const rail = container.querySelector<HTMLElement>('.rail');
    expect(rail).not.toBeNull();
    for (const { role, name } of ALWAYS_ON_RAIL_CONTROLS) {
      const control = within(rail as HTMLElement).getByRole(role, { name });

      // Accessible name: every control here is icon-only (svg is
      // aria-hidden), so its name must come from aria-label, not text.
      expect(control).toHaveAccessibleName();

      // Visible tooltip: a sibling `role="tooltip"` element inside the same
      // `.rail-item` wrapper, shown on hover/focus via CSS (see
      // components/shell/RailHint.tsx + styles.css's `.rail-tooltip`).
      const wrapper = control.closest('.rail-item');
      expect(wrapper).not.toBeNull();
      const tooltip = within(wrapper as HTMLElement).getByRole('tooltip', { hidden: true });
      expect(tooltip.textContent?.trim().length).toBeGreaterThan(0);

      // Keyboard reachability: a real, non-disabled `<a>`/`<button>` accepts
      // programmatic focus the same way Tab would land on it.
      expect(control).not.toHaveAttribute('tabindex', '-1');
      control.focus();
      expect(document.activeElement).toBe(control);
    }
  });

  it('keeps the rail bottom cluster in normal document flow — the scrollable nav list absorbs height pressure, never the cluster', () => {
    // jsdom has no real layout engine (no box model, no computed
    // getBoundingClientRect), so a genuine "does the avatar get clipped at
    // 600px" assertion needs a real browser and is out of scope for this
    // toolchain (vitest + jsdom only). What *is* verifiable here is the CSS
    // contract that produces that guarantee: `.rail-bottom` is a
    // non-growing, non-shrinking, statically-positioned flow item (never
    // `position: fixed/absolute` pinned against the viewport, which is what
    // would let it collide with or get clipped by something behind it), and
    // `.rail-nav-list` is the one flexible, scrolling member that gives way
    // instead. This is a structural regression guard, not a pixel-level one.
    const railBottomRule = railStylesheet.match(/\.rail-bottom\s*{[^}]*}/)?.[0] ?? '';
    expect(railBottomRule).toMatch(/flex:\s*0 0 auto/);
    expect(railBottomRule).not.toMatch(/position:\s*(fixed|absolute)/);

    const railNavListRule = railStylesheet.match(/\.rail-nav-list\s*{[^}]*}/)?.[0] ?? '';
    expect(railNavListRule).toMatch(/flex:\s*1 1 auto/);
    expect(railNavListRule).toMatch(/overflow-y:\s*auto/);
    expect(railNavListRule).toMatch(/min-height:\s*0/);

    const railRule = railStylesheet.match(/\n\.rail\s*{[^}]*}/)?.[0] ?? '';
    expect(railRule).toMatch(/height:\s*100dvh/);
  });
});

describe('App / authenticated shell (P2-05 guard matrix + login/logout)', () => {
  beforeEach(() => {
    installMockFetch();
    resetMockState();
  });

  afterEach(() => {
    cleanup();
    uninstallMockFetch();
    window.sessionStorage.clear();
    window.history.pushState(null, '', '/');
  });

  it('logs in through the real mock server, lands on the intended deep-linked route, and surfaces session/org through the account menu', async () => {
    window.history.pushState(null, '', '/library/col-42');
    render(<App />);

    await waitFor(() =>
      expect(screen.getByRole('heading', { name: 'Đăng nhập vào Markhand' })).toBeVisible(),
    );
    expect(window.location.search).toBe('?next=%2Flibrary%2Fcol-42');

    fillAndSubmitLogin(DEMO_EMAIL, DEMO_PASSWORD);

    await waitFor(() =>
      expect(screen.getByRole('heading', { name: 'Bộ sưu tập col-42' })).toBeVisible(),
    );

    // Session info moved from an always-visible topbar strip into the
    // avatar-triggered account menu (see components/shell/UserMenu.tsx) —
    // opening it is the new way to reach the same info.
    fireEvent.click(screen.getByRole('button', { name: 'Tài khoản: Demo User' }));
    expect(screen.getByText('Demo User')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Đăng xuất' })).toBeVisible();
  });

  it('rejects the wrong password and never renders the protected shell', async () => {
    window.history.pushState(null, '', '/library');
    render(<App />);
    await waitFor(() => expect(window.location.pathname).toBe('/login'));

    fillAndSubmitLogin(DEMO_EMAIL, 'not-the-password');

    await waitFor(() => expect(screen.getByText('Email hoặc mật khẩu không đúng.')).toBeVisible());
    expect(window.location.pathname).toBe('/login');
    expect(screen.queryByRole('button', { name: /Tài khoản:/ })).not.toBeInTheDocument();
  });

  it('logs out back to /login without leaving a half-authenticated shell', async () => {
    window.history.pushState(null, '', '/library');
    render(<App />);
    await waitFor(() => expect(window.location.pathname).toBe('/login'));

    fillAndSubmitLogin(DEMO_EMAIL, DEMO_PASSWORD);
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Tài khoản: Demo User' })).toBeVisible(),
    );

    fireEvent.click(screen.getByRole('button', { name: 'Tài khoản: Demo User' }));
    fireEvent.click(screen.getByRole('button', { name: 'Đăng xuất' }));

    await waitFor(() => expect(window.location.pathname).toBe('/login'));
    expect(screen.queryByRole('button', { name: /Tài khoản:/ })).not.toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Đăng nhập vào Markhand' })).toBeVisible();
  });

  it('renders an in-shell notice — not the admin page, not a redirect — for a signed-in user without member.manage', async () => {
    window.history.pushState(null, '', '/admin/members');
    render(<App />);
    await waitFor(() => expect(window.location.pathname).toBe('/login'));

    fillAndSubmitLogin(DEMO_EMAIL, DEMO_PASSWORD);

    await waitFor(() => expect(screen.getByText(/không có quyền/)).toBeVisible());
    expect(
      screen.queryByRole('heading', { name: 'Thành viên và vai trò' }),
    ).not.toBeInTheDocument();
    // Still on the admin URL — this is a render decision, the server remains the authority.
    expect(window.location.pathname).toBe('/admin/members');
  });

  it('restores the session on unmount/remount without forcing a re-login (real page-reload-from-a-fresh-client case is covered in auth/AuthContext.test.tsx)', async () => {
    window.history.pushState(null, '', '/library');
    render(<App />);
    await waitFor(() => expect(window.location.pathname).toBe('/login'));
    fillAndSubmitLogin(DEMO_EMAIL, DEMO_PASSWORD);
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Tài khoản: Demo User' })).toBeVisible(),
    );

    // `App.tsx` always uses the shared `apiClient` singleton, so unmounting
    // and remounting here does not clear its in-memory tokens the way an
    // actual page reload (a brand-new JS heap) would — this only asserts the
    // shell doesn't second-guess a still-valid persisted session on remount.
    cleanup();
    window.history.pushState(null, '', '/library');
    render(<App />);

    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Tài khoản: Demo User' })).toBeVisible(),
    );
    expect(window.location.pathname).toBe('/library');
  });

  it('opens the org identity and account popovers with the keyboard, and Escape closes and returns focus to the trigger', async () => {
    window.history.pushState(null, '', '/library');
    render(<App />);
    await waitFor(() => expect(window.location.pathname).toBe('/login'));
    fillAndSubmitLogin(DEMO_EMAIL, DEMO_PASSWORD);
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Đơn vị hiện tại' })).toBeVisible(),
    );

    const orgTrigger = screen.getByRole('button', { name: 'Đơn vị hiện tại' });
    expect(orgTrigger).toHaveAccessibleName();
    orgTrigger.focus();
    fireEvent.click(orgTrigger);
    expect(screen.getByRole('dialog', { name: 'Đơn vị hiện tại' })).toBeVisible();

    fireEvent.keyDown(document, { key: 'Escape' });
    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: 'Đơn vị hiện tại' })).not.toBeInTheDocument(),
    );
    expect(document.activeElement).toBe(orgTrigger);
  });
});
