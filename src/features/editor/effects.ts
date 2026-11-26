// 화면 효과 (Phase 4 step 1): the editor's crop · turn · flip · size. `planEffects` is the TS mirror of
// effects::plan in Rust (same numbers, same test cases in effects.test.ts) so the panel can say what
// the result will be; Rust plans the real export. The rest is the geometry of the stage's drawing
// layer: source ⇄ seen rectangles (the picture turned and mirrored as it is shown), the crop box's
// drags and typed numbers. No runtime imports: Node runs this file in `npm run test:ts`.
//
// Spaces: "source" = pixels of the upright source (FFmpeg's autorotate and the webview both show a
// phone video upright); the crop box is kept there. "Seen" = the source turned clockwise `turn`
// quarters, then mirrored — what the user looks at and types numbers in.

import type { Crop, Effects, Fit, Mask, MaskKind, Msg, OutputSize, Overlay, OverlaySource, VideoStream } from "../../lib/ipc";

export const MIN_SIDE = 64;
export const MAX_SIDE = 8192;
export const MAX_MASKS = 20;
export const MIN_MASK_SIDE = 16;
export const MAX_OVERLAYS = 4;
export const MIN_OVERLAY_SIDE = 16;

export const NO_EFFECTS: Effects = { crop: null, turn: 0, flipH: false, flipV: false, size: { kind: "source" }, masks: [], overlays: [] };

/** The 비율 presets as [w, h] as seen; null is 자유. */
export const RATIOS: [number, number][] = [
  [16, 9],
  [9, 16],
  [1, 1],
  [4, 3],
];

/** 1080p · 720p · 480p. */
export const SHORT_SIDES = [1080, 720, 480];

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type Size = [number, number];

export interface Pad {
  width: number;
  height: number;
  x: number;
  y: number;
}

/** effects::MaskFilter — 모자이크 shrinks the box to `cells`, 블러 blurs it by `sigma` pixels. */
export type MaskFilter = { kind: "mosaic"; cells: Size } | { kind: "blur"; sigma: number };

/** effects::MaskPlan without the times (the UI keeps them on the mask). */
export interface MaskPlan {
  rect: Rect;
  filter: MaskFilter;
}

/** effects::EffectsPlan. */
export interface EffectsPlan {
  masks: MaskPlan[];
  /** PiP boxes (even); what shows when is Rust's business. */
  overlays: { rect: Rect }[];
  crop: Rect | null;
  turn: number;
  flipH: boolean;
  flipV: boolean;
  scale: Size | null;
  pad: Pad | null;
  size: Size;
}

/** How the picture is turned and mirrored. */
export interface Orientation {
  turn: number;
  flipH: boolean;
  flipV: boolean;
}

export function isNone(e: Effects): boolean {
  return e.crop === null && e.turn % 4 === 0 && !e.flipH && !e.flipV && e.size.kind === "source" && e.masks.length === 0 && e.overlays.length === 0;
}

/** effects::mosaic_cell — cell size in pixels: 0.5 % → 6 % of the picture's short side. */
export function mosaicCell(strength: number, short: number): number {
  const s = Math.max(1, Math.min(100, strength));
  return Math.max(2, Math.round(short * (0.005 + ((s - 1) / 99) * 0.055)));
}

/** effects::blur_sigma — blur sigma in pixels: 0.2 % → 3 % of the picture's short side. */
export function blurSigma(strength: number, short: number): number {
  const s = Math.max(1, Math.min(100, strength));
  return Math.max(1, Math.round(short * (0.002 + ((s - 1) / 99) * 0.028)));
}

/** What `kind` at `strength` does to a `width` × `height` box on a picture with this short side. */
export function maskFilter(kind: MaskKind, strength: number, width: number, height: number, short: number): MaskFilter {
  if (kind === "blur") return { kind: "blur", sigma: blurSigma(strength, short) };
  const cell = mosaicCell(strength, short);
  return { kind: "mosaic", cells: [Math.max(1, Math.round(width / cell)), Math.max(1, Math.round(height / cell))] };
}

/** A new box to hide: a square of a quarter of the short side in the middle, the whole video, 모자이크 50. */
export function newMask(src: Size, duration: number): Mask {
  const side = Math.max(MIN_MASK_SIDE, evenFloor(Math.min(src[0], src[1]) * 0.25));
  return {
    x: evenFloor((src[0] - side) / 2),
    y: evenFloor((src[1] - side) / 2),
    width: side,
    height: side,
    start: 0,
    end: duration,
    kind: "mosaic",
    strength: 50,
  };
}

/** The box shows at `now` (its start in, its end out). */
export function maskShows(m: { start: number; end: number }, now: number): boolean {
  return now >= m.start - 1e-6 && now < m.end - 1e-6;
}

/** Where a w × h box sits in a seen picture for 왼쪽 위 · 오른쪽 위 · 가운데 · 왼쪽 아래 · 오른쪽 아래: 3 % of the width from the edges. */
function cornerAt(seen: Size, w: number, h: number, corner: 0 | 1 | 2 | 3 | 4): [number, number] {
  const m = evenFloor(seen[0] * 0.03);
  const right = evenFloor(seen[0] - w - m);
  const bottom = evenFloor(seen[1] - h - m);
  if (corner === 0) return [m, m];
  if (corner === 1) return [right, m];
  if (corner === 3) return [m, bottom];
  if (corner === 4) return [right, bottom];
  return [evenFloor((seen[0] - w) / 2), evenFloor((seen[1] - h) / 2)];
}

/**
 * A PiP box `r` (source pixels) moved to a position preset of the picture as seen (turned and
 * mirrored): a PiP stands upright on the result, so its corners are the screen's.
 */
export function cornerBox(src: Size, o: Orientation, r: Rect, corner: 0 | 1 | 2 | 3 | 4): Rect {
  const seen = toSeen(evenRect(r, src), src, o);
  const [x, y] = cornerAt(seenSize(src, o.turn), seen.width, seen.height, corner);
  return evenRect(fromSeen({ x, y, width: seen.width, height: seen.height }, src, o), src);
}

/** The preset `r` sits at as seen, if any. */
function cornerOf(src: Size, o: Orientation, r: Rect): 0 | 1 | 2 | 3 | 4 | null {
  const box = evenRect(r, src);
  for (const c of [0, 1, 2, 3, 4] as const) {
    const at = cornerBox(src, o, box, c);
    if (at.x === box.x && at.y === box.y) return c;
  }
  return null;
}

/**
 * A new PiP for `source` on a picture of `src` seen as `o`: 30 % of the seen width (an image no
 * bigger than itself), its own ratio upright, bottom right as seen. A video from the playhead for
 * its length; an image the whole video.
 */
export function newOverlay(src: Size, o: Orientation, source: OverlaySource, start: number, duration: number): Overlay {
  const seen = seenSize(src, o.turn);
  const ratio = source.width > 0 && source.height > 0 ? source.height / source.width : 9 / 16;
  let w = evenFloor(seen[0] * 0.3);
  if (source.kind === "image" && source.width > 0) w = Math.min(w, evenFloor(source.width));
  w = Math.max(MIN_OVERLAY_SIDE, w);
  let h = Math.max(MIN_OVERLAY_SIDE, evenFloor(w * ratio));
  if (h > seen[1]) {
    h = evenFloor(seen[1] * 0.3);
    w = Math.max(MIN_OVERLAY_SIDE, evenFloor(h / ratio));
  }
  const [x, y] = cornerAt(seen, w, h, 4);
  const box = evenRect(fromSeen({ x, y, width: w, height: h }, src, o), src);
  const video = source.kind === "video";
  const s = video ? Math.min(start, Math.max(0, duration - 0.001)) : 0;
  const end = video ? Math.min(duration, s + (source.duration ?? duration)) : duration;
  return { path: source.path, ...box, start: s, end: end > s ? end : duration, offset: 0, opacity: 100, audio: false, volume: 100 };
}

/**
 * Where a PiP box goes when the picture is turned or mirrored from `from` to `to`: it stays where
 * it was on the screen — at the same preset, else around the same spot — the same size as seen
 * (smaller if it no longer fits), upright. (모자이크 boxes cover the picture and turn with it.)
 */
export function overlayAfterTurn(r: Rect, src: Size, from: Orientation, to: Orientation): Rect {
  const a = toSeen(evenRect(r, src), src, from);
  const [fw, fh] = seenSize(src, from.turn);
  const [tw, th] = seenSize(src, to.turn);
  const k = Math.min(1, tw / a.width, th / a.height);
  const w = Math.max(MIN_OVERLAY_SIDE, evenFloor(a.width * k));
  const h = Math.max(MIN_OVERLAY_SIDE, evenFloor(a.height * k));
  const corner = cornerOf(src, from, r);
  let x: number;
  let y: number;
  if (corner !== null) [x, y] = cornerAt([tw, th], w, h, corner);
  else {
    x = Math.max(0, Math.min(tw - w, ((a.x + a.width / 2) / fw) * tw - w / 2));
    y = Math.max(0, Math.min(th - h, ((a.y + a.height / 2) / fh) * th - h / 2));
  }
  return evenRect(fromSeen({ x: evenFloor(x), y: evenFloor(y), width: w, height: h }, src, to), src);
}

/** The picture is turned or mirrored differently as seen. */
export function sameOrientation(a: Orientation, b: Orientation): boolean {
  return ((a.turn % 4) + 4) % 4 === ((b.turn % 4) + 4) % 4 && a.flipH === b.flipH && a.flipV === b.flipV;
}

/** When a PiP shows at `now`: inside its span and, for a video, before the video runs out (끝나면 사라짐). */
export function overlayShows(o: Overlay, source: OverlaySource | undefined, now: number): boolean {
  if (now < o.start - 1e-6 || now >= o.end - 1e-6) return false;
  if (source?.kind === "video" && source.duration !== null) return o.offset + (now - o.start) < source.duration - 1e-3;
  return true;
}

/** Width × height as players show them (VideoStream::upright_size). */
export function uprightSize(v: VideoStream): Size {
  const quarter = v.rotation !== null && ((Math.round(v.rotation / 90) % 2) + 2) % 2 === 1;
  return quarter ? [v.height, v.width] : [v.width, v.height];
}

/** `num / den` to the nearest even number, halves up (what `scale=-2:…` rounds to). */
export function evenRound(num: number, den: number): number {
  return Math.floor((num + den) / (2 * den)) * 2;
}

function evenFloor(n: number): number {
  return Math.floor(n / 2) * 2;
}

/** The size as seen after `turn` quarter turns. */
export function seenSize(size: Size, turn: number): Size {
  return turn % 2 === 1 ? [size[1], size[0]] : size;
}

/** effects::short_side_size: the short side down to `short` (never up); null when it is not above it. */
export function shortSideSize(w: number, h: number, short: number): Size | null {
  if (w === 0 || h === 0 || Math.min(w, h) <= short) return null;
  const other = (long: number, small: number) => evenRound(long * short, small);
  return w >= h ? [other(w, h), short] : [short, other(h, w)];
}

function shortSideAtRatio(w: number, h: number, short: number, ratio: [number, number] | null): Size | null {
  const size = shortSideSize(w, h, short);
  if (!size || !ratio || ratio[0] <= 0 || ratio[1] <= 0) return size;
  const wanted = ratio[0] / ratio[1];
  if (Math.abs((w / h - wanted) / wanted) > 0.01) return size;
  const other = evenRound(short * Math.max(...ratio), Math.min(...ratio));
  return w >= h ? [other, short] : [short, other];
}

function containSize(cw: number, ch: number, w: number, h: number): Size {
  const clamp = (n: number, max: number) => Math.max(2, Math.min(max, n));
  return w * ch <= h * cw ? [w, clamp(evenRound(w * ch, cw), h)] : [clamp(evenRound(h * cw, ch), w), h];
}

function coverCrop(crop: Rect, w: number, h: number, quarter: boolean): Rect {
  const [cw, ch] = quarter ? [crop.height, crop.width] : [crop.width, crop.height];
  const clamp = (n: number, max: number) => Math.max(2, Math.min(max, n));
  const [sw, sh] = cw * h > w * ch ? [clamp(evenRound(ch * w, h), cw), ch] : [cw, clamp(evenRound(cw * h, w), ch)];
  const [nw, nh] = quarter ? [sh, sw] : [sw, sh];
  return { x: crop.x + evenFloor((crop.width - nw) / 2), y: crop.y + evenFloor((crop.height - nh) / 2), width: nw, height: nh };
}

const sameRect = (a: Rect, b: Rect) => a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;

export type Planned = { plan: EffectsPlan | null; error: null } | { plan: null; error: Msg };

/** effects::plan — what the filters will do, or why Rust would refuse it. */
export function planEffects(v: VideoStream, e: Effects): Planned {
  const ok = (plan: EffectsPlan | null): Planned => ({ plan, error: null });
  const fail = (error: Msg): Planned => ({ plan: null, error });
  if (isNone(e)) return ok(null);
  const [sw, sh] = uprightSize(v);
  if (sw === 0 || sh === 0) return fail({ code: "pictureSizeUnknown" });
  const whole: Rect = { x: 0, y: 0, width: sw, height: sh };
  if (e.masks.length > MAX_MASKS) return fail({ code: "tooManyMasks", max: MAX_MASKS });
  const masks: MaskPlan[] = [];
  for (const m of e.masks) {
    const rect = { x: evenFloor(m.x), y: evenFloor(m.y), width: evenFloor(m.width), height: evenFloor(m.height) };
    if (rect.width < MIN_MASK_SIDE || rect.height < MIN_MASK_SIDE) return fail({ code: "maskTooSmall", min: MIN_MASK_SIDE });
    if (rect.x + rect.width > sw || rect.y + rect.height > sh) return fail({ code: "maskOutsidePicture", width: sw, height: sh });
    if (!(Number.isFinite(m.start) && Number.isFinite(m.end) && m.start >= 0 && m.end > m.start)) return fail({ code: "maskTimeInvalid" });
    masks.push({ rect, filter: maskFilter(m.kind, m.strength, rect.width, rect.height, Math.min(sw, sh)) });
  }
  if (e.overlays.length > MAX_OVERLAYS) return fail({ code: "tooManyOverlays", max: MAX_OVERLAYS });
  const overlays: { rect: Rect }[] = [];
  for (const o of e.overlays) {
    const rect = { x: evenFloor(o.x), y: evenFloor(o.y), width: evenFloor(o.width), height: evenFloor(o.height) };
    if (rect.width < MIN_OVERLAY_SIDE || rect.height < MIN_OVERLAY_SIDE) return fail({ code: "overlayTooSmall", min: MIN_OVERLAY_SIDE });
    if (rect.x + rect.width > sw || rect.y + rect.height > sh) return fail({ code: "overlayOutsidePicture", width: sw, height: sh });
    const timesOk = Number.isFinite(o.start) && Number.isFinite(o.end) && Number.isFinite(o.offset) && o.start >= 0 && o.end > o.start && o.offset >= 0;
    if (!timesOk) return fail({ code: "overlayTimeInvalid" });
    overlays.push({ rect });
  }
  let crop = whole;
  if (e.crop) {
    const c = e.crop;
    crop = { x: evenFloor(c.x), y: evenFloor(c.y), width: evenFloor(c.width), height: evenFloor(c.height) };
    if (crop.width < MIN_SIDE || crop.height < MIN_SIDE) return fail({ code: "cropTooSmall", min: MIN_SIDE });
    if (crop.x + crop.width > sw || crop.y + crop.height > sh) return fail({ code: "cropOutsidePicture", width: sw, height: sh });
  }
  const quarter = e.turn % 2 === 1;
  const seen = (r: Rect): Size => (quarter ? [r.height, r.width] : [r.width, r.height]);
  const [cw, ch] = seen(crop);
  const side = (n: number) => evenFloor(n);
  const inRange = (n: number) => n >= MIN_SIDE && n <= MAX_SIDE;
  let scale: Size | null = null;
  let pad: Pad | null = null;
  let size: Size = [cw, ch];
  const s = e.size;
  if (s.kind === "short") {
    const short = side(s.short);
    if (!inRange(short)) return fail({ code: "outputSizeInvalid", min: MIN_SIDE, max: MAX_SIDE });
    const at = shortSideAtRatio(cw, ch, short, e.crop?.ratio ?? null);
    if (at) [scale, size] = [at, at];
  } else if (s.kind === "exact") {
    const [w, h] = [side(s.width), side(s.height)];
    if (!inRange(w) || !inRange(h)) return fail({ code: "outputSizeInvalid", min: MIN_SIDE, max: MAX_SIDE });
    size = [w, h];
    if (s.fit === "stretch") scale = [w, h];
    else if (s.fit === "cover") {
      crop = coverCrop(crop, w, h, quarter);
      scale = [w, h];
    } else {
      const inner = containSize(cw, ch, w, h);
      scale = inner;
      if (inner[0] !== w || inner[1] !== h) pad = { width: w, height: h, x: evenFloor((w - inner[0]) / 2), y: evenFloor((h - inner[1]) / 2) };
    }
  }
  const after = seen(crop);
  if (scale && scale[0] === after[0] && scale[1] === after[1]) scale = null;
  const half = e.turn % 4 === 2;
  const turn = half ? 0 : e.turn % 4;
  const flipH = half ? !e.flipH : e.flipH;
  const flipV = half ? !e.flipV : e.flipV;
  const cropped = sameRect(crop, whole) ? null : crop;
  if (masks.length === 0 && overlays.length === 0 && !cropped && turn === 0 && !flipH && !flipV && !scale && !pad) return ok(null);
  return ok({ masks, overlays, crop: cropped, turn, flipH, flipV, scale, pad, size });
}

/**
 * The picture's size in the result before 맞춤's bars: the scaled size, or the (cropped, turned)
 * picture's own when nothing is scaled — 맞춤 can add bars without scaling.
 */
export function pictureSize(plan: EffectsPlan, src: Size): Size {
  if (plan.scale) return plan.scale;
  const c = plan.crop ?? { x: 0, y: 0, width: src[0], height: src[1] };
  return seenSize([c.width, c.height], plan.turn);
}

// ------------------------------------------------------------------ source ⇄ seen

/** A source rectangle as seen: turned clockwise `turn` quarters in the `src` picture, then mirrored. */
export function toSeen(r: Rect, src: Size, o: Orientation): Rect {
  let [W, H] = src;
  let { x, y, width, height } = r;
  for (let i = 0; i < ((o.turn % 4) + 4) % 4; i++) {
    [x, y, width, height] = [H - y - height, x, height, width];
    [W, H] = [H, W];
  }
  if (o.flipH) x = W - x - width;
  if (o.flipV) y = H - y - height;
  return { x, y, width, height };
}

/** The source rectangle that is seen as `r` (the inverse of toSeen). */
export function fromSeen(r: Rect, src: Size, o: Orientation): Rect {
  const turn = ((o.turn % 4) + 4) % 4;
  let [W, H] = seenSize(src, turn);
  let { x, y, width, height } = r;
  if (o.flipH) x = W - x - width;
  if (o.flipV) y = H - y - height;
  for (let i = 0; i < turn; i++) {
    [x, y, width, height] = [y, W - x - width, height, width];
    [W, H] = [H, W];
  }
  return { x, y, width, height };
}

/** Whole even pixels inside the `src` picture (4:2:0 chroma): what the crop box keeps. */
export function evenRect(r: Rect, src: Size): Rect {
  const [W, H] = [evenFloor(src[0]), evenFloor(src[1])];
  const width = Math.max(2, Math.min(W, evenFloor(Math.round(r.width))));
  const height = Math.max(2, Math.min(H, evenFloor(Math.round(r.height))));
  const x = Math.max(0, Math.min(W - width, evenFloor(Math.round(r.x))));
  const y = Math.max(0, Math.min(H - height, evenFloor(Math.round(r.y))));
  return { x, y, width, height };
}

/** The largest box of `ratio` ([w, h] as seen) in the middle of the picture, in source pixels. */
export function ratioBox(src: Size, o: Orientation, ratio: [number, number]): Rect {
  const [W, H] = seenSize(src, o.turn);
  const [a, b] = ratio;
  const [w, h] = W * b > H * a ? [Math.min(evenFloor(W), evenRound(H * a, b)), evenFloor(H)] : [evenFloor(W), Math.min(evenFloor(H), evenRound(W * b, a))];
  const seen = { x: evenFloor((W - w) / 2), y: evenFloor((H - h) / 2), width: w, height: h };
  return evenRect(fromSeen(seen, src, o), src);
}

/** The box crop starts with: 80 % of the picture, in the middle (자유). */
export function defaultBox(src: Size): Rect {
  const [W, H] = src;
  const width = evenFloor(W * 0.8);
  const height = evenFloor(H * 0.8);
  return evenRect({ x: (W - width) / 2, y: (H - height) / 2, width, height }, src);
}

export function cropOf(r: Rect, ratio: [number, number] | null): Crop {
  return { x: r.x, y: r.y, width: r.width, height: r.height, ratio };
}

// ------------------------------------------------------------------ dragging and typing

/** What is dragged: the box, an edge or a corner (compass points as seen). */
export type Handle = "move" | "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";

export const HANDLES: Exclude<Handle, "move">[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

function clamp(n: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, n));
}

/**
 * The box as seen after dragging `handle` by (dx, dy) seen pixels from where it was (`start`),
 * inside the `frame` (seen size), at least `min` on each side. With a `ratio` ([w, h]) a corner
 * keeps the opposite corner in place and the ratio, an edge keeps the box centered across it.
 * Not rounded: the caller makes it even in the source (evenRect).
 */
export function dragBox(start: Rect, handle: Handle, dx: number, dy: number, frame: Size, ratio: [number, number] | null, min = MIN_SIDE): Rect {
  const [W, H] = frame;
  if (handle === "move") {
    return { ...start, x: clamp(start.x + dx, 0, W - start.width), y: clamp(start.y + dy, 0, H - start.height) };
  }
  const west = handle.includes("w");
  const east = handle.includes("e");
  const north = handle.includes("n");
  const south = handle.includes("s");
  const right = start.x + start.width;
  const bottom = start.y + start.height;
  if (!ratio) {
    const x1 = west ? clamp(start.x + dx, 0, right - min) : start.x;
    const x2 = east ? clamp(right + dx, start.x + min, W) : right;
    const y1 = north ? clamp(start.y + dy, 0, bottom - min) : start.y;
    const y2 = south ? clamp(bottom + dy, start.y + min, H) : bottom;
    return { x: x1, y: y1, width: x2 - x1, height: y2 - y1 };
  }
  const r = ratio[0] / ratio[1];
  const minW = Math.max(min, min * r);
  if ((west || east) && (north || south)) {
    // Corner: the opposite corner stays; the box follows whichever way the pointer went further.
    const ax = west ? right : start.x;
    const ay = north ? bottom : start.y;
    const px = (west ? start.x : right) + dx;
    const py = (north ? start.y : bottom) + dy;
    const roomW = west ? ax : W - ax;
    const roomH = north ? ay : H - ay;
    const wantW = west ? ax - px : px - ax;
    const wantH = north ? ay - py : py - ay;
    const w = clamp(Math.max(wantW, wantH * r), minW, Math.min(roomW, roomH * r));
    const h = w / r;
    return { x: west ? ax - w : ax, y: north ? ay - h : ay, width: w, height: h };
  }
  if (west || east) {
    const cy = start.y + start.height / 2;
    const roomW = west ? right : W - start.x;
    let w = west ? start.width - dx : start.width + dx;
    w = clamp(w, minW, Math.min(roomW, 2 * Math.min(cy, H - cy) * r));
    const h = w / r;
    return { x: west ? right - w : start.x, y: cy - h / 2, width: w, height: h };
  }
  const cx = start.x + start.width / 2;
  const roomH = north ? bottom : H - start.y;
  let h = north ? start.height - dy : start.height + dy;
  h = clamp(h, minW / r, Math.min(roomH, (2 * Math.min(cx, W - cx)) / r));
  const w = h * r;
  return { x: cx - w / 2, y: north ? bottom - h : start.y, width: w, height: h };
}

export type BoxField = "x" | "y" | "width" | "height";

/**
 * A number typed into X · Y · 가로 · 세로 (seen pixels). Position moves the box, kept inside; a
 * side keeps the top-left corner, the other side follows a locked ratio, and both shrink to fit.
 */
export function typedBox(box: Rect, field: BoxField, value: number, frame: Size, ratio: [number, number] | null, min = MIN_SIDE): Rect {
  const [W, H] = frame;
  if (!Number.isFinite(value)) return box;
  if (field === "x") return { ...box, x: clamp(value, 0, W - box.width) };
  if (field === "y") return { ...box, y: clamp(value, 0, H - box.height) };
  const r = ratio ? ratio[0] / ratio[1] : null;
  let w = field === "width" ? value : r ? value * r : box.width;
  let h = field === "height" ? value : r ? value / r : box.height;
  w = Math.max(min, w);
  h = Math.max(min, h);
  // Too big for the picture from this corner: move the corner back, then shrink.
  const fitW = Math.min(1, W / w);
  const fitH = Math.min(1, H / h);
  if (r) {
    const k = Math.min(fitW, fitH);
    w *= k;
    h *= k;
  } else {
    w *= fitW;
    h *= fitH;
  }
  return { x: clamp(box.x, 0, W - w), y: clamp(box.y, 0, H - h), width: w, height: h };
}

// ------------------------------------------------------------------ the panel's choices

/** The box's seen size (the picture's own when there is no crop). */
export function seenCropSize(src: Size, e: Effects): Size {
  const r = e.crop ?? { x: 0, y: 0, width: src[0], height: src[1] };
  return seenSize([r.width, r.height], e.turn);
}

/** A 1080p · 720p · 480p preset can be picked: it would make the picture smaller. */
export function presetPossible(src: Size, e: Effects, short: number): boolean {
  const [w, h] = seenCropSize(src, e);
  return shortSideSize(w, h, short) !== null;
}

/** Turned a quarter: a locked ratio is seen the other way round ([9, 16] ⇄ [16, 9]). */
export function turned(e: Effects, by: 1 | -1): Effects {
  const turn = (((e.turn + by) % 4) + 4) % 4;
  const crop = e.crop && e.crop.ratio ? { ...e.crop, ratio: [e.crop.ratio[1], e.crop.ratio[0]] as [number, number] } : e.crop;
  return { ...e, turn, crop };
}

/**
 * Effects as they are kept: a 1080p · 720p · 480p preset the box has become too small for goes back
 * to 원본 (it would do nothing), so 무손실 is not blocked by an effect that does nothing.
 */
export function settled(v: VideoStream, e: Effects): Effects {
  if (e.size.kind === "short" && !presetPossible(uprightSize(v), e, e.size.short)) return { ...e, size: { kind: "source" } };
  return e;
}

export function sameRatio(a: [number, number] | null, b: [number, number] | null): boolean {
  if (!a || !b) return a === b;
  return a[0] * b[1] === a[1] * b[0];
}

export function exactSize(width: number, height: number, fit: Fit): OutputSize {
  return { kind: "exact", width, height, fit };
}
