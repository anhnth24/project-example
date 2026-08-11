// Pure presentation helpers for the library UI: how a `Document.state` reads
// (label + Organic tag/spinner combo), how timestamps and error causes are
// rendered in Vietnamese, and a diacritic-insensitive title search. Kept
// side-effect-free and framework-free so each piece is trivial to unit test
// on its own.
import { HttpApiError, NetworkError } from '../../api/errors';
import type { DocumentState } from './types';

export interface DocumentStateMeta {
  label: string;
  /** One of styles.css's existing `.tag-*` classes — never a new class. */
  tagClass: 'tag-neutral' | 'tag-accent' | 'tag-accent-2' | 'tag-outline' | 'tag-danger';
  /** Whether the badge shows the existing `.spin`-animated SpinnerIcon. */
  spinning: boolean;
}

/**
 * One entry per `Document.state` value from the contract's enum. The six
 * states plan P2.4 requires (`uploaded|converting|converted|indexing|
 * indexed|failed`) are each given a unique `(tagClass, spinning)` pair —
 * verified below the object literal — so "renders distinctly" holds even
 * for someone who can't read the Vietnamese label, not just via text.
 * `tombstoned`/`purged` (soft-delete states the schema allows but this list
 * view does not expect to normally show) get honest fallback labels rather
 * than being silently mis-rendered as something else.
 */
export const DOCUMENT_STATE_META: Record<DocumentState, DocumentStateMeta> = {
  uploaded: { label: 'Đã tải lên', tagClass: 'tag-neutral', spinning: false },
  converting: { label: 'Đang chuyển đổi', tagClass: 'tag-accent', spinning: true },
  converted: { label: 'Đã chuyển đổi', tagClass: 'tag-accent', spinning: false },
  indexing: { label: 'Đang lập chỉ mục', tagClass: 'tag-outline', spinning: true },
  indexed: { label: 'Đã lập chỉ mục', tagClass: 'tag-accent-2', spinning: false },
  failed: { label: 'Lỗi chuyển đổi', tagClass: 'tag-danger', spinning: false },
  tombstoned: { label: 'Đã xoá', tagClass: 'tag-neutral', spinning: false },
  purged: { label: 'Đã xoá vĩnh viễn', tagClass: 'tag-neutral', spinning: false },
};

/**
 * P2-08 gap close (live status polling, owner critique 2026-07-29): the
 * states a document is still mid-pipeline in, server-side — anything not in
 * this set is a resting point the worker will never move on from by itself
 * (`indexed`/`failed`) or a soft-delete state (`tombstoned`/`purged`), so
 * there is nothing left to poll for once every document on a page is one of
 * those.
 */
const NON_TERMINAL_DOCUMENT_STATES: ReadonlySet<DocumentState> = new Set([
  'uploaded',
  'converting',
  'converted',
  'indexing',
]);

export function isNonTerminalState(state: DocumentState): boolean {
  return NON_TERMINAL_DOCUMENT_STATES.has(state);
}

const dateTimeFormatter = new Intl.DateTimeFormat('vi-VN', {
  dateStyle: 'medium',
  timeStyle: 'short',
});

/** Absolute (never relative-to-now, so it's deterministic and testable) Vietnamese date/time. */
export function formatDateTime(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : dateTimeFormatter.format(date);
}

/**
 * Best-effort file-type chip derived from the document's own `title` (the
 * only field the contract's `Document` schema carries — there is no
 * separate format/size field to show, unlike the interactive prototype's
 * mock rows). Purely a presentational extraction of data already present,
 * not a fabricated fact.
 */
export function extensionLabel(title: string): string | null {
  const match = /\.([a-zA-Z0-9]{1,6})$/.exec(title.trim());
  return match ? match[1].toUpperCase() : null;
}

function foldForSearch(value: string): string {
  return value
    .normalize('NFD')
    .replace(/\p{Diacritic}/gu, '')
    .toLocaleLowerCase();
}

/** Diacritic- and case-insensitive substring match; an empty query matches everything. */
export function matchesQuery(title: string, query: string): boolean {
  const needle = foldForSearch(query.trim());
  return needle === '' || foldForSearch(title).includes(needle);
}

/** Vietnamese, user-facing message for an error thrown by `ApiClient.request`. */
export function describeApiError(cause: unknown): string {
  if (cause instanceof HttpApiError) {
    if (cause.status === 403) return 'Bạn không có quyền truy cập nội dung này.';
    if (cause.status === 404) return 'Không tìm thấy bộ sưu tập hoặc tài liệu này.';
    if (cause.status === 429) return 'Quá nhiều yêu cầu. Vui lòng thử lại sau ít phút.';
    return `Máy chủ báo lỗi (${cause.status}): ${cause.message}`;
  }
  if (cause instanceof NetworkError) {
    return 'Không thể kết nối máy chủ. Kiểm tra kết nối mạng và thử lại.';
  }
  return 'Không thể tải dữ liệu lúc này. Vui lòng thử lại.';
}
