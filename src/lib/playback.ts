// What this OS webview says it can play. Feeds the Phase 0 playback matrix
// (WKWebView on macOS, WebView2 on Windows, WebKitGTK on Ubuntu).

import type { Messages } from "./i18n/ko.ts";

export interface CodecProbe {
  label: string;
  mime: string;
  result: "" | "maybe" | "probably";
}

const PROBES: Array<Omit<CodecProbe, "result">> = [
  { label: "H.264 · MP4", mime: 'video/mp4; codecs="avc1.640028"' },
  { label: "H.265 · MP4 (hvc1)", mime: 'video/mp4; codecs="hvc1.1.6.L150.B0"' },
  { label: "H.265 · MP4 (hev1)", mime: 'video/mp4; codecs="hev1.1.6.L150.B0"' },
  { label: "AV1 · MP4", mime: 'video/mp4; codecs="av01.0.08M.08"' },
  { label: "VP9 · WebM", mime: 'video/webm; codecs="vp9"' },
  { label: "H.264 · MKV", mime: 'video/x-matroska; codecs="avc1.640028"' },
  { label: "AAC", mime: 'audio/mp4; codecs="mp4a.40.2"' },
  { label: "Opus", mime: 'audio/webm; codecs="opus"' },
];

export function codecMatrix(): CodecProbe[] {
  const v = document.createElement("video");
  return PROBES.map((p) => ({ ...p, result: v.canPlayType(p.mime) as CodecProbe["result"] }));
}

/** requestVideoFrameCallback lets the editor know exactly which frame is on screen. */
export function hasFrameCallback(): boolean {
  return typeof HTMLVideoElement !== "undefined" && "requestVideoFrameCallback" in HTMLVideoElement.prototype;
}

export type PlaybackStatus =
  | { state: "idle" }
  | { state: "loading" }
  | { state: "ok"; width: number; height: number }
  | { state: "audio-only" }
  /** MediaError code (1–4); worded when shown (`mediaErrorText`). */
  | { state: "error"; code: number };

export function mediaErrorText(t: Messages, code: number): string {
  const p = t.playback;
  return p.errors[code as 1 | 2 | 3 | 4] ?? p.unknownError(code);
}

export function playbackLabel(t: Messages, s: PlaybackStatus): string {
  const p = t.playback;
  switch (s.state) {
    case "idle":
      return "—";
    case "loading":
      return p.loading;
    case "ok":
      return p.ok(s.width, s.height);
    case "audio-only":
      return p.audioOnly;
    case "error":
      return p.failed(mediaErrorText(t, s.code));
  }
}
