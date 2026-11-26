// npm run test:ts — Node's own test runner on the TypeScript source (Node 22.18+ strips the types).
// The first group repeats the cases of src-tauri/src/plan.rs one for one.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  addRange,
  grabEdge,
  keepFromRemoved,
  minKeepLength,
  normalize,
  type Range,
  rangeIndexAt,
  settle,
  total,
} from "./cutList.ts";

const r = (start: number, end: number): Range => ({ start, end });

// --- plan.rs ---

test("plan_example_from_design", () => {
  // 10:00 file, remove 2:10–3:05 and 7:40–8:20 → keep 0–130, 185–460, 500–600 (8:25).
  const keep = keepFromRemoved([r(460, 500), r(130, 185)], 600, 1 / 30);
  assert.deepEqual(keep, [r(0, 130), r(185, 460), r(500, 600)]);
  assert.equal(total(keep), 505);
});

test("merges_overlapping_and_touching", () => {
  assert.deepEqual(normalize([r(5, 8), r(1, 3), r(2, 4), r(4, 5)], 10), [r(1, 8)]);
});

test("clamps_and_drops_junk", () => {
  assert.deepEqual(normalize([r(-5, 1), r(9, 20), r(3, 3), r(Number.NaN, 2)], 10), [r(0, 1), r(9, 10)]);
});

test("drops_slivers_shorter_than_a_frame", () => {
  assert.deepEqual(keepFromRemoved([r(0, 4.99), r(5, 10)], 10, 1 / 30), [], "the 0.01 s gap is shorter than one frame");
});

test("cuts_at_the_edges_leave_no_empty_pieces", () => {
  assert.deepEqual(keepFromRemoved([r(0, 2), r(8, 10)], 10, 0), [r(2, 8)]);
});

test("removing_nothing_keeps_everything", () => {
  assert.deepEqual(keepFromRemoved([], 42, 0.04), [r(0, 42)]);
});

/** plan.rs keep_list_properties, with the same xorshift PRNG and seed. */
test("keep_list_properties", () => {
  let seed = 0x9e3779b97f4a7c15n;
  const mask = (1n << 64n) - 1n;
  const rnd = () => {
    seed ^= (seed << 13n) & mask;
    seed ^= seed >> 7n;
    seed ^= (seed << 17n) & mask;
    return Number(seed >> 11n) / 2 ** 53;
  };
  for (let i = 0; i < 2000; i++) {
    const duration = 1 + rnd() * 600;
    const n = Math.floor(rnd() * 8);
    const removed: Range[] = [];
    for (let k = 0; k < n; k++) {
      const a = rnd() * duration * 1.2 - duration * 0.1;
      removed.push(r(a, a + rnd() * duration * 0.3));
    }
    const merged = normalize(removed, duration);
    const keep = keepFromRemoved(removed, duration, 0);
    for (let k = 1; k < keep.length; k++) assert.ok(keep[k - 1].end < keep[k].start);
    for (const p of keep) assert.ok(p.start >= 0 && p.end <= duration && p.end - p.start > 0);
    assert.ok(Math.abs(total(keep) + total(merged) - duration) < 1e-6);
  }
});

// --- command::min_keep_len (plan_precise_cut drops pieces under it) ---

test("a piece of exactly one frame stays", () => {
  assert.ok(Math.abs(minKeepLength(null) - 0.999 / 30) < 1e-12, "1/30 s when the rate is unknown");
  assert.deepEqual(keepFromRemoved([r(0, 4), r(4.04, 10)], 10, minKeepLength(25)), [r(4, 4.04)]);
  assert.deepEqual(keepFromRemoved([r(0, 4), r(4.03, 10)], 10, minKeepLength(25)), []);
});

// --- editing the list ---

test("adding merges and selects the range that holds the new one", () => {
  const list = [r(1, 2), r(5, 6)];
  assert.deepEqual(addRange(list, r(8, 9), 10), { ranges: [r(1, 2), r(5, 6), r(8, 9)], index: 2 });
  assert.deepEqual(addRange(list, r(1.5, 5.5), 10), { ranges: [r(1, 6)], index: 0 });
  assert.deepEqual(addRange(list, r(9, 12), 10), { ranges: [r(1, 2), r(5, 6), r(9, 10)], index: 2 }, "clamped to the file");
  assert.deepEqual(list, [r(1, 2), r(5, 6)], "the old list is left alone");
});

test("after a drag the moved range absorbs what it now overlaps", () => {
  // #1 (index 0) dragged over #2: one range, still selected.
  assert.deepEqual(settle([r(1, 5.5), r(5, 6), r(8, 9)], 0, 10), { ranges: [r(1, 6), r(8, 9)], index: 0 });
  // #3 dragged left of #1: the list is sorted again and the selection follows it.
  assert.deepEqual(settle([r(3, 4), r(5, 6), r(0.5, 1)], 2, 10), { ranges: [r(0.5, 1), r(3, 4), r(5, 6)], index: 0 });
});

test("rangeIndexAt is start-inclusive, end-exclusive", () => {
  const list = [r(1, 2), r(5, 6)];
  assert.equal(rangeIndexAt(list, 1), 0);
  assert.equal(rangeIndexAt(list, 2), null);
  assert.equal(rangeIndexAt(list, 5.5), 1);
  assert.equal(rangeIndexAt(list, 0.5), null);
});

test("grabEdge picks the nearest edge within reach", () => {
  const px = [r(100, 200), r(205, 300)];
  assert.deepEqual(grabEdge(103, px, 8), { index: 0, edge: "start" });
  assert.deepEqual(grabEdge(201, px, 8), { index: 0, edge: "end" }, "nearer #1's end than #2's start");
  assert.deepEqual(grabEdge(204, px, 8), { index: 1, edge: "start" });
  assert.equal(grabEdge(150, px, 8), null, "inside, far from both edges: select / seek");
  // A range under 2 px wide: its edges sit on each other.
  assert.deepEqual(grabEdge(50.5, [r(50, 51)], 8), { index: 0, edge: "either" });
  assert.deepEqual(grabEdge(45, [r(50, 51)], 8), { index: 0, edge: "start" });
  assert.deepEqual(grabEdge(57, [r(50, 51)], 8), { index: 0, edge: "end" });
});
