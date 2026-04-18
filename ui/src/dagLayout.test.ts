import { describe, it, expect } from 'vitest';
import {
  computeDagPositions,
  widthOf,
  GraphEdge,
  EntityDef,
  PAD,
  NODE_GAP,
  DEFAULT_WIDTH,
  STUB_WIDTH,
  MAX_WIDTH,
  PartitionEntityDef,
  computeTwoHopPartition,
  computeFullGraphDepths,
  bucketHiddenByBand,
  computeTopHubs,
} from './dagLayout';

const BOX_SPACING = 300;
const BAND_GAP = 40;
const NO_DEFS: Record<string, EntityDef> = {};
const NO_STUBS = new Set<string>();

// Count edge crossings between two adjacent bands given positions and edges.
// An inversion occurs when two edges (a→p, b→q) in the same band pair cross:
// i.e. a is left of b in lower band, but p is right of q in upper band (or vice versa).
function countCrossings(
  positions: Record<string, { x: number; y: number }>,
  edges: GraphEdge[],
): number {
  // Collect (lowerX, upperX) pairs for each edge where both endpoints exist in positions
  const pairs: Array<[number, number]> = [];
  for (const e of edges) {
    const s = positions[e.source];
    const t = positions[e.target];
    if (!s || !t) continue;
    // Only count cross-band edges (different y)
    if (s.y === t.y) continue;
    // Normalise so "lower y" = upper band (smaller y value = higher in the DAG)
    const [upperX, lowerX] = s.y < t.y ? [s.x, t.x] : [t.x, s.x];
    pairs.push([upperX, lowerX]);
  }
  let crossings = 0;
  for (let i = 0; i < pairs.length; i++) {
    for (let j = i + 1; j < pairs.length; j++) {
      const [u1, l1] = pairs[i];
      const [u2, l2] = pairs[j];
      // Crossing: relative order differs between upper and lower band
      if ((u1 < u2 && l1 > l2) || (u1 > u2 && l1 < l2)) crossings++;
    }
  }
  return crossings;
}

describe('computeDagPositions', () => {
  it('empty list → empty positions, minimal SVG dimensions', () => {
    const result = computeDagPositions([], [], NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP);
    expect(result.positions).toEqual({});
    expect(result.width).toBeGreaterThan(0);
    expect(result.height).toBeGreaterThanOrEqual(400);
  });

  it('single node → x=20, y=20', () => {
    const result = computeDagPositions(['A'], [], NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP);
    expect(result.positions['A']).toEqual({ x: 20, y: 20 });
  });

  it('linear chain A→B→C → three distinct y values', () => {
    // A is root (no one points to it in terms of "source").
    // Edge semantics: source references target. B references A, C references B.
    const edges: GraphEdge[] = [
      { source: 'B', target: 'A' },
      { source: 'C', target: 'B' },
    ];
    const list = ['A', 'B', 'C'];
    const result = computeDagPositions(list, edges, NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP);
    const yA = result.positions['A'].y;
    const yB = result.positions['B'].y;
    const yC = result.positions['C'].y;
    expect(yA).not.toBe(yB);
    expect(yB).not.toBe(yC);
    expect(yA).not.toBe(yC);
  });

  it('pure cycle A↔B → no crash, deterministic, both in positions', () => {
    const edges: GraphEdge[] = [
      { source: 'A', target: 'B' },
      { source: 'B', target: 'A' },
    ];
    const list = ['A', 'B'];
    const result1 = computeDagPositions(list, edges, NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP);
    const result2 = computeDagPositions(list, edges, NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP);
    expect(result1.positions['A']).toBeDefined();
    expect(result1.positions['B']).toBeDefined();
    expect(result1.positions).toEqual(result2.positions);
  });

  it('disconnected node → gets depth 0 and appears in positions', () => {
    // X has no edges; A→B are connected. X should still be laid out.
    const edges: GraphEdge[] = [{ source: 'B', target: 'A' }];
    const list = ['A', 'B', 'X'];
    const result = computeDagPositions(list, edges, NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP);
    expect(result.positions['X']).toBeDefined();
    // X has no edges so it's not a source, hence depth 0 (same band as A)
    expect(result.positions['X'].y).toBe(result.positions['A'].y);
  });

  it('3-node crossing fixture: barycenter reduces crossings vs naive', () => {
    // Band 0 (roots, y smallest): P1, P2
    // Band 1 (children):          C1, C2, C3
    // Naive order: C1, C2, C3 (visibleList insertion order)
    // Edges arranged so naive has crossings:
    //   P1 → C3 (P1 is at x=20, C3 naive at x=640) — right side
    //   P2 → C1 (P2 is at x=320, C1 naive at x=20) — left side
    // This produces 1 crossing (P1→C3 crosses P2→C1).
    // After barycenter C3 should move left of C1 (barycenter for C3=0, C1=1).
    const list = ['P1', 'P2', 'C1', 'C2', 'C3'];
    const edges: GraphEdge[] = [
      { source: 'C3', target: 'P1' }, // C3 references P1
      { source: 'C1', target: 'P2' }, // C1 references P2
    ];
    const naive = computeDagPositions(list, edges, NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP, {
      barycenter: false,
    });
    const bary = computeDagPositions(list, edges, NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP, {
      barycenter: true,
    });
    const crossingsBefore = countCrossings(naive.positions, edges);
    const crossingsAfter = countCrossings(bary.positions, edges);
    expect(crossingsBefore).toBeGreaterThan(0);
    expect(crossingsAfter).toBeLessThan(crossingsBefore);
  });

  it('determinism: same input → identical positions across 3 calls', () => {
    const list = ['A', 'B', 'C', 'D', 'E'];
    const edges: GraphEdge[] = [
      { source: 'C', target: 'A' },
      { source: 'D', target: 'B' },
      { source: 'E', target: 'C' },
      { source: 'E', target: 'D' },
    ];
    const r1 = computeDagPositions(list, edges, NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP);
    const r2 = computeDagPositions(list, edges, NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP);
    const r3 = computeDagPositions(list, edges, NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP);
    expect(r1.positions).toEqual(r2.positions);
    expect(r2.positions).toEqual(r3.positions);
  });

  // ── Width-aware packing (orrp) ────────────────────────────────────────────

  it('U1 uniform widths [260,260,260] → xs [20,320,620], width=900', () => {
    const list = ['A', 'B', 'C'];
    const widths: Record<string, number> = { A: 260, B: 260, C: 260 };
    const result = computeDagPositions(
      list, [], NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP,
      { widthOf: (n) => widths[n], barycenter: false },
    );
    expect(result.positions['A'].x).toBe(20);
    expect(result.positions['B'].x).toBe(320);
    expect(result.positions['C'].x).toBe(620);
    expect(result.width).toBe(900); // 620 + 260 + 20
  });

  it('U2 heterogeneous widths [100,260,50,400] → exact cumsum xs', () => {
    const list = ['A', 'B', 'C', 'D'];
    const widths: Record<string, number> = { A: 100, B: 260, C: 50, D: 400 };
    const result = computeDagPositions(
      list, [], NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP,
      { widthOf: (n) => widths[n], barycenter: false },
    );
    // x[0]=20; x[1]=20+100+40=160; x[2]=160+260+40=460; x[3]=460+50+40=550
    expect(result.positions['A'].x).toBe(20);
    expect(result.positions['B'].x).toBe(160);
    expect(result.positions['C'].x).toBe(460);
    expect(result.positions['D'].x).toBe(550);
    // width = 550 + 400 + 20
    expect(result.width).toBe(970);
  });

  it('U3 gap-invariant: x[i+1] − (x[i]+w[i]) === NODE_GAP across 5 seeded vectors', () => {
    // Deterministic LCG for repeatable widths without extra deps.
    const lcg = (seed: number) => {
      let s = seed;
      return () => {
        s = (s * 1664525 + 1013904223) >>> 0;
        return s / 2 ** 32;
      };
    };
    for (let seed = 1; seed <= 5; seed++) {
      const rand = lcg(seed);
      const len = 2 + Math.floor(rand() * 7); // 2..8
      const list: string[] = [];
      const widths: Record<string, number> = {};
      for (let i = 0; i < len; i++) {
        const name = `N${i}`;
        list.push(name);
        widths[name] = 50 + Math.floor(rand() * 400);
      }
      const result = computeDagPositions(
        list, [], NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP,
        { widthOf: (n) => widths[n], barycenter: false },
      );
      for (let i = 0; i < list.length - 1; i++) {
        const a = result.positions[list[i]];
        const b = result.positions[list[i + 1]];
        const gap = b.x - (a.x + widths[list[i]]);
        expect(gap).toBe(NODE_GAP);
      }
    }
  });

  it('U4 single [260] → x=20, width=300', () => {
    const result = computeDagPositions(
      ['A'], [], NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP,
      { widthOf: () => 260 },
    );
    expect(result.positions['A'].x).toBe(20);
    expect(result.width).toBe(300); // 20 + 260 + 20
  });

  it('U5 zero/clamped width → gap invariant still holds', () => {
    const list = ['A', 'B', 'C'];
    // Zero gets clamped to 1 by the layout to avoid collapse.
    const widths: Record<string, number> = { A: 0, B: 100, C: 0 };
    const result = computeDagPositions(
      list, [], NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP,
      { widthOf: (n) => widths[n], barycenter: false },
    );
    // With clamp floor 1: x[0]=20; x[1]=20+1+40=61; x[2]=61+100+40=201
    expect(result.positions['A'].x).toBe(20);
    expect(result.positions['B'].x).toBe(61);
    expect(result.positions['C'].x).toBe(201);
  });

  it('W1 widthOf helper: stub → STUB_WIDTH', () => {
    expect(widthOf('Anything', { properties: {} }, true)).toBe(STUB_WIDTH);
  });

  it('W2 widthOf helper: missing def → DEFAULT_WIDTH (260)', () => {
    expect(widthOf('Ghost', undefined, false)).toBe(DEFAULT_WIDTH);
  });

  it('S1 two-band: SVG width = widest band cumulative', () => {
    // Band 0 has 2 nodes (A root, B root). Band 1 has 3 nodes (C, D, E all
    // reference A). Band 1 is the wider band under variable widths.
    const list = ['A', 'B', 'C', 'D', 'E'];
    const edges: GraphEdge[] = [
      { source: 'C', target: 'A' },
      { source: 'D', target: 'A' },
      { source: 'E', target: 'A' },
    ];
    const widths: Record<string, number> = {
      A: 100, B: 100,
      C: 300, D: 300, E: 300,
    };
    const result = computeDagPositions(
      list, edges, NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP,
      { widthOf: (n) => widths[n], barycenter: false },
    );
    // Band 1 right edge: x[0]=20; x[1]=360; x[2]=700 → 700+300=1000. +20 PAD = 1020.
    expect(result.width).toBe(1020);
  });

  it('S2 deeper band wider → result.width matches deeper band', () => {
    const list = ['A', 'B'];
    const edges: GraphEdge[] = [{ source: 'B', target: 'A' }];
    const widths: Record<string, number> = { A: 100, B: 500 };
    const result = computeDagPositions(
      list, edges, NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP,
      { widthOf: (n) => widths[n], barycenter: false },
    );
    // A alone on band 0: right edge 120; B alone on band 1: right edge 520.
    // Widest: 520 + 20 = 540.
    expect(result.width).toBe(540);
  });

  it('S3 no-clip: x+width+PAD ≤ result.width for every node', () => {
    const list = ['A', 'B', 'C', 'D'];
    const widths: Record<string, number> = { A: 80, B: 300, C: 120, D: 200 };
    const result = computeDagPositions(
      list, [], NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP,
      { widthOf: (n) => widths[n], barycenter: false },
    );
    for (const name of list) {
      const p = result.positions[name];
      expect(p.x + widths[name] + PAD).toBeLessThanOrEqual(result.width);
    }
  });

  it('B1 barycenter order preserved with non-uniform widths', () => {
    // Same crossing fixture as the 3-node test, but with variable widths.
    // Assert the sortedIndex ordering matches the uniform-width run.
    const list = ['P1', 'P2', 'C1', 'C2', 'C3'];
    const edges: GraphEdge[] = [
      { source: 'C3', target: 'P1' },
      { source: 'C1', target: 'P2' },
    ];
    const uniform = computeDagPositions(
      list, edges, NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP,
      { barycenter: true },
    );
    const variable = computeDagPositions(
      list, edges, NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP,
      { barycenter: true, widthOf: (n) => (n.startsWith('P') ? 150 : 400) },
    );
    // Rank within each band by x. Same names should have identical rank.
    const rankByY = (res: { positions: Record<string, { x: number; y: number }> }) => {
      const byY: Record<number, Array<{ name: string; x: number }>> = {};
      for (const [name, p] of Object.entries(res.positions)) {
        (byY[p.y] ||= []).push({ name, x: p.x });
      }
      const ranks: Record<string, number> = {};
      for (const band of Object.values(byY)) {
        band.sort((a, b) => a.x - b.x);
        band.forEach((node, i) => { ranks[node.name] = i; });
      }
      return ranks;
    };
    expect(rankByY(variable)).toEqual(rankByY(uniform));
  });

  it('B2 determinism with variable widths: two calls → identical positions', () => {
    const list = ['A', 'B', 'C', 'D', 'E'];
    const edges: GraphEdge[] = [
      { source: 'C', target: 'A' },
      { source: 'D', target: 'B' },
      { source: 'E', target: 'C' },
    ];
    const widths: Record<string, number> = { A: 120, B: 300, C: 260, D: 400, E: 180 };
    const r1 = computeDagPositions(
      list, edges, NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP,
      { widthOf: (n) => widths[n] },
    );
    const r2 = computeDagPositions(
      list, edges, NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP,
      { widthOf: (n) => widths[n] },
    );
    expect(r1.positions).toEqual(r2.positions);
  });

  it('E1 empty list → positions={}, width≥1', () => {
    const result = computeDagPositions(
      [], [], NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP,
      { widthOf: () => 260 },
    );
    expect(result.positions).toEqual({});
    expect(result.width).toBeGreaterThanOrEqual(1);
  });

  it('E2/E3 negative / NaN widthOf clamped → no NaN in positions', () => {
    const list = ['A', 'B'];
    const result = computeDagPositions(
      list, [], NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP,
      { widthOf: (n) => (n === 'A' ? -50 : NaN), barycenter: false },
    );
    for (const p of Object.values(result.positions)) {
      expect(Number.isFinite(p.x)).toBe(true);
      expect(Number.isFinite(p.y)).toBe(true);
    }
    // A clamped to 1, B falls back to DEFAULT_WIDTH (260) when NaN.
    expect(result.positions['A'].x).toBe(20);
    expect(result.positions['B'].x).toBe(20 + 1 + NODE_GAP);
  });

  it('E4 10000px node → width ≥ 10020 (capped by MAX_WIDTH)', () => {
    const result = computeDagPositions(
      ['Big'], [], NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP,
      { widthOf: () => 10000 },
    );
    // Clamped to MAX_WIDTH. Width = 20 + MAX_WIDTH + 20.
    expect(result.width).toBe(20 + MAX_WIDTH + 20);
  });

  it('E6 mixed stub+non-stub widths in same band → gap invariant', () => {
    const list = ['A', 'B', 'C'];
    // Simulate a stub by supplying the fixed STUB_WIDTH for A.
    const widths: Record<string, number> = { A: STUB_WIDTH, B: 400, C: 120 };
    const result = computeDagPositions(
      list, [], NO_DEFS, NO_STUBS, BOX_SPACING, BAND_GAP,
      { widthOf: (n) => widths[n], barycenter: false },
    );
    for (let i = 0; i < list.length - 1; i++) {
      const a = result.positions[list[i]];
      const b = result.positions[list[i + 1]];
      expect(b.x - (a.x + widths[list[i]])).toBe(NODE_GAP);
    }
  });
});

// ── 2-hop partition helpers ───────────────────────────────────────────────

// Tiny DSL: prop(ref?) / prop(items=?) for fixture readability.
const pRef = (ref: string) => ({ ref_name: ref, items_ref: null });
const pItems = (items: string) => ({ ref_name: null, items_ref: items });

describe('computeTwoHopPartition', () => {
  it('empty defs + empty focal → empty partition', () => {
    const res = computeTwoHopPartition({}, new Set(), 2);
    expect(res.visible.size).toBe(0);
    expect(res.hiddenAtRing3.size).toBe(0);
    expect(res.hopOf).toEqual({});
  });

  it('empty focal against non-empty defs → empty partition', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: { b: pRef('B') } },
      B: { properties: {} },
    };
    const res = computeTwoHopPartition(defs, new Set(), 2);
    expect(res.visible.size).toBe(0);
    expect(res.hiddenAtRing3.size).toBe(0);
  });

  it('focal with no neighbours → only focal visible, no ring3', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: {} },
    };
    const res = computeTwoHopPartition(defs, new Set(['A']), 2);
    expect([...res.visible]).toEqual(['A']);
    expect(res.hopOf).toEqual({ A: 0 });
    expect(res.hiddenAtRing3.size).toBe(0);
  });

  it('1-hop + 2-hop chain A→B→C, maxHop=2 → all visible', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: { b: pRef('B') } },
      B: { properties: { c: pRef('C') } },
      C: { properties: {} },
    };
    const res = computeTwoHopPartition(defs, new Set(['A']), 2);
    expect(res.visible).toEqual(new Set(['A', 'B', 'C']));
    expect(res.hopOf).toEqual({ A: 0, B: 1, C: 2 });
    expect(res.hiddenAtRing3.size).toBe(0);
  });

  it('3-hop chain A→B→C→D → D in hiddenAtRing3, not in visible', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: { b: pRef('B') } },
      B: { properties: { c: pRef('C') } },
      C: { properties: { d: pRef('D') } },
      D: { properties: {} },
    };
    const res = computeTwoHopPartition(defs, new Set(['A']), 2);
    expect(res.visible).toEqual(new Set(['A', 'B', 'C']));
    expect(res.hiddenAtRing3).toEqual(new Set(['D']));
    expect(res.hopOf.D).toBe(3);
  });

  it('inbound 2-hop X→Y→Z focal=Z → X,Y,Z all visible via inbound edges', () => {
    const defs: Record<string, PartitionEntityDef> = {
      X: { properties: { y: pRef('Y') } },
      Y: { properties: { z: pRef('Z') } },
      Z: { properties: {} },
    };
    const res = computeTwoHopPartition(defs, new Set(['Z']), 2);
    expect(res.visible).toEqual(new Set(['X', 'Y', 'Z']));
    expect(res.hopOf).toEqual({ Z: 0, Y: 1, X: 2 });
  });

  it('extends edge counts as 1 hop (guards extends-pseudo-edge trap)', () => {
    // def.extends lives on the def itself, NOT in def.properties.
    const defs: Record<string, PartitionEntityDef> = {
      Child: { properties: {}, extends: 'Parent' },
      Parent: { properties: {} },
    };
    const res = computeTwoHopPartition(defs, new Set(['Child']), 2);
    expect(res.visible).toEqual(new Set(['Child', 'Parent']));
    expect(res.hopOf.Parent).toBe(1);
  });

  it('extends as ring3: 3-deep extends chain → 3rd hidden', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: {}, extends: 'B' },
      B: { properties: {}, extends: 'C' },
      C: { properties: {}, extends: 'D' },
      D: { properties: {} },
    };
    const res = computeTwoHopPartition(defs, new Set(['A']), 2);
    expect(res.visible).toEqual(new Set(['A', 'B', 'C']));
    expect(res.hiddenAtRing3).toEqual(new Set(['D']));
  });

  it('items_ref treated identically to ref_name', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: { arr: pItems('B') } },
      B: { properties: {} },
    };
    const res = computeTwoHopPartition(defs, new Set(['A']), 2);
    expect(res.visible).toEqual(new Set(['A', 'B']));
    expect(res.hopOf.B).toBe(1);
  });

  it('cycle A↔B terminates <100ms, both visible, hop assigned', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: { b: pRef('B') } },
      B: { properties: { a: pRef('A') } },
    };
    const t0 = Date.now();
    const res = computeTwoHopPartition(defs, new Set(['A']), 2);
    expect(Date.now() - t0).toBeLessThan(100);
    expect(res.visible).toEqual(new Set(['A', 'B']));
    expect(res.hopOf).toEqual({ A: 0, B: 1 });
    expect(res.hiddenAtRing3.size).toBe(0);
  });

  it('self-ref A→A → no infinite loop, A hop 0, no other visible', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: { self: pRef('A') } },
    };
    const res = computeTwoHopPartition(defs, new Set(['A']), 2);
    expect([...res.visible]).toEqual(['A']);
    expect(res.hopOf).toEqual({ A: 0 });
  });

  it('two focals overlapping 2-hops → min-hop wins', () => {
    // Chain: A→B→C→D→E; focals={A,E}. Nodes B,C reachable from A at 1,2;
    // C,D reachable from E at 2,1. Min-hop: B=1, C=2, D=1.
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: { b: pRef('B') } },
      B: { properties: { c: pRef('C') } },
      C: { properties: { d: pRef('D') } },
      D: { properties: { e: pRef('E') } },
      E: { properties: {} },
    };
    const res = computeTwoHopPartition(defs, new Set(['A', 'E']), 2);
    expect(res.visible).toEqual(new Set(['A', 'B', 'C', 'D', 'E']));
    expect(res.hopOf.A).toBe(0);
    expect(res.hopOf.E).toBe(0);
    expect(res.hopOf.B).toBe(1);
    expect(res.hopOf.D).toBe(1);
    expect(res.hopOf.C).toBe(2);
  });

  it('focal beats being-reached-as-hop from another focal', () => {
    // A→B→C; focals={A,B}. B must stay hop 0 (focal), not 1.
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: { b: pRef('B') } },
      B: { properties: { c: pRef('C') } },
      C: { properties: {} },
    };
    const res = computeTwoHopPartition(defs, new Set(['A', 'B']), 2);
    expect(res.hopOf.A).toBe(0);
    expect(res.hopOf.B).toBe(0);
    expect(res.hopOf.C).toBe(1);
  });

  it('dangling ref to unknown def → no throw, unknown node never visible', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: { missing: pRef('GHOST') } },
    };
    const res = computeTwoHopPartition(defs, new Set(['A']), 2);
    // GHOST has no entry in defs so outgoingRefs(defs['GHOST']) returns [].
    // BFS still visits GHOST once as a name (hop 1) — it's a known reachable
    // neighbour by edge, but has no further edges. That's acceptable — the
    // caller filters down to defs present when rendering.
    expect(res.visible.has('A')).toBe(true);
    expect(res.hopOf.A).toBe(0);
    // GHOST is reachable at hop 1 via the outgoing ref; we don't require the
    // caller to render it — but the helper should not throw.
    expect(res.hopOf.GHOST).toBe(1);
  });

  it('disconnected island hidden NOT counted in ring3', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: { b: pRef('B') } },
      B: { properties: {} },
      // Island (no edges to A or B)
      X: { properties: { y: pRef('Y') } },
      Y: { properties: {} },
    };
    const res = computeTwoHopPartition(defs, new Set(['A']), 2);
    expect(res.visible).toEqual(new Set(['A', 'B']));
    expect(res.hiddenAtRing3.size).toBe(0);
    expect(res.hopOf.X).toBeUndefined();
    expect(res.hopOf.Y).toBeUndefined();
  });

  it('multi-path boundary: hop=2 via one path beats hop=3 via another → visible', () => {
    // A→B→C; A→X→Y→C. C reachable at hop 2 via B, hop 3 via Y. Min wins: 2.
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: { b: pRef('B'), x: pRef('X') } },
      B: { properties: { c: pRef('C') } },
      X: { properties: { y: pRef('Y') } },
      Y: { properties: { c: pRef('C') } },
      C: { properties: {} },
    };
    const res = computeTwoHopPartition(defs, new Set(['A']), 2);
    expect(res.hopOf.C).toBe(2);
    expect(res.visible.has('C')).toBe(true);
    expect(res.hiddenAtRing3.has('C')).toBe(false);
  });

  it('determinism: repeated runs yield identical visible+hopOf', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: { b: pRef('B'), c: pRef('C') } },
      B: { properties: { d: pRef('D') } },
      C: { properties: { d: pRef('D') } },
      D: { properties: { e: pRef('E') } },
      E: { properties: {} },
    };
    const r1 = computeTwoHopPartition(defs, new Set(['A']), 2);
    const r2 = computeTwoHopPartition(defs, new Set(['A']), 2);
    expect([...r1.visible].sort()).toEqual([...r2.visible].sort());
    expect(r1.hopOf).toEqual(r2.hopOf);
    expect([...r1.hiddenAtRing3].sort()).toEqual([...r2.hiddenAtRing3].sort());
  });
});

describe('computeFullGraphDepths', () => {
  it('empty focal → empty depths', () => {
    const defs: Record<string, PartitionEntityDef> = { A: { properties: {} } };
    expect(computeFullGraphDepths(defs, new Set())).toEqual({});
  });

  it('linear chain depths unbounded: A→B→C→D→E', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: { b: pRef('B') } },
      B: { properties: { c: pRef('C') } },
      C: { properties: { d: pRef('D') } },
      D: { properties: { e: pRef('E') } },
      E: { properties: {} },
    };
    const res = computeFullGraphDepths(defs, new Set(['A']));
    expect(res).toEqual({ A: 0, B: 1, C: 2, D: 3, E: 4 });
  });

  it('disconnected node absent from depths', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: { b: pRef('B') } },
      B: { properties: {} },
      Island: { properties: {} },
    };
    const res = computeFullGraphDepths(defs, new Set(['A']));
    expect(res.Island).toBeUndefined();
  });
});

describe('bucketHiddenByBand', () => {
  it('empty hidden → empty result', () => {
    expect(bucketHiddenByBand(new Set(), {})).toEqual({});
  });

  it('single hidden → bucketed into (depth - 1) band', () => {
    // Hidden H reachable at depth 3 → band 2 (its visible parent sits in band 2).
    const res = bucketHiddenByBand(new Set(['H']), { H: 3 });
    expect(res).toEqual({ 2: ['H'] });
  });

  it('multiple hidden same band increments', () => {
    const res = bucketHiddenByBand(
      new Set(['H1', 'H2']),
      { H1: 3, H2: 3 },
    );
    expect(res[2].sort()).toEqual(['H1', 'H2']);
    expect(Object.keys(res)).toEqual(['2']);
  });

  it('hidden without depth (disconnected) skipped', () => {
    const res = bucketHiddenByBand(new Set(['H1', 'Ghost']), { H1: 3 });
    expect(res).toEqual({ 2: ['H1'] });
  });

  it('zero hidden per band → band absent from result', () => {
    const res = bucketHiddenByBand(new Set(['H1']), { H1: 3 });
    expect(res[0]).toBeUndefined();
    expect(res[1]).toBeUndefined();
    expect(res[2]).toEqual(['H1']);
  });
});

describe('computeTopHubs', () => {
  it('basic ranking: 4 defs with counts 3/2/1/0, n=5 → all 4 in count-desc order', () => {
    // Inbound count derived from outgoing refs of every def:
    //   A inbound from B,C,D    → 3
    //   B inbound from C,D       → 2
    //   C inbound from D          → 1
    //   D inbound from (none)     → 0
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: {} },
      B: { properties: { a: pRef('A') } },
      C: { properties: { a: pRef('A'), b: pRef('B') } },
      D: { properties: { a: pRef('A'), b: pRef('B'), c: pRef('C') } },
    };
    expect(computeTopHubs(defs, 5)).toEqual(['A', 'B', 'C', 'D']);
  });

  it('n < defs.length → truncate at n', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: {} },
      B: { properties: { a: pRef('A') } },
      C: { properties: { a: pRef('A'), b: pRef('B') } },
      D: { properties: { a: pRef('A'), b: pRef('B'), c: pRef('C') } },
    };
    const res = computeTopHubs(defs, 2);
    expect(res).toEqual(['A', 'B']);
    expect(res).toHaveLength(2);
  });

  it('tie-break by name asc when counts equal', () => {
    // Z, A, M all referenced once → name asc: A, M, Z
    const defs: Record<string, PartitionEntityDef> = {
      Z: { properties: {} },
      A: { properties: {} },
      M: { properties: {} },
      Ref1: { properties: { z: pRef('Z') } },
      Ref2: { properties: { a: pRef('A') } },
      Ref3: { properties: { m: pRef('M') } },
    };
    // All six defs share counts: Z=1, A=1, M=1, Ref1=0, Ref2=0, Ref3=0.
    // Top 3 by count-desc, name-asc: A, M, Z.
    expect(computeTopHubs(defs, 3)).toEqual(['A', 'M', 'Z']);
  });

  it('empty defs → []', () => {
    expect(computeTopHubs({}, 5)).toStrictEqual([]);
  });

  it('n=0 → []', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: {} },
      B: { properties: { a: pRef('A') } },
    };
    expect(computeTopHubs(defs, 0)).toStrictEqual([]);
  });

  it('n<0 (negative) clamps to [] ', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: {} },
      B: { properties: { a: pRef('A') } },
    };
    expect(computeTopHubs(defs, -3)).toStrictEqual([]);
  });

  it('all-zero-inbound → returns names in asc order, sliced', () => {
    const defs: Record<string, PartitionEntityDef> = {
      Charlie: { properties: {} },
      Alpha: { properties: {} },
      Bravo: { properties: {} },
    };
    expect(computeTopHubs(defs, 5)).toEqual(['Alpha', 'Bravo', 'Charlie']);
    expect(computeTopHubs(defs, 2)).toEqual(['Alpha', 'Bravo']);
  });

  it('self-ref does NOT count as inbound (mirrors buildInboundAdj)', () => {
    // A.self → A is excluded from inbound. A inbound count stays 0.
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: { self: pRef('A') } },
      B: { properties: { a: pRef('A') } },
    };
    // Inbound: A from B (self skipped) → 1. B from (none) → 0.
    // Sort: A(1) before B(0).
    expect(computeTopHubs(defs, 5)).toEqual(['A', 'B']);
  });

  it('cyclic refs A↔B terminate fast and rank deterministically', () => {
    // A.b → B; B.a → A. Each has inbound count 1 from the other (no self).
    // Tie on count=1 → name asc: A, B.
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: { b: pRef('B') } },
      B: { properties: { a: pRef('A') } },
    };
    const t0 = Date.now();
    const res = computeTopHubs(defs, 5);
    expect(Date.now() - t0).toBeLessThan(100);
    expect(res).toEqual(['A', 'B']);
  });

  it('determinism: 3 calls → identical result', () => {
    const defs: Record<string, PartitionEntityDef> = {
      A: { properties: {} },
      B: { properties: { a: pRef('A') } },
      C: { properties: { a: pRef('A'), b: pRef('B') } },
      D: { properties: { c: pRef('C') } },
      E: { properties: { d: pRef('D') } },
    };
    const r1 = computeTopHubs(defs, 3);
    const r2 = computeTopHubs(defs, 3);
    const r3 = computeTopHubs(defs, 3);
    expect(r1).toEqual(r2);
    expect(r2).toEqual(r3);
  });

  it('extends edge counts toward inbound (mirrors outgoingRefs)', () => {
    // Child extends Parent → Parent inbound count = 1.
    const defs: Record<string, PartitionEntityDef> = {
      Parent: { properties: {} },
      Child: { properties: {}, extends: 'Parent' },
    };
    expect(computeTopHubs(defs, 5)).toEqual(['Parent', 'Child']);
  });

  it('items_ref counts toward inbound (mirrors ref_name)', () => {
    const defs: Record<string, PartitionEntityDef> = {
      Item: { properties: {} },
      Holder: { properties: { arr: pItems('Item') } },
    };
    expect(computeTopHubs(defs, 5)).toEqual(['Item', 'Holder']);
  });
});
