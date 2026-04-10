import { createSignal, createEffect, onCleanup, Index, For, Show } from "solid-js";

// --- Types ---

interface SimNode {
  id: string;
  x: number;
  y: number;
  vx: number;
  vy: number;
  fx: number | null;
  fy: number | null;
  hw: number; // half-width of pill
  hh: number; // half-height of pill
  isRoot: boolean;
  isShared: boolean;
}

/** Snapshot of node state for rendering (new object each frame so SolidJS detects changes) */
interface RenderNode {
  id: string;
  x: number;
  y: number;
  hw: number;
  hh: number;
  isRoot: boolean;
  isShared: boolean;
  pinned: boolean;
}

interface RenderEdge {
  x1: number; y1: number;
  x2: number; y2: number;
  source: string; target: string;
}

interface Particle {
  linkIdx: number;
  t: number;
  speed: number;
}

interface ForceGraphProps {
  nodes: string[];
  edges: Record<string, string[]>;
  roots: Record<string, { method: string; path: string }[]>;
  shared: string[];
  selectedEntity: string | null;
  onSelectEntity: (name: string | null) => void;
}

// --- Constants ---

const PILL_H = 24;
const PILL_HH = PILL_H / 2;
const PILL_PAD = 14; // horizontal padding around text

// --- Helpers ---

function estimateLabelWidth(text: string, fontSize: number): number {
  // Monospace: each char is roughly 0.6 * fontSize
  return text.length * fontSize * 0.6 + PILL_PAD * 2;
}

function clamp(val: number, min: number, max: number) {
  return Math.max(min, Math.min(max, val));
}

/** Find where a ray from (cx,cy) toward (tx,ty) exits a rect of half-size (hw, hh). */
function pillEdge(cx: number, cy: number, tx: number, ty: number, hw: number, hh: number): { x: number; y: number } {
  const dx = tx - cx;
  const dy = ty - cy;
  if (dx === 0 && dy === 0) return { x: cx, y: cy };

  // Scale factors to each boundary
  const tX = dx !== 0 ? hw / Math.abs(dx) : Infinity;
  const tY = dy !== 0 ? hh / Math.abs(dy) : Infinity;
  const t = Math.min(tX, tY);

  return { x: cx + dx * t, y: cy + dy * t };
}

// --- Component ---

export default function ForceGraph(props: ForceGraphProps) {
  let svgRef: SVGSVGElement | undefined;
  let simNodes: SimNode[] = [];
  let simLinks: { source: string; target: string }[] = [];
  let particles: Particle[] = [];
  let alpha = 1.0;
  let animId = 0;
  let dragging: SimNode | null = null;
  let didDrag = false;
  let panning = false;
  let panStart = { x: 0, y: 0 };

  const [renderNodes, setRenderNodes] = createSignal<RenderNode[]>([]);
  const [renderEdges, setRenderEdges] = createSignal<RenderEdge[]>([]);
  const [renderParticles, setRenderParticles] = createSignal<{ x: number; y: number; opacity: number }[]>([]);
  const [transform, setTransform] = createSignal({ x: 0, y: 0, scale: 1 });

  // Build simulation state when props change
  createEffect(() => {
    const nodeNames = props.nodes;
    const edges = props.edges;
    if (!nodeNames.length) {
      simNodes = [];
      simLinks = [];
      particles = [];
      setRenderNodes([]);
      setRenderEdges([]);
      setRenderParticles([]);
      return;
    }

    const existing = new Map(simNodes.map(n => [n.id, n]));
    const rootSet = new Set(Object.keys(props.roots));
    const sharedSet = new Set(props.shared);

    simNodes = nodeNames.map((name, i) => {
      const prev = existing.get(name);
      const fontSize = rootSet.has(name) ? 12 : 11;
      const hw = estimateLabelWidth(name, fontSize) / 2;
      if (prev) {
        prev.hw = hw;
        prev.hh = PILL_HH;
        prev.isRoot = rootSet.has(name);
        prev.isShared = sharedSet.has(name);
        return prev;
      }
      const angle = (2 * Math.PI * i) / nodeNames.length;
      const spread = Math.min(500, 150 + nodeNames.length * 35);
      return {
        id: name,
        x: 400 + spread * Math.cos(angle) + (Math.random() - 0.5) * 30,
        y: 300 + spread * Math.sin(angle) + (Math.random() - 0.5) * 30,
        vx: 0, vy: 0,
        fx: null, fy: null,
        hw, hh: PILL_HH,
        isRoot: rootSet.has(name),
        isShared: sharedSet.has(name),
      };
    });

    simLinks = [];
    for (const [src, targets] of Object.entries(edges)) {
      for (const tgt of targets) {
        if (nodeNames.includes(src) && nodeNames.includes(tgt)) {
          simLinks.push({ source: src, target: tgt });
        }
      }
    }

    particles = simLinks.map((_, i) => ({
      linkIdx: i,
      t: (i * 0.23) % 1,
      speed: 0.003 + Math.random() * 0.002,
    }));

    alpha = 1.0;
  });

  // Reheat on selection change
  createEffect(() => {
    props.selectedEntity; // track
    alpha = Math.max(alpha, 0.3);
  });

  // --- Simulation tick ---

  function tick() {
    const nodes = simNodes;
    const linkList = simLinks;
    if (nodes.length === 0) {
      animId = requestAnimationFrame(tick);
      return;
    }

    const nodeMap = new Map(nodes.map(n => [n.id, n]));

    // Build connected-pair lookup
    const connected = new Set<string>();
    for (const link of linkList) {
      const key = link.source < link.target
        ? `${link.source}\0${link.target}`
        : `${link.target}\0${link.source}`;
      connected.add(key);
    }
    function isConnected(a: string, b: string): boolean {
      const key = a < b ? `${a}\0${b}` : `${b}\0${a}`;
      return connected.has(key);
    }

    // ALL forces scaled by alpha uniformly — equilibrium position never shifts,
    // alpha only controls how fast nodes settle.

    // Center gravity
    const cx = 400, cy = 300;
    for (const n of nodes) {
      if (n.fx != null) continue;
      n.vx += (cx - n.x) * 0.004 * alpha;
      n.vy += (cy - n.y) * 0.004 * alpha;
    }

    // All pairs: repulsion (1/r) + spring for connected pairs
    // ALL forces × alpha so equilibrium never shifts
    for (let i = 0; i < nodes.length; i++) {
      for (let j = i + 1; j < nodes.length; j++) {
        const a = nodes[i], b = nodes[j];
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let dist = Math.sqrt(dx * dx + dy * dy) || 1;
        const minDist = a.hw + b.hw + 20;
        if (dist < minDist) dist = minDist;

        // Repulsion — ALL pairs, 1/r
        const repel = 500 / dist * alpha;
        const rx = (dx / dist) * repel;
        const ry = (dy / dist) * repel;
        if (a.fx == null) { a.vx -= rx; a.vy -= ry; }
        if (b.fx == null) { b.vx += rx; b.vy += ry; }

        // Spring — connected pairs only, on top of repulsion
        if (isConnected(a.id, b.id)) {
          const idealDist = Math.max(240, (a.hw + b.hw) * 1.3 + 60);
          const spring = (dist - idealDist) * 0.12 * alpha;
          const sx = (dx / dist) * spring;
          const sy = (dy / dist) * spring;
          if (a.fx == null) { a.vx += sx; a.vy += sy; }
          if (b.fx == null) { b.vx -= sx; b.vy -= sy; }
        }
      }
    }

    // Apply velocity + damping
    const damping = 0.82;
    for (const n of nodes) {
      if (n.fx != null) {
        n.x = n.fx;
        n.y = n.fy!;
        n.vx = 0;
        n.vy = 0;
      } else {
        n.vx *= damping;
        n.vy *= damping;
        n.x += n.vx;
        n.y += n.vy;
      }
    }

    // Alpha decay — settles to zero so simulation stops jittering
    alpha *= 0.995;
    if (alpha < 0.001) alpha = 0;

    // Advance particles
    for (const p of particles) {
      p.t += p.speed;
      if (p.t > 1) p.t -= 1;
    }

    // --- Build render snapshots (new objects so SolidJS detects changes) ---
    const nodeSnaps: RenderNode[] = nodes.map(n => ({
      id: n.id, x: n.x, y: n.y, hw: n.hw, hh: n.hh,
      isRoot: n.isRoot, isShared: n.isShared, pinned: n.fx != null,
    }));

    const edgeSnaps: RenderEdge[] = linkList.map(l => {
      const s = nodeMap.get(l.source);
      const t = nodeMap.get(l.target);
      if (!s || !t) return { x1: 0, y1: 0, x2: 0, y2: 0, source: l.source, target: l.target };
      const start = pillEdge(s.x, s.y, t.x, t.y, s.hw, s.hh);
      const end = pillEdge(t.x, t.y, s.x, s.y, t.hw, t.hh);
      return { x1: start.x, y1: start.y, x2: end.x, y2: end.y, source: l.source, target: l.target };
    });

    const particleSnaps = particles.map(p => {
      const e = edgeSnaps[p.linkIdx];
      if (!e) return { x: 0, y: 0, opacity: 0 };
      return {
        x: e.x1 + (e.x2 - e.x1) * p.t,
        y: e.y1 + (e.y2 - e.y1) * p.t,
        opacity: Math.sin(p.t * Math.PI) * 0.6,
      };
    });

    setRenderNodes(nodeSnaps);
    setRenderEdges(edgeSnaps);
    setRenderParticles(particleSnaps);

    animId = requestAnimationFrame(tick);
  }

  // Start animation
  createEffect(() => {
    if (props.nodes.length > 0) {
      animId = requestAnimationFrame(tick);
    }
  });

  onCleanup(() => cancelAnimationFrame(animId));

  // --- Interaction ---

  function svgPoint(e: MouseEvent): { x: number; y: number } {
    const rect = svgRef!.getBoundingClientRect();
    const t = transform();
    return {
      x: (e.clientX - rect.left - t.x) / t.scale,
      y: (e.clientY - rect.top - t.y) / t.scale,
    };
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const rect = svgRef!.getBoundingClientRect();
    const t = transform();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    const factor = e.deltaY > 0 ? 0.92 : 1.08;
    const newScale = clamp(t.scale * factor, 0.15, 4);
    const nx = mx - (mx - t.x) * (newScale / t.scale);
    const ny = my - (my - t.y) * (newScale / t.scale);
    setTransform({ x: nx, y: ny, scale: newScale });
  }

  function findNodeAt(pt: { x: number; y: number }): SimNode | null {
    for (const n of [...simNodes].reverse()) {
      if (Math.abs(pt.x - n.x) < n.hw && Math.abs(pt.y - n.y) < n.hh) return n;
    }
    return null;
  }

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0) return;
    const pt = svgPoint(e);
    const node = findNodeAt(pt);
    if (node) {
      dragging = node;
      didDrag = false;
      node.fx = node.x;
      node.fy = node.y;
      alpha = Math.max(alpha, 0.15);
      svgRef?.setPointerCapture(e.pointerId);
    } else {
      panning = true;
      panStart = { x: e.clientX - transform().x, y: e.clientY - transform().y };
    }
  }

  function onPointerMove(e: PointerEvent) {
    if (dragging) {
      didDrag = true;
      const pt = svgPoint(e);
      dragging.fx = pt.x;
      dragging.fy = pt.y;
      alpha = Math.max(alpha, 0.08);
    } else if (panning) {
      setTransform(t => ({ ...t, x: e.clientX - panStart.x, y: e.clientY - panStart.y }));
    }
  }

  function onPointerUp(e: PointerEvent) {
    if (dragging) {
      if (!didDrag) {
        // Click (no movement) on a node — select it if it's not already selected
        const sel = props.selectedEntity;
        if (dragging.id !== sel) {
          props.onSelectEntity(dragging.id);
        }
      }
      // Unpin after drag or click
      dragging.fx = null;
      dragging.fy = null;
      dragging = null;
    }
    panning = false;
  }

  function onDblClick(e: MouseEvent) {
    // no-op — selection handled in pointerUp
  }

  function fitGraph() {
    if (simNodes.length === 0) return;
    const rect = svgRef!.getBoundingClientRect();
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    for (const n of simNodes) {
      minX = Math.min(minX, n.x - n.hw);
      maxX = Math.max(maxX, n.x + n.hw);
      minY = Math.min(minY, n.y - n.hh);
      maxY = Math.max(maxY, n.y + n.hh);
    }
    const gw = maxX - minX + 80;
    const gh = maxY - minY + 80;
    const scale = Math.min(rect.width / gw, rect.height / gh, 2);
    const cxg = (minX + maxX) / 2;
    const cyg = (minY + maxY) / 2;
    setTransform({
      x: rect.width / 2 - cxg * scale,
      y: rect.height / 2 - cyg * scale,
      scale,
    });
  }

  // --- Render ---

  const sel = () => props.selectedEntity;

  return (
    <div class="relative w-full h-full">
      {/* Controls */}
      <div class="absolute top-2 right-2 z-10 flex gap-1">
        <button
          class="px-2 py-1 text-[10px] bg-gray-800/80 hover:bg-gray-700/80 text-gray-300 rounded backdrop-blur-sm border border-gray-700/50"
          onClick={fitGraph}
        >Fit</button>
        <button
          class="px-2 py-1 text-[10px] bg-gray-800/80 hover:bg-gray-700/80 text-gray-300 rounded backdrop-blur-sm border border-gray-700/50"
          onClick={() => { alpha = 1.0; }}
        >Shake</button>
      </div>

      {/* Legend */}
      <div class="absolute bottom-2 left-2 z-10 bg-[#0a0f1e]/90 backdrop-blur-sm border border-gray-800/50 rounded px-3 py-2 space-y-1.5" style="pointer-events: none;">
        <div class="flex items-center gap-2">
          <div class="w-5 h-0 border-t-[1.5px] border-blue-500" />
          <span class="text-[9px] text-gray-400">References (outbound)</span>
        </div>
        <div class="flex items-center gap-2">
          <div class="w-5 h-0 border-t-[1.5px] border-dashed border-blue-500" />
          <span class="text-[9px] text-gray-400">Referenced by (inbound)</span>
        </div>
        <div class="flex items-center gap-2">
          <div class="w-4 h-2.5 rounded-sm bg-[#0f1d33] border border-blue-700" />
          <span class="text-[9px] text-gray-400">Root entity</span>
        </div>
        <div class="flex items-center gap-2">
          <div class="w-4 h-2.5 rounded-sm bg-[#2d1f0e] border border-yellow-700" />
          <span class="text-[9px] text-gray-400">Shared entity</span>
        </div>
        <div class="flex items-center gap-2">
          <div class="w-4 h-2.5 rounded-sm bg-[#111827] border border-gray-700" />
          <span class="text-[9px] text-gray-400">Child entity</span>
        </div>
      </div>

      <svg
        ref={svgRef}
        class="w-full h-full"
        style="cursor: grab; background: #070c17;"
        onWheel={onWheel}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onDblClick={onDblClick}
      >
        <defs>
          <marker id="fg-arrow" markerWidth="10" markerHeight="7" refX="10" refY="3.5" orient="auto">
            <path d="M0,0 L10,3.5 L0,7" fill="#4b5563" opacity="0.5" />
          </marker>
          <marker id="fg-arrow-active" markerWidth="10" markerHeight="7" refX="10" refY="3.5" orient="auto">
            <path d="M0,0 L10,3.5 L0,7" fill="#3b82f6" opacity="0.8" />
          </marker>
          <filter id="glow-strong" x="-50%" y="-50%" width="200%" height="200%">
            <feGaussianBlur stdDeviation="4" result="blur" />
            <feMerge>
              <feMergeNode in="blur" />
              <feMergeNode in="blur" />
              <feMergeNode in="SourceGraphic" />
            </feMerge>
          </filter>
        </defs>

        {/* Dot grid */}
        <pattern id="grid-dots" width="20" height="20" patternUnits="userSpaceOnUse">
          <circle cx="10" cy="10" r="0.5" fill="#1a2035" />
        </pattern>
        <rect width="100%" height="100%" fill="url(#grid-dots)" />

        {/* Pan/zoom group */}
        <g transform={`translate(${transform().x},${transform().y}) scale(${transform().scale})`}>
          {/* Edges + particles — no pointer events */}
          <g style="pointer-events: none;">
            <Index each={renderEdges()}>
              {(line) => {
                const isFromSel = () => line().source === sel();
                const isToSel = () => line().target === sel();
                const active = () => isFromSel() || isToSel();
                return (
                  <line
                    x1={line().x1} y1={line().y1}
                    x2={line().x2} y2={line().y2}
                    stroke={active() ? "#3b82f6" : "#1e293b"}
                    stroke-width={active() ? 1.8 : 0.8}
                    stroke-dasharray={isToSel() && !isFromSel() ? "6,4" : undefined}
                    marker-end={active() ? "url(#fg-arrow-active)" : "url(#fg-arrow)"}
                    opacity={sel() ? (active() ? 0.9 : 0.2) : 0.4}
                  />
                );
              }}
            </Index>
            <Index each={renderParticles()}>
              {(p) => (
                <Show when={p().opacity > 0.05}>
                  <circle
                    cx={p().x} cy={p().y} r={1.5}
                    fill="#60a5fa"
                    opacity={p().opacity * (sel() ? 0.3 : 0.5)}
                  />
                </Show>
              )}
            </Index>
          </g>

          {/* Nodes */}
          <Index each={renderNodes()}>
            {(node) => {
              const isSel = () => node().id === sel();
              const isConnected = () => {
                if (!sel()) return false;
                const e = props.edges;
                return (e[sel()!] || []).includes(node().id) ||
                  (e[node().id] || []).includes(sel()!);
              };
              const dimmed = () => sel() != null && !isSel() && !isConnected();

              const fillColor = () =>
                isSel() ? "#1e3a5f" :
                node().isShared ? "#2d1f0e" :
                node().isRoot ? "#0f1d33" : "#111827";
              const strokeColor = () =>
                isSel() ? "#3b82f6" :
                node().isShared ? "#a16207" :
                node().isRoot ? "#1d4ed8" : "#1e293b";
              const textColor = () =>
                isSel() ? "#93c5fd" :
                node().isShared ? "#fbbf24" :
                node().isRoot ? "#60a5fa" : "#d1d5db";

              return (
                <g
                  style={{ cursor: "pointer", "pointer-events": "all" }}
                  opacity={dimmed() ? 0.25 : 1}
                  filter={isSel() ? "url(#glow-strong)" : undefined}
                >
                  <rect
                    x={node().x - node().hw} y={node().y - PILL_HH}
                    width={node().hw * 2} height={PILL_H}
                    rx={6} ry={6}
                    fill={fillColor()}
                    stroke={strokeColor()}
                    stroke-width={isSel() ? 1.8 : 0.8}
                  />
                  <circle
                    cx={node().x - node().hw + 9} cy={node().y}
                    r={3} fill={strokeColor()} opacity={0.7}
                  />
                  <text
                    x={node().x + 4} y={node().y + 4}
                    text-anchor="middle"
                    fill={textColor()}
                    font-size={node().isRoot ? 12 : 11}
                    font-family="ui-monospace, SFMono-Regular, monospace"
                    style="pointer-events: none; user-select: none;"
                  >{node().id}</text>
                  <Show when={node().pinned}>
                    <circle
                      cx={node().x + node().hw - 9} cy={node().y}
                      r={2} fill="#f59e0b" opacity={0.6}
                    />
                  </Show>
                </g>
              );
            }}
          </Index>
        </g>

        <text
          x="8" y="98%"
          fill="#374151" font-size="10" font-family="monospace"
        >{Math.round(transform().scale * 100)}%</text>
      </svg>
    </div>
  );
}
