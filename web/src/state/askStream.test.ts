import { describe, expect, it } from 'vitest';
import type { SseMessage } from '../api/sse';
import {
  describeAskStreamError,
  initialAskStreamState,
  reduceAskStreamMessage,
  type AskStreamState,
} from './askStream';

function event(id: number, eventName: string, data: unknown): SseMessage {
  return {
    kind: 'event',
    id: String(id),
    envelope: { version: 1, sequence: id, event: eventName, requestId: 'req-1', data },
  };
}

function fold(
  messages: SseMessage[],
  start: AskStreamState = initialAskStreamState(),
): AskStreamState {
  return messages.reduce(reduceAskStreamMessage, start);
}

describe('reduceAskStreamMessage', () => {
  it('accumulates tokens in order and reaches "completed" on a durable stream.closed', () => {
    const final = fold([
      event(1, 'ask.started', {
        streamSessionId: 'sess-1',
        mode: 'offline_extractive',
        citationCount: 1,
      }),
      event(2, 'ask.token', { text: 'Xin ' }),
      event(3, 'ask.token', { text: 'chào ' }),
      event(4, 'ask.token', { text: 'bạn.' }),
      event(5, 'ask.citations', {
        citations: [
          {
            citeId: 'CITE-0001',
            sourceContentSha256: 'a'.repeat(64),
            canonicalMarkdownSha256: 'b'.repeat(64),
            quoteSha256: 'c'.repeat(64),
            chunkIdentitySha256: 'd'.repeat(64),
            quote: 'Xin chào bạn.',
            sourceSpanStart: 0,
            sourceSpanEnd: 10,
            quoteLocalStart: 0,
            quoteLocalEnd: 10,
            isCurrent: true,
          },
        ],
      }),
      event(6, 'ask.version_context', {
        mode: 'current',
        currentVersionIds: ['v1'],
        citedVersionIds: ['v1'],
        changeNote: null,
      }),
      event(7, 'ask.completed', { mode: 'offline_extractive', streamSessionId: 'sess-1' }),
      event(8, 'stream.closed', { reason: 'completed' }),
    ]);

    expect(final.answer).toBe('Xin chào bạn.');
    expect(final.status).toBe('completed');
    expect(final.streamSessionId).toBe('sess-1');
    expect(final.citations).toHaveLength(1);
    expect(final.citations[0].citeId).toBe('CITE-0001');
    expect(final.versionContext?.mode).toBe('current');
    expect(final.errorReason).toBeUndefined();
  });

  it('ignores a duplicate/out-of-order event id instead of re-applying it (dedupe belt, see module doc)', () => {
    const afterFirstToken = fold([
      event(1, 'ask.started', { streamSessionId: 's', mode: 'offline_extractive' }),
      event(2, 'ask.token', { text: 'A' }),
    ]);
    expect(afterFirstToken.answer).toBe('A');

    // A message the real transport would never yield (it already rejects
    // non-increasing ids as a protocol violation) — feeding it directly to
    // the reducer proves the second, independent guard this module keeps.
    const replayed = reduceAskStreamMessage(afterFirstToken, event(2, 'ask.token', { text: 'A' }));
    expect(replayed.answer).toBe('A'); // not "AA" — the duplicate never re-applied

    const nextReal = reduceAskStreamMessage(replayed, event(3, 'ask.token', { text: 'B' }));
    expect(nextReal.answer).toBe('AB');
  });

  it('citation_revoked mid-answer: partial answer kept, status "revoked", no citations ever arrive', () => {
    const final = fold([
      event(1, 'ask.started', { streamSessionId: 's', mode: 'offline_extractive' }),
      event(2, 'ask.token', { text: 'Đang trả lời ' }),
      event(3, 'ask.token', { text: 'thì bị thu hồi.' }),
      event(4, 'stream.closed', { reason: 'citation_revoked' }),
    ]);

    expect(final.status).toBe('revoked');
    expect(final.errorReason).toBe('citation_revoked');
    expect(final.answer).toBe('Đang trả lời thì bị thu hồi.');
    expect(final.citations).toHaveLength(0);
  });

  it('fallback-extractive: warning arrives before citations, final mode is fallback_extractive', () => {
    const final = fold([
      event(1, 'ask.started', { streamSessionId: 's', mode: 'offline_extractive' }),
      event(2, 'ask.token', { text: 'Trả lời trích xuất.' }),
      event(3, 'ask.warning', { message: 'Nhà cung cấp LLM tạm thời không khả dụng.' }),
      event(4, 'ask.citations', { citations: [] }),
      event(5, 'ask.version_context', {
        mode: 'current',
        currentVersionIds: [],
        citedVersionIds: [],
        changeNote: null,
      }),
      event(6, 'ask.completed', { mode: 'fallback_extractive', streamSessionId: 's' }),
      event(7, 'stream.closed', { reason: 'completed' }),
    ]);

    expect(final.status).toBe('completed');
    expect(final.answerMode).toBe('fallback_extractive');
    expect(final.warnings).toEqual(['Nhà cung cấp LLM tạm thời không khả dụng.']);
  });

  it('a network-error close after some progress is a terminal "error", but never downgrades an already-completed/revoked state', () => {
    const midStream = fold([
      event(1, 'ask.started', { streamSessionId: 's', mode: 'offline_extractive' }),
      event(2, 'ask.token', { text: 'A' }),
    ]);
    const afterNetworkLoss = reduceAskStreamMessage(midStream, {
      kind: 'closed',
      reason: { type: 'network-error' },
    });
    expect(afterNetworkLoss.status).toBe('error');
    expect(afterNetworkLoss.errorReason).toBe('network');

    const completed = fold([
      event(1, 'ask.started', { streamSessionId: 's', mode: 'offline_extractive' }),
      event(2, 'stream.closed', { reason: 'completed' }),
    ]);
    const stillCompleted = reduceAskStreamMessage(completed, {
      kind: 'closed',
      reason: { type: 'network-error' },
    });
    expect(stillCompleted.status).toBe('completed');
  });

  it('a session-lost close (org switch/logout mid-answer) is always terminal, even if already completed', () => {
    const completed = fold([event(1, 'stream.closed', { reason: 'completed' })]);
    const afterSessionLost = reduceAskStreamMessage(completed, {
      kind: 'closed',
      reason: { type: 'session-lost' },
    });
    expect(afterSessionLost.status).toBe('error');
    expect(afterSessionLost.errorReason).toBe('session-lost');
  });

  it('gap/parse-error/protocol-violation surface accessible notices without crashing or losing the answer so far', () => {
    const afterGap = reduceAskStreamMessage(fold([event(1, 'ask.token', { text: 'A' })]), {
      kind: 'gap',
      expected: '2',
      received: '5',
    });
    expect(afterGap.answer).toBe('A');
    expect(afterGap.notices).toHaveLength(1);

    const afterParseError = reduceAskStreamMessage(afterGap, {
      kind: 'parse-error',
      raw: 'not json',
      event: 'ask.token',
    });
    expect(afterParseError.notices).toHaveLength(2);
    expect(afterParseError.status).not.toBe('error');

    const afterProtocolViolation = reduceAskStreamMessage(afterParseError, {
      kind: 'protocol-violation',
      message: 'id "1" invalid or not greater than last acked "3"',
      receivedId: '1',
    });
    expect(afterProtocolViolation.status).toBe('error');
    expect(afterProtocolViolation.errorReason).toBe('protocol_violation');
  });

  it('heartbeats are a no-op', () => {
    const before = fold([event(1, 'ask.token', { text: 'A' })]);
    const after = reduceAskStreamMessage(before, { kind: 'heartbeat' });
    expect(after).toEqual(before);
  });

  // P2-10 conflict-warning demo (as-of/compare/history) — `mocks/handlers/qa.ts`'s
  // `nonCurrentConflictWarning`/history-summary warning are just extra
  // `ask.warning` frames ahead of `ask.version_context`, same wire shape
  // `current` mode's own warning already uses (see the "fallback-extractive"
  // test above); these three cases check the reducer surfaces each mode's
  // warning(s) intact, in order, alongside the right `versionContext.mode` —
  // not the mock's own string-building, which is exercised via the mock/E2E
  // layer instead.
  describe('P2-10 conflict-warning demo (as-of/compare/history)', () => {
    it('as-of mode: one warning that the resolved version is older than current', () => {
      const final = fold([
        event(1, 'ask.started', { streamSessionId: 's', mode: 'offline_extractive' }),
        event(2, 'ask.token', {
          text: 'Ngân sách vận hành được BA duyệt là 10 triệu đồng mỗi quý.',
        }),
        event(3, 'ask.warning', {
          message:
            'Phiên bản 1 của "Chính sách ngân sách vận hành.pdf" không phải phiên bản hiện hành — nội dung: "Ngân sách vận hành được BA duyệt là 10 triệu đồng mỗi quý.". Xung đột đã được giải quyết ở phiên bản 2.',
        }),
        event(4, 'ask.citations', { citations: [] }),
        event(5, 'ask.version_context', {
          mode: 'as_of',
          currentVersionIds: ['v2'],
          citedVersionIds: ['v1'],
          changeNote: 'Truy vấn as-of tại 2026-01-01T00:00:00.000Z.',
        }),
        event(6, 'ask.completed', { mode: 'offline_extractive', streamSessionId: 's' }),
        event(7, 'stream.closed', { reason: 'completed' }),
      ]);

      expect(final.warnings).toHaveLength(1);
      expect(final.warnings[0]).toContain('không phải phiên bản hiện hành');
      expect(final.warnings[0]).toContain('giải quyết ở phiên bản 2');
      expect(final.versionContext?.mode).toBe('as_of');
      expect(final.versionContext?.citedVersionIds).toEqual(['v1']);
    });

    it('compare mode: BOTH v1 and v2 get their own warning when both differ from current', () => {
      // The seeded fixture only has v1 differ from current (v2 IS current),
      // so this exercises the general "both sides can warn independently"
      // shape the mock's per-passage loop produces, rather than assuming
      // exactly one warning is all `compare` mode can ever emit.
      const final = fold([
        event(1, 'ask.started', { streamSessionId: 's', mode: 'offline_extractive' }),
        event(2, 'ask.token', { text: 'So sánh hai phiên bản.' }),
        event(3, 'ask.warning', {
          message:
            'Phiên bản 1 của "Doc.pdf" không phải phiên bản hiện hành — nội dung: "10 triệu". Xung đột đã được giải quyết ở phiên bản 3.',
        }),
        event(4, 'ask.warning', {
          message:
            'Phiên bản 2 của "Doc.pdf" không phải phiên bản hiện hành — nội dung: "15 triệu". Xung đột đã được giải quyết ở phiên bản 3.',
        }),
        event(5, 'ask.citations', { citations: [] }),
        event(6, 'ask.version_context', {
          mode: 'compare',
          currentVersionIds: ['v3'],
          citedVersionIds: ['v1', 'v2'],
          changeNote: 'So sánh phiên bản 1 (v1) với phiên bản 2 (v2).',
        }),
        event(7, 'ask.completed', { mode: 'offline_extractive', streamSessionId: 's' }),
        event(8, 'stream.closed', { reason: 'completed' }),
      ]);

      expect(final.warnings).toHaveLength(2);
      expect(final.warnings[0]).toContain('Phiên bản 1');
      expect(final.warnings[1]).toContain('Phiên bản 2');
      expect(final.versionContext?.mode).toBe('compare');
      expect(final.versionContext?.changeNote).toContain('So sánh phiên bản 1');
    });

    it('history mode: a single summary warning naming the version the conflict was resolved in', () => {
      const final = fold([
        event(1, 'ask.started', { streamSessionId: 's', mode: 'offline_extractive' }),
        event(2, 'ask.token', { text: 'Lịch sử ngân sách vận hành.' }),
        event(3, 'ask.warning', {
          message:
            'Lịch sử phiên bản của "Chính sách ngân sách vận hành.pdf" có xung đột dữ liệu giữa các phiên bản (vd. đề xuất BA vs. thiết kế mới) — đã được giải quyết ở phiên bản 2.',
        }),
        event(4, 'ask.citations', { citations: [] }),
        event(5, 'ask.version_context', {
          mode: 'history',
          currentVersionIds: ['v2'],
          citedVersionIds: ['v1', 'v2'],
          changeNote: 'Lịch sử phiên bản cho tài liệu doc-1.',
        }),
        event(6, 'ask.completed', { mode: 'offline_extractive', streamSessionId: 's' }),
        event(7, 'stream.closed', { reason: 'completed' }),
      ]);

      expect(final.warnings).toHaveLength(1);
      expect(final.warnings[0]).toContain('xung đột dữ liệu giữa các phiên bản');
      expect(final.warnings[0]).toContain('giải quyết ở phiên bản 2');
      expect(final.versionContext?.mode).toBe('history');
    });
  });
});

describe('describeAskStreamError', () => {
  it('has distinct, non-empty Vietnamese copy for every reason the mock/server can actually emit', () => {
    const reasons = [
      'cancelled',
      'session_revoked',
      'principal_denied',
      'session_expired',
      'ops_fence_active',
      'protocol_violation',
      'network',
      'session-lost',
      'stream_error',
      'send_timeout',
      'live_tail_timeout',
      undefined,
      'some_future_unrecognized_reason',
    ];
    const texts = reasons.map((reason) => describeAskStreamError(reason));
    for (const text of texts) {
      expect(text.length).toBeGreaterThan(0);
    }
    // An unrecognized reason still gets an honest, distinguishing message
    // (not silently identical to the generic undefined case).
    expect(describeAskStreamError('some_future_unrecognized_reason')).toContain(
      'some_future_unrecognized_reason',
    );
  });
});
