// P2-17 Document Graph MVP: a small, hand-rolled force-directed layout
// (Fruchterman-Reingold style — repulsion between every pair, spring
// attraction along edges, mild centering pull, cooling temperature) instead
// of a `d3-force` dependency. ~100 lines, deterministic given the same
// node/edge input: positions come from a seeded PRNG
// (`mulberry32`, not `Math.random`) and nodes are processed in a
// canonicalized (sorted) order regardless of the order the caller passed
// them in, so `GraphPage.test.tsx`/`forceLayout.test.ts` get the exact same
// layout on every run. See the P2-17 report for the trade-off note on why
// this is hand-rolled rather than a dependency.

export interface LayoutEdge {
  source: string;
  target: string;
}

export interface LayoutPosition {
  x: number;
  y: number;
}

export interface ForceLayoutOptions {
  width?: number;
  height?: number;
  iterations?: number;
  /** Seeds both initial placement and is otherwise fully deterministic. */
  seed?: number;
}

/** Deterministic PRNG (mulberry32) — same seed always yields the same sequence. */
function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * Computes `{x, y}` positions for `nodeIds` given `edges` between them.
 * Deterministic and independent of input ordering (node ids are sorted
 * before layout). Isolated nodes (no edges) still get a position — placed on
 * the seeded initial ring, then pulled toward center like everything else.
 */
export function computeForceLayout(
  nodeIds: readonly string[],
  edges: readonly LayoutEdge[],
  options: ForceLayoutOptions = {},
): Map<string, LayoutPosition> {
  const width = options.width ?? 640;
  const height = options.height ?? 480;
  const iterations = options.iterations ?? 150;
  const seed = options.seed ?? 42;

  const ids = [...new Set(nodeIds)].sort();
  const positions = new Map<string, LayoutPosition>();
  if (ids.length === 0) return positions;

  const centerX = width / 2;
  const centerY = height / 2;
  const radius = Math.min(width, height) / 3;
  const random = mulberry32(seed);

  if (ids.length === 1) {
    positions.set(ids[0], { x: centerX, y: centerY });
    return positions;
  }

  // Seeded initial ring placement — deterministic, and already a reasonable
  // starting layout (not all-nodes-at-origin, which would make repulsion's
  // initial direction arbitrary/NaN-prone).
  ids.forEach((id, index) => {
    const angle = (index / ids.length) * Math.PI * 2 + random() * 0.001;
    positions.set(id, {
      x: centerX + radius * Math.cos(angle),
      y: centerY + radius * Math.sin(angle),
    });
  });

  const area = width * height;
  const k = Math.sqrt(area / Math.max(ids.length, 1));
  const validEdges = edges.filter((e) => positions.has(e.source) && positions.has(e.target));

  let temperature = Math.max(width, height) / 10;
  const cooling = temperature / (iterations + 1);

  for (let iteration = 0; iteration < iterations; iteration += 1) {
    const displacement = new Map<string, LayoutPosition>(ids.map((id) => [id, { x: 0, y: 0 }]));

    // Repulsion: every pair pushes apart, inverse-square-ish (k^2 / distance).
    for (let i = 0; i < ids.length; i += 1) {
      const a = positions.get(ids[i])!;
      for (let j = i + 1; j < ids.length; j += 1) {
        const b = positions.get(ids[j])!;
        const dx = a.x - b.x;
        const dy = a.y - b.y;
        const distance = Math.max(Math.hypot(dx, dy), 0.01);
        const force = (k * k) / distance;
        const ux = dx / distance;
        const uy = dy / distance;
        const da = displacement.get(ids[i])!;
        da.x += ux * force;
        da.y += uy * force;
        const db = displacement.get(ids[j])!;
        db.x -= ux * force;
        db.y -= uy * force;
      }
    }

    // Attraction: edges pull their endpoints together (distance^2 / k).
    for (const edge of validEdges) {
      const a = positions.get(edge.source)!;
      const b = positions.get(edge.target)!;
      const dx = a.x - b.x;
      const dy = a.y - b.y;
      const distance = Math.max(Math.hypot(dx, dy), 0.01);
      const force = (distance * distance) / k;
      const ux = dx / distance;
      const uy = dy / distance;
      const da = displacement.get(edge.source)!;
      da.x -= ux * force;
      da.y -= uy * force;
      const db = displacement.get(edge.target)!;
      db.x += ux * force;
      db.y += uy * force;
    }

    // Apply, capped by the cooling temperature, with a mild pull toward
    // center so disconnected components don't drift off canvas.
    for (const id of ids) {
      const position = positions.get(id)!;
      const disp = displacement.get(id)!;
      const distance = Math.max(Math.hypot(disp.x, disp.y), 0.01);
      const capped = Math.min(distance, temperature);
      position.x += (disp.x / distance) * capped + (centerX - position.x) * 0.01;
      position.y += (disp.y / distance) * capped + (centerY - position.y) * 0.01;
      position.x = Math.min(width, Math.max(0, position.x));
      position.y = Math.min(height, Math.max(0, position.y));
    }

    temperature = Math.max(temperature - cooling, 0.01);
  }

  return positions;
}
