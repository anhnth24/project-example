import { Notice } from '../components/ui';

export function LibraryPage({ collectionId }: { collectionId?: string }) {
  return (
    <section className="page" aria-labelledby="library-heading">
      <p className="eyebrow">Thư viện</p>
      <h1 id="library-heading">
        {collectionId ? `Bộ sưu tập ${collectionId}` : 'Tất cả bộ sưu tập'}
      </h1>
      <p className="lede">
        Danh sách tài liệu, tải lên và xem trước sẽ hiển thị ở đây khi client API sẵn sàng.
      </p>
      <Notice tone="info">Chưa kết nối tới API thư viện.</Notice>
    </section>
  );
}
