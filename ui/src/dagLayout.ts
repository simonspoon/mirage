// dagLayout.ts — pure DAG layout computation (no Solid dependencies)
// Extracted from the dagLayout createMemo in index.tsx.
//
// Barycenter heuristic: 3 sweep pairs (top-down then bottom-up, 6 passes total).
//   - Top-down: for each band d=1..maxDepth, sort nodes by mean sortedIndex of
//     their band-(d-1) neighbours (xParentsOf).
//   - Bottom-up: for each band d=maxDepth-1..0, sort nodes by mean sortedIndex of
//     their band-(d+1) neighbours (xChildrenOf).
// Short-circuit: bands.length <= 1 → skip sweeps.
// NaN guard: isolated nodes (no cross-band neighbours) keep original list order.
// Final x positions are cumulative: x[0]=PAD; x[i]=x[i-1]+w[i-1]+NODE_GAP, so
// boxes of different widths never overlap.

export const ROW_HEIGHT = 24;
export const HEADER_HEIGHT = 32;

// Width constants. PAD is the left/right margin inside the SVG layout space;
// NODE_GAP is the horizontal gap between adjacent boxes in the same band.
export const PAD = 20;
export const NODE_GAP = 40;

// Per-node width bounds. Default matches the legacy uniform boxWidth.
export const DEFAULT_WIDTH = 260;
export const MIN_WIDTH = 260;
export const MAX_WIDTH = 600;
export const STUB_WIDTH = 260;

export interface GraphEdge {
  source: string;
  target: string;
  /**
   * Optional stable identifier. When supplied (and unique), dagre-backed
   * layouts key their per-edge point arrays by this id so callers can
   * look up path geometry without reconstructing tuples. When omitted,
   * computeDagrePositions falls back to an internal counter name and
   * the returned edges map is opaque. Legacy computeDagPositions ignores
   * this field entirely.
   */
  id?: string;
  /**
   * Optional semantic edge kind. When "extends", dagre-backed layouts
   * invert the edge internally (child → parent becomes parent → child at
   * the graph level) so the extends parent ranks ABOVE the extending
   * child under rankdir=TB — matching UML "parent above" convention.
   * The exposed edges map still keys by the caller-supplied id and the
   * returned polyline points are re-oriented so pts[0] = semantic source
   * (child) side, pts[last] = semantic target (parent) side; cardinality/
   * label code therefore needs no per-kind branching. Property-ref
   * ("ref"|"items") edges are laid out source → target as usual. Legacy
   * computeDagPositions ignores this field entirely.
   */
  refKind?: "extends" | "ref" | "items";
}

export interface EntityDef {
  properties: Record<string, unknown>;
  extends?: string;
}

export interface DagPositions {
  positions: Record<string, { x: number; y: number }>;
  width: number;
  height: number;
  /**
   * Per-node rank (dagre layouts only). Undefined for legacy DAG layout.
   * Consumers that bucket by rank (e.g. overflow-badge per rank) should
   * fall back safely via (ranks ?? {}).
   */
  ranks?: Record<string, number>;
  /**
   * Y coordinate of each rank's centerline (dagre with rankalign=center,
   * the dagre default). Every node in the same rank shares this center
   * y — reliable anchor for rank-level chrome (badges, separators).
   * Undefined for legacy DAG layout.
   */
  rankCenterY?: Record<number, number>;
  /**
   * Per-edge polyline points as computed by the layout engine
   * (dagre-backed layouts only). Keyed by GraphEdge.id when supplied,
   * else by an internal synthetic name. Points are in the same
   * coordinate space as `positions` (top-left-origin, shared with the
   * SVG canvas). At minimum two points: first near the source
   * connection, last near the target. Consumers should treat missing
   * entries as "no geometry" and fall back to hard-coded offsets.
   */
  edges?: Record<string, { points: { x: number; y: number }[] }>;
}

export interface DagLayoutOpts {
  barycenter?: boolean;
  /**
   * Per-node width resolver. When omitted, every node falls back to
   * DEFAULT_WIDTH (260). Return values are clamped to [MIN_WIDTH, MAX_WIDTH]
   * (with a floor of 1 to avoid NaN/negative widths corrupting the layout).
   */
  widthOf?: (name: string) => number;
}

// Default width derivation. Single source of truth for both layout and render.
// Width depends only on (name, def) — stub state no longer participates so that
// toggling a neighbour between stub and expanded does not reshuffle x packing
// across bands. Stub state continues to drive boxHeight below.
//
// Rendered name in EntityBox is truncated to 29 chars (slice(0, 28) + ellipsis)
// at font-size 13 / weight 600; ~8px/char + 24px padding approximates header
// text width closely enough for a non-clipping column. Current MIN_WIDTH (260)
// >= max headerPx (29*8+24 = 256) so every node clamps to MIN_WIDTH; if
// MIN_WIDTH is ever lowered, x-stability across stub toggle still holds because
// the resolver no longer reads stub state.
export function widthOf(
  name: string,
  def: EntityDef | undefined,
): number {
  if (!def) return DEFAULT_WIDTH;
  const renderedLen = Math.min(name.length, 29);
  const headerPx = renderedLen * 8 + 24;
  return Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, headerPx));
}

export function computeDagPositions(
  list: string[],
  edges: GraphEdge[],
  defs: Record<string, EntityDef>,
  stubs: Set<string>,
  boxSpacing: number,
  bandGap: number,
  opts?: DagLayoutOpts,
): DagPositions {
  const useBarycenter = opts?.barycenter ?? true;

  // Helper: compute box height for an entity
  const boxHeight = (entityName: string): number => {
    const def = defs[entityName];
    if (!def) return HEADER_HEIGHT;
    if (stubs.has(entityName)) return HEADER_HEIGHT;
    const rowCt = Object.keys(def.properties).length + (def.extends ? 1 : 0) || 1;
    return HEADER_HEIGHT + Math.min(rowCt, 10) * ROW_HEIGHT;
  };

  const listSet = new Set(list);

  // Build adjacency maps over all edges
  // childrenOf: target → [sources]  (parent node → its children in the DAG)
  // parentsOf:  source → [targets]  (child node → its parents in the DAG)
  const childrenOf: Record<string, string[]> = {};
  const parentsOf: Record<string, string[]> = {};
  const sourceSet = new Set<string>();

  for (const edge of edges) {
    sourceSet.add(edge.source);

    if (!childrenOf[edge.target]) childrenOf[edge.target] = [];
    childrenOf[edge.target].push(edge.source);

    if (!parentsOf[edge.source]) parentsOf[edge.source] = [];
    parentsOf[edge.source].push(edge.target);
  }

  // Roots = entities that never appear as source (don't reference anything)
  const roots = list.filter(e => !sourceSet.has(e));
  // Pure cycle: treat all entities as roots
  if (roots.length === 0) {
    for (const e of list) roots.push(e);
  }

  // Longest-path BFS for depth assignment
  const depth: Record<string, number> = {};
  for (const e of list) depth[e] = -1;
  for (const r of roots) depth[r] = 0;

  const queue: string[] = [...roots];
  const processCount: Record<string, number> = {};
  const maxIterations = list.length * list.length + 1;
  let iterations = 0;

  while (queue.length > 0 && iterations < maxIterations) {
    iterations++;
    const parent = queue.shift()!;
    const children = childrenOf[parent] || [];
    for (const child of children) {
      if (!listSet.has(child)) continue;
      const newDepth = depth[parent] + 1;
      if (newDepth > depth[child]) {
        depth[child] = newDepth;
        processCount[child] = (processCount[child] || 0) + 1;
        if (processCount[child] <= list.length) {
          queue.push(child);
        }
      }
    }
  }

  // Entities not reachable from any root: assign depth 0
  for (const e of list) {
    if (depth[e] < 0) depth[e] = 0;
  }

  // Group entities by depth band, preserving visibleList order within each band
  const maxDepth = Math.max(0, ...Object.values(depth));
  const bands: string[][] = [];
  for (let d = 0; d <= maxDepth; d++) bands.push([]);
  for (const e of list) {
    bands[depth[e]].push(e);
  }

  // ── Barycenter x-ordering ──────────────────────────────────────────────────
  // Reorders nodes within each band to reduce edge crossings.
  // Only cross-band edges (depth differs by exactly 1) participate in scoring.
  // Short-circuit: skip when there is only one band.
  if (useBarycenter && bands.length > 1) {
    // Original visibleList index: secondary sort key for stable tiebreak
    const origIndex: Record<string, number> = {};
    for (let i = 0; i < list.length; i++) origIndex[list[i]] = i;

    // Restrict adjacency maps to cross-band edges (|depth diff| === 1)
    // xParentsOf[node]  = neighbours in band(depth[node] - 1)  → used in top-down sweep
    // xChildrenOf[node] = neighbours in band(depth[node] + 1)  → used in bottom-up sweep
    const xChildrenOf: Record<string, string[]> = {};
    const xParentsOf: Record<string, string[]> = {};
    for (const edge of edges) {
      if (!listSet.has(edge.source) || !listSet.has(edge.target)) continue;
      if (Math.abs(depth[edge.source] - depth[edge.target]) !== 1) continue;
      // edge.target is the "parent" (lower depth), edge.source is the "child"
      if (!xChildrenOf[edge.target]) xChildrenOf[edge.target] = [];
      xChildrenOf[edge.target].push(edge.source);
      if (!xParentsOf[edge.source]) xParentsOf[edge.source] = [];
      xParentsOf[edge.source].push(edge.target);
    }

    // sortedIndex[entity] = current rank within its band
    const sortedIndex: Record<string, number> = {};
    for (let d = 0; d <= maxDepth; d++) {
      for (let i = 0; i < bands[d].length; i++) {
        sortedIndex[bands[d][i]] = i;
      }
    }

    // Mean sortedIndex of neighbours; NaN when node has no eligible neighbours
    const barycenter = (node: string, neighbours: string[] | undefined): number => {
      const nbrs = (neighbours || []).filter(n => listSet.has(n));
      if (nbrs.length === 0) return NaN;
      let sum = 0;
      for (const n of nbrs) sum += sortedIndex[n];
      return sum / nbrs.length;
    };

    // Sort one band in-place by barycenter score; update sortedIndex
    const sortBand = (band: string[], neighboursOf: (n: string) => string[] | undefined) => {
      const scored = band.map(node => ({
        node,
        key: barycenter(node, neighboursOf(node)),
        orig: origIndex[node],
      }));
      scored.sort((a, b) => {
        // NaN guard: isolated nodes fall back to original list order
        const ak = isNaN(a.key) ? a.orig : a.key;
        const bk = isNaN(b.key) ? b.orig : b.key;
        if (ak !== bk) return ak - bk;
        return a.orig - b.orig; // stable secondary key
      });
      for (let i = 0; i < scored.length; i++) {
        band[i] = scored[i].node;
        sortedIndex[scored[i].node] = i;
      }
    };

    // 3 sweep pairs (top-down + bottom-up) = 6 passes total
    const SWEEP_PAIRS = 3;
    for (let pass = 0; pass < SWEEP_PAIRS; pass++) {
      // Top-down: sort band[d] by positions of neighbours in band[d-1]
      for (let d = 1; d <= maxDepth; d++) {
        sortBand(bands[d], n => xParentsOf[n]);
      }
      // Bottom-up: sort band[d] by positions of neighbours in band[d+1]
      for (let d = maxDepth - 1; d >= 0; d--) {
        sortBand(bands[d], n => xChildrenOf[n]);
      }
    }
  }

  // Compute band heights (max box height in each band)
  const bandHeights = bands.map(band =>
    band.length > 0 ? Math.max(...band.map(e => boxHeight(e))) : 0,
  );

  // Compute Y offset per band (cumulative sum)
  const bandY: number[] = [];
  let cumY = PAD;
  for (let d = 0; d <= maxDepth; d++) {
    bandY.push(cumY);
    cumY += bandHeights[d] + bandGap;
  }

  // Per-node width resolver. Clamped to [1, MAX_WIDTH] to guard against NaN or
  // negative values corrupting the cumulative sum. Falls back to DEFAULT_WIDTH
  // when no resolver is supplied (preserves legacy uniform layout).
  const resolveWidth = (name: string): number => {
    const raw = opts?.widthOf ? opts.widthOf(name) : DEFAULT_WIDTH;
    if (!Number.isFinite(raw)) return DEFAULT_WIDTH;
    return Math.max(1, Math.min(MAX_WIDTH, raw));
  };

  // Pack x positions cumulatively per band: x[0]=PAD; x[i]=x[i-1]+w[i-1]+NODE_GAP.
  // Record last-node right edge per band to compute overall SVG width below.
  const positions: Record<string, { x: number; y: number }> = {};
  const bandRightEdges: number[] = [];
  for (let d = 0; d <= maxDepth; d++) {
    let x = PAD;
    let rightEdge = 0;
    for (let i = 0; i < bands[d].length; i++) {
      const name = bands[d][i];
      const w = resolveWidth(name);
      positions[name] = { x, y: bandY[d] };
      rightEdge = x + w;
      x = x + w + NODE_GAP;
    }
    bandRightEdges.push(rightEdge);
  }

  // SVG width: widest band's right edge + PAD. Math.max(1, ...) sentinel keeps
  // empty-layout width positive (matches legacy behaviour for the empty-list
  // test). Non-empty bands contribute rightEdge+PAD.
  const width = Math.max(
    1,
    ...bandRightEdges.filter(e => e > 0).map(e => e + PAD),
  );
  const height = Math.max(400, cumY + PAD);

  // Silence unused-parameter lint: boxSpacing is retained on the signature for
  // backward compatibility with standalone callers but is no longer consulted.
  void boxSpacing;

  return { positions, width, height };
}

// ── 2-hop neighborhood partition ────────────────────────────────────────────
// Minimal property shape used by the partition helpers. We accept any
// property record whose values MAY carry ref_name / items_ref strings.
// Unknown or missing values are treated as non-referential.
export interface PartitionProperty {
  ref_name?: string | null;
  items_ref?: string | null;
}

export interface PartitionEntityDef {
  properties: Record<string, PartitionProperty | unknown>;
  extends?: string;
}

export interface TwoHopPartition {
  /** Focals ∪ nodes reachable within maxHop (hop <= maxHop). */
  visible: Set<string>;
  /** Min-hop distance from the focal set; only entries for visible nodes. */
  hopOf: Record<string, number>;
  /** Nodes at exactly hop === maxHop + 1 (the immediate overflow ring). */
  hiddenRing: Set<string>;
}

// Extract outgoing refs for one def: extends (if present) + every property's
// ref_name / items_ref. extends lives on the def itself, NOT in properties —
// accessing def.properties['extends'] returns undefined and its .ref_name
// would throw. Traverse def.extends separately.
export const outgoingRefs = (def: PartitionEntityDef | undefined): string[] => {
  if (!def) return [];
  const out: string[] = [];
  if (def.extends) out.push(def.extends);
  for (const prop of Object.values(def.properties)) {
    if (!prop || typeof prop !== 'object') continue;
    const p = prop as PartitionProperty;
    if (p.ref_name) out.push(p.ref_name);
    if (p.items_ref) out.push(p.items_ref);
  }
  return out;
};

// Build inbound adjacency (target → [sources]) once over all defs. Inbound
// edges mirror outbound: if A.extends === B or any A.prop refs B, then
// inbound[B] includes A. Unknown ref targets (dangling refs to names not in
// defs) are kept — BFS filters by def presence per step. Self-refs are
// excluded so a schema cannot be its own inbound parent.
export const buildInboundAdj = (defs: Record<string, PartitionEntityDef>): Record<string, string[]> => {
  const inbound: Record<string, string[]> = {};
  for (const [name, def] of Object.entries(defs)) {
    for (const ref of outgoingRefs(def)) {
      if (ref === name) continue; // self-ref — don't count as inbound edge
      if (!inbound[ref]) inbound[ref] = [];
      inbound[ref].push(name);
    }
  }
  return inbound;
};

/**
 * Rank schemas by inbound-reference count and return the top N names.
 *
 * Inbound count for a schema X = number of distinct (sourceDef, ref) edges
 * (extends + ref_name + items_ref) that target X across `defs`. Self-refs are
 * NOT counted (mirrors buildInboundAdj). Multiple properties on one source
 * that all ref X each contribute one edge — matches the inbound-adj list
 * length used elsewhere for neighborhood traversal.
 *
 * Sort: inbound count DESC, ties broken by name ASC. Deterministic.
 *
 * Edge cases:
 *   - n <= 0  → returns []
 *   - empty defs → returns []
 *   - n > defs count → returns all defs ranked (no padding)
 *   - all-zero-inbound → all defs sorted by name asc, sliced to n
 *
 * Used by SchemasPage to suggest entry-point schemas in the empty-state
 * graph view (no focal selected yet).
 */
export function computeTopHubs(
  defs: Record<string, PartitionEntityDef>,
  n: number,
): string[] {
  if (n <= 0) return [];
  const names = Object.keys(defs);
  if (names.length === 0) return [];
  const inbound = buildInboundAdj(defs);
  const counts: Array<{ name: string; count: number }> = names.map(name => ({
    name,
    count: (inbound[name] || []).length,
  }));
  counts.sort((a, b) => {
    if (a.count !== b.count) return b.count - a.count; // count DESC
    return a.name.localeCompare(b.name);                // name ASC
  });
  return counts.slice(0, n).map(c => c.name);
}

/**
 * Partition a schema graph into focal-set + 2-hop visible neighborhood +
 * the immediate overflow ring (hop === maxHop + 1). Traverses bidirectionally
 * over outbound property refs + def.extends, inbound mirrors.
 *
 * Multi-source BFS: min-hop wins across focals; a focal already reachable
 * from another focal stays at hop 0 (focals always hop 0). Self-refs and
 * dangling refs to unknown defs are safe (no infinite loops, no throws).
 */
export function computeTwoHopPartition(
  defs: Record<string, PartitionEntityDef>,
  focalSet: Set<string>,
  maxHop: number,
): TwoHopPartition {
  const hopOf: Record<string, number> = {};
  const visible = new Set<string>();
  const hiddenRing = new Set<string>();

  if (focalSet.size === 0) {
    return { visible, hopOf, hiddenRing };
  }

  const inbound = buildInboundAdj(defs);

  // Seed: all focals at hop 0 (focals beat any other hop assignment).
  let frontier = new Set<string>();
  for (const f of focalSet) {
    hopOf[f] = 0;
    visible.add(f);
    frontier.add(f);
  }

  for (let hop = 1; hop <= maxHop + 1; hop++) {
    const next = new Set<string>();
    for (const n of frontier) {
      // Outbound neighbours
      const outs = outgoingRefs(defs[n]);
      // Inbound neighbours
      const ins = inbound[n] || [];
      const neighbours = [...outs, ...ins];
      for (const nb of neighbours) {
        if (nb === n) continue;
        if (hopOf[nb] !== undefined) continue; // already assigned (min-hop wins)
        hopOf[nb] = hop;
        if (hop <= maxHop) {
          visible.add(nb);
          next.add(nb);
        } else {
          // hop === maxHop + 1 — immediate overflow ring
          hiddenRing.add(nb);
        }
      }
    }
    frontier = next;
    if (frontier.size === 0) break;
  }

  return { visible, hopOf, hiddenRing };
}

/**
 * BFS over the full (undirected) schema graph from the focal set, unbounded.
 * Returns min-hop depth for every reachable def. Unreachable defs are absent
 * from the result. Focals are at depth 0.
 */
export function computeFullGraphDepths(
  defs: Record<string, PartitionEntityDef>,
  focals: Set<string>,
): Record<string, number> {
  const depths: Record<string, number> = {};
  if (focals.size === 0) return depths;

  const inbound = buildInboundAdj(defs);

  let frontier = new Set<string>();
  for (const f of focals) {
    depths[f] = 0;
    frontier.add(f);
  }

  let hop = 0;
  while (frontier.size > 0) {
    hop++;
    const next = new Set<string>();
    for (const n of frontier) {
      const outs = outgoingRefs(defs[n]);
      const ins = inbound[n] || [];
      for (const nb of [...outs, ...ins]) {
        if (nb === n) continue;
        if (depths[nb] !== undefined) continue;
        depths[nb] = hop;
        next.add(nb);
      }
    }
    frontier = next;
  }

  return depths;
}

/**
 * Bucket hidden schemas by depth band. Each hidden name goes into the band
 * of its (fullDepth - 1) — i.e. the band of the visible node that first
 * reaches it via BFS. Bands with zero hidden are absent from the result.
 *
 * Empty `hidden` → empty result. Hidden names without a depth entry in
 * `fullDepths` (disconnected islands) are skipped — they are not reachable
 * from the focal set and therefore not part of the overflow ring.
 */
export function bucketHiddenByBand(
  hidden: Set<string>,
  fullDepths: Record<string, number>,
): Record<number, string[]> {
  const buckets: Record<number, string[]> = {};
  for (const name of hidden) {
    const d = fullDepths[name];
    if (d === undefined || d <= 0) continue;
    const band = d - 1; // band of the visible parent that referenced this node
    if (!buckets[band]) buckets[band] = [];
    buckets[band].push(name);
  }
  return buckets;
}

// Endpoint pseudo-node key prefix. Schemas in real specs cannot collide with
// "ep::" because schema names follow Swagger identifier rules (letters,
// digits, _ and . only — no colons). Lowercase method matches backend
// EndpointEdge.method casing (entity_graph.rs ~264).
export const ENDPOINT_KEY_PREFIX = "ep::";

export interface EndpointEdgeLike {
  endpoint: { method: string; path: string };
  target_def: string;
  direction?: string;
}

/**
 * Append endpoint pseudo-nodes to a visible-defs list when the endpoint
 * layer is on. Pure: no Solid signals, no DOM. Both empty-state hub and
 * focused-graph variants share this so the keying / dedup logic stays
 * single-source.
 *
 * - layerOn === false → returns baseList UNCHANGED (byte-identical OFF
 *   path; downstream layout/render see no ep:: keys, no jitter risk).
 * - Includes an endpoint only when at least one of its edges targets a
 *   def already present in baseList (acceptance: hide endpoints whose
 *   linked schemas aren't on screen).
 * - Dedupes via Set keyed `ep::${method}::${path}`; an endpoint with both
 *   input AND output edges yields a single pseudo-node.
 * - Concatenates after baseList so stable def positions stay first
 *   (downstream <For> referential identity for string elements is
 *   value-equal — safe — but order stability still helps debugging).
 */
export function appendEndpointPseudoNodes(
  baseList: string[],
  endpointEdges: EndpointEdgeLike[],
  layerOn: boolean,
): string[] {
  if (!layerOn) return baseList;
  const baseSet = new Set(baseList);
  const seen = new Set<string>();
  const epKeys: string[] = [];
  for (const e of endpointEdges) {
    if (!baseSet.has(e.target_def)) continue;
    const key = `${ENDPOINT_KEY_PREFIX}${e.endpoint.method}::${e.endpoint.path}`;
    if (seen.has(key)) continue;
    seen.add(key);
    epKeys.push(key);
  }
  if (epKeys.length === 0) return baseList;
  return baseList.concat(epKeys);
}

/**
 * Parse an endpoint pseudo-node key back into (method, path). Returns
 * null when name is not an endpoint key. Method is lowercase per backend.
 */
export function parseEndpointKey(name: string): { method: string; path: string } | null {
  if (!name.startsWith(ENDPOINT_KEY_PREFIX)) return null;
  const rest = name.slice(ENDPOINT_KEY_PREFIX.length);
  const sep = rest.indexOf("::");
  if (sep < 0) return null;
  return { method: rest.slice(0, sep), path: rest.slice(sep + 2) };
}
