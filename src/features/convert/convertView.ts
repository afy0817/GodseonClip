// What the 일괄 변환 screen shows and sends (canvas Convert.dc.html): the settings, each listed file's
// row and status, the totals. Pure functions, no IPC (`npm run test:ts` runs them: imports are types
// or `.ts` modules). Rust decides the real thing (command::plan_convert); these mirror it to explain.

import type {
  AudioChoice,
  Container,
  ConvertSettings,
  EncoderChoice,
  EncoderOptions,
  EstimateEntry,
  MediaInfo,
  Msg,
  Quality,
  QueueItem,
  Resolution,
  VideoStream,
} from "../../lib/ipc";
import type { Messages } from "../../lib/i18n/ko.ts";
import { say } from "../../lib/i18n/text.ts";
import { formatBytes } from "../../lib/time.ts";
import { wantsTenBit } from "../editor/exportOptions.ts";
import { codecLabel, formatClock } from "../editor/format.ts";
import { rateText, resultRate, VIDEO_RATES } from "../editor/frameRate.ts";

export type ConvertCodec = "hevc" | "av1";

/** The screen's settings. Kept while the app runs (not saved — 2026-11-24 decision). */
export interface ConvertChoice {
  codec: ConvertCodec;
  /** null: "자동". */
  encoder: string | null;
  quality: Quality;
  container: Container;
  resolution: Resolution;
  /** 프레임레이트: at most this many fps (never more than a file has); null: 원본 유지. */
  frameRate: number | null;
  audio: AudioChoice;
  tenBit: boolean;
  /** 출력과 같은 코덱인 파일 건너뛰기. */
  skipSame: boolean;
  /** null: next to each original. */
  outputDir: string | null;
  /** null: the codec's own ending (`_h265` · `_av1`). */
  suffix: string | null;
}

export const DEFAULT_CONVERT: ConvertChoice = {
  codec: "hevc",
  encoder: null,
  quality: "balanced",
  container: "mp4",
  resolution: "source",
  frameRate: null,
  audio: "copyOrAac",
  tenBit: false,
  skipSame: true,
  outputDir: null,
  suffix: null,
};

/** Same list as convert::VIDEO_EXTENSIONS (what 폴더 추가 lists, and the file dialog's filter). */
export const CONVERT_EXTENSIONS = [
  "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "asf", "flv", "f4v", "ts", "m2ts", "mts", "m2t", "mpg", "mpeg",
  "vob", "3gp", "3g2", "ogv", "mxf", "dv", "divx",
];

export function codecs(t: Messages): { value: ConvertCodec; label: string; use: string }[] {
  return [
    { value: "hevc", label: "H.265", use: t.convert.codecUse.hevc },
    { value: "av1", label: "AV1", use: t.convert.codecUse.av1 },
  ];
}

/** H.265: MP4 · MKV · MOV; AV1: MP4 · MKV · WebM (command::Container::holds). */
export function containersFor(codec: ConvertCodec): Container[] {
  return codec === "hevc" ? ["mp4", "mkv", "mov"] : ["mp4", "mkv", "webm"];
}

const RESOLUTION_VALUES: Resolution[] = ["source", "p1080", "p720"];
const AUDIO_VALUES: AudioChoice[] = ["copyOrAac", "aac192", "aac128"];

export function resolutions(t: Messages): { value: Resolution; label: string }[] {
  return RESOLUTION_VALUES.map((value) => ({ value, label: t.convert.resolution[value] }));
}

/** 원본 유지 · 60 · 30 · 24 fps로 줄이기 (the editor's video presets). */
export function frameRates(t: Messages): { value: number | null; label: string }[] {
  return [{ value: null, label: t.convert.frameRate.source }, ...VIDEO_RATES.map((n) => ({ value: n, label: t.convert.frameRate.cap(n) }))];
}

export function audioChoices(t: Messages): { value: AudioChoice; label: string }[] {
  return AUDIO_VALUES.map((value) => ({ value, label: t.convert.audio[value] }));
}

/** Every preset is calibrated to x265's VMAF at these CRFs (command::quality_values). */
export function qualityNote(t: Messages, q: Quality): string {
  return t.convert.qualityNote[q];
}

export function defaultSuffix(codec: ConvertCodec): string {
  return codec === "hevc" ? "_h265" : "_av1";
}

export function suffixOf(choice: ConvertChoice): string {
  return choice.suffix ?? defaultSuffix(choice.codec);
}

/** Another codec: a container it cannot go in becomes MP4, the encoder "자동" (names are per codec). */
export function withCodec(choice: ConvertChoice, codec: ConvertCodec): ConvertChoice {
  const container = containersFor(codec).includes(choice.container) ? choice.container : "mp4";
  return { ...choice, codec, container, encoder: null };
}

export function toSettings(choice: ConvertChoice): ConvertSettings {
  return {
    codec: choice.codec,
    container: choice.container,
    resolution: choice.resolution,
    audio: choice.audio,
    tenBit: choice.tenBit,
    frameRate: choice.frameRate,
    encoder: choice.encoder,
    quality: choice.quality,
    outputDir: choice.outputDir,
    suffix: suffixOf(choice),
  };
}

/**
 * What the test encode depends on, as one string: the settings without the name ending and the
 * folder (they change nothing in it), so typing a name does not ask for the estimates again.
 */
export function estimateSettingsKey(choice: ConvertChoice): string {
  const { suffix: _suffix, outputDir: _outputDir, ...rest } = toSettings(choice);
  return JSON.stringify(rest);
}

/** "{이름}_h265.mp4" · "{name}_h265.mp4" */
export function outputPattern(t: Messages, choice: ConvertChoice): string {
  return `${t.convert.namePlaceholder}${suffixOf(choice)}.${choice.container}`;
}

/** Same check as command::check_name_suffix; null when the ending can be used. */
export function suffixProblem(t: Messages, suffix: string): string | null {
  const w = t.convert.suffixProblem;
  if ([...suffix].length > 60) return w.tooLong(60);
  const bad = [...suffix].find((c) => c.charCodeAt(0) < 32 || c.charCodeAt(0) === 127 || '/\\:*?"<>|'.includes(c));
  if (bad !== undefined) return bad.charCodeAt(0) < 32 || bad.charCodeAt(0) === 127 ? w.control : w.char(bad);
  if (suffix.toLowerCase().includes(".part")) return w.part;
  return null;
}

// ---------------------------------------------------------------------------------------------
// The encoder line

export function encodersFor(options: EncoderOptions | null, codec: ConvertCodec): EncoderChoice[] {
  return options ? options[codec] : [];
}

/** The encoder a conversion runs: the picked one, or what "자동" picks. */
export function encoderInUse(options: EncoderOptions | null, choice: ConvertChoice): EncoderChoice | null {
  if (!options) return null;
  if (choice.encoder) return options[choice.codec].find((e) => e.name === choice.encoder) ?? null;
  return options.auto[choice.codec];
}

/** "VideoToolbox (하드웨어)" · "libx265 (CPU)". */
export function encoderLabel(t: Messages, e: EncoderChoice): string {
  return `${e.label} (${e.hardware ? t.convert.encoder.hardware : "CPU"})`;
}

/** Under the encoder select: what the choice means. */
export function encoderNote(t: Messages, options: EncoderOptions | null, choice: ConvertChoice): string {
  const w = t.convert.encoder;
  const enc = encoderInUse(options, choice);
  if (!options) return "";
  if (!enc) return choice.codec === "hevc" ? w.noHevc : w.noAv1;
  if (enc.hardware) return enc.fallbackLabel ? w.hardwareFallback(enc.fallbackLabel) : w.hardwareNote;
  if (choice.codec === "av1" && !options.av1.some((e) => e.hardware)) return w.av1Cpu;
  if (choice.codec === "hevc" && options.hevc.some((e) => e.hardware)) return w.cpuSmaller;
  return w.cpu;
}

// ---------------------------------------------------------------------------------------------
// The list

/** What an add said: Rust's notices, and the files that were listed already. */
export type ListNotice = Msg | { code: "alreadyListed"; count: number };

export function noticeText(t: Messages, n: ListNotice): string {
  return n.code === "alreadyListed" ? t.convert.alreadyListed(n.count) : say(t, n);
}

export type ProbeState = { state: "loading" } | { state: "ready"; info: MediaInfo } | { state: "error"; error: Msg };

export interface ConvertRow {
  path: string;
  name: string;
  probe: ProbeState;
  checked: boolean;
  /** The queue item the last 변환 시작 made from this row. */
  itemId: number | null;
}

/** Last path component, for macOS / Linux and Windows paths alike. */
export function nameOf(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

export function newRow(path: string): ConvertRow {
  return { path, name: nameOf(path), probe: { state: "loading" }, checked: true, itemId: null };
}

/** The rows with `paths` added at the end (each path once); how many were new. */
export function addPaths(rows: ConvertRow[], paths: string[]): { rows: ConvertRow[]; added: number } {
  const listed = new Set(rows.map((r) => r.path));
  const fresh: ConvertRow[] = [];
  for (const p of paths) {
    if (listed.has(p)) continue;
    listed.add(p);
    fresh.push(newRow(p));
  }
  return { rows: fresh.length ? [...rows, ...fresh] : rows, added: fresh.length };
}

/** Width × height as players show them (rotation applied, as FFmpeg's autorotate does). */
export function displayedSize(v: VideoStream): [number, number] {
  const quarter = v.rotation !== null && ((Math.round(v.rotation / 90) % 2) + 2) % 2 === 1;
  return quarter ? [v.height, v.width] : [v.width, v.height];
}

/** The size after 해상도 (command::downscale: short side down to 1080 / 720, `-2` rounding); null: kept. */
export function downscaledSize(v: VideoStream, resolution: Resolution): [number, number] | null {
  const short = resolution === "p1080" ? 1080 : resolution === "p720" ? 720 : null;
  const [w, h] = displayedSize(v);
  if (short === null || w === 0 || h === 0 || Math.min(w, h) <= short) return null;
  const other = (long: number, small: number) => Math.floor((long * short + small) / (2 * small)) * 2;
  return w >= h ? [other(w, h), short] : [short, other(h, w)];
}

/** "H.264", "H.265 · HDR", "H.265 · 10비트". */
export function codecCell(t: Messages, info: MediaInfo): string {
  const v = info.video;
  if (!v) return info.audio.length ? t.common.audio(codecLabel(info.audio[0].codec)) : "—";
  if (v.hdr) return `${codecLabel(v.codec)} · HDR`;
  return wantsTenBit(info) ? `${codecLabel(v.codec)} · ${t.queueView.tenBit}` : codecLabel(v.codec);
}

/** "3840×2160 · 30", "1920×1080 · 29.97" (as shown: a phone video held upright is taller). */
export function sizeCell(info: MediaInfo): string {
  const v = info.video;
  if (!v) return "—";
  const [w, h] = displayedSize(v);
  const fps = v.fps ? ` · ${Number(v.fps.toFixed(2))}` : "";
  return `${w}×${h}${fps}`;
}

export function lengthCell(info: MediaInfo): string {
  return formatClock(info.duration);
}

export type Tone = "plain" | "ready" | "busy" | "done" | "bad" | "muted";

export interface RowStatus {
  /** Goes in at the next 변환 시작. */
  included: boolean;
  /** Its checkbox can be changed. */
  selectable: boolean;
  label: string;
  tone: Tone;
  /** Under the file name: what the conversion does to it. */
  notes: string[];
  /** Why (a read error, a failed item's error). */
  why: string | null;
}

/** What the conversion keeps, drops or changes (shown under the name). */
export function planNotes(t: Messages, info: MediaInfo, choice: ConvertChoice): string[] {
  const v = info.video;
  if (!v) return [];
  const w = t.convert.notes;
  const notes: string[] = [];
  const size = downscaledSize(v, choice.resolution);
  if (size) notes.push(t.queueView.scaledTo(size[0], size[1]));
  const rate = resultRate(v, choice.frameRate, false);
  if (rate !== null) notes.push(w.toFrameRate(rateText(rate)));
  if (v.hdr) notes.push(w.keepsHdr);
  else if (wantsTenBit(info)) notes.push(w.keepsTenBit);
  else if (choice.tenBit) notes.push(w.toTenBit);
  if (info.subtitleCount > 0) notes.push(w.subtitlesDropped(info.subtitleCount));
  return notes;
}

/** The status cell of one row; `item` is the queue item of its last 변환 시작 (if still in the queue). */
export function rowStatus(t: Messages, row: ConvertRow, choice: ConvertChoice, item: QueueItem | undefined): RowStatus {
  const w = t.convert.row;
  const base = { included: false, selectable: false, notes: [] as string[], why: null as string | null };
  if (row.probe.state === "loading") return { ...base, label: w.reading, tone: "muted" };
  if (row.probe.state === "error") return { ...base, label: w.unreadable, tone: "bad", why: say(t, row.probe.error) };
  const info = row.probe.info;
  if (!info.video) return { ...base, label: w.noVideo, tone: "muted" };
  const notes = planNotes(t, info, choice);
  if (item?.state === "waiting") return { ...base, notes, label: w.queued, tone: "busy" };
  if (item?.state === "running") {
    const label = item.canceling ? w.canceling : w.converting(Math.floor(item.progress?.percent ?? 0));
    return { ...base, notes, label, tone: "busy" };
  }
  if (choice.skipSame && info.video.codec === choice.codec) {
    return { ...base, notes: [], label: w.already(codecLabel(choice.codec)), tone: "muted", why: w.alreadyWhy };
  }
  const selectable = { ...base, selectable: true, notes };
  if (row.checked) return { ...selectable, included: true, label: w.planned, tone: "ready" };
  switch (item?.state) {
    case "done":
      return { ...selectable, label: w.done, tone: "done" };
    case "failed":
      return { ...selectable, label: w.failed, tone: "bad", why: item.error ? say(t, item.error) : null };
    case "canceled":
      return { ...selectable, label: w.canceled, tone: "muted" };
    default:
      return { ...selectable, label: w.excluded, tone: "plain" };
  }
}

export interface ListTotals {
  /** Files going in at the next 변환 시작. */
  count: number;
  /** Their size together (files with an unknown size left out). */
  bytes: number;
}

export function listTotals(rows: ConvertRow[], statuses: RowStatus[]): ListTotals {
  let count = 0;
  let bytes = 0;
  rows.forEach((r, i) => {
    if (!statuses[i].included) return;
    count++;
    if (r.probe.state === "ready") bytes += r.probe.info.sizeBytes ?? 0;
  });
  return { count, bytes };
}

/** "파일 4개" + " · 3개를 H.265로 변환 예정". */
export function listHeading(t: Messages, rows: number, totals: ListTotals, codec: ConvertCodec): { title: string; sub: string } {
  const w = t.convert.list;
  return { title: w.files(rows), sub: rows > 0 ? ` · ${w.planned(totals.count, codecLabel(codec))}` : "" };
}

/** "3개 · 원본 9.1GB" */
export function totalsLine(t: Messages, totals: ListTotals): string {
  return t.convert.list.totals(totals.count, formatBytes(totals.bytes));
}

// ---------------------------------------------------------------------------------------------
// 예상 크기 (Phase 3 step 3): the test encode of each file (estimate.rs), its cell and the totals

export type EstimateTone = "est" | "plain" | "busy" | "muted" | "bad";

export interface EstimateCell {
  text: string;
  tone: EstimateTone;
  /** On hover: how it was measured, or why not. */
  title: string | null;
}

const NO_ESTIMATE: EstimateCell = { text: "—", tone: "muted", title: null };

/** "3곳 × 10초 시험 인코딩 (VideoToolbox) · 구간에 따라 0.9–1.6GB" */
export function estimateHow(t: Messages, e: Extract<EstimateEntry, { state: "done" }>): string {
  const w = t.convert.estimate;
  if (e.whole) return w.whole(e.encoder);
  const range = e.high > e.low * 1.05 ? ` · ${w.range(formatBytes(e.low), formatBytes(e.high))}` : "";
  return `${w.samples(e.encoder)}${range}`;
}

/**
 * The 예상 크기 cell of a row. Rows going in at the next 변환 시작 show their test encode (`entry`;
 * `paused`: the queue runs, tests wait); rows in the queue the estimate their item took along; done
 * rows the size made.
 */
export function estimateCell(t: Messages, status: RowStatus, entry: EstimateEntry | undefined, paused: boolean, item: QueueItem | undefined): EstimateCell {
  const w = t.convert.estimate;
  if (!status.included) {
    if (item?.state === "done" && item.outputBytes !== null) return { text: formatBytes(item.outputBytes), tone: "plain", title: w.resultSize };
    if ((item?.state === "waiting" || item?.state === "running") && item.info.kind === "convert" && item.info.estimatedBytes !== null) {
      return { text: `≈ ${formatBytes(item.info.estimatedBytes)}`, tone: "est", title: w.whenQueued };
    }
    return NO_ESTIMATE;
  }
  switch (entry?.state) {
    case undefined:
      return NO_ESTIMATE;
    case "waiting":
      return paused
        ? { text: w.afterQueue, tone: "muted", title: w.afterQueueWhy }
        : { text: w.waiting, tone: "muted", title: w.waitingWhy };
    case "running":
      return { text: w.running(Math.floor(entry.percent)), tone: "busy", title: w.runningWhy };
    case "done":
      return { text: `≈ ${formatBytes(entry.bytes)}`, tone: "est", title: estimateHow(t, entry) };
    case "failed":
      return { text: w.failed, tone: "bad", title: say(t, entry.error) };
  }
}

export interface EstimateTotals {
  /** Files going in at the next 변환 시작 with an estimate, and their estimates together. */
  known: number;
  bytes: number;
  /** Of those whose original's size is known: their estimates, and the originals' size. */
  comparedBytes: number;
  sourceBytes: number;
  /** Still to be tried, or that could not be. */
  pending: number;
  failed: number;
}

export function estimateTotals(rows: ConvertRow[], statuses: RowStatus[], entries: Map<string, EstimateEntry>): EstimateTotals {
  const t: EstimateTotals = { known: 0, bytes: 0, comparedBytes: 0, sourceBytes: 0, pending: 0, failed: 0 };
  rows.forEach((r, i) => {
    if (!statuses[i].included) return;
    const e = entries.get(r.path);
    if (e?.state === "done") {
      t.known++;
      t.bytes += e.bytes;
      const size = r.probe.state === "ready" ? r.probe.info.sizeBytes : null;
      if (size != null) {
        t.comparedBytes += e.bytes;
        t.sourceBytes += size;
      }
    } else if (e?.state === "failed") t.failed++;
    else t.pending++;
  });
  return t;
}

/** "41% 절감" · "12% 증가" against the originals' size; null without one. */
export function savingText(t: Messages, bytes: number, sourceBytes: number): string | null {
  if (sourceBytes <= 0) return null;
  const p = Math.round((1 - bytes / sourceBytes) * 100);
  return p >= 0 ? t.convert.estimate.saved(p) : t.convert.estimate.grew(-p);
}

/**
 * The line after "N개 · 원본 X": "→ 약 Y" + "(Z% 절감)" once every file is tried, else how far it
 * got; null with nothing to convert.
 */
export function estimateLine(t: Messages, totals: EstimateTotals, paused: boolean): { text: string; saving: string | null; note: string | null } | null {
  const w = t.convert.estimate;
  const total = totals.known + totals.pending + totals.failed;
  if (total === 0) return null;
  if (totals.pending > 0) {
    const text = paused ? `→ ${w.lineAfterQueue}` : `→ ${w.lineRunning(totals.known + totals.failed, total)}`;
    return { text, saving: null, note: null };
  }
  if (totals.known === 0) return { text: `→ ${w.lineFailed}`, saving: null, note: null };
  const note = totals.failed > 0 ? w.lineSomeFailed(totals.failed) : null;
  return { text: `→ ${w.about(formatBytes(totals.bytes))}`, saving: savingText(t, totals.comparedBytes, totals.sourceBytes), note };
}
