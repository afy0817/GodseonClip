import {
  memo,
  type PointerEvent,
  type RefObject,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useT } from "../../lib/i18n/I18n";
import { say } from "../../lib/i18n/text";
import type { MediaInfo, ThumbnailStrip } from "../../lib/ipc";
import { formatTime } from "../../lib/time";
import type { Marks } from "./CutSection";
import { grabEdge, type Range, rangeIndexAt } from "./cutList";
import type { EditorMode } from "./exportOptions";
import { formatClock, rulerTicks } from "./format";
import { dragHandleTime, grabHandle, type TrimEdge, type TrimRange } from "./trimHandles";
import { useTimelineData } from "./useTimelineData";
import { WaveCanvas } from "./WaveCanvas";

interface Props {
  info: MediaInfo;
  /** Keyframe times from ffprobe (sorted). */
  keyframes: number[];
  /** Playhead, original's timeline. */
  now: number;
  /** Kept range to draw; null while the IN / OUT fields are invalid. */
  range: TrimRange | null;
  onSeek: (t: number) => void;
  /** An IN / OUT handle is being dragged to `t` (a frame start, inside the other edge). */
  onTrimDrag: (edge: TrimEdge, t: number) => void;
  mode: EditorMode;
  /** 구간 제거: the ranges to remove (sorted, merged), the selected one and the I · O marks. */
  cuts: Range[];
  selected: number | null;
  marks: Marks;
  /** A press inside a removed range selects it; elsewhere clears the selection. */
  onCutSelect: (index: number | null) => void;
  /** An edge of removed range `index` is being dragged to `t` (a frame start, a frame from the other edge). */
  onCutDrag: (index: number, edge: TrimEdge, t: number) => void;
  /** The drag ended: merge what the range now overlaps. */
  onCutDragEnd: (index: number) => void;
  /** 모자이크 · 블러: each box's times (the 가리기 row), the selected one, picking one, moving an end. */
  masks: TrimRange[];
  maskSel: number | null;
  onMaskSelect: (index: number) => void;
  onMaskTime: (index: number, edge: TrimEdge, t: number) => void;
  /** PiP: each file's times (the 올리기 row), as for the masks; `image` marks a picture (▣) from a video (▶). */
  overlays: (TrimRange & { image?: boolean })[];
  overlaySel: number | null;
  onOverlaySelect: (index: number) => void;
  onOverlayTime: (index: number, edge: TrimEdge, t: number) => void;
}

const THUMB_HEIGHT = 60;
const MAX_ZOOM = 10;
/** A press this close to a handle (CSS px) drags it instead of seeking. */
const HANDLE_HIT_PX = 8;

type Gesture =
  | { kind: "scrub" }
  | { kind: "handle"; edge: TrimEdge | null; x0: number }
  | { kind: "cut-edge"; index: number; edge: TrimEdge | null; x0: number };

/**
 * Keyframes, thumbnails, waveform, kept range (자르기) or removed ranges (구간 제거) and playhead
 * over the whole file (canvas mockup Editor.dc.html). Click or drag to seek, drag the IN / OUT
 * handles or a removed range's edges, zoom to scroll.
 */
export function Timeline(props: Props) {
  const { info, keyframes, now, range, onSeek, onTrimDrag, mode, cuts, selected, marks } = props;
  const { onCutSelect, onCutDrag, onCutDragEnd } = props;
  const dict = useT();
  const w = dict.timeline;
  const cutMode = mode === "cut";
  const duration = info.duration > 0 ? info.duration : 1;
  const v = info.video;
  const scrollRef = useRef<HTMLDivElement>(null);
  const trackRef = useRef<HTMLDivElement>(null);
  const viewWidth = useElementWidth(scrollRef);
  const [zoom, setZoom] = useState(1);
  const data = useTimelineData(info, () => timelineSize(scrollRef.current, info), zoom);
  const gesture = useRef<Gesture | null>(null);
  const [dragging, setDragging] = useState(false);
  const frame = useRef<{ x: number; raf: number } | null>(null);
  /** Playhead position in the viewport to keep while the zoom changes. */
  const zoomAnchor = useRef<number | null>(null);

  const timeAt = (clientX: number) => {
    const el = trackRef.current;
    if (!el) return 0;
    const r = el.getBoundingClientRect();
    return Math.max(0, Math.min(duration, ((clientX - r.left) / Math.max(1, r.width)) * duration));
  };

  useEffect(
    () => () => {
      if (frame.current) cancelAnimationFrame(frame.current.raf);
    },
    [],
  );

  const apply = (x: number, g: Gesture) => {
    if (g.kind === "scrub") {
      onSeek(timeAt(x));
      return;
    }
    if (g.kind === "cut-edge") {
      const r = cuts[g.index];
      if (!r) return;
      if (!g.edge) {
        if (Math.abs(x - g.x0) < 1) return;
        g.edge = x < g.x0 ? "start" : "end";
      }
      onCutDrag(g.index, g.edge, dragHandleTime(g.edge, timeAt(x), r, duration, v));
      return;
    }
    if (!range) return;
    // Handles on top of each other: the first move picks the side.
    if (!g.edge) {
      if (Math.abs(x - g.x0) < 1) return;
      g.edge = x < g.x0 ? "start" : "end";
    }
    onTrimDrag(g.edge, dragHandleTime(g.edge, timeAt(x), range, duration, v));
  };

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    const el = e.currentTarget;
    el.setPointerCapture(e.pointerId);
    // The zoomed track is wider than its viewport: focusing must not scroll it.
    el.focus({ preventScroll: true });
    const r = el.getBoundingClientRect();
    const px = (t: number) => (t / duration) * r.width;
    if (cutMode) {
      const edge = grabEdge(e.clientX - r.left, cuts.map((c) => ({ start: px(c.start), end: px(c.end) })), HANDLE_HIT_PX);
      if (edge) {
        const g: Gesture = { kind: "cut-edge", index: edge.index, edge: edge.edge === "either" ? null : edge.edge, x0: e.clientX };
        gesture.current = g;
        setDragging(true);
        onCutSelect(edge.index);
        if (g.edge) onCutDrag(edge.index, g.edge, g.edge === "start" ? cuts[edge.index].start : cuts[edge.index].end);
        return;
      }
      onCutSelect(rangeIndexAt(cuts, timeAt(e.clientX)));
      gesture.current = { kind: "scrub" };
      onSeek(timeAt(e.clientX));
      return;
    }
    const grabbed = range ? grabHandle(e.clientX - r.left, px(range.start), px(range.end), HANDLE_HIT_PX) : null;
    if (range && grabbed) {
      const edge = grabbed === "either" ? null : grabbed;
      gesture.current = { kind: "handle", edge, x0: e.clientX };
      setDragging(true);
      // Show the frame at the handle (and pause) before it moves.
      if (edge) onTrimDrag(edge, edge === "start" ? range.start : range.end);
      return;
    }
    gesture.current = { kind: "scrub" };
    onSeek(timeAt(e.clientX));
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const g = gesture.current;
    if (!g) return;
    // One update per animation frame while dragging.
    if (frame.current) {
      frame.current.x = e.clientX;
      return;
    }
    const pending = { x: e.clientX, raf: 0 };
    pending.raf = requestAnimationFrame(() => {
      frame.current = null;
      apply(pending.x, g);
    });
    frame.current = pending;
  };
  const endGesture = () => {
    const g = gesture.current;
    if (!g) return;
    // The last move first, then merge (the index would be stale once ranges merge).
    if (frame.current) {
      cancelAnimationFrame(frame.current.raf);
      const x = frame.current.x;
      frame.current = null;
      apply(x, g);
    }
    gesture.current = null;
    setDragging(false);
    if (g.kind === "cut-edge") onCutDragEnd(g.index);
  };

  // Zoomed in: keep the playhead in view (playing, stepping, typing IN / OUT), but not while the
  // pointer is down — the user is placing it.
  useEffect(() => {
    const el = scrollRef.current;
    const track = trackRef.current;
    if (!el || !track || zoom <= 1 || gesture.current) return;
    const x = playheadX(el, track, now / duration);
    const margin = Math.min(48, el.clientWidth * 0.1);
    if (x < margin || x > el.clientWidth - margin) el.scrollLeft += x - el.clientWidth / 2;
  }, [now, zoom, duration]);

  // Zoom around the playhead: it stays where it was on screen (or comes to the middle).
  useLayoutEffect(() => {
    const el = scrollRef.current;
    const track = trackRef.current;
    const anchor = zoomAnchor.current;
    zoomAnchor.current = null;
    if (!el || !track || anchor === null) return;
    el.scrollLeft += playheadX(el, track, now / duration) - anchor;
    // Only on a zoom change; `now` is read as it is then.
  }, [zoom]);

  const changeZoom = (z: number) => {
    const el = scrollRef.current;
    const track = trackRef.current;
    if (el && track) {
      const x = playheadX(el, track, now / duration);
      zoomAnchor.current = x >= 0 && x <= el.clientWidth ? x : el.clientWidth / 2;
    }
    setZoom(z);
  };

  const pct = (t: number) => `${(Math.max(0, Math.min(duration, t)) / duration) * 100}%`;
  const ticks = rulerTicks(duration, Math.max(2, Math.floor(((viewWidth || 600) * zoom) / 90)));
  const status = [
    data.busy ? w.busy : null,
    !v ? w.noVideo : keyframes.length === 0 ? w.noKeyframes : null,
    info.audio.length === 0 ? w.noAudio : null,
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <section className="ed-timeline" aria-label={w.title}>
      <div className="ed-tl-head">
        <div className="ed-legend">
          <span className="ed-tl-title">{w.title}</span>
          {cutMode ? (
            <span className="ed-legend-item">
              <span className="ed-swatch cut" />
              {w.removed}
            </span>
          ) : (
            <span className="ed-legend-item">
              <span className="ed-swatch keep" />
              {w.kept}
            </span>
          )}
          <span className="ed-legend-item">
            <span className="ed-swatch keyframe" />
            {w.keyframe}
          </span>
          <span className="ed-legend-item">
            <span className="ed-swatch playhead" />
            {w.playhead}
          </span>
          {status && <span className="ed-legend-item">{status}</span>}
        </div>
        <label className="ed-zoom">
          {w.zoom}
          <input
            type="range"
            min={1}
            max={MAX_ZOOM}
            step={1}
            value={zoom}
            aria-valuetext={w.zoomValue(zoom)}
            onChange={(e) => changeZoom(Number(e.target.value))}
          />
        </label>
      </div>

      <div ref={scrollRef} className="ed-tl-scroll">
        <div className="ed-tl-content" style={{ width: `calc(${zoom} * (100% - var(--tl-pad) * 2) + var(--tl-pad) * 2)` }}>
          <div className="ed-ruler mono" aria-hidden="true">
            {ticks.map((t) => (
              <span key={t} style={t === 0 ? { left: 0, transform: "none" } : { left: pct(t) }}>
                {formatClock(t)}
              </span>
            ))}
            <span style={{ right: 0, left: "auto", transform: "none" }}>{formatClock(duration)}</span>
          </div>

          <div
            ref={trackRef}
            className={dragging ? "ed-track handle-drag" : "ed-track"}
            role="slider"
            tabIndex={0}
            aria-label={w.trackLabel}
            aria-valuemin={0}
            aria-valuemax={duration}
            aria-valuenow={now}
            aria-valuetext={formatTime(now)}
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={endGesture}
            onPointerCancel={endGesture}
            onLostPointerCapture={endGesture}
            onKeyDown={(e) => {
              // ← → and Shift are the editor's shortcuts (EditorScreen).
              if (e.key === "Home") {
                e.preventDefault();
                onSeek(0);
              } else if (e.key === "End") {
                e.preventDefault();
                onSeek(duration);
              }
            }}
          >
            <KeyframeTicks keyframes={keyframes} duration={duration} />
            <Thumbs strip={data.strip} duration={duration} />
            <WaveCanvas wave={data.wave} duration={duration} className="ed-wave" />
            {cutMode && <CutRanges cuts={cuts} selected={selected} marks={marks} duration={duration} />}
            {!cutMode && range && (
              <>
                <div className="ed-dim" style={{ left: 0, width: pct(range.start) }} />
                <div className="ed-dim" style={{ left: pct(range.end), right: 0 }} />
                <div className="ed-keep" style={{ left: pct(range.start), width: pct(range.end - range.start) }} />
                <div className="ed-handle" style={{ left: pct(range.start) }} />
                <div className="ed-handle" style={{ left: pct(range.end) }} />
                <RangeTags range={range} duration={duration} trackPx={((viewWidth || 600) - 16) * zoom} />
              </>
            )}
            <div className="ed-playhead" style={{ left: pct(now) }} />
          </div>
          {props.masks.length > 0 && (
            <RangeLane
              title={dict.masks.lane}
              label={dict.masks.laneLabel}
              tag={(i) => `#${i + 1}`}
              masks={props.masks}
              selected={props.maskSel}
              now={now}
              duration={duration}
              video={v}
              onSelect={props.onMaskSelect}
              onTime={props.onMaskTime}
            />
          )}
          {props.overlays.length > 0 && (
            <RangeLane
              variant="overlay"
              title={dict.overlays.lane}
              label={dict.overlays.laneLabel}
              tag={(i) => `${props.overlays[i]?.image ? "▣" : "▶"} ${i + 1}`}
              masks={props.overlays}
              selected={props.overlaySel}
              now={now}
              duration={duration}
              video={v}
              onSelect={props.onOverlaySelect}
              onTime={props.onOverlayTime}
            />
          )}
        </div>
      </div>

      <p className="ed-hint">
        {cutMode ? w.hintCut : w.hintTrim}
      </p>
      {data.thumbError && <p className="error small">{w.thumbError(say(dict, data.thumbError))}</p>}
      {data.waveError && <p className="error small">{w.waveError(say(dict, data.waveError))}</p>}
    </section>
  );
}

/**
 * "IN 00:05.467" right of the IN handle and "OUT …" left of the OUT handle, like the mockup. Near an
 * end of the track a tag turns to the other side of its handle, and when the two would overlap
 * (a short range) the OUT tag drops one row.
 */
function RangeTags({ range, duration, trackPx }: { range: TrimRange; duration: number; trackPx: number }) {
  const inText = `IN ${formatTime(range.start)}`;
  const outText = `OUT ${formatTime(range.end)}`;
  const inX = (range.start / duration) * trackPx;
  const outX = (range.end / duration) * trackPx;
  const inW = tagWidth(inText);
  const outW = tagWidth(outText);
  const inSpan = inX + TAG_GAP + inW > trackPx ? [inX - TAG_GAP - inW, inX - TAG_GAP] : [inX + TAG_GAP, inX + TAG_GAP + inW];
  const outSpan = outX - TAG_GAP - outW < 0 ? [outX + TAG_GAP, outX + TAG_GAP + outW] : [outX - TAG_GAP - outW, outX - TAG_GAP];
  const low = inSpan[0] < outSpan[1] + 4 && outSpan[0] < inSpan[1] + 4;
  const place = (t: number, span: number[], x: number) =>
    span[0] > x
      ? { className: "after", style: { left: `${(t / duration) * 100}%` } }
      : { className: "before", style: { right: `${(1 - t / duration) * 100}%` } };
  const a = place(range.start, inSpan, inX);
  const b = place(range.end, outSpan, outX);
  return (
    <>
      <span className={`ed-tag mono ${a.className}`} style={a.style}>
        {inText}
      </span>
      <span className={`ed-tag mono ${b.className}${low ? " low" : ""}`} style={b.style}>
        {outText}
      </span>
    </>
  );
}

const TAG_GAP = 6;

/** 구간 제거: hatched removed ranges with their number, the selected one brighter, and the I · O marks. */
function CutRanges({ cuts, selected, marks, duration }: { cuts: Range[]; selected: number | null; marks: Marks; duration: number }) {
  const pct = (t: number) => `${(Math.max(0, Math.min(duration, t)) / duration) * 100}%`;
  const { start: ms, end: me } = marks;
  return (
    <>
      {cuts.map((c, i) => (
        <div
          key={i}
          className={i === selected ? "ed-cut selected" : "ed-cut"}
          style={{ left: pct(c.start), width: pct(c.end - c.start) }}
        >
          <span className="ed-cut-tag">#{i + 1}</span>
        </div>
      ))}
      {ms !== null && me !== null ? (
        <div className="ed-mark-range" style={{ left: pct(ms), width: pct(me - ms) }} />
      ) : (
        (ms ?? me) !== null && <div className="ed-mark-line" style={{ left: pct((ms ?? me)!) }} />
      )}
      {ms !== null && (
        <span className="ed-tag mono mark after" style={{ left: pct(ms) }}>
          I {formatTime(ms)}
        </span>
      )}
      {me !== null && (
        <span className={`ed-tag mono mark before${ms !== null ? " low" : ""}`} style={{ right: `${(1 - Math.min(duration, me) / duration) * 100}%` }}>
          O {formatTime(me)}
        </span>
      )}
    </>
  );
}

/**
 * A row under the track (가리기 · 올리기): one bar per item over its times. A press picks the item
 * under it (pressing again where items overlap picks the next one); the selected item's ends drag
 * like the IN / OUT handles (on frame starts, a frame apart).
 */
function RangeLane({
  title,
  label,
  tag,
  variant,
  masks,
  selected,
  now,
  duration,
  video,
  onSelect,
  onTime,
}: {
  title: string;
  label: string;
  tag: (index: number) => string;
  variant?: string;
  masks: TrimRange[];
  selected: number | null;
  now: number;
  duration: number;
  video: MediaInfo["video"];
  onSelect: (index: number) => void;
  onTime: (index: number, edge: TrimEdge, t: number) => void;
}) {
  const drag = useRef<{ index: number; edge: TrimEdge } | null>(null);
  const pct = (t: number) => `${(Math.max(0, Math.min(duration, t)) / duration) * 100}%`;
  const timeAt = (el: HTMLElement, clientX: number) => {
    const r = el.getBoundingClientRect();
    return Math.max(0, Math.min(duration, ((clientX - r.left) / Math.max(1, r.width)) * duration));
  };
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    const el = e.currentTarget;
    const r = el.getBoundingClientRect();
    const px = (t: number) => (t / duration) * r.width;
    const x = e.clientX - r.left;
    if (selected !== null && masks[selected]) {
      const m = masks[selected];
      const edge = grabHandle(x, px(m.start), px(m.end), HANDLE_HIT_PX);
      if (edge) {
        el.setPointerCapture(e.pointerId);
        drag.current = { index: selected, edge: edge === "either" ? (x < px(m.start) ? "start" : "end") : edge };
        return;
      }
    }
    const t = timeAt(el, e.clientX);
    const under = masks.map((m, i) => (t >= m.start && t < m.end ? i : -1)).filter((i) => i >= 0);
    if (under.length === 0) return;
    // Pressing again where boxes overlap goes on to the next one.
    const cur = selected !== null ? under.indexOf(selected) : -1;
    onSelect(cur >= 0 ? under[(cur + 1) % under.length] : under[0]);
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d || !masks[d.index]) return;
    onTime(d.index, d.edge, dragHandleTime(d.edge, timeAt(e.currentTarget, e.clientX), masks[d.index], duration, video));
  };
  const end = () => {
    drag.current = null;
  };
  return (
    <div
      className={variant ? `ed-mask-lane ${variant}` : "ed-mask-lane"}
      role="group"
      aria-label={label}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={end}
      onPointerCancel={end}
      onLostPointerCapture={end}
    >
      <span className="ed-mask-lane-title" aria-hidden="true">
        {title}
      </span>
      {masks.map((m, i) => (
        <span key={i} className={i === selected ? "ed-mask-bar selected" : "ed-mask-bar"} style={{ left: pct(m.start), width: pct(m.end - m.start) }}>
          {tag(i)}
        </span>
      ))}
      <span className="ed-mask-lane-now" style={{ left: pct(now) }} aria-hidden="true" />
    </div>
  );
}

/** About how wide a tag is (11px bold monospace + padding). */
function tagWidth(text: string): number {
  return text.length * 6.8 + 12;
}

/** One 1px tick per keyframe in the 8px row above the thumbnails; scales with the zoom. */
const KeyframeTicks = memo(function KeyframeTicks({ keyframes, duration }: { keyframes: number[]; duration: number }) {
  const d = useMemo(
    () =>
      keyframes
        .filter((k) => k >= 0 && k <= duration)
        .map((k) => `M${k.toFixed(4)} 0V8`)
        .join(""),
    [keyframes, duration],
  );
  return (
    <svg className="ed-keyframes" viewBox={`0 0 ${duration} 8`} preserveAspectRatio="none" aria-hidden="true">
      <path d={d} vectorEffect="non-scaling-stroke" shapeRendering="crispEdges" />
    </svg>
  );
});

const Thumbs = memo(function Thumbs({ strip, duration }: { strip: ThumbnailStrip | null; duration: number }) {
  const pct = (t: number) => `${(t / duration) * 100}%`;
  return (
    <div className="ed-thumbs" aria-hidden="true">
      {strip?.images.map((src, k) => (
        <img
          key={k}
          src={src}
          alt=""
          draggable={false}
          style={{ left: pct(strip.start + k * strip.interval), width: pct(strip.interval) }}
        />
      ))}
    </div>
  );
});

/** Where fraction `f` of the track is, in px from the scroll viewport's left edge. */
function playheadX(scroll: HTMLElement, track: HTMLElement, f: number): number {
  const s = scroll.getBoundingClientRect();
  const t = track.getBoundingClientRect();
  return t.left + f * t.width - s.left;
}

/**
 * Enough tiles to cover the visible track without stretching much (more per zoom level), and
 * waveform buckets for one per device pixel at the deepest zoom.
 */
function timelineSize(view: HTMLElement | null, info: MediaInfo) {
  const width = view?.clientWidth || 900;
  const dpr = window.devicePixelRatio || 1;
  const v = info.video;
  let aspect = v && v.width > 0 && v.height > 0 ? v.width / v.height : 16 / 9;
  if (v?.rotation && Math.abs(v.rotation) % 180 === 90) aspect = 1 / aspect;
  const tileWidth = THUMB_HEIGHT * Math.min(3, Math.max(0.4, aspect));
  return {
    tiles: Math.max(4, Math.min(60, Math.round(width / tileWidth))),
    height: Math.min(360, Math.round(THUMB_HEIGHT * dpr)),
    // preview::MAX_WAVEFORM_BUCKETS
    buckets: Math.max(200, Math.min(20_000, Math.round(width * dpr * MAX_ZOOM))),
  };
}

function useElementWidth(ref: RefObject<HTMLElement | null>): number {
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const observer = new ResizeObserver(() => setWidth(el.clientWidth));
    observer.observe(el);
    setWidth(el.clientWidth);
    return () => observer.disconnect();
  }, [ref]);
  return width;
}
