// Chat panel: a scrolling in-session history of question/answer turns, each
// one an independent `POST /ask/stream` call (P2-10 baseline, now P2-10 chat
// UI) — current/as-of/compare/history mode selector (every field here —
// `mode`/`asOf`/`documentId`/`versionA`/`versionB` — is a real `AskRequest`
// field, `api/generated/contract.ts`; nothing invented client-side) applies
// to whichever turn is submitted next, exactly like the single-turn
// `AskPanel` this replaces.
//
// Architecture, locked by the task brief: the server stays single-turn — no
// history is ever sent to `/ask`/`/ask/stream`, no client-side "conversation
// memory" is faked into a fake context. Chat history here is UI-only, kept in
// this component's React state for the lifetime of the session: it is never
// persisted (no localStorage — it can hold document content) and is lost on
// reload, which is the accepted, documented trade-off. Each turn owns its own
// `useAskStream` instance (`ChatTurnBubble`) — this file never touches
// `state/askStream.ts`'s reducer directly, only assembles requests and lays
// out turns.
//
// Scope-safety (P2-06): an org switch or logout must discard the whole chat
// history, not just abort in-flight requests — `useScopeSafeSse` (used
// inside every `ChatTurnBubble`'s `useAskStream`) already stops a live stream
// from delivering further messages the instant the scope epoch moves on, but
// it does not by itself erase what a bubble has already rendered from a
// *previous* org. So `turns` here is itself kept scoped to the epoch it was
// built under, same "retained state keyed by epoch, discarded on mismatch"
// idiom `LibraryPage.tsx`'s `effectiveView`/`retainedDocuments` and
// `useScopeSafeRequest.ts`'s own `useRequestGeneration` use — adjusted while
// rendering, so no stale-org bubble is ever painted even for a frame.
//
// Document/version pickers for compare/history reuse whatever documents a
// search already turned up (`candidateDocuments`, from `SearchPanel`) plus
// the real `GET /documents/{documentId}/versions` endpoint — no invented
// picker data.
//
// Citation deep-link gap (see `CitationCard.tsx`'s own doc for the full
// reasoning): the `CitationPin` the contract returns from `ask`/`ask/stream`
// carries no document/version id, so citations here render content only —
// `ChatTurnBubble` says so below the list instead of a silently-dead link.
import { useEffect, useId, useRef, useState, type FormEvent } from 'react';
import { apiClient, type ApiClient } from '../../api/client';
import type { components } from '../../api/generated/contract';
import { useScopeSafeRequest } from '../../hooks/useScopeSafeRequest';
import { useScope } from '../../state/ScopeProvider';
import { SelectControl, type SelectOption } from '../ui';
import { ChatTurnBubble, type ChatTurnStatus } from './ChatTurnBubble';
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

interface ChatTurn {
  readonly id: string;
  readonly question: string;
  readonly request: AskRequest;
}

/** A turn still needs the composer disabled/its own "Hủy" wired up until it reaches one of these — mirrors `AskPanel`'s old `isDone` check, now evaluated per turn instead of for one shared stream state. `'cancelled'` (client-initiated abort) counts as settled too — see `ChatTurnBubble`'s own doc for why `reset()` alone can't report that distinctly. */
function isTurnSettled(status: ChatTurnStatus | undefined): boolean {
  return (
    status === 'completed' || status === 'revoked' || status === 'error' || status === 'cancelled'
  );
}

export function ChatPanel({
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
  const { epoch } = useScope();

  const [question, setQuestion] = useState('');
  const [mode, setMode] = useState<AskMode>('current');
  const [asOf, setAsOf] = useState('');
  const [documentId, setDocumentId] = useState('');
  const [versionA, setVersionA] = useState('');
  const [versionB, setVersionB] = useState('');

  // The chat history itself, scoped to the epoch it was built under — see
  // this file's module doc for why a plain `useState<ChatTurn[]>` is not
  // enough by itself.
  const [chat, setChat] = useState<{ epoch: number; turns: ChatTurn[] }>(() => ({
    epoch,
    turns: [],
  }));
  let turns = chat.turns;
  if (chat.epoch !== epoch) {
    // Org switch/logout: discard every turn from the previous scope in this
    // same render, before anything from it can be painted (adjust-state-
    // while-rendering — same idiom `LibraryPage.tsx`'s `effectiveView` uses
    // for a `collectionId` change).
    turns = [];
    setChat({ epoch, turns });
  }
  const nextTurnSeq = useRef(0);

  // Per-turn live status, reported up by each `ChatTurnBubble` — only the
  // status is lifted here (never the growing answer text), just enough to
  // gate the composer and know which turn "Hủy" currently targets.
  const [statuses, setStatuses] = useState<Record<string, ChatTurnStatus>>({});
  const abortFns = useRef<Record<string, () => void>>({});

  const lastTurn = turns[turns.length - 1];
  const busy = lastTurn !== undefined && !isTurnSettled(statuses[lastTurn.id]);

  const inputRef = useRef<HTMLInputElement>(null);
  const wasBusyRef = useRef(false);
  useEffect(() => {
    if (wasBusyRef.current && !busy) {
      // A turn just settled and the composer became enabled again — return
      // focus to the input so the next question can be typed immediately,
      // without requiring a click (a11y requirement: "focus quay về ô nhập
      // sau khi gửi").
      inputRef.current?.focus();
    }
    wasBusyRef.current = busy;
  }, [busy]);

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
      // `title` is optional (see this file's + `SearchPanel.tsx`'s own module
      // docs: `hits` has no fixed wire shape) — the option label must never
      // fall back to the raw `documentId` uuid, so an untitled hit reads as
      // "Tài liệu không có tiêu đề" instead.
    ].map((h) => ({ value: h.documentId!, label: h.title ?? 'Tài liệu không có tiêu đề' })),
  ];
  const versionOptions: SelectOption[] = versions.map((v) => ({
    value: v.id,
    label: versionLabel(v),
  }));

  function submit(event: FormEvent) {
    event.preventDefault();
    if (question.trim() === '' || busy) return;
    const request: AskRequest = { question, collectionIds, mode, limit: 10 };
    if (mode === 'as_of' && asOf) request.asOf = new Date(asOf).toISOString();
    if (needsDocument && documentId) request.documentId = documentId;
    if (mode === 'compare') {
      if (versionA) request.versionA = versionA;
      if (versionB) request.versionB = versionB;
    }
    nextTurnSeq.current += 1;
    const id = `turn-${nextTurnSeq.current}`;
    setChat({ epoch, turns: [...turns, { id, question, request }] });
    setQuestion('');
    // Kept in the input (rather than moving to the submit button) even
    // though the field disables the instant this turn's status arrives —
    // "focus quay về ô nhập sau khi gửi" also covers the moment of sending
    // itself, not just once the turn later settles (handled above).
    inputRef.current?.focus();
  }

  function handleAbort() {
    if (lastTurn) abortFns.current[lastTurn.id]?.();
  }

  return (
    <div className="card" aria-labelledby="qa-ask-heading">
      <p className="eyebrow">Hỏi đáp</p>
      <h2 id="qa-ask-heading">Trò chuyện hỏi đáp có trích dẫn</h2>
      <p className="text-muted">
        Lịch sử hội thoại chỉ lưu tạm trong phiên làm việc này (mất khi tải lại trang) và không được
        gửi lên máy chủ — mỗi câu hỏi vẫn được máy chủ xử lý độc lập, không kèm ngữ cảnh các câu hỏi
        trước.
      </p>

      <div
        role="log"
        aria-label="Lịch sử hỏi đáp"
        style={{
          display: 'grid',
          gap: 'var(--space-4)',
          maxHeight: '32rem',
          overflowY: 'auto',
          marginTop: 'var(--space-3)',
          paddingRight: turns.length > 0 ? 'var(--space-2)' : undefined,
        }}
      >
        {turns.length === 0 && (
          <p className="text-muted">Chưa có câu hỏi nào trong phiên này — đặt câu hỏi bên dưới.</p>
        )}
        {turns.map((turn) => (
          <ChatTurnBubble
            key={turn.id}
            question={turn.question}
            request={turn.request}
            client={client}
            onStatusChange={(status) =>
              setStatuses((prev) =>
                prev[turn.id] === status ? prev : { ...prev, [turn.id]: status },
              )
            }
            onReady={(abort) => {
              abortFns.current[turn.id] = abort;
            }}
          />
        ))}
      </div>

      <form
        onSubmit={submit}
        style={{ display: 'grid', gap: 'var(--space-3)', marginTop: 'var(--space-4)' }}
      >
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

        <div className="field">
          <label htmlFor={questionId}>Câu hỏi</label>
          <input
            id={questionId}
            ref={inputRef}
            className="input"
            type="text"
            value={question}
            onChange={(event) => setQuestion(event.target.value)}
            disabled={busy}
            placeholder="Ví dụ: Chính sách nghỉ phép hiện tại là gì?"
          />
        </div>

        <div style={{ display: 'flex', gap: 'var(--space-2)' }}>
          <button
            type="submit"
            className="btn btn-primary"
            disabled={question.trim() === '' || busy}
          >
            Hỏi
          </button>
          {busy && (
            <button type="button" className="btn btn-secondary" onClick={handleAbort}>
              Hủy
            </button>
          )}
        </div>
      </form>
    </div>
  );
}
