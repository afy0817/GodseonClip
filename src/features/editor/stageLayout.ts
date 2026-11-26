// Where the picture goes on the stage when 화면 효과 are on (pure; stageLayout.test.ts). The <video>
// element always shows the upright source (original or proxy — the proxy is scaled, so everything
// here is in source pixels and the element is stretched to the box); turns and flips are a CSS
// transform on it. Two views: 편집 화면 — the whole picture as seen, the crop box drawn over it — and
// 결과 — only what the export will contain, in its frame (black bars for 맞춤).

import { type EffectsPlan, type Orientation, pictureSize, type Rect, type Size, seenSize, toSeen } from "./effects.ts";

/** A box in stage pixels. */
export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** The largest box of `size`'s ratio in the middle of `area`. */
export function fitBox(area: Box, size: Size): Box {
  const [w, h] = size;
  if (!(w > 0 && h > 0 && area.width > 0 && area.height > 0)) return { ...area, width: 0, height: 0 };
  const k = Math.min(area.width / w, area.height / h);
  const width = w * k;
  const height = h * k;
  return { left: area.left + (area.width - width) / 2, top: area.top + (area.height - height) / 2, width, height };
}

/** `area` with `margin` taken off every side (room for the crop box's handles). */
export function inset(area: Box, margin: number): Box {
  return {
    left: area.left + margin,
    top: area.top + margin,
    width: Math.max(0, area.width - 2 * margin),
    height: Math.max(0, area.height - 2 * margin),
  };
}

export interface VideoCss {
  left: number;
  top: number;
  width: number;
  height: number;
  transform: string;
}

/**
 * CSS for the <video> (transform-origin at its center) so that the whole source picture, turned and
 * mirrored by `o`, fills `seen` — which may stretch it (늘림). The element is laid out upright: a
 * quarter turn makes its width the seen height.
 */
export function videoCss(seen: Box, o: Orientation): VideoCss {
  const turn = ((o.turn % 4) + 4) % 4;
  const [width, height] = turn % 2 === 1 ? [seen.height, seen.width] : [seen.width, seen.height];
  const cx = seen.left + seen.width / 2;
  const cy = seen.top + seen.height / 2;
  // Applied right to left: turn first, then mirror as seen.
  const parts: string[] = [];
  if (o.flipH || o.flipV) parts.push(`scale(${o.flipH ? -1 : 1}, ${o.flipV ? -1 : 1})`);
  if (turn) parts.push(`rotate(${turn * 90}deg)`);
  return { left: cx - width / 2, top: cy - height / 2, width, height, transform: parts.join(" ") || "none" };
}

/** 편집 화면: the seen picture in the stage, and pixels per seen pixel. */
export function editLayout(area: Box, src: Size, o: Orientation): { seen: Box; scale: number } {
  const seen = fitBox(area, seenSize(src, o.turn));
  const [w] = seenSize(src, o.turn);
  return { seen, scale: w > 0 ? seen.width / w : 0 };
}

/** A seen rectangle (pixels) on the stage, the seen picture drawn at `seen`. */
export function onStage(r: Rect, seen: Box, scale: number): Box {
  return { left: seen.left + r.x * scale, top: seen.top + r.y * scale, width: r.width * scale, height: r.height * scale };
}

/**
 * 결과: the result's frame in the stage (`frame`, black), the picture's place in it (`picture` — the
 * frame itself, or inside 맞춤's bars; clip the video there), and where the whole seen picture goes
 * (`seen`, relative to the stage) so that exactly the crop fills `picture` (stretched for 늘림).
 */
export function resultLayout(area: Box, src: Size, plan: EffectsPlan): { frame: Box; picture: Box; seen: Box } {
  const o: Orientation = plan;
  const frame = fitBox(area, plan.size);
  const k = plan.size[0] > 0 ? frame.width / plan.size[0] : 0;
  const [pw, ph] = pictureSize(plan, src);
  const picture = plan.pad ? { x: plan.pad.x, y: plan.pad.y, width: pw, height: ph } : { x: 0, y: 0, width: plan.size[0], height: plan.size[1] };
  const crop = toSeen(plan.crop ?? { x: 0, y: 0, width: src[0], height: src[1] }, src, o);
  const [W, H] = seenSize(src, o.turn);
  const kx = crop.width > 0 ? (picture.width * k) / crop.width : 0;
  const ky = crop.height > 0 ? (picture.height * k) / crop.height : 0;
  return {
    frame,
    picture: { left: frame.left + picture.x * k, top: frame.top + picture.y * k, width: picture.width * k, height: picture.height * k },
    seen: {
      left: frame.left + picture.x * k - crop.x * kx,
      top: frame.top + picture.y * k - crop.y * ky,
      width: W * kx,
      height: H * ky,
    },
  };
}
