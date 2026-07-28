// P2-12 (plans/markhand-web/phase-2-web-spa.md §P2.6). Renders the real page
// against the real mock server, same convention `LibraryPage.test.tsx` uses.
// Unlike `AdminMembersPage`, this page never reads `useAuth()`, so a plain
// `client.login()` (no `AuthProvider`) is enough — see
// `AdminMembersPage.test.tsx`'s module doc for why that page needs more.
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createApiClient, type ApiClient } from '../api/client';
import { installMockFetch, mockControl, resetMockState, uninstallMockFetch } from '../mocks';
import { grantMemberManage } from '../components/admin/testSupport';
import { ScopeProvider } from '../state/ScopeProvider';
import { AdminUsagePage } from './AdminUsagePage';

const DEMO_EMAIL = 'demo@markhand.test';
const DEMO_PASSWORD = 'demo-password';

async function loggedInClient(): Promise<ApiClient> {
  const client = createApiClient({ baseUrl: '' });
  grantMemberManage();
  await client.login({ email: DEMO_EMAIL, password: DEMO_PASSWORD });
  return client;
}

function renderPage(client: ApiClient) {
  return render(
    <ScopeProvider>
      <AdminUsagePage client={client} />
    </ScopeProvider>,
  );
}

beforeEach(() => {
  installMockFetch();
  resetMockState();
});

afterEach(() => {
  cleanup();
  uninstallMockFetch();
});

describe('AdminUsagePage', () => {
  it('renders a usage card for every resource from GET /usage', async () => {
    const client = await loggedInClient();
    renderPage(client);

    expect(await screen.findByText('Dung lượng lưu trữ')).toBeVisible();
    expect(screen.getByText('Số tài liệu')).toBeVisible();
    expect(screen.getByText('Tác vụ đồng thời')).toBeVisible();
    expect(screen.getByText('Token (LLM)')).toBeVisible();
  });

  it('renders a meter bar reflecting committed+reserved against the limit', async () => {
    const client = await loggedInClient();
    renderPage(client);

    await screen.findByText('Số tài liệu');
    // Seeded mock values (handlers/members.ts): documents committed=812,
    // reserved=3, limit=5000 -> (812+3)/5000 = 16.3% -> rounds to 16%.
    const meter = screen.getByRole('progressbar', { name: 'Đã dùng Số tài liệu' });
    expect(meter).toHaveAttribute('aria-valuenow', '16');
  });

  it('shows an error notice with a working retry when the usage request fails', async () => {
    const client = await loggedInClient();
    mockControl.forceStatus('getUsage', 429, { times: 1 });
    renderPage(client);

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(/quá nhiều yêu cầu/i);

    fireEvent.click(screen.getByRole('button', { name: 'Thử lại' }));
    await waitFor(() => expect(screen.getByText('Dung lượng lưu trữ')).toBeVisible());
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});
