// P2-10 (Q&A). `search`/`ask` were the P2-02 baseline (synchronous, still kept
// below for back-compat with anything that calls them directly); `askStream`
// is new here.
//
// `askStream` is one of the two operations `registry.ts`'s
// `DELIBERATELY_UNMOCKED_OPERATIONS` still lists (alongside `jobEvents`) —
// that comment predates P2-10 and describes a real, still-true limitation:
// `mocks/**` is fetch-*level*, and a genuinely live `ReadableStream` (bytes
// trickling in over real ticks, the way `jobEvents` would need to for a
// long-lived job) is out of scope for it. `/ask/stream`'s shape sidesteps
// that rather than needing a fix to it: every event this mock will ever emit
// for one ask is already fully decided (from the request body) before the
// response is even built — nothing is genuinely asynchronous the way a job's
// live progress is — so the whole `text/event-stream` body is pre-serialized
// as one static string (`buildAskStreamBody` below) and returned as a normal
// `rawBody`, same mechanism `redeemDownload` already uses for a non-JSON
// response. `registerOperation('askStream', ...)` below is what actually
// makes this reachable — `DELIBERATELY_UNMOCKED_OPERATIONS` is scanned by
// `fetchMock.ts` only as a *fallback* once no registered route matches, so a
// registered handler wins first and that stale doc comment in `registry.ts`
// is the one loose end this leaves (out of this task's edit scope — see the
// P2-10 report). Reading the whole body at once, with no `setTimeout`
// between events, is also exactly what "deterministic, no sleep-race" (the
// task's own requirement) asks for: `SseConnection` (`api/sse.ts`) yields one
// parsed message at a time regardless of how many arrived in a single
// underlying chunk, so the client-side reducer still sees the same ordered
// sequence of dispatches a real trickle would produce — just without any
// wall-clock delay to race against in a test.
//
// Wire shape mirrored here, byte-for-byte against the real producer (cited so
// a change on that side is a deliberate decision to update this file, not a
// silent drift):
//   - Event names/data and their order — `ask.started` -> `ask.token`* ->
//     [`ask.warning`]* -> `ask.citations` -> `ask.version_context` ->
//     `ask.completed` -> terminal `stream.closed` (durable, carries an SSE
//     `id:`) — `crates/server/src/services/qa/ask_stream.rs` `run_producer`.
//   - `VersionContext`'s camelCase fields (`mode`/`currentVersionIds`/
//     `citedVersionIds`/`changeNote`) — `services/qa/grounding.rs`.
//   - The "current mode cited a non-current version" warning —
//     `services/qa/grounding.rs`'s `validate_answer_citations`.
//   - `stream.closed` reasons (`completed`, `citation_revoked`, ...) —
//     `services/qa/ask_stream.rs`'s `config_reason`, consumed client-side by
//     `api/sse.ts`'s `classifySseCloseCode`.
import { registerOperation } from '../registry';
import { notFound, unauthorized } from '../apiError';
import { nextRequestId } from '../ids';
import {
  authContextForHeader,
  collectionIdsForProject,
  getOrgProjects,
  getStore,
  nextId,
  QA_COMPARE_DOCUMENT_ID,
} from '../fixtures';
import type { components } from '../../api/generated/contract';

type SearchRequest = components['schemas']['SearchRequest'];
type AskRequest = components['schemas']['AskRequest'];
type CitationPin = components['schemas']['CitationPin'];
type SseEnvelope = components['schemas']['SseEnvelope'];

/**
 * P2-18 — resolves `projectId` (absent = "all projects", today's exact
 * behavior) into the effective `collectionIds` scope, narrowing (never
 * widening) whatever the request already specified — same contract as
 * `db::projects::resolve_project_scope` server-side. Returns `undefined` (a
 * 404) when `projectId` does not resolve to a project in the caller's org.
 */
function resolveProjectScope(
  orgId: string,
  projectId: string | undefined,
  requested: string[] | undefined,
): { collectionIds: string[] | undefined } | undefined {
  if (!projectId) return { collectionIds: requested };
  if (!getOrgProjects(orgId).some((p) => p.id === projectId)) return undefined;
  const projectCollections = collectionIdsForProject(orgId, projectId);
  const collectionIds = requested
    ? requested.filter((id) => projectCollections.includes(id))
    : projectCollections;
  return { collectionIds };
}

// ---------------------------------------------------------------------------
// Scenario markers — a test seam. Appending one of these tokens to a
// `question`/`query` string selects a deterministic mock scenario instead of
// the default grounded-answer path. Exported so `web/e2e/qa.spec.ts` and
// component tests import the exact token rather than hand-rolling a string
// that could silently drift from what this handler actually checks for —
// same "single source of truth" reasoning as `mocks/control.ts`'s
// `ForcedFailureKind` union.
export const QA_STREAM_MARKERS = {
  /** Mid-answer citation revocation: a few tokens stream, then the durable terminal closes with `reason: "citation_revoked"` — no citations/completed event ever arrives, matching `ask_stream.rs`'s `append()` failing closed the instant a cited document/version becomes inaccessible. */
  citationRevoked: '[[qa-e2e:citation-revoked]]',
  /** Provider-outage fallback: tokens still stream (from the same extractive answer, per `ask_stream.rs`'s `!streamed_any || FallbackExtractive` branch), plus an `ask.warning` and `ask.completed.mode: "fallback_extractive"`. */
  providerFallback: '[[qa-e2e:provider-fallback]]',
} as const;

function stripMarkers(text: string): string {
  return text
    .replace(QA_STREAM_MARKERS.citationRevoked, '')
    .replace(QA_STREAM_MARKERS.providerFallback, '')
    .trim();
}

// ---------------------------------------------------------------------------
// Passage catalog — the mock's stand-in for "what hybrid retrieval would
// find". Every seeded document (`mocks/fixtures.ts`) that ought to be
// citable gets one synthetic Vietnamese passage per published version here;
// nothing about it is derived from the contract (there is no ground-truth
// *content* to derive — same caveat `fixtures.ts`'s own module doc makes for
// its records).
// ---------------------------------------------------------------------------

interface Passage {
  documentId: string;
  collectionId: string;
  versionId: string;
  versionNumber: number;
  isCurrent: boolean;
  title: string;
  quote: string;
}

function passageCatalog(): Passage[] {
  const store = getStore();
  const passages: Passage[] = [];
  // Keyed by title, one passage per document that actually has a published
  // (`isCurrent`) version to cite — "Leave Policy.docx" (`mocks/fixtures.ts`)
  // is deliberately absent: it's seeded `state: 'converting'`/no version yet,
  // so real retrieval would have nothing to cite there either.
  const staticQuotes: Record<string, string> = {
    'Onboarding Guide.pdf':
      'Nhân viên mới cần hoàn thành khóa đào tạo hội nhập trong 30 ngày đầu tiên.',
    'Roadmap.xlsx': 'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
  };
  for (const [collectionId, docs] of store.documents) {
    for (const doc of docs) {
      if (doc.id === QA_COMPARE_DOCUMENT_ID) continue; // handled separately below (2 versions)
      const versions = store.versions.get(doc.id) ?? [];
      const current = versions.find((v) => v.isCurrent);
      const quote = staticQuotes[doc.title];
      if (!current || !quote) continue; // no published version, or nothing seeded to say about it
      passages.push({
        documentId: doc.id,
        collectionId,
        versionId: current.id,
        versionNumber: current.versionNumber,
        isCurrent: true,
        title: doc.title,
        quote,
      });
    }
  }
  // The one multi-version document — both versions are individually citable
  // (mode: 'current' only ever surfaces the current one; 'compare'/'history'
  // ask for both explicitly — see `resolveCompareOrHistoryPassages` below).
  const compareVersions = store.versions.get(QA_COMPARE_DOCUMENT_ID) ?? [];
  const compareDoc = [...store.documents.values()]
    .flat()
    .find((d) => d.id === QA_COMPARE_DOCUMENT_ID);
  const compareCollectionId = compareDoc?.collectionId ?? '';
  const compareQuotes: Record<number, string> = {
    1: 'Ngân sách vận hành được duyệt là 10 triệu đồng mỗi quý.',
    2: 'Ngân sách vận hành được điều chỉnh thành 15 triệu đồng mỗi quý theo thiết kế mới.',
  };
  for (const version of compareVersions) {
    const quote = compareQuotes[version.versionNumber];
    if (!quote || !compareDoc) continue;
    passages.push({
      documentId: compareDoc.id,
      collectionId: compareCollectionId,
      versionId: version.id,
      versionNumber: version.versionNumber,
      isCurrent: version.isCurrent,
      title: compareDoc.title,
      quote,
    });
  }
  return passages;
}

/** Word-level tokens (Unicode-aware, so Vietnamese diacritics survive), 2+ characters — a whole-question "does this substring literally appear in the passage" check would almost never hit, since a real question is never phrased as an exact quote of its answer. */
function tokenizeForMatch(text: string): string[] {
  return text
    .toLowerCase()
    .split(/[^\p{L}\p{N}]+/u)
    .filter((token) => token.length >= 2);
}

function matchPassages(
  query: string,
  collectionIds: string[] | undefined,
  limit: number,
): Passage[] {
  const queryTokens = tokenizeForMatch(query);
  const scoped = passageCatalog().filter(
    (p) => !collectionIds?.length || collectionIds.includes(p.collectionId),
  );
  // Only the current version of each document participates in plain
  // search/current-mode matching — compare/history reach the non-current one
  // through `resolveCompareOrHistoryPassages` instead, same as the real
  // server never surfacing a superseded version from ordinary retrieval.
  const current = scoped.filter((p) => p.isCurrent);
  const matched =
    queryTokens.length === 0
      ? current
      : current.filter((p) => {
          const passageTokens = new Set(tokenizeForMatch(`${p.title} ${p.quote}`));
          return queryTokens.some((token) => passageTokens.has(token));
        });
  return matched.slice(0, limit);
}

function citeIdFor(index: number): string {
  return `CITE-${String(index + 1).padStart(4, '0')}`;
}

function passageToCitation(passage: Passage, citeId: string): CitationPin {
  return {
    citeId,
    // P2-10 gap close — these three now mirror the real
    // `services::citation::CitationPin` shape (see `openapi.yaml`'s
    // `CitationPin` doc comment), matching the same `documentId`/
    // `collectionId`/`versionId` this mock's `passageCatalog()` already
    // carries for the corresponding `search` hit, so a citation and its
    // matching search hit deep-link to the exact same document/version.
    logicalDocumentId: passage.documentId,
    versionId: passage.versionId,
    collectionId: passage.collectionId,
    sourceContentSha256: `src-${passage.versionId}`,
    canonicalMarkdownSha256: `md-${passage.versionId}`,
    quoteSha256: `quote-${passage.versionId}`,
    chunkIdentitySha256: `chunk-${passage.versionId}`,
    quote: passage.quote,
    sourceSpanStart: 0,
    sourceSpanEnd: passage.quote.length,
    quoteLocalStart: 0,
    quoteLocalEnd: passage.quote.length,
    isCurrent: passage.isCurrent,
    anchor: `mhcite-${passage.versionNumber}.${passage.documentId.slice(-4)}`,
  };
}

/** `mode: 'compare'`/`'history'` resolve against the one seeded multi-version document, by explicit `versionA`/`versionB`/`documentId` — never inferred, matching the real server's "the caller names the exact versions" contract for these modes. */
function resolveCompareOrHistoryPassages(body: SearchRequest | AskRequest): {
  passages: Passage[];
  warnings: string[];
} {
  const all = passageCatalog();
  const warnings: string[] = [];
  if (body.mode === 'compare') {
    if (!body.versionA || !body.versionB) {
      return {
        passages: [],
        warnings: ['Chế độ so sánh cần chọn cả phiên bản A và phiên bản B.'],
      };
    }
    const a = all.find((p) => p.versionId === body.versionA);
    const b = all.find((p) => p.versionId === body.versionB);
    if (!a || !b) {
      return { passages: [], warnings: ['Không tìm thấy một trong hai phiên bản để so sánh.'] };
    }
    return { passages: [a, b], warnings };
  }
  if (body.mode === 'history') {
    if (!body.documentId) {
      return { passages: [], warnings: ['Chế độ lịch sử cần chọn một tài liệu.'] };
    }
    const versions = all.filter((p) => p.documentId === body.documentId);
    if (versions.length === 0) {
      return {
        passages: [],
        warnings: [`Không có phiên bản nào cho tài liệu ${body.documentId}.`],
      };
    }
    return { passages: versions, warnings };
  }
  if (body.mode === 'as_of') {
    if (!body.documentId || !body.asOf) {
      return { passages: [], warnings: ['Chế độ as-of cần chọn tài liệu và thời điểm (asOf).'] };
    }
    const store = getStore();
    const versions = store.versions.get(body.documentId) ?? [];
    const asOfMs = Date.parse(body.asOf);
    const effective = versions
      .filter((v) => Date.parse(v.effectiveFrom) <= asOfMs)
      .sort((x, y) => Date.parse(y.effectiveFrom) - Date.parse(x.effectiveFrom))[0];
    const passage = effective && all.find((p) => p.versionId === effective.id);
    if (!passage) {
      return { passages: [], warnings: [`Không có phiên bản nào hiệu lực tại ${body.asOf}.`] };
    }
    return { passages: [passage], warnings };
  }
  return { passages: [], warnings: [] };
}

function versionContextFor(
  body: SearchRequest | AskRequest,
  passages: Passage[],
): {
  mode: string;
  currentVersionIds: string[];
  citedVersionIds: string[];
  changeNote: string | null;
} {
  const mode = body.mode ?? 'current';
  const currentVersionIds = [
    ...new Set(
      passageCatalog()
        .filter((p) => p.isCurrent)
        .map((p) => p.versionId),
    ),
  ];
  const citedVersionIds = [...new Set(passages.map((p) => p.versionId))];
  let changeNote: string | null = null;
  if (mode === 'compare' && passages.length === 2) {
    const [a, b] = passages;
    changeNote = `So sánh phiên bản ${a.versionNumber} (${a.versionId}) với phiên bản ${b.versionNumber} (${b.versionId}).`;
  } else if (mode === 'history' && body.documentId) {
    changeNote = `Lịch sử phiên bản cho tài liệu ${body.documentId}.`;
  } else if (mode === 'as_of' && body.asOf) {
    changeNote = `Truy vấn as-of tại ${body.asOf}.`;
  }
  return { mode, currentVersionIds, citedVersionIds, changeNote };
}

/** Grounded, citation-tagged answer text — every factual sentence carries the `[CITE-n]` label the real grounding validator (`services/qa/grounding.rs`) requires. */
function buildAnswer(citations: { citeId: string; quote: string }[]): string {
  if (citations.length === 0) {
    return 'Không tìm thấy nội dung liên quan trong tài liệu đã lập chỉ mục để trả lời câu hỏi này.';
  }
  const clauses = citations.map((c) => `${c.quote} [${c.citeId}]`);
  return `Dựa trên tài liệu đã lập chỉ mục: ${clauses.join(' ')}`;
}

function currentModeWarnings(mode: string, citations: CitationPin[]): string[] {
  if (mode !== 'current') return [];
  return citations.some((c) => c.isCurrent === false)
    ? [
        'Câu trả lời (chế độ hiện hành) đang trích dẫn ít nhất một phiên bản không phải bản mới nhất; kiểm tra lại trước khi sử dụng.',
      ]
    : [];
}

// ---------------------------------------------------------------------------
// search / ask (synchronous, P2-02 baseline)
// ---------------------------------------------------------------------------

registerOperation('search', async (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const body = await ctx.json<SearchRequest>();
  const scope = resolveProjectScope(auth.orgId, body.projectId, body.collectionIds);
  if (!scope) return notFound(`Project ${body.projectId} does not exist in this org.`);
  const mode = body.mode ?? 'current';
  const limit = body.limit ?? 10;
  let passages: Passage[];
  let warnings: string[];
  if (mode === 'compare' || mode === 'history' || mode === 'as_of') {
    const resolved = resolveCompareOrHistoryPassages(body);
    passages = resolved.passages;
    warnings = resolved.warnings;
  } else {
    passages = matchPassages(body.query, scope.collectionIds, limit);
    warnings = [];
  }
  const citations = passages.map((p, i) => passageToCitation(p, citeIdFor(i)));
  warnings = [...warnings, ...currentModeWarnings(mode, citations)];
  const hits = passages.map((p, i) => ({
    citeId: citations[i].citeId,
    documentId: p.documentId,
    collectionId: p.collectionId,
    versionId: p.versionId,
    title: p.title,
    score: Number((1 - i * 0.1).toFixed(2)),
    snippet: p.quote,
  }));
  return {
    status: 200,
    body: { hits, citations, requestId: nextRequestId(), warnings },
  };
});

registerOperation('ask', async (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const body = await ctx.json<AskRequest>();
  const scope = resolveProjectScope(auth.orgId, body.projectId, body.collectionIds);
  if (!scope) return notFound(`Project ${body.projectId} does not exist in this org.`);
  const mode = body.mode ?? 'current';
  let passages: Passage[];
  let warnings: string[];
  if (mode === 'compare' || mode === 'history' || mode === 'as_of') {
    const resolved = resolveCompareOrHistoryPassages(body);
    passages = resolved.passages;
    warnings = resolved.warnings;
  } else {
    passages = matchPassages(body.question, scope.collectionIds, body.limit ?? 10);
    warnings = [];
  }
  const citations = passages.map((p, i) => passageToCitation(p, citeIdFor(i)));
  warnings = [...warnings, ...currentModeWarnings(mode, citations)];
  const versionContext = versionContextFor(body, passages);
  return {
    status: 200,
    body: {
      answer: buildAnswer(citations),
      mode,
      citations,
      warnings,
      versionContext,
      embeddingMode: 'mock_offline',
      requestId: nextRequestId(),
    },
  };
});

// ---------------------------------------------------------------------------
// /ask/stream
// ---------------------------------------------------------------------------

/** Splits an answer into whitespace-preserving tokens — a stand-in for the real `tokenize_answer` (`services/qa/stream.rs`); good enough to demonstrate a token-growing UI, not a claim about the real tokenizer's boundaries. */
function tokenizeAnswer(answer: string): string[] {
  const tokens = answer.match(/\S+\s*/g);
  return tokens ?? [answer];
}

interface SseFrame {
  id?: number;
  event: string;
  data: unknown;
}

function serializeSseFrames(frames: SseFrame[], requestId: string): string {
  return frames
    .map((frame) => {
      const envelope: SseEnvelope = {
        version: 1,
        sequence: frame.id ?? 0,
        event: frame.event,
        requestId,
        data: frame.data,
      };
      const idLine = frame.id !== undefined ? `id: ${frame.id}\n` : '';
      return `${idLine}event: ${frame.event}\ndata: ${JSON.stringify(envelope)}\n\n`;
    })
    .join('');
}

registerOperation('askStream', async (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const body = await ctx.json<AskRequest>();
  const scope = resolveProjectScope(auth.orgId, body.projectId, body.collectionIds);
  if (!scope) return notFound(`Project ${body.projectId} does not exist in this org.`);
  const rawQuestion = body.question;
  const revoke = rawQuestion.includes(QA_STREAM_MARKERS.citationRevoked);
  const fallback = rawQuestion.includes(QA_STREAM_MARKERS.providerFallback);
  const question = stripMarkers(rawQuestion);
  const mode = body.mode ?? 'current';

  let passages: Passage[];
  let scenarioWarnings: string[];
  if (mode === 'compare' || mode === 'history' || mode === 'as_of') {
    const resolved = resolveCompareOrHistoryPassages(body);
    passages = resolved.passages;
    scenarioWarnings = resolved.warnings;
  } else {
    passages = matchPassages(question, scope.collectionIds, body.limit ?? 10);
    scenarioWarnings = [];
  }
  const citations = passages.map((p, i) => passageToCitation(p, citeIdFor(i)));
  const answer = buildAnswer(citations);
  const tokens = tokenizeAnswer(answer);
  const versionContext = versionContextFor(body, passages);
  const warnings = [...scenarioWarnings, ...currentModeWarnings(mode, citations)];
  if (fallback) {
    warnings.push(
      'Nhà cung cấp LLM tạm thời không khả dụng; đã chuyển sang câu trả lời trích xuất.',
    );
  }

  const requestId = nextRequestId();
  const sessionId = nextId();
  const frames: SseFrame[] = [];
  let seq = 0;
  const next = () => {
    seq += 1;
    return seq;
  };

  frames.push({
    id: next(),
    event: 'ask.started',
    data: {
      streamSessionId: sessionId,
      mode: 'offline_extractive',
      embeddingMode: 'mock_offline',
      citationCount: citations.length,
    },
  });

  if (revoke) {
    // Real behaviour (`ask_stream.rs`'s `append()`/`close()`): the instant a
    // cited document/version becomes inaccessible mid-stream, the producer
    // stops appending tokens and closes durably with `citation_revoked` —
    // citations/version_context/completed never arrive. A couple of tokens
    // stream first so the UI has visibly started an answer before the close.
    for (const token of tokens.slice(0, 2)) {
      frames.push({ id: next(), event: 'ask.token', data: { text: token } });
    }
    frames.push({
      id: next(),
      event: 'stream.closed',
      data: { reason: 'citation_revoked' },
    });
    return {
      status: 200,
      rawBody: { text: serializeSseFrames(frames, requestId), contentType: 'text/event-stream' },
    };
  }

  for (const token of tokens) {
    frames.push({ id: next(), event: 'ask.token', data: { text: token } });
  }
  for (const warning of warnings) {
    frames.push({ id: next(), event: 'ask.warning', data: { message: warning } });
  }
  frames.push({ id: next(), event: 'ask.citations', data: { citations } });
  frames.push({ id: next(), event: 'ask.version_context', data: versionContext });
  frames.push({
    id: next(),
    event: 'ask.completed',
    data: {
      mode: fallback ? 'fallback_extractive' : 'offline_extractive',
      streamSessionId: sessionId,
    },
  });
  frames.push({ id: next(), event: 'stream.closed', data: { reason: 'completed' } });

  return {
    status: 200,
    rawBody: { text: serializeSseFrames(frames, requestId), contentType: 'text/event-stream' },
  };
});
