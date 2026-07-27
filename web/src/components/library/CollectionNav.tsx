// Collection navigation for the library (plan P2.4 §"collection
// navigation"). A row of quick-switch pills built from real `GET
// /collections` data — no filesystem tree, no invented hierarchy. Reuses
// `RouteLink` (so back/forward and the shared router stay authoritative)
// and the existing `.btn`/`.btn-primary`/`.btn-secondary` classes for the
// active/inactive look; no new CSS.
import { RouteLink } from '../RouteLink';
import { buildScopedPath } from '../../lib/router';
import type { Collection } from './types';

export function CollectionNav({
  collections,
  activeCollectionId,
  loading,
}: {
  collections: Collection[];
  activeCollectionId?: string;
  loading: boolean;
}) {
  if (loading) {
    return <p className="text-muted">Đang tải danh sách bộ sưu tập…</p>;
  }
  if (collections.length === 0) {
    return <p className="text-muted">Chưa có bộ sưu tập nào.</p>;
  }
  return (
    <nav
      aria-label="Điều hướng bộ sưu tập"
      style={{ display: 'flex', flexWrap: 'wrap', gap: 'var(--space-2)' }}
    >
      {collections.map((collection) => {
        const isActive = collection.id === activeCollectionId;
        return (
          <RouteLink
            key={collection.id}
            to={buildScopedPath('library', collection.id)}
            className={`btn btn-sm ${isActive ? 'btn-primary' : 'btn-secondary'}`}
            aria-current={isActive ? 'page' : undefined}
          >
            {collection.name}
          </RouteLink>
        );
      })}
    </nav>
  );
}
