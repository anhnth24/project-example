// P2-12: renders `GET /usage`'s per-resource snapshot as a grid of cards,
// each with a meter bar. Reuses the upload panel's existing progress-track
// classes (`.upload-progress-track`/`-value`) rather than inventing new CSS
// for what is, structurally, the same "filled fraction of a whole" bar —
// see styles.css's own note against adding classes a feature could reuse.
import { USAGE_RESOURCE_LABEL, formatUsageValue, usageFraction } from './memberPresentation';
import type { UsageEntry } from './types';

function UsageCard({ entry }: { entry: UsageEntry }) {
  const fraction = usageFraction(entry.committed, entry.reserved, entry.limit);
  const percentLabel = `${Math.round(fraction * 100)}%`;
  return (
    <div className="card" style={{ minWidth: 0 }}>
      <p className="card-kicker">{USAGE_RESOURCE_LABEL[entry.resource]}</p>
      <p className="card-title" style={{ margin: 0 }}>
        {formatUsageValue(entry.resource, entry.committed + entry.reserved)}
        <span className="text-muted"> / {formatUsageValue(entry.resource, entry.limit)}</span>
      </p>
      <span
        className="upload-progress-track"
        role="progressbar"
        aria-label={`Đã dùng ${USAGE_RESOURCE_LABEL[entry.resource]}`}
        aria-valuenow={Math.round(fraction * 100)}
        aria-valuemin={0}
        aria-valuemax={100}
      >
        <span className="upload-progress-value" style={{ width: percentLabel }} />
      </span>
      <p className="card-meta" style={{ margin: 0 }}>
        Đã dùng {percentLabel} — còn lại {formatUsageValue(entry.resource, entry.remaining)}
        {entry.reserved > 0 && (
          <> (trong đó {formatUsageValue(entry.resource, entry.reserved)} đang được giữ chỗ)</>
        )}
      </p>
    </div>
  );
}

export function UsageCards({ items, loading }: { items: UsageEntry[]; loading: boolean }) {
  if (loading) {
    return <p className="text-muted">Đang tải dữ liệu sử dụng…</p>;
  }
  if (items.length === 0) {
    return <p className="text-muted">Chưa có dữ liệu sử dụng.</p>;
  }

  return (
    <div
      style={{
        display: 'grid',
        gridTemplateColumns: 'repeat(auto-fill, minmax(min(220px, 100%), 1fr))',
        gap: 'var(--space-4)',
        minWidth: 0,
        width: '100%',
      }}
    >
      {items.map((entry) => (
        <UsageCard key={entry.resource} entry={entry} />
      ))}
    </div>
  );
}
