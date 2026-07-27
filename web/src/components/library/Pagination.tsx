// Cursor-based pager (plan P2.4: "Pagination must round-trip the real
// PageInfo cursor — do not invent offset paging"). This component itself is
// dumb — `pageNumber`/`hasMore` and the prev/next callbacks are entirely
// owned by `LibraryPage`, which is the part that actually tracks the
// `PageInfo.nextCursor` history.
export function Pagination({
  pageNumber,
  hasMore,
  onPrev,
  onNext,
}: {
  /** 1-based page number of the currently displayed page. */
  pageNumber: number;
  hasMore: boolean;
  onPrev: () => void;
  onNext: () => void;
}) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--space-3)' }}>
      <button
        type="button"
        className="btn btn-secondary btn-sm"
        onClick={onPrev}
        disabled={pageNumber <= 1}
      >
        Trang trước
      </button>
      <span className="text-muted" aria-live="polite">
        Trang {pageNumber}
      </span>
      <button
        type="button"
        className="btn btn-secondary btn-sm"
        onClick={onNext}
        disabled={!hasMore}
      >
        Trang sau
      </button>
    </div>
  );
}
