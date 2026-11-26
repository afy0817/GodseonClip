// npm run test:ts — the cases of command::plan_lossless_cut's tests in src-tauri/src/command.rs.

import assert from "node:assert/strict";
import { test } from "node:test";
import { keepFromRemoved, type Range } from "./cutList.ts";
import { losslessCutPreview, removalOutcomes } from "./losslessCut.ts";

const r = (start: number, end: number): Range => ({ start, end });
const GOP2 = [0, 2, 4, 6, 8];
const MIN = 0.999 / 25;
const preview = (removed: Range[], mov: boolean, keyframes = GOP2) => losslessCutPreview(keepFromRemoved(removed, 10, MIN), keyframes, 10, mov);

test("lossless_cut_copies_every_piece_from_its_keyframe", () => {
  const removed = [r(8.5, 10), r(5, 5.5), r(1, 2.4)];
  for (const mov of [true, false]) {
    const p = preview(removed, mov);
    assert.deepEqual(p.keptRemovals, [r(5, 5.5)]);
    assert.equal(p.exactStart, false);
    assert.deepEqual(
      p.pieces.map((x) => [x.keep, x.visibleStart]),
      [
        [r(0, 1), 0],
        [r(2.4, 8.5), 2],
      ],
    );
    assert.ok(Math.abs(p.length - 7.5) < 1e-9, "Rust's length is 1 ms per piece shorter (the end margin)");
    assert.equal(p.problem, null);
  }
});

test("lossless_cut_with_one_piece_left_is_a_trim", () => {
  const mp4 = preview([r(0, 3.3)], true);
  assert.deepEqual([mp4.pieces.length, mp4.pieces[0].visibleStart, mp4.exactStart], [1, 3.3, true]);
  const mkv = preview([r(0, 3.3)], false);
  assert.deepEqual([mkv.pieces[0].visibleStart, mkv.exactStart, mkv.length], [2, false, 8]);
  const joined = preview([r(5, 5.5), r(9, 10)], true);
  assert.deepEqual([joined.pieces.length, joined.pieces[0].keep, joined.keptRemovals], [1, r(0, 9), [r(5, 5.5)]]);
});

test("lossless_cut_refuses_what_it_cannot_do", () => {
  assert.deepEqual(preview([r(5, 5.5)], true).problem, { code: "nothingRemovableLossless" });
  assert.deepEqual(preview([r(0, 1.5)], false).problem, { code: "nothingRemovableLossless" }, "pulled back to 0");
  assert.deepEqual(preview([r(1, 2)], true, []).problem, { code: "cutNeedsKeyframes" });
  assert.equal(losslessCutPreview([], GOP2, 10, true).pieces.length, 0);
});

test("what happens to each removed range", () => {
  const removed = [r(1, 2.4), r(5, 5.5), r(6, 7), r(8.5, 10)];
  const p = preview(removed, false);
  // 1–2.4: the piece after starts at keyframe 2 → 0.4 s of it comes back. 5–5.5: stays.
  // 6–7: the piece after starts at keyframe 6 = its start → all of it comes back… so it is kept.
  // 8.5–10: runs to the end.
  assert.deepEqual(
    removalOutcomes(removed, p).map((o) => [o.kept, Number(o.back.toFixed(6))]),
    [
      [false, 0.4],
      [true, 0],
      [true, 0],
      [false, 0],
    ],
  );
  // A single MP4 piece starts exactly where asked: nothing comes back.
  assert.deepEqual(removalOutcomes([r(0, 3.3)], preview([r(0, 3.3)], true)), [{ kept: false, back: 0 }]);
});
