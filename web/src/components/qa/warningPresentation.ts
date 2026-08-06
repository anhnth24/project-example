// Splits ask-stream / chat-history `warnings` into what the chat UI should
// show as a short Vietnamese summary versus technical detail lines, and
// extracts the optional UAT discarded-LLM draft (server prefix below).
// Kept pure so ChatTurnBubble and HistoricalTurnBubble share one mapper.

/** Must stay in sync with `DISCARDED_LLM_DRAFT_WARNING_PREFIX` in `crates/server/src/services/qa/mod.rs`. */
export const DISCARDED_LLM_DRAFT_PREFIX = 'Discarded LLM draft (UAT):\n';

export interface WarningPresentation {
  readonly summary: string | null;
  readonly technicalDetails: string[];
  readonly discardedLlmDraft: string | null;
}

const TECHNICAL_HINTS = [
  /factual claim/i,
  /unverifiable/i,
  /fabricated citation/i,
  /claim cites unknown/i,
  /claim not supported/i,
  /negation\/contradiction/i,
  /date\/unit mismatch/i,
  /misplaced citation/i,
  /có đoạn trả lời dài không gắn citation/i,
  /structured entailment/i,
  /fail-closed/i,
  /grounding failed/i,
  /using extractive fallback/i,
  /dev-gate:/i,
  /not available — this answer is unverified/i,
  /llm không trả citation/i,
  /llm dùng citation không tồn tại/i,
];

function isTechnical(warning: string): boolean {
  return TECHNICAL_HINTS.some((pattern) => pattern.test(warning));
}

function summarize(warnings: readonly string[], hasDiscardedDraft: boolean): string | null {
  if (warnings.length === 0 && !hasDiscardedDraft) return null;

  const joined = warnings.join('\n');
  if (/timed out/i.test(joined)) {
    return 'Nhà cung cấp mô hình hết thời gian chờ; đang hiện các đoạn nguồn liên quan.';
  }
  if (/provider unavailable|không khả dụng/i.test(joined)) {
    return 'Nhà cung cấp mô hình tạm thời không khả dụng; đang hiện các đoạn nguồn liên quan.';
  }
  if (/no chat provider|no embedding/i.test(joined)) {
    return 'Chưa cấu hình đủ nhà cung cấp; đang hiện các đoạn nguồn liên quan.';
  }
  if (
    hasDiscardedDraft ||
    /unverifiable|grounding failed|extractive fallback|fail-closed|factual claim/i.test(joined)
  ) {
    return 'Câu trả lời từ mô hình không đạt kiểm chứng trích dẫn; đang hiện các đoạn nguồn liên quan.';
  }
  if (/dev-gate:|unverified/i.test(joined)) {
    return 'Câu trả lời từ mô hình chưa được kiểm chứng đầy đủ (entailment chưa sẵn sàng).';
  }
  // User-facing Vietnamese warnings (version conflict, etc.) — keep the first as summary.
  const userFacing = warnings.find((w) => !isTechnical(w));
  return userFacing ?? 'Có cảnh báo kèm câu trả lời này.';
}

export function presentWarnings(warnings: readonly string[]): WarningPresentation {
  let discardedLlmDraft: string | null = null;
  const remainder: string[] = [];
  for (const warning of warnings) {
    if (warning.startsWith(DISCARDED_LLM_DRAFT_PREFIX)) {
      discardedLlmDraft = warning.slice(DISCARDED_LLM_DRAFT_PREFIX.length);
      continue;
    }
    remainder.push(warning);
  }

  const technicalDetails = remainder.filter(isTechnical);
  const summary = summarize(remainder, discardedLlmDraft !== null);

  // Non-technical warnings that weren't used as the summary still belong in details
  // so nothing is silently dropped.
  const extraDetails = remainder.filter((w) => !isTechnical(w) && w !== summary);
  return {
    summary,
    technicalDetails: [...technicalDetails, ...extraDetails],
    discardedLlmDraft,
  };
}
