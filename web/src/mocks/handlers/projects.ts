// P2-18: org -> project -> collection -> document grouping. `assign-project`
// itself is registered in `handlers/library.ts` (it's a `/collections/{id}/...`
// sub-route) — see that file's doc.
import { registerOperation } from '../registry';
import { notFound, unauthorized } from '../apiError';
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

registerOperation('createProject', async (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const body = await ctx.json<CreateProjectRequest>();
  const project: Project = { id: nextId(), name: body.name, createdAt: mockTimestamp(0) };
  getOrgProjects(auth.orgId).push(project);
  return { status: 201, body: project };
});

registerOperation('updateProject', async (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const project = getOrgProjects(auth.orgId).find((p) => p.id === ctx.params.projectId);
  if (!project) return notFound(`Project ${ctx.params.projectId} does not exist.`);
  const body = await ctx.json<UpdateProjectRequest>();
  project.name = body.name;
  return { status: 200, body: project };
});
