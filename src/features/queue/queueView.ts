// What the 작업 큐 screen and the editor's export panel say about queue items. Pure functions, no
// IPC (`npm run test:ts` runs them: imports are types or `.ts` modules).

import type { AudioOut, ConvertTask, ItemProgress, QueueItem, QueueSnapshot, Quality, VideoCodec } from "../../lib/ipc";
import type { Messages } from "../../lib/i18n/ko.ts";
import { say } from "../../lib/i18n/text.ts";
import { formatBytes, formatTime } from "../../lib/time.ts";
import { codecLabel, formatClock } from "../editor/format.ts";

export interface QueueGroups {
  running: QueueItem[];
  /** In the order they will run. */
  waiting: QueueItem[];
  /** Done, failed and canceled, the last finished first. */
  finished: QueueItem[];
}

export function groupItems(items: QueueItem[]): QueueGroups {
  const finished = items.filter((i) => i.state === "done" || i.state === "failed" || i.state === "canceled");
  finished.sort((a, b) => (b.finishedOrder ?? 0) - (a.finishedOrder ?? 0));
  return {
    running: items.filter((i) => i.state === "running"),
    waiting: items.filter((i) => i.state === "waiting"),
    finished,
  };
}

/** Running + waiting: what the header badge counts and quitting would cancel. */
export function activeCount(items: QueueItem[]): number {
  return items.filter((i) => i.state === "running" || i.state === "waiting").length;
}

/** "진행 중 1 · 대기 2 · 완료 2 · 실패 1" (취소 only when there is one). */
export function queueSummary(t: Messages, items: QueueItem[]): string {
  const n = (s: QueueItem["state"]) => items.filter((i) => i.state === s).length;
  const w = t.queueView.count;
  const parts = [w.running(n("running")), w.waiting(n("waiting")), w.done(n("done")), w.failed(n("failed"))];
  if (n("canceled") > 0) parts.push(w.canceled(n("canceled")));
  return parts.join(" · ");
}

/** Jobs that start before a waiting item: the running ones and the waiting ones ahead of it. */
export function jobsAhead(items: QueueItem[], id: number): number {
  const waiting = items.filter((i) => i.state === "waiting");
  const at = waiting.findIndex((i) => i.id === id);
  if (at < 0) return 0;
  return items.filter((i) => i.state === "running").length + at;
}

/** A progress event applied to the snapshot (only to the item while it runs). */
export function withProgress(snapshot: QueueSnapshot | null, p: ItemProgress): QueueSnapshot | null {
  if (!snapshot) return snapshot;
  const index = snapshot.items.findIndex((i) => i.id === p.id && i.state === "running");
  if (index < 0) return snapshot;
  const items = snapshot.items.slice();
  const item = items[index];
  items[index] = { ...item, progress: p.progress, note: p.progress.note ?? item.note };
  return { ...snapshot, items };
}

/** The newer of two snapshots (events and replies can cross). */
export function newer(current: QueueSnapshot | null, next: QueueSnapshot): QueueSnapshot {
  return current && current.version > next.version ? current : next;
}

export function baseName(path: string): string {
  const i = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return i >= 0 ? path.slice(i + 1) : path;
}

function extensionOf(path: string): string {
  const name = baseName(path);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

function qualityLabel(t: Messages, q: Quality): string {
  return t.quality[q]?.label ?? q;
}

/** "MP4 · hvc1" — the container, and the HEVC tag Apple players need. */
export function containerTag(output: string, codec: VideoCodec | null): string {
  const ext = extensionOf(output);
  const label = ext ? ext.toUpperCase() : "—";
  return codec === "hevc" && ["mp4", "mov", "m4v"].includes(ext) ? `${label} · hvc1` : label;
}

/** "오디오 복사" · "AAC 192k" · "Opus 128k"; null without audio. */
export function audioLabel(t: Messages, audio: AudioOut): string | null {
  switch (audio.kind) {
    case "none":
      return null;
    case "copy":
      return t.queueView.audioCopy;
    case "aac":
      return `AAC ${audio.bitrate}k`;
    case "opus":
      return `Opus ${audio.bitrate}k`;
  }
}

/** ["일괄 변환", "H.265", "VideoToolbox", "균형", "1280×720로 축소", "10비트 · HDR", "MP4 · hvc1", "오디오 복사"]. */
function convertTags(t: Messages, output: string, info: ConvertTask): string[] {
  const s = info.summary;
  const w = t.queueView;
  const tags = [t.kind.convert, codecLabel(s.codec), s.encoder, qualityLabel(t, info.quality)];
  if (s.scaled) tags.push(w.scaledTo(s.scaled[0], s.scaled[1]));
  if (s.tenBit) tags.push(s.hdr ? w.tenBitHdr : w.tenBit);
  tags.push(containerTag(output, s.codec));
  const audio = audioLabel(t, s.audio);
  if (audio) tags.push(audio);
  return tags;
}

/** Tags under a running item's name: ["구간 제거 · 정밀", "H.265", "VideoToolbox", "균형", "MP4 · hvc1"]. */
export function itemTags(t: Messages, item: QueueItem): string[] {
  const { info } = item;
  if (info.kind === "convert") return convertTags(t, item.output, info);
  const s = info.summary;
  const w = t.queueView;
  const edit = t.kind[info.edit];
  if (!s.codec) {
    const what = s.pieces > 1 ? `${edit} · ${w.lossless} · ${w.keptPieces(s.pieces)}` : `${edit} · ${w.lossless}`;
    return [what, w.streamCopy, containerTag(item.output, s.codec)];
  }
  const tags = [`${edit} · ${w.precise}`, codecLabel(s.codec)];
  if (s.encoder) tags.push(s.encoder);
  tags.push(qualityLabel(t, info.quality));
  if (s.picture) tags.push(w.picture(s.picture[0], s.picture[1]));
  if (s.masks > 0) tags.push(w.masks(s.masks));
  if (s.overlays > 0) tags.push(w.overlays(s.overlays));
  if (s.tenBit) tags.push(w.tenBit);
  tags.push(containerTag(item.output, s.codec));
  return tags;
}

/** "예상 1.5GB" (or "약 1.5GB" with `word`): the 예상 크기 a 일괄 변환 item took along from the convert screen; null without one. */
export function itemEstimate(t: Messages, item: QueueItem, word: "estimate" | "about" = "estimate"): string | null {
  const { info } = item;
  if (info.kind !== "convert" || info.estimatedBytes === null) return null;
  return t.queueView[word](formatBytes(info.estimatedBytes));
}

/** One line for a waiting item: "자르기 · H.265 · VideoToolbox · 균형" / "구간 제거 · 무손실 · 00:17.898". */
export function waitingLine(t: Messages, item: QueueItem): string {
  const { info } = item;
  if (info.kind === "convert") {
    const s = info.summary;
    const parts = [t.kind.convert, codecLabel(s.codec), s.encoder, qualityLabel(t, info.quality), formatTime(s.length)];
    const estimate = itemEstimate(t, item, "about");
    if (estimate) parts.push(estimate);
    return parts.join(" · ");
  }
  const s = info.summary;
  const parts: string[] = [t.kind[info.edit]];
  if (s.codec) parts.push(codecLabel(s.codec), ...(s.encoder ? [s.encoder] : []), qualityLabel(t, info.quality));
  else parts.push(t.queueView.lossless);
  parts.push(formatTime(s.length));
  return parts.join(" · ");
}

/** 6400 → "6.4초" · "6.4s", 64000 → "1분 4초" · "1m 4s", 4_000_000 → "1시간 6분" · "1h 6m". */
export function formatElapsed(t: Messages, ms: number): string {
  const u = t.queueView.elapsed;
  const sec = ms / 1000;
  if (sec < 10) return u.s(sec.toFixed(1));
  if (sec < 60) return u.s(String(Math.round(sec)));
  const total = Math.round(sec);
  const h = Math.floor(total / 3600);
  const m = Math.floor(total / 60) % 60;
  const s = total % 60;
  if (h > 0) return m > 0 ? u.hm(h, m) : u.h(h);
  return s > 0 ? u.ms(m, s) : u.m(m);
}

/** The line under a finished item's name. */
export function finishedLine(t: Messages, item: QueueItem): string {
  const { info } = item;
  const w = t.queueView;
  switch (item.state) {
    case "done": {
      const parts: string[] = [];
      if (info.kind === "convert") parts.push(t.kind.convert, codecLabel(info.summary.codec), qualityLabel(t, info.quality));
      else if (info.summary.codec) parts.push(t.kind[info.edit], codecLabel(info.summary.codec), qualityLabel(t, info.quality));
      else parts.push(t.kind[info.edit], w.lossless);
      if (item.elapsedMs !== null) parts.push(formatElapsed(t, item.elapsedMs));
      if (item.outputBytes !== null) parts.push(formatBytes(item.outputBytes));
      return parts.join(" · ");
    }
    case "failed":
      return w.failedLine(trimPeriod(say(t, item.error ?? { code: "ffmpegFailed" })));
    case "canceled":
      return item.startedAt === null ? w.canceledBeforeStart : w.canceledWhileRunning;
    default:
      return "";
  }
}

function trimPeriod(s: string): string {
  return s.replace(/[.\s]+$/, "");
}

/** Seconds left from FFmpeg's speed: (length − out_time) ÷ speed; null without a speed. */
export function remainingSeconds(item: QueueItem): number | null {
  const p = item.progress;
  if (item.state !== "running" || !p?.speed || p.speed <= 0) return null;
  return Math.max(0, (item.info.summary.length - p.outTime) / p.speed);
}

/** "62% · 경과 2:00 · 남은 시간 약 1:14 · 3.1× · 93 fps" pieces for a running item (`now`: Unix ms). */
export function runningStats(t: Messages, item: QueueItem, now: number): string[] {
  const p = item.progress;
  const w = t.queueView;
  const parts = [`${Math.floor(p?.percent ?? 0)}%`];
  if (item.startedAt !== null) parts.push(w.elapsedFor(formatClock(Math.max(0, (now - item.startedAt) / 1000))));
  const left = remainingSeconds(item);
  if (left !== null) parts.push(w.left(formatClock(left)));
  if (p?.speed) parts.push(w.speed(p.speed.toFixed(1)));
  if (p?.fps) parts.push(`${Math.round(p.fps)} fps`);
  return parts;
}

/** The FFmpeg log view: the command, then the latest progress (running) or the end of stderr. */
export function logText(t: Messages, item: QueueItem): string {
  const lines = item.commandLine.split("\n").map((l) => `$ ${l}`);
  const p = item.progress;
  if (item.state === "running" && p) {
    lines.push(`out_time=${formatTime(p.outTime)}`);
    if (p.fps) lines.push(`fps=${p.fps.toFixed(1)}`);
    if (p.speed) lines.push(`speed=${p.speed.toFixed(2)}x`);
    lines.push(`progress=${p.percent >= 100 ? "end" : "continue"}`);
  }
  if (item.note) lines.push(`# ${say(t, item.note)}`);
  if (item.log) lines.push(item.log);
  else if (item.state === "done") lines.push(`# ${t.queueView.cleanLog}`);
  return lines.join("\n");
}
