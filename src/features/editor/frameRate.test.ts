// npm run test:ts — 프레임레이트 · GIF 크기 (frameRate.ts, the same cases as command::plan_frame_rate ·
// plan_gif_scale) and what the export panel names and sends for a GIF (exportOptions.ts).

import assert from "node:assert/strict";
import { test } from "node:test";
import type { MediaInfo, QueueItem, VideoStream } from "../../lib/ipc";
import { en } from "../../lib/i18n/en.ts";
import { ko } from "../../lib/i18n/ko.ts";
import { itemTags, waitingLine } from "../queue/queueView.ts";
import { DEFAULT_CONVERT, planNotes } from "../convert/convertView.ts";
import { DEFAULT_EXPORT, encoderFor, encoderLine, predictedExportName, requestOptions, saveNote } from "./exportOptions.ts";
import { clampRate, gifScale, gifSizeButtons, rateButtons, rateNote, ratePossible, rateText, resultRate, sourceRate } from "./frameRate.ts";

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

// --- The export panel's rows (ExportSection's FrameRateGroup · GifSizeGroup · ExportFooter) ---------

/** "원본" · "24" · "직접" for the pressed button, "-" for an off preset: one string per row. */
function row(b: ReturnType<typeof rateButtons>): string {
  const presets = b.presets.map((p) => (p.disabled ? "-" : p.pressed ? `[${p.value}]` : `${p.value}`));
  return [b.sourcePressed ? "[원본]" : "원본", ...presets, b.customPressed ? "[직접]" : "직접"].join(" ");
}

test("프레임레이트 buttons: exactly one pressed, the presets that change nothing off", () => {
  const sixty = video(60);
  // A video: 원본 · 60 · 30 · 24 · 직접.
  assert.equal(row(rateButtons(sixty, null, false, false)), "[원본] - 30 24 직접", "60 adds nothing to 60 fps");
  assert.equal(row(rateButtons(sixty, 30, false, false)), "원본 - [30] 24 직접");
  assert.equal(row(rateButtons(sixty, 60, false, false)), "[원본] - 30 24 직접", "a rate that changes nothing shows as 원본");
  assert.equal(row(rateButtons(sixty, 45, false, false)), "원본 - 30 24 [직접]", "a typed rate no preset has");
  assert.equal(row(rateButtons(sixty, 30, false, true)), "원본 - 30 24 [직접]", "while typing, even a preset's number");
  assert.equal(row(rateButtons(video(30000 / 1001), null, false, false)), "[원본] - - 24 직접", "29.97: 30 changes nothing");
  assert.equal(row(rateButtons(video(120, 29.6), 24, false, false)), "원본 - - [24] 직접", "a phone video by its average");
  assert.equal(row(rateButtons(video(null), 24, false, false)), "원본 60 30 [24] 직접", "rate unknown: every preset");
  // A GIF: 원본 · 25 · 15 · 10 · 직접, at most 50.
  assert.equal(row(rateButtons(sixty, 15, true, false)), "원본 25 [15] 10 직접");
  assert.equal(row(rateButtons(video(12), 15, true, false)), "[원본] - - 10 직접", "the default 15 adds nothing to 12 fps");
  // Where 직접 starts: the result's rate, else the source's, else 30 — in range.
  assert.deepEqual(
    [rateButtons(sixty, null, false, false), rateButtons(sixty, 24, false, false), rateButtons(video(null), null, false, false), rateButtons(sixty, null, true, false), rateButtons(video(12), 15, true, false)].map((b) => b.start),
    [60, 24, 30, 50, 12],
  );
  // Never two pressed, never none, across sources, rates and typing.
  for (const v of [sixty, video(24), video(12), video(30000 / 1001), video(null)]) {
    for (const gif of [false, true]) {
      for (const rate of [null, 10, 15, 24, 25, 30, 45, 60]) {
        for (const typing of [false, true]) {
          const b = rateButtons(v, rate, gif, typing);
          const pressed = [b.sourcePressed, b.customPressed, ...b.presets.map((p) => p.pressed)].filter(Boolean).length;
          assert.equal(pressed, 1, `${v.avgFps} fps, ${gif ? "GIF" : "video"}, ${rate}, typing ${typing}: ${row(b)}`);
          assert.ok(b.presets.every((p) => !(p.pressed && p.disabled)), row(b));
        }
      }
    }
  }
});

test("프레임레이트 note: what the result gets, from what the source has", () => {
  const w = ko.exportPanel;
  assert.equal(rateNote(ko, video(60), null, false), w.rateKept("60"));
  assert.equal(rateNote(ko, video(60), 60, false), w.rateKept("60"), "a cap at the source's rate keeps it");
  assert.equal(rateNote(ko, video(60), 30, false), w.rateDown("30", "60"));
  assert.equal(rateNote(ko, video(null), 24, false), w.rateDown("24", null));
  assert.equal(rateNote(ko, video(60), null, true), w.rateGifMax("60"), "원본 above 50: the GIF limit");
  assert.equal(rateNote(ko, video(60), 15, true), w.rateGif("15", "60"));
  assert.equal(rateNote(ko, video(30000 / 1001), null, true), w.rateGif("29.97", "29.97"));
  assert.equal(rateNote(ko, video(12), 15, true), w.rateGif("12", "12"), "never more frames than the source");
  assert.equal(rateNote(ko, video(null), null, true), w.rateGif("15", null));
  assert.equal(rateNote(en, video(60), 30, false), en.exportPanel.rateDown("30", "60"));
});

test("GIF 크기 buttons: never larger than the picture after the 화면 효과", () => {
  const show = (b: ReturnType<typeof gifSizeButtons>) =>
    [b.sourcePressed ? "[원본]" : "원본", ...b.presets.map((p) => (p.disabled ? "-" : p.pressed ? `[${p.value}p]` : `${p.value}p`)), b.size.join("×")].join(" ");
  assert.equal(show(gifSizeButtons([1920, 1080], 360)), "원본 480p [360p] 240p 640×360");
  assert.equal(show(gifSizeButtons([1920, 1080], null)), "[원본] 480p 360p 240p 1920×1080");
  assert.equal(show(gifSizeButtons([640, 360], 360)), "[원본] - - 240p 640×360", "the default 360p on a 360p picture: as it is");
  assert.equal(show(gifSizeButtons([1080, 1920], 240)), "원본 480p 360p [240p] 240×426", "portrait: the width is the short side");
  assert.equal(show(gifSizeButtons([300, 200], 240)), "[원본] - - - 300×200");
  assert.equal(gifSizeButtons([1280, 720], 480).side, 720);
});

test("the export footer's line: 무손실 size, 정밀 changes, a GIF's own size and rate", () => {
  const w = ko.exportPanel;
  const info = media("폰 영상.mp4", video(60));
  const precise = { ...DEFAULT_EXPORT, mode: "precise" as const, codec: "hevc" as const };
  assert.equal(saveNote(ko, info, DEFAULT_EXPORT, null, [1920, 1080], "12 MB"), w.saveNoteLossless("12 MB"));
  assert.equal(saveNote(ko, info, precise, null, [1920, 1080], null), w.saveNotePrecise, "nothing changes");
  assert.equal(saveNote(ko, info, { ...precise, frameRate: 30 }, null, [1920, 1080], null), w.saveNotePreciseWith("30fps"));
  assert.equal(saveNote(ko, info, { ...precise, frameRate: 60 }, null, [1920, 1080], null), w.saveNotePrecise, "60 on 60 fps changes nothing");
  assert.equal(saveNote(ko, info, { ...precise, frameRate: 24 }, [1280, 720], [1280, 720], null), w.saveNotePreciseWith("1280 × 720 · 24fps"));
  // A GIF: the default 15 fps · 360p, from the picture after the 화면 효과 (a crop to 1080 × 1080 here).
  const gif = { ...DEFAULT_EXPORT, mode: "precise" as const, codec: "gif" as const };
  assert.equal(saveNote(ko, info, gif, null, [1920, 1080], null), w.saveNoteGif("640 × 360 · 15fps"));
  assert.equal(saveNote(ko, info, gif, [1080, 1080], [1080, 1080], null), w.saveNoteGif("360 × 360 · 15fps"));
  assert.equal(saveNote(ko, info, { ...gif, gifShort: null, gifRate: null }, null, [1920, 1080], null), w.saveNoteGif("1920 × 1080 · 50fps"));
  assert.equal(saveNote(en, info, gif, null, [1920, 1080], null), en.exportPanel.saveNoteGif("640 × 360 · 15 fps"));
});
