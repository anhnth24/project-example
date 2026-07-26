import { Notice } from '../components/ui';

export function QaPage({ collectionId }: { collectionId?: string }) {
  return (
    <section className="page" aria-labelledby="qa-heading">
      <p className="eyebrow">Hỏi đáp</p>
      <h1 id="qa-heading">
        {collectionId ? `Hỏi đáp trên ${collectionId}` : 'Hỏi đáp trên toàn bộ thư viện'}
      </h1>
      <p className="lede">
        Tìm kiếm, câu trả lời có trích dẫn và trạng thái chỉ mục sẽ hiển thị ở đây khi client
        API/SSE sẵn sàng.
      </p>
      <div aria-live="polite">
        <Notice tone="info">Chưa kết nối tới API hỏi đáp.</Notice>
      </div>
    </section>
  );
}
