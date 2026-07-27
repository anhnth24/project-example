// Vietnamese, user-facing error copy for the three mutations this component
// performs (download's capability-issue step, reindex, delete). Every action
// needs its own 403/429 message per plans/markhand-web/phase-2-web-spa.md
// §P2.4; this is the single place that mapping lives so the three call sites
// in `DocumentRowActions.tsx` stay consistent instead of hand-rolling copy.
//
// Ground truth on which actions can *actually* 403 today, read from
// `web/src/api/generated/contract.ts` (generated from the server's OpenAPI
// doc): `issueDownloadCapability` declares 403/404/429. `reindexDocument`
// declares only 429 — no 403, no 404. `deleteDocument` declares 404/429 — no
// 403. So a permission-denied reindex/delete is not a documented response
// today; the mapping below still handles an arbitrary status defensively
// (a real server sending one anyway shouldn't crash the row), but the 403
// copy for reindex/delete is currently unreachable through the contract as
// written — flagged in the P2-09 report rather than silently assumed fixed.
import { HttpApiError, NetworkError } from '../../api/errors';

export type RowAction = 'download' | 'reindex' | 'delete';

function formatRetryAfter(seconds: number | undefined): string {
  if (seconds === undefined || seconds <= 0) {
    return 'Quá nhiều yêu cầu. Vui lòng thử lại sau ít phút.';
  }
  if (seconds < 60) {
    return `Quá nhiều yêu cầu. Vui lòng thử lại sau ${seconds} giây.`;
  }
  const minutes = Math.ceil(seconds / 60);
  return `Quá nhiều yêu cầu. Vui lòng thử lại sau khoảng ${minutes} phút.`;
}

/** Maps a thrown error from any of the three mutations to a Vietnamese message for the row. */
export function describeActionError(error: unknown, action: RowAction): string {
  if (error instanceof HttpApiError) {
    switch (error.status) {
      case 403:
        return 'Bạn không có quyền thực hiện thao tác này với tài liệu này.';
      case 429:
        return formatRetryAfter(error.rateLimit?.retryAfterSeconds);
      case 404:
        return action === 'download'
          ? 'Liên kết tải xuống đã hết hạn hoặc đã được dùng. Vui lòng thử tải lại.'
          : 'Tài liệu không còn tồn tại — danh sách có thể đã lỗi thời, hãy tải lại trang.';
      case 409:
        return 'Tài liệu đang ở trạng thái xung đột với thao tác này. Vui lòng tải lại trang và thử lại.';
      default:
        if (error.status >= 500) {
          return 'Máy chủ đang gặp sự cố. Vui lòng thử lại sau.';
        }
        return error.message || 'Không thể hoàn tất thao tác. Vui lòng thử lại.';
    }
  }
  if (error instanceof NetworkError) {
    return 'Không thể kết nối máy chủ. Kiểm tra kết nối mạng và thử lại.';
  }
  return 'Không thể hoàn tất thao tác. Vui lòng thử lại.';
}
