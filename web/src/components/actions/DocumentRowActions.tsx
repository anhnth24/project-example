// P2-09 (plans/markhand-web/phase-2-web-spa.md §P2.4): download, delete,
// reindex, retry for one document row. Self-contained — this file is not
// mounted anywhere by this task; the caller wires it into the library row.
//
// Naming note, because it matters for correctness: the fixed contract this
// component must expose names its prop `document`. Destructuring it as
// `document` would shadow the DOM's global `document` for the rest of this
// file's scope — a real hazard, not a style nit, since `saveBlob.ts` (the
// one place that needs the real global) is a separate module specifically
// to avoid it, and this file uses `window` (not `document`) for the
// download menu's outside-click/Escape listeners for the same reason. The
// prop is destructured as `doc` below; the exported signature keeps the
// `document` name the caller depends on.
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { apiClient, type ApiClient } from '../../api/client';
import type { components } from '../../api/generated/contract';
import { Button, Modal, Notice } from '../ui';
import { describeActionError } from './actionErrors';
import {
  downloadDocumentVersion,
  requestDelete,
  requestReindex,
  type DownloadPurpose,
} from './documentActionsApi';
import { DeleteIcon, DownloadIcon, ReindexIcon, RetryIcon } from './icons';
import { useSingleFlightAction } from './useSingleFlightAction';

export interface DocumentRowActionsProps {
  document: components['schemas']['Document'];
  /** Called after reindex or delete settles successfully, so the caller can refetch/refresh its list. Never called for download (it doesn't change document state). */
  onChanged?: () => void;
  /** Injectable for tests; defaults to the app-wide singleton, same convention as `LibraryPage` and `AuthProvider`. */
  client?: ApiClient;
}

const GONE_STATES = new Set(['tombstoned', 'purged']);

export function DocumentRowActions({
  document: doc,
  onChanged,
  client = apiClient,
}: DocumentRowActionsProps): ReactNode {
  const [downloadMenuOpen, setDownloadMenuOpen] = useState(false);
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  const downloadRootRef = useRef<HTMLDivElement>(null);

  const downloadAction = useSingleFlightAction<void>();
  const reindexAction = useSingleFlightAction<{ jobId: string; created: boolean }>();
  const deleteAction = useSingleFlightAction<void>();

  // "Latest callback" ref — same convention as `useFocusTrap`'s `onCloseRef`
  // and `useScopeSafeRequest`'s `fnRef` — so the effects below never need
  // `onChanged` itself in their dependency array (a fresh function identity
  // from the caller every render must not re-fire them).
  const onChangedRef = useRef(onChanged);
  useEffect(() => {
    onChangedRef.current = onChanged;
  }, [onChanged]);

  useEffect(() => {
    if (reindexAction.phase === 'success') onChangedRef.current?.();
  }, [reindexAction.phase]);

  useEffect(() => {
    if (deleteAction.phase === 'success') onChangedRef.current?.();
  }, [deleteAction.phase]);

  useEffect(() => {
    if (!downloadMenuOpen) return;
    function handlePointerDown(event: PointerEvent) {
      if (!downloadRootRef.current?.contains(event.target as Node)) {
        setDownloadMenuOpen(false);
      }
    }
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') setDownloadMenuOpen(false);
    }
    window.addEventListener('pointerdown', handlePointerDown);
    window.addEventListener('keydown', handleKeyDown);
    return () => {
      window.removeEventListener('pointerdown', handlePointerDown);
      window.removeEventListener('keydown', handleKeyDown);
    };
  }, [downloadMenuOpen]);

  const isGone = GONE_STATES.has(doc.state) || deleteAction.phase === 'success';
  const isFailed = doc.state === 'failed';
  const anyBusy =
    downloadAction.phase === 'pending' ||
    reindexAction.phase === 'pending' ||
    deleteAction.phase === 'pending';
  const canDownload = !isGone && doc.currentVersionId !== null;

  function startDownload(purpose: DownloadPurpose) {
    setDownloadMenuOpen(false);
    const versionId = doc.currentVersionId;
    if (!versionId) return;
    const documentId = doc.id;
    const title = doc.title;
    downloadAction.dispatch(`download-${purpose}`, (signal) =>
      downloadDocumentVersion({ client, documentId, versionId, purpose, title, signal }),
    );
  }

  function startReindex() {
    const documentId = doc.id;
    reindexAction.dispatch('reindex', (signal) => requestReindex({ client, documentId, signal }));
  }

  function confirmDelete() {
    const documentId = doc.id;
    deleteAction.dispatch('delete', (signal) => requestDelete({ client, documentId, signal }));
    setDeleteConfirmOpen(false);
  }

  return (
    <div>
      <div
        style={{ display: 'flex', alignItems: 'center', gap: 'var(--space-2)', flexWrap: 'wrap' }}
      >
        <div ref={downloadRootRef} style={{ position: 'relative', display: 'inline-block' }}>
          <Button
            variant="secondary"
            size="sm"
            icon={<DownloadIcon />}
            loading={downloadAction.phase === 'pending'}
            disabled={anyBusy || !canDownload}
            aria-haspopup="menu"
            aria-expanded={downloadMenuOpen}
            onClick={() => setDownloadMenuOpen((open) => !open)}
          >
            Tải xuống
          </Button>
          {downloadMenuOpen && (
            <div
              role="menu"
              aria-label={`Chọn định dạng tải xuống cho ${doc.title}`}
              className="ui-select-menu"
              style={{
                position: 'absolute',
                top: '100%',
                left: 0,
                marginTop: 'var(--space-1)',
                zIndex: 20,
                width: 'max-content',
                minWidth: 220,
              }}
            >
              <button
                type="button"
                role="menuitem"
                className="ui-select-option"
                onClick={() => startDownload('markdown')}
              >
                <span>Markdown (.md)</span>
              </button>
              <button
                type="button"
                role="menuitem"
                className="ui-select-option"
                onClick={() => startDownload('original')}
              >
                <span>Tệp gốc</span>
              </button>
            </div>
          )}
        </div>

        <Button
          variant={isFailed ? 'primary' : 'secondary'}
          size="sm"
          icon={isFailed ? <RetryIcon /> : <ReindexIcon />}
          loading={reindexAction.phase === 'pending'}
          disabled={anyBusy || isGone}
          onClick={startReindex}
        >
          {isFailed ? 'Thử lại lập chỉ mục' : 'Lập chỉ mục lại'}
        </Button>

        <Button
          variant="danger"
          size="sm"
          icon={<DeleteIcon />}
          disabled={anyBusy || isGone}
          onClick={() => setDeleteConfirmOpen(true)}
        >
          Xóa
        </Button>
      </div>

      {isGone && (
        <Notice tone="info">
          Đã yêu cầu xóa tài liệu này. Việc dọn dẹp diễn ra trong nền — tài liệu có thể vẫn hiển thị
          trong danh sách một lúc trước khi biến mất hẳn.
        </Notice>
      )}

      {!isGone && isFailed && (
        <Notice tone="warning">
          Xử lý tài liệu trước đó thất bại. Markhand chưa có API thử lại chuyển đổi riêng — dùng
          &quot;Thử lại lập chỉ mục&quot; để đưa tài liệu vào lại hàng đợi xử lý.
        </Notice>
      )}

      {reindexAction.phase === 'success' && reindexAction.value && (
        <Notice tone="info">
          {reindexAction.value.created
            ? 'Đã đưa tài liệu vào hàng đợi lập chỉ mục.'
            : 'Đã có một tác vụ lập chỉ mục đang chạy cho tài liệu này — không tạo job mới (yêu cầu lặp lại có tính idempotent).'}
        </Notice>
      )}

      {downloadAction.phase === 'error' && (
        <Notice tone="error">{describeActionError(downloadAction.error, 'download')}</Notice>
      )}
      {reindexAction.phase === 'error' && (
        <Notice tone="error">{describeActionError(reindexAction.error, 'reindex')}</Notice>
      )}
      {deleteAction.phase === 'error' && (
        <Notice tone="error">{describeActionError(deleteAction.error, 'delete')}</Notice>
      )}

      {deleteConfirmOpen && (
        <Modal
          title="Xóa tài liệu này?"
          description={`"${doc.title}" sẽ được đánh dấu xóa. Việc dọn dẹp diễn ra trong nền, không tức thời — tài liệu có thể vẫn xuất hiện trong danh sách một thời gian ngắn trước khi biến mất hẳn.`}
          onClose={() => setDeleteConfirmOpen(false)}
          footer={
            <>
              <Button variant="ghost" onClick={() => setDeleteConfirmOpen(false)}>
                Hủy
              </Button>
              <Button variant="danger" onClick={confirmDelete} icon={<DeleteIcon />}>
                Xóa tài liệu
              </Button>
            </>
          }
        >
          <p>Thao tác này không thể hoàn tác từ giao diện.</p>
        </Modal>
      )}
    </div>
  );
}
