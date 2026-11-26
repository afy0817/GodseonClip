// What a lossless trim of [start, end) will produce, as the UI explains it before exporting.
// Rust decides the real cut (command::plan_lossless_trim); these rules mirror it for display.

import { isMovFamily, type ProbeResult } from "../../lib/ipc";
import type { Messages } from "../../lib/i18n/ko";
import { keyframeAtOrBefore } from "../../lib/time";

/** Shortest range worth exporting (one millisecond, the precision of the fields). */
const MIN_LENGTH = 0.001;

export interface LosslessPreview {
  /** Keyframe at or before the start (the stream copy begins there), or null if unknown. */
  snapped: number | null;
  /** start − snapped. */
  pulledBy: number;
  /** MP4 / MOV: playback starts exactly at `start` (edit list). */
  exactStart: boolean;
  /** Where the result's first visible frame comes from. */
  visibleStart: number;
  /** Playing length of the result. */
  length: number;
}

/** Why the range cannot be exported, else null. */
export function trimProblem(t: Messages, start: number, end: number, duration: number): string | null {
  if (!(duration > 0)) return t.trimRules.noDuration;
  if (start >= duration) return t.trimRules.startPastEnd;
  if (end - start < MIN_LENGTH) return t.trimRules.endBeforeStart;
  return null;
}

export function losslessPreview(probe: ProbeResult, start: number, end: number): LosslessPreview {
  const snapped = keyframeAtOrBefore(probe.keyframes, start);
  const exactStart = isMovFamily(probe.info.formatName);
  // MP4 / MOV play from the requested start; other containers show the frames from the keyframe.
  const visibleStart = exactStart || snapped === null ? start : snapped;
  return {
    snapped,
    pulledBy: snapped === null ? 0 : start - snapped,
    exactStart,
    visibleStart,
    length: Math.max(0, end - visibleStart),
  };
}
