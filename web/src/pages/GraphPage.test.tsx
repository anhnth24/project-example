import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createApiClient, type ApiClient } from '../api/client';
import { installMockFetch, resetMockState, uninstallMockFetch } from '../mocks';
import { mockUuid } from '../mocks/ids';
import { RouterProvider } from '../state/RouterProvider';
import { ScopeProvider } from '../state/ScopeProvider';
import { GraphPage } from './GraphPage';

const DEMO_EMAIL = 'demo@markhand.test';
const DEMO_PASSWORD = 'demo-password';
const SPECS_COLLECTION_ID = mockUuid(11);

async function loggedInClient(): Promise<ApiClient> {
  const client = createApiClient({ baseUrl: '' });
  await client.login({ email: DEMO_EMAIL, password: DEMO_PASSWORD });
  return client;
}

function renderGraph(client: ApiClient, collectionId?: string) {
  return render(
    <RouterProvider>
      <ScopeProvider>
        <GraphPage collectionId={collectionId} client={client} />
      </ScopeProvider>
    </RouterProvider>,
  );
}

describe('GraphPage', () => {
  beforeEach(() => {
    installMockFetch();
    resetMockState();
    window.history.pushState(null, '', '/');
  });

  afterEach(() => {
    cleanup();
    uninstallMockFetch();
  });

  it('renders the seeded graph: 13 nodes across 3 communities with per-cluster counts', async () => {
    const client = await loggedInClient();
    renderGraph(client);

    expect(await screen.findByText('Sổ tay nhân viên 2024')).toBeVisible();
    expect(screen.getByText('Đặc tả sản phẩm v1')).toBeVisible();
    expect(screen.getByText('Biên bản họp liên phòng')).toBeVisible();

    // Sidebar communities + counts (matches mocks/handlers/graph.ts's fixed catalog).
    expect(screen.getByRole('heading', { name: 'Cộng đồng' })).toBeVisible();
    const employeeHandbookRow = screen.getByLabelText(/Employee Handbook/);
    const productSpecsRow = screen.getByLabelText(/Product Specs/);
    const crossTeamRow = screen.getByLabelText(/Liên phòng ban/);
    expect(employeeHandbookRow.closest('li')).toHaveTextContent('5');
    expect(productSpecsRow.closest('li')).toHaveTextContent('5');
    expect(crossTeamRow.closest('li')).toHaveTextContent('3');
  });

  it('turning off a community hides its nodes; "Chọn tất cả" brings them back', async () => {
    const client = await loggedInClient();
    renderGraph(client);
    await screen.findByText('Sổ tay nhân viên 2024');

    fireEvent.click(screen.getByLabelText(/Employee Handbook/));
    await waitFor(() =>
      expect(screen.queryByText('Sổ tay nhân viên 2024')).not.toBeInTheDocument(),
    );
    // A different cluster's node is unaffected.
    expect(screen.getByText('Đặc tả sản phẩm v1')).toBeVisible();

    fireEvent.click(screen.getByRole('button', { name: 'Chọn tất cả' }));
    await waitFor(() => expect(screen.getByText('Sổ tay nhân viên 2024')).toBeVisible());
  });

  it('clicking a node navigates to its own collection in the library (P2-07 route)', async () => {
    const client = await loggedInClient();
    renderGraph(client);
    await screen.findByText('Đặc tả sản phẩm v1');

    fireEvent.click(screen.getByRole('button', { name: /Đặc tả sản phẩm v1/ }));
    await waitFor(() => expect(window.location.pathname).toBe(`/library/${SPECS_COLLECTION_ID}`));
  });

  it('filtering by collection narrows the graph to that collection only', async () => {
    const client = await loggedInClient();
    renderGraph(client);
    await screen.findByText('Sổ tay nhân viên 2024');
    expect(screen.getByText('Đặc tả sản phẩm v1')).toBeVisible();

    fireEvent.click(screen.getByRole('combobox', { name: 'Lọc theo bộ sưu tập' }));
    fireEvent.click(screen.getByRole('option', { name: 'Employee Handbook' }));

    await waitFor(() => expect(screen.queryByText('Đặc tả sản phẩm v1')).not.toBeInTheDocument());
    expect(screen.getByText('Sổ tay nhân viên 2024')).toBeVisible();
  });

  it('table view exposes edges structurally (kind + weight), not just node titles', async () => {
    const client = await loggedInClient();
    renderGraph(client);
    await screen.findByText('Sổ tay nhân viên 2024');

    fireEvent.click(screen.getByRole('button', { name: 'Chế độ xem bảng' }));

    expect(screen.getByText('Danh sách liên kết trong đồ thị')).toBeVisible();
    expect(screen.getAllByText('Xung đột').length).toBeGreaterThan(0);
  });

  it('announces the visible node/community counts via an aria-live region', async () => {
    const client = await loggedInClient();
    renderGraph(client);
    await screen.findByText('Sổ tay nhân viên 2024');

    expect(await screen.findByTestId('graph-live-region')).toHaveTextContent(
      'Đang hiển thị 13 trong 13 tài liệu, 3 trong 3 cụm.',
    );

    fireEvent.click(screen.getByLabelText(/Employee Handbook/));
    await waitFor(() =>
      expect(screen.getByTestId('graph-live-region')).toHaveTextContent(
        'Đang hiển thị 8 trong 13 tài liệu, 2 trong 3 cụm.',
      ),
    );
  });

  it('scopes the graph to org B after a switch — a different, smaller graph', async () => {
    // Same "seed independence" spirit as other org-scoped pages: this test
    // stays deliberately minimal — full org-switch UI wiring (`useAuth().
    // switchOrg`) already has its own dedicated suite (org-switch.spec.ts).
    // It drives the same `/orgs/switch` request `switchOrg` does and applies
    // the returned tokens directly (bypassing `AuthProvider`, which this
    // page-level test doesn't render), only to prove the mock's own org-B
    // graph fixture (`mocks/handlers/graph.ts`) is reachable and distinct.
    const client = await loggedInClient();
    const tokens = await client.request('post', '/orgs/switch', {
      body: { orgId: '00000000-0000-4000-8000-000000000003' },
    });
    client.sessionManager.setTokens(tokens);
    renderGraph(client);

    expect(await screen.findByText('Globex Master Plan.pdf')).toBeVisible();
    expect(screen.queryByText('Sổ tay nhân viên 2024')).not.toBeInTheDocument();
  });
});
