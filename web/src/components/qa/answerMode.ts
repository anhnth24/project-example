// Maps the server's own `AnswerMode` wire string (`ask.started`/`ask.completed`'s
// `mode` field in `state/askStream.ts` — displayed, never re-derived
// client-side) to Vietnamese display copy for the chat UI.
//
// Deliberately keyed by a plain string, not the generated
// `api/generated/contract.ts` enum: the server owns this vocabulary and may
// ship a new wire string (e.g. `llm_unverified`) ahead of a contract
// regeneration here — display copy must degrade gracefully, never fail the
// type-check, when the enum grows.
// An unrecognized mode (including today's default `offline_extractive`)
// gets a neutral "đoạn nguồn" badge when the answer came from extractive
// paths that previously showed nothing.
export interface AnswerModeInfo {
  readonly label: string;
  readonly tone: 'neutral' | 'warning';
}

const ANSWER_MODE_LABELS: Record<string, AnswerModeInfo> = {
  offline_extractive: {
    label: 'Đoạn nguồn liên quan',
    tone: 'neutral',
  },
  fallback_extractive: {
    label: 'Dùng đoạn nguồn (câu mô hình không đạt kiểm chứng)',
    tone: 'neutral',
  },
  llm_unverified: {
    label: 'Trả lời từ mô hình (chưa kiểm chứng đối chiếu)',
    tone: 'warning',
  },
  assistant: {
    label: 'Trợ lý',
    tone: 'neutral',
  },
  local_llm: {
    label: 'Trả lời từ mô hình cục bộ',
    tone: 'neutral',
  },
  cloud_llm: {
    label: 'Trả lời từ mô hình đám mây',
    tone: 'neutral',
  },
  subscription_cli: {
    label: 'Trả lời từ CLI đăng ký',
    tone: 'neutral',
  },
};

export function describeAnswerMode(mode: string | undefined): AnswerModeInfo | null {
  if (!mode) return null;
  return ANSWER_MODE_LABELS[mode] ?? null;
}
