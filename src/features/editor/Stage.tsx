import { type CSSProperties, type ReactNode, type RefObject, useEffect, useRef, useState } from "react";
import { fileNameOf } from "../../lib/files";
import { useT } from "../../lib/i18n/I18n";
import type { Messages } from "../../lib/i18n/ko";
import { say } from "../../lib/i18n/text";
import type { Crop, Effects, MediaInfo, Msg, OverlaySource, VideoStream } from "../../lib/ipc";
import { type PlaybackStatus, playbackLabel } from "../../lib/playback";
import { formatTime } from "../../lib/time";
import { CropBox } from "./CropBox";
import { MaskBoxes, type MaskFrame, MaskPreviews } from "./MaskLayer";
import { OverlayBoxes, OverlayPreviews } from "./OverlayLayer";
import { cropOf, type EffectsPlan, evenRect, fromSeen, isNone, type Rect, seenCropSize, seenSize, toSeen, uprightSize } from "./effects";
import { codecLabel, containerLabel } from "./format";
import { editLayout, inset, onStage, resultLayout, type VideoCss, videoCss } from "./stageLayout";
import { DropIcon, FolderIcon } from "./icons";
import type { Player } from "./usePlayer";
import type { ProxyState } from "./useProxy";

export interface FfmpegState {
  ready: boolean;
  locating: boolean;
  problem: Msg | null;
  /** Why the user's own FFmpeg path was skipped (a saved path that vanished or is too old). */
  notices: Msg[];
  /** The ffmpeg in use (encoder choices are per FFmpeg). */
  path: string | null;
}

/** 화면 효과 on the stage: the picture turned as it will be, the crop box, or only the result. */
export interface StageFx {
  effects: Effects;
  /** effects::plan's numbers (null: nothing changes the picture). */
  plan: EffectsPlan | null;
  view: "edit" | "result";
  onView: (view: "edit" | "result") => void;
  /** The crop box dragged (source pixels, even). */
  onCrop: (crop: Crop) => void;
  /** 모자이크 · 블러: the selected box is dragged, another one is picked. */
  maskSel: number | null;
  onMaskSelect: (index: number) => void;
  onMaskChange: (index: number, rect: Rect) => void;
  /** PiP: the files' probes (by path), the selected one, picking one, its box dragged. */
  sources: Record<string, OverlaySource>;
  overlaySel: number | null;
  onOverlaySelect: (index: number) => void;
  onOverlayChange: (index: number, rect: Rect) => void;
}

/** Room around the picture for the crop box's handles; above it, a strip for the badge and 편집 화면 | 결과. */
const STAGE_MARGIN = 14;
const STAGE_TOP = 44;

/** Files dragged over the window. */
export interface DropState {
  paths: string[];
  /** Why the drop will be ignored, or null when it opens the first path. */
  blocked: string | null;
}

interface Props {
  videoRef: RefObject<HTMLVideoElement | null>;
  info: MediaInfo | null;
  src: string;
  /** The element shows the proxy, not the original. */
  onProxy: boolean;
  original: PlaybackStatus;
  proxyStatus: PlaybackStatus;
  proxy: ProxyState;
  player: Player;
  ffmpeg: FfmpegState;
  opening: boolean;
  openError: Msg | null;
  canOpen: boolean;
  drop: DropState | null;
  onOpen: () => void;
  onOpenSettings: () => void;
  onLoadedMetadata: (el: HTMLVideoElement) => void;
  onError: (el: HTMLVideoElement) => void;
  fx: StageFx | null;
}

/** The picture: original or proxy, with what is going on drawn over it. */
export function Stage(p: Props) {
  const t = useT();
  const { info, player } = p;
  const v = info?.video ?? null;
  const overlay = stageMessage(t, p);
  const stageRef = useRef<HTMLDivElement>(null);
  const size = useSize(stageRef);
  const fx = p.fx && v && !overlay && !isNone(p.fx.effects) && size.width > 0 ? drawFx(p.fx, v, size) : null;

  return (
    <div className={fx ? "ed-stage fx" : "ed-stage"} ref={stageRef}>
      {fx?.frame}
      {/* The video stays in this one wrapper, so turning effects on and off never reloads it. */}
      <div className={fx ? `ed-picture fx ${fx.result ? "result" : ""}` : "ed-picture"} style={fx?.picture}>
        {info && (
          <video
            ref={p.videoRef}
            key={p.src}
            src={p.src}
            preload="auto"
            playsInline
            aria-label={t.stage.preview}
            className={v && !overlay ? undefined : "ed-video-hidden"}
            style={fx?.video}
            onClick={player.toggle}
            onLoadedMetadata={(e) => p.onLoadedMetadata(e.currentTarget)}
            onError={(e) => p.onError(e.currentTarget)}
            {...player.events}
          />
        )}
        {fx && p.fx && v && <MaskPreviews masks={p.fx.effects.masks} now={player.now} src={uprightSize(v)} frame={fx.masks} videoRef={p.videoRef} />}
        {fx && p.fx && v && (
          <OverlayPreviews overlays={p.fx.effects.overlays} sources={p.fx.sources} now={player.now} playing={player.playing} src={uprightSize(v)} frame={fx.masks} />
        )}
      </div>
      {fx?.crop}
      {fx?.edit && p.fx && v && (
        <MaskBoxes
          masks={p.fx.effects.masks}
          selected={p.fx.maskSel}
          now={player.now}
          src={uprightSize(v)}
          seen={fx.edit.seen}
          scale={fx.edit.scale}
          o={p.fx.effects}
          onSelect={p.fx.onMaskSelect}
          onChange={p.fx.onMaskChange}
        />
      )}
      {fx?.edit && p.fx && v && (
        <OverlayBoxes
          overlays={p.fx.effects.overlays}
          sources={p.fx.sources}
          selected={p.fx.overlaySel}
          now={player.now}
          src={uprightSize(v)}
          seen={fx.edit.seen}
          scale={fx.edit.scale}
          o={p.fx.effects}
          onSelect={p.fx.onOverlaySelect}
          onChange={p.fx.onOverlayChange}
        />
      )}
      {fx && p.fx && (
        <div className="ed-stage-view" role="group" aria-label={t.effects.view}>
          <button type="button" aria-pressed={p.fx.view === "edit"} onClick={() => p.fx?.onView("edit")}>
            {t.effects.viewEdit}
          </button>
          <button type="button" aria-pressed={p.fx.view === "result"} onClick={() => p.fx?.onView("result")}>
            {t.effects.viewResult(fx.out[0], fx.out[1])}
          </button>
        </div>
      )}
      {overlay && (
        <div className="ed-stage-msg">
          <Corners />
          {overlay}
        </div>
      )}
      {p.drop && <DropMessage {...p.drop} />}
      {info && p.onProxy && <span className="ed-badge">{t.stage.proxyBadge}</span>}
      {info && v && !overlay && <span className="ed-stage-tc mono">{formatTime(player.now)}</span>}
    </div>
  );
}

function useSize(ref: RefObject<HTMLElement | null>): { width: number; height: number } {
  const [size, setSize] = useState({ width: 0, height: 0 });
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setSize({ width: el.clientWidth, height: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return size;
}

function cssOf(c: VideoCss): CSSProperties {
  return { left: c.left, top: c.top, width: c.width, height: c.height, transform: c.transform };
}

/**
 * 편집 화면: the whole picture turned and mirrored as it will be, the crop box over it. 결과: only
 * what the export contains, in its frame. Everything in source pixels; the element (original or
 * proxy) is stretched to the box.
 */
function drawFx(fx: StageFx, v: VideoStream, stage: { width: number; height: number }) {
  const e = fx.effects;
  const src = uprightSize(v);
  const area = inset({ left: 0, top: STAGE_TOP - STAGE_MARGIN, width: stage.width, height: stage.height - STAGE_TOP + STAGE_MARGIN }, STAGE_MARGIN);
  const out = fx.plan?.size ?? seenCropSize(src, e);
  if (fx.view === "result" && fx.plan) {
    const { frame, picture, seen } = resultLayout(area, src, fx.plan);
    const video = videoCss({ ...seen, left: seen.left - picture.left, top: seen.top - picture.top }, fx.plan);
    // The frame (black, where 맞춤 puts bars) behind; the video clipped to the picture's place in it.
    const bars = <div className="ed-result-frame" style={{ left: frame.left, top: frame.top, width: frame.width, height: frame.height }} />;
    const [W, H] = seenSize(src, fx.plan.turn);
    const masks: MaskFrame = {
      seen: { ...seen, left: seen.left - picture.left, top: seen.top - picture.top },
      kx: W > 0 ? seen.width / W : 0,
      ky: H > 0 ? seen.height / H : 0,
      o: fx.plan,
    };
    return { result: true, out, frame: bars, picture: { left: picture.left, top: picture.top, width: picture.width, height: picture.height }, video: cssOf(video), crop: null, edit: null, masks };
  }
  const { seen, scale } = editLayout(area, src, e);
  const picture: CSSProperties = { left: 0, top: 0, width: stage.width, height: stage.height };
  let crop: ReactNode = null;
  if (e.crop && fx.view === "edit") {
    const c = e.crop;
    const box = toSeen(c, src, e);
    const label = `${box.width} × ${box.height}${c.ratio ? ` · ${c.ratio.join(":")}` : ""}`;
    crop = (
      <CropBox
        box={onStage(box, seen, scale)}
        seen={box}
        frame={seenSize(src, e.turn)}
        scale={scale}
        ratio={c.ratio}
        label={label}
        onDrag={(r) => fx.onCrop(cropOf(evenRect(fromSeen(r, src, e), src), c.ratio))}
      />
    );
  }
  const masks: MaskFrame = { seen, kx: scale, ky: scale, o: e };
  return { result: false, out, frame: null, picture, video: cssOf(videoCss(seen, e)), crop, edit: { seen, scale }, masks };
}

function stageMessage(t: Messages, p: Props): ReactNode {
  const { info, ffmpeg, proxy } = p;
  const w = t.stage;
  if (!info) {
    if (p.opening) return <Title>{w.reading}</Title>;
    if (ffmpeg.locating) return <Title>{w.locating}</Title>;
    if (!ffmpeg.ready) {
      return (
        <>
          <Title>{w.needsFfmpeg}</Title>
          <Sub>{ffmpeg.problem ? say(t, ffmpeg.problem) : w.notInstalled}</Sub>
          <Notices notices={ffmpeg.notices} />
          <button type="button" className="ed-btn primary" onClick={p.onOpenSettings}>
            {w.openSettings}
          </button>
        </>
      );
    }
    return (
      <>
        <Title>{w.emptyTitle}</Title>
        <Sub>{w.emptySub}</Sub>
        {p.openError && <p className="ed-stage-error">{say(t, p.openError)}</p>}
        <button type="button" className="ed-btn primary" onClick={p.onOpen} disabled={!p.canOpen}>
          <FolderIcon />
          {t.editor.open}
        </button>
        {ffmpeg.notices.length > 0 && (
          <>
            <Notices notices={ffmpeg.notices} />
            <button type="button" className="ed-link" onClick={p.onOpenSettings}>
              {w.seeSettings}
            </button>
          </>
        )}
      </>
    );
  }

  const v = info.video;
  if (!v) return <Title>{w.noVideo}</Title>;

  if (p.onProxy) {
    if (p.proxyStatus.state === "error" || p.proxyStatus.state === "audio-only") {
      return (
        <>
          <Title>{w.proxyUnplayable}</Title>
          <Sub>{playbackLabel(t, p.proxyStatus)}</Sub>
        </>
      );
    }
    return null;
  }

  if (p.original.state === "loading" || p.original.state === "idle") return <Title>{w.loading}</Title>;
  if (p.original.state === "ok") return null;

  // The webview cannot show the original.
  const what = `${containerLabel(info.fileName, info.formatName)} · ${codecLabel(v.codec)}`;
  const job = proxy.job;
  if (job.running || (!proxy.error && !job.finished)) {
    return (
      <>
        <Title>{w.makingProxy}</Title>
        <Sub>{w.makingProxySub(what)}</Sub>
        <div className="ed-stage-progress">
          <div
            className="progress"
            role="progressbar"
            aria-label={w.proxyProgress}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(job.percent)}
          >
            <div style={{ width: `${job.percent}%` }} />
          </div>
          <span className="mono small muted">
            {job.percent.toFixed(0)}%{job.progress?.speed ? ` · ${job.progress.speed.toFixed(1)}×` : ""}
          </span>
        </div>
        {job.running && (
          <button type="button" className="ed-btn danger" onClick={job.cancel}>
            {w.cancelProxy}
          </button>
        )}
      </>
    );
  }
  const failed = proxy.error ?? job.finished?.error ?? null;
  return (
    <>
      <Title>{w.cannotPlay(what)}</Title>
      <Sub>{job.finished?.canceled ? w.proxyCanceled : failed ? say(t, failed) : w.proxyFailed}</Sub>
      <button type="button" className="ed-btn primary" onClick={proxy.start}>
        {w.makeProxy}
      </button>
    </>
  );
}

/** Over everything on the stage while files are dragged over the window (the drop lands anywhere). */
function DropMessage({ paths, blocked }: DropState) {
  const w = useT().stage;
  const name = fileNameOf(paths[0]);
  return (
    <div className={blocked ? "ed-stage-drop blocked" : "ed-stage-drop"} role="status">
      {blocked ? (
        <>
          <Title>{w.dropBlocked}</Title>
          <Sub>{blocked}</Sub>
        </>
      ) : (
        <>
          <DropIcon />
          <Title>{w.dropOpens}</Title>
          <span className="ed-stage-drop-name" title={paths[0]}>
            {name}
          </span>
          {paths.length > 1 && <Sub>{w.dropFirstOnly(paths.length)}</Sub>}
        </>
      )}
    </div>
  );
}

function Title({ children }: { children: ReactNode }) {
  return <span className="ed-stage-title">{children}</span>;
}

function Sub({ children }: { children: ReactNode }) {
  return <span className="ed-stage-sub">{children}</span>;
}

function Notices({ notices }: { notices: Msg[] }) {
  const t = useT();
  return notices.map((n, i) => (
    <p key={i} className="ed-stage-notice" role="status">
      {say(t, n)}
    </p>
  ));
}

function Corners() {
  return (
    <>
      <span className="ed-corner tl" aria-hidden="true" />
      <span className="ed-corner tr" aria-hidden="true" />
      <span className="ed-corner bl" aria-hidden="true" />
      <span className="ed-corner br" aria-hidden="true" />
    </>
  );
}
