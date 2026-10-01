import { act, renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ApiClient } from '../../api/client';
import type { components } from '../../api/generated/contract';
import { createScopeManager, type Scope, type ScopeManager } from '../../state/scope';
import { ScopeProvider } from '../../state/ScopeProvider';
import { useChatHistory, type RecordableTurn } from './useChatHistory';

type ChatSession = components['schemas']['ChatSession'];
type ChatTurn = components['schemas']['ChatTurn'];

interface MockRequestOptions {
  body?: {
    title?: string;
    question?: string;
    answer?: string;
    answerMode?: components['schemas']['AppendChatTurnRequest']['answerMode'];
    citations?: components['schemas']['CitationPin'][];
    warnings?: string[];
  };
  params?: {
    path?: { sessionId?: string };
    query?: { limit?: number; cursor?: string };
  };
  signal?: AbortSignal;
}

function scope(orgId: string): Scope {
  return { orgId, permissions: [], allowedCollectionIds: [] };
}

function wrapperFor(manager: ScopeManager) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return <ScopeProvider manager={manager}>{children}</ScopeProvider>;
  };
}

const sampleTurn: RecordableTurn = {
  question: 'Quy trình tiếp nhận nhân viên mới như thế nào?',
  answer: 'Nhân viên mới cần hoàn thành hồ sơ trong 3 ngày làm việc.',
  answerMode: 'assistant',
  citations: [],
  warnings: [],
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useChatHistory', () => {
  it('creates a new session and syncs activeSessionIdRef when recording a turn with no active session', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-1'));

    const createdSession: ChatSession = {
      id: 'sess-new-1',
      title: 'Quy trình tiếp nhận nhân viên mới như thế nào?',
      createdAt: '2026-10-01T00:00:00Z',
      updatedAt: '2026-10-01T00:00:00Z',
    };

    const requestMock = vi.fn(
      async (method: string, path: string, options?: MockRequestOptions) => {
        if (method === 'get' && path === '/chat-sessions') {
          return { items: [], page: { nextCursor: null, hasMore: false } };
        }
        if (method === 'post' && path === '/chat-sessions') {
          return createdSession;
        }
        if (method === 'post' && path === '/chat-sessions/{sessionId}/turns') {
          const turn: ChatTurn = {
            id: 'turn-1',
            seq: 1,
            question: options?.body?.question ?? '',
            answer: options?.body?.answer ?? '',
            answerMode: options?.body?.answerMode ?? 'offline_extractive',
            citations: options?.body?.citations ?? [],
            warnings: options?.body?.warnings ?? [],
            createdAt: '2026-10-01T00:00:00Z',
          };
          return turn;
        }
        if (method === 'get' && path === '/chat-sessions/{sessionId}') {
          return {
            ...createdSession,
            turns: [],
          };
        }
        return {};
      },
    );

    const client = { request: requestMock } as unknown as ApiClient;
    const { result } = renderHook(() => useChatHistory(client), {
      wrapper: wrapperFor(manager),
    });

    expect(result.current.activeSessionId).toBeUndefined();

    // Trigger recording a turn when no session is active
    act(() => {
      result.current.recordTurn(sampleTurn);
    });

    // Wait for the new session to be created and activeSessionId to be set
    await waitFor(() => {
      expect(result.current.activeSessionId).toBe('sess-new-1');
    });

    // Verify session creation called with truncated question as title
    expect(requestMock).toHaveBeenCalledWith(
      'post',
      '/chat-sessions',
      expect.objectContaining({
        body: { title: sampleTurn.question },
      }),
    );

    // Verify the turn was recorded into the newly created session
    expect(requestMock).toHaveBeenCalledWith(
      'post',
      '/chat-sessions/{sessionId}/turns',
      expect.objectContaining({
        params: { path: { sessionId: 'sess-new-1' } },
        body: expect.objectContaining({
          question: sampleTurn.question,
          answer: sampleTurn.answer,
          answerMode: 'assistant',
        }),
      }),
    );

    // Record a second turn to verify activeSessionIdRef was updated via effect and reused
    const secondTurn: RecordableTurn = {
      question: 'Thời gian thử việc là bao lâu?',
      answer: 'Thời gian thử việc tiêu chuẩn là 2 tháng.',
      answerMode: 'assistant',
      citations: [],
      warnings: [],
    };

    act(() => {
      result.current.recordTurn(secondTurn);
    });

    await waitFor(() => {
      expect(requestMock).toHaveBeenCalledWith(
        'post',
        '/chat-sessions/{sessionId}/turns',
        expect.objectContaining({
          params: { path: { sessionId: 'sess-new-1' } },
          body: expect.objectContaining({
            question: secondTurn.question,
            answer: secondTurn.answer,
          }),
        }),
      );
    });

    // Confirms that POST /chat-sessions was called only ONCE for the whole conversation
    const createSessionCalls = requestMock.mock.calls.filter(
      (call) => call[0] === 'post' && call[1] === '/chat-sessions',
    );
    expect(createSessionCalls).toHaveLength(1);
  });

  it('resets activeSessionId and historicalTurns when switching organization/scope', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));

    const orgASession: ChatSession = {
      id: 'sess-org-a',
      title: 'Phiên của Tổ chức A',
      createdAt: '2026-10-01T00:00:00Z',
      updatedAt: '2026-10-01T00:00:00Z',
    };

    const orgATurn: ChatTurn = {
      id: 'turn-a1',
      seq: 1,
      question: 'Câu hỏi ở Org A',
      answer: 'Câu trả lời ở Org A',
      answerMode: 'assistant',
      citations: [],
      warnings: [],
      createdAt: '2026-10-01T00:00:00Z',
    };

    const createdOrgBSession: ChatSession = {
      id: 'sess-org-b',
      title: 'Phiên của Tổ chức B',
      createdAt: '2026-10-01T00:00:00Z',
      updatedAt: '2026-10-01T00:00:00Z',
    };

    const requestMock = vi.fn(
      async (method: string, path: string, options?: MockRequestOptions) => {
        if (method === 'get' && path === '/chat-sessions') {
          return {
            items: [orgASession],
            page: { nextCursor: null, hasMore: false },
          };
        }
        if (method === 'get' && path === '/chat-sessions/{sessionId}') {
          if (options?.params?.path?.sessionId === 'sess-org-a') {
            return {
              ...orgASession,
              turns: [orgATurn],
            };
          }
          return {
            ...createdOrgBSession,
            turns: [],
          };
        }
        if (method === 'post' && path === '/chat-sessions') {
          return createdOrgBSession;
        }
        if (method === 'post' && path === '/chat-sessions/{sessionId}/turns') {
          return {
            id: 'turn-b1',
            seq: 1,
            question: options?.body?.question ?? '',
            answer: options?.body?.answer ?? '',
            answerMode: options?.body?.answerMode ?? 'offline_extractive',
            citations: [],
            warnings: [],
            createdAt: '2026-10-01T00:00:00Z',
          };
        }
        return {};
      },
    );

    const client = { request: requestMock } as unknown as ApiClient;
    const { result } = renderHook(() => useChatHistory(client), {
      wrapper: wrapperFor(manager),
    });

    // Select the session from Org A
    act(() => {
      result.current.selectSession('sess-org-a');
    });

    await waitFor(() => {
      expect(result.current.activeSessionId).toBe('sess-org-a');
      expect(result.current.historicalTurns).toHaveLength(1);
    });

    // Switch scope to Org B
    act(() => {
      manager.setScope(scope('org-b'));
    });

    // activeSessionId must be cleared to undefined, historicalTurns reset to empty
    await waitFor(() => {
      expect(result.current.activeSessionId).toBeUndefined();
      expect(result.current.historicalTurns).toHaveLength(0);
    });

    // When recording a turn in Org B, it must create a new session rather than reusing Org A's ID
    act(() => {
      result.current.recordTurn(sampleTurn);
    });

    await waitFor(() => {
      expect(result.current.activeSessionId).toBe('sess-org-b');
    });

    expect(requestMock).toHaveBeenCalledWith(
      'post',
      '/chat-sessions/{sessionId}/turns',
      expect.objectContaining({
        params: { path: { sessionId: 'sess-org-b' } },
      }),
    );
  });

  it('sets appendError when recording a turn fails without throwing outward', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-1'));

    let shouldFail = true;
    const requestMock = vi.fn(async (method: string, path: string) => {
      if (method === 'get' && path === '/chat-sessions') {
        return { items: [], page: { nextCursor: null, hasMore: false } };
      }
      if (method === 'post' && path === '/chat-sessions') {
        if (shouldFail) {
          throw new Error('500 Internal Server Error');
        }
        return {
          id: 'sess-recovered',
          title: 'Tiêu đề phục hồi',
          createdAt: '2026-10-01T00:00:00Z',
          updatedAt: '2026-10-01T00:00:00Z',
        };
      }
      if (method === 'post' && path === '/chat-sessions/{sessionId}/turns') {
        return { id: 'turn-1', seq: 1 };
      }
      return {};
    });

    const client = { request: requestMock } as unknown as ApiClient;
    const { result } = renderHook(() => useChatHistory(client), {
      wrapper: wrapperFor(manager),
    });

    // Invoking recordTurn when backend throws must not throw unhandled rejection
    expect(() => {
      act(() => {
        result.current.recordTurn(sampleTurn);
      });
    }).not.toThrow();

    // appendError state is set
    await waitFor(() => {
      expect(result.current.appendError).toBe(
        'Không thể lưu lượt hỏi đáp này vào lịch sử — cuộc trò chuyện vẫn tiếp tục bình thường.',
      );
    });

    // Dismissing error clears appendError
    act(() => {
      result.current.dismissAppendError();
    });
    expect(result.current.appendError).toBeUndefined();

    // Subsequent chat actions continue working normally
    shouldFail = false;
    act(() => {
      result.current.recordTurn({
        ...sampleTurn,
        question: 'Câu hỏi mới sau khi mạng ổn định',
      });
    });

    await waitFor(() => {
      expect(result.current.activeSessionId).toBe('sess-recovered');
      expect(result.current.appendError).toBeUndefined();
    });
  });

  it('handles concurrent renameSession / deleteSession and recordTurn without race conditions', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-1'));

    const existingSession: ChatSession = {
      id: 'sess-concurrent',
      title: 'Phiên đồng thời ban đầu',
      createdAt: '2026-10-01T00:00:00Z',
      updatedAt: '2026-10-01T00:00:00Z',
    };

    let sessionTitle = existingSession.title;
    const requestMock = vi.fn(
      async (method: string, path: string, options?: MockRequestOptions) => {
        if (method === 'get' && path === '/chat-sessions') {
          return {
            items: [{ ...existingSession, title: sessionTitle }],
            page: { nextCursor: null, hasMore: false },
          };
        }
        if (method === 'get' && path === '/chat-sessions/{sessionId}') {
          return {
            ...existingSession,
            title: sessionTitle,
            turns: [],
          };
        }
        if (method === 'patch' && path === '/chat-sessions/{sessionId}') {
          await new Promise((r) => setTimeout(r, 20));
          sessionTitle = options?.body?.title ?? sessionTitle;
          return {
            ...existingSession,
            title: sessionTitle,
          };
        }
        if (method === 'post' && path === '/chat-sessions/{sessionId}/turns') {
          await new Promise((r) => setTimeout(r, 20));
          return { id: 'turn-1', seq: 1 };
        }
        if (method === 'delete' && path === '/chat-sessions/{sessionId}') {
          await new Promise((r) => setTimeout(r, 20));
          return {};
        }
        return {};
      },
    );

    const client = { request: requestMock } as unknown as ApiClient;
    const { result } = renderHook(() => useChatHistory(client), {
      wrapper: wrapperFor(manager),
    });

    act(() => {
      result.current.selectSession('sess-concurrent');
    });

    await waitFor(() => {
      expect(result.current.activeSessionId).toBe('sess-concurrent');
    });

    // 1. Concurrent renameSession and recordTurn
    act(() => {
      result.current.recordTurn(sampleTurn);
      void result.current.renameSession('sess-concurrent', 'Phiên đã đổi tên');
    });

    expect(result.current.renamingSessionId).toBe('sess-concurrent');

    await waitFor(() => {
      expect(result.current.renamingSessionId).toBeUndefined();
      expect(result.current.renameError).toBeUndefined();
      expect(result.current.appendError).toBeUndefined();
    });

    // 2. Concurrent deleteSession and recordTurn
    const turnAfterRename: RecordableTurn = {
      ...sampleTurn,
      question: 'Hỏi thêm trước khi xóa',
    };

    act(() => {
      result.current.recordTurn(turnAfterRename);
      void result.current.deleteSession('sess-concurrent');
    });

    expect(result.current.deletingSessionId).toBe('sess-concurrent');

    await waitFor(() => {
      expect(result.current.deletingSessionId).toBeUndefined();
      expect(result.current.deleteError).toBeUndefined();
      expect(result.current.activeSessionId).toBeUndefined();
    });
  });

  it('serializes rapid consecutive recordTurn calls to prevent creating duplicate sessions', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-1'));

    let sessionCreateCount = 0;
    const requestMock = vi.fn(
      async (method: string, path: string, options?: MockRequestOptions) => {
        if (method === 'get' && path === '/chat-sessions') {
          return { items: [], page: { nextCursor: null, hasMore: false } };
        }
        if (method === 'post' && path === '/chat-sessions') {
          sessionCreateCount += 1;
          await new Promise((r) => setTimeout(r, 15));
          return {
            id: 'sess-single-created',
            title: options?.body?.title ?? 'Session',
            createdAt: '2026-10-01T00:00:00Z',
            updatedAt: '2026-10-01T00:00:00Z',
          };
        }
        if (method === 'post' && path === '/chat-sessions/{sessionId}/turns') {
          await new Promise((r) => setTimeout(r, 10));
          return { id: 'turn-id', seq: 1 };
        }
        if (method === 'get' && path === '/chat-sessions/{sessionId}') {
          return {
            id: 'sess-single-created',
            title: 'Session',
            turns: [],
          };
        }
        return {};
      },
    );

    const client = { request: requestMock } as unknown as ApiClient;
    const { result } = renderHook(() => useChatHistory(client), {
      wrapper: wrapperFor(manager),
    });

    const turn1: RecordableTurn = { ...sampleTurn, question: 'Câu 1' };
    const turn2: RecordableTurn = { ...sampleTurn, question: 'Câu 2' };

    act(() => {
      result.current.recordTurn(turn1);
      result.current.recordTurn(turn2);
    });

    await waitFor(() => {
      expect(result.current.activeSessionId).toBe('sess-single-created');
    });

    expect(sessionCreateCount).toBe(1);

    const turnCalls = requestMock.mock.calls.filter(
      (call) => call[0] === 'post' && call[1] === '/chat-sessions/{sessionId}/turns',
    );
    expect(turnCalls).toHaveLength(2);
    expect(turnCalls[0]?.[2]?.params?.path?.sessionId).toBe('sess-single-created');
    expect(turnCalls[1]?.[2]?.params?.path?.sessionId).toBe('sess-single-created');
  });

  it('truncates session titles longer than 80 characters', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-1'));

    const longQuestion = 'A'.repeat(120);
    const requestMock = vi.fn(
      async (method: string, path: string, options?: MockRequestOptions) => {
        if (method === 'get' && path === '/chat-sessions') {
          return { items: [], page: { nextCursor: null, hasMore: false } };
        }
        if (method === 'post' && path === '/chat-sessions') {
          return {
            id: 'sess-long',
            title: options?.body?.title ?? 'Session',
            createdAt: '2026-10-01T00:00:00Z',
            updatedAt: '2026-10-01T00:00:00Z',
          };
        }
        if (method === 'post' && path === '/chat-sessions/{sessionId}/turns') {
          return { id: 'turn-1', seq: 1 };
        }
        return {};
      },
    );

    const client = { request: requestMock } as unknown as ApiClient;
    const { result } = renderHook(() => useChatHistory(client), {
      wrapper: wrapperFor(manager),
    });

    act(() => {
      result.current.recordTurn({
        ...sampleTurn,
        question: longQuestion,
      });
    });

    await waitFor(() => {
      expect(result.current.activeSessionId).toBe('sess-long');
    });

    const expectedTitle = `${'A'.repeat(79)}…`;
    expect(requestMock).toHaveBeenCalledWith(
      'post',
      '/chat-sessions',
      expect.objectContaining({
        body: { title: expectedTitle },
      }),
    );
  });

  it('degrades unrecognized answerMode to fallback_extractive/offline_extractive', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-1'));

    const requestMock = vi.fn(async (method: string, path: string) => {
      if (method === 'get' && path === '/chat-sessions') {
        return { items: [], page: { nextCursor: null, hasMore: false } };
      }
      if (method === 'post' && path === '/chat-sessions') {
        return {
          id: 'sess-mode',
          title: 'Title',
          createdAt: '2026-10-01T00:00:00Z',
          updatedAt: '2026-10-01T00:00:00Z',
        };
      }
      if (method === 'post' && path === '/chat-sessions/{sessionId}/turns') {
        return { id: 'turn-1', seq: 1 };
      }
      return {};
    });

    const client = { request: requestMock } as unknown as ApiClient;
    const { result } = renderHook(() => useChatHistory(client), {
      wrapper: wrapperFor(manager),
    });

    act(() => {
      result.current.recordTurn({
        ...sampleTurn,
        answerMode: 'unrecognized_super_ai_mode',
      });
    });

    await waitFor(() => {
      expect(result.current.activeSessionId).toBe('sess-mode');
    });

    expect(requestMock).toHaveBeenCalledWith(
      'post',
      '/chat-sessions/{sessionId}/turns',
      expect.objectContaining({
        body: expect.objectContaining({
          answerMode: 'offline_extractive',
        }),
      }),
    );
  });

  it('manages sessionSwitchToken and startNewConversation correctly', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-1'));

    const requestMock = vi.fn(async (method: string, path: string) => {
      if (method === 'get' && path === '/chat-sessions') {
        return { items: [], page: { nextCursor: null, hasMore: false } };
      }
      if (method === 'get' && path === '/chat-sessions/{sessionId}') {
        return { id: 'sess-1', title: 'Session 1', turns: [] };
      }
      return {};
    });

    const client = { request: requestMock } as unknown as ApiClient;
    const { result } = renderHook(() => useChatHistory(client), {
      wrapper: wrapperFor(manager),
    });

    expect(result.current.sessionSwitchToken).toBe(0);

    // Select a session -> sessionSwitchToken increments
    act(() => {
      result.current.selectSession('sess-1');
    });
    expect(result.current.activeSessionId).toBe('sess-1');
    expect(result.current.sessionSwitchToken).toBe(1);

    // Re-selecting the already active session does NOT bump sessionSwitchToken
    act(() => {
      result.current.selectSession('sess-1');
    });
    expect(result.current.sessionSwitchToken).toBe(1);

    // Starting a new conversation bumps token and resets activeSessionId
    act(() => {
      result.current.startNewConversation();
    });
    expect(result.current.activeSessionId).toBeUndefined();
    expect(result.current.sessionSwitchToken).toBe(2);
  });
});
