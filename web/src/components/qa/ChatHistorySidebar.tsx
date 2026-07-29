// Part A of the owner's Q&A redesign spec: "Sidebar Lịch sử" — a collapsible
// left-hand panel listing the caller's own chat sessions
// (`listChatSessions`, most recently active first), with "Cuộc trò chuyện
// mới", inline rename (`updateChatSession`), and delete-with-confirm
// (`deleteChatSession`). All the actual data/mutation plumbing lives in
// `useChatHistory.ts` — this component is presentation + the small amount of
// local UI state (which row is being renamed, which row's delete confirm is
// open) that has no reason to live outside it.
import { useState } from 'react';
import type { components } from '../../api/generated/contract';
import { HttpApiError, NetworkError } from '../../api/client';
import { Button, Modal, Notice } from '../ui';

type ChatSession = components['schemas']['ChatSession'];

function describeSessionActionError(cause: unknown): string {
  if (cause instanceof HttpApiError) return `Máy chủ báo lỗi (${cause.status}): ${cause.message}`;
  if (cause instanceof NetworkError)
    return 'Không thể kết nối máy chủ. Kiểm tra kết nối và thử lại.';
  return 'Không thể thực hiện thao tác này. Vui lòng thử lại.';
}

function formatSessionTime(iso: string): string {
  return new Date(iso).toLocaleString('vi-VN', {
    day: '2-digit',
    month: '2-digit',
    year: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

export function ChatHistorySidebar({
  collapsed,
  onToggleCollapsed,
  sessions,
  sessionsStatus,
  hasMoreSessions,
  onLoadMore,
  activeSessionId,
  onSelectSession,
  onNewConversation,
  onRename,
  renamingSessionId,
  renameError,
  onDelete,
  deletingSessionId,
  deleteError,
}: {
  collapsed: boolean;
  onToggleCollapsed: () => void;
  sessions: ChatSession[];
  sessionsStatus: 'loading' | 'success' | 'error';
  hasMoreSessions: boolean;
  onLoadMore: () => void;
  activeSessionId: string | undefined;
  onSelectSession: (sessionId: string) => void;
  onNewConversation: () => void;
  onRename: (sessionId: string, title: string) => Promise<void>;
  renamingSessionId: string | undefined;
  renameError: unknown;
  onDelete: (sessionId: string) => Promise<void>;
  deletingSessionId: string | undefined;
  deleteError: unknown;
}) {
  const [editingId, setEditingId] = useState<string | undefined>(undefined);
  const [editValue, setEditValue] = useState('');
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | undefined>(undefined);

  if (collapsed) {
    return (
      <div className="card" style={{ padding: 'var(--space-2)' }}>
        <button
          type="button"
          className="btn btn-icon"
          aria-label="Mở rộng lịch sử hỏi đáp"
          aria-expanded={false}
          onClick={onToggleCollapsed}
        >
          »
        </button>
      </div>
    );
  }

  function startEditing(session: ChatSession) {
    setEditingId(session.id);
    setEditValue(session.title);
  }

  async function saveRename(sessionId: string) {
    const trimmed = editValue.trim();
    if (trimmed === '') return;
    try {
      await onRename(sessionId, trimmed);
      setEditingId(undefined);
    } catch {
      // renameError (from the hook) already carries this for display below;
      // stay in editing mode so the user can retry without retyping.
    }
  }

  return (
    <nav
      className="card"
      aria-label="Lịch sử hỏi đáp"
      style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)', minWidth: '16rem' }}
    >
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <h2 className="card-title" style={{ margin: 0 }}>
          Lịch sử
        </h2>
        <button
          type="button"
          className="btn btn-icon"
          aria-label="Thu gọn lịch sử hỏi đáp"
          aria-expanded={true}
          onClick={onToggleCollapsed}
        >
          «
        </button>
      </div>

      <Button variant="primary" size="sm" onClick={onNewConversation}>
        Cuộc trò chuyện mới
      </Button>

      {sessionsStatus === 'loading' && <p className="text-muted">Đang tải lịch sử…</p>}
      {sessionsStatus === 'error' && <Notice tone="error">Không thể tải lịch sử hỏi đáp.</Notice>}
      {sessionsStatus === 'success' && sessions.length === 0 && (
        <p className="text-muted">Chưa có cuộc trò chuyện nào được lưu.</p>
      )}

      <ul
        style={{ listStyle: 'none', margin: 0, padding: 0, display: 'grid', gap: 'var(--space-2)' }}
      >
        {sessions.map((session) => {
          const isActive = session.id === activeSessionId;
          const isEditing = editingId === session.id;
          const isRenaming = renamingSessionId === session.id;
          const isDeleting = deletingSessionId === session.id;
          return (
            <li key={session.id}>
              {isEditing ? (
                <div style={{ display: 'grid', gap: 'var(--space-1)' }}>
                  <input
                    className="input"
                    type="text"
                    aria-label={`Tên mới cho phiên ${session.title}`}
                    value={editValue}
                    onChange={(event) => setEditValue(event.target.value)}
                    autoFocus
                  />
                  <div style={{ display: 'flex', gap: 'var(--space-2)' }}>
                    <Button
                      variant="primary"
                      size="sm"
                      disabled={editValue.trim() === '' || isRenaming}
                      onClick={() => void saveRename(session.id)}
                    >
                      Lưu
                    </Button>
                    <Button
                      variant="secondary"
                      size="sm"
                      disabled={isRenaming}
                      onClick={() => setEditingId(undefined)}
                    >
                      Hủy
                    </Button>
                  </div>
                  {renameError !== undefined && (
                    <Notice tone="error">{describeSessionActionError(renameError)}</Notice>
                  )}
                </div>
              ) : (
                <div
                  className={`card ${isActive ? 'active' : ''}`}
                  style={{
                    padding: 'var(--space-2)',
                    display: 'flex',
                    flexDirection: 'column',
                    gap: 'var(--space-1)',
                    border: isActive
                      ? '2px solid var(--color-accent-700, currentColor)'
                      : undefined,
                  }}
                >
                  <button
                    type="button"
                    className="link-button"
                    style={{ textAlign: 'left', fontWeight: isActive ? 700 : 500 }}
                    aria-current={isActive ? 'true' : undefined}
                    onClick={() => onSelectSession(session.id)}
                  >
                    {session.title}
                  </button>
                  <span className="text-muted" style={{ fontSize: '0.85em' }}>
                    {formatSessionTime(session.updatedAt)}
                  </span>
                  <div style={{ display: 'flex', gap: 'var(--space-2)' }}>
                    <button
                      type="button"
                      className="btn btn-ghost btn-sm"
                      aria-label={`Đổi tên phiên ${session.title}`}
                      onClick={() => startEditing(session)}
                    >
                      Đổi tên
                    </button>
                    <button
                      type="button"
                      className="btn btn-ghost btn-sm"
                      aria-label={`Xóa phiên ${session.title}`}
                      disabled={isDeleting}
                      onClick={() => setConfirmDeleteId(session.id)}
                    >
                      Xóa
                    </button>
                  </div>
                </div>
              )}
            </li>
          );
        })}
      </ul>

      {hasMoreSessions && (
        <Button
          variant="secondary"
          size="sm"
          disabled={sessionsStatus === 'loading'}
          onClick={onLoadMore}
        >
          Tải thêm
        </Button>
      )}

      {deleteError !== undefined && confirmDeleteId === undefined && (
        <Notice tone="error">{describeSessionActionError(deleteError)}</Notice>
      )}

      {confirmDeleteId !== undefined && (
        <Modal
          title="Xóa cuộc trò chuyện này?"
          description="Toàn bộ câu hỏi, câu trả lời và trích dẫn đã lưu trong phiên này sẽ bị xóa vĩnh viễn."
          onClose={() => setConfirmDeleteId(undefined)}
          footer={
            <>
              <Button variant="ghost" onClick={() => setConfirmDeleteId(undefined)}>
                Hủy
              </Button>
              <Button
                variant="danger"
                onClick={async () => {
                  const id = confirmDeleteId;
                  setConfirmDeleteId(undefined);
                  await onDelete(id).catch(() => {
                    // deleteError (from the hook) already carries this for display above.
                  });
                }}
              >
                Xóa cuộc trò chuyện
              </Button>
            </>
          }
        >
          <p>Thao tác này không thể hoàn tác.</p>
        </Modal>
      )}
    </nav>
  );
}
