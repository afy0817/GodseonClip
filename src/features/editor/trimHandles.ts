// Dragging the IN / OUT handles on the timeline. Pure functions, no DOM.

import type { VideoStream } from "../../lib/ipc";
import { frameStart, stepFrame } from "./format.ts";

export type TrimEdge = "start" | "end";

export interface TrimRange {
  start: number;
  end: number;
}

/**
 * Which handle a press at `x` grabs (all in pixels along the track): the nearer one within `hit`,
 * "either" when the two handles sit on top of each other and the press is between them
 * (the first move decides), or null for a plain seek.
 */
export function grabHandle(x: number, inX: number, outX: number, hit: number): TrimEdge | "either" | null {
  const dIn = Math.abs(x - inX);
  const dOut = Math.abs(x - outX);
  if (Math.min(dIn, dOut) > hit) return null;
  if (outX - inX < 2) {
    if (x < inX - 1) return "start";
    if (x > outX + 1) return "end";
    return "either";
  }
  return dIn <= dOut ? "start" : "end";
}

/**
 * Where dragging `edge` to `t` puts it. Both edges land on frame starts (so the frame on screen is
 * the cut), keep at least one frame between them, and OUT dragged to the end of the file is the end itself.
 */
export function dragHandleTime(
  edge: TrimEdge,
  t: number,
  range: TrimRange,
  duration: number,
  v: VideoStream | null,
): number {
  if (edge === "start") {
    // Last frame start before OUT.
    const f = frameStart(range.end, v);
    const max = f < range.end - 1e-6 ? f : stepFrame(f, -1, v);
    return Math.max(0, Math.min(frameStart(Math.max(0, t), v), max));
  }
  if (t >= duration) return duration;
  // First frame start after IN.
  const min = stepFrame(range.start, 1, v);
  return Math.min(duration, Math.max(frameStart(Math.max(0, t), v), min));
}
