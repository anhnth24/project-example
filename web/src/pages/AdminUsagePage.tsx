import { Notice } from '../components/ui';

export function AdminUsagePage() {
  return (
    <section className="page" aria-labelledby="admin-usage-heading">
      <p className="eyebrow">Quản trị</p>
      <h1 id="admin-usage-heading">Sử dụng và hạn mức</h1>
      <p className="lede">
        Tổng hợp sử dụng, hạn mức và trạng thái reservation/job sẽ hiển thị ở đây khi có endpoint
        tổng hợp usage.
      </p>
      <Notice tone="info">Chỉ có quota header hiện tại, chưa có endpoint tổng hợp.</Notice>
    </section>
  );
}
