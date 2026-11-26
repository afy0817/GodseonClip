// npm run test:ts — where the picture goes on the stage with 화면 효과 on: the turned <video>'s CSS,
// the crop box over the seen picture, and the 결과 view that shows only what the export contains.

import assert from "node:assert/strict";
import { test } from "node:test";
import type { EffectsPlan, Orientation } from "./effects.ts";
import { type Box, editLayout, fitBox, inset, onStage, resultLayout, videoCss } from "./stageLayout.ts";

const STAGE: Box = { left: 0, top: 0, width: 1000, height: 600 };
const NONE: Orientation = { turn: 0, flipH: false, flipV: false };

const near = (a: Box, b: Box) => {
  for (const k of ["left", "top", "width", "height"] as const) assert.ok(Math.abs(a[k] - b[k]) < 1e-6, `${k}: ${a[k]} vs ${b[k]}`);
};

test("fitBox centers the largest box of the ratio", () => {
  near(fitBox(STAGE, [1920, 1080]), { left: 0, top: 18.75, width: 1000, height: 562.5 });
  near(fitBox(STAGE, [1080, 1920]), { left: 331.25, top: 0, width: 337.5, height: 600 });
  near(inset(STAGE, 12), { left: 12, top: 12, width: 976, height: 576 });
  assert.equal(fitBox(STAGE, [0, 0]).width, 0);
});

test("videoCss: the upright element turned and mirrored onto the seen box", () => {
  const seen: Box = { left: 331.25, top: 0, width: 337.5, height: 600 };
  const css = videoCss(seen, { turn: 1, flipH: false, flipV: false });
  // Laid out upright (600 × 337.5) around the same center, then turned.
  assert.deepEqual([css.width, css.height, css.transform], [600, 337.5, "rotate(90deg)"]);
  assert.ok(Math.abs(css.left + css.width / 2 - 500) < 1e-9 && Math.abs(css.top + css.height / 2 - 300) < 1e-9);
  assert.equal(videoCss(seen, NONE).transform, "none");
  assert.equal(videoCss(seen, { turn: 3, flipH: true, flipV: false }).transform, "scale(-1, 1) rotate(270deg)");
  assert.equal(videoCss(seen, { turn: 0, flipH: false, flipV: true }).transform, "scale(1, -1)");
});

test("editLayout and onStage: the crop box over the seen picture", () => {
  const { seen, scale } = editLayout(STAGE, [3840, 2160], NONE);
  near(seen, { left: 0, top: 18.75, width: 1000, height: 562.5 });
  assert.ok(Math.abs(scale - 1000 / 3840) < 1e-12);
  near(onStage({ x: 1312, y: 0, width: 1216, height: 2160 }, seen, scale), { left: 1312 * scale, top: 18.75, width: 1216 * scale, height: 562.5 });
  const turned = editLayout(STAGE, [3840, 2160], { turn: 1, flipH: false, flipV: false });
  near(turned.seen, { left: 331.25, top: 0, width: 337.5, height: 600 });
});

const plan = (p: Partial<EffectsPlan>): EffectsPlan => ({ masks: [], overlays: [], crop: null, turn: 0, flipH: false, flipV: false, scale: null, pad: null, size: [1920, 1080], ...p });

test("result view: the crop fills the frame", () => {
  // 9:16 box out of 4K, scaled to 1080 × 1920: the frame is 337.5 × 600, the box's left edge at its left.
  const p = plan({ crop: { x: 1312, y: 0, width: 1216, height: 2160 }, scale: [1080, 1920], size: [1080, 1920] });
  const { frame, picture, seen } = resultLayout(STAGE, [3840, 2160], p);
  near(frame, { left: 331.25, top: 0, width: 337.5, height: 600 });
  near(picture, frame);
  const kx = 337.5 / 1216;
  const ky = 600 / 2160;
  near(seen, { left: 331.25 - 1312 * kx, top: 0, width: 3840 * kx, height: 2160 * ky });
});

test("result view: 맞춤 puts the picture inside the bars, a turn swaps the seen size", () => {
  // 1920 × 1080 turned right, contained in 1080 × 1080: picture 608 × 1080 at x 236.
  const p = plan({ turn: 1, scale: [608, 1080], pad: { width: 1080, height: 1080, x: 236, y: 0 }, size: [1080, 1080] });
  const { frame, picture, seen } = resultLayout(STAGE, [1920, 1080], p);
  near(frame, { left: 200, top: 0, width: 600, height: 600 });
  const k = 600 / 1080;
  near(seen, { left: 200 + 236 * k, top: 0, width: 608 * k, height: 1080 * k });
  near(picture, seen);
});

test("result view: 맞춤 with bars but no scaling", () => {
  // A 608 × 1080 box out of 1920 × 1080 on a 1080 × 1080 frame: bars of 236 each side.
  const p = plan({ crop: { x: 656, y: 0, width: 608, height: 1080 }, pad: { width: 1080, height: 1080, x: 236, y: 0 }, size: [1080, 1080] });
  const { frame, picture, seen } = resultLayout(STAGE, [1920, 1080], p);
  const k = 600 / 1080;
  near(seen, { left: 200 + 236 * k - 656 * k, top: 0, width: 1920 * k, height: 1080 * k });
  assert.ok(Math.abs(frame.width - 600) < 1e-9);
  // The video is clipped to the picture between the bars, not to the frame.
  near(picture, { left: 200 + 236 * k, top: 0, width: 608 * k, height: 600 });
});

test("result view: 늘림 stretches the seen picture", () => {
  const p = plan({ scale: [1080, 1080], size: [1080, 1080] });
  const { frame, seen } = resultLayout(STAGE, [1920, 1080], p);
  near(seen, frame);
  const css = videoCss(seen, p);
  assert.deepEqual([css.width, css.height], [600, 600]);
});
