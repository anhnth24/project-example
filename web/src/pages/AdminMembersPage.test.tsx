// P2-11 (plans/markhand-web/phase-2-web-spa.md §P2.6). Renders the real page
// against the real mock server (`web/src/mocks/handlers/members.ts`), the
// same convention `LibraryPage.test.tsx` uses — no hand-stubbed `apiClient`.
// Unlike `LibraryPage`, this page reads the signed-in caller's own `userId`
// via `useAuth()` (to compute `isOwnerActive`), so tests wrap the page in a
// real `AuthProvider` and restore its session the same way
// `AuthContext.test.tsx`'s "restores an authenticated session from a
// persisted refresh token" test does, rather than a raw `client.login()`
// call `AuthProvider` would never see.
//
// Role-name text (e.g. "Chủ sở hữu") appears twice per row once a row's own
// role `<select>` renders — once as this row's `.tag`, once as the select's
// own displayed value, and possibly a third time as another row's tag or
// select — so every text query for a role/state name below is scoped either
// to a specific row (`within(row)`) or to the `span.tag` selector to
// disambiguate from the `<select>` trigger's own text.
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createApiClient, type ApiClient } from '../api/client';
import { installMockFetch, mockControl, resetMockState, uninstallMockFetch } from '../mocks';
import {
  getOrgMemberships,
  getStore,
  mintTokenPair,
  ORG_A_ID,
  SECOND_MEMBER_USER_ID,
  THIRD_MEMBER_USER_ID,
} from '../mocks/fixtures';
import { mockTimestamp, mockUuid } from '../mocks/ids';
import { grantMemberManage } from '../components/admin/testSupport';
import { AuthProvider } from '../auth/AuthContext';
import { ScopeProvider } from '../state/ScopeProvider';
import { AdminMembersPage } from './AdminMembersPage';

/** Logs the shared demo user in as a `member.manage` holder and restores the session through `AuthProvider`'s own persisted-refresh-token bootstrap path (see this file's module doc) instead of a raw `client.login()`, which `AuthProvider` would never observe. */
function loggedInClient(): ApiClient {
  const client = createApiClient({ baseUrl: '' });
  grantMemberManage();
  const [user] = getStore().users;
  const { refreshToken } = mintTokenPair(user);
  window.sessionStorage.setItem('markhand.refreshToken', refreshToken);
  return client;
}

function renderPage(client: ApiClient) {
  return render(
    <ScopeProvider>
      <AuthProvider client={client}>
        <AdminMembersPage client={client} />
      </AuthProvider>
    </ScopeProvider>,
  );
}

/** The `<tr>` whose role/state `.tag` (not its own `<select>`'s displayed text) reads `label`. */
function findRowByTagText(label: string): HTMLElement {
  return screen.getByText(label, { selector: 'span.tag' }).closest('tr') as HTMLElement;
}

/**
 * The `<tr>` for a given `userId` (rendered verbatim as `<code>`, see
 * `MembersTable.tsx`) — the only row locator that stays valid across a role
 * change, unlike `findRowByTagText`, whose label is exactly what a role
 * mutation changes. Load-bearing: an earlier version of the role-change test
 * below used `findByText('Biên tập viên', ...)` with no row scoping and
 * passed even with the refetch wiring deliberately broken, because the
 * seeded invite fixture (`moi-nguoi-moi@example.com`, role `editor`) renders
 * the exact same Vietnamese label in the *invites* table — a false positive
 * only mutation-testing caught.
 */
function findRowByUserId(userId: string): HTMLElement {
  return screen.getByText(userId).closest('tr') as HTMLElement;
}

beforeEach(() => {
  installMockFetch();
  resetMockState();
});

afterEach(() => {
  cleanup();
  uninstallMockFetch();
  window.sessionStorage.clear();
});

describe('AdminMembersPage', () => {
  it('renders the seeded members with their role and state', async () => {
    renderPage(loggedInClient());

    expect(await screen.findByText('Chủ sở hữu', { selector: 'span.tag' })).toBeVisible();
    expect(screen.getByText('Quản trị viên', { selector: 'span.tag' })).toBeVisible();
    expect(screen.getByText('Người xem', { selector: 'span.tag' })).toBeVisible();
    expect(screen.getByText('Đã tạm ngưng', { selector: 'span.tag' })).toBeVisible();
    expect(screen.getAllByText('Đang hoạt động', { selector: 'span.tag' }).length).toBe(2);
  });

  describe('invite flow', () => {
    it('creates an invite and shows the one-time token exactly once', async () => {
      renderPage(loggedInClient());
      await screen.findByText('Chủ sở hữu', { selector: 'span.tag' });

      fireEvent.change(screen.getByLabelText('Email người được mời'), {
        target: { value: 'nguoi-moi@example.com' },
      });
      fireEvent.click(screen.getByRole('button', { name: 'Gửi lời mời' }));

      const dialog = await screen.findByRole('dialog', { name: 'Đã tạo lời mời' });
      const tokenInput = within(dialog).getByLabelText('Mã mời (một lần)') as HTMLInputElement;
      expect(tokenInput.value).toMatch(/^mock-invite-token\./);
      const token = tokenInput.value;

      // The invite list refetches and now includes it, but the token itself
      // is never shown again anywhere outside this one modal.
      fireEvent.click(within(dialog).getByRole('button', { name: 'Đã sao chép, đóng lại' }));
      expect(screen.queryByRole('dialog', { name: 'Đã tạo lời mời' })).not.toBeInTheDocument();
      expect(await screen.findByText('nguoi-moi@example.com')).toBeVisible();
      expect(screen.queryByText(token)).not.toBeInTheDocument();
    });
  });

  describe('member mutations', () => {
    it('changes a member role via PATCH and refreshes the list', async () => {
      renderPage(loggedInClient());
      await screen.findByText('Quản trị viên', { selector: 'span.tag' });

      const roleSelect = screen.getByRole('combobox', {
        name: `Vai trò của ${SECOND_MEMBER_USER_ID}`,
      });
      fireEvent.click(roleSelect);
      fireEvent.click(screen.getByRole('option', { name: 'Biên tập viên' }));

      await waitFor(() => {
        const membership = getOrgMemberships(ORG_A_ID).find(
          (m) => m.userId === SECOND_MEMBER_USER_ID,
        );
        expect(membership?.role).toBe('editor');
      });
      // Scoped to this member's own row — see `findRowByUserId`'s doc for why
      // an unscoped `findByText('Biên tập viên')` would be a false positive
      // here (the seeded invite fixture shares that exact label).
      await waitFor(() => {
        const row = findRowByUserId(SECOND_MEMBER_USER_ID);
        expect(within(row).getByText('Biên tập viên', { selector: 'span.tag' })).toBeVisible();
      });
    });

    it('reactivates a suspended member via PATCH and refreshes the list', async () => {
      renderPage(loggedInClient());
      await screen.findByText('Người xem', { selector: 'span.tag' });

      const row = findRowByTagText('Người xem');
      fireEvent.click(within(row).getByRole('button', { name: 'Kích hoạt lại' }));

      await waitFor(() => {
        const membership = getOrgMemberships(ORG_A_ID).find(
          (m) => m.userId === THIRD_MEMBER_USER_ID,
        );
        expect(membership?.state).toBe('active');
      });
    });

    it('removes a member via DELETE after confirmation, and refreshes the list', async () => {
      renderPage(loggedInClient());
      await screen.findByText('Quản trị viên', { selector: 'span.tag' });

      const row = findRowByTagText('Quản trị viên');
      fireEvent.click(within(row).getByRole('button', { name: 'Xóa khỏi tổ chức' }));
      fireEvent.click(screen.getByRole('button', { name: 'Xóa thành viên' }));

      await waitFor(() =>
        expect(getOrgMemberships(ORG_A_ID).some((m) => m.userId === SECOND_MEMBER_USER_ID)).toBe(
          false,
        ),
      );
      await waitFor(() =>
        expect(
          screen.queryByText('Quản trị viên', { selector: 'span.tag' }),
        ).not.toBeInTheDocument(),
      );
    });

    it('surfaces a 403 from the server clearly, even for an otherwise-permitted action', async () => {
      mockControl.forceStatus('patchMember', 403, { times: 1 });
      renderPage(loggedInClient());
      await screen.findByText('Người xem', { selector: 'span.tag' });

      const row = findRowByTagText('Người xem');
      fireEvent.click(within(row).getByRole('button', { name: 'Kích hoạt lại' }));

      const alert = await within(row).findByRole('alert');
      expect(alert).toHaveTextContent(/không có quyền/i);
    });
  });

  describe('owner-tier restriction ("admin không quản owner")', () => {
    it('hides owner-granting and locks an owner row for a non-owner caller', async () => {
      const client = loggedInClient();
      const roster = getOrgMemberships(ORG_A_ID);
      const callerRow = roster.find((m) => m.role === 'owner');
      expect(callerRow).toBeDefined();
      callerRow!.role = 'admin'; // the signed-in caller is no longer an owner...
      const otherOwnerId = mockUuid(40);
      roster.push({
        userId: otherOwnerId,
        role: 'owner',
        state: 'active',
        createdAt: mockTimestamp(0),
      }); // ...but the org still has one, elsewhere.

      renderPage(client);
      await screen.findByText('Chủ sở hữu', { selector: 'span.tag' }); // the other owner's row

      const inviteRoleSelect = screen.getByRole('combobox', { name: 'Vai trò cho lời mời' });
      fireEvent.click(inviteRoleSelect);
      expect(screen.getByRole('option', { name: 'Chủ sở hữu' })).toBeDisabled();

      const ownerRow = findRowByTagText('Chủ sở hữu');
      expect(within(ownerRow).getByRole('combobox')).toBeDisabled();
      expect(within(ownerRow).getByRole('button', { name: 'Tạm ngưng' })).toBeDisabled();
      expect(within(ownerRow).getByRole('button', { name: 'Xóa khỏi tổ chức' })).toBeDisabled();
      expect(within(ownerRow).getByText(/Chỉ chủ sở hữu/)).toBeVisible();
    });
  });

  describe('last_owner invariant (409)', () => {
    it('shows the specific last_owner message when suspending the sole active owner', async () => {
      renderPage(loggedInClient());
      await screen.findByText('Chủ sở hữu', { selector: 'span.tag' });

      const row = findRowByTagText('Chủ sở hữu');
      fireEvent.click(within(row).getByRole('button', { name: 'Tạm ngưng' }));

      expect(
        await within(row).findByText(/tổ chức phải luôn còn ít nhất một chủ sở hữu/),
      ).toBeVisible();
      // Fails closed: the membership itself must not have been mutated.
      const owner = getOrgMemberships(ORG_A_ID).find((m) => m.role === 'owner');
      expect(owner?.state).toBe('active');
    });
  });
});
