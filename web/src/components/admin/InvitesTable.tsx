// P2-11: the invites list (open and terminal), same `.table` convention as
// `MembersTable.tsx`/`components/library/DocumentList.tsx`.
import { InviteRow } from './InviteRow';
import type { ApiClient } from '../../api/client';
import type { Invite } from './types';

export function InvitesTable({
  invites,
  loading,
  onChanged,
  client,
}: {
  invites: Invite[];
  loading: boolean;
  onChanged: () => void;
  client?: ApiClient;
}) {
  if (loading) {
    return <p className="text-muted">Đang tải danh sách lời mời…</p>;
  }
  if (invites.length === 0) {
    return <p className="text-muted">Chưa có lời mời nào.</p>;
  }

  return (
    <table className="table" aria-label="Danh sách lời mời">
      <thead>
        <tr>
          <th scope="col">Email</th>
          <th scope="col">Vai trò</th>
          <th scope="col">Trạng thái</th>
          <th scope="col">Hết hạn</th>
          <th scope="col">Quản lý</th>
        </tr>
      </thead>
      <tbody>
        {invites.map((invite) => (
          <InviteRow key={invite.id} invite={invite} onChanged={onChanged} client={client} />
        ))}
      </tbody>
    </table>
  );
}
