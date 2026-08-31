// Sanitized document preview panel (plan P2.4 §"sanitized Markdown
// preview"). All Markdown goes through `SafeMarkdown` — never
// `dangerouslySetInnerHTML` — per the task's hard rule.
//
// Per-document actions (components/actions/**) mount here, in the
// `document-actions:<documentId>` slot, passed in as `actions` rather than
// imported: this panel stays presentational and its tests keep rendering it
// without an API client. The library page is what supplies the node.
//
// The actions live here and not in each table row on purpose — see
// `DocumentList.tsx`'s note.
import type { ReactNode } from 'react';
import { SafeMarkdown } from '../SafeMarkdown';
import { Notice } from '../ui';
import { DocumentStateBadge } from './DocumentStateBadge';
import { formatDateTime } from './documentPresentation';
import type { LibraryDocument } from './types';

export type PreviewLoadState = 'no-version' | 'loading' | 'error' | 'success';

export function DocumentPreview({
  document,
  loadState,
  markdown,
  versionNumber,
  isCurrent,
  serverTruncated,
  errorMessage,
  actions,
}: {
  document: LibraryDocument | null;
  loadState?: PreviewLoadState;
  markdown?: string;
  versionNumber?: number;
  isCurrent?: boolean;
  serverTruncated?: boolean;
  errorMessage?: string;
  /** Rendered in the `document-actions:<id>` slot. Omitted in tests that only exercise preview rendering. */
  actions?: ReactNode;
}) {
  if (!document) {
    return (
      <aside
        className="card"
        style={{ minWidth: 0, maxWidth: '100%' }}
        aria-labelledby="library-preview-heading"
      >
        <p className="eyebrow">Xem trước</p>
        <h2 id="library-preview-heading">Chưa chọn tài liệu</h2>
        <p className="text-muted">Chọn một tài liệu ở danh sách bên trái để xem nội dung.</p>
      </aside>
    );
  }

  return (
    <aside
      className="card"
      style={{ minWidth: 0, maxWidth: '100%' }}
      aria-labelledby="library-preview-heading"
    >
      <p className="eyebrow">Xem trước</p>
      <h2 id="library-preview-heading">{document.title}</h2>
      <div
        style={{
          display: 'flex',
          gap: 'var(--space-2)',
          alignItems: 'center',
          flexWrap: 'wrap',
          minWidth: 0,
        }}
      >
        <DocumentStateBadge state={document.state} />
        <span className="text-muted">Cập nhật {formatDateTime(document.updatedAt)}</span>
      </div>

      <div data-slot={`document-actions:${document.id}`}>{actions}</div>

      {loadState === 'no-version' && (
        <p className="text-muted">
          {document.state === 'failed'
            ? 'Chuyển đổi thất bại — chưa có nội dung để xem trước.'
            : 'Đang xử lý — chưa có phiên bản nào sẵn sàng để xem trước.'}
        </p>
      )}
      {loadState === 'loading' && <p className="text-muted">Đang tải nội dung xem trước…</p>}
      {loadState === 'error' && <Notice tone="error">{errorMessage}</Notice>}
      {loadState === 'success' && markdown !== undefined && (
        <>
          <p className="text-muted">
            Phiên bản {versionNumber}
            {isCurrent === false ? ' (không phải bản hiện hành)' : ''}
          </p>
          {serverTruncated && (
            <Notice tone="warning">Máy chủ đã rút gọn nội dung xem trước này.</Notice>
          )}
          <div className="card-body" data-testid="document-preview-markdown">
            <SafeMarkdown>{markdown}</SafeMarkdown>
          </div>
        </>
      )}
    </aside>
  );
}
