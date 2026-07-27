import { describe, expect, it } from 'vitest';
import type { TokenProvider } from './session';
import { classifySseCloseCode, SseConnection, SseFrameParser, type SseMessage } from './sse';

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

function envelopeFrame(id: string, event: string, data: unknown, requestId = 'req-1'): string {
  const envelope = { version: 1, sequence: Number(id), event, requestId, data };
  return `id: ${id}\nevent: ${event}\ndata: ${JSON.stringify(envelope)}\n\n`;
}

function controlFrame(reason: string, event = 'stream.closed', requestId = 'req-1'): string {
  const payload = { version: 1, event, requestId, data: { reason }, control: true };
  return `event: ${event}\ndata: ${JSON.stringify(payload)}\n\n`;
}

/** A simple queued SSE body: emits each string chunk on successive pulls, then closes. */
function streamOf(
  chunks: string[],
  onCancel?: (reason: unknown) => void,
): ReadableStream<Uint8Array> {
  let index = 0;
  return new ReadableStream<Uint8Array>({
    pull(controller) {
      if (index >= chunks.length) {
        controller.close();
        return;
      }
      controller.enqueue(new TextEncoder().encode(chunks[index]));
      index += 1;
    },
    cancel(reason) {
      onCancel?.(reason);
    },
  });
}

/** A body that never ends on its own and can be pushed into from the test. */
function openStream(onCancel?: (reason: unknown) => void) {
  let controllerRef!: ReadableStreamDefaultController<Uint8Array>;
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      controllerRef = controller;
    },
    cancel(reason) {
      onCancel?.(reason);
    },
  });
  return {
    stream,
    push: (text: string) => controllerRef.enqueue(new TextEncoder().encode(text)),
    close: () => controllerRef.close(),
  };
}

/** A body that counts how many times the network layer was asked to produce a chunk (`pull`), one frame per pull, forever. */
function countingFrameStream(makeFrame: (seq: number) => string) {
  let produced = 0;
  let pulls = 0;
  const stream = new ReadableStream<Uint8Array>({
    pull(controller) {
      pulls += 1;
      produced += 1;
      controller.enqueue(new TextEncoder().encode(makeFrame(produced)));
    },
  });
  return { stream, pulls: () => pulls, produced: () => produced };
}

interface FetchCall {
  url: string;
  headers: Headers;
  method?: string;
}

function fetchQueue(...responses: Array<Response | (() => Response)>) {
  const calls: FetchCall[] = [];
  let index = 0;
  const fetchImpl = (async (input: string | URL | Request, init?: RequestInit) => {
    calls.push({
      url: String(input),
      headers: new Headers(init?.headers),
      method: init?.method,
    });
    const entry = responses[Math.min(index, responses.length - 1)];
    index += 1;
    return typeof entry === 'function' ? entry() : entry;
  }) as typeof fetch;
  return { fetchImpl, calls };
}

function fakeTokenProvider(initialToken = 'token-1') {
  const listeners = new Set<() => void>();
  let token = initialToken;
  let refreshCount = 0;
  const provider: TokenProvider = {
    async getAccessToken() {
      return token;
    },
    async refreshNow() {
      refreshCount += 1;
      token = `${initialToken}-refreshed-${refreshCount}`;
      return token;
    },
    onSessionLost(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
  return {
    provider,
    fireSessionLost: () => listeners.forEach((listener) => listener()),
    refreshCount: () => refreshCount,
    currentToken: () => token,
  };
}

async function collect(connection: SseConnection, limit = 50): Promise<SseMessage[]> {
  const out: SseMessage[] = [];
  for await (const message of connection) {
    out.push(message);
    if (out.length >= limit) break;
  }
  return out;
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

// ---------------------------------------------------------------------------
// SseFrameParser — pure framing, no network
// ---------------------------------------------------------------------------

describe('SseFrameParser', () => {
  it('parses a basic event delivered in one push', () => {
    const parser = new SseFrameParser();
    const frames = parser.push('event: ask.token\nid: 1\ndata: {"text":"hi"}\n\n');
    expect(frames).toEqual([
      { event: 'ask.token', id: '1', data: '{"text":"hi"}', comment: false },
    ]);
  });

  it('joins multi-line data fields with \\n, per the SSE spec', () => {
    const parser = new SseFrameParser();
    const frames = parser.push('data: line one\ndata: line two\n\n');
    expect(frames).toHaveLength(1);
    expect(frames[0].data).toBe('line one\nline two');
  });

  it('handles CRLF line endings', () => {
    const parser = new SseFrameParser();
    const frames = parser.push('event: job.progress\r\nid: 3\r\ndata: {"a":1}\r\n\r\n');
    expect(frames).toEqual([{ event: 'job.progress', id: '3', data: '{"a":1}', comment: false }]);
  });

  it('handles a bare CR as a line ending', () => {
    const parser = new SseFrameParser();
    const frames = parser.push('event: e\rid: 1\rdata: d\r\r');
    expect(frames).toEqual([{ event: 'e', id: '1', data: 'd', comment: false }]);
  });

  it('treats a comment-only block as a heartbeat, not a data frame', () => {
    const parser = new SseFrameParser();
    const frames = parser.push(':heartbeat\n\n');
    expect(frames).toEqual([{ event: undefined, id: undefined, data: '', comment: true }]);
  });

  it('does not confuse a control frame (no id) with a durable one', () => {
    const parser = new SseFrameParser();
    const frames = parser.push('event: stream.closed\ndata: {"reason":"stream_error"}\n\n');
    expect(frames).toEqual([
      { event: 'stream.closed', id: undefined, data: '{"reason":"stream_error"}', comment: false },
    ]);
  });

  it('parses multiple frames delivered in a single push', () => {
    const parser = new SseFrameParser();
    const frames = parser.push(
      'id: 1\nevent: a\ndata: 1\n\n' + 'id: 2\nevent: b\ndata: 2\n\n' + ':ping\n\n',
    );
    expect(frames).toHaveLength(3);
    expect(frames.map((f) => f.id)).toEqual(['1', '2', undefined]);
    expect(frames[2].comment).toBe(true);
  });

  it('survives a chunk boundary splitting a frame mid-way (across two pushes)', () => {
    const parser = new SseFrameParser();
    const whole = 'id: 42\nevent: ask.token\ndata: {"text":"phần"}\n\n';
    // Split in the middle of the data line, mid multi-byte-looking content.
    const cut = whole.indexOf('"text"') + 3;
    const first = parser.push(whole.slice(0, cut));
    expect(first).toEqual([]); // nothing dispatched yet — frame incomplete
    const second = parser.push(whole.slice(cut));
    expect(second).toEqual([
      { event: 'ask.token', id: '42', data: '{"text":"phần"}', comment: false },
    ]);
  });

  it('survives a chunk boundary splitting a field name itself (e.g. "da" | "ta: x")', () => {
    const parser = new SseFrameParser();
    const first = parser.push('id: 1\nev');
    expect(first).toEqual([]);
    const second = parser.push('ent: a\nda');
    expect(second).toEqual([]);
    const third = parser.push('ta: hello\n\n');
    expect(third).toEqual([{ event: 'a', id: '1', data: 'hello', comment: false }]);
  });

  it('survives a chunk boundary splitting the CRLF terminator itself', () => {
    const parser = new SseFrameParser();
    const first = parser.push('id: 1\r\nevent: a\r\ndata: x\r');
    expect(first).toEqual([]); // trailing bare \r is ambiguous — held back as carry
    const second = parser.push('\n\r\n');
    expect(second).toEqual([{ event: 'a', id: '1', data: 'x', comment: false }]);
  });

  it('flushes a final frame that arrives without a trailing blank line', () => {
    const parser = new SseFrameParser();
    expect(parser.push('id: 9\nevent: a\ndata: last')).toEqual([]);
    expect(parser.flush()).toEqual([{ event: 'a', id: '9', data: 'last', comment: false }]);
  });

  it('ignores stray blank lines between frames', () => {
    const parser = new SseFrameParser();
    const frames = parser.push('\n\nid: 1\nevent: a\ndata: x\n\n\n');
    expect(frames).toEqual([{ event: 'a', id: '1', data: 'x', comment: false }]);
  });
});

// ---------------------------------------------------------------------------
// classifySseCloseCode
// ---------------------------------------------------------------------------

describe('classifySseCloseCode', () => {
  it('classifies every close reason emitted by the server', () => {
    // See services/stream_auth.rs:37-49 and services/qa/ask_stream.rs for the
    // exhaustive set of reasons the server actually sends.
    expect(classifySseCloseCode('token_expired')).toBe('refresh');
    expect(classifySseCloseCode('completed')).toBe('complete');
    expect(classifySseCloseCode('snapshot_complete')).toBe('complete');
    expect(classifySseCloseCode('stream_error')).toBe('retry');
    expect(classifySseCloseCode('send_timeout')).toBe('retry');
    expect(classifySseCloseCode('live_tail_timeout')).toBe('retry');
    expect(classifySseCloseCode('session_revoked')).toBe('terminal');
    expect(classifySseCloseCode('principal_denied')).toBe('terminal');
    expect(classifySseCloseCode('auth_revoked')).toBe('terminal');
    expect(classifySseCloseCode('citation_revoked')).toBe('terminal');
    expect(classifySseCloseCode('cancelled')).toBe('terminal');
    expect(classifySseCloseCode('session_expired')).toBe('terminal');
  });

  it('fails closed (terminal) for any reason it does not recognize', () => {
    expect(classifySseCloseCode('some_future_reason_nobody_told_us_about')).toBe('terminal');
  });
});

// ---------------------------------------------------------------------------
// SseConnection — property tests
// ---------------------------------------------------------------------------

describe('SseConnection', () => {
  it('delivers events in sequence order and resumes from the last acked id', async () => {
    const first = streamOf([
      envelopeFrame('1', 'ask.token', { text: 'A' }),
      envelopeFrame('2', 'ask.token', { text: 'B' }),
      controlFrame('stream_error'),
    ]);
    const second = streamOf([
      envelopeFrame('3', 'ask.completed', { mode: 'x' }),
      envelopeFrame('4', 'stream.closed', { reason: 'completed', streamSessionId: 's1' }),
    ]);
    const { fetchImpl, calls } = fetchQueue(
      new Response(first, { status: 200 }),
      new Response(second, { status: 200 }),
    );
    const { provider } = fakeTokenProvider();
    const connection = new SseConnection(() => ({ url: '/api/v1/ask/stream', method: 'POST' }), {
      tokenProvider: provider,
      fetchImpl,
      backoff: () => 0,
    });

    const messages = await collect(connection, 6);
    expect(messages.map((m) => m.kind)).toEqual([
      'event',
      'event',
      'control',
      'event',
      'event',
      'closed',
    ]);
    const ids = messages
      .filter((m): m is Extract<SseMessage, { kind: 'event' }> => m.kind === 'event')
      .map((m) => m.id);
    expect(ids).toEqual(['1', '2', '3', '4']);
    expect(ids.every((id, index) => index === 0 || Number(id) > Number(ids[index - 1]))).toBe(true);

    expect(calls).toHaveLength(2);
    expect(calls[0].headers.has('last-event-id')).toBe(false); // fresh connect: no resume params
    expect(calls[1].headers.get('last-event-id')).toBe('2'); // resumes after the last acked event
    expect(calls[1].url).toContain('lastEventId=2');
  });

  it('mutation check: without the reconnect-on-retry path, this would stop at the control frame instead of resuming', async () => {
    // Same fixture as above, asserting the specific behavior that a broken
    // "retry classification always terminal" mutant would fail: after the
    // stream_error control frame, iteration continues and a fresh event (id 3)
    // is delivered rather than the stream ending there.
    const first = streamOf([
      envelopeFrame('1', 'ask.token', { text: 'A' }),
      controlFrame('stream_error'),
    ]);
    const second = streamOf([envelopeFrame('2', 'ask.completed', {})]);
    const { fetchImpl } = fetchQueue(
      new Response(first, { status: 200 }),
      new Response(second, { status: 200 }),
    );
    const { provider } = fakeTokenProvider();
    const connection = new SseConnection(() => ({ url: '/api/v1/ask/stream' }), {
      tokenProvider: provider,
      fetchImpl,
      backoff: () => 0,
    });
    const messages = await collect(connection, 4);
    expect(messages.some((m) => m.kind === 'event' && m.id === '2')).toBe(true);
  });

  it('a 401 with code "unauthorized" at connect triggers exactly one refresh, then succeeds', async () => {
    const unauthorized = new Response(
      JSON.stringify({ code: 'unauthorized', message: 'x', requestId: 'r' }),
      {
        status: 401,
      },
    );
    const ok = streamOf([envelopeFrame('1', 'ask.started', {})]);
    const { fetchImpl, calls } = fetchQueue(unauthorized, new Response(ok, { status: 200 }));
    const { provider, refreshCount, currentToken } = fakeTokenProvider();
    const connection = new SseConnection(() => ({ url: '/api/v1/ask/stream' }), {
      tokenProvider: provider,
      fetchImpl,
      backoff: () => 0,
    });
    const messages = await collect(connection, 2);
    expect(messages[0]).toMatchObject({ kind: 'event', id: '1' });
    expect(refreshCount()).toBe(1);
    expect(calls[1].headers.get('authorization')).toBe(`Bearer ${currentToken()}`);
  });

  it('a token_expired control frame mid-stream triggers exactly one refresh and one resume — not a storm', async () => {
    const first = streamOf([
      envelopeFrame('1', 'ask.token', { text: 'A' }),
      controlFrame('token_expired'),
    ]);
    const second = streamOf([
      envelopeFrame('2', 'ask.completed', {}),
      envelopeFrame('3', 'stream.closed', { reason: 'completed', streamSessionId: 's1' }),
    ]);
    const { fetchImpl, calls } = fetchQueue(
      new Response(first, { status: 200 }),
      new Response(second, { status: 200 }),
    );
    const { provider, refreshCount } = fakeTokenProvider();
    const connection = new SseConnection(() => ({ url: '/api/v1/ask/stream' }), {
      tokenProvider: provider,
      fetchImpl,
      backoff: () => 0,
    });
    const messages = await collect(connection, 6);
    expect(refreshCount()).toBe(1);
    expect(calls).toHaveLength(2);
    expect(calls[1].headers.get('last-event-id')).toBe('1');
    expect(messages.map((m) => m.kind)).toEqual(['event', 'control', 'event', 'event', 'closed']);
  });

  it('does not storm-refresh: a second consecutive auth failure with no progress ends the connection', async () => {
    const firstControl = streamOf([controlFrame('token_expired')]);
    const secondUnauthorized = new Response(
      JSON.stringify({ code: 'token_expired', message: 'x', requestId: 'r' }),
      { status: 401 },
    );
    const { fetchImpl, calls } = fetchQueue(
      new Response(firstControl, { status: 200 }),
      secondUnauthorized,
    );
    const { provider, refreshCount } = fakeTokenProvider();
    const connection = new SseConnection(() => ({ url: '/api/v1/ask/stream' }), {
      tokenProvider: provider,
      fetchImpl,
      backoff: () => 0,
    });
    const messages = await collect(connection, 6);
    expect(refreshCount()).toBe(1); // never a second refresh attempt
    expect(calls).toHaveLength(2); // one retry, then stop — not a storm
    expect(messages[messages.length - 1]).toEqual({
      kind: 'closed',
      reason: { type: 'server', code: 'token_expired' },
    });
  });

  it('mutation check: without the storm guard, the above would keep refreshing/reconnecting forever', async () => {
    // Sanity: prove the fixture really would loop without the guard by
    // showing a *third* queued response is never touched.
    const control = () => streamOf([controlFrame('token_expired')]);
    const { fetchImpl, calls } = fetchQueue(
      new Response(control(), { status: 200 }),
      new Response(JSON.stringify({ code: 'token_expired', message: 'x', requestId: 'r' }), {
        status: 401,
      }),
      new Response(control(), { status: 200 }), // would be hit by a storming client; must not be
    );
    const { provider, refreshCount } = fakeTokenProvider();
    const connection = new SseConnection(() => ({ url: '/api/v1/ask/stream' }), {
      tokenProvider: provider,
      fetchImpl,
      backoff: () => 0,
    });
    await collect(connection, 6);
    expect(calls).toHaveLength(2);
    expect(refreshCount()).toBe(1);
  });

  for (const reason of [
    'session_revoked',
    'principal_denied',
    'citation_revoked',
    'auth_revoked',
    'cancelled',
  ]) {
    it(`delivers zero further content after a "${reason}" revoke, and does not reconnect`, async () => {
      const body = streamOf([envelopeFrame('1', 'ask.token', { text: 'A' }), controlFrame(reason)]);
      const { fetchImpl, calls } = fetchQueue(
        new Response(body, { status: 200 }),
        new Response(
          streamOf([envelopeFrame('99', 'ask.token', { text: 'should never be seen' })]),
          {
            status: 200,
          },
        ),
      );
      const { provider } = fakeTokenProvider();
      const connection = new SseConnection(() => ({ url: '/api/v1/ask/stream' }), {
        tokenProvider: provider,
        fetchImpl,
        backoff: () => 0,
      });
      const messages = await collect(connection, 6);
      expect(messages.map((m) => m.kind)).toEqual(['event', 'control', 'closed']);
      expect(messages[messages.length - 1]).toEqual({
        kind: 'closed',
        reason: { type: 'server', code: reason },
      });
      expect(calls).toHaveLength(1); // never reconnected
    });
  }

  it('a durable stream.closed (with an id) ends the connection without reconnecting', async () => {
    const body = streamOf([
      envelopeFrame('1', 'ask.token', { text: 'A' }),
      envelopeFrame('2', 'stream.closed', { reason: 'completed', streamSessionId: 's1' }),
    ]);
    const { fetchImpl, calls } = fetchQueue(new Response(body, { status: 200 }));
    const { provider } = fakeTokenProvider();
    const connection = new SseConnection(() => ({ url: '/api/v1/ask/stream' }), {
      tokenProvider: provider,
      fetchImpl,
      backoff: () => 0,
    });
    const messages = await collect(connection, 5);
    // The terminal frame carried an id, so it is an 'event' (cursor-advancing), not a synthetic 'control'.
    expect(messages[1]).toMatchObject({ kind: 'event', id: '2' });
    expect(messages[messages.length - 1]).toEqual({
      kind: 'closed',
      reason: { type: 'server', code: 'completed' },
    });
    expect(calls).toHaveLength(1);
  });

  it('surfaces a non-contiguous but increasing sequence as a gap rather than silently accepting it', async () => {
    const body = streamOf([
      envelopeFrame('1', 'ask.token', { text: 'A' }),
      envelopeFrame('2', 'ask.token', { text: 'B' }),
      envelopeFrame('7', 'ask.token', { text: 'skipped ahead' }), // e.g. purge/loss between 2 and 7
    ]);
    const { fetchImpl } = fetchQueue(new Response(body, { status: 200 }));
    const { provider } = fakeTokenProvider();
    const connection = new SseConnection(() => ({ url: '/api/v1/ask/stream' }), {
      tokenProvider: provider,
      fetchImpl,
      backoff: () => 0,
    });
    const messages = await collect(connection, 5);
    const gapIndex = messages.findIndex((m) => m.kind === 'gap');
    expect(gapIndex).toBeGreaterThan(-1);
    expect(messages[gapIndex]).toEqual({ kind: 'gap', expected: '3', received: '7' });
    // The event itself still gets delivered — a gap is surfaced, not dropped.
    expect(messages[gapIndex + 1]).toMatchObject({ kind: 'event', id: '7' });
  });

  it('treats a non-increasing (duplicate/out-of-order) id as a protocol violation and stops — never delivers it twice', async () => {
    const body = streamOf([
      envelopeFrame('5', 'ask.token', { text: 'A' }),
      envelopeFrame('5', 'ask.token', { text: 'A again — must not happen' }),
    ]);
    const { fetchImpl, calls } = fetchQueue(
      new Response(body, { status: 200 }),
      new Response(streamOf([envelopeFrame('6', 'ask.token', { text: 'never reached' })]), {
        status: 200,
      }),
    );
    const { provider } = fakeTokenProvider();
    const connection = new SseConnection(() => ({ url: '/api/v1/ask/stream' }), {
      tokenProvider: provider,
      fetchImpl,
      backoff: () => 0,
    });
    const messages = await collect(connection, 5);
    // Cursor starts at '0', so id "5" arriving first is itself non-contiguous
    // (a legitimate, benign gap — see advanceCursor's doc) and is surfaced
    // before the duplicate-id protocol violation stops the connection.
    expect(messages.map((m) => m.kind)).toEqual(['gap', 'event', 'protocol-violation', 'closed']);
    expect(calls).toHaveLength(1); // does not reconnect and re-accept
  });

  it('an external session-lost signal aborts the fetch/reader immediately, delivering nothing further', async () => {
    let cancelled = false;
    const { stream, push } = openStream(() => {
      cancelled = true;
    });
    const { fetchImpl } = fetchQueue(new Response(stream, { status: 200 }));
    const { provider, fireSessionLost } = fakeTokenProvider();
    const connection = new SseConnection(() => ({ url: '/api/v1/jobs/j1/events' }), {
      tokenProvider: provider,
      fetchImpl,
      backoff: () => 0,
    });

    const it_ = connection[Symbol.asyncIterator]();
    push(envelopeFrame('1', 'job.progress', {}));
    const firstResult = await it_.next();
    expect(firstResult.value).toMatchObject({ kind: 'event', id: '1' });

    fireSessionLost();
    // The underlying source is cancelled synchronously by the abort — trying
    // to push more into it now throws, which is an even stronger guarantee
    // than "not delivered": nothing more can even be produced.
    expect(() =>
      push(envelopeFrame('2', 'job.progress', { text: 'must never be delivered' })),
    ).toThrow();

    const second = await it_.next();
    expect(second.value).toEqual({ kind: 'closed', reason: { type: 'session-lost' } });
    const third = await it_.next();
    expect(third.done).toBe(true);
    expect(cancelled).toBe(true);
  });

  it('abort() stops the fetch and releases the reader without yielding further messages', async () => {
    let cancelled = false;
    const { stream, push } = openStream(() => {
      cancelled = true;
    });
    const { fetchImpl } = fetchQueue(new Response(stream, { status: 200 }));
    const { provider } = fakeTokenProvider();
    const connection = new SseConnection(() => ({ url: '/api/v1/jobs/j1/events' }), {
      tokenProvider: provider,
      fetchImpl,
      backoff: () => 0,
    });

    const it_ = connection[Symbol.asyncIterator]();
    push(envelopeFrame('1', 'job.progress', {}));
    await it_.next();

    connection.abort();
    const result = await it_.next();
    expect(result.done).toBe(true);
    expect(cancelled).toBe(true);
    expect(connection.aborted).toBe(true);
  });

  it('applies bounded backpressure: the network is not pulled far ahead of what the consumer has read', async () => {
    const { stream, pulls } = countingFrameStream((seq) =>
      envelopeFrame(String(seq), 'job.progress', { seq }),
    );
    const { fetchImpl } = fetchQueue(new Response(stream, { status: 200 }));
    const { provider } = fakeTokenProvider();
    const connection = new SseConnection(() => ({ url: '/api/v1/jobs/j1/events' }), {
      tokenProvider: provider,
      fetchImpl,
      backoff: () => 0,
    });
    const it_ = connection[Symbol.asyncIterator]();

    const first = await it_.next();
    expect(first.value).toMatchObject({ kind: 'event', id: '1' });
    const pullsAfterOne = pulls();
    // A slow consumer: do not call next() again for a while. If reads ran
    // ahead of consumption, `pulls()` would keep climbing on its own.
    await sleep(50);
    expect(pulls()).toBe(pullsAfterOne);
    expect(pulls()).toBeLessThanOrEqual(2); // small constant pipeline depth (stream highWaterMark), not unbounded

    for (let i = 0; i < 5; i += 1) {
      await it_.next();
    }
    // Pulls track consumption 1:1 (plus the same small constant), never runaway.
    expect(pulls()).toBeLessThanOrEqual(6 + 2);
  });

  it('retries a transient network failure with backoff, then succeeds', async () => {
    let attempts = 0;
    const fetchImpl = (async () => {
      attempts += 1;
      if (attempts < 3) {
        throw new TypeError('network down');
      }
      return new Response(
        streamOf([
          envelopeFrame('1', 'ask.token', { text: 'ok' }),
          envelopeFrame('2', 'stream.closed', { reason: 'completed' }),
        ]),
        { status: 200 },
      );
    }) as typeof fetch;
    const { provider } = fakeTokenProvider();
    const connection = new SseConnection(() => ({ url: '/api/v1/ask/stream' }), {
      tokenProvider: provider,
      fetchImpl,
      backoff: () => 0,
      maxTransientAttempts: 5,
    });
    const messages = await collect(connection, 3);
    expect(attempts).toBe(3); // exactly the transient failures + 1 success — no extra reconnect
    expect(messages[0]).toMatchObject({ kind: 'event', id: '1' });
  });

  it('gives up after maxTransientAttempts and reports a network-error close', async () => {
    const fetchImpl = (async () => {
      throw new TypeError('network down');
    }) as typeof fetch;
    const { provider } = fakeTokenProvider();
    const connection = new SseConnection(() => ({ url: '/api/v1/ask/stream' }), {
      tokenProvider: provider,
      fetchImpl,
      backoff: () => 0,
      maxTransientAttempts: 2,
    });
    const messages = await collect(connection, 5);
    expect(messages).toEqual([{ kind: 'closed', reason: { type: 'network-error' } }]);
  });

  it('respects Retry-After on a 429 before retrying', async () => {
    const rateLimited = new Response(
      JSON.stringify({ code: 'rate_limited', message: 'x', requestId: 'r' }),
      {
        status: 429,
        headers: { 'Retry-After': '0' },
      },
    );
    const ok = streamOf([
      envelopeFrame('1', 'ask.token', { text: 'ok' }),
      envelopeFrame('2', 'stream.closed', { reason: 'completed' }),
    ]);
    const { fetchImpl, calls } = fetchQueue(rateLimited, new Response(ok, { status: 200 }));
    const { provider } = fakeTokenProvider();
    const connection = new SseConnection(() => ({ url: '/api/v1/ask/stream' }), {
      tokenProvider: provider,
      fetchImpl,
      backoff: () => 0,
    });
    const messages = await collect(connection, 3);
    expect(calls).toHaveLength(2);
    expect(messages[0]).toMatchObject({ kind: 'event', id: '1' });
  });

  it('treats a malformed JSON data field as a parse-error instead of throwing', async () => {
    const body = streamOf(['id: 1\nevent: ask.token\ndata: {not json\n\n']);
    const { fetchImpl } = fetchQueue(new Response(body, { status: 200 }));
    const { provider } = fakeTokenProvider();
    const connection = new SseConnection(() => ({ url: '/api/v1/ask/stream' }), {
      tokenProvider: provider,
      fetchImpl,
      backoff: () => 0,
    });
    const messages = await collect(connection, 3);
    expect(messages[0]).toMatchObject({ kind: 'parse-error' });
  });

  it('passes the bearer token and Accept header on every request', async () => {
    const body = streamOf([envelopeFrame('1', 'ask.token', { text: 'ok' })]);
    const { fetchImpl, calls } = fetchQueue(new Response(body, { status: 200 }));
    const { provider, currentToken } = fakeTokenProvider('secret-token');
    const connection = new SseConnection(
      () => ({ url: '/api/v1/ask/stream', method: 'POST', body: '{}' }),
      {
        tokenProvider: provider,
        fetchImpl,
      },
    );
    await collect(connection, 1);
    expect(calls[0].headers.get('authorization')).toBe(`Bearer ${currentToken()}`);
    expect(calls[0].headers.get('accept')).toBe('text/event-stream');
    expect(calls[0].method).toBe('POST');
  });
});
