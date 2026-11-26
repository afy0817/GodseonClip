// npm run test:ts — the notification center's groups, unseen marks and overall progress.
import assert from "node:assert/strict";
import { test } from "node:test";
import type { ExportSummary, QueueItem } from "../../lib/ipc";
import { lastFinishedOrder, notifyGroups, overallPercent, RECENT_LIMIT, unseenOf } from "./notifyView.ts";

const SUMMARY: ExportSummary = {
  visibleStart: 0,
  exactStart: true,
  length: 60,
  codec: "hevc",
  encoder: "VideoToolbox",
  fallback: null,
  tenBit: false,
  audio: "copy",
  pieces: 1,
  keptRemovals: [],
};

function item(id: number, state: QueueItem["state"], patch: Partial<QueueItem> = {}): QueueItem {
  return {
    id,
    info: { kind: "export", source: "/영상/테스트 클립 01.mp4", sourceName: "테스트 클립 01.mp4", edit: "trim", mode: "precise", codec: "hevc", quality: "balanced", summary: SUMMARY },
    state,
    canceling: false,
    output: `/영상/테스트 클립 01_h265 (${id}).mp4`,
    outputBytes: null,
    progress: null,
    note: null,
    error: null,
    commandLine: "ffmpeg",
    log: null,
    startedAt: null,
    elapsedMs: null,
    finishedOrder: null,
    ...patch,
  };
}

const progress = (percent: number) => ({ id: 1, percent, outTime: 1, speed: 2, fps: 60, note: null });

test("groups: running, waiting, the last finished first and at most RECENT_LIMIT", () => {
  const done = Array.from({ length: RECENT_LIMIT + 2 }, (_, k) => item(10 + k, "done", { finishedOrder: k + 1 }));
  const g = notifyGroups([item(1, "waiting"), item(2, "running"), ...done, item(3, "waiting")]);
  assert.deepEqual(g.running.map((i) => i.id), [2]);
  assert.deepEqual(g.waiting.map((i) => i.id), [1, 3]);
  assert.equal(g.recent.length, RECENT_LIMIT);
  assert.equal(g.recent[0].finishedOrder, RECENT_LIMIT + 2, "the last finished first");
});

test("unseen: finished after what was seen, orange when one failed", () => {
  const items = [item(1, "done", { finishedOrder: 1 }), item(2, "failed", { finishedOrder: 2 }), item(3, "canceled", { finishedOrder: 3 }), item(4, "running")];
  assert.deepEqual(unseenOf(items, 0), { count: 3, failed: true });
  assert.deepEqual(unseenOf(items, 2), { count: 1, failed: false });
  assert.deepEqual(unseenOf(items, lastFinishedOrder(items)), { count: 0, failed: false }, "opening the panel sees them all");
  assert.equal(lastFinishedOrder([]), 0);
  // The history cleared on the queue screen: nothing left to be unseen.
  assert.deepEqual(unseenOf([item(4, "running")], 3), { count: 0, failed: false });
});

test("overall progress of the running items", () => {
  assert.equal(overallPercent([item(1, "waiting")]), null);
  assert.equal(overallPercent([item(1, "running", { progress: progress(40) }), item(2, "running", { progress: progress(80) })]), 60);
  assert.equal(overallPercent([item(1, "running")]), 0, "started, no progress yet");
});
