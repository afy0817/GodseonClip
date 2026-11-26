import { type PointerEvent, useRef } from "react";
import { useT } from "../../lib/i18n/I18n";
import { dragBox, HANDLES, type Handle, MIN_SIDE, type Rect, type Size } from "./effects";
import type { Box } from "./stageLayout";

interface Props {
  /** The box on the stage (stage pixels). */
  box: Box;
  /** The same box in seen pixels, and the seen picture's size. */
  seen: Rect;
  frame: Size;
  /** Stage pixels per seen pixel. */
  scale: number;
  ratio: [number, number] | null;
  /** "1216 × 2160 · 9:16" (source pixels as seen, even). */
  label: string;
  /** The box as seen while dragging (not rounded). */
  onDrag: (seen: Rect) => void;
  /** "mask" (모자이크 · 블러 box: orange, nothing dimmed; "mask off" outside its times). Default: the crop box. */
  variant?: string;
  /** Smallest side in seen pixels (default MIN_SIDE). */
  min?: number;
  ariaLabel?: string;
}

/**
 * The crop box over the picture (canvas Editor.dc.html): drag inside to move it, a corner or an edge
 * to resize it (a locked ratio is kept); outside it the picture is dimmed. Pointer moves are turned
 * into seen pixels and handed up; the editor keeps the box even in source pixels.
 */
export function CropBox({ box, seen, frame, scale, ratio, label, onDrag, variant, min = MIN_SIDE, ariaLabel }: Props) {
  const w = useT().effects;
  const drag = useRef<{ id: number; x: number; y: number; start: Rect; handle: Handle } | null>(null);

  const down = (handle: Handle) => (e: PointerEvent<HTMLElement>) => {
    if (e.button !== 0 || !(scale > 0)) return;
    e.preventDefault();
    e.stopPropagation();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    drag.current = { id: e.pointerId, x: e.clientX, y: e.clientY, start: seen, handle };
  };
  const move = (e: PointerEvent<HTMLElement>) => {
    const d = drag.current;
    if (!d || d.id !== e.pointerId) return;
    onDrag(dragBox(d.start, d.handle, (e.clientX - d.x) / scale, (e.clientY - d.y) / scale, frame, ratio, min));
  };
  const up = (e: PointerEvent<HTMLElement>) => {
    if (drag.current?.id === e.pointerId) drag.current = null;
  };
  const events = { onPointerMove: move, onPointerUp: up, onPointerCancel: up };

  return (
    <div
      className={variant ? `ed-crop ${variant}` : "ed-crop"}
      role="group"
      aria-label={ariaLabel ?? w.box}
      style={{ left: box.left, top: box.top, width: box.width, height: box.height }}
      onPointerDown={down("move")}
      onClick={(e) => e.stopPropagation()}
      {...events}
    >
      <span className="ed-crop-thirds" aria-hidden="true" />
      <span className="ed-crop-label mono">{label}</span>
      {HANDLES.map((h) => (
        <span key={h} className={`ed-crop-handle ${h}`} aria-hidden="true" onPointerDown={down(h)} {...events} />
      ))}
    </div>
  );
}
