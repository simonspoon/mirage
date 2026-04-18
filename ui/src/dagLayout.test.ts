import { describe, it, expect } from 'vitest';
import { computeDagPositions, GraphEdge, EntityDef } from './dagLayout';

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
});
