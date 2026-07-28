// P2-06/P2-15 org switch. Renders the real component against the real mock
// server (`mocks/handlers/orgs.ts`), same convention `AdminMembersPage.test.tsx`
// uses — no hand-stubbed `apiClient`. Session is restored through
// `AuthProvider`'s own persisted-refresh-token bootstrap (via
// `mintTokenPair`), same shortcut `AdminMembersPage.test.tsx`'s
// `loggedInClient()` uses, so every test starts already authenticated as org
// A without driving the login form.
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createApiClient, type ApiClient } from '../../api/client';
import { AuthProvider } from '../../auth/AuthContext';
import { installMockFetch, mockControl, resetMockState, uninstallMockFetch } from '../../mocks';
import { getStore, mintTokenPair, ORG_A_ID, ORG_B_ID } from '../../mocks/fixtures';
import { RouterProvider } from '../../state/RouterProvider';
import { ScopeProvider, useScope } from '../../state/ScopeProvider';
import { OrgSwitch } from './OrgSwitch';
import { seedThirdOrgMembership } from './testSupport';

function loggedInClient(): ApiClient {
  const client = createApiClient({ baseUrl: '' });
  const [user] = getStore().users;
  const { refreshToken } = mintTokenPair(user);
  window.sessionStorage.setItem('markhand.refreshToken', refreshToken);
  return client;
}

function ScopeProbe() {
  const { scope } = useScope();
  return <span data-testid="scope-org">{scope ? scope.orgId : 'null'}</span>;
}

function renderOrgSwitch(client: ApiClient) {
  return render(
    <RouterProvider>
      <ScopeProvider>
        <AuthProvider client={client}>
          <OrgSwitch client={client} />
          <ScopeProbe />
        </AuthProvider>
      </ScopeProvider>
    </RouterProvider>,
  );
}

async function renderSignedIn(): Promise<ApiClient> {
  const client = loggedInClient();
  renderOrgSwitch(client);
  await waitFor(() => expect(screen.getByTestId('scope-org')).toHaveTextContent(ORG_A_ID));
  return client;
}

function openPopover(): void {
  fireEvent.click(screen.getByRole('button', { name: 'Đơn vị hiện tại' }));
}

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

describe('OrgSwitch', () => {
  it('lists every org the caller is an active member of, marking the current one', async () => {
    await renderSignedIn();
    openPopover();

    await waitFor(() => expect(screen.getByText('Acme Co')).toBeVisible());
    expect(screen.getByText('Globex Labs')).toBeVisible();

    const currentOption = screen.getByText('Acme Co').closest('button');
    expect(currentOption).toHaveAttribute('aria-current', 'true');
    expect(currentOption).toBeDisabled();

    const otherOption = screen.getByText('Globex Labs').closest('button');
    expect(otherOption).not.toBeDisabled();
    expect(otherOption).not.toHaveAttribute('aria-current');
  });

  it('switching org swaps the session to org B, bumps scope, closes the popover, and navigates home', async () => {
    window.history.pushState(null, '', '/library/some-collection');
    await renderSignedIn();
    openPopover();
    await waitFor(() => expect(screen.getByText('Globex Labs')).toBeVisible());

    fireEvent.click(screen.getByText('Globex Labs').closest('button')!);

    await waitFor(() => expect(screen.getByTestId('scope-org')).toHaveTextContent(ORG_B_ID));
    // Popover closes on success.
    expect(screen.queryByRole('dialog', { name: 'Đơn vị hiện tại' })).not.toBeInTheDocument();
    // Navigates back to a neutral, org-agnostic route rather than staying on
    // a path that named something specific to the org just left.
    expect(window.location.pathname).toBe('/');
  });

  it('a denied switch (403) shows an accessible error and leaves org A active', async () => {
    await renderSignedIn();
    mockControl.forceStatus('switchOrg', 403, { times: 1 });
    openPopover();
    await waitFor(() => expect(screen.getByText('Globex Labs')).toBeVisible());

    fireEvent.click(screen.getByText('Globex Labs').closest('button')!);

    await waitFor(() => expect(screen.getByRole('alert')).toBeVisible());
    expect(screen.getByTestId('scope-org')).toHaveTextContent(ORG_A_ID);
    // Still open — the failure is shown in place, not as a silent close.
    expect(screen.getByRole('dialog', { name: 'Đơn vị hiện tại' })).toBeVisible();
  });

  it('a second org click while a switch is already pending is a no-op — the first click alone decides the outcome', async () => {
    const third = seedThirdOrgMembership();
    await renderSignedIn();
    openPopover();
    await waitFor(() => expect(screen.getByText('Globex Labs')).toBeVisible());
    const orgBButton = screen.getByText('Globex Labs').closest('button')!;
    const thirdButton = screen.getByText(third.orgName).closest('button')!;

    fireEvent.click(orgBButton);
    // Fired in the same tick, before org B's switch has resolved — the
    // pending guard (`pendingOrgId !== null`) must discard this one entirely
    // rather than starting a second in-flight switch.
    fireEvent.click(thirdButton);

    await waitFor(() => expect(screen.getByTestId('scope-org')).toHaveTextContent(ORG_B_ID));
    expect(screen.getByTestId('scope-org')).not.toHaveTextContent(third.orgId);
  });
});
