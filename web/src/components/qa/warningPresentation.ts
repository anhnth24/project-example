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
  /lacked citations; retried once/i,
  /using extractive fallback/i,
  /dev-gate:/i,
  /not available — this answer is unverified/i,
  /llm không trả citation/i,
  /llm dùng citation không tồn tại/i,
  /embedding timed out/i,
  /using FTS-only retrieval/i,
];

function isTechnical(warning: string): boolean {
  return TECHNICAL_HINTS.some((pattern) => pattern.test(warning));
}

// Server warnings arrive as fixed English strings (crates/server/src/services/
// qa/{mod,grounding}.rs and retrieval/mod.rs). Classification above runs on the
// raw strings; this table only rewrites them to Vietnamese for display, so a
// new/unknown server string falls through untranslated instead of being hidden.
const WARNING_TRANSLATIONS: ReadonlyArray<[RegExp, string]> = [
  [
    /^Removed (\d+) unverifiable sentence\(s\) from LLM draft; remainder passed claim checks\.$/,
    'Đã loại $1 câu không kiểm chứng được khỏi bản nháp của mô hình; phần còn lại đã vượt qua kiểm tra trích dẫn.',
  ],
  [
    /^Dev-gate: LLM answer passed citation\/claim checks but structured entailment is NOT available — this answer is unverified, not grounded\.$/,
    'Chế độ thử nghiệm: câu trả lời của mô hình đã qua kiểm tra trích dẫn, nhưng bộ kiểm chứng suy diễn (structured entailment) chưa sẵn sàng — câu trả lời này chưa được xác minh đầy đủ.',
  ],
  [
    /^Structured entailment unavailable; fail-closed extractive-only grounding\.$/,
    'Bộ kiểm chứng suy diễn chưa sẵn sàng; hệ thống chỉ trả lời dạng trích xuất để bảo đảm an toàn.',
  ],
  [
    /^LLM draft lacked citations; retried once with a citation reminder\.$/,
    'Bản nháp của mô hình thiếu trích dẫn; hệ thống đã nhắc lại một lần để bổ sung trích dẫn.',
  ],
  [
    /^Auto-attached citations to (\d+) sentence\(s\); full draft passed claim checks\.$/,
    'Hệ thống đã tự gắn trích dẫn cho $1 câu; toàn bộ câu trả lời đã vượt qua kiểm tra trích dẫn.',
  ],
  [
    /^LLM provider timed out; using extractive fallback\.$/,
    'Nhà cung cấp mô hình hết thời gian chờ; chuyển sang trả lời trích xuất.',
  ],
  [
    /^LLM provider unavailable; using extractive fallback\.$/,
    'Nhà cung cấp mô hình không khả dụng; chuyển sang trả lời trích xuất.',
  ],
  [
    /^No chat provider configured; using extractive answer\.$/,
    'Chưa cấu hình nhà cung cấp mô hình; đang dùng trả lời trích xuất.',
  ],
  [
    /^LLM provider timed out; using offline assistant reply\.$/,
    'Nhà cung cấp mô hình hết thời gian chờ; đang dùng trả lời ngoại tuyến.',
  ],
  [
    /^No chat provider configured; using offline assistant reply\.$/,
    'Chưa cấu hình nhà cung cấp mô hình; đang dùng trả lời ngoại tuyến.',
  ],
  [
    /^Unverifiable claim-level grounding; using extractive fallback\.$/,
    'Không kiểm chứng được các khẳng định trong câu trả lời của mô hình; chuyển sang trả lời trích xuất.',
  ],
  [
    /^LLM grounding failed validation; using extractive fallback\.$/,
    'Câu trả lời của mô hình không đạt kiểm tra trích dẫn; chuyển sang trả lời trích xuất.',
  ],
  [
    /^Embedding provider error; using FTS-only retrieval\.$/,
    'Lỗi dịch vụ embedding; đang tìm kiếm theo từ khóa.',
  ],
  [
    /^Embedding recovered after one retry \(transient provider error\)\.$/,
    'Dịch vụ embedding phục hồi sau một lần thử lại (lỗi thoáng qua).',
  ],
  [
    /^Embedding timed out; using FTS-only retrieval\.$/,
    'Embedding hết thời gian chờ; đang tìm kiếm theo từ khóa.',
  ],
  [
    /^No embedding runtime configured; using FTS-only retrieval\.$/,
    'Chưa cấu hình embedding; đang tìm kiếm theo từ khóa.',
  ],
  [
    /^FTS leg unavailable; continuing with vector-only retrieval\.$/,
    'Tìm kiếm từ khóa không khả dụng; tiếp tục với tìm kiếm theo nghĩa.',
  ],
  [
    /^Vector leg unavailable; continuing with FTS-only retrieval\.$/,
    'Tìm kiếm theo nghĩa không khả dụng; tiếp tục với tìm kiếm từ khóa.',
  ],
  [
    /^Factual claim lacks citation; unverifiable: (.*)$/,
    'Câu khẳng định thiếu trích dẫn nên không kiểm chứng được: $1',
  ],
  [
    /^Claim cites unknown (.+); unverifiable\.$/,
    'Khẳng định trích dẫn nguồn không tồn tại ($1); không kiểm chứng được.',
  ],
  [
    /^Claim not supported by passage\/span of (.+); unverifiable\.$/,
    'Khẳng định không được đoạn nguồn $1 hỗ trợ; không kiểm chứng được.',
  ],
  [
    /^Claim negation\/contradiction vs (.+); unverifiable\.$/,
    'Khẳng định mâu thuẫn với đoạn nguồn $1; không kiểm chứng được.',
  ],
  [
    /^Claim date\/unit mismatch vs (.+); unverifiable\.$/,
    'Ngày tháng/đơn vị trong khẳng định lệch với đoạn nguồn $1; không kiểm chứng được.',
  ],
  [
    /^Misplaced citation (.+); passage subject mismatch\.$/,
    'Trích dẫn $1 đặt sai chỗ; đoạn nguồn nói về chủ đề khác.',
  ],
  [/^Fabricated citation id: (.+)$/, 'Mã trích dẫn không tồn tại: $1'],
  [
    /^Qualitative factual answer without citations; unverifiable\.$/,
    'Câu trả lời khẳng định thông tin nhưng không có trích dẫn; không kiểm chứng được.',
  ],
  [
    /^Current answer cited non-current version via (.+)$/,
    'Câu trả lời cho phiên bản hiện hành lại trích dẫn phiên bản cũ qua $1.',
  ],
  [
    /^Compare answer must cite both old and new versions in the lineage\.$/,
    'Câu trả lời so sánh phải trích dẫn cả phiên bản cũ và phiên bản mới.',
  ],
  [
    /^Wrong compare delta: newer value attributed to older citation (.+)$/,
    'Sai lệch khi so sánh: giá trị của bản mới bị gán cho trích dẫn bản cũ $1.',
  ],
];

export function translateWarning(warning: string): string {
  for (const [pattern, replacement] of WARNING_TRANSLATIONS) {
    if (pattern.test(warning)) return warning.replace(pattern, replacement);
  }
  return warning;
}

function summarize(warnings: readonly string[], hasDiscardedDraft: boolean): string | null {
  if (warnings.length === 0 && !hasDiscardedDraft) return null;

  const joined = warnings.join('\n');
  if (/embedding timed out/i.test(joined)) {
    return 'Tìm kiếm theo nghĩa chưa kịp; đang lấy đoạn khớp từ khóa trong tài liệu.';
  }
  if (/timed out/i.test(joined)) {
    return 'Nhà cung cấp mô hình hết thời gian chờ; đang hiện các đoạn nguồn liên quan.';
  }
  if (/provider unavailable|không khả dụng/i.test(joined)) {
    return 'Nhà cung cấp mô hình tạm thời không khả dụng; đang hiện các đoạn nguồn liên quan.';
  }
  if (/no chat provider|no embedding/i.test(joined)) {
    return 'Chưa cấu hình đủ nhà cung cấp; đang hiện các đoạn nguồn liên quan.';
  }
  // Default extractive-only (no LLM was attempted) is already labelled by the
  // mode badge — do not tell the user the model "failed citation checks".
  if (isFailClosedOnly(warnings) && !hasDiscardedDraft) {
    return null;
  }
  if (
    hasDiscardedDraft ||
    /unverifiable|grounding failed|extractive fallback|factual claim/i.test(joined)
  ) {
    return 'Câu trả lời từ mô hình không đạt kiểm chứng trích dẫn; đang hiện các đoạn nguồn liên quan.';
  }
  if (/dev-gate:|unverified/i.test(joined)) {
    return 'Câu trả lời từ mô hình chưa được kiểm chứng đầy đủ (entailment chưa sẵn sàng).';
  }
  const userFacing = warnings.find((w) => !isTechnical(w));
  return userFacing ?? 'Có cảnh báo kèm câu trả lời này.';
}

function isFailClosedOnly(warnings: readonly string[]): boolean {
  return (
    warnings.length > 0 &&
    warnings.every((warning) =>
      /structured entailment unavailable|fail-closed extractive-only/i.test(warning),
    )
  );
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

  if (isFailClosedOnly(remainder) && discardedLlmDraft === null) {
    return { summary: null, technicalDetails: [], discardedLlmDraft: null };
  }

  // Non-technical warnings that weren't used as the summary still belong in details
  // so nothing is silently dropped.
  const extraDetails = remainder.filter((w) => !isTechnical(w) && w !== summary);
  return {
    // Crafted summaries are already Vietnamese; translate only covers the case
    // where a raw server warning was promoted to summary verbatim.
    summary: summary === null ? null : translateWarning(summary),
    technicalDetails: [...technicalDetails, ...extraDetails].map(translateWarning),
    discardedLlmDraft,
  };
}
