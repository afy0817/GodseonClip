// Display helpers for the editor. Pure functions, no IPC.

import type { MediaInfo, VideoStream } from "../../lib/ipc";
import type { Messages } from "../../lib/i18n/ko.ts";
// `.ts` so Node runs this module in `npm run test:ts` (format.test.ts).
import { formatBytes } from "../../lib/time.ts";

const CODEC_LABELS: Record<string, string> = {
  h264: "H.264",
  hevc: "H.265",
  av1: "AV1",
  gif: "GIF",
  vp9: "VP9",
  vp8: "VP8",
  prores: "ProRes",
  mpeg4: "MPEG-4",
  mpeg2video: "MPEG-2",
  mjpeg: "MJPEG",
  dnxhd: "DNxHD",
  wmv3: "WMV",
  vc1: "VC-1",
};

export function codecLabel(codec: string): string {
  return CODEC_LABELS[codec] ?? codec.toUpperCase();
}

/** "MKV" from the file extension; ffprobe's format name ("matroska,webm") when there is none. */
export function containerLabel(fileName: string, formatName: string): string {
  const dot = fileName.lastIndexOf(".");
  return dot > 0 ? fileName.slice(dot + 1).toUpperCase() : formatName.split(",")[0].toUpperCase();
}

/** 600 → "10:00", 3725 → "1:02:05". For the ruler and the header. */
export function formatClock(sec: number): string {
  const s = Math.max(0, Math.round(sec));
  const h = Math.floor(s / 3600);
  const m = Math.floor(s / 60) % 60;
  const r = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${r}` : `${m}:${r}`;
}

/** "H.264 · 3840×2160 · 30fps · 10:00 · 3.9GB" */
export function mediaSummary(t: Messages, info: MediaInfo): string {
  const v = info.video;
  const parts: string[] = [];
  if (v) {
    parts.push(codecLabel(v.codec), `${v.width}×${v.height}`);
    if (v.fps) parts.push(`${v.fps.toFixed(3).replace(/\.?0+$/, "")}fps`);
  } else {
    parts.push(info.audio.length ? t.common.audio(codecLabel(info.audio[0].codec)) : t.common.noTracks);
  }
  parts.push(formatClock(info.duration));
  if (info.sizeBytes != null) parts.push(formatBytes(info.sizeBytes));
  return parts.join(" · ");
}

/** Stream copy keeps the bit rate, so the size scales with the kept length. */
export function estimateCopySize(info: MediaInfo, length: number): string | null {
  if (info.sizeBytes == null || info.duration <= 0) return null;
  return formatBytes(Math.round(info.sizeBytes * Math.min(1, Math.max(0, length / info.duration))));
}

/** Seconds of one frame (1/30 when unknown). */
export function frameDuration(v: VideoStream | null): number {
  return v?.fps && v.fps > 0 ? 1 / v.fps : 1 / 30;
}

/** Index of the last of the sorted `times` at or before `t` (−1 when `t` comes before them all). */
function frameIndexAt(times: number[], t: number): number {
  let lo = 0;
  let hi = times.length - 1;
  let found = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (times[mid] <= t + 1e-6) {
      found = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  return found;
}

/**
 * Start of the frame shown at `t`. Variable frame rate: the last of the stream's own frame times
 * at or before `t`. Otherwise a constant rate whose frames sit on the video stream's start time.
 * A variable rate without frame times (or unknown fps): `t` unchanged.
 */
export function frameStart(t: number, v: VideoStream | null): number {
  const times = v?.frameTimes;
  if (times && times.length > 0) return Math.max(0, times[Math.max(0, frameIndexAt(times, t))]);
  if (!v?.fps || v.fps <= 0 || v.vfrSuspect) return t;
  const frame = 1 / v.fps;
  const phase = (((v.startTime ?? 0) % frame) + frame) % frame;
  const k = Math.floor((t - phase) / frame + 1e-3);
  return Math.max(0, phase + k * frame);
}

/**
 * Start of the frame `n` frames after (n < 0: before) the frame shown at `t` — ← → and the
 * one-frame gaps of the handles. On a variable rate it walks the frame times and stops at the
 * first and last frame; otherwise it steps by the frame duration.
 */
export function stepFrame(t: number, n: number, v: VideoStream | null): number {
  const times = v?.frameTimes;
  if (times && times.length > 0) {
    const i = Math.max(0, frameIndexAt(times, t)) + n;
    return Math.max(0, times[Math.min(times.length - 1, Math.max(0, i))]);
  }
  return frameStart(t, v) + n * frameDuration(v);
}

/**
 * A time typed into the IN / OUT fields → the start of the frame shown at it, like I / O and the
 * handles. The fields show frame starts rounded to the millisecond (30 fps 245.1666… s reads
 * "04:05.167"), so a typed value may sit up to half a millisecond before the frame it names.
 */
export function typedFrameStart(t: number, v: VideoStream | null): number {
  if (!v?.frameTimes?.length && (!v?.fps || v.fps <= 0 || v.vfrSuspect)) return t;
  return frameStart(t + 0.0005, v);
}

const RULER_STEPS = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200];

/** Ruler label times: a round step giving at most `maxLabels` labels, always including 0. */
export function rulerTicks(duration: number, maxLabels: number): number[] {
  if (!(duration > 0)) return [0];
  const step = RULER_STEPS.find((s) => duration / s <= Math.max(1, maxLabels - 1)) ?? Math.ceil(duration / maxLabels);
  const ticks: number[] = [];
  for (let t = 0; t < duration - step * 0.35; t += step) ticks.push(t);
  return ticks;
}
