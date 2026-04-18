// crossings-check.ts — Petstore-like fixture comparison: naive vs barycenter.
// Run via: npx tsx src/crossings-check.ts
// Exits 1 if crossings_after >= crossings_before.

import { computeDagPositions, GraphEdge } from './dagLayout';

const BOX_SPACING = 300;
const BAND_GAP = 40;

// ── Petstore-like fixture ──────────────────────────────────────────────────────
// Band 0 (roots — not referenced by anything as a source): Pet, Order, User
// Band 1 (direct children): Tag, Category, Address
// Band 2 (deeper): Status
//
// Edges (source references target):
//   Tag      → Pet      (Tag is a property of Pet)
//   Category → Pet      (Category is also a property of Pet)
//   Address  → User     (Address is a property of User)
//   Status   → Order    (Status is a property of Order)
//
// Naive x-order in band 1 (visibleList order): Tag(0), Category(1), Address(2)
//   Pet is at x=20, Order at x=320, User at x=620
//   Tag→Pet:      Tag(x=20)  → Pet(x=20)     no cross
//   Category→Pet: Cat(x=320) → Pet(x=20)     right→left
//   Address→User: Addr(x=620)→ User(x=620)   no cross
//   Status→Order: Status(x=20)→ Order(x=320) left→right
//
// Add deliberate extra crossing: swap order so Address references Pet (not User)
// and Tag references User — this creates crossings in naive order.
//
// Concrete crossing arrangement:
//   Band 0: Pet(0), Order(1), User(2)   → x = 20, 320, 620
//   Band 1: Tag, Category, Address       → naive x = 20, 320, 620
//   Edges:
//     Tag      → User     (x=20 → x=620: far right)
//     Category → Pet      (x=320 → x=20: goes left)
//     Address  → Order    (x=620 → x=320: goes left)
//   This produces crossings between Tag→User and Category→Pet,
//   and between Tag→User and Address→Order.
//   Barycenter should pull Tag rightward (its parent User is at index 2),
//   reducing crossings.

const list = ['Pet', 'Order', 'User', 'Tag', 'Category', 'Address'];

const edges: GraphEdge[] = [
  { source: 'Tag', target: 'User' },      // Tag references User
  { source: 'Category', target: 'Pet' },  // Category references Pet
  { source: 'Address', target: 'Order' }, // Address references Order
];

// ── Crossing counter ────────────────────────────────────────────────────────
// For each pair of cross-band edges, check if they cross (inversion count).
function countCrossings(
  positions: Record<string, { x: number; y: number }>,
  edgeList: GraphEdge[],
): number {
  // Build (upperBandX, lowerBandX) pairs for each cross-band edge
  const pairs: Array<[number, number]> = [];
  for (const e of edgeList) {
    const s = positions[e.source];
    const t = positions[e.target];
    if (!s || !t) continue;
    if (s.y === t.y) continue; // skip intra-band edges
    // Smaller y = higher band (band 0 is at top)
    const [upperX, lowerX] = s.y < t.y ? [s.x, t.x] : [t.x, s.x];
    pairs.push([upperX, lowerX]);
  }

  let crossings = 0;
  for (let i = 0; i < pairs.length; i++) {
    for (let j = i + 1; j < pairs.length; j++) {
      const [u1, l1] = pairs[i];
      const [u2, l2] = pairs[j];
      if ((u1 < u2 && l1 > l2) || (u1 > u2 && l1 < l2)) crossings++;
    }
  }
  return crossings;
}

// ── Run comparison ──────────────────────────────────────────────────────────
const naive = computeDagPositions(
  list, edges, {}, new Set<string>(), BOX_SPACING, BAND_GAP,
  { barycenter: false },
);
const bary = computeDagPositions(
  list, edges, {}, new Set<string>(), BOX_SPACING, BAND_GAP,
  { barycenter: true },
);

const before = countCrossings(naive.positions, edges);
const after = countCrossings(bary.positions, edges);

console.log(`before=${before} after=${after}`);

if (after >= before) {
  console.error(`FAIL: crossings_after (${after}) >= crossings_before (${before})`);
  process.exit(1);
}

console.log('PASS: barycenter reduced edge crossings');
