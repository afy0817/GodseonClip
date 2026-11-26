import { type RefObject, useEffect, useRef } from "react";
import { useT } from "../../lib/i18n/I18n";
import type { Mask } from "../../lib/ipc";
import { CropBox } from "./CropBox";
import { evenRect, fromSeen, MIN_MASK_SIDE, maskFilter, maskShows, type Orientation, type Rect, type Size, seenSize, toSeen } from "./effects";
import { type Box, videoCss } from "./stageLayout";

/** Where the whole seen picture is drawn (relative to the layer) and its pixels per seen pixel. */
export interface MaskFrame {
  seen: Box;
  kx: number;
  ky: number;
  o: Orientation;
}

function boxOf(r: Rect, f: MaskFrame): Box {
  return { left: f.seen.left + r.x * f.kx, top: f.seen.top + r.y * f.ky, width: r.width * f.kx, height: r.height * f.ky };
}

/**
 * The masks as they will look, over the picture (inside its clipping wrapper, both views): 블러 is a
 * `backdrop-filter` blur of what is under the box, 모자이크 a canvas of the box's cells drawn from the
 * playing video and scaled up without smoothing. Only the masks that show at `now`. An approximation:
 * the result is FFmpeg's.
 */
export function MaskPreviews({ masks, now, src, frame, videoRef }: { masks: Mask[]; now: number; src: Size; frame: MaskFrame; videoRef: RefObject<HTMLVideoElement | null> }) {
  const short = Math.min(src[0], src[1]);
  return masks.map((m, i) => {
    if (!maskShows(m, now)) return null;
    const rect = evenRect(m, src);
    const box = boxOf(toSeen(rect, src, frame.o), frame);
    const filter = maskFilter(m.kind, m.strength, rect.width, rect.height, short);
    if (filter.kind === "blur") {
      const px = filter.sigma * (frame.kx + frame.ky) * 0.5;
      const blur = `blur(${px.toFixed(2)}px)`;
      return <div key={i} className="ed-mask-blur" style={{ ...box, backdropFilter: blur, WebkitBackdropFilter: blur }} />;
    }
    return <MosaicCanvas key={i} rect={rect} cells={filter.cells} src={src} box={box} o={frame.o} videoRef={videoRef} />;
  });
}

/**
 * One 모자이크 box: a canvas of `cells` pixels filled from the box of the video frame (the element's
 * own pixels: the original, or the smaller proxy — both upright), laid out like the video (turned and
 * mirrored by CSS) and scaled up pixelated. Redrawn every animation frame while shown.
 */
function MosaicCanvas({ rect, cells, src, box, o, videoRef }: { rect: Rect; cells: Size; src: Size; box: Box; o: Orientation; videoRef: RefObject<HTMLVideoElement | null> }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const { x, y, width, height } = rect;
  useEffect(() => {
    let raf = 0;
    const draw = () => {
      const canvas = ref.current;
      const video = videoRef.current;
      const ctx = canvas?.getContext("2d");
      if (canvas && video && ctx && video.videoWidth > 0) {
        const k = video.videoWidth / src[0];
        ctx.imageSmoothingEnabled = true;
        ctx.imageSmoothingQuality = "high";
        try {
          ctx.drawImage(video, x * k, y * k, width * k, height * k, 0, 0, canvas.width, canvas.height);
        } catch {
          // Not decodable yet: the next frame.
        }
      }
      raf = requestAnimationFrame(draw);
    };
    draw();
    return () => cancelAnimationFrame(raf);
  }, [x, y, width, height, src, videoRef]);
  const css = videoCss(box, o);
  return <canvas ref={ref} className="ed-mask-mosaic" width={cells[0]} height={cells[1]} style={css} />;
}

/**
 * The masks' boxes in 편집 화면 (over the stage, above the crop box): the selected one with handles
 * to move and resize it, the others a click away from being selected; a box that does not show at
 * `now` is dashed.
 */
export function MaskBoxes({
  masks,
  selected,
  now,
  src,
  seen,
  scale,
  o,
  onSelect,
  onChange,
}: {
  masks: Mask[];
  selected: number | null;
  now: number;
  src: Size;
  seen: Box;
  scale: number;
  o: Orientation;
  onSelect: (index: number) => void;
  onChange: (index: number, rect: Rect) => void;
}) {
  const w = useT().masks;
  const frame: MaskFrame = { seen, kx: scale, ky: scale, o };
  const kind = (m: Mask) => (m.kind === "mosaic" ? w.mosaic : w.blur);
  return masks.map((m, i) => {
    const r = toSeen(evenRect(m, src), src, o);
    const box = boxOf(r, frame);
    const label = `#${i + 1} ${w.tag(kind(m), m.strength)}`;
    const off = !maskShows(m, now);
    if (i === selected) {
      return (
        <CropBox
          key={i}
          variant={off ? "mask off" : "mask"}
          box={box}
          seen={r}
          frame={seenSize(src, o.turn)}
          scale={scale}
          ratio={null}
          min={MIN_MASK_SIDE}
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
        className={off ? "ed-mask-box off" : "ed-mask-box"}
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
