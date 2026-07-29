// P2-18 "Khu Quản trị" move (owner critique, 2026-07-29): project
// management (create/rename/assign/unassign) moved out of `LibraryPage`'s
// old `ProjectsPanel` into this dedicated admin page. Renders the real page
// against the real mock server (`mocks/handlers/projects.ts`/`library.ts`),
// same convention `LibraryPage.test.tsx` uses — no hand-stubbed `apiClient`.
// This page never calls `useAuth()` itself (permission gating lives one
// level up, at the route — `App.tsx`'s `ProtectedRoute permission="doc.upload"`
// — same split `AdminMembersPage.tsx`/`AdminUsagePage.tsx` already use), so
// tests here need only `ScopeProvider`, not a real `AuthProvider`.
//
// Row locator convention: a project's own `<tr>` is found via its exact-text
// name `<span>` and `.closest('tr')` — same pattern
// `AdminMembersPage.test.tsx`'s `findRowByTagText` uses — rather than
// `getByRole('row', {name})`: a `<tr>`'s computed accessible name is built
// from nested interactive elements' own `aria-label`s (the "Sửa"/"×"
// buttons here), not just its visible text, which makes a `name` regex
// against it unreliable once a row contains more than one such label.
//
// Fixture ground truth (mocks/fixtures.ts): org A seeds "Nhân sự" (assigned
// to Employee Handbook, mockUuid(10)) and "Sản phẩm" (no collections yet);
// "Product Specs" (mockUuid(11)) starts unassigned.
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createApiClient, type ApiClient } from '../api/client';
import { installMockFetch, resetMockState, uninstallMockFetch } from '../mocks';
import { ScopeProvider } from '../state/ScopeProvider';
import { AdminProjectsPage } from './AdminProjectsPage';

const DEMO_EMAIL = 'demo@markhand.test';
const DEMO_PASSWORD = 'demo-password';

async function loggedInClient(): Promise<ApiClient> {
  const client = createApiClient({ baseUrl: '' });
  await client.login({ email: DEMO_EMAIL, password: DEMO_PASSWORD });
  return client;
}

function renderPage(client: ApiClient) {
  return render(
    <ScopeProvider>
      <AdminProjectsPage client={client} />
    </ScopeProvider>,
  );
}

/** The project row whose name `<span>` (exact text, not a fragment of a
 * button's `aria-label` elsewhere in the row) reads `projectName` — see this
 * file's own module doc for why `getByRole('row', {name})` is not used here. */
function findProjectRow(projectName: string): HTMLElement {
  return screen
    .getByText(projectName, { selector: 'td > div > span' })
    .closest('tr') as HTMLElement;
}

async function findProjectRowAsync(projectName: string): Promise<HTMLElement> {
  const span = await screen.findByText(projectName, { selector: 'td > div > span' });
  return span.closest('tr') as HTMLElement;
}

describe('AdminProjectsPage', () => {
  beforeEach(() => {
    installMockFetch();
    resetMockState();
  });

  afterEach(() => {
    cleanup();
    uninstallMockFetch();
  });

  it('renders one row per project, its assigned collections as chips, and a count, plus an unassigned-collections section', async () => {
    const client = await loggedInClient();
    renderPage(client);

    const hrRow = await findProjectRowAsync('Nhân sự');
    expect(within(hrRow).getByText('Employee Handbook')).toBeVisible();
    expect(within(hrRow).getByText('1')).toBeVisible();

    const productRow = findProjectRow('Sản phẩm');
    expect(within(productRow).getByText('Chưa có bộ sưu tập nào')).toBeVisible();
    expect(within(productRow).getByText('0')).toBeVisible();

    expect(screen.getByRole('heading', { name: 'Chưa thuộc dự án' })).toBeVisible();
    expect(
      screen.getByRole('combobox', { name: 'Gán dự án cho bộ sưu tập Product Specs' }),
    ).toBeVisible();
  });

  it('creates a project and lists it immediately', async () => {
    const client = await loggedInClient();
    renderPage(client);

    await findProjectRowAsync('Nhân sự');
    fireEvent.change(screen.getByLabelText('Tên dự án'), { target: { value: 'Marketing' } });
    fireEvent.click(screen.getByRole('button', { name: 'Tạo dự án' }));

    await waitFor(() => expect(screen.getByLabelText('Tên dự án')).toHaveValue(''));
    expect(await findProjectRowAsync('Marketing')).toBeVisible();
  });

  it('shows the 409 name_taken inline for a duplicate project name, without clearing the form', async () => {
    const client = await loggedInClient();
    renderPage(client);

    await findProjectRowAsync('Nhân sự');
    fireEvent.change(screen.getByLabelText('Tên dự án'), { target: { value: 'Nhân sự' } });
    fireEvent.click(screen.getByRole('button', { name: 'Tạo dự án' }));

    expect(
      await screen.findByText('Tên dự án này đã được dùng trong tổ chức — hãy chọn một tên khác.'),
    ).toBeVisible();
    expect(screen.getByLabelText('Tên dự án')).toHaveValue('Nhân sự');
    // No duplicate row was created — exactly one project named "Nhân sự".
    expect(screen.getAllByText('Nhân sự', { selector: 'td > div > span' })).toHaveLength(1);
  });

  it('renames a project inline via PATCH, keeping its assigned collections', async () => {
    const client = await loggedInClient();
    renderPage(client);

    const hrRow = await findProjectRowAsync('Nhân sự');
    fireEvent.click(within(hrRow).getByRole('button', { name: 'Sửa tên dự án Nhân sự' }));
    fireEvent.change(screen.getByLabelText('Tên mới cho dự án Nhân sự'), {
      target: { value: 'Nhân sự & Văn hóa' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Lưu' }));

    const renamedRow = await findProjectRowAsync('Nhân sự & Văn hóa');
    expect(within(renamedRow).getByText('Employee Handbook')).toBeVisible();
    expect(screen.queryByText('Nhân sự', { selector: 'td > div > span' })).not.toBeInTheDocument();
  });

  it('cancelling an inline rename discards the edit', async () => {
    const client = await loggedInClient();
    renderPage(client);

    const hrRow = await findProjectRowAsync('Nhân sự');
    fireEvent.click(within(hrRow).getByRole('button', { name: 'Sửa tên dự án Nhân sự' }));
    fireEvent.change(screen.getByLabelText('Tên mới cho dự án Nhân sự'), {
      target: { value: 'Sẽ không lưu' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Hủy' }));

    expect(findProjectRow('Nhân sự')).toBeVisible();
    expect(screen.queryByText('Sẽ không lưu')).not.toBeInTheDocument();
  });

  it('assigns an unassigned collection to a project from the "Chưa thuộc dự án" section', async () => {
    const client = await loggedInClient();
    renderPage(client);

    await findProjectRowAsync('Sản phẩm');
    const assignSelect = screen.getByRole('combobox', {
      name: 'Gán dự án cho bộ sưu tập Product Specs',
    });
    fireEvent.click(assignSelect);
    fireEvent.click(await screen.findByRole('option', { name: 'Sản phẩm' }));

    await waitFor(() => {
      const productProjectRow = findProjectRow('Sản phẩm');
      expect(within(productProjectRow).getByText('Product Specs')).toBeVisible();
    });
    expect(
      screen.queryByRole('combobox', { name: 'Gán dự án cho bộ sưu tập Product Specs' }),
    ).not.toBeInTheDocument();
  });

  it('unassigns a collection from a project via its chip\'s "x" button, moving it back to "Chưa thuộc dự án"', async () => {
    const client = await loggedInClient();
    renderPage(client);

    const hrRow = await findProjectRowAsync('Nhân sự');
    fireEvent.click(
      within(hrRow).getByRole('button', {
        name: 'Bỏ gán bộ sưu tập Employee Handbook khỏi dự án Nhân sự',
      }),
    );

    await waitFor(() => {
      const refreshedRow = findProjectRow('Nhân sự');
      expect(within(refreshedRow).getByText('Chưa có bộ sưu tập nào')).toBeVisible();
    });
    expect(
      await screen.findByRole('combobox', { name: 'Gán dự án cho bộ sưu tập Employee Handbook' }),
    ).toBeVisible();
  });
});
