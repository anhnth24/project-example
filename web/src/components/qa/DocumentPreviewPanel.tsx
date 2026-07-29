// Search-hit deep-link target: `GET /documents/{documentId}/preview`, the
// same endpoint/`SafeMarkdown` pattern P2-07's `DocumentPreview` uses — kept
// as its own small component here (rather than reusing that one directly)
// because this panel is keyed off a *search hit* (`documentId`/`versionId`
// the mock's `hits` shape carries — see `mocks/handlers/qa.ts`), not a
// `LibraryDocument` row, and has no per-document actions slot to render.
import { apiClient, type ApiClient } from '../../api/client';
import { SafeMarkdown } from '../SafeMarkdown';
import { Notice } from '../ui';
import { useScopeSafeRequest } from '../../hooks/useScopeSafeRequest';

export function DocumentPreviewPanel({
  documentId,
  versionId,
  client = apiClient,
}: {
  documentId: string;
  /** The specific version this citation/hit came from — passed as the `versionId` query param so a non-current hit previews *that* version, not silently the current one. */
  versionId?: string;
  client?: ApiClient;
}) {
  const result = useScopeSafeRequest(
    (signal) =>
      client.request('get', '/documents/{documentId}/preview', {
        params: { path: { documentId }, query: versionId ? { versionId } : undefined },
        signal,
      }),
    [client, documentId, versionId],
  );

  return (
    <aside className="card" aria-labelledby="qa-preview-heading" data-testid="qa-preview-panel">
      <p className="eyebrow">Xem trước tài liệu</p>
      <h3 id="qa-preview-heading">Bản xem trước</h3>
      {result.status === 'loading' && <p className="text-muted">Đang tải nội dung xem trước…</p>}
      {result.status === 'error' && <Notice tone="error">Không tải được bản xem trước.</Notice>}
      {result.status === 'success' && result.data && (
        <>
          <p className="text-muted">
            Phiên bản {result.data.versionNumber}
            {result.data.isCurrent === false ? ' (không phải bản hiện hành)' : ' (bản hiện hành)'}
          </p>
          <div className="card-body" data-testid="qa-preview-markdown">
            <SafeMarkdown>{result.data.markdown}</SafeMarkdown>
          </div>
        </>
      )}
    </aside>
  );
}
