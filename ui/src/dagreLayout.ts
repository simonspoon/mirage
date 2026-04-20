// dagreLayout.ts — dagre-backed layout returning the same DagPositions shape
// as computeDagPositions, so callers (edge router, fit-to-viewport, EntityBox
// render) can swap engines without rewiring downstream code.
//
// dagre returns CENTER coordinates per node; existing renderer expects TOP-LEFT.
// We convert in the final pass: x = node.x - width/2, y = node.y - height/2.

import dagre from "@dagrejs/dagre";
import {
  DEFAULT_WIDTH,
  HEADER_HEIGHT,
  MAX_WIDTH,
  PAD,
  ROW_HEIGHT,
  type DagLayoutOpts,
  type DagPositions,
  type EntityDef,
  type GraphEdge,
} from "./dagLayout";

export interface DagreLayoutOpts extends DagLayoutOpts {
  /** Layout direction. Default "TB" matches the existing top-down DAG. */
  rankdir?: "TB" | "LR" | "BT" | "RL";
  /** Horizontal spacing between siblings within a rank. Default 40. */
  nodesep?: number;
  /** Vertical spacing between ranks. Default 60. */
  ranksep?: number;
}

export function computeDagrePositions(
  list: string[],
  edges: GraphEdge[],
  defs: Record<string, EntityDef>,
  stubs: Set<string>,
  bandGap: number,
  opts?: DagreLayoutOpts,
): DagPositions {
  const rankdir = opts?.rankdir ?? "TB";
  const nodesep = opts?.nodesep ?? 40;
  const ranksep = opts?.ranksep ?? bandGap;

  const resolveWidth = (name: string): number => {
    const raw = opts?.widthOf ? opts.widthOf(name) : DEFAULT_WIDTH;
    if (!Number.isFinite(raw)) return DEFAULT_WIDTH;
    return Math.max(1, Math.min(MAX_WIDTH, raw));
  };

  const heightOf = (name: string): number => {
    const def = defs[name];
    if (!def) return HEADER_HEIGHT;
    if (stubs.has(name)) return HEADER_HEIGHT;
    const rowCt = Object.keys(def.properties).length + (def.extends ? 1 : 0) || 1;
    return HEADER_HEIGHT + Math.min(rowCt, 10) * ROW_HEIGHT;
  };

  const g = new dagre.graphlib.Graph({ multigraph: true });
  g.setGraph({
    rankdir,
    nodesep,
    ranksep,
    marginx: PAD,
    marginy: PAD,
  });
  g.setDefaultEdgeLabel(() => ({}));

  const listSet = new Set(list);
  for (const name of list) {
    g.setNode(name, { width: resolveWidth(name), height: heightOf(name) });
  }

  // Use a per-edge unique name so multigraph keeps parallel edges distinct
  // (e.g. two properties on the same source both refing the same target).
  // Prefer caller-supplied GraphEdge.id (stable, meaningful key for point
  // lookup downstream) with a de-dup counter suffix if the same id appears
  // more than once; fall back to a synthetic name when id is omitted.
  //
  // refKind === "extends" inversion: at the dagre graph level we set the
  // edge as target → source (parent → child) so dagre's rank assignment
  // places the extends parent ABOVE the extending child under rankdir=TB.
  // The caller-supplied id (keyed on child::extends::parent) is preserved
  // verbatim; the retrieval loop below uses matching {v,w} so points come
  // back, and re-orients the polyline so pts[0] stays near the semantic
  // source (child) and pts[last] near the semantic target (parent).
  let edgeIdx = 0;
  const usedNames = new Set<string>();
  const edgeNameOf = new Map<GraphEdge, string>();
  const invertedEdges = new Set<GraphEdge>();
  for (const e of edges) {
    if (!listSet.has(e.source) || !listSet.has(e.target)) continue;
    if (e.source === e.target) continue; // dagre rejects self-loops in layout
    let name = e.id ?? `e${edgeIdx++}`;
    if (usedNames.has(name)) {
      // Preserve uniqueness without losing the caller-provided id prefix.
      let k = 1;
      while (usedNames.has(`${name}#${k}`)) k++;
      name = `${name}#${k}`;
    }
    usedNames.add(name);
    edgeNameOf.set(e, name);
    if (e.refKind === "extends") {
      invertedEdges.add(e);
      g.setEdge(e.target, e.source, {}, name);
    } else {
      g.setEdge(e.source, e.target, {}, name);
    }
  }

  dagre.layout(g);

  // dagre populates node.rank (int, 0-based after normalizeRanks) and node.y
  // (center y). Under default rankalign="center" every node in the same rank
  // shares the same center y — that y is the rank centerline, the reliable
  // anchor for rank-level chrome (overflow badges, etc.). If a future call
  // passes rankalign="top"|"bottom" the rankCenterY map stops being a single
  // shared y per rank; revisit this block if we ever flip rankalign.
  const positions: Record<string, { x: number; y: number }> = {};
  const ranks: Record<string, number> = {};
  const rankCenterY: Record<number, number> = {};
  for (const name of list) {
    const node = g.node(name);
    if (!node) continue;
    positions[name] = {
      x: node.x - node.width / 2,
      y: node.y - node.height / 2,
    };
    if (typeof node.rank === "number") {
      ranks[name] = node.rank;
      // Same rank → same node.y under rankalign=center, so blind overwrite
      // is safe. First-writer-wins also acceptable; pick blind for brevity.
      rankCenterY[node.rank] = node.y;
    }
  }

  // Collect dagre's computed polyline per edge. dagre.edge({v,w,name}).points
  // is an array of absolute-coord {x,y} (same coord basis as node.x/node.y,
  // which is the global SVG canvas frame — node top-left conversion above
  // does NOT shift the coord system, so edge points need no adjustment).
  // First and last points sit near source/target connection; intermediates
  // are bend points introduced by dagre's edge router. We rely on this to
  // anchor edge labels (midpoint + cardinality) against true layout geometry
  // rather than hard-coded pixel offsets that drift on back-routed or
  // detour edges.
  const edgesOut: Record<string, { points: { x: number; y: number }[] }> = {};
  for (const [inputEdge, name] of edgeNameOf) {
    const inverted = invertedEdges.has(inputEdge);
    // Inverted (extends) edges were registered as target → source at
    // graph level, so the retrieval {v,w} must match that registration
    // or g.edge() returns undefined and the polyline is lost.
    const v = inverted ? inputEdge.target : inputEdge.source;
    const w = inverted ? inputEdge.source : inputEdge.target;
    const eObj = g.edge({ v, w, name });
    if (!eObj || !Array.isArray(eObj.points)) continue;
    const pts = eObj.points
      .filter((p: { x?: unknown; y?: unknown }) =>
        Number.isFinite(p.x) && Number.isFinite(p.y),
      )
      .map((p: { x: number; y: number }) => ({ x: p.x, y: p.y }));
    if (pts.length === 0) continue;
    // For inverted (extends) edges, dagre's polyline runs parent → child
    // (pts[0] near parent, pts[last] near child). Reverse so downstream
    // consumers can treat pts[0] as always near the semantic source
    // (child side for extends) and pts[last] as the semantic target
    // (parent side for extends) — keeps cardinality-label and arrow-
    // marker code uniform across edge kinds.
    edgesOut[name] = { points: inverted ? pts.slice().reverse() : pts };
  }

  const graph = g.graph();
  const width = Math.max(1, (graph.width ?? 0) + PAD);
  const height = Math.max(400, (graph.height ?? 0) + PAD);

  return { positions, width, height, ranks, rankCenterY, edges: edgesOut };
}
