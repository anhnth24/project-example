// One turn (user question + streamed/settled assistant answer) in
// `ChatPanel`'s chat log. Owns its own `useAskStream` instance — deliberately
// one hook instance per turn, never a single instance reused across turns —
// so a later turn's stream can never mutate an earlier turn's already-settled
// state, and an earlier turn's citations/warnings/notices stay exactly as
// they were once that turn finished. This file is pure UI plumbing around the
// untouched `state/askStream.ts` reducer + `useAskStream` hook (P2-10
// baseline) — nothing here re-implements or forks that logic; it only calls
// `ask()` once (on mount) and renders whatever `state` comes back.
import { useEffect, useRef, useState } from 'react';
import { apiClient, type ApiClient } from '../../api/client';
import type { components } from '../../api/generated/contract';
import { Notice } from '../ui';
import {
  describeAskStreamError,
  type AskStreamState,
  type AskStreamStatus,
} from '../../state/askStream';
import { AnswerText } from './AnswerText';
import { describeAnswerMode } from './answerMode';
import { CitationFootnotes } from './CitationFootnotes';
import { useAskStream } from './useAskStream';

type AskRequest = components['schemas']['AskRequest'];

/** What this bubble reports upward for composer gating — the hook's own `AskStreamStatus` plus a client-initiated `'cancelled'`, which `reset()` alone cannot distinguish from "never started" (both land on `'idle'`). `ChatPanel` treats `'cancelled'` as settled, same as `'completed'`/`'revoked'`/`'error'`. */
export type ChatTurnStatus = AskStreamStatus | 'cancelled';

type CitationPin = components['schemas']['CitationPin'];

/** A snapshot of this turn's displayed content at the moment `onStatusChange` fires — this is what `ChatPanel` needs to persist a settled turn (part A, "Ghi lịch sử") without lifting the whole growing answer on every token. */
export interface ChatTurnSnapshot {
  readonly answer: string;
  readonly answerMode?: string;
  readonly citations: CitationPin[];
  readonly warnings: string[];
}

export function ChatTurnBubble({
  turnId,
  request,
  question,
  client = apiClient,
  collectionNameById,
  onStatusChange,
  onReady,
}: {
  /** This turn's stable id (`ChatPanel`'s own `turn.id`) — used only to namespace this bubble's footnote anchors so two turns on the same page never collide. */
  turnId: string;
  request: AskRequest;
  /** The question text as typed for this turn — captured at submit time so a later composer edit can never retroactively change what an earlier bubble shows it asked. */
  question: string;
  client?: ApiClient;
  /** `collectionId -> tên bộ sưu tập`, for `CitationFootnotes`'s fallback label (see that component's module doc for the `documentTitle` gap this works around). */
  collectionNameById: ReadonlyMap<string, string>;
  /** Reports this turn's live status (plus a content snapshot, for persistence once settled) up to `ChatPanel` — fired once per status transition, never on every growing token. */
  onStatusChange?: (status: ChatTurnStatus, snapshot: ChatTurnSnapshot) => void;
  /** Hands this turn's own abort function up once, right after mount — `ChatPanel`'s "Hủy" button calls whichever turn is currently active through this. */
  onReady?: (abort: () => void) => void;
}) {
  const { state, ask, reset } = useAskStream(client.tokenProvider);

  // Kick off exactly once for this turn's whole lifetime: `request` never
  // changes after a bubble is created (a new turn always gets a brand-new
  // `ChatTurnBubble`, keyed by turn id in `ChatPanel`), so this must not
  // re-fire on re-render — a `useRef` guard rather than a `[request]` dep
  // array, since re-running `ask()` would restart the same turn's stream.
  const startedRef = useRef(false);
  useEffect(() => {
    if (startedRef.current) return;
    startedRef.current = true;
    ask(request);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Client-initiated cancel. `reset()` (from `useAskStream`, untouched) is
  // what actually aborts the underlying stream — the same mechanism the
  // single-turn `AskPanel`'s "Hủy" used — but it also reinitializes local
  // state back to `'idle'`, indistinguishable from "never started". A chat
  // bubble that just goes blank after being cancelled reads as a bug, so the
  // partial answer/citations/warnings gathered so far are frozen here first
  // and rendered from that snapshot instead of from the now-idle live state.
  const [cancelled, setCancelled] = useState(false);
  // A `useState` snapshot, not a ref: this value is read during render (to
  // decide what to display), and ref values must never be read at render
  // time (`react-hooks/refs`) — only `useState`/props are safe there.
  const [frozen, setFrozen] = useState<AskStreamState | null>(null);

  function abort() {
    if (cancelled) return;
    setFrozen(state);
    setCancelled(true);
    reset();
  }

  useEffect(() => {
    onReady?.(abort);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const display = cancelled && frozen ? frozen : state;
  const isDone =
    display.status === 'completed' || display.status === 'revoked' || display.status === 'error';
  const modeInfo = isDone && !cancelled ? describeAnswerMode(display.answerMode) : null;

  const lastReportedRef = useRef<ChatTurnStatus | undefined>(undefined);
  useEffect(() => {
    const reported: ChatTurnStatus = cancelled ? 'cancelled' : state.status;
    if (lastReportedRef.current === reported) return;
    lastReportedRef.current = reported;
    onStatusChange?.(reported, {
      answer: display.answer,
      answerMode: display.answerMode,
      citations: display.citations,
      warnings: display.warnings,
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.status, cancelled]);

  return (
    <div className="chat-turn" style={{ display: 'grid', gap: 'var(--space-2)' }}>
      <p style={{ margin: 0 }}>
        <span className="tag tag-outline">Bạn</span>{' '}
        <span style={{ fontWeight: 600 }}>{question}</span>
      </p>

      <div aria-live="polite" role="status">
        {!cancelled && display.status === 'streaming' && !display.answer && (
          <p className="text-muted">Đang tạo câu trả lời…</p>
        )}
        {display.answer && (
          <AnswerText text={display.answer} citations={display.citations} scopeId={turnId} />
        )}
        {cancelled && (
          <Notice tone="info">
            Đã hủy câu trả lời này — nội dung phía trên (nếu có) có thể chưa đầy đủ.
          </Notice>
        )}
        {modeInfo?.tone === 'neutral' && (
          <p>
            <span className="tag tag-neutral">{modeInfo.label}</span>
          </p>
        )}
        {modeInfo?.tone === 'warning' && <Notice tone="warning">{modeInfo.label}</Notice>}
        {!cancelled && display.status === 'revoked' && (
          <Notice tone="warning">
            Trích dẫn đã bị thu hồi giữa chừng — câu trả lời phía trên có thể không đầy đủ.{' '}
            {describeAskStreamError(display.errorReason)}
          </Notice>
        )}
        {!cancelled && display.status === 'error' && (
          <Notice tone="error">{describeAskStreamError(display.errorReason)}</Notice>
        )}
      </div>

      {display.notices.map((notice, i) => (
        <p key={i} className="text-muted" role="status">
          {notice}
        </p>
      ))}

      {display.warnings.length > 0 && (
        <div style={{ display: 'grid', gap: 'var(--space-1)' }}>
          {display.warnings.map((warning, i) => (
            <Notice key={i} tone="warning">
              {warning}
            </Notice>
          ))}
        </div>
      )}

      {display.versionContext?.changeNote && (
        <p className="text-muted">{display.versionContext.changeNote}</p>
      )}

      <CitationFootnotes
        citations={display.citations}
        collectionNameById={collectionNameById}
        scopeId={turnId}
      />
    </div>
  );
}
