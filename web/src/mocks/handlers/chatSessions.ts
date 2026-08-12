// P2-19 web follow-up — private per-user Q&A chat history mock, mirroring
// `crates/server/src/routes/chat_sessions.rs` (see that file's module doc for
// the full server-side rationale this mirrors):
//
//   - Every operation resolves the caller via `authContextForHeader` and then
//     scopes to `(auth.orgId, auth.user.userId)` through
//     `getUserChatSessions`/`findUserChatSession` (`mocks/fixtures.ts`) — a
//     session id that exists but belongs to another user (even in the same
//     org) 404s exactly like one that never existed, never a 403 (no
//     existence oracle — same "RLS + ownership 404 identically" contract the
//     real DB layer enforces via RLS).
//   - Citations/warnings are accepted and returned opaque/verbatim — this
//     mock never re-validates a citation's hash/span/ACL on read, same
//     documented trade-off as the real route (`AppendChatTurnRequest`'s own
//     doc comment, `contract.ts`).
//   - Permission note: the real server gates every route here on `qa.query`
//     (`services::retrieval::PERMISSION_QA_QUERY`) — same precedent
//     `handlers/qa.ts`'s `search`/`ask` and `handlers/graph.ts` already set
//     (the seeded `DEMO_USER` fixture does not carry `qa.query`, so enforcing
//     it here alone would make the demo user unable to open the very page
//     this whole slice lives on). Left unenforced for the same reason.
//   - `seq` is assigned as `(current turn count) + 1`, matching the real
//     server's `max+1` — mock handlers run one request at a time (no
//     concurrent-append race to guard against with a `FOR UPDATE` lock here).
import { registerOperation } from '../registry';
import { apiError, notFound, unauthorized } from '../apiError';
import { decodeCursor, encodeCursor, mockTimestamp } from '../ids';
import {
  authContextForHeader,
  findUserChatSession,
  getUserChatSessions,
  nextChatSessionActivityRank,
  nextId,
  type ChatSessionRecord,
  type ChatTurnRecord,
} from '../fixtures';
import type { components } from '../../api/generated/contract';

type ChatSession = components['schemas']['ChatSession'];
type ChatSessionDetail = components['schemas']['ChatSessionDetail'];
type ChatTurn = components['schemas']['ChatTurn'];
type CreateChatSessionRequest = components['schemas']['CreateChatSessionRequest'];
type UpdateChatSessionRequest = components['schemas']['UpdateChatSessionRequest'];
type AppendChatTurnRequest = components['schemas']['AppendChatTurnRequest'];

/** Mirrors `db::chat_sessions::MAX_TITLE_LEN`. */
const MAX_TITLE_LEN = 200;
/** Mirrors `db::chat_sessions::MAX_QUESTION_LEN`. */
const MAX_QUESTION_LEN = 8_192;
/** Mirrors `db::chat_sessions::ALLOWED_ANSWER_MODES` exactly. */
const ALLOWED_ANSWER_MODES = new Set([
  'offline_extractive',
  'fallback_extractive',
  'local_llm',
  'cloud_llm',
  'subscription_cli',
  'llm_unverified',
  'assistant',
]);

function validationFailed(message: string) {
  return { status: 400, body: apiError('validation_failed', message) };
}

function validateTitle(rawTitle: unknown): string | null {
  if (typeof rawTitle !== 'string') return null;
  const trimmed = rawTitle.trim();
  if (trimmed.length === 0 || trimmed.length > MAX_TITLE_LEN) return null;
  return trimmed;
}

function validateQuestion(rawQuestion: unknown): string | null {
  if (typeof rawQuestion !== 'string') return null;
  const trimmed = rawQuestion.trim();
  if (trimmed.length === 0 || trimmed.length > MAX_QUESTION_LEN) return null;
  return trimmed;
}

function sessionDto(session: ChatSessionRecord): ChatSession {
  return {
    id: session.id,
    title: session.title,
    createdAt: session.createdAt,
    updatedAt: session.updatedAt,
  };
}

function turnDto(turn: ChatTurnRecord): ChatTurn {
  return {
    id: turn.id,
    seq: turn.seq,
    question: turn.question,
    answer: turn.answer,
    answerMode: turn.answerMode as ChatTurn['answerMode'],
    citations: turn.citations,
    warnings: turn.warnings,
    createdAt: turn.createdAt,
  };
}

/** "Most recently active first" — `activityRank` descending (see that field's own doc for why a plain `updatedAt` comparison isn't enough in this fixture set). */
function byMostRecentlyActive(a: ChatSessionRecord, b: ChatSessionRecord): number {
  return b.activityRank - a.activityRank;
}

// ---------------------------------------------------------------------------
// GET /chat-sessions
// ---------------------------------------------------------------------------

registerOperation('listChatSessions', (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();

  const sessions = [...getUserChatSessions(auth.orgId, auth.user.userId)].sort(
    byMostRecentlyActive,
  );

  const rawLimit = Number(ctx.query.get('limit'));
  const limit =
    Number.isFinite(rawLimit) && rawLimit > 0 ? Math.min(Math.floor(rawLimit), 100) : 50;
  const offset = decodeCursor(ctx.query.get('cursor'));
  const page = sessions.slice(offset, offset + limit);
  const hasMore = offset + limit < sessions.length;

  return {
    status: 200,
    body: {
      items: page.map(sessionDto),
      page: { hasMore, nextCursor: hasMore ? encodeCursor(offset + limit) : null },
    },
  };
});

// ---------------------------------------------------------------------------
// POST /chat-sessions
// ---------------------------------------------------------------------------

registerOperation('createChatSession', async (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const body = await ctx.json<CreateChatSessionRequest>();
  const title = validateTitle(body.title);
  if (title === null) return validationFailed('title must be 1..=200 characters (trimmed).');

  const now = mockTimestamp(0);
  const session: ChatSessionRecord = {
    id: nextId(),
    orgId: auth.orgId,
    userId: auth.user.userId,
    title,
    createdAt: now,
    updatedAt: now,
    activityRank: nextChatSessionActivityRank(),
    turns: [],
  };
  getUserChatSessions(auth.orgId, auth.user.userId).push(session);
  return { status: 201, body: sessionDto(session) };
});

// ---------------------------------------------------------------------------
// GET /chat-sessions/{sessionId}
// ---------------------------------------------------------------------------

registerOperation('getChatSession', (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const session = findUserChatSession(auth.orgId, auth.user.userId, ctx.params.sessionId);
  if (!session) return notFound(`Chat session ${ctx.params.sessionId} does not exist.`);
  const detail: ChatSessionDetail = {
    ...sessionDto(session),
    turns: session.turns.map(turnDto),
  };
  return { status: 200, body: detail };
});

// ---------------------------------------------------------------------------
// PATCH /chat-sessions/{sessionId} — rename
// ---------------------------------------------------------------------------

registerOperation('updateChatSession', async (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const session = findUserChatSession(auth.orgId, auth.user.userId, ctx.params.sessionId);
  if (!session) return notFound(`Chat session ${ctx.params.sessionId} does not exist.`);
  const body = await ctx.json<UpdateChatSessionRequest>();
  const title = validateTitle(body.title);
  if (title === null) return validationFailed('title must be 1..=200 characters (trimmed).');
  session.title = title;
  session.updatedAt = mockTimestamp(0);
  session.activityRank = nextChatSessionActivityRank();
  return { status: 200, body: sessionDto(session) };
});

// ---------------------------------------------------------------------------
// DELETE /chat-sessions/{sessionId}
// ---------------------------------------------------------------------------

registerOperation('deleteChatSession', (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const sessions = getUserChatSessions(auth.orgId, auth.user.userId);
  const index = sessions.findIndex((s) => s.id === ctx.params.sessionId);
  if (index === -1) return notFound(`Chat session ${ctx.params.sessionId} does not exist.`);
  sessions.splice(index, 1);
  return { status: 204 };
});

// ---------------------------------------------------------------------------
// POST /chat-sessions/{sessionId}/turns
// ---------------------------------------------------------------------------

registerOperation('appendChatTurn', async (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const session = findUserChatSession(auth.orgId, auth.user.userId, ctx.params.sessionId);
  if (!session) return notFound(`Chat session ${ctx.params.sessionId} does not exist.`);

  const body = await ctx.json<AppendChatTurnRequest>();
  const question = validateQuestion(body.question);
  if (question === null) {
    return validationFailed('question must be 1..=8192 characters (trimmed).');
  }
  if (typeof body.answer !== 'string' || body.answer.length === 0) {
    return validationFailed('answer must not be empty.');
  }
  if (!ALLOWED_ANSWER_MODES.has(body.answerMode)) {
    return validationFailed(`answerMode "${body.answerMode}" is not a recognized answer mode.`);
  }
  if (body.citations !== undefined && !Array.isArray(body.citations)) {
    return validationFailed('citations must be an array when present.');
  }
  if (body.warnings !== undefined && !Array.isArray(body.warnings)) {
    return validationFailed('warnings must be an array when present.');
  }

  const now = mockTimestamp(0);
  const turn: ChatTurnRecord = {
    id: nextId(),
    seq: session.turns.length + 1,
    question,
    answer: body.answer,
    answerMode: body.answerMode,
    citations: body.citations ?? [],
    warnings: body.warnings ?? [],
    createdAt: now,
  };
  session.turns.push(turn);
  session.updatedAt = now;
  session.activityRank = nextChatSessionActivityRank();

  return { status: 201, body: turnDto(turn) };
});
