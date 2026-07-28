// Thin wrappers around the members/invites mutations the admin pages offer,
// kept out of the row/form components so those only orchestrate UI state and
// never touch `apiClient` directly — same split `documentActionsApi.ts` uses
// for `DocumentRowActions.tsx`.
import type { ApiClient } from '../../api/client';
import type { components } from '../../api/generated/contract';
import type { Membership, MembershipRole, MembershipState } from './types';

export interface CreateInviteParams {
  client: ApiClient;
  email: string;
  role: MembershipRole;
  /** Omitted entirely (not sent as `undefined`) lets the server apply its own default (7 days). */
  ttlSecs?: number;
  signal: AbortSignal;
}

/** `POST /members/invites`. The response's plaintext `token` is returned exactly once — callers must show it immediately and never attempt to refetch or persist it (see the response schema's own doc comment). */
export async function createInvite(
  params: CreateInviteParams,
): Promise<components['schemas']['CreateInviteResponse']> {
  const { client, email, role, ttlSecs, signal } = params;
  return client.request('post', '/members/invites', {
    body: ttlSecs === undefined ? { email, role } : { email, role, ttlSecs },
    signal,
  });
}

export interface RevokeInviteParams {
  client: ApiClient;
  inviteId: string;
  signal: AbortSignal;
}

/** `POST /members/invites/{inviteId}/revoke`. */
export async function revokeInvite(
  params: RevokeInviteParams,
): Promise<components['schemas']['Invite']> {
  const { client, inviteId, signal } = params;
  return client.request('post', '/members/invites/{inviteId}/revoke', {
    params: { path: { inviteId } },
    signal,
  });
}

export interface PatchMemberParams {
  client: ApiClient;
  userId: string;
  role?: MembershipRole;
  state?: MembershipState;
  signal: AbortSignal;
}

/** `PATCH /members/{userId}`. Exactly one of `role`/`state` per call from this UI (see `MemberRowActions.tsx`) even though the wire shape allows both at once. */
export async function patchMember(params: PatchMemberParams): Promise<Membership> {
  const { client, userId, role, state, signal } = params;
  return client.request('patch', '/members/{userId}', {
    params: { path: { userId } },
    body: { role, state },
    signal,
  });
}

export interface RemoveMemberParams {
  client: ApiClient;
  userId: string;
  signal: AbortSignal;
}

/** `DELETE /members/{userId}`. */
export async function removeMember(params: RemoveMemberParams): Promise<void> {
  const { client, userId, signal } = params;
  await client.request('delete', '/members/{userId}', {
    params: { path: { userId } },
    signal,
  });
}
