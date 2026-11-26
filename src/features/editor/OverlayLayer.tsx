import { convertFileSrc } from "@tauri-apps/api/core";
import { type CSSProperties, useEffect, useRef } from "react";
import { useT } from "../../lib/i18n/I18n";
import type { Overlay, OverlaySource } from "../../lib/ipc";
import { CropBox } from "./CropBox";
import { evenRect, fromSeen, MIN_OVERLAY_SIDE, overlayShows, type Rect, type Size, seenSize, toSeen } from "./effects";
import type { MaskFrame } from "./MaskLayer";
import type { Box } from "./stageLayout";

function boxOf(r: Rect, f: MaskFrame): Box {
  return { left: f.seen.left + r.x * f.kx, top: f.seen.top + r.y * f.ky, width: r.width * f.kx, height: r.height * f.ky };
}

/**
 * The PiP files as they will look, over the picture (inside its wrapper, both views): an image as
 * an <img>, a video as a muted <video> kept at `offset + (now − start)` and playing with the source.
 * Upright in their box as seen (the picture under them may be turned or mirrored), at their opacity.
 */
export function OverlayPreviews({
  overlays,
  sources,
  now,
  playing,
  src,
  frame,
}: {
  overlays: Overlay[];
  sources: Record<string, OverlaySource>;
  now: number;
  playing: boolean;
  src: Size;
  frame: MaskFrame;
}) {
  return overlays.map((o, i) => {
    const source = sources[o.path];
    if (!overlayShows(o, source, now)) return null;
    // Upright on the turned picture, like the result (command.rs turns each file back).
    const box = boxOf(toSeen(evenRect(o, src), src, frame.o), frame);
    const css = { ...box, opacity: o.opacity / 100 };
    if (source?.kind === "video") {
      return <OverlayVideo key={`${i}-${o.path}`} path={o.path} at={o.offset + (now - o.start)} playing={playing} style={css} />;
    }
    return <img key={`${i}-${o.path}`} className="ed-overlay-media" src={convertFileSrc(o.path)} alt="" draggable={false} style={css} />;
  });
}

function OverlayVideo({ path, at, playing, style }: { path: string; at: number; playing: boolean; style: CSSProperties }) {
  const ref = useRef<HTMLVideoElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    // Follow the source: jump when off by more than a few frames, play and pause with it.
    if (Math.abs(el.currentTime - at) > 0.2 || !playing) el.currentTime = Math.max(0, at);
    if (playing && el.paused) void el.play().catch(() => {});
    if (!playing && !el.paused) el.pause();
  }, [at, playing]);
  return <video ref={ref} className="ed-overlay-media" src={convertFileSrc(path)} muted playsInline preload="auto" style={style} />;
}

/**
 * The PiP boxes in 편집 화면 (over the stage): the selected one with corner handles (its ratio kept),
 * the others a click away from being selected; dashed when not showing at `now`.
 */
export function OverlayBoxes({
  overlays,
  sources,
  selected,
  now,
  src,
  seen,
  scale,
  o,
  onSelect,
  onChange,
}: {
  overlays: Overlay[];
  sources: Record<string, OverlaySource>;
  selected: number | null;
  now: number;
  src: Size;
  seen: Box;
  scale: number;
  o: { turn: number; flipH: boolean; flipV: boolean };
  onSelect: (index: number) => void;
  onChange: (index: number, rect: Rect) => void;
}) {
  const w = useT().overlays;
  const frame: MaskFrame = { seen, kx: scale, ky: scale, o };
  return overlays.map((ov, i) => {
    const r = toSeen(evenRect(ov, src), src, o);
    const box = boxOf(r, frame);
    const source = sources[ov.path];
    const label = `${source?.kind === "image" ? "▣" : "▶"} ${i + 1} ${source?.fileName ?? ""}`;
    const off = !overlayShows(ov, source, now);
    if (i === selected) {
      return (
        <CropBox
          key={i}
          variant={off ? "overlay off" : "overlay"}
          box={box}
          seen={r}
          frame={seenSize(src, o.turn)}
          scale={scale}
          ratio={source && source.width > 0 && source.height > 0 ? [source.width, source.height] : [r.width, r.height]}
          min={MIN_OVERLAY_SIDE}
          label={label}
          ariaLabel={w.box(i + 1)}
          onDrag={(next) => onChange(i, evenRect(fromSeen(next, src, o), src))}
        />
      );
    }
    return (
      <button
        key={i}
        type="button"
        className={off ? "ed-overlay-box off" : "ed-overlay-box"}
        style={box}
        aria-label={w.box(i + 1)}
        onPointerDown={(e) => {
          e.stopPropagation();
          onSelect(i);
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <span className="ed-crop-label mono">{label}</span>
      </button>
    );
  });
}
