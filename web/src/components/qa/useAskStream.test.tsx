import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { TokenProvider } from '../../api/client';
import type { components } from '../../api/generated/contract';
import type { SseMessage } from '../../api/sse';
import type { ScopeSafeSseSource } from '../../hooks/useScopeSafeSse';
import { describeAskStreamError } from '../../state/askStream';
import { createScopeManager, type Scope, type ScopeManager } from '../../state/scope';
import { ScopeProvider } from '../../state/ScopeProvider';
import { createAskStreamSource } from './askStreamSource';
import { useAskStream } from './useAskStream';
import { presentWarnings } from './warningPresentation';

type AskRequest = components['schemas']['AskRequest'];

vi.mock('./askStreamSource', () => ({
  createAskStreamSource: vi.fn(),
}));

function scope(orgId: string): Scope {
  return { orgId, permissions: [], allowedCollectionIds: [] };
}

function wrapperFor(manager: ScopeManager) {
  return function Wrapper({ children }: { children: React.ReactNode }) {
    return <ScopeProvider manager={manager}>{children}</ScopeProvider>;
  };
}

function fakeSseSource<M>() {
  let pendingResolve: ((result: IteratorResult<M>) => void) | undefined;
  const queue: M[] = [];
  const abort = vi.fn();

  const source: ScopeSafeSseSource<M> = {
    abort,
    [Symbol.asyncIterator]() {
      return {
        next(): Promise<IteratorResult<M>> {
          if (queue.length > 0) {
            return Promise.resolve({ value: queue.shift() as M, done: false });
          }
          return new Promise<IteratorResult<M>>((resolve) => {
            pendingResolve = resolve;
          });
        },
      };
    },
  };

  return {
    source,
    abort,
    push(message: M) {
      if (pendingResolve) {
        const resolve = pendingResolve;
        pendingResolve = undefined;
        resolve({ value: message, done: false });
      } else {
        queue.push(message);
      }
    },
  };
}

async function flushMicrotasks(times = 3) {
  for (let i = 0; i < times; i += 1) {
    await Promise.resolve();
  }
}

function makeEvent(id: number, event: string, data: unknown): SseMessage {
  return {
    kind: 'event',
    id: String(id),
    envelope: {
      version: 1,
      sequence: id,
      event,
      requestId: 'req-1',
      data,
    },
  };
}

const mockTokenProvider: TokenProvider = {
  getAccessToken: vi.fn().mockResolvedValue('test-token'),
  refreshNow: vi.fn().mockResolvedValue('test-token'),
  onSessionLost: vi.fn().mockReturnValue(() => {}),
};

const sampleRequest: AskRequest = {
  question: 'Chính sách nghỉ phép năm là gì?',
  projectId: 'proj-1',
};

describe('useAskStream', () => {
  let manager: ScopeManager;
  let fakeSources: ReturnType<typeof fakeSseSource<SseMessage>>[];

  beforeEach(() => {
    manager = createScopeManager();
    manager.setScope(scope('org-a'));
    fakeSources = [];

    vi.mocked(createAskStreamSource).mockImplementation(() => {
      const fake = fakeSseSource<SseMessage>();
      fakeSources.push(fake);
      return fake.source;
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('updates state incrementally and in order across partial chunks until completed', async () => {
    const { result } = renderHook(() => useAskStream(mockTokenProvider), {
      wrapper: wrapperFor(manager),
    });

    expect(result.current.isActive).toBe(false);
    expect(result.current.state.status).toBe('idle');
    expect(result.current.state.answer).toBe('');

    act(() => {
      result.current.ask(sampleRequest);
    });

    expect(result.current.isActive).toBe(true);
    expect(fakeSources).toHaveLength(1);
    const fake = fakeSources[0];

    await act(async () => {
      fake.push(
        makeEvent(1, 'ask.started', {
          streamSessionId: 'sess-100',
          mode: 'offline_extractive',
        }),
      );
      await flushMicrotasks();
    });

    expect(result.current.state.status).toBe('streaming');
    expect(result.current.state.streamSessionId).toBe('sess-100');
    expect(result.current.state.answerMode).toBe('offline_extractive');

    await act(async () => {
      fake.push(makeEvent(2, 'ask.token', { text: 'Theo quy định, ' }));
      await flushMicrotasks();
    });
    expect(result.current.state.answer).toBe('Theo quy định, ');

    await act(async () => {
      fake.push(makeEvent(3, 'ask.token', { text: 'mỗi nhân viên được 12 ngày phép.' }));
      await flushMicrotasks();
    });
    expect(result.current.state.answer).toBe('Theo quy định, mỗi nhân viên được 12 ngày phép.');

    await act(async () => {
      fake.push(
        makeEvent(4, 'ask.completed', {
          mode: 'offline_extractive',
          streamSessionId: 'sess-100',
        }),
      );
      fake.push({
        kind: 'closed',
        reason: { type: 'server', code: 'completed' },
      });
      await flushMicrotasks();
    });

    expect(result.current.state.status).toBe('completed');
    expect(result.current.state.errorReason).toBeUndefined();
  });

  it('interrupted mid-way: escapes "streaming" state instead of hanging, showing error reason', async () => {
    const { result } = renderHook(() => useAskStream(mockTokenProvider), {
      wrapper: wrapperFor(manager),
    });

    act(() => {
      result.current.ask(sampleRequest);
    });

    const fake = fakeSources[0];

    await act(async () => {
      fake.push(
        makeEvent(1, 'ask.started', {
          streamSessionId: 'sess-101',
          mode: 'offline_extractive',
        }),
      );
      fake.push(makeEvent(2, 'ask.token', { text: 'Nội dung đang được truyền thì...' }));
      await flushMicrotasks();
    });

    expect(result.current.state.status).toBe('streaming');
    expect(result.current.state.answer).toBe('Nội dung đang được truyền thì...');

    // Simulate stream interruption via network error close frame
    await act(async () => {
      fake.push({
        kind: 'closed',
        reason: { type: 'network-error' },
      });
      await flushMicrotasks();
    });

    // Must not be stuck in "streaming" ("đang trả lời")
    expect(result.current.state.status).toBe('error');
    expect(result.current.state.errorReason).toBe('network');
    expect(describeAskStreamError(result.current.state.errorReason)).toBe(
      'Mất kết nối mạng khi đang nhận câu trả lời.',
    );
    // Partial tokens received prior to interruption are preserved
    expect(result.current.state.answer).toBe('Nội dung đang được truyền thì...');
  });

  it('cancels stream on reset: aborts request and ignores further chunks', async () => {
    const { result } = renderHook(() => useAskStream(mockTokenProvider), {
      wrapper: wrapperFor(manager),
    });

    act(() => {
      result.current.ask(sampleRequest);
    });

    const fake = fakeSources[0];

    await act(async () => {
      fake.push(
        makeEvent(1, 'ask.started', {
          streamSessionId: 'sess-102',
          mode: 'offline_extractive',
        }),
      );
      fake.push(makeEvent(2, 'ask.token', { text: 'Câu trả lời đang chạy dở...' }));
      await flushMicrotasks();
    });

    expect(result.current.isActive).toBe(true);
    expect(result.current.state.status).toBe('streaming');

    // Cancel / reset in-flight stream
    act(() => {
      result.current.reset();
    });

    expect(result.current.isActive).toBe(false);
    expect(result.current.state.status).toBe('idle');
    expect(result.current.state.answer).toBe('');
    expect(fake.abort).toHaveBeenCalled();

    // Late chunk arriving from aborted stream must not mutate state
    await act(async () => {
      fake.push(makeEvent(3, 'ask.token', { text: 'tin nhắn trễ sau khi huỷ' }));
      await flushMicrotasks();
    });

    expect(result.current.state.status).toBe('idle');
    expect(result.current.state.answer).toBe('');
  });

  it('asks again after cancellation: prevents old and new turns from mixing contents', async () => {
    const { result } = renderHook(() => useAskStream(mockTokenProvider), {
      wrapper: wrapperFor(manager),
    });

    // 1. First turn: asked then canceled mid-stream
    act(() => {
      result.current.ask({ ...sampleRequest, question: 'Lượt hỏi 1' });
    });
    const firstFake = fakeSources[0];

    await act(async () => {
      firstFake.push(
        makeEvent(1, 'ask.started', { streamSessionId: 'sess-turn-1', mode: 'offline' }),
      );
      firstFake.push(makeEvent(2, 'ask.token', { text: 'Dữ liệu lượt 1' }));
      await flushMicrotasks();
    });
    expect(result.current.state.answer).toBe('Dữ liệu lượt 1');

    act(() => {
      result.current.reset();
    });
    expect(firstFake.abort).toHaveBeenCalled();

    // 2. Second turn: fresh question
    act(() => {
      result.current.ask({ ...sampleRequest, question: 'Lượt hỏi 2' });
    });
    expect(fakeSources).toHaveLength(2);
    const secondFake = fakeSources[1];

    await act(async () => {
      secondFake.push(
        makeEvent(1, 'ask.started', { streamSessionId: 'sess-turn-2', mode: 'offline' }),
      );
      secondFake.push(makeEvent(2, 'ask.token', { text: 'Nội dung hoàn toàn mới.' }));
      await flushMicrotasks();
    });

    // Old turn text must not be prepended or mixed into new turn
    expect(result.current.state.answer).toBe('Nội dung hoàn toàn mới.');
    expect(result.current.state.streamSessionId).toBe('sess-turn-2');

    // Late message from first turn should be discarded
    await act(async () => {
      firstFake.push(makeEvent(3, 'ask.token', { text: ' và rác từ lượt 1' }));
      await flushMicrotasks();
    });

    expect(result.current.state.answer).toBe('Nội dung hoàn toàn mới.');
  });

  it('handles warnings (e.g. token limits, fallback) properly without throwing or crashing', async () => {
    const { result } = renderHook(() => useAskStream(mockTokenProvider), {
      wrapper: wrapperFor(manager),
    });

    act(() => {
      result.current.ask(sampleRequest);
    });
    const fake = fakeSources[0];

    const warningText =
      'Dev-gate: LLM answer passed citation/claim checks but structured entailment is NOT available — this answer is unverified, not grounded.';

    await act(async () => {
      fake.push(makeEvent(1, 'ask.started', { streamSessionId: 'sess-warn', mode: 'offline' }));
      fake.push(makeEvent(2, 'ask.warning', { message: warningText }));
      fake.push(makeEvent(3, 'ask.token', { text: 'Câu trả lời kèm cảnh báo.' }));
      await flushMicrotasks();
    });

    expect(result.current.state.warnings).toHaveLength(1);
    expect(result.current.state.warnings[0]).toBe(warningText);
    expect(result.current.state.answer).toBe('Câu trả lời kèm cảnh báo.');

    // Ensure warningPresentation cleanly processes it
    const presented = presentWarnings(result.current.state.warnings);
    expect(presented.summary).toContain('chưa được kiểm chứng đầy đủ');
  });
});
