// P2-10 (Q&A). Pure reducer over `api/sse.ts`'s `SseMessage` stream for
// `POST /ask/stream` — no React/DOM here, same "state/** is plain logic,
// hooks/components consume it" split as `state/scope.ts`/`scopeCache.ts`.
//
// Ordering/dedupe: `SseConnection` itself already refuses to yield a
// duplicate or out-of-order `id`-bearing event (`advanceCursor` in
// `api/lastEventId.ts` — a violation there ends the stream as
// `protocol-violation`, never silently reapplied), so in practice this
// reducer never sees a real duplicate from a well-behaved connection. The
// `lastEventSequence` guard below is a second, independent belt for the same
// invariant — applied at the point the answer/citations state is actually
// built, not just at the transport's framing layer — kept deliberately
// redundant with the transport's own guarantee (mirrors
// `useScopeSafeRequest.ts`'s documented "belt and suspenders" layering) and
// exercised directly in `askStream.test.ts` by feeding a hand-built duplicate
// message the transport itself would never actually produce.
import type { components } from '../api/generated/contract';
import type { SseCloseReason, SseMessage } from '../api/sse';

export type CitationPin = components['schemas']['CitationPin'];

export interface AskVersionContext {
  mode: string;
  currentVersionIds: string[];
  citedVersionIds: string[];
  changeNote: string | null;
}

export type AskStreamStatus = 'idle' | 'streaming' | 'completed' | 'revoked' | 'error';

export interface AskStreamState {
  readonly status: AskStreamStatus;
  readonly streamSessionId?: string;
  /** The server's own `AnswerMode` wire string (`offline_extractive`/`fallback_extractive`/...) — displayed, never re-derived client-side. */
  readonly answerMode?: string;
  readonly answer: string;
  readonly citations: CitationPin[];
  readonly warnings: string[];
  readonly versionContext?: AskVersionContext;
  /** Set once `status` becomes `'error'` (or `'revoked'`, which also carries a reason for the accessible notice) — the `stream.closed` reason code, or a transport-level code (`'network'`, `'session-lost'`, `'protocol_violation'`). */
  readonly errorReason?: string;
  /** Non-fatal transport notices (`gap`/`parse-error`) surfaced as accessible status text; never blocks the answer from continuing to render. */
  readonly notices: string[];
  readonly lastEventSequence?: number;
}

export function initialAskStreamState(): AskStreamState {
  return { status: 'idle', answer: '', citations: [], warnings: [], notices: [] };
}

function parseSequence(id: string): number | undefined {
  const n = Number(id);
  return Number.isFinite(n) ? n : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function applyEnvelope(state: AskStreamState, event: string, data: unknown): AskStreamState {
  switch (event) {
    case 'ask.started': {
      const streamSessionId =
        isRecord(data) && typeof data.streamSessionId === 'string'
          ? data.streamSessionId
          : undefined;
      const mode = isRecord(data) && typeof data.mode === 'string' ? data.mode : undefined;
      return { ...state, status: 'streaming', streamSessionId, answerMode: mode };
    }
    case 'ask.token': {
      const text = isRecord(data) && typeof data.text === 'string' ? data.text : '';
      return { ...state, status: 'streaming', answer: state.answer + text };
    }
    case 'ask.warning': {
      const message = isRecord(data) && typeof data.message === 'string' ? data.message : undefined;
      return message ? { ...state, warnings: [...state.warnings, message] } : state;
    }
    case 'ask.citations': {
      const citations =
        isRecord(data) && Array.isArray(data.citations) ? (data.citations as CitationPin[]) : [];
      return { ...state, citations };
    }
    case 'ask.version_context': {
      if (!isRecord(data)) return state;
      const versionContext: AskVersionContext = {
        mode: typeof data.mode === 'string' ? data.mode : 'current',
        currentVersionIds: Array.isArray(data.currentVersionIds)
          ? (data.currentVersionIds as string[])
          : [],
        citedVersionIds: Array.isArray(data.citedVersionIds)
          ? (data.citedVersionIds as string[])
          : [],
        changeNote: typeof data.changeNote === 'string' ? data.changeNote : null,
      };
      return { ...state, versionContext };
    }
    case 'ask.completed': {
      const mode = isRecord(data) && typeof data.mode === 'string' ? data.mode : state.answerMode;
      return { ...state, answerMode: mode };
    }
    case 'stream.closed': {
      const reason =
        isRecord(data) && typeof data.reason === 'string' ? data.reason : 'stream_error';
      return closeWithReason(state, reason);
    }
    default:
      return state; // unknown event name — forward-compatible no-op, not a crash
  }
}

function closeWithReason(state: AskStreamState, reason: string): AskStreamState {
  if (reason === 'completed') {
    return { ...state, status: 'completed', errorReason: undefined };
  }
  if (reason === 'citation_revoked') {
    return { ...state, status: 'revoked', errorReason: reason };
  }
  return { ...state, status: 'error', errorReason: reason };
}

function applyClose(state: AskStreamState, reason: SseCloseReason): AskStreamState {
  if (reason.type === 'server') {
    return closeWithReason(state, reason.code);
  }
  if (reason.type === 'network-error') {
    return state.status === 'completed' || state.status === 'revoked'
      ? state
      : { ...state, status: 'error', errorReason: 'network' };
  }
  // session-lost: the caller's org/session changed under it (logout, org switch, revoke) —
  // always terminal regardless of whatever status was current.
  return { ...state, status: 'error', errorReason: 'session-lost' };
}

/** Applies one `SseMessage` (from `api/sse.ts`'s `SseConnection`) to `state`. Pure — safe to unit-test directly, and to fold over a canned message array to reconstruct any point in a stream. */
export function reduceAskStreamMessage(state: AskStreamState, message: SseMessage): AskStreamState {
  switch (message.kind) {
    case 'event': {
      const seq = parseSequence(message.id);
      if (
        seq !== undefined &&
        state.lastEventSequence !== undefined &&
        seq <= state.lastEventSequence
      ) {
        return state; // stale/duplicate id — see module doc's "belt" note
      }
      const applied = applyEnvelope(state, message.envelope.event, message.envelope.data);
      return { ...applied, lastEventSequence: seq ?? state.lastEventSequence };
    }
    case 'control':
      return applyEnvelope(state, message.envelope.event, message.envelope.data);
    case 'heartbeat':
      return state;
    case 'gap':
      return {
        ...state,
        notices: [...state.notices, 'Đã bỏ lỡ một số sự kiện cập nhật; nội dung tiếp tục đồng bộ.'],
      };
    case 'protocol-violation':
      return {
        ...state,
        status: 'error',
        errorReason: 'protocol_violation',
        notices: [...state.notices, 'Luồng dữ liệu không hợp lệ; đã dừng nhận câu trả lời.'],
      };
    case 'parse-error':
      return {
        ...state,
        notices: [...state.notices, 'Không đọc được một phần dữ liệu trả lời từ máy chủ.'],
      };
    case 'closed':
      return applyClose(state, message.reason);
  }
}

/** Vietnamese, accessible copy for a terminal `errorReason`. Every reason this mock/server can actually emit (`ask_stream.rs`'s `config_reason` + `api/sse.ts`'s transport-level codes) gets its own line — an unrecognized one still gets an honest generic message instead of silently showing nothing. */
export function describeAskStreamError(reason: string | undefined): string {
  switch (reason) {
    case 'cancelled':
      return 'Yêu cầu đã được hủy.';
    case 'session_revoked':
      return 'Phiên đăng nhập đã bị thu hồi.';
    case 'principal_denied':
      return 'Bạn không còn quyền truy cập nội dung này.';
    case 'session_expired':
      return 'Phiên hỏi đáp đã hết hạn.';
    case 'ops_fence_active':
      return 'Hệ thống đang bảo trì; vui lòng thử lại sau.';
    case 'protocol_violation':
      return 'Dữ liệu trả lời không hợp lệ; vui lòng thử lại.';
    case 'network':
      return 'Mất kết nối mạng khi đang nhận câu trả lời.';
    case 'session-lost':
      return 'Phiên làm việc đã thay đổi (đăng xuất hoặc chuyển tổ chức); đã dừng nhận câu trả lời.';
    case 'stream_error':
    case 'send_timeout':
    case 'live_tail_timeout':
      return 'Máy chủ tạm thời gặp sự cố khi trả lời; vui lòng thử lại.';
    case undefined:
      return 'Đã dừng nhận câu trả lời.';
    default:
      return `Đã dừng nhận câu trả lời (${reason}).`;
  }
}
