// npm run test:ts — the 일괄 변환 screen's rules, the same cases as command.rs where they mirror it.
import assert from "node:assert/strict";
import { test } from "node:test";
import type { EncoderOptions, EstimateEntry, MediaInfo, Msg, QueueItem, VideoStream } from "../../lib/ipc";
import { en } from "../../lib/i18n/en.ts";
import { ko } from "../../lib/i18n/ko.ts";
import {
  addPaths,
  codecCell,
  type ConvertRow,
  DEFAULT_CONVERT,
  downscaledSize,
  encoderInUse,
  encoderNote,
  estimateCell,
  estimateLine,
  estimateSettingsKey,
  estimateTotals,
  savingText,
  listHeading,
  listTotals,
  nameOf,
  outputPattern,
  rowStatus,
  sizeCell,
  suffixProblem,
  toSettings,
  totalsLine,
  withCodec,
} from "./convertView.ts";

function video(width: number, height: number, patch: Partial<VideoStream> = {}): VideoStream {
  return {
    index: 0,
    codec: "h264",
    profile: null,
    width,
    height,
    startTime: 0,
    hasBFrames: 2,
    fps: 30,
    avgFps: 30,
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

function media(path: string, v: VideoStream | null, patch: Partial<MediaInfo> = {}): MediaInfo {
  return {
    path,
    fileName: nameOf(path),
    sizeBytes: 3_900_000_000,
    duration: 600,
    formatName: "mov,mp4,m4a,3gp,3g2,mj2",
    bitRate: null,
    video: v,
    audio: [],
    subtitleCount: 0,
    otherCount: 0,
    ...patch,
  };
}

function ready(path: string, info: MediaInfo, patch: Partial<ConvertRow> = {}): ConvertRow {
  return { path, name: nameOf(path), probe: { state: "ready", info }, checked: true, itemId: null, ...patch };
}

test("downscale follows command::downscale (short side, rotation, -2 rounding, never up)", () => {
  assert.deepEqual(downscaledSize(video(3840, 2160), "p1080"), [1920, 1080]);
  assert.equal(downscaledSize(video(1920, 1080), "p1080"), null);
  assert.equal(downscaledSize(video(1280, 720), "p1080"), null);
  assert.deepEqual(downscaledSize(video(1440, 1080), "p720"), [960, 720]);
  assert.deepEqual(downscaledSize(video(1918, 1080), "p720"), [1278, 720]);
  assert.deepEqual(downscaledSize(video(1080, 1920), "p720"), [720, 1280]);
  assert.deepEqual(downscaledSize(video(1920, 1080, { rotation: 90 }), "p720"), [720, 1280]);
  assert.deepEqual(downscaledSize(video(1920, 1080, { rotation: -90 }), "p720"), [720, 1280]);
  assert.deepEqual(downscaledSize(video(1920, 1080, { rotation: 180 }), "p720"), [1280, 720]);
  assert.equal(downscaledSize(video(3840, 2160), "source"), null);
});

test("cells: codec, size as shown, rate", () => {
  const hdr = media("/v/아이폰 가족.MOV", video(3840, 2160, { codec: "hevc", hdr: true, pixFmt: "yuv420p10le", rotation: -90, fps: 29.97002997 }));
  assert.equal(codecCell(ko, hdr), "H.265 · HDR");
  assert.equal(sizeCell(hdr), "2160×3840 · 29.97");
  assert.equal(codecCell(ko, media("/v/a.mkv", video(1920, 1080, { codec: "hevc", pixFmt: "yuv420p10le" }))), "H.265 · 10비트");
  assert.equal(codecCell(ko, media("/v/a.webm", video(2560, 1440, { codec: "vp9" }))), "VP9");
  assert.equal(sizeCell(media("/v/a.mp4", video(1920, 1080, { fps: 60 }))), "1920×1080 · 60");
});

test("settings: codec switch, ending, what is sent", () => {
  const av1 = withCodec({ ...DEFAULT_CONVERT, container: "mov", encoder: "libx265" }, "av1");
  assert.deepEqual([av1.codec, av1.container, av1.encoder], ["av1", "mp4", null], "no AV1 in MOV; encoders are per codec");
  assert.equal(withCodec({ ...DEFAULT_CONVERT, codec: "av1", container: "webm" }, "hevc").container, "mp4");
  assert.equal(withCodec({ ...DEFAULT_CONVERT, container: "mkv" }, "av1").container, "mkv");
  assert.equal(outputPattern(ko, DEFAULT_CONVERT), "{이름}_h265.mp4");
  assert.equal(outputPattern(ko, { ...av1, container: "webm" }), "{이름}_av1.webm");
  assert.equal(outputPattern(ko, { ...DEFAULT_CONVERT, suffix: "" }), "{이름}.mp4");
  assert.deepEqual(toSettings({ ...DEFAULT_CONVERT, suffix: " 작게", outputDir: "/변환 결과" }), {
    codec: "hevc",
    container: "mp4",
    resolution: "source",
    audio: "copyOrAac",
    tenBit: false,
    encoder: null,
    quality: "balanced",
    outputDir: "/변환 결과",
    suffix: " 작게",
  });
  // Same cases as command::check_name_suffix.
  for (const ok of ["_h265", "", " 작게", "_v2", "-보관용 (2026)", "가".repeat(60)]) assert.equal(suffixProblem(ko, ok), null, ok);
  for (const bad of ["a/b", "a\\b", "a:b", "제목?", "x\u0007", "_작게.part", ".PART2", "가".repeat(61)]) assert.notEqual(suffixProblem(ko, bad), null, bad);
});

test("encoder line: automatic, picked, none", () => {
  const vt = { name: "hevc_videotoolbox", label: "VideoToolbox", hardware: true, fallback: "libx265", fallbackLabel: "libx265" };
  const x265 = { name: "libx265", label: "libx265", hardware: false, fallback: null, fallbackLabel: null };
  const svt = { name: "libsvtav1", label: "SVT-AV1", hardware: false, fallback: null, fallbackLabel: null };
  const options: EncoderOptions = { auto: { h264: null, hevc: vt, av1: svt }, hevc: [vt, x265], av1: [svt] };
  assert.equal(encoderInUse(options, DEFAULT_CONVERT)?.name, "hevc_videotoolbox");
  assert.equal(encoderNote(ko, options, DEFAULT_CONVERT), "하드웨어 인코딩 · 실패하면 libx265(으)로 다시");
  const picked = { ...DEFAULT_CONVERT, encoder: "libx265" };
  assert.equal(encoderInUse(options, picked)?.name, "libx265");
  assert.equal(encoderNote(ko, options, picked), "CPU 인코딩 · 느리지만 같은 화질에서 하드웨어보다 작게 만듭니다");
  assert.match(encoderNote(ko, options, withCodec(DEFAULT_CONVERT, "av1")), /AV1 하드웨어 인코더가 없어/);
  assert.equal(encoderInUse(options, { ...DEFAULT_CONVERT, encoder: "hevc_nvenc" }), null, "gone after a new test");
  assert.equal(encoderNote(ko, null, DEFAULT_CONVERT), "");
});

function queued(id: number, state: QueueItem["state"], patch: Partial<QueueItem> = {}): QueueItem {
  return {
    id,
    info: {
      kind: "convert",
      source: "/v/a.mp4",
      sourceName: "a.mp4",
      quality: "balanced",
      summary: { length: 600, codec: "hevc", encoder: "VideoToolbox", fallback: "libx265", tenBit: false, hdr: false, scaled: null, audio: { kind: "copy" }, container: "mp4" },
      estimatedBytes: null,
    },
    state,
    canceling: false,
    output: "/v/a_h265.mp4",
    outputBytes: null,
    progress: null,
    note: null,
    error: null,
    commandLine: "",
    log: null,
    startedAt: null,
    elapsedMs: null,
    finishedOrder: null,
    ...patch,
  };
}

test("row status: reading, refused, queued, skipped, chosen", () => {
  const h264 = media("/v/제주 여행.mov", video(3840, 2160));
  const c = DEFAULT_CONVERT;
  const loading: ConvertRow = { path: "/v/a.mp4", name: "a.mp4", probe: { state: "loading" }, checked: true, itemId: null };
  assert.deepEqual([rowStatus(ko, loading, c, undefined).label, rowStatus(ko, loading, c, undefined).included], ["읽는 중…", false]);
  const bad: ConvertRow = { ...loading, probe: { state: "error", error: { code: "notMedia" } } };
  assert.deepEqual([rowStatus(ko, bad, c, undefined).label, rowStatus(ko, bad, c, undefined).why], ["읽지 못함", "영상 파일로 읽을 수 없습니다. FFmpeg가 모르는 형식이거나 손상된 파일입니다."]);
  assert.equal(rowStatus(ko, ready("/v/m.m4a", media("/v/m.m4a", null)), c, undefined).label, "건너뜀 · 비디오 없음");

  const row = ready("/v/제주 여행.mov", h264);
  const s = rowStatus(ko, row, { ...c, resolution: "p1080" }, undefined);
  assert.deepEqual([s.label, s.included, s.selectable, s.notes], ["변환 예정", true, true, ["1920×1080로 축소"]]);
  assert.equal(rowStatus(ko, { ...row, checked: false }, c, undefined).label, "제외");

  // An HEVC HDR phone video while converting to H.265: skipped (the option decides, not the box).
  const hevc = ready("/v/아이폰.MOV", media("/v/아이폰.MOV", video(3840, 2160, { codec: "hevc", hdr: true })));
  const skipped = rowStatus(ko, hevc, c, undefined);
  assert.deepEqual([skipped.label, skipped.included, skipped.selectable], ["건너뜀 · 이미 H.265", false, false]);
  const kept = rowStatus(ko, hevc, { ...c, skipSame: false }, undefined);
  assert.deepEqual([kept.label, kept.included, kept.notes], ["변환 예정", true, ["10비트 · HDR 유지"]]);
  assert.equal(rowStatus(ko, hevc, withCodec(c, "av1"), undefined).label, "변환 예정");

  // Sent to the queue: follows its item, and cannot go in twice while it waits or runs.
  const sent = { ...row, checked: false, itemId: 5 };
  assert.deepEqual([rowStatus(ko, sent, c, queued(5, "waiting")).label, rowStatus(ko, sent, c, queued(5, "waiting")).selectable], ["작업 큐 · 대기 중", false]);
  assert.equal(rowStatus(ko, sent, c, queued(5, "running", { progress: { id: 1, percent: 37.8, outTime: 1, speed: 2, fps: 60, note: null } })).label, "변환 중 37%");
  assert.equal(rowStatus(ko, sent, c, queued(5, "running", { canceling: true })).label, "취소하는 중…");
  assert.equal(rowStatus(ko, sent, c, queued(5, "done")).label, "완료");
  const failed = rowStatus(ko, sent, c, queued(5, "failed", { error: { code: "raw", text: "No space left on device" } }));
  assert.deepEqual([failed.label, failed.why, failed.selectable], ["실패", "No space left on device", true]);
  assert.equal(rowStatus(ko, { ...sent, checked: true }, c, queued(5, "done")).label, "변환 예정", "checked again: converts once more");
  assert.equal(rowStatus(ko, sent, c, undefined).label, "제외", "the item was taken off the queue");
});

test("list: paths once, totals, headings", () => {
  const first = addPaths([], ["/v/a.mp4", "/v/b.mkv", "/v/a.mp4"]);
  assert.deepEqual([first.added, first.rows.map((r) => r.name)], [2, ["a.mp4", "b.mkv"]]);
  const again = addPaths(first.rows, ["/v/b.mkv", "C:\\영상\\c.mov"]);
  assert.deepEqual([again.added, again.rows.map((r) => r.name)], [1, ["a.mp4", "b.mkv", "c.mov"]]);
  assert.equal(addPaths(again.rows, ["/v/a.mp4"]).rows, again.rows, "nothing new: same rows");

  const rows = [
    ready("/v/a.mp4", media("/v/a.mp4", video(1920, 1080), { sizeBytes: 4_200_000_000 })),
    ready("/v/b.mp4", media("/v/b.mp4", video(1920, 1080), { sizeBytes: 812_000_000 })),
    ready("/v/c.mp4", media("/v/c.mp4", video(1920, 1080), { sizeBytes: 1 }), { checked: false }),
  ];
  const totals = listTotals(rows, rows.map((r) => rowStatus(ko, r, DEFAULT_CONVERT, undefined)));
  assert.deepEqual(totals, { count: 2, bytes: 5_012_000_000 });
  assert.equal(totalsLine(ko, totals), "2개 · 원본 5.0GB");
  assert.deepEqual(listHeading(ko, 3, totals, "av1"), { title: "파일 3개", sub: " · 2개를 AV1로 변환 예정" });
  assert.deepEqual(listHeading(ko, 0, { count: 0, bytes: 0 }, "hevc"), { title: "파일 0개", sub: "" });
});

test("예상 크기 cell: the test encode while chosen, the queue's estimate, then the size made", () => {
  const row = ready("/v/제주 여행.mov", media("/v/제주 여행.mov", video(3840, 2160)));
  const chosen = rowStatus(ko, row, DEFAULT_CONVERT, undefined);
  const at = (state: EstimateEntry) => estimateCell(ko, chosen, state, false, undefined);
  const k = { path: row.path, key: "k" };
  assert.deepEqual(estimateCell(ko, chosen, undefined, false, undefined), { text: "—", tone: "muted", title: null });
  assert.equal(at({ ...k, state: "waiting" }).text, "대기");
  assert.deepEqual(estimateCell(ko, chosen, { ...k, state: "waiting" }, true, undefined), { text: "큐가 끝나면", tone: "muted", title: "작업 큐가 도는 동안은 시험 인코딩을 쉽니다" });
  assert.deepEqual([at({ ...k, state: "running", percent: 67.9 }).text, at({ ...k, state: "running", percent: 67.9 }).tone], ["계산 중 67%", "busy"]);
  assert.deepEqual(at({ ...k, state: "done", bytes: 1_234_000_000, low: 900_000_000, high: 1_600_000_000, encoder: "VideoToolbox", whole: false }), {
    text: "≈ 1.2GB",
    tone: "est",
    title: "3곳 × 10초 시험 인코딩 (VideoToolbox) · 구간에 따라 900MB–1.6GB",
  });
  assert.equal(at({ ...k, state: "done", bytes: 5e6, low: 5e6, high: 5.1e6, encoder: "libx265", whole: false }).title, "3곳 × 10초 시험 인코딩 (libx265)", "even stretches: no range");
  assert.equal(at({ ...k, state: "done", bytes: 4321, low: 4321, high: 4321, encoder: "SVT-AV1", whole: true }).title, "파일 전체를 시험 인코딩한 크기 (SVT-AV1)");
  assert.deepEqual(at({ ...k, state: "failed", error: { code: "sampleFailed", why: { code: "raw", text: "x" } } }), { text: "계산 못 함", tone: "bad", title: "시험 인코딩이 실패했습니다: x" });

  // Sent to the queue: the estimate it took along, then what was made; unchecked: nothing.
  const sent = { ...row, checked: false, itemId: 5 };
  const withEstimate = (state: QueueItem["state"], patch: Partial<QueueItem> = {}) => {
    const q = queued(5, state, patch);
    return { ...q, info: { ...q.info, estimatedBytes: 1_500_000_000 } } as QueueItem;
  };
  const cell = (item: QueueItem | undefined) => estimateCell(ko, rowStatus(ko, sent, DEFAULT_CONVERT, item), undefined, false, item);
  assert.equal(cell(withEstimate("waiting")).text, "≈ 1.5GB");
  assert.equal(cell(withEstimate("running")).text, "≈ 1.5GB");
  assert.equal(cell(queued(5, "waiting")).text, "—", "queued before its estimate was ready");
  assert.deepEqual(cell(withEstimate("done", { outputBytes: 1_420_000_000 })), { text: "1.4GB", tone: "plain", title: "변환 결과 크기" });
  assert.equal(cell(undefined).text, "—");
});

test("예상 크기 totals: about how much, how much smaller, or how far it got", () => {
  const rows = [
    ready("/v/a.mp4", media("/v/a.mp4", video(1920, 1080), { sizeBytes: 4_000_000_000 })),
    ready("/v/b.mp4", media("/v/b.mp4", video(1920, 1080), { sizeBytes: 1_000_000_000 })),
    ready("/v/c.mp4", media("/v/c.mp4", video(1920, 1080), { sizeBytes: 9 }), { checked: false }),
  ];
  const statuses = rows.map((r) => rowStatus(ko, r, DEFAULT_CONVERT, undefined));
  const done = (path: string, bytes: number): [string, EstimateEntry] => [path, { path, key: "k", state: "done", bytes, low: bytes, high: bytes, encoder: "VideoToolbox", whole: false }];
  const both = new Map([done("/v/a.mp4", 2_000_000_000), done("/v/b.mp4", 1_000_000_000), done("/v/c.mp4", 1)]);
  const t = estimateTotals(rows, statuses, both);
  assert.deepEqual(t, { known: 2, bytes: 3_000_000_000, comparedBytes: 3_000_000_000, sourceBytes: 5_000_000_000, pending: 0, failed: 0 }, "the unchecked file is left out");
  assert.deepEqual(estimateLine(ko, t, false), { text: "→ 약 3.0GB", saving: "40% 절감", note: null });

  const half = new Map([done("/v/a.mp4", 2_000_000_000), ["/v/b.mp4", { path: "/v/b.mp4", key: "k", state: "running", percent: 30 }] as [string, EstimateEntry]]);
  assert.deepEqual(estimateLine(ko, estimateTotals(rows, statuses, half), false), { text: "→ 예상 크기 계산 중 (1/2)", saving: null, note: null });
  assert.equal(estimateLine(ko, estimateTotals(rows, statuses, half), true)?.text, "→ 예상 크기는 작업 큐가 끝나면 계산합니다");
  assert.equal(estimateLine(ko, estimateTotals(rows, statuses, new Map()), false)?.text, "→ 예상 크기 계산 중 (0/2)");

  const oneFailed = new Map([done("/v/a.mp4", 2_000_000_000), ["/v/b.mp4", { path: "/v/b.mp4", key: "", state: "failed", error: { code: "sampleEmpty" } }] as [string, EstimateEntry]]);
  assert.deepEqual(estimateLine(ko, estimateTotals(rows, statuses, oneFailed), false), { text: "→ 약 2.0GB", saving: "50% 절감", note: "1개는 계산하지 못해 빼고 더했습니다" });
  const allFailed = new Map(rows.map((r) => [r.path, { path: r.path, key: "", state: "failed", error: { code: "sampleEmpty" } }] as [string, EstimateEntry]));
  assert.equal(estimateLine(ko, estimateTotals(rows, statuses, allFailed), false)?.text, "→ 예상 크기를 계산하지 못했습니다");
  assert.equal(estimateLine(ko, estimateTotals([], [], both), false), null, "nothing to convert");

  assert.equal(savingText(ko, 1_200, 1_000), "20% 증가");
  assert.equal(savingText(ko, 1_000, 1_000), "0% 절감");
  assert.equal(savingText(ko, 1, 0), null);
});

test("the estimates are asked again for what changes the test, not for the name or the folder", () => {
  const c = DEFAULT_CONVERT;
  assert.equal(estimateSettingsKey(c), estimateSettingsKey({ ...c, suffix: " 작게", outputDir: "/변환 결과" }));
  for (const other of [{ quality: "high" as const }, { encoder: "libx265" }, { container: "mkv" as const }, { resolution: "p720" as const }, { audio: "aac128" as const }, { tenBit: true }]) {
    assert.notEqual(estimateSettingsKey(c), estimateSettingsKey({ ...c, ...other }), JSON.stringify(other));
  }
  assert.notEqual(estimateSettingsKey(c), estimateSettingsKey(withCodec(c, "av1")));
});

test("English: rows, totals and estimates", () => {
  const row = ready("/v/a.mp4", media("/v/a.mp4", video(3840, 2160, { codec: "h264" }), { sizeBytes: 3_000_000_000 }));
  const c = DEFAULT_CONVERT;
  assert.equal(rowStatus(en, row, c, undefined).label, "To convert");
  assert.deepEqual(rowStatus(en, row, { ...c, resolution: "p1080" }, undefined).notes, ["Scaled to 1920×1080"]);
  const failed: Msg = { code: "raw", text: "No space left on device" };
  assert.deepEqual(
    [rowStatus(en, { ...row, checked: false, itemId: 5 }, c, queued(5, "failed", { error: failed })).label],
    ["Failed"],
  );
  assert.deepEqual(listHeading(en, 1, { count: 1, bytes: 3e9 }, "hevc"), { title: "1 file", sub: " · 1 to convert to H.265" });
  assert.equal(totalsLine(en, { count: 2, bytes: 5e9 }), "2 files · 5.0GB original");
  assert.equal(outputPattern(en, DEFAULT_CONVERT), "{name}_h265.mp4");
  assert.equal(suffixProblem(en, "제목?"), "“?” isn't allowed.");
  assert.equal(savingText(en, 600, 1_000), "40% smaller");
  assert.equal(savingText(en, 1_200, 1_000), "20% larger");
  const k = { path: "/v/a.mp4", key: "k" };
  const status = rowStatus(en, row, c, undefined);
  assert.equal(
    estimateCell(en, status, { ...k, state: "done", bytes: 1_234_000_000, low: 900_000_000, high: 1_600_000_000, encoder: "VideoToolbox", whole: false }, false, undefined).title,
    "Test encode of 3 × 10 s (VideoToolbox) · 900MB–1.6GB depending on the part",
  );
  assert.equal(
    estimateCell(en, status, { ...k, state: "failed", error: { code: "sampleFailed", why: null } }, false, undefined).title,
    "The test encode failed.",
  );
});
