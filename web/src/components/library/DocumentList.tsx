// The document list itself (plan P2.4). Renders with the existing `.table`
// component classes. Distinguishes three empty situations honestly: still
// loading, genuinely no documents in the collection, and documents present
// but none match the current client-side filter.
//
// Mount point for the row-actions agent (components/actions/**): the last
// `<td data-slot="document-row-actions:<documentId>">` per row is left
// empty on purpose — approve-intake/reindex/download/delete controls mount
// there, scoped by that document's id.
import { DocumentStateBadge } from './DocumentStateBadge';
import { extensionLabel, formatDateTime } from './documentPresentation';
import type { LibraryDocument } from './types';

export function DocumentList({
  items,
  totalOnPage,
  selectedDocumentId,
  onSelect,
  loading,
}: {
  /** Documents visible after the client-side search/status filter. */
  items: LibraryDocument[];
  /** Unfiltered count of documents on the current server page. */
  totalOnPage: number;
  selectedDocumentId: string | null;
  onSelect: (documentId: string) => void;
  loading: boolean;
}) {
  if (loading) {
    return <p className="text-muted">Đang tải danh sách tài liệu…</p>;
  }
  if (totalOnPage === 0) {
    return <p className="text-muted">Chưa có tài liệu nào trong bộ sưu tập này.</p>;
  }
  if (items.length === 0) {
    return <p className="text-muted">Không tìm thấy tài liệu phù hợp với bộ lọc hiện tại.</p>;
  }

  return (
    <table className="table" aria-label="Danh sách tài liệu">
      <thead>
        <tr>
          <th scope="col">Tài liệu</th>
          <th scope="col">Trạng thái</th>
          <th scope="col">Cập nhật</th>
          <th scope="col">
            <span className="text-muted">Thao tác</span>
          </th>
        </tr>
      </thead>
      <tbody>
        {items.map((doc) => {
          const isSelected = doc.id === selectedDocumentId;
          const ext = extensionLabel(doc.title);
          return (
            <tr key={doc.id}>
              <td>
                <button
                  type="button"
                  className="btn btn-ghost"
                  aria-pressed={isSelected}
                  onClick={() => onSelect(doc.id)}
                >
                  {ext && <span className="tag tag-neutral">{ext}</span>}
                  {doc.title}
                </button>
              </td>
              <td>
                <DocumentStateBadge state={doc.state} />
              </td>
              <td className="text-muted">{formatDateTime(doc.updatedAt)}</td>
              <td data-slot={`document-row-actions:${doc.id}`} />
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
