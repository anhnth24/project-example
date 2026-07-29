// Ask panel: question + current/as-of/compare/history mode selector (every
// field here — `mode`/`asOf`/`documentId`/`versionA`/`versionB` — is a real
// `AskRequest` field, `api/generated/contract.ts`; nothing invented
// client-side) streamed through `POST /ask/stream` via `useAskStream`
// (P2-04 transport, never native `EventSource`).
//
// Document/version pickers for compare/history reuse whatever documents a
// search already turned up (`candidateDocuments`, from `SearchPanel`) plus
// the real `GET /documents/{documentId}/versions` endpoint — no invented
// picker data.
//
// Citation deep-link gap (see `CitationCard.tsx`'s own doc for the full
// reasoning): the `CitationPin` the contract returns from `ask`/`ask/stream`
// carries no document/version id, so citations here render content only —
// the muted note below the list says so instead of a silently-dead link.
import { useId, useState, type FormEvent } from 'react';
import { apiClient, type ApiClient } from '../../api/client';
import type { components } from '../../api/generated/contract';
import { useScopeSafeRequest } from '../../hooks/useScopeSafeRequest';
import { Notice, SelectControl, type SelectOption } from '../ui';
import { describeAskStreamError } from '../../state/askStream';
import { CitationCard } from './CitationCard';
import { useAskStream } from './useAskStream';
import type { SearchHit } from './SearchPanel';

type AskRequest = components['schemas']['AskRequest'];
type AskMode = NonNullable<AskRequest['mode']>;
type DocumentVersion = components['schemas']['DocumentVersion'];

const MODE_OPTIONS: SelectOption[] = [
  { value: 'current', label: 'Hiện hành' },
  { value: 'as_of', label: 'Tại một thời điểm (as-of)' },
  { value: 'compare', label: 'So sánh 2 phiên bản' },
  { value: 'history', label: 'Lịch sử phiên bản' },
];

function versionLabel(version: DocumentVersion): string {
  return `Phiên bản ${version.versionNumber}${version.isCurrent ? ' (hiện hành)' : ''}`;
}

export function AskPanel({
  collectionIds,
  client = apiClient,
  candidateDocuments,
}: {
  collectionIds?: string[];
  client?: ApiClient;
  /** Documents a search already found — feeds the compare/history document picker below instead of asking for a raw UUID. */
  candidateDocuments: SearchHit[];
}) {
  const questionId = useId();
  const asOfId = useId();
  const [question, setQuestion] = useState('');
  const [mode, setMode] = useState<AskMode>('current');
  const [asOf, setAsOf] = useState('');
  const [documentId, setDocumentId] = useState('');
  const [versionA, setVersionA] = useState('');
  const [versionB, setVersionB] = useState('');

  const needsDocument = mode === 'compare' || mode === 'history';
  const versionsResult = useScopeSafeRequest(
    async (signal) => {
      if (!documentId || !needsDocument) return null;
      return client.request('get', '/documents/{documentId}/versions', {
        params: { path: { documentId } },
        signal,
      });
    },
    [client, documentId, needsDocument],
  );
  const versions: DocumentVersion[] = versionsResult.data?.items ?? [];

  const documentOptions: SelectOption[] = [
    { value: '', label: 'Chọn tài liệu…' },
    ...[
      ...new Map(
        candidateDocuments.filter((h) => h.documentId).map((h) => [h.documentId!, h]),
      ).values(),
    ].map((h) => ({ value: h.documentId!, label: h.title ?? h.documentId! })),
  ];
  const versionOptions: SelectOption[] = versions.map((v) => ({
    value: v.id,
    label: versionLabel(v),
  }));

  const { state, ask, reset, isActive } = useAskStream(client.tokenProvider);

  function submit(event: FormEvent) {
    event.preventDefault();
    if (question.trim() === '') return;
    const request: AskRequest = { question, collectionIds, mode, limit: 10 };
    if (mode === 'as_of' && asOf) request.asOf = new Date(asOf).toISOString();
    if (needsDocument && documentId) request.documentId = documentId;
    if (mode === 'compare') {
      if (versionA) request.versionA = versionA;
      if (versionB) request.versionB = versionB;
    }
    ask(request);
  }

  const showFallbackBadge = state.answerMode === 'fallback_extractive';
  const isDone =
    state.status === 'completed' || state.status === 'revoked' || state.status === 'error';

  return (
    <div className="card" aria-labelledby="qa-ask-heading">
      <p className="eyebrow">Hỏi đáp</p>
      <h2 id="qa-ask-heading">Đặt câu hỏi có trích dẫn</h2>

      <form onSubmit={submit} style={{ display: 'grid', gap: 'var(--space-3)' }}>
        <div className="field">
          <label htmlFor={questionId}>Câu hỏi</label>
          <input
            id={questionId}
            className="input"
            type="text"
            value={question}
            onChange={(event) => setQuestion(event.target.value)}
            placeholder="Ví dụ: Chính sách nghỉ phép hiện tại là gì?"
          />
        </div>

        <div
          style={{
            display: 'flex',
            gap: 'var(--space-3)',
            flexWrap: 'wrap',
            alignItems: 'flex-end',
          }}
        >
          <div>
            <span id="qa-mode-label" className="field-label">
              Chế độ truy vấn
            </span>
            <SelectControl
              value={mode}
              options={MODE_OPTIONS}
              onChange={(value) => setMode(value as AskMode)}
              ariaLabel="Chế độ truy vấn"
            />
          </div>

          {mode === 'as_of' && (
            <div className="field">
              <label htmlFor={asOfId}>Thời điểm (as-of)</label>
              <input
                id={asOfId}
                className="input"
                type="datetime-local"
                value={asOf}
                onChange={(event) => setAsOf(event.target.value)}
              />
            </div>
          )}

          {needsDocument && (
            <div>
              <span className="field-label">Tài liệu</span>
              <SelectControl
                value={documentId}
                options={
                  documentOptions.length > 1
                    ? documentOptions
                    : [{ value: '', label: 'Tìm kiếm trước để chọn tài liệu' }]
                }
                onChange={setDocumentId}
                ariaLabel="Tài liệu để so sánh hoặc xem lịch sử"
              />
            </div>
          )}

          {mode === 'compare' && documentId && (
            <>
              <div>
                <span className="field-label">Phiên bản A (cũ)</span>
                <SelectControl
                  value={versionA}
                  options={[{ value: '', label: 'Chọn…' }, ...versionOptions]}
                  onChange={setVersionA}
                  ariaLabel="Phiên bản A"
                />
              </div>
              <div>
                <span className="field-label">Phiên bản B (mới)</span>
                <SelectControl
                  value={versionB}
                  options={[{ value: '', label: 'Chọn…' }, ...versionOptions]}
                  onChange={setVersionB}
                  ariaLabel="Phiên bản B"
                />
              </div>
            </>
          )}
        </div>

        <div style={{ display: 'flex', gap: 'var(--space-2)' }}>
          <button type="submit" className="btn btn-primary" disabled={question.trim() === ''}>
            Hỏi
          </button>
          {isActive && !isDone && (
            <button type="button" className="btn btn-secondary" onClick={reset}>
              Hủy
            </button>
          )}
        </div>
      </form>

      {isActive && (
        <div style={{ marginTop: 'var(--space-4)' }}>
          {/* P2-14: accessible streaming status. `aria-live="polite"` on the
              growing answer text itself (not just a separate status line) is
              what the backlog acceptance asks for; screen readers coalesce
              rapid mutations to a polite region rather than announcing every
              token, which is the deliberate trade-off here over
              `aria-live="off"` (silent until the end) or `"assertive"`
              (would interrupt on every token). */}
          <div aria-live="polite" role="status">
            {state.status === 'streaming' && !state.answer && (
              <p className="text-muted">Đang tạo câu trả lời…</p>
            )}
            {state.answer && (
              <p data-testid="qa-answer" style={{ whiteSpace: 'pre-wrap' }}>
                {state.answer}
              </p>
            )}
            {state.status === 'completed' && showFallbackBadge && (
              <p>
                <span className="tag tag-neutral">Trả lời trích xuất (không qua LLM)</span>
              </p>
            )}
            {state.status === 'revoked' && (
              <Notice tone="warning">
                Trích dẫn đã bị thu hồi giữa chừng — câu trả lời phía trên có thể không đầy đủ.{' '}
                {describeAskStreamError(state.errorReason)}
              </Notice>
            )}
            {state.status === 'error' && (
              <Notice tone="error">{describeAskStreamError(state.errorReason)}</Notice>
            )}
          </div>

          {state.notices.map((notice, i) => (
            <p key={i} className="text-muted" role="status">
              {notice}
            </p>
          ))}

          {state.warnings.length > 0 && (
            <div style={{ marginTop: 'var(--space-2)' }}>
              {state.warnings.map((warning, i) => (
                <Notice key={i} tone="warning">
                  {warning}
                </Notice>
              ))}
            </div>
          )}

          {state.versionContext?.changeNote && (
            <p className="text-muted" style={{ marginTop: 'var(--space-2)' }}>
              {state.versionContext.changeNote}
            </p>
          )}

          {state.citations.length > 0 && (
            <div style={{ marginTop: 'var(--space-3)' }}>
              <p className="eyebrow">Trích dẫn</p>
              <ul
                style={{
                  listStyle: 'none',
                  margin: 0,
                  padding: 0,
                  display: 'grid',
                  gap: 'var(--space-2)',
                }}
              >
                {state.citations.map((citation) => (
                  <CitationCard key={citation.citeId} citation={citation} />
                ))}
              </ul>
              <p className="text-muted" style={{ marginTop: 'var(--space-2)' }}>
                Trích dẫn ở đây chưa kèm định danh tài liệu/phiên bản theo hợp đồng hiện tại — dùng
                ô Tìm kiếm phía trên để mở bản xem trước theo tài liệu.
              </p>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
