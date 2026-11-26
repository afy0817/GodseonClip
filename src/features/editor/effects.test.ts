// npm run test:ts — 화면 효과: the plan mirrors effects::plan (the same cases as effects.rs), and the
// drawing layer's geometry (source ⇄ seen, the crop box's drags and typed numbers).

import assert from "node:assert/strict";
import { test } from "node:test";
import type { Crop, Effects, Mask, Overlay, OverlaySource, VideoStream } from "../../lib/ipc";
import {
  blurSigma,
  cornerBox,
  defaultBox,
  maskShows,
  mosaicCell,
  newMask,
  newOverlay,
  overlayAfterTurn,
  overlayShows,
  dragBox,
  evenRect,
  fromSeen,
  isNone,
  NO_EFFECTS,
  type Orientation,
  pictureSize,
  planEffects,
  presetPossible,
  type Rect,
  ratioBox,
  sameOrientation,
  settled,
  shortSideSize,
  toSeen,
  turned,
  typedBox,
} from "./effects.ts";

const video = (width: number, height: number, rotation: number | null = null): VideoStream => ({
  index: 0,
  codec: "h264",
  profile: null,
  width,
  height,
  startTime: 0,
  hasBFrames: 0,
  fps: 30,
  avgFps: 30,
  pixFmt: "yuv420p",
  colorPrimaries: null,
  colorTransfer: null,
  colorSpace: null,
  hdr: false,
  rotation,
  vfrSuspect: false,
});

const crop = (x: number, y: number, width: number, height: number, ratio: [number, number] | null = null): Crop => ({ x, y, width, height, ratio });
const fx = (e: Partial<Effects>): Effects => ({ ...NO_EFFECTS, ...e });
const plan = (v: VideoStream, e: Partial<Effects>) => {
  const p = planEffects(v, fx(e));
  assert.equal(p.error, null, JSON.stringify(p.error));
  return p.plan;
};
const UHD = video(3840, 2160);

// --- the plan (effects.rs tests, same numbers) ---

test("nothing asked or nothing changed: no filters", () => {
  assert.ok(isNone(NO_EFFECTS));
  assert.equal(plan(UHD, {}), null);
  assert.equal(plan(UHD, { crop: crop(0, 0, 3840, 2160) }), null);
  assert.equal(plan(UHD, { size: { kind: "short", short: 2160 } }), null);
  assert.equal(plan(UHD, { turn: 4 }), null);
  assert.equal(plan(UHD, { size: { kind: "exact", width: 3840, height: 2160, fit: "stretch" } }), null);
});

test("crop: even, inside the picture, not too small", () => {
  const v = video(1920, 1080);
  const p = plan(v, { crop: crop(101, 51, 641, 361) })!;
  assert.deepEqual(p.crop, { x: 100, y: 50, width: 640, height: 360 });
  assert.deepEqual([p.scale, p.pad, p.size], [null, null, [640, 360]]);
  assert.deepEqual(planEffects(v, fx({ crop: crop(1300, 0, 640, 360) })).error, { code: "cropOutsidePicture", width: 1920, height: 1080 });
  assert.deepEqual(planEffects(v, fx({ crop: crop(0, 0, 62, 400) })).error, { code: "cropTooSmall", min: 64 });
});

test("crop is in the upright picture of a phone video", () => {
  const v = video(1920, 1080, -90);
  const p = plan(v, { crop: crop(0, 420, 1080, 1080, [1, 1]) })!;
  assert.deepEqual([p.crop, p.size], [{ x: 0, y: 420, width: 1080, height: 1080 }, [1080, 1080]]);
  assert.equal(planEffects(v, fx({ crop: crop(0, 0, 1920, 1080) })).error?.code, "cropOutsidePicture");
});

test("turns and flips fold and turn the size", () => {
  const v = video(1920, 1080);
  const right = plan(v, { turn: 1 })!;
  assert.deepEqual([right.turn, right.flipH, right.flipV, right.size, right.crop], [1, false, false, [1080, 1920], null]);
  assert.equal(plan(v, { turn: 7 })!.turn, 3);
  const half = plan(v, { turn: 2 })!;
  assert.deepEqual([half.turn, half.flipH, half.flipV], [0, true, true]);
  assert.equal(plan(v, { turn: 2, flipH: true, flipV: true }), null);
});

test("short side presets: scale=-2:N's numbers, never bigger", () => {
  assert.deepEqual(shortSideSize(3840, 2160, 1080), [1920, 1080]);
  assert.deepEqual(shortSideSize(1080, 1920, 720), [720, 1280]);
  assert.deepEqual(shortSideSize(1920, 1080, 480), [854, 480]);
  assert.deepEqual(shortSideSize(1918, 1080, 720), [1278, 720]);
  assert.equal(shortSideSize(1920, 1080, 1080), null);
  assert.equal(shortSideSize(1280, 720, 1080), null);
});

test("a locked ratio lands presets on the ratio", () => {
  const at = (c: Crop, short: number, turn = 0) => plan(UHD, { crop: c, turn, size: { kind: "short", short } })!;
  assert.deepEqual(at(crop(1312, 0, 1216, 2160, [9, 16]), 1080).size, [1080, 1920]);
  assert.deepEqual(at(crop(1312, 0, 1216, 2160), 1080).size, [1080, 1918]);
  assert.deepEqual(at(crop(1312, 0, 1216, 2160, [9, 16]), 480).size, [480, 854]);
  assert.deepEqual(at(crop(0, 0, 1600, 2160, [9, 16]), 1080).size, [1080, 1458]);
  const t = at(crop(1312, 0, 1216, 2160, [16, 9]), 720, 1);
  assert.deepEqual([t.turn, t.scale, t.size], [1, [1280, 720], [1280, 720]]);
});

test("exact size: contain, cover, stretch", () => {
  const v = video(1920, 1080);
  const at = (c: Crop | null, turn: number, width: number, height: number, fit: "contain" | "cover" | "stretch") =>
    plan(v, { crop: c, turn, size: { kind: "exact", width, height, fit } })!;
  let p = at(null, 0, 1080, 1080, "contain");
  assert.deepEqual([p.crop, p.scale, p.size, p.pad], [null, [1080, 608], [1080, 1080], { width: 1080, height: 1080, x: 0, y: 236 }]);
  p = at(null, 0, 1280, 720, "contain");
  assert.deepEqual([p.scale, p.pad], [[1280, 720], null]);
  // 맞춤 that only adds bars: a 608 × 1080 box in a square needs no scaling.
  p = at(crop(656, 0, 608, 1080), 0, 1080, 1080, "contain");
  assert.deepEqual([p.scale, p.pad, p.size], [null, { width: 1080, height: 1080, x: 236, y: 0 }, [1080, 1080]]);
  assert.deepEqual(pictureSize(p, [1920, 1080]), [608, 1080]);
  p = at(null, 0, 720, 720, "cover");
  assert.deepEqual([p.crop, p.scale, p.pad, p.size], [{ x: 420, y: 0, width: 1080, height: 1080 }, [720, 720], null, [720, 720]]);
  p = at(crop(0, 0, 1920, 1080), 1, 1080, 1350, "cover");
  assert.deepEqual([p.crop, p.scale, p.size], [{ x: 284, y: 0, width: 1350, height: 1080 }, null, [1080, 1350]]);
  p = at(null, 0, 1080, 1080, "stretch");
  assert.deepEqual([p.crop, p.scale, p.pad, p.size], [null, [1080, 1080], null, [1080, 1080]]);
  assert.deepEqual(at(crop(0, 0, 640, 360), 0, 1921, 1081, "stretch").size, [1920, 1080]);
  assert.equal(planEffects(v, fx({ size: { kind: "exact", width: 32, height: 720, fit: "contain" } })).error?.code, "outputSizeInvalid");
  assert.equal(planEffects(v, fx({ size: { kind: "short", short: 0 } })).error?.code, "outputSizeInvalid");
  assert.deepEqual(planEffects(video(0, 0), fx({ flipH: true })).error, { code: "pictureSizeUnknown" });
});

// --- source ⇄ seen ---

const R: Rect = { x: 100, y: 40, width: 300, height: 200 };
const SRC: [number, number] = [1920, 1080];

test("toSeen turns and mirrors a rectangle; fromSeen undoes it", () => {
  // A quarter turn right: the left edge goes to the top; x is measured from the bottom edge.
  assert.deepEqual(toSeen(R, SRC, { turn: 1, flipH: false, flipV: false }), { x: 1080 - 40 - 200, y: 100, width: 200, height: 300 });
  assert.deepEqual(toSeen(R, SRC, { turn: 2, flipH: false, flipV: false }), { x: 1920 - 100 - 300, y: 1080 - 40 - 200, width: 300, height: 200 });
  assert.deepEqual(toSeen(R, SRC, { turn: 3, flipH: false, flipV: false }), { x: 40, y: 1920 - 100 - 300, width: 200, height: 300 });
  assert.deepEqual(toSeen(R, SRC, { turn: 0, flipH: true, flipV: false }), { x: 1920 - 100 - 300, y: 40, width: 300, height: 200 });
  // A half turn is both flips.
  assert.deepEqual(toSeen(R, SRC, { turn: 2, flipH: false, flipV: false }), toSeen(R, SRC, { turn: 0, flipH: true, flipV: true }));
  for (let turn = 0; turn < 4; turn++) {
    for (const flipH of [false, true]) {
      for (const flipV of [false, true]) {
        const o: Orientation = { turn, flipH, flipV };
        assert.deepEqual(fromSeen(toSeen(R, SRC, o), SRC, o), R, JSON.stringify(o));
      }
    }
  }
});

test("evenRect keeps whole even pixels inside the picture", () => {
  assert.deepEqual(evenRect({ x: 101.4, y: -3, width: 641.2, height: 2000 }, SRC), { x: 100, y: 0, width: 640, height: 1080 });
  assert.deepEqual(evenRect({ x: 1800, y: 0, width: 300, height: 100 }, [1919, 1079]), { x: 1618, y: 0, width: 300, height: 100 });
});

test("ratioBox: the largest box of the ratio as seen, in the middle", () => {
  const none: Orientation = { turn: 0, flipH: false, flipV: false };
  assert.deepEqual(ratioBox([3840, 2160], none, [9, 16]), { x: 1312, y: 0, width: 1216, height: 2160 });
  assert.deepEqual(ratioBox([3840, 2160], none, [16, 9]), { x: 0, y: 0, width: 3840, height: 2160 });
  assert.deepEqual(ratioBox([3840, 2160], none, [1, 1]), { x: 840, y: 0, width: 2160, height: 2160 });
  assert.deepEqual(ratioBox([3840, 2160], none, [4, 3]), { x: 480, y: 0, width: 2880, height: 2160 });
  // Turned a quarter, the picture is seen 2160 × 3840: a 16:9 box seen is 2160 × 1216 — 1216 × 2160 in the source.
  assert.deepEqual(ratioBox([3840, 2160], { turn: 1, flipH: false, flipV: false }, [16, 9]), { x: 1312, y: 0, width: 1216, height: 2160 });
  assert.deepEqual(defaultBox([1920, 1080]), { x: 192, y: 108, width: 1536, height: 864 });
});

// --- drags and typed numbers (seen pixels) ---

const FRAME: [number, number] = [1920, 1080];
const BOX: Rect = { x: 400, y: 200, width: 800, height: 450 };

test("dragging the box moves it, kept inside", () => {
  assert.deepEqual(dragBox(BOX, "move", 100, -50, FRAME, null), { ...BOX, x: 500, y: 150 });
  assert.deepEqual(dragBox(BOX, "move", 5000, 5000, FRAME, null), { ...BOX, x: 1120, y: 630 });
});

test("free edges and corners move only their sides, at least the minimum", () => {
  assert.deepEqual(dragBox(BOX, "e", 120, 999, FRAME, null), { x: 400, y: 200, width: 920, height: 450 });
  assert.deepEqual(dragBox(BOX, "nw", -100, -100, FRAME, null), { x: 300, y: 100, width: 900, height: 550 });
  assert.deepEqual(dragBox(BOX, "sw", 2000, 0, FRAME, null), { x: 1136, y: 200, width: 64, height: 450 });
  assert.deepEqual(dragBox(BOX, "se", 5000, 5000, FRAME, null), { x: 400, y: 200, width: 1520, height: 880 });
});

test("with a ratio a corner keeps the opposite corner and the ratio", () => {
  const r: [number, number] = [16, 9];
  const se = dragBox(BOX, "se", 160, 0, FRAME, r);
  assert.deepEqual(se, { x: 400, y: 200, width: 960, height: 540 });
  const nw = dragBox(BOX, "nw", 0, -90, FRAME, r);
  assert.deepEqual(nw, { x: 240, y: 110, width: 960, height: 540 });
  // Out of room: stops at the picture's edge with the ratio.
  const big = dragBox(BOX, "se", 5000, 5000, FRAME, r);
  assert.equal(big.x + big.width, 1920);
  assert.ok(Math.abs(big.width / big.height - 16 / 9) < 1e-9);
  // An edge keeps the box centered across it.
  const e = dragBox(BOX, "e", 160, 0, FRAME, r);
  assert.deepEqual(e, { x: 400, y: 425 - 270, width: 960, height: 540 });
  const n = dragBox(BOX, "n", 0, 2000, FRAME, r);
  assert.ok(n.height >= 64 && n.width >= 64 && Math.abs(n.width / n.height - 16 / 9) < 1e-9);
});

test("typed numbers: position kept inside, a side follows a locked ratio and both fit", () => {
  assert.deepEqual(typedBox(BOX, "x", 1500, FRAME, null), { ...BOX, x: 1120 });
  assert.deepEqual(typedBox(BOX, "width", 1000, FRAME, null), { ...BOX, width: 1000 });
  assert.deepEqual(typedBox(BOX, "width", 1600, FRAME, [16, 9]), { x: 320, y: 180, width: 1600, height: 900 });
  const huge = typedBox(BOX, "height", 3000, FRAME, [1, 1]);
  assert.deepEqual(huge, { x: 400, y: 0, width: 1080, height: 1080 });
  assert.deepEqual(typedBox(BOX, "height", 10, FRAME, null), { ...BOX, height: 64 });
  assert.equal(typedBox(BOX, "x", Number.NaN, FRAME, null), BOX);
});

// --- the panel's choices ---

test("presets the box is too small for are not offered and fall back to 원본", () => {
  const v = video(1920, 1080);
  const small = fx({ crop: crop(0, 0, 640, 360), size: { kind: "short", short: 720 } });
  assert.equal(presetPossible([1920, 1080], small, 720), false);
  assert.equal(presetPossible([1920, 1080], small, 480), false);
  assert.equal(presetPossible([1920, 1080], fx({ crop: crop(0, 0, 1280, 720) }), 480), true);
  assert.deepEqual(settled(v, small).size, { kind: "source" });
  const fine = fx({ size: { kind: "short", short: 720 } });
  assert.equal(settled(v, fine), fine);
});

test("a quarter turn flips a locked ratio", () => {
  const e = turned(fx({ crop: crop(1312, 0, 1216, 2160, [9, 16]) }), 1);
  assert.deepEqual([e.turn, e.crop?.ratio], [1, [16, 9]]);
  assert.equal(turned(fx({ turn: 0 }), -1).turn, 3);
  assert.equal(turned(fx({ crop: crop(0, 0, 100, 100) }), 1).crop?.ratio, null);
});

// --- 모자이크 · 블러 (effects.rs mask tests, same numbers) ---

const mask = (x: number, y: number, width: number, height: number, start: number, end: number, kind: "mosaic" | "blur", strength: number): Mask => ({
  x, y, width, height, start, end, kind, strength,
});

test("strength scales with the picture", () => {
  assert.deepEqual([mosaicCell(1, 1080), mosaicCell(50, 1080), mosaicCell(100, 1080)], [5, 35, 65]);
  assert.deepEqual([mosaicCell(50, 2160), mosaicCell(50, 240)], [70, 8]);
  assert.equal(mosaicCell(0, 1080), 5);
  assert.equal(mosaicCell(1, 100), 2);
  assert.deepEqual([blurSigma(1, 1080), blurSigma(50, 1080), blurSigma(100, 1080)], [2, 17, 32]);
  assert.equal(blurSigma(1, 240), 1);
});

test("masks plan even boxes and cells; a phone video uses its upright picture", () => {
  const v = video(1920, 1080);
  const p = plan(v, { masks: [mask(101, 51, 351, 211, 1.5, 4, "mosaic", 50), mask(0, 0, 64, 64, 0, 10, "blur", 100)] })!;
  assert.deepEqual([p.crop, p.size], [null, [1920, 1080]]);
  assert.deepEqual(p.masks[0], { rect: { x: 100, y: 50, width: 350, height: 210 }, filter: { kind: "mosaic", cells: [10, 6] } });
  assert.deepEqual(p.masks[1].filter, { kind: "blur", sigma: 32 });
  const phone = plan(video(1920, 1080, 90), { masks: [mask(0, 1700, 1080, 200, 0, 1, "mosaic", 50)] })!;
  assert.deepEqual(phone.masks[0].filter, { kind: "mosaic", cells: [31, 6] });
});

test("bad masks are refused like Rust does", () => {
  const v = video(1920, 1080);
  const err = (m: Mask) => planEffects(v, fx({ masks: [m] })).error;
  assert.deepEqual(err(mask(1900, 0, 64, 64, 0, 1, "blur", 50)), { code: "maskOutsidePicture", width: 1920, height: 1080 });
  assert.deepEqual(err(mask(0, 0, 14, 64, 0, 1, "blur", 50)), { code: "maskTooSmall", min: 16 });
  assert.deepEqual(err(mask(0, 0, 64, 64, 2, 2, "blur", 50)), { code: "maskTimeInvalid" });
  const many = Array.from({ length: 21 }, () => mask(0, 0, 64, 64, 0, 1, "blur", 50));
  assert.deepEqual(planEffects(v, fx({ masks: many })).error, { code: "tooManyMasks", max: 20 });
  assert.equal(isNone(fx({ masks: [mask(0, 0, 64, 64, 0, 1, "blur", 50)] })), false);
});

test("a new box sits in the middle for the whole video; it shows from its start up to its end", () => {
  assert.deepEqual(newMask([1920, 1080], 12.5), { x: 824, y: 404, width: 270, height: 270, start: 0, end: 12.5, kind: "mosaic", strength: 50 });
  assert.equal(newMask([40, 30], 1).width, 16, "at least the smallest box");
  const m = mask(0, 0, 64, 64, 1, 2, "blur", 50);
  assert.deepEqual([maskShows(m, 0.99), maskShows(m, 1), maskShows(m, 1.999), maskShows(m, 2)], [false, true, true, false]);
});

// --- PiP (effects.rs overlay tests, same numbers) ---

const overlay = (x: number, y: number, width: number, height: number, start: number, end: number, offset: number): Overlay => ({
  path: "/v/카메라 1.mp4",
  x,
  y,
  width,
  height,
  start,
  end,
  offset,
  opacity: 100,
  audio: false,
  volume: 100,
});
const source = (kind: "video" | "image", width: number, height: number, duration: number | null): OverlaySource => ({
  path: kind === "video" ? "/v/카메라 1.mp4" : "/로고/채널 로고.png",
  fileName: kind === "video" ? "카메라 1.mp4" : "채널 로고.png",
  kind,
  width,
  height,
  duration,
  hasAudio: kind === "video",
});

test("PiP boxes plan even and bad ones are refused like Rust does", () => {
  const v = video(1920, 1080);
  assert.deepEqual(plan(v, { overlays: [overlay(1281, 721, 577, 325, 2, 9.5, 12)] })!.overlays, [{ rect: { x: 1280, y: 720, width: 576, height: 324 } }]);
  const err = (o: Overlay) => planEffects(v, fx({ overlays: [o] })).error;
  assert.deepEqual(err(overlay(1600, 0, 400, 200, 0, 1, 0)), { code: "overlayOutsidePicture", width: 1920, height: 1080 });
  assert.deepEqual(err(overlay(0, 0, 14, 200, 0, 1, 0)), { code: "overlayTooSmall", min: 16 });
  assert.deepEqual(err(overlay(0, 0, 64, 64, 3, 2, 0)), { code: "overlayTimeInvalid" });
  assert.deepEqual(err(overlay(0, 0, 64, 64, 0, 2, -1)), { code: "overlayTimeInvalid" });
  const many = Array.from({ length: 5 }, () => overlay(0, 0, 64, 64, 0, 1, 0));
  assert.deepEqual(planEffects(v, fx({ overlays: many })).error, { code: "tooManyOverlays", max: 4 });
  assert.equal(isNone(fx({ overlays: [overlay(0, 0, 64, 64, 0, 1, 0)] })), false);
});

const UP: Orientation = { turn: 0, flipH: false, flipV: false };
const RIGHT: Orientation = { turn: 1, flipH: false, flipV: false };

test("the five positions sit 3 % of the width from the edges, on even pixels — of the screen", () => {
  const box = { x: 0, y: 0, width: 576, height: 324 };
  const at = (c: 0 | 1 | 2 | 3 | 4) => {
    const r = cornerBox([1920, 1080], UP, box, c);
    return [r.x, r.y];
  };
  assert.deepEqual([at(0), at(1), at(2), at(3), at(4)], [[56, 56], [1288, 56], [672, 378], [56, 700], [1288, 700]]);
  assert.deepEqual(cornerBox([1920, 1080], UP, box, 4), { x: 1288, y: 700, width: 576, height: 324 });
  // Turned right the screen is 1080 × 1920: 오른쪽 아래 as seen is the original's top right.
  const turned = cornerBox([1920, 1080], RIGHT, { x: 0, y: 0, width: 324, height: 576 }, 4);
  assert.deepEqual(turned, { x: 1564, y: 32, width: 324, height: 576 });
  assert.deepEqual(toSeen(turned, [1920, 1080], RIGHT), { x: 472, y: 1564, width: 576, height: 324 });
});

test("a new PiP: 30 % of the width, its own ratio, bottom right; a video from the playhead for its length", () => {
  const src: [number, number] = [1920, 1080];
  assert.deepEqual(newOverlay(src, UP, source("video", 1280, 720, 5), 3, 20), { ...overlay(1288, 700, 576, 324, 3, 8, 0) });
  // Past the end of the source the span stops at the end; at the very end it still has a moment.
  assert.deepEqual([newOverlay(src, UP, source("video", 1280, 720, 30), 3, 20).end, newOverlay(src, UP, source("video", 1280, 720, 5), 20, 20).start], [20, 19.999]);
  // An image: no bigger than itself, the whole video.
  const logo = newOverlay(src, UP, source("image", 200, 60, null), 7, 20);
  assert.deepEqual([logo.path, logo.x, logo.y, logo.width, logo.height, logo.start, logo.end], ["/로고/채널 로고.png", 1664, 964, 200, 60, 0, 20]);
  // A phone video on a low picture: as tall as fits, its ratio kept.
  const tall = newOverlay([640, 300], UP, source("video", 1080, 1920, 5), 0, 20);
  assert.deepEqual([tall.width, tall.height], [50, 90]);
  assert.equal(planEffects(video(640, 300), fx({ overlays: [tall] })).error, null);
  // On a picture turned right: 30 % of the seen width (1080), upright, bottom right as seen.
  const cam = newOverlay(src, RIGHT, source("video", 1280, 720, 5), 0, 20);
  assert.deepEqual([cam.x, cam.y, cam.width, cam.height], [1706, 32, 182, 324]);
  assert.deepEqual(toSeen(cam, src, RIGHT), { x: 724, y: 1706, width: 324, height: 182 });
});

test("turning or mirroring the picture keeps a PiP where it was on the screen", () => {
  const src: [number, number] = [1920, 1080];
  // At a preset: the same preset as seen, the same seen size.
  const logo = { x: 1664, y: 964, width: 200, height: 60 };
  const turned = overlayAfterTurn(logo, src, UP, RIGHT);
  assert.deepEqual(turned, { x: 1828, y: 32, width: 60, height: 200 });
  assert.deepEqual(toSeen(turned, src, RIGHT), { x: 848, y: 1828, width: 200, height: 60 });
  assert.deepEqual(overlayAfterTurn(turned, src, RIGHT, UP), logo, "and back");
  // Elsewhere: around the same spot. Mirrored, a box on the left of the screen stays on the left.
  const mirror: Orientation = { turn: 0, flipH: true, flipV: false };
  assert.deepEqual(overlayAfterTurn({ x: 100, y: 500, width: 120, height: 80 }, src, UP, mirror), { x: 1700, y: 500, width: 120, height: 80 });
  // Too wide for the turned screen: smaller, its ratio kept.
  const wide = overlayAfterTurn({ x: 0, y: 0, width: 1600, height: 200 }, src, UP, RIGHT);
  assert.deepEqual(toSeen(wide, src, RIGHT).width, 1080);
  assert.equal(sameOrientation(UP, { turn: 4, flipH: false, flipV: false }), true);
  assert.equal(sameOrientation(UP, mirror), false);
});

test("a PiP shows in its span and a video disappears when it runs out", () => {
  const cam = source("video", 1280, 720, 4);
  const o = overlay(0, 0, 64, 64, 1, 9, 2);
  // 2 s of the video left from its start: it runs out at 3 s.
  assert.deepEqual([0.999, 1, 2.998, 3, 8].map((t) => overlayShows(o, cam, t)), [false, true, true, false, false]);
  const logo = source("image", 64, 64, null);
  assert.deepEqual([0.999, 1, 8.999, 9].map((t) => overlayShows(o, logo, t)), [false, true, true, false]);
});
