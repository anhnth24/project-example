// The document list itself (plan P2.4). Renders with the existing `.table`
// component classes. Distinguishes three empty situations honestly: still
// loading, genuinely no documents in the collection, and documents present
// but none match the current client-side filter.
//
// This table used to carry an empty fourth column reserved for per-row
// actions (`data-slot="document-row-actions:<id>"`). The column is gone and
// `DocumentRowActions` mounts once, in the preview panel next to this list,
// for two measured reasons rather than taste:
//   - Fit. The three controls are pill buttons with Vietnamese labels
//     ("Tải xuống", "Lập chỉ mục lại", "Xóa") and the component renders its
//     result/error notices inline beneath them. In a cell of a four-column
//     table inside the `minmax(0, 2fr)` list card, that wraps every row.
//   - Cost per row. Each instance registers `pointerdown` + `keydown`
//     listeners on `window` while its download menu is open and owns three
//     independent single-flight states. One mounted instance for the
//     selected document does the same job without N copies of that, and
//     without two live delete confirmations for one document (row + preview)
//     racing each other.
// Selecting a row is already how the preview loads, so acting on a document
// costs no extra step.
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
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
