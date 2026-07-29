import { describe, expect, it } from 'vitest';
import { computeForceLayout } from './forceLayout';

describe('computeForceLayout', () => {
  it('is deterministic: the same input yields the exact same positions every time', () => {
    const nodeIds = ['a', 'b', 'c', 'd'];
    const edges = [
      { source: 'a', target: 'b' },
      { source: 'b', target: 'c' },
    ];
    const first = computeForceLayout(nodeIds, edges);
    const second = computeForceLayout(nodeIds, edges);
    expect([...second.entries()]).toEqual([...first.entries()]);
  });

  it('is independent of input ordering', () => {
    const edgesA = [
      { source: 'a', target: 'b' },
      { source: 'b', target: 'c' },
    ];
    const edgesB = [
      { source: 'b', target: 'c' },
      { source: 'a', target: 'b' },
    ];
    const layoutA = computeForceLayout(['a', 'b', 'c', 'd'], edgesA);
    const layoutB = computeForceLayout(['d', 'c', 'b', 'a'], edgesB);
    expect([...layoutB.entries()]).toEqual([...layoutA.entries()]);
  });

  it('gives every requested node a finite position', () => {
    const nodeIds = ['a', 'b', 'c', 'd', 'e'];
    const edges = [{ source: 'a', target: 'b' }];
    const layout = computeForceLayout(nodeIds, edges);
    expect(layout.size).toBe(nodeIds.length);
    for (const id of nodeIds) {
      const position = layout.get(id)!;
      expect(Number.isFinite(position.x)).toBe(true);
      expect(Number.isFinite(position.y)).toBe(true);
    }
  });

  it('places a single isolated node at the canvas center', () => {
    const layout = computeForceLayout(['only'], [], { width: 640, height: 480 });
    expect(layout.get('only')).toEqual({ x: 320, y: 240 });
  });

  it('returns an empty map for no nodes', () => {
    const layout = computeForceLayout([], []);
    expect(layout.size).toBe(0);
  });

  it('pulls a connected pair closer together than two unconnected nodes, relatively', () => {
    // Two components: {a-b connected}, {c, d unconnected}. Connected nodes
    // should end up closer to each other than to a same-size unconnected pair.
    const nodeIds = ['a', 'b', 'c', 'd'];
    const edges = [{ source: 'a', target: 'b' }];
    const layout = computeForceLayout(nodeIds, edges, { iterations: 200 });
    const dist = (x: string, y: string) => {
      const p = layout.get(x)!;
      const q = layout.get(y)!;
      return Math.hypot(p.x - q.x, p.y - q.y);
    };
    expect(dist('a', 'b')).toBeLessThan(dist('a', 'c'));
    expect(dist('a', 'b')).toBeLessThan(dist('a', 'd'));
  });

  it('ignores edges referencing an id outside nodeIds rather than throwing', () => {
    const layout = computeForceLayout(['a', 'b'], [{ source: 'a', target: 'ghost' }]);
    expect(layout.size).toBe(2);
  });

  // Regression coverage for the post-nghiệm-thu visual defect (#8aeb539):
  // the first cut's repulsion overpowered its centering pull for small
  // graphs, so every node piled up against the canvas boundary (a "hollow
  // rectangle") instead of clustering near the center. These two tests
  // assert the shape of the fix directly — not just "some pixel moved" —
  // using the same 13-node/3-community topology as the seeded mock graph
  // (`mocks/handlers/graph.ts`), recreated inline here rather than imported
  // so this test has no dependency on that fixture module.
  describe('clustering near center (regression: nodes must not pile up at the canvas edge)', () => {
    const THREE_CLUSTER_NODES = [
      'n200',
      'n201',
      'n202',
      'n203',
      'n204', // cluster 1 (5 nodes)
      'n205',
      'n206',
      'n207',
      'n208',
      'n209', // cluster 2 (5 nodes)
      'n210',
      'n211',
      'n212', // cluster 3 (3 nodes)
    ];
    const THREE_CLUSTER_EDGES = [
      { source: 'n200', target: 'n201' },
      { source: 'n201', target: 'n202' },
      { source: 'n200', target: 'n203' },
      { source: 'n203', target: 'n204' },
      { source: 'n200', target: 'n204' },
      { source: 'n205', target: 'n206' },
      { source: 'n206', target: 'n207' },
      { source: 'n208', target: 'n209' },
      { source: 'n207', target: 'n208' },
      { source: 'n210', target: 'n211' },
      { source: 'n211', target: 'n212' },
      { source: 'n210', target: 'n212' },
    ];
    const WIDTH = 640;
    const HEIGHT = 480;

    function centroidOf(layout: Map<string, { x: number; y: number }>, ids: string[]) {
      let x = 0;
      let y = 0;
      for (const id of ids) {
        const p = layout.get(id)!;
        x += p.x;
        y += p.y;
      }
      return { x: x / ids.length, y: y / ids.length };
    }

    function averagePairDistance(layout: Map<string, { x: number; y: number }>, ids: string[]) {
      let sum = 0;
      let count = 0;
      for (let i = 0; i < ids.length; i += 1) {
        for (let j = i + 1; j < ids.length; j += 1) {
          const a = layout.get(ids[i])!;
          const b = layout.get(ids[j])!;
          sum += Math.hypot(a.x - b.x, a.y - b.y);
          count += 1;
        }
      }
      return count > 0 ? sum / count : 0;
    }

    it('keeps every node within a comfortable margin of the canvas edges — none pinned to a wall', () => {
      const layout = computeForceLayout(THREE_CLUSTER_NODES, THREE_CLUSTER_EDGES, {
        width: WIDTH,
        height: HEIGHT,
      });
      // A wide margin (not "exactly centered") — the point is "not stuck on
      // the boundary", matching the owner's acceptance bar ("không node nào
      // dính mép trừ khi đồ thị quá đông").
      const margin = 60;
      for (const id of THREE_CLUSTER_NODES) {
        const p = layout.get(id)!;
        expect(p.x).toBeGreaterThan(margin);
        expect(p.x).toBeLessThan(WIDTH - margin);
        expect(p.y).toBeGreaterThan(margin);
        expect(p.y).toBeLessThan(HEIGHT - margin);
      }
    });

    it('contracts each community close together while keeping communities visibly separated', () => {
      const layout = computeForceLayout(THREE_CLUSTER_NODES, THREE_CLUSTER_EDGES, {
        width: WIDTH,
        height: HEIGHT,
      });
      const cluster1 = THREE_CLUSTER_NODES.slice(0, 5);
      const cluster2 = THREE_CLUSTER_NODES.slice(5, 10);
      const cluster3 = THREE_CLUSTER_NODES.slice(10, 13);

      const intra1 = averagePairDistance(layout, cluster1);
      const intra2 = averagePairDistance(layout, cluster2);
      const intra3 = averagePairDistance(layout, cluster3);

      const centroid1 = centroidOf(layout, cluster1);
      const centroid2 = centroidOf(layout, cluster2);
      const centroid3 = centroidOf(layout, cluster3);
      const inter12 = Math.hypot(centroid1.x - centroid2.x, centroid1.y - centroid2.y);
      const inter13 = Math.hypot(centroid1.x - centroid3.x, centroid1.y - centroid3.y);
      const inter23 = Math.hypot(centroid2.x - centroid3.x, centroid2.y - centroid3.y);

      // Every community's own average spread is clearly tighter than the
      // distance between any two communities' centroids — the visual
      // signature of "distinct clusters", not "one uniform cloud".
      for (const intra of [intra1, intra2, intra3]) {
        expect(intra).toBeLessThan(inter12 * 0.75);
        expect(intra).toBeLessThan(inter13 * 0.75);
        expect(intra).toBeLessThan(inter23 * 0.75);
      }
    });

    it("centers the whole graph's bounding box near the canvas center, not off in a corner", () => {
      const layout = computeForceLayout(THREE_CLUSTER_NODES, THREE_CLUSTER_EDGES, {
        width: WIDTH,
        height: HEIGHT,
      });
      let minX = Infinity;
      let maxX = -Infinity;
      let minY = Infinity;
      let maxY = -Infinity;
      for (const id of THREE_CLUSTER_NODES) {
        const p = layout.get(id)!;
        minX = Math.min(minX, p.x);
        maxX = Math.max(maxX, p.x);
        minY = Math.min(minY, p.y);
        maxY = Math.max(maxY, p.y);
      }
      const bboxCenterX = (minX + maxX) / 2;
      const bboxCenterY = (minY + maxY) / 2;
      // Within a generous quarter of the canvas from true center — enough to
      // catch "everything piled in one corner" without demanding
      // pixel-perfect centering from a physical simulation.
      expect(Math.abs(bboxCenterX - WIDTH / 2)).toBeLessThan(WIDTH / 4);
      expect(Math.abs(bboxCenterY - HEIGHT / 2)).toBeLessThan(HEIGHT / 4);
    });
  });
});
