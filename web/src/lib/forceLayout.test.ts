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
});
