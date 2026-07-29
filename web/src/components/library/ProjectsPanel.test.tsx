// P2-18. Unlike `LibraryPage.test.tsx`'s bare-`client`-injection convention,
// `ProjectsPanel` reads the signed-in caller's permissions via
// `useAuth().hasPermission` (to gate the whole panel), so this file wraps in
// a real `AuthProvider` and restores its session the same way
// `AdminMembersPage.test.tsx` does — a raw `client.login()` call
// `AuthProvider` would never observe.
import { useState } from 'react';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createApiClient, type ApiClient } from '../../api/client';
import { AuthProvider } from '../../auth/AuthContext';
import { useScopeSafeRequest } from '../../hooks/useScopeSafeRequest';
import { installMockFetch, resetMockState, uninstallMockFetch } from '../../mocks';
import { getStore, mintTokenPair, ORG_A_ID } from '../../mocks/fixtures';
import { ScopeProvider } from '../../state/ScopeProvider';
import { grantDocUpload } from './testSupport';
import { ProjectsPanel } from './ProjectsPanel';

function seedSession(withDocUpload: boolean): ApiClient {
  const client = createApiClient({ baseUrl: '' });
  if (withDocUpload) grantDocUpload();
  const [user] = getStore().users;
  const { refreshToken } = mintTokenPair(user, ORG_A_ID);
  window.sessionStorage.setItem('markhand.refreshToken', refreshToken);
  return client;
}

/** Owns a real `GET /collections` fetch + refetch-on-`onChanged`, the same
 * way `LibraryPage` does — `ProjectsPanel` itself never refetches
 * collections (that's the caller's job), so this wrapper is needed for the
 * assigned/unassigned `<select>` value to actually update after a mutation. */
function Harness({ client }: { client: ApiClient }) {
  const [retry, setRetry] = useState(0);
  // `useScopeSafeRequest` (not a bare `client.request(...).then(...)`) so
  // this tolerates the same "AuthProvider hasn't finished restoring the
  // session yet" window every other real page in this codebase already
  // handles — a bare call here would otherwise throw `NoSessionError` on
  // the harness's very first render.
  const result = useScopeSafeRequest(
    (signal) => client.request('get', '/collections', { signal }),
    [client, retry],
  );
  return (
    <ProjectsPanel
      collections={result.data?.items ?? []}
      client={client}
      onChanged={() => setRetry((n) => n + 1)}
    />
  );
}

function renderPanel(client: ApiClient) {
  return render(
    <ScopeProvider>
      <AuthProvider client={client}>
        <Harness client={client} />
      </AuthProvider>
    </ScopeProvider>,
  );
}

describe('ProjectsPanel', () => {
  beforeEach(() => {
    installMockFetch();
    resetMockState();
  });

  afterEach(() => {
    cleanup();
    uninstallMockFetch();
  });

  it('renders nothing for a caller without doc.upload', async () => {
    const client = seedSession(false);
    renderPanel(client);
    // `hasPermission` is `false` both before and after the AuthProvider's
    // session restore settles (missing permission, not missing session), so
    // there is no in-between state where the heading could flash — waiting
    // a macrotask (`setTimeout`) is enough to prove it never appears at all
    // rather than merely "not yet".
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByText('Dự án')).not.toBeInTheDocument();
  });

  it('creates a project and assigns/unassigns a collection to it for a doc.upload holder', async () => {
    const client = seedSession(true);
    renderPanel(client);

    expect(await screen.findByRole('heading', { name: 'Dự án' })).toBeVisible();

    fireEvent.change(screen.getByLabelText('Tên dự án mới'), { target: { value: 'Marketing' } });
    fireEvent.click(screen.getByRole('button', { name: 'Tạo dự án' }));
    await waitFor(() => expect(screen.getByLabelText('Tên dự án mới')).toHaveValue(''));

    // Seeded: Product Specs (mockUuid(11)) starts unassigned. Re-queried
    // fresh (not held across the assign) because `onChanged`'s retry makes
    // `useScopeSafeRequest` report `data: undefined` for the whole re-run
    // (see that hook's own doc), which unmounts and remounts this row's
    // `<select>` while the collections refetch is in flight.
    const findAssignSelect = () =>
      screen.findByRole('combobox', { name: 'Dự án cho bộ sưu tập Product Specs' });
    expect(await findAssignSelect()).toHaveTextContent('Chưa thuộc dự án');
    fireEvent.click(await findAssignSelect());
    fireEvent.click(await screen.findByRole('option', { name: 'Marketing' }));

    await waitFor(async () => expect(await findAssignSelect()).toHaveTextContent('Marketing'));

    // Unassign again — `projectId: null`.
    fireEvent.click(await findAssignSelect());
    fireEvent.click(await screen.findByRole('option', { name: 'Chưa thuộc dự án' }));
    await waitFor(async () =>
      expect(await findAssignSelect()).toHaveTextContent('Chưa thuộc dự án'),
    );
  });
});
