// 프레임레이트 · GIF 크기: TS mirrors of command::plan_frame_rate and command::plan_gif_scale, so the
// export panel and the convert screen can say what the result will be (Rust plans the real thing,
// the same cases are in frameRate.test.ts and the Rust tests). No runtime imports but `.ts` modules:
// Node runs this file in `npm run test:ts`.

import type { VideoStream } from "../../lib/ipc";
import type { Messages } from "../../lib/i18n/ko.ts";
import { type Size, shortSideSize } from "./effects.ts";

/** command::MAX_FRAME_RATE */
export const MAX_FRAME_RATE = 120;
/** command::GIF_MAX_FRAME_RATE: a GIF frame lasts whole 1/100 s, and browsers slow down frames under 2/100 s. */
export const GIF_MAX_FRAME_RATE = 50;
/** command::GIF_FALLBACK_FRAME_RATE: a GIF at 원본 from a source whose rate is unknown. */
export const GIF_FALLBACK_FRAME_RATE = 15;
/** command::GIF_MIN_SHORT */
export const GIF_MIN_SHORT = 16;

/** The export panel's 프레임레이트 buttons after 원본 (then 직접). */
export const VIDEO_RATES = [60, 30, 24];
export const GIF_RATES = [25, 15, 10];
/** GIF 크기 buttons after 원본 (짧은 변, never larger). */
export const GIF_SHORT_SIDES = [480, 360, 240];
/** A GIF starts at 15 fps and 360p: small enough to share, smooth enough to watch. */
export const DEFAULT_GIF_RATE = 15;
export const DEFAULT_GIF_SHORT = 360;

/** command::source_rate: the average (what a variable-rate phone video holds), else the stream's rate. */
export function sourceRate(v: VideoStream): number | null {
  if (v.avgFps !== null && Number.isFinite(v.avgFps) && v.avgFps > 0) return v.avgFps;
  if (v.fps !== null && Number.isFinite(v.fps) && v.fps > 0) return v.fps;
  return null;
}

/**
 * command::plan_frame_rate (for a cap in range): the result's fps, or null when every frame stays —
 * a cap at or above the source's rate never adds frames. A GIF always gets one: the cap, else the
 * source's rate, at most 50.
 */
export function resultRate(v: VideoStream, cap: number | null, gif: boolean): number | null {
  const source = sourceRate(v);
  if (gif) {
    const limit = cap ?? GIF_MAX_FRAME_RATE;
    return source === null ? (cap ?? GIF_FALLBACK_FRAME_RATE) : Math.min(source, limit);
  }
  if (cap === null) return null;
  if (source !== null && cap >= source - 0.01) return null;
  return cap;
}

/** A preset button does something: it would bring the rate down. */
export function ratePossible(v: VideoStream, rate: number, gif: boolean): boolean {
  const source = sourceRate(v);
  if (source === null) return true;
  return rate < (gif ? Math.min(source, GIF_MAX_FRAME_RATE) : source) - 0.01;
}

/** 30 → "30", 29.97002997 → "29.97", 12.5 → "12.5". */
export function rateText(rate: number): string {
  return String(Number(rate.toFixed(2)));
}

/** A typed frame rate: whole, 1 … 120 (a GIF: … 50). */
export function clampRate(n: number, gif: boolean): number {
  const max = gif ? GIF_MAX_FRAME_RATE : MAX_FRAME_RATE;
  return Math.max(1, Math.min(max, Math.round(n)));
}

/** command::plan_gif_scale: the GIF made smaller so its short side is `short` (never larger); null: as it is. */
export function gifScale(picture: Size, short: number | null): Size | null {
  return short === null ? null : shortSideSize(picture[0], picture[1], short);
}

/** A preset button of the export panel: pressed, or off because it would change nothing. */
export interface PresetButton {
  value: number;
  pressed: boolean;
  disabled: boolean;
}

/** The 프레임레이트 row (ExportSection's `FrameRateGroup`): 원본 · presets · 직접. */
export interface RateButtons {
  sourcePressed: boolean;
  presets: PresetButton[];
  /** 직접: while typing (`typing`), or for a rate no preset has. Only one button is ever pressed. */
  customPressed: boolean;
  /** What 직접 starts at and its field shows before anything is typed: the result's rate, else the source's, else 30 — in range. */
  start: number;
}

/**
 * The 프레임레이트 buttons for the chosen `rate` (a video's or a GIF's; null: 원본). A preset at or
 * above what the result already has is off, as 출력 크기's presets that would enlarge are, and a
 * chosen rate that does nothing shows as 원본.
 */
export function rateButtons(v: VideoStream, rate: number | null, gif: boolean, typing: boolean): RateButtons {
  const presets = gif ? GIF_RATES : VIDEO_RATES;
  const custom = typing || (rate !== null && !presets.includes(rate));
  return {
    sourcePressed: !custom && (rate === null || !ratePossible(v, rate, gif)),
    presets: presets.map((value) => {
      const possible = ratePossible(v, value, gif);
      return { value, pressed: possible && !custom && rate === value, disabled: !possible };
    }),
    customPressed: custom,
    start: clampRate(resultRate(v, rate, gif) ?? sourceRate(v) ?? 30, gif),
  };
}

/** The line under the 프레임레이트 buttons: what the result gets, from what the source has. */
export function rateNote(t: Messages, v: VideoStream, rate: number | null, gif: boolean): string {
  const w = t.exportPanel;
  const source = sourceRate(v);
  const sourceText = source !== null ? rateText(source) : null;
  const out = resultRate(v, rate, gif);
  if (gif) {
    if (rate === null && source !== null && source > GIF_MAX_FRAME_RATE) return w.rateGifMax(rateText(source));
    return w.rateGif(rateText(out ?? GIF_FALLBACK_FRAME_RATE), sourceText);
  }
  return out === null ? w.rateKept(sourceText) : w.rateDown(rateText(out), sourceText);
}

/** The GIF 크기 row (ExportSection's `GifSizeGroup`) for the picture after the 화면 효과. */
export interface GifSizeButtons {
  /** The picture's short side: a preset at or above it is off. */
  side: number;
  sourcePressed: boolean;
  presets: PresetButton[];
  /** The GIF's width × height. */
  size: Size;
}

export function gifSizeButtons(shown: Size, short: number | null): GifSizeButtons {
  const side = Math.min(shown[0], shown[1]);
  const possible = (n: number) => side > n;
  return {
    side,
    sourcePressed: short === null || !possible(short),
    presets: GIF_SHORT_SIDES.map((value) => ({ value, pressed: possible(value) && short === value, disabled: !possible(value) })),
    size: gifScale(shown, short) ?? shown,
  };
}
