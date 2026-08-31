// P2-17 Document Graph MVP (owner request 2026-07-29 — force-directed graph
// + "Communities" checkbox sidebar with per-cluster counts, from the
// reference screenshot). Built directly on the OpenAPI contract + mock
// server like every other P2-0x page (`GET /graph`, `routes/graph.rs`).
//
// Layout: a hand-rolled force simulation (`lib/forceLayout.ts`) — no
// `d3-force` dependency (see that module's doc + the P2-17 report for the
// trade-off). Node click deep-links straight into that node's own document
// preview: `/library/:collectionId?doc=:documentId` (`buildLibraryDocPath`,
// the same P2-07 helper `CitationCard`/`CitationFootnotes` already use).
// `GraphNode.id` doubles as the document id here — confirmed against
// `crates/server/src/db/graph.rs::list_visible_documents`, which selects
// `d.id` straight off the `documents` table, so on the real API a graph
// node's id always names a real, previewable document (small-gap close for
// P2-17; this module's older comment claiming no document-level route
// existed predates P2-07 and was stale).
import { useMemo, useState } from 'react';
import { apiClient, type ApiClient } from '../api/client';
import type { components } from '../api/generated/contract';
import { CommunitySidebar, GraphCanvas } from '../components/graph';
import type { Collection } from '../components/library';
import { describeApiError } from '../components/library';
import { Notice, SelectControl, type SelectOption } from '../components/ui';
import { useScopeSafeRequest } from '../hooks/useScopeSafeRequest';
import { computeForceLayout } from '../lib/forceLayout';
import { buildLibraryDocPath } from '../lib/router';
import { useRouter } from '../state/RouterProvider';

type GraphNode = components['schemas']['GraphNode'];

const ALL_COLLECTIONS_VALUE = '';

export function GraphPage({
  collectionId,
  client = apiClient,
}: {
  collectionId?: string;
  /** Injectable for tests; defaults to the app-wide singleton, same convention as `QaPage`/`LibraryPage`. */
  client?: ApiClient;
}) {
  const { navigate } = useRouter();
  const [selectedCollectionId, setSelectedCollectionId] = useState(
    collectionId ?? ALL_COLLECTIONS_VALUE,
  );
  const [hiddenCommunityIds, setHiddenCommunityIds] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const [viewMode, setViewMode] = useState<'canvas' | 'table'>('canvas');

  const collectionsResult = useScopeSafeRequest(
    (signal) => client.request('get', '/collections', { signal }),
    [client],
  );
  const collections: Collection[] = collectionsResult.data?.items ?? [];

  const graphResult = useScopeSafeRequest(
    (signal) =>
      client.request('get', '/graph', {
        params: {
          query: selectedCollectionId ? { collectionId: selectedCollectionId } : undefined,
        },
        signal,
      }),
    [client, selectedCollectionId],
  );
  const graph = graphResult.data;

  const communityIndexByNodeId = useMemo(() => {
    const map = new Map<string, number>();
    graph?.communities.forEach((community, index) => {
      community.nodeIds.forEach((id) => map.set(id, index));
    });
    return map;
  }, [graph]);

  const hiddenNodeIds = useMemo(() => {
    const hidden = new Set<string>();
    graph?.communities.forEach((community) => {
      if (hiddenCommunityIds.has(community.id)) {
        community.nodeIds.forEach((id) => hidden.add(id));
      }
    });
    return hidden;
  }, [graph, hiddenCommunityIds]);

  const visibleNodes = useMemo(
    () => (graph?.nodes ?? []).filter((node) => !hiddenNodeIds.has(node.id)),
    [graph, hiddenNodeIds],
  );
  const visibleNodeIdSet = useMemo(() => new Set(visibleNodes.map((n) => n.id)), [visibleNodes]);
  const visibleEdges = useMemo(
    () =>
      (graph?.edges ?? []).filter(
        (edge) => visibleNodeIdSet.has(edge.source) && visibleNodeIdSet.has(edge.target),
      ),
    [graph, visibleNodeIdSet],
  );

  const positions = useMemo(
    () =>
      computeForceLayout(
        visibleNodes.map((n) => n.id),
        visibleEdges,
      ),
    [visibleNodes, visibleEdges],
  );

  const visibleCommunityCount = graph
    ? graph.communities.filter((c) => !hiddenCommunityIds.has(c.id)).length
    : 0;
  const liveMessage = graph
    ? `Đang hiển thị ${visibleNodes.length} trong ${graph.nodes.length} tài liệu, ${visibleCommunityCount} trong ${graph.communities.length} cụm.`
    : '';

  function handleCollectionChange(value: string) {
    setSelectedCollectionId(value);
    setHiddenCommunityIds(new Set());
  }

  function toggleCommunity(communityId: string) {
    setHiddenCommunityIds((prev) => {
      const next = new Set(prev);
      if (next.has(communityId)) {
        next.delete(communityId);
      } else {
        next.add(communityId);
      }
      return next;
    });
  }

  function selectAllCommunities() {
    setHiddenCommunityIds(new Set());
  }

  function handleActivateNode(node: GraphNode) {
    navigate(buildLibraryDocPath(node.collectionId, node.id));
  }

  const collectionOptions: SelectOption[] = [
    { value: ALL_COLLECTIONS_VALUE, label: 'Tất cả bộ sưu tập' },
    ...collections.map((collection) => ({ value: collection.id, label: collection.name })),
  ];

  return (
    <section
      className="page graph-page"
      style={{ maxWidth: 'none', minWidth: 0 }}
      aria-labelledby="graph-heading"
    >
      <p className="eyebrow">Đồ thị</p>
      <h1 id="graph-heading">Đồ thị tài liệu</h1>
      <p className="lede">
        Khám phá quan hệ xung đột, đồng trích dẫn và tương đồng giữa các tài liệu đã lập chỉ mục,
        theo từng cộng đồng tài liệu.
      </p>

      <div className="graph-toolbar">
        <SelectControl
          value={selectedCollectionId}
          options={collectionOptions}
          onChange={handleCollectionChange}
          ariaLabel="Lọc theo bộ sưu tập"
        />
        <button
          type="button"
          className="btn btn-secondary btn-sm"
          onClick={() => setViewMode((mode) => (mode === 'canvas' ? 'table' : 'canvas'))}
        >
          {viewMode === 'canvas' ? 'Chế độ xem bảng' : 'Chế độ xem đồ thị'}
        </button>
      </div>

      <p
        className="visually-hidden"
        role="status"
        aria-live="polite"
        data-testid="graph-live-region"
      >
        {liveMessage}
      </p>

      {graphResult.status === 'loading' && <p>Đang tải đồ thị…</p>}
      {graphResult.status === 'error' && (
        <Notice tone="error">{describeApiError(graphResult.error)}</Notice>
      )}
      {graph && graph.nodes.length === 0 && (
        <p className="text-muted">Không có tài liệu nào để hiển thị trong đồ thị này.</p>
      )}
      {graph && graph.nodes.length > 0 && (
        <div className="graph-layout">
          <GraphCanvas
            nodes={visibleNodes}
            edges={visibleEdges}
            positions={positions}
            communityIndexByNodeId={communityIndexByNodeId}
            viewMode={viewMode}
            onActivateNode={handleActivateNode}
          />
          <CommunitySidebar
            communities={graph.communities}
            hiddenCommunityIds={hiddenCommunityIds}
            onToggle={toggleCommunity}
            onSelectAll={selectAllCommunities}
          />
        </div>
      )}
    </section>
  );
}
