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
// gets no badge at all, same as the pre-chat `AskPanel` only ever showing
// one for `fallback_extractive`.
export interface AnswerModeInfo {
  readonly label: string;
  readonly tone: 'neutral' | 'warning';
}

const ANSWER_MODE_LABELS: Record<string, AnswerModeInfo> = {
  fallback_extractive: {
    label: 'Trả lời trích xuất (không qua LLM)',
    tone: 'neutral',
  },
  llm_unverified: {
    label: 'Trả lời từ LLM (chưa kiểm chứng đối chiếu)',
    tone: 'warning',
  },
};

export function describeAnswerMode(mode: string | undefined): AnswerModeInfo | null {
  if (!mode) return null;
  return ANSWER_MODE_LABELS[mode] ?? null;
}
