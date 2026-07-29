// P2-17 Document Graph MVP: a small, hand-rolled force-directed layout
// (Fruchterman-Reingold style — repulsion between every pair, spring
// attraction along edges, gravity toward the canvas center, cooling
// temperature) instead of a `d3-force` dependency. Deterministic given the
// same node/edge input: positions come from a seeded PRNG (`mulberry32`, not
// `Math.random`) and nodes are processed in a canonicalized (sorted) order
// regardless of the order the caller passed them in, so
// `GraphPage.test.tsx`/`forceLayout.test.ts` get the exact same layout on
// every run. See the P2-17 report for the trade-off note on why this is
// hand-rolled rather than a dependency.
//
// Tuning note (visual defect fix, post-nghiệm-thu #8aeb539): the first cut of
// this module derived its repulsion/attraction scale `k` from
// `sqrt(canvasArea / nodeCount)` with only a token 1%-of-gap centering pull
// applied *after* the temperature cap. For a small graph (the seeded mock's
// 13 nodes) that made `k` — and therefore repulsion — large relative to the
// canvas, while the centering pull was too weak to ever compete with it: every
// node's true equilibrium distance from center sat *outside* the canvas, so
// each iteration pushed everything outward until the boundary clamp caught
// it — nodes piling up along the four edges/corners (a hollow rectangle)
// instead of clustering. The fix has two parts:
//   1. `k` (the ideal spring/repulsion length) is now a fixed constant
//      independent of canvas size or node count, so repulsion strength
//      doesn't blow up for small graphs.
//   2. Gravity toward the center is folded into the *same*
//      temperature-capped displacement budget as repulsion/attraction
//      (proportional to distance from center, like a spring anchored at the
//      canvas center) instead of being a separate, weak, uncapped nudge.
//      Because it now competes fairly with repulsion inside one capped
//      vector, the simulation settles at a genuine equilibrium radius well
//      inside the canvas rather than at whatever wall it happened to hit —
//      the boundary clamp is left in only as a safety net for pathological
//      inputs, not something the normal case ever reaches.
// Re-tuned constants were verified by inspecting actual computed positions
// (bounding box / pairwise distances) for the seeded 13-node/3-community mock
// graph, then confirmed visually via a Playwright screenshot of the real
// GraphPage (see the report for the screenshot path) before landing.

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
 * Ideal edge length ("k" in Fruchterman-Reingold terms) — a fixed constant,
 * NOT derived from canvas area or node count. Keeping it fixed is what keeps
 * repulsion (which scales with `k^2`) from overpowering gravity on a small
 * graph; a larger graph instead relies on more nodes competing for the same
 * gravity well, which naturally spreads them further from center, still
 * bounded (see `GRAVITY_STRENGTH`'s doc below).
 */
const IDEAL_EDGE_LENGTH = 45;
/** Multiplies the repulsion force (`k^2 / distance`) between every pair. */
const REPULSION_STRENGTH = 1;
/** Multiplies the spring/attraction force (`distance^2 / k`) along edges — kept above 1 so same-community nodes visibly contract toward each other rather than merely resisting repulsion. */
const SPRING_STRENGTH = 2.2;
/**
 * Multiplies the gravity force pulling every node toward the canvas center,
 * proportional to its current distance from center (a spring anchored at
 * center). This is what bounds the whole layout to a cluster near the
 * middle instead of the repulsion-only system's "expand until the wall"
 * behavior — see this module's top-of-file doc for the incident this fixes.
 *
 * Sized (along with `IDEAL_EDGE_LENGTH`) from the equilibrium-radius
 * approximation `radius ≈ k * sqrt((n - 1) / GRAVITY_STRENGTH)` for a
 * roughly-uniform repulsion/gravity balance: for the seeded mock's 13-node
 * graph this keeps the whole layout's radius around ~140px — comfortably
 * inside a 640×480 canvas — while `SPRING_STRENGTH` still contracts each
 * community's own nodes down to a much tighter ~30px spread, so clusters
 * read as visually distinct clumps rather than one uniform cloud. Verified
 * both numerically (bounding box / pairwise distances) and visually (a
 * Playwright screenshot of the real `GraphPage`) — see the P2-17 report.
 */
const GRAVITY_STRENGTH = 0.85;

function computeInitialTemperature(width: number, height: number): number {
  return Math.min(width, height) / 8;
}

/**
 * Computes `{x, y}` positions for `nodeIds` given `edges` between them.
 * Deterministic and independent of input ordering (node ids are sorted
 * before layout). Isolated nodes (no edges) still get a position — placed on
 * the seeded initial ring, then pulled toward center by gravity like
 * everything else, so they end up near — not away from — the main cluster.
 */
export function computeForceLayout(
  nodeIds: readonly string[],
  edges: readonly LayoutEdge[],
  options: ForceLayoutOptions = {},
): Map<string, LayoutPosition> {
  const width = options.width ?? 640;
  const height = options.height ?? 480;
  const iterations = options.iterations ?? 200;
  const seed = options.seed ?? 42;

  const ids = [...new Set(nodeIds)].sort();
  const positions = new Map<string, LayoutPosition>();
  if (ids.length === 0) return positions;

  const centerX = width / 2;
  const centerY = height / 2;
  // Initial ring radius is deliberately modest (not tied to canvas size) —
  // just a reasonable, non-degenerate starting spread for gravity/repulsion
  // to settle from; the canvas-sized ring the first cut used just made the
  // "already at the wall" starting condition worse.
  const radius = Math.min(IDEAL_EDGE_LENGTH * 1.5, Math.min(width, height) / 4);
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

  const k = IDEAL_EDGE_LENGTH;
  // Canonicalized (sorted) edge order — not the caller's — for the same
  // reason node ids are sorted above: floating-point addition is
  // commutative but not associative, so accumulating a node's displacement
  // from several edges in a different order can round to a different last
  // bit. Sorting makes that accumulation order fixed regardless of the
  // order `edges` was passed in, so `computeForceLayout` stays exactly
  // (bit-for-bit) independent of input ordering, not just approximately so.
  const validEdges = edges
    .filter((e) => positions.has(e.source) && positions.has(e.target))
    .slice()
    .sort((a, b) => {
      if (a.source !== b.source) return a.source < b.source ? -1 : 1;
      if (a.target !== b.target) return a.target < b.target ? -1 : 1;
      return 0;
    });

  let temperature = computeInitialTemperature(width, height);
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
        const force = (REPULSION_STRENGTH * (k * k)) / distance;
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
      const force = (SPRING_STRENGTH * (distance * distance)) / k;
      const ux = dx / distance;
      const uy = dy / distance;
      const da = displacement.get(edge.source)!;
      da.x -= ux * force;
      da.y -= uy * force;
      const db = displacement.get(edge.target)!;
      db.x += ux * force;
      db.y += uy * force;
    }

    // Gravity: every node is pulled toward the canvas center, proportional
    // to its current distance from it — folded into the same displacement
    // vector as repulsion/attraction (see module doc: this is the part that
    // actually fixes the "nodes pinned to the wall" defect, because it now
    // competes fairly with repulsion inside the same temperature cap below
    // instead of being a separate, weak, uncapped nudge).
    for (const id of ids) {
      const position = positions.get(id)!;
      const disp = displacement.get(id)!;
      disp.x += (centerX - position.x) * GRAVITY_STRENGTH;
      disp.y += (centerY - position.y) * GRAVITY_STRENGTH;
    }

    // Apply, capped by the cooling temperature. The boundary clamp below is
    // a safety net for pathological inputs (e.g. an unbounded edge weight
    // computed upstream) — gravity is what keeps the normal case away from
    // it, not the clamp itself.
    for (const id of ids) {
      const position = positions.get(id)!;
      const disp = displacement.get(id)!;
      const distance = Math.max(Math.hypot(disp.x, disp.y), 0.01);
      const capped = Math.min(distance, temperature);
      position.x += (disp.x / distance) * capped;
      position.y += (disp.y / distance) * capped;
      position.x = Math.min(width, Math.max(0, position.x));
      position.y = Math.min(height, Math.max(0, position.y));
    }

    temperature = Math.max(temperature - cooling, 0.01);
  }

  return positions;
}
