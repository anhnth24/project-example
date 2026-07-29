// P2-11: the membership list itself. Renders with the existing `.table`
// component classes, same convention as `components/library/DocumentList.tsx`.
import { MemberRowActions } from './MemberRowActions';
import { ROLE_META, STATE_META, formatDateTime, memberInitials } from './memberPresentation';
import type { ApiClient } from '../../api/client';
import type { Membership } from './types';

export function MembersTable({
  members,
  currentUserId,
  isOwnerActive,
  loading,
  onChanged,
  client,
}: {
  members: Membership[];
  /** The signed-in caller's own `userId`, or `undefined` before the session is known — used only to label "Bạn", never to decide what's allowed. */
  currentUserId: string | undefined;
  isOwnerActive: boolean;
  loading: boolean;
  onChanged: () => void;
  client?: ApiClient;
}) {
  if (loading) {
    return <p className="text-muted">Đang tải danh sách thành viên…</p>;
  }
  if (members.length === 0) {
    return <p className="text-muted">Chưa có thành viên nào.</p>;
  }

  return (
    <table className="table" aria-label="Danh sách thành viên">
      <thead>
        <tr>
          <th scope="col">Thành viên</th>
          <th scope="col">Vai trò</th>
          <th scope="col">Trạng thái</th>
          <th scope="col">Tham gia</th>
          <th scope="col">Quản lý</th>
        </tr>
      </thead>
      <tbody>
        {members.map((membership) => (
          <tr key={membership.userId}>
            <td>
              <div className="member-identity">
                <span className="member-avatar" aria-hidden="true">
                  {memberInitials(membership.displayName)}
                </span>
                <div className="member-identity-text">
                  <span className="member-identity-name">{membership.displayName}</span>
                  <span className="member-identity-email text-muted">{membership.email}</span>
                </div>
                {membership.userId === currentUserId && (
                  <span className="tag tag-neutral">Bạn</span>
                )}
              </div>
            </td>
            <td>
              <span className={`tag ${ROLE_META[membership.role].tagClass}`}>
                {ROLE_META[membership.role].label}
              </span>
            </td>
            <td>
              <span className={`tag ${STATE_META[membership.state].tagClass}`}>
                {STATE_META[membership.state].label}
              </span>
            </td>
            <td className="text-muted">{formatDateTime(membership.createdAt)}</td>
            <td>
              <MemberRowActions
                membership={membership}
                isOwnerActive={isOwnerActive}
                isSelf={membership.userId === currentUserId}
                onChanged={onChanged}
                client={client}
              />
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
