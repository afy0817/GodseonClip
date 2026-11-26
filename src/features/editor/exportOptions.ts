// What the export panel's choices (방식 · 비디오 코덱 · 품질) will produce, as the UI explains it
// before exporting. Rust decides the real thing (command::plan_precise_trim); these mirror it.

import type { EncoderChoice, ExportEncoders, ExportMode, MediaInfo, OutputCodec, Quality, VideoCodec } from "../../lib/ipc";
import type { Messages } from "../../lib/i18n/ko.ts";
import { say } from "../../lib/i18n/text.ts";
// `.ts` so Node runs this module in `npm run test:ts` (queueView.test.ts).
import { codecLabel } from "./format.ts";

/** 자르기 (one kept range) or 구간 제거 (ranges to remove). */
export type EditorMode = "trim" | "cut";

export interface ExportChoice {
  mode: ExportMode;
  codec: OutputCodec;
  quality: Quality;
}

export const DEFAULT_EXPORT: ExportChoice = { mode: "lossless", codec: "source", quality: "balanced" };

export const QUALITY_VALUES: Quality[] = ["high", "balanced", "small"];

/** 고화질 · 균형 · 작은 용량, with what each is for. */
export function qualities(t: Messages): { value: Quality; label: string; use: string }[] {
  return QUALITY_VALUES.map((value) => ({ value, ...t.quality[value] }));
}

/** Same as WHOLE_FILE_SLACK in command.rs. */
const WHOLE_FILE_SLACK = 0.001;

export function isWholeFile(start: number, end: number, duration: number): boolean {
  return start <= WHOLE_FILE_SLACK && end >= duration - WHOLE_FILE_SLACK;
}

/** The codec a precise export writes; "원본 유지" keeps H.264 · H.265 · AV1, anything else becomes H.264. */
export function targetCodec(codec: OutputCodec, sourceCodec: string | undefined): VideoCodec {
  if (codec !== "source") return codec;
  return sourceCodec === "hevc" ? "hevc" : sourceCodec === "av1" ? "av1" : "h264";
}

export function encoderFor(encoders: ExportEncoders | null, codec: VideoCodec): EncoderChoice | null {
  return encoders ? encoders[codec] : null;
}

/** Why no precise export to `codec` is possible (Rust's Msg::NoEncoder, the same words). */
export function missingEncoderText(t: Messages, codec: VideoCodec): string {
  return say(t, { code: "noEncoder", codec });
}

function extensionOf(fileName: string): string {
  const dot = fileName.lastIndexOf(".");
  return dot > 0 ? fileName.slice(dot + 1) : "";
}

/**
 * `{suffix}.{ext}` of the result, as `command::plan_precise_trim` · `plan_precise_cut` and the
 * lossless exports name it (lossless: `_trim` / `_cut` + same container; 구간 제거 정밀: `_cut`,
 * `_cut_h265`, `_cut_av1`).
 */
export function outputEnding(info: MediaInfo, choice: ExportChoice, whole: boolean, editor: EditorMode = "trim"): { suffix: string; ext: string } {
  const sourceExt = extensionOf(info.fileName);
  if (choice.mode === "lossless") return { suffix: editor === "cut" ? "_cut" : "_trim", ext: sourceExt || "mkv" };
  const codec = targetCodec(choice.codec, info.video?.codec);
  const lower = sourceExt.toLowerCase();
  const keeps =
    codec === "av1" ? ["mp4", "mkv"].includes(lower) : ["mp4", "m4v", "mov", "mkv"].includes(lower);
  const ext = choice.codec === "source" && keeps ? sourceExt : "mp4";
  const codecSuffix = choice.codec === "hevc" ? "_h265" : choice.codec === "av1" ? "_av1" : "";
  const suffix = editor === "cut" ? `_cut${codecSuffix}` : whole ? codecSuffix || "_trim" : `_trim${codecSuffix}`;
  return { suffix, ext };
}

/** "클립_trim_h265.mp4" — the name when nothing is in the way (Rust adds " (1)" otherwise). */
export function predictedExportName(info: MediaInfo, choice: ExportChoice, whole: boolean, editor: EditorMode = "trim"): string {
  const { suffix, ext } = outputEnding(info, choice, whole, editor);
  const dot = info.fileName.lastIndexOf(".");
  const stem = dot > 0 ? info.fileName.slice(0, dot) : info.fileName;
  return `${stem}${suffix}.${ext}`;
}

/** Same rule as probe::is_high_bit_depth / VideoStream::wants_ten_bit. */
export function wantsTenBit(info: MediaInfo): boolean {
  const v = info.video;
  if (!v) return false;
  if (v.hdr) return true;
  const f = (v.pixFmt ?? "").replace(/(le|be)$/, "");
  return ["p10", "p12", "p14", "p16"].some((d) => f.includes(d)) || ["p010", "p016", "p210", "p216", "p410", "p416", "gray1"].some((d) => f.startsWith(d));
}

/**
 * One line under the codec buttons: which encoder runs and what happens to the picture, e.g.
 * "인코더 자동 — VideoToolbox (하드웨어) · 실패하면 libx265로 다시 · 10비트 · HDR 유지".
 */
export function encoderLine(t: Messages, info: MediaInfo, choice: ExportChoice, encoders: ExportEncoders | null): string | null {
  if (choice.mode !== "precise" || !encoders) return null;
  const w = t.encoderLine;
  const codec = targetCodec(choice.codec, info.video?.codec);
  const enc = encoderFor(encoders, codec);
  if (!enc) return missingEncoderText(t, codec);
  const parts: string[] = [];
  if (enc.hardware) {
    parts.push(w.autoHardware(enc.label));
    if (enc.fallbackLabel) parts.push(w.fallback(enc.fallbackLabel));
  } else {
    parts.push(w.software(enc.label));
    if (codec === "av1") parts.push(w.av1Slow);
  }
  const source = info.video?.codec;
  if (choice.codec === "source" && codec === "h264" && source && source !== "h264") {
    parts.push(w.toH264(codecLabel(source)));
  }
  if (codec !== "h264" && wantsTenBit(info)) parts.push(info.video?.hdr ? w.tenBitHdr : w.tenBit);
  return parts.join(" · ");
}
