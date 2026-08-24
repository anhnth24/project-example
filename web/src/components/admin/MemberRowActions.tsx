// P2-11: per-member row controls (role change, suspend/reactivate, remove).
// Owner-tier gating happens twice, deliberately: `AdminMembersPage.tsx`
// already omits/disables these controls for a non-owner viewer before this
// component ever mounts fully-enabled, but every mutation here is *also*
// individually guarded against `isOwnerActive` (role option disabling,
// `rowLocked`) — belt and suspenders, since the server is the only real
// authority (see `RouteGuard.tsx`'s own module doc for the same principle
// applied to whole routes). A 403 that gets through anyway (a race: the
// caller's own owner status changed between page load and this click) is
// still handled honestly via `describeMemberActionError`, not assumed
// impossible.
import { useEffect, useRef, useState } from 'react';
import { Button, Modal, Notice, SelectControl, type SelectOption } from '../ui';
import { patchMember, removeMember } from './membersApi';
import {
  ROLE_ORDER,
  ROLE_META,
  STATE_META,
  describeMemberActionError,
  formatDateTime,
  operationManagesOwner,
} from './memberPresentation';
import { ReactivateIcon, RemoveMemberIcon, SuspendIcon } from './icons';
import { useSingleFlightAction } from '../actions/useSingleFlightAction';
import { apiClient, type ApiClient } from '../../api/client';
import type { Membership, MembershipRole } from './types';

export interface MemberRowActionsProps {
  membership: Membership;
  /** Whether the *signed-in caller* is currently an active owner — never derived from `membership` itself, which describes the row being rendered, not the viewer. */
  isOwnerActive: boolean;
  /** True when `membership` is the signed-in caller's own row — purely a label ("Bạn"), never used to change what's allowed. */
  isSelf?: boolean;
  /** Called after a role change, suspend/reactivate, or removal settles successfully, so the caller can refetch the members list. */
  onChanged?: () => void;
  /** Injectable for tests; defaults to the app-wide singleton, same convention as `DocumentRowActions`. */
  client?: ApiClient;
}

export function MemberRowActions({
  membership,
  isOwnerActive,
  isSelf = false,
  onChanged,
  client = apiClient,
}: MemberRowActionsProps) {
  const [confirmRemoveOpen, setConfirmRemoveOpen] = useState(false);
  // Remembers which role a role-change attempt targeted, purely so a failed
  // attempt's error message can tell whether *that* attempt was owner-tier
  // (`grantsOwner`) even though `membership.role` itself never changed. State
  // changes/removal need no equivalent: they never grant owner, so their
  // owner-tier-ness is fully determined by `membership.role` at render time.
  const [attemptedRole, setAttemptedRole] = useState<MembershipRole | null>(null);

  const roleAction = useSingleFlightAction<Membership>();
  const stateAction = useSingleFlightAction<Membership>();
  const removeAction = useSingleFlightAction<void>();

  const onChangedRef = useRef(onChanged);
  useEffect(() => {
    onChangedRef.current = onChanged;
  }, [onChanged]);
  useEffect(() => {
    if (roleAction.phase === 'success') onChangedRef.current?.();
  }, [roleAction.phase]);
  useEffect(() => {
    if (stateAction.phase === 'success') onChangedRef.current?.();
  }, [stateAction.phase]);
  useEffect(() => {
    if (removeAction.phase === 'success') onChangedRef.current?.();
  }, [removeAction.phase]);

  const isRemoved = removeAction.phase === 'success';
  const anyBusy =
    roleAction.phase === 'pending' ||
    stateAction.phase === 'pending' ||
    removeAction.phase === 'pending';
  // Any mutation against a row that's *currently* owner requires the caller
  // to be an active owner (mirrors `operation_manages_owner` server-side —
  // see `memberPresentation.ts`). Locks the whole row, not just the role
  // select, since suspend/reactivate/remove of an owner is equally gated.
  const rowLocked = operationManagesOwner(membership.role, false) && !isOwnerActive;
  const disabled = anyBusy || isRemoved || rowLocked;

  const roleOptions: SelectOption[] = ROLE_ORDER.map((role) => ({
    value: role,
    label: ROLE_META[role].label,
    disabled: role === 'owner' && !isOwnerActive,
  }));

  function handleRoleChange(nextValue: string) {
    const role = nextValue as MembershipRole;
    if (role === membership.role) return;
    setAttemptedRole(role);
    roleAction.dispatch(`role-${role}`, (signal) =>
      patchMember({ client, userId: membership.userId, role, signal }),
    );
  }

  function toggleState() {
    const nextState = membership.state === 'active' ? 'suspended' : 'active';
    stateAction.dispatch(`state-${nextState}`, (signal) =>
      patchMember({ client, userId: membership.userId, state: nextState, signal }),
    );
  }

  function confirmRemove() {
    removeAction.dispatch('remove', (signal) =>
      removeMember({ client, userId: membership.userId, signal }),
    );
    setConfirmRemoveOpen(false);
  }

  const roleErrorOwnerTier = attemptedRole
    ? operationManagesOwner(membership.role, attemptedRole === 'owner')
    : false;
  const stateErrorOwnerTier = membership.role === 'owner';
  const removeErrorOwnerTier = membership.role === 'owner';

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)', minWidth: 0 }}>
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 'var(--space-2)',
          flexWrap: 'wrap',
          minWidth: 0,
        }}
      >
        <SelectControl
          value={membership.role}
          options={roleOptions}
          onChange={handleRoleChange}
          ariaLabel={`Vai trò của ${membership.displayName}`}
          disabled={disabled}
          compact
        />

        <Button
          variant="secondary"
          size="sm"
          icon={membership.state === 'active' ? <SuspendIcon /> : <ReactivateIcon />}
          loading={stateAction.phase === 'pending'}
          disabled={disabled}
          onClick={toggleState}
        >
          {membership.state === 'active' ? 'Tạm ngưng' : 'Kích hoạt lại'}
        </Button>

        <Button
          variant="danger"
          size="sm"
          icon={<RemoveMemberIcon />}
          disabled={disabled}
          onClick={() => setConfirmRemoveOpen(true)}
        >
          Xóa khỏi tổ chức
        </Button>

        {isSelf && <span className="tag tag-neutral">Bạn</span>}
      </div>

      {rowLocked && !isRemoved && (
        <Notice tone="info">
          Chỉ chủ sở hữu (owner) đang hoạt động mới có thể thay đổi vai trò, tạm ngưng hoặc xóa một
          chủ sở hữu khác.
        </Notice>
      )}

      {isRemoved && <Notice tone="info">Đã xóa thành viên này khỏi tổ chức.</Notice>}

      {roleAction.phase === 'error' && (
        <Notice tone="error">
          {describeMemberActionError(roleAction.error, roleErrorOwnerTier)}
        </Notice>
      )}
      {stateAction.phase === 'error' && (
        <Notice tone="error">
          {describeMemberActionError(stateAction.error, stateErrorOwnerTier)}
        </Notice>
      )}
      {removeAction.phase === 'error' && (
        <Notice tone="error">
          {describeMemberActionError(removeAction.error, removeErrorOwnerTier)}
        </Notice>
      )}

      {confirmRemoveOpen && (
        <Modal
          title="Xóa thành viên này khỏi tổ chức?"
          description={`Thành viên (${formatDateTime(membership.createdAt)}) sẽ mất toàn bộ quyền truy cập ngay lập tức. Thao tác này không thể hoàn tác từ giao diện.`}
          onClose={() => setConfirmRemoveOpen(false)}
          footer={
            <>
              <Button variant="ghost" onClick={() => setConfirmRemoveOpen(false)}>
                Hủy
              </Button>
              <Button variant="danger" onClick={confirmRemove} icon={<RemoveMemberIcon />}>
                Xóa thành viên
              </Button>
            </>
          }
        >
          <p>Kiểm tra kỹ vai trò và trạng thái hiện tại trước khi xác nhận.</p>
          <p>
            <span className={`tag ${ROLE_META[membership.role].tagClass}`}>
              {ROLE_META[membership.role].label}
            </span>{' '}
            <span className={`tag ${STATE_META[membership.state].tagClass}`}>
              {STATE_META[membership.state].label}
            </span>
          </p>
        </Modal>
      )}
    </div>
  );
}
