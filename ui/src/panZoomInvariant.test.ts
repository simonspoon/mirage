/// <reference types="vite/client" />
import { describe, it, expect } from 'vitest';
// Vite/vitest `?raw` import: loads the file as a string without pulling in
// @types/node (which this package does not depend on). Keeps `pnpm tsc
// --noEmit` clean of new errors. Typed by the vite/client triple-slash
// reference above.
import indexSrc from './index.tsx?raw';

// ── Pan/zoom regression guard (task smeu / rwgk) ─────────────────────────────
// Source-scan invariant for ui/src/index.tsx. Guards three properties:
//   1. Total setGraphPan/setGraphZoom call-site count is pinned — fail loud
//      if a new writer is introduced anywhere (caller must audit + update).
//   2. Hover / edge / badge-popover / tooltip syntactic blocks contain NO
//      direct pan/zoom setter calls AND NO animatePanZoom()/fitGraph() calls
//      (the latter guard catches indirect writes via free helpers that would
//      escape a raw-setter grep).
//   3. Every setGraphPan / setGraphZoom write resides within one of five
//      allowed syntactic regions: wheel handler, pointerMove drag handler,
//      fitGraph body, animatePanZoom arrow body, focusSet createEffect.
//
// Test is source-scan only (no DOM). vitest env is 'node' per ui/vitest.config.ts.
// Line numbers are NEVER asserted — count + brace-balanced syntactic ranges
// keep the test robust against edits that shift line offsets.

// Replace every char inside a range with a space (preserves indices).
function blankRange(out: string[], start: number, end: number): void {
  for (let i = start; i < end && i < out.length; i++) {
    // Keep newlines so line-based debugging still works if ever needed.
    if (out[i] !== '\n') out[i] = ' ';
  }
}

// Strip // line comments, /* */ block comments, and single/double/backtick
// string literals (including TS string escapes). Preserves byte/char indices
// so all downstream anchor matches line up with the raw source.
function stripCommentsAndStrings(src: string): string {
  // Normalise CRLF first so indices are consistent.
  const norm = src.replace(/\r\n/g, '\n');
  const out = norm.split('');
  let i = 0;
  while (i < norm.length) {
    const c = norm[i];
    const c2 = norm[i + 1];
    if (c === '/' && c2 === '/') {
      // Line comment to end of line (exclusive).
      let j = i + 2;
      while (j < norm.length && norm[j] !== '\n') j++;
      blankRange(out, i, j);
      i = j;
      continue;
    }
    if (c === '/' && c2 === '*') {
      // Block comment.
      let j = i + 2;
      while (j < norm.length && !(norm[j] === '*' && norm[j + 1] === '/')) j++;
      j = Math.min(norm.length, j + 2);
      blankRange(out, i, j);
      i = j;
      continue;
    }
    if (c === '"' || c === "'" || c === '`') {
      const quote = c;
      let j = i + 1;
      while (j < norm.length) {
        const cj = norm[j];
        if (cj === '\\') {
          j += 2;
          continue;
        }
        if (cj === quote) {
          j++;
          break;
        }
        // Template literal ${...} can embed code; conservative — treat entire
        // backtick body as opaque. index.tsx does not need ${} guard-bypass
        // for this test (no forbidden tokens live inside template strings).
        j++;
      }
      blankRange(out, i, j);
      i = j;
      continue;
    }
    i++;
  }
  return out.join('');
}

// Walk brace-balanced starting at the first '{' at or after `fromIdx`.
// Returns [openIdx, closeIdxExclusive]. Throws if unbalanced.
function braceBody(s: string, fromIdx: number): [number, number] {
  let open = s.indexOf('{', fromIdx);
  if (open < 0) throw new Error(`braceBody: no '{' after index ${fromIdx}`);
  let depth = 0;
  for (let i = open; i < s.length; i++) {
    const ch = s[i];
    if (ch === '{') depth++;
    else if (ch === '}') {
      depth--;
      if (depth === 0) return [open, i + 1];
    }
  }
  throw new Error(`braceBody: unbalanced from ${open}`);
}

// All match indices for `needle` (literal string) in `hay`.
function allIndices(hay: string, needle: string): number[] {
  const idxs: number[] = [];
  let from = 0;
  while (true) {
    const i = hay.indexOf(needle, from);
    if (i < 0) break;
    idxs.push(i);
    from = i + needle.length;
  }
  return idxs;
}

// Find unique anchor. Throws if anchor missing or ambiguous.
function uniqueIndex(hay: string, needle: string): number {
  const idxs = allIndices(hay, needle);
  if (idxs.length !== 1) {
    throw new Error(
      `uniqueIndex: expected exactly 1 match of ${JSON.stringify(needle)}, got ${idxs.length}`,
    );
  }
  return idxs[0];
}

describe('panZoomInvariant — ui/src/index.tsx source scan', () => {
  const stripped = stripCommentsAndStrings(indexSrc);

  // ── (1) Count check ────────────────────────────────────────────────────
  // Pinned expected totals. These include both the signal declarations
  // (setGraphPan / setGraphZoom destructured from createSignal) AND every
  // call site that invokes the setter. Any drift — new writer, new helper,
  // deletion of a writer, rename — fails this assertion and forces the
  // author to revisit the allowlist below.
  //
  // Current inventory (do NOT treat as assertion — scout reference only):
  //   setGraphPan  : decl L275, prop wire L1786, Setter<> L2267, pointerMove
  //                  drag L3472, fitGraph body L3506, animatePanZoom step
  //                  L3541, focusSet first-fit L3584  → 7 occurrences
  //   setGraphZoom : decl L276, prop wire L1788, Setter<> L2269, wheel L3431,
  //                  fitGraph body L3505, animatePanZoom step L3540, focusSet
  //                  first-fit L3583                   → 7 occurrences
  it('setGraphPan + setGraphZoom occurrence count is pinned', () => {
    const panCount = allIndices(stripped, 'setGraphPan').length;
    const zoomCount = allIndices(stripped, 'setGraphZoom').length;
    // 8 each, because JSX prop forwarding like `setGraphPan={setGraphPan}`
    // produces two identifier occurrences on the same line. Break-down:
    //   setGraphPan  : decl (1) + JSX attr+value (2) + Setter<> type (1) +
    //                  4 writer call sites (pointerMove, fitGraph, anim step,
    //                  focusSet first-fit) = 8.
    //   setGraphZoom : same shape = 8 (writer sites: wheel, fitGraph, anim
    //                  step, focusSet first-fit).
    expect(panCount).toBe(8);
    expect(zoomCount).toBe(8);
  });

  // ── (2) Hover / edge / badge-popover / tooltip syntactic-block absence ──
  // Forbidden tokens in any hover/edge block: setGraphPan, setGraphZoom,
  // animatePanZoom(, fitGraph(. The free-function guards catch indirect pan/
  // zoom writes that would slip past a raw-setter grep.
  const FORBIDDEN = ['setGraphPan', 'setGraphZoom', 'animatePanZoom(', 'fitGraph('];

  // Block A: arrow-function bodies containing setHoveredEdgeId(). For each
  // such call, walk up from the anchor to the nearest unmatched '{' and
  // forward to its matching '}'. Captures `onMouseEnter={() => ...}` style
  // inline handlers AND multi-statement arrow bodies equally.
  const hoverAnchors = allIndices(stripped, 'setHoveredEdgeId(');
  it('hover anchors exist (guards grep regression on the anchor itself)', () => {
    // Scout: 4 sites (L3261, L3268, L3867-3868 pair, L3885-3886 pair).
    // setHoveredEdgeId is called twice per JSX handler pair, so count is 6.
    expect(hoverAnchors.length).toBeGreaterThanOrEqual(4);
  });

  function enclosingArrowBody(anchor: number): [number, number] {
    // Walk backward: track unmatched '{' by counting '}'. First '{' whose
    // matching '}' sits past `anchor` wraps the anchor.
    let depth = 0;
    for (let i = anchor; i >= 0; i--) {
      const ch = stripped[i];
      if (ch === '}') depth++;
      else if (ch === '{') {
        if (depth === 0) {
          // Found unmatched opener. Now find its matching close.
          const [, close] = braceBody(stripped, i);
          return [i, close];
        }
        depth--;
      }
    }
    throw new Error(`enclosingArrowBody: no enclosing '{' for anchor ${anchor}`);
  }

  it('no forbidden pan/zoom tokens in hover-handler syntactic blocks', () => {
    for (const anchor of hoverAnchors) {
      const [start, end] = enclosingArrowBody(anchor);
      const block = stripped.slice(start, end);
      for (const bad of FORBIDDEN) {
        expect(
          block.includes(bad),
          `forbidden token ${JSON.stringify(bad)} found in hover block containing setHoveredEdgeId anchor at ${anchor}`,
        ).toBe(false);
      }
    }
  });

  // Block B: known edge-JSX region. Anchored by two unique text markers —
  // start at the edge route memoisation, end at the closing </For> for the
  // edge callback. Covers the full visible-path / hit-area / label JSX that
  // carries onMouseEnter/Leave and native <title> tooltips.
  it('no forbidden pan/zoom tokens in edge-JSX region', () => {
    // Start: edge routeInfo declaration (unique, top of edge callback render body).
    // End:   entity-box <For> which immediately follows the edge </For>.
    //        Two occurrences of this tag exist in the file (schemas graph +
    //        outer); the one directly after `routeInfo` closes the edge
    //        block, so we take the first match AFTER start.
    const startMark = 'const routeInfo =';
    const start = uniqueIndex(stripped, startMark);
    const entityBoxFor = '<For each={visibleList()}>';
    const end = stripped.indexOf(entityBoxFor, start + startMark.length);
    expect(end).toBeGreaterThan(start);
    const block = stripped.slice(start, end);
    for (const bad of FORBIDDEN) {
      expect(
        block.includes(bad),
        `forbidden token ${JSON.stringify(bad)} found in edge-JSX region`,
      ).toBe(false);
    }
  });

  // ── (3) Allowlist: every setter write sits inside an allowed region ────
  // Five allowed regions, each defined by a unique anchor + brace-balanced
  // body. A "write" is any occurrence of `setGraphPan(` or `setGraphZoom(`
  // (NOT the bare identifier — signal decls, prop wires, and Setter<> type
  // refs don't end in '(' and are filtered out by this rule).
  it('every setGraphPan/setGraphZoom call site sits in an allowed region', () => {
    // Anchors — each must be unique in the file.
    const wheelAnchor = uniqueIndex(stripped, 'const wheelHandler = (e: WheelEvent) =>');
    const pointerMoveAnchor = uniqueIndex(stripped, 'const handlePointerMove = (e: PointerEvent) =>');
    const fitGraphAnchor = uniqueIndex(stripped, 'const fitGraph = () =>');
    const animatePanZoomAnchor = uniqueIndex(stripped, 'const animatePanZoom = (');
    const focusSetAnchor = uniqueIndex(stripped, 'let firstFocusFit = true;');

    const ranges: Array<[string, number, number]> = [
      ['wheel', ...braceBody(stripped, wheelAnchor)],
      ['pointerMove', ...braceBody(stripped, pointerMoveAnchor)],
      ['fitGraph', ...braceBody(stripped, fitGraphAnchor)],
      ['animatePanZoom', ...braceBody(stripped, animatePanZoomAnchor)],
      ['focusSetEffect', ...braceBody(stripped, focusSetAnchor)],
    ];

    const writerSites = [
      ...allIndices(stripped, 'setGraphPan('),
      ...allIndices(stripped, 'setGraphZoom('),
    ];
    expect(writerSites.length).toBeGreaterThan(0);

    for (const site of writerSites) {
      const inside = ranges.some(([, s, e]) => site >= s && site < e);
      expect(
        inside,
        `setter call at char ${site} not inside any allowed region (${ranges.map(r => r[0]).join(', ')})`,
      ).toBe(true);
    }
  });
});
