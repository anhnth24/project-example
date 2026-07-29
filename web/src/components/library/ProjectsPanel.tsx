// P2-18 — simple project management: create a project + assign/unassign
// collections to one. Deliberately placed inside the Library page (not a new
// top-level admin route): projects exist purely to organize collections,
// which the Library page already owns the nav/CRUD context for (see
// `CollectionNav.tsx`'s grouping), so this keeps project management next to
// the thing it organizes instead of adding a new rail destination + router
// entry + `RouteGuard` wiring for what the owner scoped as "simple"
// (create + assign/unassign, no delete). Gated by `hasPermission('doc.upload')`
// — same permission `POST /projects`/`POST /collections/{id}/assign-project`
// require server-side (see `routes::projects`'s module doc); this is UI
// convenience only, the server's 403 is the real authority, same caveat
// `AdminMembersPage.tsx`'s module doc makes about its own owner-tier gating.
import { useState, type FormEvent } from 'react';
import { apiClient, type ApiClient } from '../../api/client';
import { useAuth } from '../../auth/AuthContext';
import { useScopeSafeRequest } from '../../hooks/useScopeSafeRequest';
import { useScope } from '../../state/ScopeProvider';
import { Notice, SelectControl } from '../ui';
import { describeApiError } from './documentPresentation';
import type { Collection } from './types';

export function ProjectsPanel({
  collections,
  client = apiClient,
  onChanged,
}: {
  collections: Collection[];
  client?: ApiClient;
  /** Called after a successful create or assign/unassign so the caller (LibraryPage) can refetch `GET /collections` and pick up the new `projectId`/`projectName`. */
  onChanged?: () => void;
}) {
  const { hasPermission } = useAuth();
  const { epoch } = useScope();
  const canManage = hasPermission('doc.upload');

  const [projectsRetry, setProjectsRetry] = useState(0);
  const projectsResult = useScopeSafeRequest(
    (signal) => client.request('get', '/projects', { signal }),
    [client, projectsRetry, epoch],
  );
  const projects = projectsResult.data?.items ?? [];

  const [name, setName] = useState('');
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<unknown>(undefined);

  async function createProject(event: FormEvent) {
    event.preventDefault();
    const trimmed = name.trim();
    if (trimmed === '' || creating) return;
    setCreating(true);
    setCreateError(undefined);
    try {
      await client.request('post', '/projects', { body: { name: trimmed } });
      setName('');
      setProjectsRetry((n) => n + 1);
      onChanged?.();
    } catch (error) {
      setCreateError(error);
    } finally {
      setCreating(false);
    }
  }

  const [assigningId, setAssigningId] = useState<string | undefined>(undefined);
  const [assignError, setAssignError] = useState<unknown>(undefined);

  async function assign(collectionId: string, projectId: string): Promise<void> {
    setAssigningId(collectionId);
    setAssignError(undefined);
    try {
      await client.request('post', '/collections/{collectionId}/assign-project', {
        params: { path: { collectionId } },
        body: { projectId: projectId === '' ? null : projectId },
      });
      onChanged?.();
    } catch (error) {
      setAssignError(error);
    } finally {
      setAssigningId(undefined);
    }
  }

  if (!canManage) return null;

  const projectOptions = [
    { value: '', label: 'Chưa thuộc dự án' },
    ...projects.map((p) => ({ value: p.id, label: p.name })),
  ];

  return (
    <div
      className="card"
      aria-labelledby="projects-panel-heading"
      style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}
    >
      <h2 id="projects-panel-heading" className="card-title">
        Dự án
      </h2>
      <p className="text-muted">
        Nhóm bộ sưu tập theo dự án để hỏi đáp và duyệt thư viện theo phạm vi hẹp hơn.
      </p>

      <form
        onSubmit={createProject}
        style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'flex-end', flexWrap: 'wrap' }}
      >
        <div className="field" style={{ flex: '1 1 200px' }}>
          <label htmlFor="new-project-name">Tên dự án mới</label>
          <input
            id="new-project-name"
            className="input"
            type="text"
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="Ví dụ: Nhân sự"
          />
        </div>
        <button type="submit" className="btn btn-primary" disabled={name.trim() === '' || creating}>
          Tạo dự án
        </button>
      </form>
      {createError !== undefined && <Notice tone="error">{describeApiError(createError)}</Notice>}

      {collections.length > 0 && (
        <div style={{ display: 'grid', gap: 'var(--space-2)' }}>
          <p className="field-label">Gán bộ sưu tập vào dự án</p>
          {collections.map((collection) => (
            <div
              key={collection.id}
              style={{
                display: 'flex',
                gap: 'var(--space-2)',
                alignItems: 'center',
                flexWrap: 'wrap',
              }}
            >
              <span style={{ minWidth: '12rem' }}>{collection.name}</span>
              <SelectControl
                value={collection.projectId ?? ''}
                options={projectOptions}
                onChange={(value) => void assign(collection.id, value)}
                ariaLabel={`Dự án cho bộ sưu tập ${collection.name}`}
              />
              {assigningId === collection.id && <span className="text-muted">Đang lưu…</span>}
            </div>
          ))}
        </div>
      )}
      {assignError !== undefined && <Notice tone="error">{describeApiError(assignError)}</Notice>}
    </div>
  );
}
