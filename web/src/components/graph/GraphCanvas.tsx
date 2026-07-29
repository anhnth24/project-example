// P2-17 Document Graph MVP: renders the force-directed layout as SVG, plus
// two accessible fallbacks required alongside any canvas/SVG graph
// (owner's a11y note — "canvas graph phải có fallback ngữ nghĩa"):
//
//   1. The SVG itself is `role="img"` with one summary `aria-label`; every
//      shape inside it is `aria-hidden` — a screen reader hears the one
//      summary, not one announcement per circle/line.
//   2. The REAL interactive/semantic surface is the node list below the
//      SVG: plain, always-visible, keyboard-focusable `<button>`s (not
//      hidden — visible content already satisfies "exposed to a screen
//      reader", and it directly is the "keyboard focus vào node list" the
//      brief asks for). Clicking an SVG circle mirrors the same action for
//      mouse users (`tabIndex={-1}` — it is not a second, redundant tab
//      stop).
//   3. `viewMode="table"` swaps the SVG for two real `<table>`s (nodes +
//      edges) — the "chế độ xem bảng chuyển đổi được" alternative, and the
//      only one of the three that also exposes edges structurally.
import type { components } from '../../api/generated/contract';
import type { LayoutPosition } from '../../lib/forceLayout';

type GraphNode = components['schemas']['GraphNode'];
type GraphEdge = components['schemas']['GraphEdge'];

const EDGE_KIND_LABELS: Record<GraphEdge['kind'], string> = {
  conflict: 'Xung đột',
  co_citation: 'Đồng trích dẫn',
  similarity: 'Tương đồng',
};

/** Fixed, cycling palette — enough distinct hues for a handful of communities. */
const COMMUNITY_COLORS = [
  'var(--color-accent-600)',
  'var(--color-accent-2-600)',
  '#3b6ea5',
  '#a5473b',
  '#7a4ba5',
  '#4ba58f',
];

function colorForCommunity(index: number): string {
  return COMMUNITY_COLORS[index % COMMUNITY_COLORS.length];
}

function radiusForDegree(degree: number): number {
  return Math.min(20, 7 + degree * 2);
}

export function GraphCanvas({
  nodes,
  edges,
  positions,
  communityIndexByNodeId,
  viewMode,
  width = 640,
  height = 480,
  onActivateNode,
}: {
  nodes: GraphNode[];
  edges: GraphEdge[];
  positions: Map<string, LayoutPosition>;
  communityIndexByNodeId: Map<string, number>;
  viewMode: 'canvas' | 'table';
  width?: number;
  height?: number;
  onActivateNode: (node: GraphNode) => void;
}) {
  if (viewMode === 'table') {
    return (
      <div className="graph-table-view">
        <table>
          <caption>Danh sách tài liệu trong đồ thị</caption>
          <thead>
            <tr>
              <th scope="col">Tài liệu</th>
              <th scope="col">Bộ sưu tập</th>
              <th scope="col">Trạng thái</th>
              <th scope="col">Bậc (degree)</th>
            </tr>
          </thead>
          <tbody>
            {nodes.map((node) => (
              <tr key={node.id}>
                <td>
                  <button
                    type="button"
                    className="link-button"
                    onClick={() => onActivateNode(node)}
                  >
                    {node.title}
                  </button>
                </td>
                <td>{node.collectionName}</td>
                <td>{node.status}</td>
                <td>{node.degree}</td>
              </tr>
            ))}
          </tbody>
        </table>

        <table>
          <caption>Danh sách liên kết trong đồ thị</caption>
          <thead>
            <tr>
              <th scope="col">Từ</th>
              <th scope="col">Đến</th>
              <th scope="col">Loại</th>
              <th scope="col">Trọng số</th>
            </tr>
          </thead>
          <tbody>
            {edges.map((edge, index) => {
              const sourceTitle = nodes.find((n) => n.id === edge.source)?.title ?? edge.source;
              const targetTitle = nodes.find((n) => n.id === edge.target)?.title ?? edge.target;
              return (
                <tr key={`${edge.source}-${edge.target}-${edge.kind}-${index}`}>
                  <td>{sourceTitle}</td>
                  <td>{targetTitle}</td>
                  <td>{EDGE_KIND_LABELS[edge.kind]}</td>
                  <td>{edge.weight.toFixed(2)}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    );
  }

  return (
    <div className="graph-canvas-wrap">
      <svg
        role="img"
        aria-label={`Đồ thị gồm ${nodes.length} tài liệu và ${edges.length} liên kết`}
        viewBox={`0 0 ${width} ${height}`}
        className="graph-canvas"
      >
        <g aria-hidden="true">
          {edges.map((edge, index) => {
            const source = positions.get(edge.source);
            const target = positions.get(edge.target);
            if (!source || !target) return null;
            return (
              <line
                key={`${edge.source}-${edge.target}-${edge.kind}-${index}`}
                x1={source.x}
                y1={source.y}
                x2={target.x}
                y2={target.y}
                className={`graph-edge graph-edge-${edge.kind}`}
                strokeWidth={Math.max(1, edge.weight * 3)}
              />
            );
          })}
          {nodes.map((node) => {
            const position = positions.get(node.id);
            if (!position) return null;
            const communityIndex = communityIndexByNodeId.get(node.id) ?? 0;
            return (
              <circle
                key={node.id}
                cx={position.x}
                cy={position.y}
                r={radiusForDegree(node.degree)}
                fill={colorForCommunity(communityIndex)}
                className="graph-node-dot"
                tabIndex={-1}
                onClick={() => onActivateNode(node)}
              >
                <title>{`${node.title} (${node.collectionName})`}</title>
              </circle>
            );
          })}
        </g>
      </svg>

      <div className="graph-node-list-wrap">
        <h2 id="graph-node-list-heading">Danh sách tài liệu</h2>
        <ul className="graph-node-list" aria-labelledby="graph-node-list-heading">
          {nodes.map((node) => (
            <li key={node.id}>
              <button
                type="button"
                className="graph-node-button"
                onClick={() => onActivateNode(node)}
              >
                <span
                  className="graph-node-swatch"
                  style={{
                    background: colorForCommunity(communityIndexByNodeId.get(node.id) ?? 0),
                  }}
                  aria-hidden="true"
                />
                <span>{node.title}</span>
                <span className="text-muted">
                  {node.collectionName} · bậc {node.degree}
                </span>
              </button>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
