// Seam for P2.3 (Wave 1): real login/session/token/refresh logic is out of
// scope for this workspace-foundations task. This only establishes the
// shape — an in-memory session the shell and pages can read — so the auth
// agent can replace `AuthProvider`'s implementation (wire it to
// `api/client.ts`, keep the access token in memory, add org switch) without
// having to introduce the context/hook pair from scratch.
import { createContext, useContext, useMemo, type ReactNode } from 'react';

export type Role = 'owner' | 'admin' | 'editor' | 'viewer';

export type Session =
  | { status: 'anonymous' }
  | { status: 'authenticated'; email: string; role: Role };

export interface AuthContextValue {
  session: Session;
}

const defaultValue: AuthContextValue = { session: { status: 'anonymous' } };

const AuthContext = createContext<AuthContextValue>(defaultValue);

export function AuthProvider({
  children,
  session = defaultValue.session,
}: {
  children: ReactNode;
  session?: Session;
}) {
  const value = useMemo<AuthContextValue>(() => ({ session }), [session]);
  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthContextValue {
  return useContext(AuthContext);
}
