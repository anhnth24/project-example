// P2-18: org -> project -> collection -> document grouping. `assign-project`
// itself is registered in `handlers/library.ts` (it's a `/collections/{id}/...`
// sub-route) — see that file's doc.
import { registerOperation } from '../registry';
import { apiError, notFound, unauthorized } from '../apiError';
import { mockTimestamp } from '../ids';
import { authContextForHeader, getOrgProjects, nextId } from '../fixtures';
import type { components } from '../../api/generated/contract';

type Project = components['schemas']['Project'];
type CreateProjectRequest = components['schemas']['CreateProjectRequest'];
type UpdateProjectRequest = components['schemas']['UpdateProjectRequest'];

registerOperation('listProjects', (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  return {
    status: 200,
    body: { items: getOrgProjects(auth.orgId), page: { hasMore: false, nextCursor: null } },
  };
});

// AdminProjectsPage (P2-18 "Khu Quản trị" move) — `uq_projects__org_name`
// mapped to `409 name_taken`, same precedent `POST /orgs`'s `slug_taken`
// already set (see `openapi.yaml`'s own doc comment on `createProject`'s
// 409, which the mock was silently not enforcing until this closed the gap).
registerOperation('createProject', async (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const body = await ctx.json<CreateProjectRequest>();
  const projects = getOrgProjects(auth.orgId);
  if (projects.some((p) => p.name === body.name)) {
    return {
      status: 409,
      body: apiError('name_taken', `Project name "${body.name}" is already used in this org.`),
    };
  }
  const project: Project = { id: nextId(), name: body.name, createdAt: mockTimestamp(0) };
  projects.push(project);
  return { status: 201, body: project };
});

// `updateProject` (rename) does NOT declare a 409 in `openapi.yaml` (only
// 400/403/404/429 — see `api/generated/contract.ts`'s `updateProject`
// responses) — the mock's own drift guard (`spec/driftGuard.ts`'s
// `assertStatusDeclared`) would reject one, so a rename that collides with
// another project's name in this mock is intentionally left as whatever the
// (undeclared) real server behavior turns out to be — not something this
// task's scope covers (no `openapi/contract` edits allowed here). Only
// `createProject` above enforces the uniqueness the acceptance criteria asks
// for.
registerOperation('updateProject', async (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const project = getOrgProjects(auth.orgId).find((p) => p.id === ctx.params.projectId);
  if (!project) return notFound(`Project ${ctx.params.projectId} does not exist.`);
  const body = await ctx.json<UpdateProjectRequest>();
  project.name = body.name;
  return { status: 200, body: project };
});
