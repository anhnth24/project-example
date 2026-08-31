// P2-18 "Khu Quản trị" move (owner critique, 2026-07-29: "quản lý
// project/document/người dùng đang thiết kế UIUX chưa hợp lý"). Project
// management (create + rename + assign/unassign collections) moves out of
// `LibraryPage`'s old `ProjectsPanel` into its own admin route,
// `/admin/projects` — gated by `ProtectedRoute permission="doc.upload"` in
// `App.tsx` (same permission `POST /projects`/`PATCH /projects/{id}`/
// `POST /collections/{id}/assign-project` require server-side, see
// `mocks/handlers/projects.ts`'s own doc) — so, like `AdminMembersPage.tsx`/
// `AdminUsagePage.tsx`, this page does not re-check the permission itself:
// the route guard already decided whether to render it at all, and the
// server's own 403 remains the real authority regardless.
//
// Every fetch goes through `useScopeSafeRequest` (never a raw `useEffect` +
// fetch), and every list is retained across a mutation-triggered refetch
// (`retainedProjects`/`retainedCollections`) so the table doesn't blank for a
// frame and lose whatever inline-edit/error state a row was showing — same
// "refresh must not blank the list" pattern `LibraryPage.tsx`/
// `AdminMembersPage.tsx` already use, see those files' own module docs for
// why each guard exists.
//
// 409 `name_taken` is only handled for CREATE: `openapi.yaml` declares 409 on
// `POST /projects` but not on `PATCH /projects/{id}` (see
// `mocks/handlers/projects.ts`'s own note) — this task's scope does not
// include editing the OpenAPI contract, so the inline rename below only
// surfaces whatever the (undeclared-for-409) generic error path produces.
import { useState, type FormEvent } from 'react';
import { apiClient, type ApiClient } from '../api/client';
import { HttpApiError } from '../api/errors';
import type { Collection, Project } from '../components/library';
import { describeApiError } from '../components/library';
import { Notice, SelectControl } from '../components/ui';
import { useScopeSafeRequest } from '../hooks/useScopeSafeRequest';
import { useScope } from '../state/ScopeProvider';

const UNASSIGNED_LABEL = 'Chưa thuộc dự án';

/** `name_taken` (409) gets a specific, actionable message; anything else falls back to the shared generic mapping. */
function describeProjectSaveError(cause: unknown): string {
  if (cause instanceof HttpApiError && cause.code === 'name_taken') {
    return 'Tên dự án này đã được dùng trong tổ chức — hãy chọn một tên khác.';
  }
  return describeApiError(cause);
}

export function AdminProjectsPage({ client = apiClient }: { client?: ApiClient } = {}) {
  const { epoch } = useScope();

  const [projectsRetry, setProjectsRetry] = useState(0);
  const projectsResult = useScopeSafeRequest(
    (signal) => client.request('get', '/projects', { signal }),
    [client, projectsRetry],
  );
  const [retainedProjects, setRetainedProjects] = useState<{
    epoch: number;
    data: NonNullable<typeof projectsResult.data>;
  } | null>(null);
  if (projectsResult.data && retainedProjects?.data !== projectsResult.data) {
    setRetainedProjects({ epoch, data: projectsResult.data });
  }
  const projectsData =
    projectsResult.data ?? (retainedProjects?.epoch === epoch ? retainedProjects.data : undefined);
  const projects: Project[] = projectsData?.items ?? [];

  const [collectionsRetry, setCollectionsRetry] = useState(0);
  const collectionsResult = useScopeSafeRequest(
    (signal) => client.request('get', '/collections', { signal }),
    [client, collectionsRetry],
  );
  const [retainedCollections, setRetainedCollections] = useState<{
    epoch: number;
    data: NonNullable<typeof collectionsResult.data>;
  } | null>(null);
  if (collectionsResult.data && retainedCollections?.data !== collectionsResult.data) {
    setRetainedCollections({ epoch, data: collectionsResult.data });
  }
  const collectionsData =
    collectionsResult.data ??
    (retainedCollections?.epoch === epoch ? retainedCollections.data : undefined);
  const collections: Collection[] = collectionsData?.items ?? [];

  const collectionsByProject = new Map<string, Collection[]>();
  const unassignedCollections: Collection[] = [];
  for (const collection of collections) {
    if (collection.projectId) {
      const list = collectionsByProject.get(collection.projectId) ?? [];
      list.push(collection);
      collectionsByProject.set(collection.projectId, list);
    } else {
      unassignedCollections.push(collection);
    }
  }

  function refetchCollections() {
    setCollectionsRetry((n) => n + 1);
  }

  // --- Create -----------------------------------------------------------
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
    } catch (error) {
      setCreateError(error);
    } finally {
      setCreating(false);
    }
  }

  // --- Inline rename ------------------------------------------------------
  const [editingId, setEditingId] = useState<string | undefined>(undefined);
  const [editValue, setEditValue] = useState('');
  const [renaming, setRenaming] = useState(false);
  const [renameError, setRenameError] = useState<unknown>(undefined);

  function startEditing(project: Project) {
    setEditingId(project.id);
    setEditValue(project.name);
    setRenameError(undefined);
  }

  function cancelEditing() {
    setEditingId(undefined);
    setRenameError(undefined);
  }

  async function saveRename(projectId: string) {
    const trimmed = editValue.trim();
    if (trimmed === '' || renaming) return;
    setRenaming(true);
    setRenameError(undefined);
    try {
      await client.request('patch', '/projects/{projectId}', {
        params: { path: { projectId } },
        body: { name: trimmed },
      });
      setEditingId(undefined);
      setProjectsRetry((n) => n + 1);
    } catch (error) {
      setRenameError(error);
    } finally {
      setRenaming(false);
    }
  }

  // --- Assign / unassign ---------------------------------------------------
  const [assigningId, setAssigningId] = useState<string | undefined>(undefined);
  const [assignError, setAssignError] = useState<unknown>(undefined);

  async function assign(collectionId: string, projectId: string | null): Promise<void> {
    setAssigningId(collectionId);
    setAssignError(undefined);
    try {
      await client.request('post', '/collections/{collectionId}/assign-project', {
        params: { path: { collectionId } },
        body: { projectId },
      });
      refetchCollections();
    } catch (error) {
      setAssignError(error);
    } finally {
      setAssigningId(undefined);
    }
  }

  const projectOptions = [
    { value: '', label: UNASSIGNED_LABEL },
    ...projects.map((p) => ({ value: p.id, label: p.name })),
  ];

  const loadingProjects = projectsResult.status === 'loading' && projectsData === undefined;

  return (
    <section
      className="page"
      style={{ maxWidth: 'none', minWidth: 0 }}
      aria-labelledby="admin-projects-heading"
    >
      <p className="eyebrow">Quản trị</p>
      <h1 id="admin-projects-heading">Dự án</h1>
      <p className="lede">
        Nhóm bộ sưu tập theo dự án để duyệt thư viện và hỏi đáp theo phạm vi hẹp hơn. Một bộ sưu tập
        chỉ thuộc tối đa một dự án.
      </p>

      {projectsResult.status === 'error' && (
        <Notice
          tone="error"
          action={
            <button
              type="button"
              className="btn btn-secondary btn-sm"
              onClick={() => setProjectsRetry((n) => n + 1)}
            >
              Thử lại
            </button>
          }
        >
          {describeApiError(projectsResult.error)}
        </Notice>
      )}
      {collectionsResult.status === 'error' && (
        <Notice
          tone="error"
          action={
            <button type="button" className="btn btn-secondary btn-sm" onClick={refetchCollections}>
              Thử lại
            </button>
          }
        >
          {describeApiError(collectionsResult.error)}
        </Notice>
      )}

      <div className="card" style={{ gap: 'var(--space-3)' }}>
        <h2 className="card-title">Tạo dự án mới</h2>
        <form
          onSubmit={createProject}
          style={{
            display: 'flex',
            gap: 'var(--space-2)',
            alignItems: 'flex-end',
            flexWrap: 'wrap',
            minWidth: 0,
            maxWidth: '100%',
          }}
        >
          <div className="field" style={{ flex: '1 1 16rem', minWidth: 0, maxWidth: '100%' }}>
            <label htmlFor="new-project-name">Tên dự án</label>
            <input
              id="new-project-name"
              className="input"
              type="text"
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="Ví dụ: Nhân sự"
            />
          </div>
          <button
            type="submit"
            className="btn btn-primary"
            disabled={name.trim() === '' || creating}
          >
            Tạo dự án
          </button>
        </form>
        {createError !== undefined && (
          <Notice tone="error">{describeProjectSaveError(createError)}</Notice>
        )}
      </div>

      <div className="card" style={{ gap: 'var(--space-3)', minWidth: 0 }}>
        <h2 className="card-title">Danh sách dự án</h2>
        {loadingProjects && <p className="text-muted">Đang tải danh sách dự án…</p>}
        {!loadingProjects && projects.length === 0 && (
          <p className="text-muted">Chưa có dự án nào. Tạo dự án đầu tiên ở trên.</p>
        )}
        {!loadingProjects && projects.length > 0 && (
          <table className="table" aria-label="Danh sách dự án">
            <thead>
              <tr>
                <th scope="col">Tên dự án</th>
                <th scope="col">Bộ sưu tập</th>
                <th scope="col">Số bộ sưu tập</th>
              </tr>
            </thead>
            <tbody>
              {projects.map((project) => {
                const assigned = collectionsByProject.get(project.id) ?? [];
                const isEditing = editingId === project.id;
                return (
                  <tr key={project.id}>
                    <td>
                      {isEditing ? (
                        <div
                          style={{
                            display: 'flex',
                            gap: 'var(--space-2)',
                            alignItems: 'center',
                            flexWrap: 'wrap',
                            minWidth: 0,
                          }}
                        >
                          <input
                            className="input"
                            type="text"
                            aria-label={`Tên mới cho dự án ${project.name}`}
                            value={editValue}
                            onChange={(event) => setEditValue(event.target.value)}
                            autoFocus
                            style={{ flex: '1 1 12rem', minWidth: 0 }}
                          />
                          <button
                            type="button"
                            className="btn btn-primary btn-sm"
                            disabled={editValue.trim() === '' || renaming}
                            onClick={() => void saveRename(project.id)}
                          >
                            Lưu
                          </button>
                          <button
                            type="button"
                            className="btn btn-secondary btn-sm"
                            disabled={renaming}
                            onClick={cancelEditing}
                          >
                            Hủy
                          </button>
                        </div>
                      ) : (
                        <div
                          style={{
                            display: 'flex',
                            gap: 'var(--space-2)',
                            alignItems: 'center',
                            flexWrap: 'wrap',
                            minWidth: 0,
                          }}
                        >
                          <span>{project.name}</span>
                          <button
                            type="button"
                            className="btn btn-ghost btn-sm"
                            aria-label={`Sửa tên dự án ${project.name}`}
                            onClick={() => startEditing(project)}
                          >
                            Sửa
                          </button>
                        </div>
                      )}
                      {isEditing && renameError !== undefined && (
                        <Notice tone="error">{describeProjectSaveError(renameError)}</Notice>
                      )}
                    </td>
                    <td>
                      {assigned.length === 0 ? (
                        <span className="text-muted">Chưa có bộ sưu tập nào</span>
                      ) : (
                        <div style={{ display: 'flex', flexWrap: 'wrap', gap: 'var(--space-2)' }}>
                          {assigned.map((collection) => (
                            <span
                              key={collection.id}
                              className="tag tag-neutral"
                              style={{ display: 'inline-flex', gap: 'var(--space-1)' }}
                            >
                              {collection.name}
                              <button
                                type="button"
                                className="btn btn-ghost btn-sm"
                                aria-label={`Bỏ gán bộ sưu tập ${collection.name} khỏi dự án ${project.name}`}
                                disabled={assigningId === collection.id}
                                onClick={() => void assign(collection.id, null)}
                              >
                                ×
                              </button>
                            </span>
                          ))}
                        </div>
                      )}
                    </td>
                    <td>{assigned.length}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>

      <div className="card" style={{ gap: 'var(--space-3)', minWidth: 0 }}>
        <h2 className="card-title">{UNASSIGNED_LABEL}</h2>
        {unassignedCollections.length === 0 ? (
          <p className="text-muted">Mọi bộ sưu tập đều đã thuộc một dự án.</p>
        ) : (
          <div style={{ display: 'grid', gap: 'var(--space-2)' }}>
            {unassignedCollections.map((collection) => (
              <div
                key={collection.id}
                style={{
                  display: 'flex',
                  gap: 'var(--space-2)',
                  alignItems: 'center',
                  flexWrap: 'wrap',
                  minWidth: 0,
                  maxWidth: '100%',
                }}
              >
                <span style={{ flex: '1 1 12rem', minWidth: 0 }}>{collection.name}</span>
                <SelectControl
                  value=""
                  options={projectOptions}
                  onChange={(value) => {
                    if (value !== '') void assign(collection.id, value);
                  }}
                  ariaLabel={`Gán dự án cho bộ sưu tập ${collection.name}`}
                  placeholder={UNASSIGNED_LABEL}
                  disabled={assigningId === collection.id}
                />
                {assigningId === collection.id && <span className="text-muted">Đang lưu…</span>}
              </div>
            ))}
          </div>
        )}
        {assignError !== undefined && <Notice tone="error">{describeApiError(assignError)}</Notice>}
      </div>
    </section>
  );
}
