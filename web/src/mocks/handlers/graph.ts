/**
 * P2-17 Document Graph MVP mock for `GET /api/v1/graph`.
 *
 * Deliberately self-contained (own fixed node/edge/community catalog below,
 * mockUuid range 200-212/320-322 — disjoint from every id used elsewhere in
 * `mocks/fixtures.ts`, same "own numeric range" convention `QA_COMPARE_*`
 * already follows there) rather than woven into the shared mutable `Store`:
 * the graph is a read-only derived view with nothing else in the mock ever
 * mutating it, so a small fixed catalog here — one per org, keyed by
 * `authContextForHeader(...).orgId` the same way `listCollections` scopes
 * itself — is simpler than adding graph-shaped fields to `Store`.
 *
 * One deliberate exception to "disjoint": the "Sổ tay nhân viên 2024" node
 * reuses `mockUuid(100)` — `fixtures.ts`'s real "Onboarding Guide.pdf"
 * document id (Employee Handbook collection) — instead of a node-only id.
 * `GraphNode.id` doubles as a document id on the real API (node click deep-
 * links to `/library/:collectionId?doc=:id`, P2-17 gap close), so at least
 * one seeded node needs a real, previewable document behind it for that
 * link to resolve to more than a 404 in tests. The other 12 nodes keep
 * their node-only ids (200-212 minus 200) since nothing here exercises their
 * document-preview path.
 *
 * Permission note: the real server gates `GET /graph` on `qa.query`
 * (`routes/graph.rs`), but this mock does not check it — same precedent
 * `handlers/qa.ts`'s `search`/`ask` already set (the real `/search`/`/ask`
 * also require `qa.query`, and the seeded `DEMO_USER` fixture does not have
 * it, yet those mocks never enforce it either). Enforcing it here alone
 * would make the demo user unable to open a page every other org-scoped
 * "read" surface lets them into.
 *
 * Org A: 13 nodes across the two seeded org-A collections
 * (`mockUuid(10)` "Employee Handbook", `mockUuid(11)` "Product Specs"),
 * split into 3 disjoint communities and covering all three edge kinds
 * (`conflict` / `co_citation` / `similarity`) — enough for the sidebar
 * "Communities" checkboxes + force-directed layout to have something real
 * to render. Org B gets a much smaller graph (3 nodes, 1 community) so an
 * org switch visibly changes what's shown, same as every other org-scoped
 * mock fixture in this package.
 */
import { registerOperation } from '../registry';
import { notFound, unauthorized } from '../apiError';
import { nextRequestId, mockUuid } from '../ids';
import {
  authContextForHeader,
  getStore,
  ORG_B_COLLECTION_ID,
  ORG_B_DOCUMENT_ID,
  ORG_B_ID,
} from '../fixtures';
import type { components } from '../../api/generated/contract';

type GraphNode = components['schemas']['GraphNode'];
type GraphEdge = components['schemas']['GraphEdge'];
type GraphCommunity = components['schemas']['GraphCommunity'];

interface NodeSeed {
  id: string;
  title: string;
  collectionId: string;
  collectionName: string;
  status: string;
}

interface EdgeSeed {
  source: string;
  target: string;
  kind: GraphEdge['kind'];
  weight: number;
}

interface CommunitySeed {
  id: string;
  label: string;
  nodeIds: string[];
}

const HANDBOOK_COLLECTION_ID = mockUuid(10);
const HANDBOOK_COLLECTION_NAME = 'Employee Handbook';
const SPECS_COLLECTION_ID = mockUuid(11);
const SPECS_COLLECTION_NAME = 'Product Specs';

const ORG_A_NODES: NodeSeed[] = [
  // Cluster "Nhân sự" (Employee Handbook) — 5 nodes.
  {
    // Reuses fixtures.ts's real "Onboarding Guide.pdf" document id (see
    // module doc above) so this node's ?doc= deep-link resolves to a real
    // preview instead of a 404.
    id: mockUuid(100),
    title: 'Sổ tay nhân viên 2024',
    collectionId: HANDBOOK_COLLECTION_ID,
    collectionName: HANDBOOK_COLLECTION_NAME,
    status: 'indexed',
  },
  {
    id: mockUuid(201),
    title: 'Chính sách nghỉ phép',
    collectionId: HANDBOOK_COLLECTION_ID,
    collectionName: HANDBOOK_COLLECTION_NAME,
    status: 'indexed',
  },
  {
    id: mockUuid(202),
    title: 'Chính sách nghỉ phép (bản cập nhật)',
    collectionId: HANDBOOK_COLLECTION_ID,
    collectionName: HANDBOOK_COLLECTION_NAME,
    status: 'indexed',
  },
  {
    id: mockUuid(203),
    title: 'Quy trình onboarding',
    collectionId: HANDBOOK_COLLECTION_ID,
    collectionName: HANDBOOK_COLLECTION_NAME,
    status: 'indexed',
  },
  {
    id: mockUuid(204),
    title: 'Chính sách phúc lợi',
    collectionId: HANDBOOK_COLLECTION_ID,
    collectionName: HANDBOOK_COLLECTION_NAME,
    status: 'converting',
  },
  // Cluster "Sản phẩm" (Product Specs) — 5 nodes.
  {
    id: mockUuid(205),
    title: 'Đặc tả sản phẩm v1',
    collectionId: SPECS_COLLECTION_ID,
    collectionName: SPECS_COLLECTION_NAME,
    status: 'indexed',
  },
  {
    id: mockUuid(206),
    title: 'Đặc tả sản phẩm v2',
    collectionId: SPECS_COLLECTION_ID,
    collectionName: SPECS_COLLECTION_NAME,
    status: 'indexed',
  },
  {
    id: mockUuid(207),
    title: 'Tài liệu API',
    collectionId: SPECS_COLLECTION_ID,
    collectionName: SPECS_COLLECTION_NAME,
    status: 'indexed',
  },
  {
    id: mockUuid(208),
    title: 'Lộ trình sản phẩm Q3',
    collectionId: SPECS_COLLECTION_ID,
    collectionName: SPECS_COLLECTION_NAME,
    status: 'indexed',
  },
  {
    id: mockUuid(209),
    title: 'Lộ trình sản phẩm Q4',
    collectionId: SPECS_COLLECTION_ID,
    collectionName: SPECS_COLLECTION_NAME,
    status: 'indexed',
  },
  // Cluster "Liên phòng ban" — cross-collection, 3 nodes.
  {
    id: mockUuid(210),
    title: 'Biên bản họp liên phòng',
    collectionId: HANDBOOK_COLLECTION_ID,
    collectionName: HANDBOOK_COLLECTION_NAME,
    status: 'indexed',
  },
  {
    id: mockUuid(211),
    title: 'Ngân sách dự án chung',
    collectionId: SPECS_COLLECTION_ID,
    collectionName: SPECS_COLLECTION_NAME,
    status: 'indexed',
  },
  {
    id: mockUuid(212),
    title: 'Kế hoạch phối hợp Q3',
    collectionId: HANDBOOK_COLLECTION_ID,
    collectionName: HANDBOOK_COLLECTION_NAME,
    status: 'indexed',
  },
];

const ORG_A_EDGES: EdgeSeed[] = [
  // Cluster 1 (Nhân sự).
  { source: mockUuid(100), target: mockUuid(201), kind: 'co_citation', weight: 0.4 },
  { source: mockUuid(201), target: mockUuid(202), kind: 'conflict', weight: 0.9 },
  { source: mockUuid(100), target: mockUuid(203), kind: 'co_citation', weight: 0.3 },
  { source: mockUuid(203), target: mockUuid(204), kind: 'similarity', weight: 0.5 },
  { source: mockUuid(100), target: mockUuid(204), kind: 'co_citation', weight: 0.2 },
  // Cluster 2 (Sản phẩm).
  { source: mockUuid(205), target: mockUuid(206), kind: 'similarity', weight: 0.8 },
  { source: mockUuid(206), target: mockUuid(207), kind: 'co_citation', weight: 0.35 },
  { source: mockUuid(208), target: mockUuid(209), kind: 'conflict', weight: 0.6 },
  { source: mockUuid(207), target: mockUuid(208), kind: 'similarity', weight: 0.25 },
  // Cluster 3 (Liên phòng ban).
  { source: mockUuid(210), target: mockUuid(211), kind: 'conflict', weight: 0.55 },
  { source: mockUuid(211), target: mockUuid(212), kind: 'co_citation', weight: 0.45 },
  { source: mockUuid(210), target: mockUuid(212), kind: 'similarity', weight: 0.3 },
];

const ORG_A_COMMUNITIES: CommunitySeed[] = [
  {
    id: 'community-0',
    label: HANDBOOK_COLLECTION_NAME,
    nodeIds: [mockUuid(100), mockUuid(201), mockUuid(202), mockUuid(203), mockUuid(204)],
  },
  {
    id: 'community-1',
    label: SPECS_COLLECTION_NAME,
    nodeIds: [mockUuid(205), mockUuid(206), mockUuid(207), mockUuid(208), mockUuid(209)],
  },
  {
    id: 'community-2',
    label: 'Liên phòng ban',
    nodeIds: [mockUuid(210), mockUuid(211), mockUuid(212)],
  },
];

const GLOBEX_EXPANSION_PLAN_ID = mockUuid(320);
const GLOBEX_EXPANSION_BUDGET_ID = mockUuid(321);

const ORG_B_NODES: NodeSeed[] = [
  {
    id: ORG_B_DOCUMENT_ID,
    title: 'Globex Master Plan.pdf',
    collectionId: ORG_B_COLLECTION_ID,
    collectionName: 'Globex Roadmap',
    status: 'indexed',
  },
  {
    id: GLOBEX_EXPANSION_PLAN_ID,
    title: 'Kế hoạch mở rộng Globex',
    collectionId: ORG_B_COLLECTION_ID,
    collectionName: 'Globex Roadmap',
    status: 'indexed',
  },
  {
    id: GLOBEX_EXPANSION_BUDGET_ID,
    title: 'Ngân sách mở rộng Globex',
    collectionId: ORG_B_COLLECTION_ID,
    collectionName: 'Globex Roadmap',
    status: 'indexed',
  },
];

const ORG_B_EDGES: EdgeSeed[] = [
  { source: ORG_B_DOCUMENT_ID, target: GLOBEX_EXPANSION_PLAN_ID, kind: 'co_citation', weight: 0.5 },
  {
    source: GLOBEX_EXPANSION_PLAN_ID,
    target: GLOBEX_EXPANSION_BUDGET_ID,
    kind: 'conflict',
    weight: 0.65,
  },
];

const ORG_B_COMMUNITIES: CommunitySeed[] = [
  {
    id: 'community-0',
    label: 'Globex Roadmap',
    nodeIds: [ORG_B_DOCUMENT_ID, GLOBEX_EXPANSION_PLAN_ID, GLOBEX_EXPANSION_BUDGET_ID],
  },
];

function degreeOf(nodeId: string, edges: EdgeSeed[]): number {
  return edges.filter((e) => e.source === nodeId || e.target === nodeId).length;
}

function buildGraph(
  nodes: NodeSeed[],
  edges: EdgeSeed[],
  communities: CommunitySeed[],
  collectionFilter: string | null,
): { nodes: GraphNode[]; edges: GraphEdge[]; communities: GraphCommunity[] } {
  const keptNodes = collectionFilter
    ? nodes.filter((n) => n.collectionId === collectionFilter)
    : nodes;
  const keptIds = new Set(keptNodes.map((n) => n.id));
  const keptEdges = edges.filter((e) => keptIds.has(e.source) && keptIds.has(e.target));

  return {
    nodes: keptNodes.map((n) => ({
      id: n.id,
      title: n.title,
      collectionId: n.collectionId,
      collectionName: n.collectionName,
      status: n.status,
      degree: degreeOf(n.id, keptEdges),
    })),
    edges: keptEdges.map((e) => ({ ...e })),
    communities: communities
      .map((c) => ({ ...c, nodeIds: c.nodeIds.filter((id) => keptIds.has(id)) }))
      .filter((c) => c.nodeIds.length > 0)
      .map((c) => ({ id: c.id, label: c.label, nodeIds: c.nodeIds, size: c.nodeIds.length })),
  };
}

registerOperation('getGraph', (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();

  const collectionFilter = ctx.query.get('collectionId');
  if (collectionFilter) {
    const collection = getStore().collections.find((c) => c.id === collectionFilter);
    if (!collection || getStore().collectionOrgId.get(collectionFilter) !== auth.orgId) {
      return notFound(`Collection ${collectionFilter} does not exist.`);
    }
  }

  const [nodes, edges, communities] =
    auth.orgId === ORG_B_ID
      ? [ORG_B_NODES, ORG_B_EDGES, ORG_B_COMMUNITIES]
      : [ORG_A_NODES, ORG_A_EDGES, ORG_A_COMMUNITIES];
  const graph = buildGraph(nodes, edges, communities, collectionFilter);

  return {
    status: 200,
    body: { ...graph, requestId: nextRequestId() },
  };
});
