// npm run test:ts — frame starts and frame steps the editor puts IN / OUT, handles and ← → on.

import assert from "node:assert/strict";
import { test } from "node:test";
import type { VideoStream } from "../../lib/ipc";
import { frameStart, stepFrame, typedFrameStart } from "./format.ts";
import { dragHandleTime } from "./trimHandles.ts";

const video = (fps: number | null, extra: Partial<VideoStream> = {}): VideoStream => ({
  index: 0,
  codec: "h264",
  profile: null,
  width: 1920,
  height: 1080,
  startTime: 0,
  hasBFrames: 2,
  fps,
  avgFps: fps,
  pixFmt: "yuv420p",
  colorPrimaries: null,
  colorTransfer: null,
  colorSpace: null,
  hdr: false,
  rotation: null,
  vfrSuspect: false,
  ...extra,
});

// 60 fps keeping 3 of every 5 frames (the integration test's variable-rate source): gaps 1/60, 1/60, 3/60.
const THINNED = Array.from({ length: 30 }, (_, n) => n)
  .filter((n) => n % 5 < 3)
  .map((n) => n / 60);
const VFR = video(36.2, { vfrSuspect: true, frameTimes: THINNED });

test("constant rate: frames on the k / fps grid", () => {
  const ntsc = video(30000 / 1001);
  const f62 = 62 / (30000 / 1001);
  assert.equal(frameStart(f62 + 0.01, ntsc), f62);
  assert.equal(frameStart(f62, ntsc), f62);
  assert.ok(Math.abs(stepFrame(f62, 1, ntsc) - 63 / (30000 / 1001)) < 1e-9);
  assert.ok(Math.abs(stepFrame(f62 + 0.02, -2, ntsc) - 60 / (30000 / 1001)) < 1e-9);
  // Typed "00:02.069" (frame 62 rounded to the millisecond) names frame 62.
  assert.equal(typedFrameStart(2.069, ntsc), f62);
});

test("variable rate: the stream's own frame times", () => {
  // 0.09 s sits inside the frame at 5/60 (0.0833…), shown until 6/60.
  assert.equal(frameStart(0.09, VFR), 5 / 60);
  assert.equal(frameStart(5 / 60, VFR), 5 / 60, "on a frame start");
  assert.equal(frameStart(5 / 60 + 0.001, VFR), 5 / 60, "the player's seek a hair past it");
  assert.equal(frameStart(0.04, VFR), 2 / 60, "inside the 3/60 gap: the frame before it");
  assert.equal(frameStart(-1, VFR), 0);
  assert.equal(frameStart(99, VFR), THINNED[THINNED.length - 1]);
  // Steps walk the frames, not 1/36.2 s, and stop at the ends.
  assert.equal(stepFrame(2 / 60, 1, VFR), 5 / 60);
  assert.equal(stepFrame(5 / 60, -1, VFR), 2 / 60);
  assert.equal(stepFrame(0, -3, VFR), 0);
  assert.equal(stepFrame(99, 1, VFR), THINNED[THINNED.length - 1]);
  assert.equal(typedFrameStart(0.083, VFR), 5 / 60, "typed ms value of 5/60");
});

test("variable rate without frame times keeps the player's time", () => {
  const phone = video(30, { vfrSuspect: true });
  assert.equal(frameStart(1.2345, phone), 1.2345);
  assert.equal(typedFrameStart(1.2345, phone), 1.2345);
  assert.ok(Math.abs(stepFrame(1.2345, 1, phone) - (1.2345 + 1 / 30)) < 1e-9);
});

test("handles keep one real frame apart on a variable rate", () => {
  const range = { start: 5 / 60, end: 12 / 60 };
  // OUT dragged onto IN stops at the next frame (6/60), not IN + 1/36.2.
  assert.equal(dragHandleTime("end", 0, range, 0.5, VFR), 6 / 60);
  // IN dragged onto OUT stops at the frame before it (11/60).
  assert.equal(dragHandleTime("start", 1, range, 0.5, VFR), 11 / 60);
  // Anywhere else: the frame shown there.
  assert.equal(dragHandleTime("start", 0.04, range, 0.5, VFR), 2 / 60);
  assert.equal(dragHandleTime("end", 0.3, range, 0.5, VFR), 17 / 60, "18/60 was dropped");
});
