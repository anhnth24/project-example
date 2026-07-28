// P2-11: one invite row's revoke control. Split out from `InvitesTable.tsx`
// (rather than dispatching from inside that component's `.map`) for the same
// reason `DocumentRowActions` is its own component: a hook instance per row,
// not one shared across every row in the list.
import { useEffect, useRef, useState } from 'react';
import { Button, Modal, Notice } from '../ui';
import { revokeInvite } from './membersApi';
import {
  INVITE_STATUS_META,
  ROLE_META,
  describeMemberActionError,
  formatDateTime,
  inviteIsRevocable,
} from './memberPresentation';
import { useSingleFlightAction } from '../actions/useSingleFlightAction';
import { apiClient, type ApiClient } from '../../api/client';
import type { Invite } from './types';

export function InviteRow({
  invite,
  onChanged,
  client = apiClient,
}: {
  invite: Invite;
  onChanged: () => void;
  client?: ApiClient;
}) {
  const [confirmOpen, setConfirmOpen] = useState(false);
  const revokeAction = useSingleFlightAction<Invite>();

  const onChangedRef = useRef(onChanged);
  useEffect(() => {
    onChangedRef.current = onChanged;
  }, [onChanged]);
  useEffect(() => {
    if (revokeAction.phase === 'success') onChangedRef.current?.();
  }, [revokeAction.phase]);

  function confirmRevoke() {
    revokeAction.dispatch('revoke', (signal) =>
      revokeInvite({ client, inviteId: invite.id, signal }),
    );
    setConfirmOpen(false);
  }

  const revoked = revokeAction.phase === 'success';
  const canRevoke = inviteIsRevocable(invite) && !revoked;

  return (
    <tr>
      <td>{invite.email}</td>
      <td>
        <span className={`tag ${ROLE_META[invite.role].tagClass}`}>
          {ROLE_META[invite.role].label}
        </span>
      </td>
      <td>
        <span className={`tag ${INVITE_STATUS_META[invite.status].tagClass}`}>
          {INVITE_STATUS_META[invite.status].label}
        </span>
      </td>
      <td className="text-muted">{formatDateTime(invite.expiresAt)}</td>
      <td>
        <Button
          variant="danger"
          size="sm"
          loading={revokeAction.phase === 'pending'}
          disabled={!canRevoke}
          onClick={() => setConfirmOpen(true)}
        >
          Thu hồi
        </Button>
        {revokeAction.phase === 'error' && (
          <Notice tone="error">{describeMemberActionError(revokeAction.error, false)}</Notice>
        )}
        {confirmOpen && (
          <Modal
            title="Thu hồi lời mời này?"
            description={`Lời mời gửi tới "${invite.email}" sẽ không thể được chấp nhận nữa.`}
            onClose={() => setConfirmOpen(false)}
            footer={
              <>
                <Button variant="ghost" onClick={() => setConfirmOpen(false)}>
                  Hủy
                </Button>
                <Button variant="danger" onClick={confirmRevoke}>
                  Thu hồi lời mời
                </Button>
              </>
            }
          >
            <p>Người được mời sẽ cần một lời mời mới nếu bạn muốn thêm họ sau này.</p>
          </Modal>
        )}
      </td>
    </tr>
  );
}
