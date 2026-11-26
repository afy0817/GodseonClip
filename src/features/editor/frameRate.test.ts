// npm run test:ts — 프레임레이트 · GIF 크기 (frameRate.ts, the same cases as command::plan_frame_rate ·
// plan_gif_scale) and what the export panel names and sends for a GIF (exportOptions.ts).

import assert from "node:assert/strict";
import { test } from "node:test";
import type { MediaInfo, QueueItem, VideoStream } from "../../lib/ipc";
import { en } from "../../lib/i18n/en.ts";
import { ko } from "../../lib/i18n/ko.ts";
import { itemTags, waitingLine } from "../queue/queueView.ts";
import { DEFAULT_CONVERT, planNotes } from "../convert/convertView.ts";
import { DEFAULT_EXPORT, encoderFor, encoderLine, predictedExportName, requestOptions } from "./exportOptions.ts";
import { clampRate, gifScale, ratePossible, rateText, resultRate, sourceRate } from "./frameRate.ts";

function video(fps: number | null, avgFps: number | null = fps, patch: Partial<VideoStream> = {}): VideoStream {
  return {
    index: 0,
    codec: "h264",
    profile: null,
    width: 1920,
    height: 1080,
    startTime: 0,
    hasBFrames: 2,
    fps,
    avgFps,
    pixFmt: "yuv420p",
    colorPrimaries: null,
    colorTransfer: null,
    colorSpace: null,
    hdr: false,
    rotation: null,
    vfrSuspect: false,
    ...patch,
  };
}

function media(fileName: string, v: VideoStream): MediaInfo {
  return {
    path: `/영상/${fileName}`,
    fileName,
    sizeBytes: 1_000_000,
    duration: 10,
    formatName: "mov,mp4,m4a,3gp,3g2,mj2",
    bitRate: null,
    video: v,
    audio: [],
    subtitleCount: 0,
    otherCount: 0,
  } as MediaInfo;
}

test("a frame rate cap never adds frames (command::plan_frame_rate)", () => {
  const sixty = video(60);
  assert.equal(resultRate(sixty, null, false), null, "원본");
  assert.equal(resultRate(sixty, 30, false), 30);
  assert.equal(resultRate(sixty, 60, false), null);
  assert.equal(resultRate(sixty, 120, false), null);
  const ntsc = video(30000 / 1001);
  assert.equal(resultRate(ntsc, 30, false), null);
  assert.equal(resultRate(ntsc, 24, false), 24);
  const phone = video(120, 29.6);
  assert.equal(sourceRate(phone), 29.6, "the average, not the timebase guess");
  assert.equal(resultRate(phone, 60, false), null);
  assert.equal(resultRate(video(null), 30, false), 30);
  assert.equal(resultRate(video(25, null), 24, false), 24);
  // A GIF always gets a rate: the cap, else the source's up to 50.
  assert.equal(resultRate(sixty, 15, true), 15);
  assert.equal(resultRate(sixty, null, true), 50);
  assert.equal(resultRate(ntsc, null, true), 30000 / 1001);
  assert.equal(resultRate(video(12), 15, true), 12);
  assert.equal(resultRate(video(null), null, true), 15);
  // Buttons: only what brings the rate down.
  assert.deepEqual([60, 30, 24].map((n) => ratePossible(video(30), n, false)), [false, false, true]);
  assert.deepEqual([25, 15, 10].map((n) => ratePossible(video(24), n, true)), [false, true, true]);
  assert.equal(ratePossible(sixty, 50, true), false, "60 fps is 50 as a GIF already");
  assert.equal(ratePossible(video(null), 60, false), true);
});

test("rates as text, typed rates, GIF 크기", () => {
  assert.deepEqual([30, 30000 / 1001, 12.5, 23.976].map(rateText), ["30", "29.97", "12.5", "23.98"]);
  assert.deepEqual([0, 12.4, 200].map((n) => clampRate(n, false)), [1, 12, 120]);
  assert.equal(clampRate(60, true), 50);
  assert.deepEqual(gifScale([1920, 1080], 360), [640, 360]);
  assert.deepEqual(gifScale([1080, 1920], 240), [240, 426]);
  assert.equal(gifScale([640, 360], 480), null, "never larger");
  assert.equal(gifScale([1920, 1080], null), null);
});

test("a GIF export: its name, what is sent, its line", () => {
  const info = media("제주 여행.mov", video(30));
  const gif = { ...DEFAULT_EXPORT, mode: "precise" as const, codec: "gif" as const };
  assert.equal(predictedExportName(info, gif, false), "제주 여행_trim.gif");
  assert.equal(predictedExportName(info, gif, true), "제주 여행.gif", "the whole file: .gif tells it apart");
  assert.equal(predictedExportName(info, gif, false, "cut"), "제주 여행_cut.gif");
  assert.deepEqual(requestOptions(gif), { mode: "precise", codec: "gif", quality: "balanced", frameRate: 15, gifShort: 360 });
  // A video sends its own rate and no GIF 크기; 무손실 sends neither.
  const hevc = { ...gif, codec: "hevc" as const, frameRate: 30 };
  assert.deepEqual(requestOptions(hevc), { mode: "precise", codec: "hevc", quality: "balanced", frameRate: 30, gifShort: null });
  assert.deepEqual(requestOptions({ ...hevc, mode: "lossless", codec: "source" }), { mode: "lossless", codec: "source", quality: "balanced", frameRate: null, gifShort: null });
  // No encoder test needed: the line is there before the encoders are known.
  assert.equal(encoderFor(null, "gif")?.label, "GIF");
  assert.equal(encoderLine(ko, info, gif, null), ko.encoderLine.gif);
  const hdr = media("hdr.mov", video(30, 30, { hdr: true }));
  assert.equal(encoderLine(en, hdr, gif, null), `${en.encoderLine.gif} · ${en.encoderLine.gifHdr}`);
});

test("the queue names a GIF once and shows a changed frame rate", () => {
  const summary = {
    visibleStart: 2,
    exactStart: true,
    length: 3,
    codec: "gif" as const,
    encoder: "GIF",
    fallback: null,
    tenBit: false,
    audio: "none" as const,
    pieces: 1,
    keptRemovals: [],
    picture: [640, 360] as [number, number],
    frameRate: 15,
    masks: 0,
    overlays: 0,
  };
  const item = {
    id: 1,
    info: { kind: "export", source: "/영상/a.mp4", sourceName: "a.mp4", edit: "trim", mode: "precise", codec: "gif", quality: "balanced", summary },
    state: "running",
    output: "/영상/a_trim.gif",
  } as unknown as QueueItem;
  assert.deepEqual(itemTags(ko, item), ["자르기 · 정밀", "GIF", "균형", "화면 640×360", "15fps"]);
  assert.deepEqual(itemTags(en, item).slice(-1), ["15 fps"]);
  assert.equal(waitingLine(ko, item), "자르기 · GIF · 균형 · 00:03.000");
  // 일괄 변환 at 30 fps: the note under each 60 fps file, nothing under a 25 fps one.
  const choice = { ...DEFAULT_CONVERT, frameRate: 30 };
  assert.ok(planNotes(ko, media("폰.mp4", video(60)), choice).includes("30fps로"));
  assert.ok(!planNotes(ko, media("캠.mp4", video(25)), choice).some((n) => n.includes("fps")));
});
