// Collection navigation for the library (plan P2.4 §"collection
// navigation"). A row of quick-switch pills built from real `GET
// /collections` data — no filesystem tree, no invented hierarchy. Reuses
// `RouteLink` (so back/forward and the shared router stay authoritative)
// and the existing `.btn`/`.btn-primary`/`.btn-secondary` classes for the
// active/inactive look; no new CSS.
//
// P2-18: grouped by project (`collection.projectId`/`projectName`, already
// joined server-side — see `api/types.rs`'s `CollectionDto` doc) under a
// heading per project, with a final "Chưa thuộc dự án" group for anything
// unassigned. Groups are ordered by project name, "Chưa thuộc dự án" last;
// within a group, collections keep the server's own order (already
// alphabetical — `db::collections::list`) so this file never re-sorts
// data the server already ordered.
import { RouteLink } from '../RouteLink';
import { buildScopedPath } from '../../lib/router';
import type { Collection } from './types';

const UNASSIGNED_LABEL = 'Chưa thuộc dự án';

function groupByProject(collections: Collection[]): { label: string; items: Collection[] }[] {
  const groups = new Map<string, { label: string; items: Collection[] }>();
  for (const collection of collections) {
    const key = collection.projectId ?? '';
    const label = collection.projectId ? (collection.projectName ?? '') : UNASSIGNED_LABEL;
    const group = groups.get(key) ?? { label, items: [] };
    group.items.push(collection);
    groups.set(key, group);
  }
  const assigned = [...groups.entries()]
    .filter(([key]) => key !== '')
    .map(([, group]) => group)
    .sort((a, b) => a.label.localeCompare(b.label));
  const unassigned = groups.get('');
  return unassigned ? [...assigned, unassigned] : assigned;
}

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
  const groups = groupByProject(collections);
  return (
    <nav
      aria-label="Điều hướng bộ sưu tập"
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-3)',
        minWidth: 0,
        maxWidth: '100%',
      }}
    >
      {groups.map((group) => (
        <div
          key={group.label}
          style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}
        >
          <p className="eyebrow" style={{ margin: 0 }}>
            {group.label}
          </p>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 'var(--space-2)' }}>
            {group.items.map((collection) => {
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
          </div>
        </div>
      ))}
    </nav>
  );
}
