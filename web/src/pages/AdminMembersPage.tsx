// P2-11 (plans/markhand-web/phase-2-web-spa.md §P2.6): member/role admin —
// list members, change role/suspend/reactivate/remove, and invite/revoke.
// Every fetch goes through `useScopeSafeRequest` (never a raw `useEffect` +
// fetch) so an org switch discards stale/cross-tenant responses — see
// `LibraryPage.tsx`'s own module doc for the pattern this follows, including
// the "retain data across a refresh" trick below (`retainedMembers`/
// `retainedInvites`): without it, the refetch a mutation triggers would blank
// the table for a frame and unmount every row's `MemberRowActions` — taking
// any success/error notice it had just shown with it.
//
// Owner-tier gating ("admin không quản owner", 1C-02 acceptance): this page
// computes `isOwnerActive` from the signed-in caller's *own* row in the just-
// fetched members list (there is no separate "my role" field on `MeResponse`
// — see `auth/AuthContext.tsx`'s own module doc for why) and passes it down
// so `MemberRowActions`/`InviteForm` never render an owner-tier control
// (granting owner, or touching a currently-owner row) as available to a
// non-owner. This is UI convenience only, same caveat `RouteGuard.tsx` makes
// about its own `permission` prop — the server's 403 is the real authority,
// and every component downstream still surfaces one honestly if it happens
// anyway (a race: the caller's own owner status changed after this page
// loaded).
import { useState } from 'react';
import { apiClient, type ApiClient } from '../api/client';
import {
  InviteForm,
  InvitesTable,
  MembersTable,
  describeMemberReadError,
  isActiveOwner,
} from '../components/admin';
import { Notice } from '../components/ui';
import { useAuth } from '../auth/AuthContext';
import { useScopeSafeRequest } from '../hooks/useScopeSafeRequest';
import { useScope } from '../state/ScopeProvider';

export function AdminMembersPage({ client = apiClient }: { client?: ApiClient } = {}) {
  const { session } = useAuth();
  const currentUserId = session.status === 'authenticated' ? session.userId : undefined;
  const { epoch } = useScope();

  const [membersRetry, setMembersRetry] = useState(0);
  const membersResult = useScopeSafeRequest(
    (signal) => client.request('get', '/members', { signal }),
    [client, membersRetry],
  );
  // See module doc: keeps the table (and every row's own action state)
  // mounted across a mutation-triggered refetch instead of flashing back to
  // the loading placeholder. Retained only within the same scope epoch —
  // an org switch must still discard it, exactly like `LibraryPage.tsx`'s
  // `retainedDocuments`.
  const [retainedMembers, setRetainedMembers] = useState<{
    epoch: number;
    data: NonNullable<typeof membersResult.data>;
  } | null>(null);
  if (membersResult.data && retainedMembers?.data !== membersResult.data) {
    setRetainedMembers({ epoch, data: membersResult.data });
  }
  const membersData =
    membersResult.data ?? (retainedMembers?.epoch === epoch ? retainedMembers.data : undefined);
  const members = membersData?.items ?? [];

  const [invitesRetry, setInvitesRetry] = useState(0);
  const invitesResult = useScopeSafeRequest(
    (signal) => client.request('get', '/members/invites', { signal }),
    [client, invitesRetry],
  );
  const [retainedInvites, setRetainedInvites] = useState<{
    epoch: number;
    data: NonNullable<typeof invitesResult.data>;
  } | null>(null);
  if (invitesResult.data && retainedInvites?.data !== invitesResult.data) {
    setRetainedInvites({ epoch, data: invitesResult.data });
  }
  const invitesData =
    invitesResult.data ?? (retainedInvites?.epoch === epoch ? retainedInvites.data : undefined);
  const invites = invitesData?.items ?? [];

  const ownMembership = members.find((m) => m.userId === currentUserId);
  const isOwnerActive = isActiveOwner(ownMembership);

  function refreshMembers() {
    setMembersRetry((n) => n + 1);
  }
  function refreshInvites() {
    setInvitesRetry((n) => n + 1);
  }

  return (
    <section className="page" style={{ maxWidth: 'none' }} aria-labelledby="admin-members-heading">
      <p className="eyebrow">Quản trị</p>
      <h1 id="admin-members-heading">Thành viên và vai trò</h1>
      <p className="lede">
        Quản lý thành viên, vai trò và lời mời tham gia tổ chức. Chỉ chủ sở hữu (owner) đang hoạt
        động mới có thể cấp hoặc quản lý vai trò chủ sở hữu.
      </p>

      {membersResult.status === 'error' && (
        <Notice
          tone="error"
          action={
            <button type="button" className="btn btn-secondary btn-sm" onClick={refreshMembers}>
              Thử lại
            </button>
          }
        >
          {describeMemberReadError(membersResult.error)}
        </Notice>
      )}

      <div
        className="card"
        style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}
      >
        <h2 className="card-title">Thành viên</h2>
        <MembersTable
          members={members}
          currentUserId={currentUserId}
          isOwnerActive={isOwnerActive}
          loading={membersResult.status === 'loading' && membersData === undefined}
          onChanged={refreshMembers}
          client={client}
        />
      </div>

      <div
        className="card"
        style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}
      >
        <h2 className="card-title">Mời thành viên mới</h2>
        <InviteForm isOwnerActive={isOwnerActive} onCreated={refreshInvites} client={client} />
      </div>

      {invitesResult.status === 'error' && (
        <Notice
          tone="error"
          action={
            <button type="button" className="btn btn-secondary btn-sm" onClick={refreshInvites}>
              Thử lại
            </button>
          }
        >
          {describeMemberReadError(invitesResult.error)}
        </Notice>
      )}

      <div
        className="card"
        style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}
      >
        <h2 className="card-title">Lời mời</h2>
        <InvitesTable
          invites={invites}
          loading={invitesResult.status === 'loading' && invitesData === undefined}
          onChanged={refreshInvites}
          client={client}
        />
      </div>
    </section>
  );
}
