// Chat panel: the "Hỏi đáp" tab's whole content — a scrolling log of
// question/answer turns (P2-10 baseline), each one an independent
// `POST /ask/stream` call — current/as-of/compare/history mode selector
// (every field here — `mode`/`asOf`/`documentId`/`versionA`/`versionB` — is a
// real `AskRequest` field, `api/generated/contract.ts`; nothing invented
// client-side) applies to whichever turn is submitted next.
//
// Architecture, locked by the task brief: the server stays single-turn — no
// history is ever sent to `/ask`/`/ask/stream`, no client-side "conversation
// memory" is faked into a fake context. Each turn owns its own `useAskStream`
// instance (`ChatTurnBubble`) — this file never touches `state/askStream.ts`'s
// reducer directly, only assembles requests and lays out turns.
//
// Chat history redesign (owner spec, part A): "session in-memory only, lost
// on reload" is retired now that server-side per-user history
// (`useChatHistory.ts`) exists. This component itself still never persists
// anything directly — it only reports a settled turn's final content up via
// `onTurnSettled` once its status becomes `completed`/`revoked` (an
// `error`/`cancelled` turn is never reported, per the task brief); the parent
// (`QaPage`) owns the actual `useChatHistory()` instance and decides whether
// that means creating a brand-new session or appending to the one already
// open. `activeSessionId`/`historicalTurns` flow the other way: whichever
// session the sidebar has open gets rendered (via `HistoricalTurnBubble`)
// ahead of whatever live turns this render of the composer adds on top of it
// — switching sessions (or starting a new one) always clears this
// component's own in-memory live turns first (see the `chat` state's reset
// condition below), so a stale live turn from a previously-open session can
// never bleed into a freshly-opened one.
//
// Scope-safety (P2-06): an org switch or logout must discard the whole chat
// history, not just abort in-flight requests — `useScopeSafeSse` (used
// inside every `ChatTurnBubble`'s `useAskStream`) already stops a live stream
// from delivering further messages the instant the scope epoch moves on, but
// it does not by itself erase what a bubble has already rendered from a
// *previous* org. So `turns` here is itself kept scoped to the epoch it was
// built under (and to whichever session is open), same "retained state keyed
// by epoch, discarded on mismatch" idiom `LibraryPage.tsx`'s `effectiveView`/
// `retainedDocuments` and `useScopeSafeRequest.ts`'s own `useRequestGeneration`
// use — adjusted while rendering, so no stale-org bubble is ever painted even
// for a frame.
//
// Document/version pickers for compare/history reuse whatever documents a
// search already turned up (`candidateDocuments`, from `SearchPanel`) plus
// the real `GET /documents/{documentId}/versions` endpoint — no invented
// picker data.
import { useEffect, useId, useRef, useState, type FormEvent } from 'react';
import { apiClient, type ApiClient } from '../../api/client';
import type { components } from '../../api/generated/contract';
import { useScopeSafeRequest } from '../../hooks/useScopeSafeRequest';
import { useScope } from '../../state/ScopeProvider';
import { SelectControl, type SelectOption } from '../ui';
import { ChatTurnBubble, type ChatTurnStatus } from './ChatTurnBubble';
import { HistoricalTurnBubble } from './HistoricalTurnBubble';
import { ProjectPicker } from './ProjectPicker';
import type { RecordableTurn } from './useChatHistory';
import type { SearchHit } from './SearchPanel';

type AskRequest = components['schemas']['AskRequest'];
type AskMode = NonNullable<AskRequest['mode']>;
type DocumentVersion = components['schemas']['DocumentVersion'];
type ChatTurnRecord = components['schemas']['ChatTurn'];

const MODE_OPTIONS: SelectOption[] = [
  { value: 'current', label: 'Hiện hành' },
  { value: 'as_of', label: 'Tại một thời điểm (as-of)' },
  { value: 'compare', label: 'So sánh 2 phiên bản' },
  { value: 'history', label: 'Lịch sử phiên bản' },
];

function versionLabel(version: DocumentVersion): string {
  return `Phiên bản ${version.versionNumber}${version.isCurrent ? ' (hiện hành)' : ''}`;
}

interface LiveTurn {
  readonly id: string;
  readonly question: string;
  readonly request: AskRequest;
}

/** A turn still needs the composer disabled/its own "Hủy" wired up until it reaches one of these — `'cancelled'` (client-initiated abort) counts as settled too — see `ChatTurnBubble`'s own doc for why `reset()` alone can't report that distinctly. */
function isTurnSettled(status: ChatTurnStatus | undefined): boolean {
  return (
    status === 'completed' || status === 'revoked' || status === 'error' || status === 'cancelled'
  );
}

export function ChatPanel({
  collectionIds,
  client = apiClient,
  candidateDocuments,
  onProjectIdsChange,
  activeSessionId,
  historicalTurns,
  historicalStatus,
  sessionSwitchToken,
  collectionNameById,
  onTurnSettled,
}: {
  collectionIds?: string[];
  client?: ApiClient;
  /** Documents a search already found — feeds the compare/history document picker below instead of asking for a raw UUID. */
  candidateDocuments: SearchHit[];
  /**
   * The multi-project picker lives in this composer, but its value must also
   * scope the "Tìm kiếm" tab's request — this callback is how the selection
   * reaches `QaPage` without lifting the picker's UI itself out of the
   * composer. Called with `[]` for "Tất cả dự án" and again with `[]`
   * whenever the org switches — never left pointing at a previous org's
   * project ids.
   */
  onProjectIdsChange?: (projectIds: string[]) => void;
  /** Whichever session the sidebar currently has open, or `undefined` for a fresh/not-yet-created conversation. */
  activeSessionId: string | undefined;
  /** That session's own turns (oldest first), already loaded — rendered ahead of any live turn this composer adds on top. */
  historicalTurns: ChatTurnRecord[];
  historicalStatus: 'idle' | 'loading' | 'success' | 'error';
  /** `useChatHistory`'s own `sessionSwitchToken` — see that hook's doc for why this (not `activeSessionId` directly) is what actually gates this component's live-turns reset. */
  sessionSwitchToken: number;
  /** `collectionId -> tên bộ sưu tập`, threaded down to every bubble's `CitationFootnotes`. */
  collectionNameById: ReadonlyMap<string, string>;
  /** Called once a live turn settles (`completed`/`revoked`) so the parent can persist it — never called for `error`/`cancelled` turns, per the task brief. */
  onTurnSettled?: (turn: RecordableTurn) => void;
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

  // Multi-project picker (part B) — reset to "Tất cả dự án" on an org switch,
  // same {epoch, ...} reset idiom `chat` (below) already uses.
  const [scope, setScope] = useState<{ epoch: number; projectIds: string[] }>(() => ({
    epoch,
    projectIds: [],
  }));
  let projectIds = scope.projectIds;
  if (scope.epoch !== epoch) {
    projectIds = [];
    setScope({ epoch, projectIds });
  }
  const projectsResult = useScopeSafeRequest(
    (signal) => client.request('get', '/projects', { signal }),
    [client],
  );
  const projects = projectsResult.data?.items ?? [];
  // Reports the effective projectIds to the parent exactly once per change
  // (including the epoch-reset above) — same adjust-state-while-rendering
  // idiom `SearchPanel.tsx`'s `onHitsChanged` reporting uses.
  const [reportedProjectIds, setReportedProjectIds] = useState<string[] | undefined>(undefined);
  if (
    reportedProjectIds === undefined ||
    reportedProjectIds.length !== projectIds.length ||
    reportedProjectIds.some((id, i) => id !== projectIds[i])
  ) {
    setReportedProjectIds(projectIds);
    onProjectIdsChange?.(projectIds);
  }

  // The chat log's own live turns — reset whenever the org switches OR the
  // user explicitly opens a different conversation (`sessionSwitchToken`,
  // bumped only by `startNewConversation()`/`selectSession()` — see that
  // field's own doc for why keying off `activeSessionId` directly would
  // wrongly wipe the very turn `recordTurn()` just finished persisting).
  const [chat, setChat] = useState<{
    epoch: number;
    sessionSwitchToken: number;
    turns: LiveTurn[];
  }>(() => ({ epoch, sessionSwitchToken, turns: [] }));
  let turns = chat.turns;
  if (chat.epoch !== epoch || chat.sessionSwitchToken !== sessionSwitchToken) {
    turns = [];
    setChat({ epoch, sessionSwitchToken, turns });
  }
  const nextTurnSeq = useRef(0);

  // Per-turn live status, reported up by each `ChatTurnBubble` — only the
  // status is lifted here (never the growing answer text), just enough to
  // gate the composer and know which turn "Hủy" currently targets.
  const [statuses, setStatuses] = useState<Record<string, ChatTurnStatus>>({});
  const abortFns = useRef<Record<string, () => void>>({});
  // Guards against reporting the same turn's settlement twice (belt: each
  // `ChatTurnBubble` only reports a terminal status once anyway, but this
  // keeps `onTurnSettled` itself trivially idempotent per turn id too).
  const reportedSettledRef = useRef<Set<string>>(new Set());

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
    if (projectIds.length > 0) request.projectIds = projectIds;
    if (mode === 'as_of' && asOf) request.asOf = new Date(asOf).toISOString();
    if (needsDocument && documentId) request.documentId = documentId;
    if (mode === 'compare') {
      if (versionA) request.versionA = versionA;
      if (versionB) request.versionB = versionB;
    }
    nextTurnSeq.current += 1;
    const id = `turn-${nextTurnSeq.current}`;
    setChat({ epoch, sessionSwitchToken, turns: [...turns, { id, question, request }] });
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

  const showSavedSessionNotice = activeSessionId !== undefined && historicalTurns.length > 0;

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
      <div
        role="log"
        aria-label="Lịch sử hỏi đáp"
        style={{
          display: 'grid',
          gap: 'var(--space-4)',
          maxHeight: '60vh',
          minHeight: '16rem',
          overflowY: 'auto',
          padding: 'var(--space-3)',
        }}
        className="card"
      >
        {showSavedSessionNotice && (
          <p className="text-muted" style={{ margin: 0 }}>
            Phiên đã lưu — tài liệu có thể đã thay đổi.
          </p>
        )}

        {historicalStatus === 'loading' && (
          <p className="text-muted">Đang tải lại cuộc trò chuyện…</p>
        )}
        {historicalStatus === 'error' && (
          <p className="text-muted">Không thể tải lại cuộc trò chuyện đã lưu.</p>
        )}

        {historicalTurns.map((turn) => (
          <HistoricalTurnBubble key={turn.id} turn={turn} collectionNameById={collectionNameById} />
        ))}

        {turns.length === 0 && historicalTurns.length === 0 && historicalStatus !== 'loading' && (
          <p className="text-muted">
            Chưa có câu hỏi nào trong cuộc trò chuyện này — đặt câu hỏi bên dưới.
          </p>
        )}

        {turns.map((turn) => (
          <ChatTurnBubble
            key={turn.id}
            turnId={turn.id}
            question={turn.question}
            request={turn.request}
            client={client}
            collectionNameById={collectionNameById}
            onStatusChange={(status, snapshot) => {
              setStatuses((prev) =>
                prev[turn.id] === status ? prev : { ...prev, [turn.id]: status },
              );
              if (
                (status === 'completed' || status === 'revoked') &&
                !reportedSettledRef.current.has(turn.id)
              ) {
                reportedSettledRef.current.add(turn.id);
                onTurnSettled?.({
                  question: turn.question,
                  answer: snapshot.answer,
                  answerMode: snapshot.answerMode,
                  citations: snapshot.citations,
                  warnings: snapshot.warnings,
                });
              }
            }}
            onReady={(abort) => {
              abortFns.current[turn.id] = abort;
            }}
          />
        ))}
      </div>

      <form onSubmit={submit} className="card" style={{ display: 'grid', gap: 'var(--space-3)' }}>
        <div
          style={{
            display: 'flex',
            gap: 'var(--space-3)',
            flexWrap: 'wrap',
            alignItems: 'flex-end',
          }}
        >
          <ProjectPicker
            projects={projects}
            selectedProjectIds={projectIds}
            onChange={(next) => setScope({ epoch, projectIds: next })}
          />

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
