// Part A of the owner's Q&A redesign spec: private per-user chat history
// (`listChatSessions`/`createChatSession`/`getChatSession`/`updateChatSession`/
// `deleteChatSession`/`appendChatTurn` — all real P2-19 contract operations,
// `api/generated/contract.ts`). Centralizes every piece of state
// `ChatHistorySidebar`/`QaPage`/`ChatPanel` all need to share about "which
// session is open" and "what's in it" so none of them re-derive it
// independently.
//
// Scope-safety (P2-06): the sessions list itself re-fetches on every scope
// epoch change for free (`useScopeSafeRequest`'s own contract), but
// `activeSessionId` and the accumulated "tải thêm" pages are plain component
// state, so both are reset here with the same {epoch, ...} adjust-while-
// rendering idiom `ChatPanel.tsx`'s own `chat`/`scope` state already uses —
// an org switch must never leave the sidebar pointed at a session id (or a
// page of titles) that belonged to the org just left.
import { useState } from 'react';
import { apiClient, type ApiClient } from '../../api/client';
import type { components } from '../../api/generated/contract';
import { useScopeSafeRequest } from '../../hooks/useScopeSafeRequest';
import { useScope } from '../../state/ScopeProvider';

type ChatSession = components['schemas']['ChatSession'];
type ChatTurn = components['schemas']['ChatTurn'];
type CitationPin = components['schemas']['CitationPin'];
type AnswerMode = components['schemas']['AppendChatTurnRequest']['answerMode'];

const ALLOWED_ANSWER_MODES = new Set<string>([
  'offline_extractive',
  'fallback_extractive',
  'local_llm',
  'cloud_llm',
  'subscription_cli',
  'llm_unverified',
]);
const FALLBACK_ANSWER_MODE: AnswerMode = 'offline_extractive';

/** Server wire mode -> one it's actually willing to persist. `state/askStream.ts`'s `answerMode` is deliberately never re-typed against the contract enum (that module's own doc: the server may ship a new wire string ahead of a contract regen here) — this is the one place that leniency meets a field the wire schema *does* pin down to a fixed union, so an unrecognized value degrades to a safe default instead of failing the whole append. */
function toAllowedAnswerMode(mode: string | undefined): AnswerMode {
  return mode !== undefined && ALLOWED_ANSWER_MODES.has(mode)
    ? (mode as AnswerMode)
    : FALLBACK_ANSWER_MODE;
}

const TITLE_PREVIEW_MAX = 80;

/** First-question -> session title, truncated so a very long question doesn't blow past `CreateChatSessionRequest.title`'s 200-char bound with room to spare. */
function titleFromQuestion(question: string): string {
  const trimmed = question.trim();
  if (trimmed.length <= TITLE_PREVIEW_MAX) return trimmed || 'Cuộc trò chuyện mới';
  return `${trimmed.slice(0, TITLE_PREVIEW_MAX - 1)}…`;
}

export interface RecordableTurn {
  question: string;
  answer: string;
  answerMode: string | undefined;
  citations: CitationPin[];
  warnings: string[];
}

interface SessionsPageState {
  epoch: number;
  items: ChatSession[];
  hasMore: boolean;
}

interface ActiveSessionState {
  epoch: number;
  id: string | undefined;
}

export interface UseChatHistoryResult {
  sessions: ChatSession[];
  sessionsStatus: 'loading' | 'success' | 'error';
  hasMoreSessions: boolean;
  loadMoreSessions(): void;
  refreshSessions(): void;

  activeSessionId: string | undefined;
  /** Turns loaded from the server for `activeSessionId`, oldest first — empty for a brand-new (not-yet-created) conversation. */
  historicalTurns: ChatTurn[];
  historicalStatus: 'idle' | 'loading' | 'success' | 'error';
  /**
   * Bumped ONLY by an explicit `startNewConversation()`/`selectSession()`
   * call — never by `recordTurn()` quietly assigning a brand-new session id
   * to the conversation already in progress. `ChatPanel` keys its own live
   * `turns` reset off this (plus the scope epoch), not off `activeSessionId`
   * directly: `activeSessionId` itself flips from `undefined` to a real id
   * the instant `recordTurn` creates a session for the current conversation,
   * and that transition must NOT clear the very turn that's still being
   * displayed live — only a genuine "the user opened a different
   * conversation" should.
   */
  sessionSwitchToken: number;

  startNewConversation(): void;
  selectSession(sessionId: string): void;

  renameSession(sessionId: string, title: string): Promise<void>;
  renamingSessionId: string | undefined;
  renameError: unknown;

  deleteSession(sessionId: string): Promise<void>;
  deletingSessionId: string | undefined;
  deleteError: unknown;

  /**
   * Persists one settled live turn — creates a session first (title = the
   * turn's own question, truncated) if none is active yet, then appends.
   * Fire-and-forget from the caller's point of view (never throws into the
   * chat UI); a failure surfaces via `appendError` only, per the task brief's
   * "Append lỗi → notice nhỏ không phá chat".
   */
  recordTurn(turn: RecordableTurn): void;
  appendError: string | undefined;
  dismissAppendError(): void;
}

export function useChatHistory(client: ApiClient = apiClient): UseChatHistoryResult {
  const { epoch } = useScope();

  // --- Sessions list (paginated via "tải thêm") ---------------------------
  const [cursor, setCursor] = useState<string | undefined>(undefined);
  const [sessionsRefresh, setSessionsRefresh] = useState(0);
  const sessionsResult = useScopeSafeRequest(
    (signal) =>
      client.request('get', '/chat-sessions', {
        params: { query: cursor ? { cursor } : undefined },
        signal,
      }),
    [client, cursor, sessionsRefresh],
  );

  const [pages, setPages] = useState<SessionsPageState>(() => ({
    epoch,
    items: [],
    hasMore: false,
  }));
  const [mergedFor, setMergedFor] = useState<unknown>(undefined);

  if (pages.epoch !== epoch) {
    setPages({ epoch, items: [], hasMore: false });
    setMergedFor(undefined);
    if (cursor !== undefined) setCursor(undefined);
  } else if (sessionsResult.data && mergedFor !== sessionsResult.data) {
    setMergedFor(sessionsResult.data);
    const items =
      cursor === undefined
        ? sessionsResult.data.items
        : [...pages.items, ...sessionsResult.data.items];
    setPages({ epoch, items, hasMore: sessionsResult.data.page.hasMore });
  }

  function refreshSessionsFromScratch() {
    setPages({ epoch, items: [], hasMore: false });
    setMergedFor(undefined);
    setCursor(undefined);
    setSessionsRefresh((n) => n + 1);
  }

  function loadMoreSessions() {
    const next = sessionsResult.data?.page.nextCursor;
    if (next) setCursor(next);
  }

  const sessionsStatus: 'loading' | 'success' | 'error' =
    pages.items.length === 0 && sessionsResult.status === 'loading'
      ? 'loading'
      : pages.items.length === 0 && sessionsResult.status === 'error'
        ? 'error'
        : 'success';

  // --- Active session + its loaded transcript -----------------------------
  const [activeSession, setActiveSession] = useState<ActiveSessionState>(() => ({
    epoch,
    id: undefined,
  }));
  let activeSessionId = activeSession.id;
  if (activeSession.epoch !== epoch) {
    activeSessionId = undefined;
    setActiveSession({ epoch, id: undefined });
  }

  const historicalResult = useScopeSafeRequest(
    async (signal) => {
      if (!activeSessionId) return null;
      return client.request('get', '/chat-sessions/{sessionId}', {
        params: { path: { sessionId: activeSessionId } },
        signal,
      });
    },
    [client, activeSessionId],
  );
  const historicalTurns = historicalResult.data?.turns ?? [];
  const historicalStatus: 'idle' | 'loading' | 'success' | 'error' =
    activeSessionId === undefined ? 'idle' : historicalResult.status;

  const [sessionSwitchToken, setSessionSwitchToken] = useState(0);

  function startNewConversation() {
    setActiveSession({ epoch, id: undefined });
    setSessionSwitchToken((n) => n + 1);
  }

  function selectSession(sessionId: string) {
    setActiveSession({ epoch, id: sessionId });
    setSessionSwitchToken((n) => n + 1);
  }

  // --- Rename / delete -----------------------------------------------------
  const [renamingSessionId, setRenamingSessionId] = useState<string | undefined>(undefined);
  const [renameError, setRenameError] = useState<unknown>(undefined);

  async function renameSession(sessionId: string, title: string): Promise<void> {
    setRenamingSessionId(sessionId);
    setRenameError(undefined);
    try {
      await client.request('patch', '/chat-sessions/{sessionId}', {
        params: { path: { sessionId } },
        body: { title },
      });
      refreshSessionsFromScratch();
    } catch (error) {
      setRenameError(error);
      throw error;
    } finally {
      setRenamingSessionId(undefined);
    }
  }

  const [deletingSessionId, setDeletingSessionId] = useState<string | undefined>(undefined);
  const [deleteError, setDeleteError] = useState<unknown>(undefined);

  async function deleteSession(sessionId: string): Promise<void> {
    setDeletingSessionId(sessionId);
    setDeleteError(undefined);
    try {
      await client.request('delete', '/chat-sessions/{sessionId}', {
        params: { path: { sessionId } },
      });
      if (activeSessionId === sessionId) setActiveSession({ epoch, id: undefined });
      refreshSessionsFromScratch();
    } catch (error) {
      setDeleteError(error);
      throw error;
    } finally {
      setDeletingSessionId(undefined);
    }
  }

  // --- Recording a settled live turn ----------------------------------------
  const [appendError, setAppendError] = useState<string | undefined>(undefined);

  function recordTurn(turn: RecordableTurn): void {
    void (async () => {
      try {
        let sessionId = activeSessionId;
        if (!sessionId) {
          const created = await client.request('post', '/chat-sessions', {
            body: { title: titleFromQuestion(turn.question) },
          });
          sessionId = created.id;
          setActiveSession({ epoch, id: sessionId });
        }
        await client.request('post', '/chat-sessions/{sessionId}/turns', {
          params: { path: { sessionId } },
          body: {
            question: turn.question,
            answer: turn.answer,
            answerMode: toAllowedAnswerMode(turn.answerMode),
            citations: turn.citations,
            warnings: turn.warnings,
          },
        });
        refreshSessionsFromScratch();
      } catch {
        setAppendError(
          'Không thể lưu lượt hỏi đáp này vào lịch sử — cuộc trò chuyện vẫn tiếp tục bình thường.',
        );
      }
    })();
  }

  function dismissAppendError() {
    setAppendError(undefined);
  }

  return {
    sessions: pages.items,
    sessionsStatus,
    hasMoreSessions: pages.hasMore,
    loadMoreSessions,
    refreshSessions: refreshSessionsFromScratch,

    activeSessionId,
    historicalTurns,
    historicalStatus,
    sessionSwitchToken,

    startNewConversation,
    selectSession,

    renameSession,
    renamingSessionId,
    renameError,

    deleteSession,
    deletingSessionId,
    deleteError,

    recordTurn,
    appendError,
    dismissAppendError,
  };
}
