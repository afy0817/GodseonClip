// 구간 제거: the removed ranges the editor keeps, and what stays of the file. Pure functions with
// no runtime imports, so `npm run test:ts` runs them under plain Node (cutList.test.ts).
//
// `normalize` · `keepFromRemoved` · `total` are src-tauri/src/plan.rs, with the same test cases.
// Rust works the kept ranges out again from the removed ones (the UI only sends what to remove);
// these are for the timeline, the list and the length summary.

/** plan::Range */
export interface Range {
  start: number;
  end: number;
}

/** Touching ranges closer than this are merged (plan.rs EPS). */
export const EPS = 1e-6;

export function total(ranges: readonly Range[]): number {
  return ranges.reduce((s, r) => s + (r.end - r.start), 0);
}

/** Clamps to [0, duration], drops empty or invalid ranges, sorts and merges overlapping or touching ones. */
export function normalize(ranges: readonly Range[], duration: number): Range[] {
  const v = ranges
    .filter((r) => Number.isFinite(r.start) && Number.isFinite(r.end))
    .map((r) => ({ start: Math.max(0, r.start), end: Math.min(duration, r.end) }))
    .filter((r) => r.end - r.start > EPS)
    .sort((a, b) => a.start - b.start);
  const out: Range[] = [];
  for (const r of v) {
    const last = out[out.length - 1];
    if (last && r.start <= last.end + EPS) last.end = Math.max(last.end, r.end);
    else out.push(r);
  }
  return out;
}

/** Ranges to keep after removing `removed`; kept pieces shorter than `minLen` (about one frame) are dropped. */
export function keepFromRemoved(removed: readonly Range[], duration: number, minLen: number): Range[] {
  // A gap must be longer than EPS too, or a cut starting at 0 would leave an empty piece when minLen is 0.
  const longEnough = (gap: number) => gap > EPS && gap >= minLen;
  const keep: Range[] = [];
  let cursor = 0;
  for (const r of normalize(removed, duration)) {
    if (longEnough(r.start - cursor)) keep.push({ start: cursor, end: r.start });
    cursor = r.end;
  }
  if (longEnough(duration - cursor)) keep.push({ start: cursor, end: duration });
  return keep;
}

/** command::min_keep_len — just under one frame (1/30 s when the rate is unknown, like `frameDuration`). */
export function minKeepLength(fps: number | null | undefined): number {
  return (fps && fps > 0 ? 1 / fps : 1 / 30) * 0.999;
}

/** Index of the range holding `t` (start inclusive, end exclusive), or null. */
export function rangeIndexAt(ranges: readonly Range[], t: number): number | null {
  const i = ranges.findIndex((r) => t >= r.start - EPS && t < r.end - EPS);
  return i >= 0 ? i : null;
}

/**
 * The list with `r` added (merged with what it overlaps or touches) and the index of the range that
 * now holds it, to select.
 */
export function addRange(ranges: readonly Range[], r: Range, duration: number): { ranges: Range[]; index: number | null } {
  const merged = normalize([...ranges, r], duration);
  return { ranges: merged, index: rangeIndexAt(merged, (Math.max(0, r.start) + Math.min(duration, r.end)) / 2) };
}

/**
 * After an edge was dragged: merge what the moved range now overlaps, and the index of the range
 * holding it (the selection follows it).
 */
export function settle(ranges: readonly Range[], index: number, duration: number): { ranges: Range[]; index: number | null } {
  const moved = ranges[index];
  const merged = normalize(ranges, duration);
  return { ranges: merged, index: moved ? rangeIndexAt(merged, (moved.start + moved.end) / 2) : null };
}

export type Edge = "start" | "end";

/**
 * Which removed range's edge a press at `x` grabs (all in px along the track): the nearest edge
 * within `hit`. When a range is narrower than 2 px its edges sit on each other and `edge` is
 * "either" (the first move decides), unless the press is clearly outside it.
 */
export function grabEdge(x: number, px: readonly Range[], hit: number): { index: number; edge: Edge | "either" } | null {
  let found: { index: number; edge: Edge | "either" } | null = null;
  let nearest = hit;
  for (let index = 0; index < px.length; index++) {
    const r = px[index];
    const dStart = Math.abs(x - r.start);
    const dEnd = Math.abs(x - r.end);
    const d = Math.min(dStart, dEnd);
    if (d > nearest || (found && d === nearest)) continue;
    nearest = d;
    let edge: Edge | "either" = dStart <= dEnd ? "start" : "end";
    if (r.end - r.start < 2) edge = x < r.start - 1 ? "start" : x > r.end + 1 ? "end" : "either";
    found = { index, edge };
  }
  return found;
}
