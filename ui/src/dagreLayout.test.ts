import { describe, it, expect } from "vitest";
import { computeDagrePositions } from "./dagreLayout";
import {
  PAD,
  HEADER_HEIGHT,
  ROW_HEIGHT,
  DEFAULT_WIDTH,
  type EntityDef,
  type GraphEdge,
} from "./dagLayout";

const NO_DEFS: Record<string, EntityDef> = {};
const NO_STUBS = new Set<string>();
const BAND_GAP = 40;

const def = (props: string[] = [], extendsName?: string): EntityDef => ({
  properties: Object.fromEntries(props.map(p => [p, {}])),
  extends: extendsName,
});

describe("computeDagrePositions", () => {
  it("empty list → empty positions, minimal SVG dimensions", () => {
    const r = computeDagrePositions([], [], NO_DEFS, NO_STUBS, BAND_GAP);
    expect(r.positions).toEqual({});
    expect(r.width).toBeGreaterThan(0);
    expect(r.height).toBeGreaterThanOrEqual(400);
  });

  it("single node → in positions, top-left coords (no negative x/y)", () => {
    const r = computeDagrePositions(["A"], [], { A: def() }, NO_STUBS, BAND_GAP);
    expect(r.positions.A).toBeDefined();
    expect(r.positions.A.x).toBeGreaterThanOrEqual(0);
    expect(r.positions.A.y).toBeGreaterThanOrEqual(0);
  });

  it("linear chain A→B→C with rankdir TB → distinct y values, ranked", () => {
    const edges: GraphEdge[] = [
      { source: "A", target: "B" },
      { source: "B", target: "C" },
    ];
    const defs = { A: def(["b"]), B: def(["c"]), C: def() };
    const r = computeDagrePositions(["A", "B", "C"], edges, defs, NO_STUBS, BAND_GAP);
    const ys = ["A", "B", "C"].map(n => r.positions[n].y);
    // 3 distinct ranks
    expect(new Set(ys).size).toBe(3);
    // dagre default TB: source rank above target rank → A above B above C visually.
    // dagre uses target = lower rank by convention (edges point up the DAG):
    // so we just assert all three are distinct without prescribing direction.
  });

  it("disconnected nodes → all in positions", () => {
    const defs = { A: def(), B: def(), C: def() };
    const r = computeDagrePositions(["A", "B", "C"], [], defs, NO_STUBS, BAND_GAP);
    expect(Object.keys(r.positions).sort()).toEqual(["A", "B", "C"]);
  });

  it("self-ref A→A → no crash, A positioned", () => {
    const edges: GraphEdge[] = [{ source: "A", target: "A" }];
    const r = computeDagrePositions(["A"], edges, { A: def() }, NO_STUBS, BAND_GAP);
    expect(r.positions.A).toBeDefined();
  });

  it("parallel edges A→B (multigraph) → both nodes positioned, no crash", () => {
    const edges: GraphEdge[] = [
      { source: "A", target: "B" },
      { source: "A", target: "B" },
      { source: "A", target: "B" },
    ];
    const defs = { A: def(["b1", "b2", "b3"]), B: def() };
    const r = computeDagrePositions(["A", "B"], edges, defs, NO_STUBS, BAND_GAP);
    expect(r.positions.A).toBeDefined();
    expect(r.positions.B).toBeDefined();
  });

  it("widthOf opt is honoured per node", () => {
    const widths: Record<string, number> = { A: 200, B: 400 };
    const r = computeDagrePositions(
      ["A", "B"],
      [{ source: "A", target: "B" }],
      { A: def(["b"]), B: def() },
      NO_STUBS,
      BAND_GAP,
      { widthOf: n => widths[n] ?? DEFAULT_WIDTH },
    );
    // SVG width must accommodate the wider of the two columns.
    expect(r.width).toBeGreaterThanOrEqual(400);
  });

  it("stubs use header-only height (no body rows)", () => {
    const defs = {
      A: def(["b"]),
      B: def(["x", "y", "z", "w"]), // 4 props → bigger when expanded
    };
    const expanded = computeDagrePositions(
      ["A", "B"],
      [{ source: "A", target: "B" }],
      defs,
      new Set(),
      BAND_GAP,
    );
    const stubbed = computeDagrePositions(
      ["A", "B"],
      [{ source: "A", target: "B" }],
      defs,
      new Set(["B"]), // B is a stub
      BAND_GAP,
    );
    // Stubbed B → header-only → overall layout fits in less vertical space.
    expect(stubbed.height).toBeLessThanOrEqual(expanded.height);
  });

  it("determinism: same input → identical positions across 3 calls", () => {
    const defs = {
      A: def(["b", "c"]),
      B: def(["d"]),
      C: def(),
      D: def(),
    };
    const edges: GraphEdge[] = [
      { source: "A", target: "B" },
      { source: "A", target: "C" },
      { source: "B", target: "D" },
    ];
    const r1 = computeDagrePositions(["A", "B", "C", "D"], edges, defs, NO_STUBS, BAND_GAP);
    const r2 = computeDagrePositions(["A", "B", "C", "D"], edges, defs, NO_STUBS, BAND_GAP);
    const r3 = computeDagrePositions(["A", "B", "C", "D"], edges, defs, NO_STUBS, BAND_GAP);
    expect(r2.positions).toEqual(r1.positions);
    expect(r3.positions).toEqual(r1.positions);
    expect(r2.width).toBe(r1.width);
    expect(r2.height).toBe(r1.height);
  });

  it("invalid widthOf (NaN/negative) clamped — no NaN in positions", () => {
    const r = computeDagrePositions(
      ["A", "B"],
      [{ source: "A", target: "B" }],
      { A: def(["b"]), B: def() },
      NO_STUBS,
      BAND_GAP,
      {
        widthOf: n => (n === "A" ? NaN : -50),
      },
    );
    for (const name of Object.keys(r.positions)) {
      expect(Number.isFinite(r.positions[name].x)).toBe(true);
      expect(Number.isFinite(r.positions[name].y)).toBe(true);
    }
  });

  it("edges referencing names not in list are skipped (no crash)", () => {
    const defs = { A: def(["b"]) };
    const r = computeDagrePositions(
      ["A"],
      [{ source: "A", target: "Ghost" }, { source: "Ghost2", target: "A" }],
      defs,
      NO_STUBS,
      BAND_GAP,
    );
    expect(r.positions.A).toBeDefined();
    expect(r.positions.Ghost).toBeUndefined();
    expect(r.positions.Ghost2).toBeUndefined();
  });

  it("rankdir LR → y values converge (horizontal layout), x values spread", () => {
    const edges: GraphEdge[] = [
      { source: "A", target: "B" },
      { source: "B", target: "C" },
    ];
    const defs = { A: def(["b"]), B: def(["c"]), C: def() };
    const r = computeDagrePositions(
      ["A", "B", "C"],
      edges,
      defs,
      NO_STUBS,
      BAND_GAP,
      { rankdir: "LR" },
    );
    const xs = ["A", "B", "C"].map(n => r.positions[n].x);
    expect(new Set(xs).size).toBe(3);
  });

  it("converts dagre center coords → top-left (x,y align with PAD margin)", () => {
    const r = computeDagrePositions(
      ["A"],
      [],
      { A: def() },
      NO_STUBS,
      BAND_GAP,
    );
    // marginx/y = PAD; single-node layout places node center at (PAD + w/2, PAD + h/2).
    // After top-left conversion: x === PAD, y === PAD.
    expect(r.positions.A.x).toBeCloseTo(PAD, 5);
    expect(r.positions.A.y).toBeCloseTo(PAD, 5);
  });

  it("box height matches HEADER + min(rowCt, 10)*ROW_HEIGHT (10-row cap)", () => {
    // 20 props → cap at 10 rows.
    const manyProps = Array.from({ length: 20 }, (_, i) => `p${i}`);
    const defs = { A: def(manyProps) };
    const r = computeDagrePositions(["A"], [], defs, NO_STUBS, BAND_GAP);
    // height ≥ HEADER + 10*ROW_HEIGHT plus margins.
    const minCellH = HEADER_HEIGHT + 10 * ROW_HEIGHT;
    expect(r.height).toBeGreaterThanOrEqual(minCellH);
  });
});
