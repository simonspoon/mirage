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

  it("exposes per-node ranks + rank centerline y (same rank → same centerY)", () => {
    // Two-rank fan-out: A at rank 0; B, C, D all at rank 1. Heterogeneous
    // heights on rank 1 (B tall, C short, D short) — under rankalign=center
    // every rank-1 node shares the same center y.
    const defs = {
      A: def(["b", "c", "d"]),                              // rank 0 root
      B: def(["p1", "p2", "p3", "p4", "p5", "p6"]),        // tall
      C: def(),                                             // short
      D: def(),                                             // short
    };
    const edges: GraphEdge[] = [
      { source: "A", target: "B" },
      { source: "A", target: "C" },
      { source: "A", target: "D" },
    ];
    const r = computeDagrePositions(
      ["A", "B", "C", "D"],
      edges,
      defs,
      NO_STUBS,
      BAND_GAP,
    );
    expect(r.ranks).toBeDefined();
    expect(r.rankCenterY).toBeDefined();
    const ranks = r.ranks!;
    const rankCenterY = r.rankCenterY!;
    // A on one rank, B/C/D on the other — dagre picks either 0/1 per its
    // own source→target convention. Assert cardinality instead.
    const rankA = ranks.A;
    const rankB = ranks.B;
    expect(rankA).not.toBe(rankB);
    expect(ranks.C).toBe(rankB);
    expect(ranks.D).toBe(rankB);
    // centerY map has one entry per occupied rank.
    expect(Object.keys(rankCenterY).sort()).toEqual(
      [String(rankA), String(rankB)].sort(),
    );
    // Ranks A and B occupy distinct vertical bands — different centerY.
    expect(rankCenterY[rankA]).not.toBeCloseTo(rankCenterY[rankB], 5);
    // Verify the invariant via positions+heights: center y of each rank-1
    // node = positions[n].y + height/2. Pick B (tall) and C (short) and
    // assert their center y matches rankCenterY[rankB].
    const heightOf = (n: string) => {
      const d = defs[n as keyof typeof defs];
      const rowCt = Object.keys(d.properties).length + (d.extends ? 1 : 0) || 1;
      return HEADER_HEIGHT + Math.min(rowCt, 10) * ROW_HEIGHT;
    };
    const cyB = r.positions.B.y + heightOf("B") / 2;
    const cyC = r.positions.C.y + heightOf("C") / 2;
    expect(cyB).toBeCloseTo(rankCenterY[rankB], 5);
    expect(cyC).toBeCloseTo(rankCenterY[rankB], 5);
    expect(cyB).toBeCloseTo(cyC, 5);
  });

  it("exposes per-edge polyline points keyed by GraphEdge.id", () => {
    // A→B and A→C both with explicit ids; output edges map holds both.
    const defs = { A: def(["b", "c"]), B: def(), C: def() };
    const edges: GraphEdge[] = [
      { id: "A::b::B", source: "A", target: "B" },
      { id: "A::c::C", source: "A", target: "C" },
    ];
    const r = computeDagrePositions(
      ["A", "B", "C"],
      edges,
      defs,
      NO_STUBS,
      BAND_GAP,
    );
    expect(r.edges).toBeDefined();
    const emap = r.edges!;
    // Both ids present, each with >=2 points, all finite.
    expect(emap["A::b::B"]).toBeDefined();
    expect(emap["A::c::C"]).toBeDefined();
    expect(emap["A::b::B"].points.length).toBeGreaterThanOrEqual(2);
    expect(emap["A::c::C"].points.length).toBeGreaterThanOrEqual(2);
    for (const k of Object.keys(emap)) {
      for (const p of emap[k].points) {
        expect(Number.isFinite(p.x)).toBe(true);
        expect(Number.isFinite(p.y)).toBe(true);
      }
    }
  });

  it("duplicate GraphEdge.id values do not collide — both preserved", () => {
    // Two separate edges A→B sharing the same caller id (pathological but
    // possible if caller constructs ids naively). Both must appear in edges
    // map with distinct keys (second gets #N suffix).
    const defs = { A: def(["x", "y"]), B: def() };
    const edges: GraphEdge[] = [
      { id: "dup", source: "A", target: "B" },
      { id: "dup", source: "A", target: "B" },
    ];
    const r = computeDagrePositions(["A", "B"], edges, defs, NO_STUBS, BAND_GAP);
    expect(r.edges).toBeDefined();
    const keys = Object.keys(r.edges!).sort();
    expect(keys.length).toBe(2);
    // Both keys retain the caller-supplied prefix.
    for (const k of keys) expect(k.startsWith("dup")).toBe(true);
  });

  it("edges without id get synthetic keys, still populated", () => {
    const defs = { A: def(["b"]), B: def() };
    const edges: GraphEdge[] = [{ source: "A", target: "B" }];
    const r = computeDagrePositions(["A", "B"], edges, defs, NO_STUBS, BAND_GAP);
    expect(r.edges).toBeDefined();
    expect(Object.keys(r.edges!).length).toBe(1);
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

  // ── refKind === "extends" inversion ─────────────────────────────────────
  // Dagre-layer inversion places the extends parent ABOVE the extending
  // child under rankdir=TB (matches UML parent-above convention) while
  // keeping the caller-facing edge source/target / id semantics intact.
  describe("refKind='extends' inversion", () => {
    it("extends edge: parent ranks ABOVE child under rankdir=TB", () => {
      // Child → Parent extends edge. After inversion, parent (BaseAudit)
      // ranks lower (numerically smaller rank = higher on screen under TB)
      // than the extending child (ComposedEntity).
      const defs = {
        ComposedEntity: def([], "BaseAudit"),
        BaseAudit: def(["created_at"]),
      };
      const edges: GraphEdge[] = [
        {
          id: "ComposedEntity::extends::BaseAudit",
          source: "ComposedEntity",
          target: "BaseAudit",
          refKind: "extends",
        },
      ];
      const r = computeDagrePositions(
        ["ComposedEntity", "BaseAudit"],
        edges,
        defs,
        NO_STUBS,
        BAND_GAP,
      );
      expect(r.ranks).toBeDefined();
      const ranks = r.ranks!;
      // parent rank < child rank (parent visually above child)
      expect(ranks.BaseAudit).toBeLessThan(ranks.ComposedEntity);
      // Positional y also confirms: smaller y = higher on screen.
      expect(r.positions.BaseAudit.y).toBeLessThan(r.positions.ComposedEntity.y);
    });

    it("property-ref edge: target ranks BELOW source (unchanged)", () => {
      // Non-extends property reference. Standard dagre direction: source
      // ranks above target.
      const defs = {
        ComposedEntity: def(["owner"]),
        Owner: def(),
      };
      const edges: GraphEdge[] = [
        {
          id: "ComposedEntity::owner::Owner",
          source: "ComposedEntity",
          target: "Owner",
          refKind: "ref",
        },
      ];
      const r = computeDagrePositions(
        ["ComposedEntity", "Owner"],
        edges,
        defs,
        NO_STUBS,
        BAND_GAP,
      );
      const ranks = r.ranks!;
      expect(ranks.Owner).toBeGreaterThan(ranks.ComposedEntity);
      expect(r.positions.Owner.y).toBeGreaterThan(r.positions.ComposedEntity.y);
    });

    it("extends edge points: pts[0] (child side) below pts[last] (parent side)", () => {
      // Post-reversal invariant: first point = semantic source (child) end,
      // last point = semantic target (parent) end. Under TB with parent
      // above child, pts[0].y > pts[last].y.
      const defs = {
        ComposedEntity: def([], "BaseAudit"),
        BaseAudit: def(),
      };
      const edges: GraphEdge[] = [
        {
          id: "ComposedEntity::extends::BaseAudit",
          source: "ComposedEntity",
          target: "BaseAudit",
          refKind: "extends",
        },
      ];
      const r = computeDagrePositions(
        ["ComposedEntity", "BaseAudit"],
        edges,
        defs,
        NO_STUBS,
        BAND_GAP,
      );
      expect(r.edges).toBeDefined();
      const pts = r.edges!["ComposedEntity::extends::BaseAudit"].points;
      expect(pts.length).toBeGreaterThanOrEqual(2);
      expect(pts[0].y).toBeGreaterThan(pts[pts.length - 1].y);
    });

    it("property-ref edge points: pts[0] (source side) above pts[last] (target side)", () => {
      const defs = {
        ComposedEntity: def(["owner"]),
        Owner: def(),
      };
      const edges: GraphEdge[] = [
        {
          id: "ComposedEntity::owner::Owner",
          source: "ComposedEntity",
          target: "Owner",
          refKind: "ref",
        },
      ];
      const r = computeDagrePositions(
        ["ComposedEntity", "Owner"],
        edges,
        defs,
        NO_STUBS,
        BAND_GAP,
      );
      const pts = r.edges!["ComposedEntity::owner::Owner"].points;
      expect(pts.length).toBeGreaterThanOrEqual(2);
      expect(pts[0].y).toBeLessThan(pts[pts.length - 1].y);
    });

    it("mixed focal: extends parent above, property target below focal", () => {
      // ComposedEntity extends BaseAudit AND has property owner → Owner.
      // Expected ranks: BaseAudit < ComposedEntity < Owner.
      const defs = {
        ComposedEntity: def(["owner"], "BaseAudit"),
        BaseAudit: def(["created_at"]),
        Owner: def(),
      };
      const edges: GraphEdge[] = [
        {
          id: "ComposedEntity::extends::BaseAudit",
          source: "ComposedEntity",
          target: "BaseAudit",
          refKind: "extends",
        },
        {
          id: "ComposedEntity::owner::Owner",
          source: "ComposedEntity",
          target: "Owner",
          refKind: "ref",
        },
      ];
      const r = computeDagrePositions(
        ["ComposedEntity", "BaseAudit", "Owner"],
        edges,
        defs,
        NO_STUBS,
        BAND_GAP,
      );
      const ranks = r.ranks!;
      expect(ranks.BaseAudit).toBeLessThan(ranks.ComposedEntity);
      expect(ranks.Owner).toBeGreaterThan(ranks.ComposedEntity);
      // BaseAudit and Owner MUST end up on different ranks — otherwise
      // they read as siblings (the exact bug this task fixes).
      expect(ranks.BaseAudit).not.toBe(ranks.Owner);
    });

    it("multi-level extends chain: strict rank ordering parent-above", () => {
      // GrandChild extends Child; Child extends Parent. Expect rank
      // ordering Parent < Child < GrandChild.
      const defs = {
        GrandChild: def([], "Child"),
        Child: def([], "Parent"),
        Parent: def(),
      };
      const edges: GraphEdge[] = [
        {
          id: "GrandChild::extends::Child",
          source: "GrandChild",
          target: "Child",
          refKind: "extends",
        },
        {
          id: "Child::extends::Parent",
          source: "Child",
          target: "Parent",
          refKind: "extends",
        },
      ];
      const r = computeDagrePositions(
        ["GrandChild", "Child", "Parent"],
        edges,
        defs,
        NO_STUBS,
        BAND_GAP,
      );
      const ranks = r.ranks!;
      expect(ranks.Parent).toBeLessThan(ranks.Child);
      expect(ranks.Child).toBeLessThan(ranks.GrandChild);
    });

    it("missing parent from visible list: no crash, child positioned", () => {
      // extends target not in the list — dagreLayout should skip the edge
      // and still return a valid layout (does NOT invent the missing node).
      const defs = {
        ComposedEntity: def([], "BaseAudit"),
      };
      const edges: GraphEdge[] = [
        {
          id: "ComposedEntity::extends::BaseAudit",
          source: "ComposedEntity",
          target: "BaseAudit",
          refKind: "extends",
        },
      ];
      const r = computeDagrePositions(
        ["ComposedEntity"],
        edges,
        defs,
        NO_STUBS,
        BAND_GAP,
      );
      expect(r.positions.ComposedEntity).toBeDefined();
      expect(Number.isFinite(r.positions.ComposedEntity.x)).toBe(true);
      expect(Number.isFinite(r.positions.ComposedEntity.y)).toBe(true);
      expect(r.positions.BaseAudit).toBeUndefined();
    });

    it("edge.id preserved as map key after inversion", () => {
      // Inversion is a graph-layer implementation detail: the returned
      // edges map must still key by the original caller-supplied id
      // (child::extends::parent), not by the inverted tuple.
      const defs = {
        ComposedEntity: def([], "BaseAudit"),
        BaseAudit: def(),
      };
      const edges: GraphEdge[] = [
        {
          id: "ComposedEntity::extends::BaseAudit",
          source: "ComposedEntity",
          target: "BaseAudit",
          refKind: "extends",
        },
      ];
      const r = computeDagrePositions(
        ["ComposedEntity", "BaseAudit"],
        edges,
        defs,
        NO_STUBS,
        BAND_GAP,
      );
      expect(Object.keys(r.edges!)).toEqual(["ComposedEntity::extends::BaseAudit"]);
    });

    it("isolated node with extends to missing parent: unaffected positioning", () => {
      // Only the isolated node is in the list — its position must still be
      // finite and default (PAD-offset like any other single-node layout).
      const defs = {
        Solo: def([], "Missing"),
      };
      const edges: GraphEdge[] = [
        {
          id: "Solo::extends::Missing",
          source: "Solo",
          target: "Missing",
          refKind: "extends",
        },
      ];
      const r = computeDagrePositions(["Solo"], edges, defs, NO_STUBS, BAND_GAP);
      expect(r.positions.Solo).toBeDefined();
      expect(r.positions.Solo.x).toBeGreaterThanOrEqual(0);
      expect(r.positions.Solo.y).toBeGreaterThanOrEqual(0);
    });
  });
});
