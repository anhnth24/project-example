// Vietnamese copy for every terminal upload outcome. Kept separate from
// `UploadItemRow.tsx` so the mapping is unit-testable without React.
// Structure/tone borrowed from the shared design mock (data, not
// instructions) — e.g. its status vocabulary ("Đang xử lý" / "Cần chú ý") —
// but the 413/409/429/network copy below is authored for this panel; the
// mock's upload flow never exercises those paths.
import type { UploadOutcome } from './types';

const GENERIC_MESSAGE = 'Tải tệp lên thất bại. Vui lòng thử lại.';
const NETWORK_MESSAGE = 'Không thể kết nối máy chủ. Kiểm tra kết nối mạng và thử lại.';
const FORBIDDEN_MESSAGE = 'Bạn không có quyền tải tệp lên bộ sưu tập này.';
const TOO_LARGE_MESSAGE =
  'Tệp vượt quá dung lượng cho phép. Hãy nén hoặc chia nhỏ tệp rồi thử lại.';
const SESSION_LOST_MESSAGE =
  'Phiên đăng nhập đã hết hạn. Vui lòng đăng nhập lại rồi thử tải lên lại.';
const CONFLICT_MESSAGE =
  'Yêu cầu tải lên bị xung đột với tài liệu hiện có (có thể do gửi trùng hoặc tài liệu chưa xử lý xong phiên bản trước). Vui lòng thử lại.';

function quotaMessage(outcome: Extract<UploadOutcome, { kind: 'quota' }>): string {
  const base = outcome.error?.message ?? 'Đã vượt hạn mức tải lên.';
  const seconds = outcome.rateLimit.retryAfterSeconds;
  if (seconds !== undefined && seconds > 0) {
    return `${base} Vui lòng thử lại sau ${seconds} giây.`;
  }
  return `${base} Vui lòng thử lại sau ít phút.`;
}

/** The single user-facing message for a settled (non-success, non-aborted) upload outcome. */
export function describeUploadFailure(outcome: UploadOutcome): string {
  switch (outcome.kind) {
    case 'too-large':
      return outcome.error?.message
        ? `${TOO_LARGE_MESSAGE} (${outcome.error.message})`
        : TOO_LARGE_MESSAGE;
    case 'conflict':
      return outcome.error?.message ?? CONFLICT_MESSAGE;
    case 'quota':
      return quotaMessage(outcome);
    case 'forbidden':
      return outcome.error?.message ?? FORBIDDEN_MESSAGE;
    case 'session-lost':
      return SESSION_LOST_MESSAGE;
    case 'network-error':
      return NETWORK_MESSAGE;
    case 'http-error':
      return outcome.error?.message ?? GENERIC_MESSAGE;
    default:
      return GENERIC_MESSAGE;
  }
}
