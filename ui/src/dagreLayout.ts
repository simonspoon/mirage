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
  let edgeIdx = 0;
  for (const e of edges) {
    if (!listSet.has(e.source) || !listSet.has(e.target)) continue;
    if (e.source === e.target) continue; // dagre rejects self-loops in layout
    g.setEdge(e.source, e.target, {}, `e${edgeIdx++}`);
  }

  dagre.layout(g);

  const positions: Record<string, { x: number; y: number }> = {};
  for (const name of list) {
    const node = g.node(name);
    if (!node) continue;
    positions[name] = {
      x: node.x - node.width / 2,
      y: node.y - node.height / 2,
    };
  }

  const graph = g.graph();
  const width = Math.max(1, (graph.width ?? 0) + PAD);
  const height = Math.max(400, (graph.height ?? 0) + PAD);

  return { positions, width, height };
}
