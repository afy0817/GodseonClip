// Editor (canvas UI mockup Editor.dc.html): open (file dialog, or a file dropped on the window) →
// play (original, or the 540p proxy when the webview cannot) → 자르기 (one kept range: IN / OUT
// handles, I · O) or 구간 제거 (ranges to remove: I · O mark, A adds, Delete removes, edges drag) →
// export (lossless or precise, for both modes) — each export is a 작업 큐 item, so another file can
// be opened while it runs.

import { useCallback, useEffect, useRef, useState } from "react";
import { say } from "../../lib/i18n/text";
import { convertFileSrc } from "@tauri-apps/api/core";
import { pickOverlayFile, pickVideoFile } from "../../lib/files";
import { useT } from "../../lib/i18n/I18n";
import { api, type Crop, type Effects, isMovFamily, type Mask, type Msg, type Overlay, type OverlaySource, type ProbeResult, type QueueItem, toMsg, type VideoStream } from "../../lib/ipc";
import type { PlaybackStatus } from "../../lib/playback";
import type { QueueState } from "../../lib/useQueue";
import { CutSection, type Marks, NO_MARKS } from "./CutSection";
import { addRange, keepFromRemoved, minKeepLength, type Range, settle } from "./cutList";
import { isNone, MAX_MASKS, MAX_OVERLAYS, NO_EFFECTS, newMask, newOverlay, type Orientation, overlayAfterTurn, planEffects, sameOrientation, settled, uprightSize } from "./effects";
import { type EffectsNote, EffectsSection } from "./EffectsSection";
import { ExportFooter, type ExportProps, ExportSection } from "./ExportSection";
import { MaskSection } from "./MaskSection";
import { OverlaySection } from "./OverlaySection";
import { DEFAULT_EXPORT, type EditorMode, encoderFor, type ExportChoice, isWholeFile, requestOptions, targetCodec } from "./exportOptions";
import { frameStart, mediaSummary, stepFrame, typedFrameStart } from "./format";
import { FolderIcon } from "./icons";
import { losslessCutPreview } from "./losslessCut";
import { keyBelongsToTarget, REPEATING, type Shortcut, shortcutFor } from "./shortcuts";
import { type FfmpegState, Stage } from "./Stage";
import { Timeline } from "./Timeline";
import { TopBar, type WorkKind } from "./TopBar";
import { Transport } from "./Transport";
import { losslessPreview, trimProblem } from "./trim";
import type { TrimEdge } from "./trimHandles";
import { TrimSection } from "./TrimSection";
import { useExportEncoders } from "./useExportEncoders";
import { useFileDrop } from "./useFileDrop";
import { usePlayer } from "./usePlayer";
import { useProxy } from "./useProxy";
import "./editor.css";

interface Props {
  /** False while another screen (작업 큐, settings) covers the editor. */
  active: boolean;
  ffmpeg: FfmpegState;
  /** 자르기 / 구간 제거 (the other screens' header switches it too). */
  mode: EditorMode;
  /** The header's work kinds (App switches the mode or the screen). */
  onKind: (k: WorkKind) => void;
  queue: QueueState;
  /** Running + waiting queue items (header badge). */
  queueCount: number;
  onOpenQueue: () => void;
  onOpenSettings: () => void;
}

/** 구간 제거: the ranges to remove (sorted, merged) and the selected one, updated together. */
interface Cuts {
  removed: Range[];
  selected: number | null;
}

const NO_CUTS: Cuts = { removed: [], selected: null };

/** Length of a range added at the playhead without I · O marks. */
const DEFAULT_CUT_LENGTH = 5;

export function EditorScreen({ active, ffmpeg, mode, onKind, queue, queueCount, onOpenQueue, onOpenSettings }: Props) {
  const t = useT();
  const videoRef = useRef<HTMLVideoElement>(null);
  const [probe, setProbe] = useState<ProbeResult | null>(null);
  const [opening, setOpening] = useState(false);
  const [openError, setOpenError] = useState<Msg | null>(null);
  const [original, setOriginal] = useState<PlaybackStatus>({ state: "idle" });
  const [proxyStatus, setProxyStatus] = useState<PlaybackStatus>({ state: "idle" });
  const [range, setRange] = useState({ start: 0, end: 0 });
  // 자르기 / 구간 제거 (`mode`) stays as chosen when another file is opened; the ranges do not.
  const [cuts, setCuts] = useState<Cuts>(NO_CUTS);
  const [marks, setMarks] = useState<Marks>(NO_MARKS);
  // 방식 · 코덱 · 품질 stay as chosen when another file is opened.
  const [exportChoice, setExportChoice] = useState<ExportChoice>(DEFAULT_EXPORT);
  // 화면 효과 belong to the opened file (its picture); the crop box is remembered while it is off.
  const [effects, setEffects] = useState<Effects>(NO_EFFECTS);
  const [rememberedCrop, setRememberedCrop] = useState<Crop | null>(null);
  const [fxNote, setFxNote] = useState<EffectsNote>(null);
  const [stageView, setStageView] = useState<"edit" | "result">("edit");
  // 모자이크 · 블러: the selected box, and the boxes 무손실 turned off (they can come back).
  const [maskSel, setMaskSel] = useState<number | null>(null);
  const [rememberedMasks, setRememberedMasks] = useState<Mask[]>([]);
  // PiP: the selected file, the files 무손실 turned off, the files' probes, a file being read.
  const [overlaySel, setOverlaySel] = useState<number | null>(null);
  const [rememberedOverlays, setRememberedOverlays] = useState<Overlay[]>([]);
  // How the picture was turned when 무손실 took them off: they come back to the same place on screen.
  const [rememberedTurn, setRememberedTurn] = useState<Orientation>(NO_EFFECTS);
  const [overlaySources, setOverlaySources] = useState<Record<string, OverlaySource>>({});
  const [addingOverlay, setAddingOverlay] = useState(false);
  const [overlayError, setOverlayError] = useState<Msg | null>(null);
  const [starting, setStarting] = useState(false);
  const [exportError, setExportError] = useState<Msg | null>(null);
  // The queue item of this file's last export, and what queueExport replied (until the queue's own
  // snapshot has it): the foot says pressing again adds another. Its progress is in the header's
  // notifications; opening another file lets go of it.
  const [followedId, setFollowedId] = useState<number | null>(null);
  const [queued, setQueued] = useState<QueueItem | null>(null);
  const seenInQueue = useRef(false);
  const fromQueue = followedId === null ? undefined : queue.items.find((i) => i.id === followedId);
  if (fromQueue) seenInQueue.current = true;
  // Gone from a snapshot that had it: taken off the queue.
  const followed = fromQueue ?? (!seenInQueue.current && queued?.id === followedId ? queued : null);

  const info = probe?.info ?? null;
  const v = info?.video ?? null;
  const duration = info?.duration ?? 0;
  // The original has a video track the webview cannot show: preview through the proxy.
  const needsProxy = v !== null && (original.state === "error" || original.state === "audio-only");
  const proxy = useProxy(info, needsProxy);
  const onProxy = proxy.path !== null;
  const src = info ? convertFileSrc(proxy.path ?? info.path) : "";
  const player = usePlayer(videoRef, info, src);
  const { pause, seek } = player;

  useEffect(() => {
    setProxyStatus({ state: proxy.path ? "loading" : "idle" });
  }, [proxy.path]);

  // Behind the settings screen: stop the sound.
  useEffect(() => {
    if (!active) pause();
  }, [active, pause]);

  // Exports run in the queue: another file can be opened meanwhile.
  const canOpen = ffmpeg.ready && !opening;
  // Why a file dragged over the window will not open: the drop is ignored.
  const dropBlocked = !ffmpeg.ready
    ? ffmpeg.locating
      ? t.editor.dropLocating
      : t.editor.dropNeedsFfmpeg
    : opening
      ? t.editor.dropOpening
      : null;

  // The file dialog and a drop both end here.
  const openPath = useCallback(async (path: string) => {
    setOpening(true);
    setOpenError(null);
    try {
      const r = await api.probeMedia(path);
      pause();
      // Reopening the same file keeps its player (the element would not reload).
      if (r.info.path !== probe?.info.path) setOriginal({ state: "loading" });
      setRange({ start: 0, end: r.info.duration });
      setCuts(NO_CUTS);
      setMarks(NO_MARKS);
      setEffects(NO_EFFECTS);
      setRememberedCrop(null);
      setFxNote(null);
      setStageView("edit");
      setMaskSel(null);
      setRememberedMasks([]);
      setOverlaySel(null);
      setRememberedOverlays([]);
      setOverlayError(null);
      setExportError(null);
      // The last export goes on in the queue; this panel is for the new file.
      setFollowedId(null);
      setQueued(null);
      setProbe(r);
    } catch (e) {
      setOpenError(toMsg(e));
    } finally {
      setOpening(false);
    }
  }, [pause, probe?.info.path]);

  // One at a time: `opening` is set only once there is a path, so a drop while the file dialog is
  // up would otherwise race the dialog's answer.
  const busy = useRef(false);
  const once = async (run: () => Promise<void>) => {
    if (busy.current) return;
    busy.current = true;
    try {
      await run();
    } finally {
      busy.current = false;
    }
  };
  const open = () =>
    once(async () => {
      const path = await pickVideoFile(t);
      if (path) await openPath(path);
    });
  // Several files: the first one (the drop overlay says so).
  const dragged = useFileDrop(active, (paths) => {
    if (canOpen) void once(() => openPath(paths[0]));
  });

  const status = (el: HTMLVideoElement): PlaybackStatus =>
    el.videoWidth > 0 ? { state: "ok", width: el.videoWidth, height: el.videoHeight } : { state: "audio-only" };
  const setStatus = onProxy ? setProxyStatus : setOriginal;
  const onLoadedMetadata = (el: HTMLVideoElement) => {
    // With only metadata loaded WKWebView never fires loadeddata before play, so judge from
    // loadedmetadata: a video track the webview cannot decode reports width 0.
    setStatus(status(el));
    player.restore();
  };
  const onError = (el: HTMLVideoElement) => {
    const code = el.error?.code ?? 0;
    setStatus({ state: "error", code });
  };

  // While playing, the playhead is mid-frame: take the start of the frame on screen.
  // 구간 제거: I · O mark the next range to remove; A adds it.
  const markIn = () => {
    const t = frameStart(player.now, v);
    if (mode === "cut") setMarks((m) => ({ start: t, end: m.end !== null && m.end > t ? m.end : null }));
    else setRange((r) => ({ start: t, end: t >= r.end ? duration : r.end }));
  };
  const markOut = () => {
    const t = Math.min(duration, frameStart(player.now, v));
    if (mode === "cut") setMarks((m) => ({ start: m.start !== null && m.start < t ? m.start : null, end: t }));
    else setRange((r) => ({ start: t <= r.start ? 0 : r.start, end: t }));
  };

  const nextCut = info ? cutToAdd(marks, player.now, duration, v) : null;
  const addCut = () => {
    if (!nextCut) return;
    setCuts((c) => {
      const { ranges, index } = addRange(c.removed, nextCut.range, duration);
      return { removed: ranges, selected: index };
    });
    setMarks(NO_MARKS);
  };
  const deleteCut = (index: number | null) => {
    if (index === null) return;
    setCuts((c) => ({ removed: c.removed.filter((_, i) => i !== index), selected: null }));
  };
  const selectCut = (index: number | null, seekTo = false) => {
    setCuts((c) => ({ ...c, selected: index }));
    if (seekTo && index !== null && cuts.removed[index]) {
      pause();
      seek(cuts.removed[index].start);
    }
  };
  // Dragging an edge shows its frame (the first removed frame, or the first one kept after it).
  const dragCut = useCallback(
    (index: number, edge: TrimEdge, t: number) => {
      pause();
      setCuts((c) => ({
        removed: c.removed.map((r, i) => (i !== index ? r : edge === "start" ? { ...r, start: t } : { ...r, end: t })),
        selected: index,
      }));
      seek(t);
    },
    [pause, seek],
  );
  const dragCutEnd = useCallback(
    (index: number) =>
      setCuts((c) => {
        const { ranges, index: selected } = settle(c.removed, index, duration);
        return { removed: ranges, selected };
      }),
    [duration],
  );

  // Dragging a handle shows its frame: the IN frame is the result's first frame.
  const dragTrim = useCallback(
    (edge: TrimEdge, t: number) => {
      pause();
      setRange((r) => (edge === "start" ? { ...r, start: t } : { ...r, end: t }));
      seek(t);
    },
    [pause, seek],
  );

  const shortcuts = useRef<Record<Shortcut, () => void>>(null!);
  shortcuts.current = {
    markIn,
    markOut,
    prevFrame: () => player.stepFrames(-1),
    nextFrame: () => player.stepFrames(1),
    back1s: () => player.jump(-1),
    forward1s: () => player.jump(1),
    togglePlay: player.toggle,
    addRange: () => {
      if (mode === "cut") addCut();
    },
    deleteRange: () => {
      if (mode === "cut") deleteCut(cuts.selected);
    },
  };
  const hasFile = info !== null;
  // Window-wide, except while typing in a field or behind the settings screen.
  useEffect(() => {
    if (!active || !hasFile) return;
    let spaceDown = false;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing) return;
      const s = shortcutFor(e);
      if (!s || keyBelongsToTarget(e.target as HTMLElement | null, s)) return;
      e.preventDefault();
      if (s === "togglePlay") spaceDown = true;
      if (e.repeat && !REPEATING.has(s)) return;
      shortcuts.current[s]();
    };
    // A focused button clicks on Space's keyup: that press was play / pause.
    const onKeyUp = (e: KeyboardEvent) => {
      if (spaceDown && (e.code === "Space" || e.key === " ")) {
        spaceDown = false;
        e.preventDefault();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
    };
  }, [active, hasFile]);

  const cutMode = mode === "cut";
  const problem = info ? trimProblem(t, range.start, range.end, duration) : null;
  const lossless = probe && !problem ? losslessPreview(probe, range.start, range.end) : null;
  const keep = info && v ? keepFromRemoved(cuts.removed, duration, minKeepLength(v.fps)) : [];
  // 방식 · 코덱 · 품질 are shared by 자르기 and 구간 제거; a file without video exports losslessly only.
  const choice: ExportChoice = !v ? DEFAULT_EXPORT : exportChoice;
  const precise = choice.mode === "precise";
  // 구간 제거 · 무손실: every kept piece from its keyframe (mirrors command::plan_lossless_cut).
  const losslessCut =
    cutMode && probe && v && !precise ? losslessCutPreview(keep, probe.keyframes, duration, isMovFamily(probe.info.formatName)) : null;
  const toPrecise = v ? () => setExportChoice((c) => ({ ...c, mode: "precise" })) : null;
  const effectsOn = v !== null && !isNone(effects);
  const planned = v ? planEffects(v, effects) : { plan: null, error: null };
  // An effect switched on while 무손실 is picked: effects need re-encoding (as H.265 · AV1 do).
  const changeEffects = (next: Effects) => {
    if (!v) return;
    let e = settled(v, next);
    // Turned or mirrored: the PiP files keep their place on the screen (they stand upright).
    if (!sameOrientation(effects, e) && e.overlays.length > 0) {
      const src = uprightSize(v);
      e = { ...e, overlays: e.overlays.map((o) => ({ ...o, ...overlayAfterTurn(o, src, effects, e) })) };
    }
    if (e.crop) setRememberedCrop(e.crop);
    setEffects(e);
    if (!isNone(e) && choice.mode === "lossless") {
      setExportChoice((c) => ({ ...c, mode: "precise" }));
      setFxNote("precise");
    } else if (fxNote === "off" || isNone(e)) setFxNote(null);
  };
  // 모자이크 · 블러 boxes live in the effects; changing them switches to 정밀 like any effect.
  const masks = effects.masks;
  const setMasks = (next: Mask[]) => changeEffects({ ...effects, masks: next });
  const addMask = () => {
    if (!v || masks.length >= MAX_MASKS) return;
    setMasks([...masks, newMask(uprightSize(v), duration)]);
    setRememberedMasks([]);
    setMaskSel(masks.length);
  };
  const deleteMask = (i: number) => {
    setMasks(masks.filter((_, k) => k !== i));
    setMaskSel((s) => (s === null || s === i ? null : s > i ? s - 1 : s));
  };
  const changeMask = (i: number, patch: Partial<Mask>) => setMasks(masks.map((m, k) => (k === i ? { ...m, ...patch } : m)));
  const restoreMasks = () => {
    setMasks(rememberedMasks);
    setRememberedMasks([]);
    setMaskSel(rememberedMasks.length > 0 ? 0 : null);
  };
  // PiP files live in the effects too.
  const overlays = effects.overlays;
  const setOverlays = (next: Overlay[]) => changeEffects({ ...effects, overlays: next });
  const addOverlay = async () => {
    if (!v || overlays.length >= MAX_OVERLAYS || addingOverlay) return;
    setOverlayError(null);
    const path = await pickOverlayFile(t);
    if (!path) return;
    setAddingOverlay(true);
    try {
      const source = await api.probeOverlay(path);
      setOverlaySources((m) => ({ ...m, [source.path]: source }));
      setOverlays([...overlays, newOverlay(uprightSize(v), effects, source, frameStart(player.now, v), duration)]);
      setRememberedOverlays([]);
      setOverlaySel(overlays.length);
    } catch (e) {
      setOverlayError(toMsg(e));
    } finally {
      setAddingOverlay(false);
    }
  };
  const deleteOverlay = (i: number) => {
    setOverlays(overlays.filter((_, k) => k !== i));
    setOverlaySel((s) => (s === null || s === i ? null : s > i ? s - 1 : s));
  };
  const changeOverlay = (i: number, patch: Partial<Overlay>) => setOverlays(overlays.map((o, k) => (k === i ? { ...o, ...patch } : o)));
  const restoreOverlays = () => {
    const src = v ? uprightSize(v) : null;
    setOverlays(src ? rememberedOverlays.map((o) => ({ ...o, ...overlayAfterTurn(o, src, rememberedTurn, effects) })) : rememberedOverlays);
    setRememberedOverlays([]);
    setOverlaySel(rememberedOverlays.length > 0 ? 0 : null);
  };
  // 무손실 takes the effects off (the crop box, the boxes to hide and the PiP files stay remembered).
  const changeChoice = (c: ExportChoice) => {
    if (c.mode === "lossless" && effectsOn) {
      if (masks.length > 0) setRememberedMasks(masks);
      if (overlays.length > 0) {
        setRememberedOverlays(overlays);
        setRememberedTurn({ turn: effects.turn, flipH: effects.flipH, flipV: effects.flipV });
      }
      setOverlaySel(null);
      setMaskSel(null);
      setEffects(NO_EFFECTS);
      setStageView("edit");
      setFxNote("off");
    } else if (c.mode !== choice.mode) setFxNote(null);
    setExportChoice(c);
  };
  // A GIF uses FFmpeg's own encoder: no encoder test to wait for.
  const exportEncoders = useExportEncoders(precise && choice.codec !== "gif" && ffmpeg.ready, ffmpeg.path, active);
  const encoderReady = !precise || encoderFor(exportEncoders.encoders, targetCodec(choice.codec, v?.codec)) !== null;
  const editReady = cutMode
    ? v !== null && cuts.removed.length > 0 && keep.length > 0 && (losslessCut?.problem ?? null) === null
    : lossless !== null;
  const canExport = ffmpeg.ready && editReady && encoderReady && !starting && planned.error === null;
  const keptLength = keep.reduce((s, r) => s + r.end - r.start, 0);
  const resultLength = cutMode
    ? editReady
      ? (losslessCut?.length ?? keptLength)
      : null
    : problem
      ? null
      : precise
        ? range.end - range.start
        : (lossless?.length ?? null);
  const whole = !cutMode && isWholeFile(range.start, range.end, duration);

  const runExport = async () => {
    if (!info || !editReady) return;
    setStarting(true);
    setExportError(null);
    try {
      const edit = cutMode
        ? { kind: "cut" as const, removed: cuts.removed }
        : { kind: "trim" as const, start: range.start, end: range.end };
      const item = await api.queueExport({ path: info.path, edit, ...requestOptions(choice), effects: precise ? effects : NO_EFFECTS });
      seenInQueue.current = false;
      setQueued(item);
      setFollowedId(item.id);
    } catch (e) {
      setExportError(toMsg(e));
    } finally {
      setStarting(false);
    }
  };

  const exportProps: ExportProps = {
    info,
    choice,
    onChoice: changeChoice,
    effectsOn,
    picture: precise ? (planned.plan?.size ?? null) : null,
    shown: v ? (planned.plan?.size ?? uprightSize(v)) : null,
    editor: mode,
    encoders: exportEncoders,
    length: resultLength,
    whole,
    canExport,
    item: followed,
    starting,
    error: exportError,
    onExport: runExport,
  };

  return (
    <div className="ed ed-split">
      <TopBar
        kind={mode}
        onKind={onKind}
        queueCount={queueCount}
        queueCurrent={false}
        onOpenQueue={onOpenQueue}
        onOpenSettings={onOpenSettings}
        left={
          info && (
            <div className="ed-file">
              <b title={info.path}>{info.fileName}</b>
              <span>{mediaSummary(t, info)}</span>
            </div>
          )
        }
      >
        <button type="button" className="ed-btn" onClick={open} disabled={!canOpen}>
          <FolderIcon />
          {opening ? t.editor.reading : t.editor.open}
        </button>
      </TopBar>

      <div className="ed-body">
        <main className="ed-main">
          {info && openError && <p className="error">{say(t, openError)}</p>}
          <Stage
            videoRef={videoRef}
            info={info}
            src={src}
            onProxy={onProxy}
            original={original}
            proxyStatus={proxyStatus}
            proxy={proxy}
            player={player}
            ffmpeg={ffmpeg}
            opening={opening}
            openError={openError}
            canOpen={canOpen}
            drop={dragged && { paths: dragged, blocked: dropBlocked }}
            onOpen={open}
            onOpenSettings={onOpenSettings}
            onLoadedMetadata={onLoadedMetadata}
            onError={onError}
            fx={
              v
                ? {
                    effects,
                    plan: planned.plan,
                    view: stageView,
                    onView: setStageView,
                    onCrop: (crop) => changeEffects({ ...effects, crop }),
                    maskSel,
                    onMaskSelect: setMaskSel,
                    onMaskChange: (i, rect) => changeMask(i, rect),
                    sources: overlaySources,
                    overlaySel,
                    onOverlaySelect: setOverlaySel,
                    onOverlayChange: (i, rect) => changeOverlay(i, rect),
                  }
                : null
            }
          />
          <Transport
            player={player}
            duration={duration}
            enabled={!!info}
            mode={mode}
            onMarkIn={markIn}
            onMarkOut={markOut}
            onAdd={nextCut ? addCut : null}
          />
          {probe ? (
            <Timeline
              key={probe.info.path}
              info={probe.info}
              keyframes={probe.keyframes}
              now={player.now}
              range={problem ? null : range}
              onSeek={seek}
              onTrimDrag={dragTrim}
              mode={mode}
              cuts={cuts.removed}
              selected={cuts.selected}
              marks={marks}
              onCutSelect={selectCut}
              onCutDrag={dragCut}
              onCutDragEnd={dragCutEnd}
              masks={masks}
              maskSel={maskSel}
              onMaskSelect={setMaskSel}
              onMaskTime={(i, edge, t) => {
                pause();
                changeMask(i, edge === "start" ? { start: t } : { end: t });
                seek(t);
              }}
              overlays={overlays.map((o) => ({ start: o.start, end: o.end, image: overlaySources[o.path]?.kind === "image" }))}
              overlaySel={overlaySel}
              onOverlaySelect={setOverlaySel}
              onOverlayTime={(i, edge, t) => {
                pause();
                changeOverlay(i, edge === "start" ? { start: t } : { end: t });
                seek(t);
              }}
            />
          ) : (
            <section className="ed-timeline empty" aria-label={t.timeline.title}>
              <span className="ed-tl-title">{t.timeline.title}</span>
              <p className="muted small">{t.editor.timelineEmpty}</p>
            </section>
          )}
        </main>

        <aside className="ed-aside" aria-label={t.editor.aside}>
          <div className="ed-aside-scroll">
          {cutMode ? (
            <CutSection
              enabled={v !== null}
              noVideo={info !== null && v === null}
              duration={duration}
              removed={cuts.removed}
              keep={keep}
              selected={cuts.selected}
              marks={marks}
              next={nextCut}
              now={player.now}
              onSelect={(i) => selectCut(i, true)}
              onDelete={deleteCut}
              onAdd={addCut}
              onClearMarks={() => setMarks(NO_MARKS)}
              lossless={losslessCut}
              bFrames={(v?.hasBFrames ?? 0) > 0}
              onPrecise={toPrecise}
            />
          ) : (
          <TrimSection
            enabled={!!info}
            start={range.start}
            end={range.end}
            duration={duration}
            onStart={(t) => setRange((r) => ({ ...r, start: typedFrameStart(t, v) }))}
            onEnd={(t) => setRange((r) => ({ ...r, end: Math.min(duration, typedFrameStart(t, v)) }))}
            lossless={lossless}
            problem={problem}
            precise={precise}
            bFrames={(v?.hasBFrames ?? 0) > 0}
            onPrecise={toPrecise}
          />
          )}
          <EffectsSection
            video={v}
            hasFile={info !== null}
            effects={effects}
            remembered={rememberedCrop}
            planned={planned}
            note={fxNote}
            onChange={changeEffects}
          />
          <MaskSection
            video={v}
            hasFile={info !== null}
            duration={duration}
            now={player.now}
            masks={masks}
            selected={maskSel}
            remembered={rememberedMasks.length}
            onAdd={addMask}
            onSelect={setMaskSel}
            onDelete={deleteMask}
            onChange={changeMask}
            onRestore={restoreMasks}
          />
          <OverlaySection
            video={v}
            hasFile={info !== null}
            hasAudio={(info?.audio.length ?? 0) > 0}
            duration={duration}
            now={player.now}
            overlays={overlays}
            orientation={effects}
            sources={overlaySources}
            selected={overlaySel}
            remembered={rememberedOverlays.length}
            adding={addingOverlay}
            error={overlayError}
            onAdd={() => void addOverlay()}
            onSelect={setOverlaySel}
            onDelete={deleteOverlay}
            onChange={changeOverlay}
            onRestore={restoreOverlays}
          />
          <div className="ed-divider" />
          <ExportSection {...exportProps} />
          </div>
          <div className="ed-aside-foot">
            <ExportFooter {...exportProps} />
          </div>
        </aside>
      </div>
    </div>
  );
}

/**
 * The range 추가 (A) adds: the I · O marks; one mark and the playhead; or DEFAULT_CUT_LENGTH from
 * the playhead. Cut points are frame starts (the file's end stays the end); null when less than a
 * frame would be added.
 */
function cutToAdd(marks: Marks, now: number, duration: number, v: VideoStream | null): { range: Range; fromMarks: boolean } | null {
  if (!(duration > 0)) return null;
  const p = Math.min(duration, frameStart(now, v));
  let start: number;
  let end: number;
  if (marks.start !== null && marks.end !== null) [start, end] = [marks.start, marks.end];
  else if (marks.start !== null) [start, end] = [marks.start, p > marks.start ? p : marks.start + DEFAULT_CUT_LENGTH];
  else if (marks.end !== null) [start, end] = [p < marks.end ? p : marks.end - DEFAULT_CUT_LENGTH, marks.end];
  else [start, end] = [p, p + DEFAULT_CUT_LENGTH];
  start = Math.max(0, frameStart(Math.max(0, start), v));
  end = end >= duration ? duration : frameStart(end, v);
  // Less than the frame at `start` (on a variable rate frames differ in length).
  if (end - start < (stepFrame(start, 1, v) - start) * 0.999) return null;
  return { range: { start, end }, fromMarks: marks.start !== null || marks.end !== null };
}
