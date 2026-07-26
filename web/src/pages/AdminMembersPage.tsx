import { Notice } from '../components/ui';

export function AdminMembersPage() {
  return (
    <section className="page" aria-labelledby="admin-members-heading">
      <p className="eyebrow">Quản trị</p>
      <h1 id="admin-members-heading">Thành viên và vai trò</h1>
      <p className="lede">
        Danh sách thành viên, mời và đổi vai trò sẽ hiển thị ở đây khi API thành viên tồn tại.
      </p>
      <Notice tone="warning">Backend chưa có API thành viên/quyền (xem plan P2.6, B2).</Notice>
    </section>
  );
}
