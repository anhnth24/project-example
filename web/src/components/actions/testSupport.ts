// Shared test-only plumbing for `components/actions/**`'s tests. Not
// imported by any production file — safe to keep alongside the tests it
// supports without touching `mocks/**`/`api/**` themselves.
import { apiClient } from '../../api/client';
import { getStore, mintTokenPair } from '../../mocks/fixtures';

/**
 * Logs the shared `apiClient` singleton in as the mock's seeded demo user,
 * bypassing `POST /auth/login` (this component never calls it) by minting a
 * token pair directly against the mock store, the same way
 * `AuthContext.test.tsx` avoids re-testing login in every unrelated test.
 * Call after `resetMockState()`; call `apiClient.sessionManager.clear()`
 * in `afterEach` since `apiClient` is a module-level singleton shared by
 * every test in a file.
 */
export function signInDemoUser(): void {
  const [user] = getStore().users;
  const { accessToken, refreshToken } = mintTokenPair(user);
  apiClient.sessionManager.setTokens({
    accessToken,
    refreshToken,
    tokenType: 'Bearer',
    expiresIn: 3600,
    orgId: user.orgId,
    userId: user.userId,
  });
}
