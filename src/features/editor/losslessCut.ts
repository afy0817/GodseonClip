// What 구간 제거 · 무손실 will produce, as the editor explains it before exporting. Rust decides the
// real thing (command::plan_lossless_cut); these rules mirror it, with the same test cases
// (losslessCut.test.ts). Imports carry `.ts` so `npm run test:ts` runs this under plain Node.

import type { Msg } from "../../lib/ipc";
import { keyframeAtOrBefore } from "../../lib/time.ts";
import { EPS, type Range } from "./cutList.ts";

/** Same as WHOLE_FILE_SLACK in command.rs. */
const WHOLE_FILE_SLACK = 0.001;

export interface LosslessCutPiece {
  /** The kept range (after the joins below). */
  keep: Range;
  /** Where its first frame comes from: the keyframe at or before its start (its start itself for a single MP4 piece). */
  visibleStart: number;
}

export interface LosslessCutPreview {
  pieces: LosslessCutPiece[];
  /** Removed ranges stream copy cannot take out: no keyframe after their start before their end. */
  keptRemovals: Range[];
  /** A single piece in MP4 / MOV: the edit list starts it exactly, as a trim. */
  exactStart: boolean;
  /** Playing length of the result (without the frames a B-frame source brings past OUT). */
  length: number;
  /** Why a lossless export is not possible (Rust's own codes), else null. */
  problem: Msg | null;
}

/**
 * `keep` (cutList.keepFromRemoved) copied piece by piece from their keyframes. A piece whose keyframe
 * is not past the end of the piece before would bring back what was removed between them: the two
 * become one and that removal stays.
 */
export function losslessCutPreview(keep: readonly Range[], keyframes: readonly number[], duration: number, mov: boolean): LosslessCutPreview {
  const none: LosslessCutPreview = { pieces: [], keptRemovals: [], exactStart: false, length: 0, problem: null };
  if (keep.length === 0) return none;
  if (keyframes.length === 0) {
    return { ...none, problem: { code: "cutNeedsKeyframes" } };
  }
  const pieces: LosslessCutPiece[] = [];
  const keptRemovals: Range[] = [];
  for (const k of keep) {
    const from = keyframeAtOrBefore(keyframes as number[], k.start) ?? 0;
    const prev = pieces[pieces.length - 1];
    if (prev && from <= prev.keep.end + EPS) {
      keptRemovals.push({ start: prev.keep.end, end: k.start });
      prev.keep = { start: prev.keep.start, end: k.end };
    } else {
      pieces.push({ keep: { ...k }, visibleStart: from });
    }
  }
  const exactStart = pieces.length === 1 && mov;
  if (exactStart) pieces[0].visibleStart = pieces[0].keep.start;
  const only = pieces.length === 1 ? pieces[0] : null;
  const problem: Msg | null =
    only && only.visibleStart <= WHOLE_FILE_SLACK && only.keep.end >= duration - WHOLE_FILE_SLACK
      ? { code: "nothingRemovableLossless" }
      : null;
  const length = pieces.reduce((s, p) => s + Math.max(0, p.keep.end - p.visibleStart), 0);
  return { pieces, keptRemovals, exactStart, length, problem };
}

/**
 * For each removed range (as the editor lists them), what a lossless export does with it: `kept` —
 * it stays; else `back` — how much of its end comes back because the piece after it starts at the
 * keyframe before (0 when it ends on a keyframe or at the file's end).
 */
export function removalOutcomes(removed: readonly Range[], preview: LosslessCutPreview): { kept: boolean; back: number }[] {
  const overlaps = (a: Range, b: Range) => a.start < b.end - EPS && b.start < a.end - EPS;
  return removed.map((r) => {
    if (preview.keptRemovals.some((k) => overlaps(k, r))) return { kept: true, back: 0 };
    // The piece that starts where this range ends.
    const next = preview.pieces.find((p) => Math.abs(p.keep.start - r.end) < 1e-3);
    return { kept: false, back: next && !preview.exactStart ? Math.max(0, next.keep.start - Math.max(next.visibleStart, r.start)) : 0 };
  });
}
