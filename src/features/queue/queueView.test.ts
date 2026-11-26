// npm run test:ts — the queue screen's grouping and wording.
import assert from "node:assert/strict";
import { test } from "node:test";
import type { ConvertTask, ExportSummary, Msg, QueueItem, QueueSnapshot } from "../../lib/ipc";
import { en } from "../../lib/i18n/en.ts";
import { ko } from "../../lib/i18n/ko.ts";
import {
  activeCount,
  finishedLine,
  formatElapsed,
  groupItems,
  itemEstimate,
  itemTags,
  jobsAhead,
  logText,
  newer,
  queueSummary,
  remainingSeconds,
  runningStats,
  waitingLine,
  withProgress,
} from "./queueView.ts";

const COPY: ExportSummary = {
  visibleStart: 2,
  exactStart: true,
  length: 17.898,
  codec: null,
  encoder: null,
  fallback: null,
  tenBit: false,
  audio: "copy",
  pieces: 3,
  keptRemovals: [],
};

const HEVC: ExportSummary = { ...COPY, length: 600, codec: "hevc", encoder: "VideoToolbox", fallback: "libx265", pieces: 1, audio: "aac" };

function item(id: number, state: QueueItem["state"], patch: Partial<QueueItem> = {}): QueueItem {
  return {
    id,
    info: { kind: "export", source: "/영상/제주 여행.mov", sourceName: "제주 여행.mov", edit: "trim", mode: "precise", codec: "hevc", quality: "balanced", summary: HEVC },
    state,
    canceling: false,
    output: "/영상/제주 여행_trim_h265.mp4",
    outputBytes: null,
    progress: null,
    note: null,
    error: null,
    commandLine: "/opt/homebrew/bin/ffmpeg -i a.mov b.mp4",
    log: null,
    startedAt: null,
    elapsedMs: null,
    finishedOrder: null,
    ...patch,
  };
}

const progress = (percent: number, outTime: number, speed: number | null) => ({ id: 9, percent, outTime, speed, fps: 93, note: null });

test("groups by state: running, waiting in queue order, finished last first", () => {
  const items = [
    item(1, "done", { finishedOrder: 1 }),
    item(2, "running"),
    item(3, "waiting"),
    item(4, "failed", { finishedOrder: 3 }),
    item(5, "waiting"),
    item(6, "canceled", { finishedOrder: 2 }),
  ];
  const g = groupItems(items);
  assert.deepEqual(g.running.map((i) => i.id), [2]);
  assert.deepEqual(g.waiting.map((i) => i.id), [3, 5]);
  assert.deepEqual(g.finished.map((i) => i.id), [4, 6, 1]);
  assert.equal(activeCount(items), 3);
  assert.equal(queueSummary(ko, items), "진행 중 1 · 대기 2 · 완료 1 · 실패 1 · 취소 1");
  assert.equal(queueSummary(ko, []), "진행 중 0 · 대기 0 · 완료 0 · 실패 0");
  assert.equal(jobsAhead(items, 3), 1);
  assert.equal(jobsAhead(items, 5), 2);
  assert.equal(jobsAhead(items, 1), 0);
});

test("progress applies to the running item only, and the newer snapshot wins", () => {
  const snap: QueueSnapshot = { version: 4, concurrency: 1, items: [item(1, "running"), item(2, "waiting")] };
  const note: Msg = { code: "retriedWith", failed: "VideoToolbox", next: "libx265", reason: null };
  const p = { ...progress(62, 372, 3.1), note };
  const next = withProgress(snap, { id: 1, progress: p })!;
  assert.equal(next.items[0].progress?.percent, 62);
  assert.equal(next.items[0].note, p.note);
  assert.equal(next.version, 4);
  assert.equal(withProgress(snap, { id: 2, progress: p }), snap, "a waiting item gets no progress");
  assert.equal(withProgress(null, { id: 1, progress: p }), null);
  const older = { ...snap, version: 3 };
  assert.equal(newer(snap, older), snap);
  assert.equal(newer(older, snap), snap);
  assert.equal(newer(null, snap), snap);
});

test("tags and lines say what each item makes", () => {
  const running = item(1, "running");
  assert.deepEqual(itemTags(ko, running), ["자르기 · 정밀", "H.265", "VideoToolbox", "균형", "MP4 · hvc1"]);
  const cut = item(2, "waiting", {
    info: { ...running.info, edit: "cut", mode: "lossless", codec: "source", summary: COPY },
    output: "/영상/제주 여행_cut.mov",
  });
  assert.deepEqual(itemTags(ko, cut), ["구간 제거 · 무손실 · 유지 구간 3개", "스트림 복사", "MOV"]);
  assert.equal(waitingLine(ko, cut), "구간 제거 · 무손실 · 00:17.898");
  assert.equal(waitingLine(ko, running), "자르기 · H.265 · VideoToolbox · 균형 · 10:00.000");
  const av1 = item(3, "waiting", { info: { ...running.info, codec: "av1", quality: "small", summary: { ...HEVC, codec: "av1", encoder: "SVT-AV1", tenBit: true } }, output: "/a/b_av1.mkv" });
  assert.deepEqual(itemTags(ko, av1), ["자르기 · 정밀", "AV1", "SVT-AV1", "작은 용량", "10비트", "MKV"]);
});

const CONVERT: ConvertTask = {
  source: "/영상/아이폰 가족.MOV",
  sourceName: "아이폰 가족.MOV",
  quality: "high",
  summary: {
    length: 165,
    codec: "hevc",
    encoder: "VideoToolbox",
    fallback: "libx265",
    tenBit: true,
    hdr: true,
    scaled: [1080, 1920],
    audio: { kind: "copy" },
    container: "mov",
  },
  estimatedBytes: null,
};

test("a 일괄 변환 item says what it converts to", () => {
  const c = item(7, "running", { info: { kind: "convert", ...CONVERT }, output: "/변환 결과/아이폰 가족_h265.mov" });
  assert.deepEqual(itemTags(ko, c), ["일괄 변환", "H.265", "VideoToolbox", "고화질", "1080×1920로 축소", "10비트 · HDR", "MOV · hvc1", "오디오 복사"]);
  assert.equal(waitingLine(ko, c), "일괄 변환 · H.265 · VideoToolbox · 고화질 · 02:45.000");
  assert.equal(remainingSeconds({ ...c, progress: progress(50, 65, 2) }), 50);
  const webm = { kind: "convert" as const, ...CONVERT, quality: "small" as const, summary: { ...CONVERT.summary, codec: "av1" as const, encoder: "SVT-AV1", tenBit: false, hdr: false, scaled: null, audio: { kind: "opus" as const, bitrate: 128 }, container: "webm" as const } };
  assert.deepEqual(itemTags(ko, item(8, "waiting", { info: webm, output: "/a/b_av1.webm" })), ["일괄 변환", "AV1", "SVT-AV1", "작은 용량", "WEBM", "Opus 128k"]);
  assert.equal(finishedLine(ko, item(9, "done", { info: webm, elapsedMs: 64_000, outputBytes: 120_000_000 })), "일괄 변환 · AV1 · 작은 용량 · 1분 4초 · 120MB");

  // The 예상 크기 measured on the convert screen goes along (as the mockup: "약 1.5GB" · "예상 1.6GB").
  const sized = item(10, "waiting", { info: { kind: "convert", ...CONVERT, estimatedBytes: 1_500_000_000 } });
  assert.equal(waitingLine(ko, sized), "일괄 변환 · H.265 · VideoToolbox · 고화질 · 02:45.000 · 약 1.5GB");
  assert.equal(itemEstimate(ko, sized), "예상 1.5GB");
  assert.equal(itemEstimate(ko, c), null, "queued before its estimate was ready");
  assert.equal(itemEstimate(ko, item(11, "running")), null, "editor exports have none");
});

test("finished lines: size and time when done, why when failed or canceled", () => {
  assert.equal(
    finishedLine(ko, item(1, "done", { info: { ...item(1, "done").info, edit: "cut", mode: "lossless", summary: COPY }, elapsedMs: 6000, outputBytes: 3_300_000_000 })),
    "구간 제거 · 무손실 · 6.0초 · 3.3GB",
  );
  assert.equal(finishedLine(ko, item(1, "done", { elapsedMs: 18 * 60_000, outputBytes: 2_200_000_000 })), "자르기 · H.265 · 균형 · 18분 · 2.2GB");
  assert.equal(
    finishedLine(ko, item(2, "failed", { error: { code: "raw", text: "moov atom not found." } })),
    "실패 · moov atom not found. 출력 파일은 만들지 않았습니다.",
  );
  assert.equal(finishedLine(ko, item(3, "canceled", { startedAt: 1 })), "취소 · 만들던 파일은 지웠고, 원본은 그대로입니다.");
  assert.equal(finishedLine(ko, item(4, "canceled")), "취소 · 시작하기 전에 취소했습니다.");
  assert.equal(formatElapsed(ko, 400), "0.4초");
  assert.equal(formatElapsed(ko, 42_400), "42초");
  assert.equal(formatElapsed(ko, 64_000), "1분 4초");
  assert.equal(formatElapsed(ko, 3_960_000), "1시간 6분");
  assert.equal(formatElapsed(ko, 7_200_000), "2시간");
});

test("running stats and the log view", () => {
  const started = 1_000_000;
  const r = item(1, "running", { startedAt: started, progress: progress(62.4, 372, 3.1) });
  assert.equal(remainingSeconds(r), (600 - 372) / 3.1);
  assert.deepEqual(runningStats(ko, r, started + 120_000), ["62%", "경과 2:00", "남은 시간 약 1:14", "속도 3.1×", "93 fps"]);
  assert.deepEqual(runningStats(ko, item(2, "running", { startedAt: started }), started), ["0%", "경과 0:00"]);
  assert.equal(remainingSeconds(item(3, "waiting", { progress: progress(1, 1, 2) })), null);
  assert.equal(
    logText(ko, r),
    "$ /opt/homebrew/bin/ffmpeg -i a.mov b.mp4\nout_time=06:12.000\nfps=93.0\nspeed=3.10x\nprogress=continue",
  );
  const note: Msg = { code: "retriedWith", failed: "NVENC", next: "libx265", reason: "Error while opening encoder" };
  const failed = item(4, "failed", { commandLine: "ffmpeg piece\nffmpeg join", log: "[in#0 @ 0x1] moov atom not found", note });
  assert.equal(
    logText(ko, failed),
    "$ ffmpeg piece\n$ ffmpeg join\n# NVENC 인코딩이 실패해 libx265(으)로 다시 시작했습니다: Error while opening encoder\n[in#0 @ 0x1] moov atom not found",
  );
  assert.equal(
    logText(en, failed),
    "$ ffmpeg piece\n$ ffmpeg join\n# NVENC encoding failed, so it started again with libx265: Error while opening encoder\n[in#0 @ 0x1] moov atom not found",
  );
  assert.equal(logText(ko, item(5, "done")), "$ /opt/homebrew/bin/ffmpeg -i a.mov b.mp4\n# FFmpeg가 남긴 오류 · 경고 없음");
});

test("English: counts, tags and times", () => {
  const items = [item(1, "running"), item(2, "waiting"), item(3, "failed"), item(4, "canceled")];
  assert.equal(queueSummary(en, items), "1 running · 1 waiting · 0 done · 1 failed · 1 canceled");
  assert.deepEqual(itemTags(en, item(5, "running")), ["Trim · Precise", "H.265", "VideoToolbox", "Balanced", "MP4 · hvc1"]);
  const cut = item(6, "waiting", { info: { ...item(6, "waiting").info, edit: "cut", mode: "lossless", summary: COPY }, output: "/a/b_cut.mov" });
  assert.deepEqual(itemTags(en, cut), ["Cut out · Lossless · 3 kept parts", "Stream copy", "MOV"]);
  assert.equal(
    finishedLine(en, item(7, "done", { elapsedMs: 64_000, outputBytes: 120_000_000 })),
    "Trim · H.265 · Balanced · 1m 4s · 120MB",
  );
  assert.equal(finishedLine(en, item(8, "failed", { error: { code: "ffmpegFailed" } })), "Failed · FFmpeg ended with an error. No output file was made.");
  assert.equal(finishedLine(en, item(9, "canceled")), "Canceled · Canceled before it started.");
  assert.equal(formatElapsed(en, 400), "0.4s");
  assert.equal(formatElapsed(en, 3_960_000), "1h 6m");
  const started = 1_000_000;
  const r = item(10, "running", { startedAt: started, progress: progress(62.4, 372, 3.1) });
  assert.deepEqual(runningStats(en, r, started + 120_000), ["62%", "2:00 elapsed", "about 1:14 left", "3.1× speed", "93 fps"]);
});
