// Pure presentation helpers for the admin members/usage UI: how a role/state/
// invite-status reads (label + one of styles.css's existing `.tag-*`
// classes — never a new class, same rule `DocumentStateBadge`'s doc follows),
// and the owner-tier decision the UI must make *before* ever calling the API
// (never render a control that would just 403). Kept side-effect-free and
// framework-free so each piece is trivial to unit test on its own.
import { HttpApiError, NetworkError } from '../../api/errors';
import type {
  Invite,
  InviteStatus,
  Membership,
  MembershipRole,
  MembershipState,
  UsageResource,
} from './types';

export interface TagMeta {
  label: string;
  tagClass: 'tag-neutral' | 'tag-accent' | 'tag-accent-2' | 'tag-outline';
}

/** Every role the `Membership`/`Invite`/`PatchMemberRequest` schemas declare, in privilege order (most privileged first) — the order the role `<select>` presents them in. */
export const ROLE_ORDER: readonly MembershipRole[] = ['owner', 'admin', 'editor', 'viewer'];

export const ROLE_META: Record<MembershipRole, TagMeta> = {
  owner: { label: 'Chủ sở hữu', tagClass: 'tag-accent' },
  admin: { label: 'Quản trị viên', tagClass: 'tag-accent-2' },
  editor: { label: 'Biên tập viên', tagClass: 'tag-outline' },
  viewer: { label: 'Người xem', tagClass: 'tag-neutral' },
};

export const STATE_META: Record<MembershipState, TagMeta> = {
  active: { label: 'Đang hoạt động', tagClass: 'tag-accent-2' },
  suspended: { label: 'Đã tạm ngưng', tagClass: 'tag-outline' },
};

export const INVITE_STATUS_META: Record<InviteStatus, TagMeta> = {
  pending: { label: 'Đang chờ', tagClass: 'tag-accent-2' },
  accepted: { label: 'Đã chấp nhận', tagClass: 'tag-accent' },
  revoked: { label: 'Đã thu hồi', tagClass: 'tag-neutral' },
  expired: { label: 'Đã hết hạn', tagClass: 'tag-outline' },
};

export const USAGE_RESOURCE_LABEL: Record<UsageResource, string> = {
  storage_bytes: 'Dung lượng lưu trữ',
  documents: 'Số tài liệu',
  concurrent_jobs: 'Tác vụ đồng thời',
  tokens: 'Token (LLM)',
};

const dateTimeFormatter = new Intl.DateTimeFormat('vi-VN', {
  dateStyle: 'medium',
  timeStyle: 'short',
});

/** Absolute (never relative-to-now) Vietnamese date/time — same convention as `components/library/documentPresentation.ts`. */
export function formatDateTime(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : dateTimeFormatter.format(date);
}

/**
 * Mirrors `services::members::operation_manages_owner` on the server exactly
 * (see `crates/server/src/services/members.rs`): true when the operation
 * would grant the owner role, or when its target currently holds it — either
 * way the caller must themselves be an active owner or the server 403s. Pure
 * and server-mirroring on purpose so the UI's "hide it before it 403s" rule
 * and the server's actual guard can never silently drift apart.
 */
export function operationManagesOwner(
  targetCurrentRole: MembershipRole,
  grantsOwner: boolean,
): boolean {
  return grantsOwner || targetCurrentRole === 'owner';
}

/** Whether `caller` may perform an owner-tier operation right now — an active owner membership only. Fails closed (false) if `caller` is `null`/`undefined` (own membership not found/not yet loaded). */
export function isActiveOwner(caller: Membership | null | undefined): boolean {
  return !!caller && caller.role === 'owner' && caller.state === 'active';
}

/** Formats bytes as a compact human-readable size for the storage_bytes usage row. Binary (1024-based), matching how storage is actually measured. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes)) return String(bytes);
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let unitIndex = 0;
  while (Math.abs(value) >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  const decimals = unitIndex === 0 ? 0 : 1;
  return `${value.toFixed(decimals)} ${units[unitIndex]}`;
}

/** Formats a plain integer count with locale thousands separators (documents/jobs/tokens usage rows). */
export function formatCount(value: number): string {
  return new Intl.NumberFormat('vi-VN').format(value);
}

export function formatUsageValue(resource: UsageResource, value: number): string {
  return resource === 'storage_bytes' ? formatBytes(value) : formatCount(value);
}

/** Fraction of `limit` already committed+reserved, clamped to [0, 1] for the meter bar (a limit of 0 reads as fully consumed rather than dividing by zero). */
export function usageFraction(committed: number, reserved: number, limit: number): number {
  if (limit <= 0) return committed + reserved > 0 ? 1 : 0;
  return Math.min(1, Math.max(0, (committed + reserved) / limit));
}

/** Vietnamese, user-facing message for an error thrown while reading members/invites/usage (not a mutation — see `describeMemberActionError` for those). */
export function describeMemberReadError(cause: unknown): string {
  if (cause instanceof HttpApiError) {
    if (cause.status === 403) {
      return 'Bạn không có quyền quản trị thành viên (cần quyền member.manage).';
    }
    if (cause.status === 429) return 'Quá nhiều yêu cầu. Vui lòng thử lại sau ít phút.';
    return `Máy chủ báo lỗi (${cause.status}): ${cause.message}`;
  }
  if (cause instanceof NetworkError) {
    return 'Không thể kết nối máy chủ. Kiểm tra kết nối mạng và thử lại.';
  }
  return 'Không thể tải dữ liệu lúc này. Vui lòng thử lại.';
}

function formatRetryAfter(seconds: number | undefined): string {
  if (seconds === undefined || seconds <= 0) {
    return 'Quá nhiều yêu cầu. Vui lòng thử lại sau ít phút.';
  }
  if (seconds < 60) return `Quá nhiều yêu cầu. Vui lòng thử lại sau ${seconds} giây.`;
  const minutes = Math.ceil(seconds / 60);
  return `Quá nhiều yêu cầu. Vui lòng thử lại sau khoảng ${minutes} phút.`;
}

/**
 * Vietnamese, user-facing message for a thrown member/invite mutation error.
 * `ownerTier` marks a mutation this client already knows touches the owner
 * tier (see `operationManagesOwner`) — a 403 here gets the specific
 * "chỉ chủ sở hữu" copy the task asks for instead of a generic permission
 * message, since the server's `forbidden` code/message is identical for both
 * causes (see `crates/server/src/routes/members.rs`'s `RouteError::Denied`
 * arm — always `"Permission denied"`) and cannot be told apart from the
 * response alone.
 */
export function describeMemberActionError(cause: unknown, ownerTier: boolean): string {
  if (cause instanceof HttpApiError) {
    if (cause.status === 403) {
      return ownerTier
        ? 'Chỉ chủ sở hữu (owner) đang hoạt động mới có thể cấp hoặc quản lý vai trò chủ sở hữu.'
        : 'Bạn không có quyền thực hiện thao tác này (cần quyền member.manage).';
    }
    if (cause.status === 404) {
      return 'Thành viên hoặc lời mời này không còn tồn tại. Danh sách sẽ được tải lại.';
    }
    if (cause.status === 409) {
      switch (cause.code) {
        case 'last_owner':
          return 'Không thể thực hiện: tổ chức phải luôn còn ít nhất một chủ sở hữu (owner) đang hoạt động.';
        case 'already_member':
          return 'Người này đã là thành viên của tổ chức.';
        case 'invite_terminal':
          return 'Lời mời này đã được chấp nhận hoặc thu hồi trước đó.';
        case 'invite_expired':
          return 'Lời mời này đã hết hạn.';
        default:
          return 'Yêu cầu xung đột với trạng thái hiện tại. Vui lòng tải lại trang và thử lại.';
      }
    }
    if (cause.status === 429) return formatRetryAfter(cause.rateLimit?.retryAfterSeconds);
    if (cause.status === 400) return cause.message || 'Dữ liệu không hợp lệ.';
    if (cause.status >= 500) return 'Máy chủ đang gặp sự cố. Vui lòng thử lại sau.';
    return cause.message || 'Không thể hoàn tất thao tác. Vui lòng thử lại.';
  }
  if (cause instanceof NetworkError) {
    return 'Không thể kết nối máy chủ. Kiểm tra kết nối mạng và thử lại.';
  }
  return 'Không thể hoàn tất thao tác. Vui lòng thử lại.';
}

/** True for any `Invite` whose token is still capable of being accepted (mirrors `revokeMemberInvite`'s own "already terminal" 409 condition, so the revoke button never offers an action the server would just reject). */
export function inviteIsRevocable(invite: Invite): boolean {
  return invite.status === 'pending';
}
