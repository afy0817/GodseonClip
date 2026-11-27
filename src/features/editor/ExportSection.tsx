import { useState } from "react";
import { useT } from "../../lib/i18n/I18n";
import { say } from "../../lib/i18n/text";
import type { MediaInfo, Msg, OutputCodec, QueueItem, VideoStream } from "../../lib/ipc";
import type { Messages } from "../../lib/i18n/ko.ts";
import { NumberField } from "./EffectsSection";
import {
  type EditorMode,
  encoderFor,
  encoderLine,
  type ExportChoice,
  missingEncoderText,
  predictedExportName,
  qualities,
  saveNote,
  targetCodec,
} from "./exportOptions";
import { codecLabel, estimateCopySize } from "./format";
import { clampRate, GIF_MAX_FRAME_RATE, gifSizeButtons, MAX_FRAME_RATE, rateButtons, rateNote, rateText, sourceRate } from "./frameRate";
import type { ExportEncodersState } from "./useExportEncoders";

export interface ExportProps {
  info: MediaInfo | null;
  choice: ExportChoice;
  onChoice: (c: ExportChoice) => void;
  /** 화면 효과 are on: the export is 정밀 (무손실 would turn them off). */
  effectsOn: boolean;
  /** The result's size with 화면 효과 (정밀); null: the source's. */
  picture: [number, number] | null;
  /** The picture's size after the 화면 효과 (the upright source's without them): what GIF 크기 makes smaller. */
  shown: [number, number] | null;
  /** 구간 제거 is named `_cut` (lossless: pieces joined by stream copy). */
  editor: EditorMode;
  encoders: ExportEncodersState;
  /** Playing length of the result; null while the range is invalid. */
  length: number | null;
  /** The range is the whole file (a conversion: no `_trim` in the name). */
  whole: boolean;
  canExport: boolean;
  /** The queue item of this file's last export (its progress is in the header's notifications). */
  item: QueueItem | null;
  /** The export is being planned and queued. */
  starting: boolean;
  error: Msg | null;
  onExport: () => void;
}

/**
 * Export: lossless stream copy, or precise re-encoding to the same codec, H.265, AV1 or a GIF (canvas
 * Editor.dc.html), with 프레임레이트 (and GIF 크기) when re-encoding. The choices scroll with the
 * panel; what the result will be and 내보내기 are its fixed foot (`ExportFooter`, the same props).
 * 내보내기 adds a 작업 큐 item — its progress, cancel and result are in the header's notification
 * center (features/notify) — and pressing it again while the item runs adds another one.
 */
export function ExportSection(props: ExportProps) {
  const { info, choice, onChoice, editor, encoders } = props;
  const t = useT();
  const w = t.exportPanel;
  const v = info?.video ?? null;
  const cut = editor === "cut";
  const precise = choice.mode === "precise";
  const gif = choice.codec === "gif";

  const set = (patch: Partial<ExportChoice>) => onChoice({ ...choice, ...patch });
  // H.265 · AV1 need re-encoding; going back to lossless keeps the source codec (as in the mockup).
  const pickCodec = (codec: OutputCodec) => set(codec === "source" ? { codec } : { codec, mode: "precise" });
  // Known after the first encoder test: a codec with no working encoder cannot be picked.
  const unavailable = (codec: OutputCodec): string | null => {
    if (!encoders.encoders || !v) return null;
    const target = targetCodec(codec, v.codec);
    return encoderFor(encoders.encoders, target) ? null : missingEncoderText(t, target);
  };
  const line = info ? encoderLine(t, info, choice, encoders.encoders) : null;
  // A GIF needs no encoder test: its line is there at once.
  const codecHelp = !precise
    ? w.codecHint
    : gif
      ? (line ?? "")
      : encoders.loading && !encoders.encoders
        ? w.checkingEncoders
        : encoders.error
          ? say(t, encoders.error)
          : (line ?? "");

  return (
    <div className="ed-section">
      <h2 className="ed-h3">{w.title}</h2>

      <div className="ed-group" role="group" aria-label={w.mode}>
        <span className="ed-label">{w.mode}</span>
        <div className="ed-seg">
          <button type="button" aria-pressed={!precise} onClick={() => set({ mode: "lossless", codec: "source" })}>
            <b>{w.lossless}</b>
            <span>{w.losslessUse}</span>
          </button>
          <button
            type="button"
            aria-pressed={precise}
            disabled={!v}
            title={!v && info ? w.noVideoTitle : undefined}
            onClick={() => set({ mode: "precise" })}
          >
            <b>{w.precise}</b>
            <span>{w.preciseUse}</span>
          </button>
        </div>
      </div>

      <div className="ed-group" role="group" aria-label={w.codec}>
        <span className="ed-label">{w.codec}</span>
        <div className="ed-seg codecs">
          <button type="button" aria-pressed={choice.codec === "source"} onClick={() => pickCodec("source")}>
            <b>{w.keepCodec}</b>
            <span>{v ? codecLabel(precise ? targetCodec("source", v.codec) : v.codec) : t.queueView.streamCopy}</span>
          </button>
          {(["hevc", "av1"] as const).map((c) => {
            const why = unavailable(c);
            return (
              <button
                key={c}
                type="button"
                aria-pressed={choice.codec === c}
                disabled={!v || why !== null}
                title={why ?? undefined}
                onClick={() => pickCodec(c)}
              >
                <b>{c === "hevc" ? "H.265" : "AV1"}</b>
                <span>{c === "hevc" ? "HEVC" : w.royaltyFree}</span>
              </button>
            );
          })}
          <button type="button" aria-pressed={gif} disabled={!v} onClick={() => pickCodec("gif")}>
            <b>GIF</b>
            <span>{w.gifUse}</span>
          </button>
        </div>
        <span className="ed-help">
          {codecHelp}
          {cut && !v && info ? w.noVideoCut : null}
        </span>
        {precise && props.effectsOn && <span className="ed-help">{w.effectsPrecise}</span>}
      </div>

      {precise && (
        <div className="ed-group" role="group" aria-label={w.quality}>
          <span className="ed-label">{w.quality}</span>
          <div className="ed-seg">
            {qualities(t).map((q) => (
              <button key={q.value} type="button" aria-pressed={choice.quality === q.value} onClick={() => set({ quality: q.value })}>
                <b>{q.label}</b>
                <span>{q.use}</span>
              </button>
            ))}
          </div>
          {gif && <span className="ed-help">{w.gifQuality[choice.quality]}</span>}
        </div>
      )}

      {/* A video and a GIF keep their own rate: 직접's typing does not carry over either. */}
      {precise && v && <FrameRateGroup key={gif ? "gif" : "video"} t={t} v={v} choice={choice} set={set} />}

      {precise && gif && props.shown && <GifSizeGroup t={t} shown={props.shown} short={choice.gifShort} onShort={(gifShort) => set({ gifShort })} />}
    </div>
  );
}

/**
 * 프레임레이트: 원본 · a few caps · 직접 (a typed number). A video and a GIF keep their own choice
 * (a GIF wants far fewer frames). A cap at or above the source's rate does nothing, so its button is
 * off, as 출력 크기's presets that would enlarge are.
 */
function FrameRateGroup({ t, v, choice, set }: { t: Messages; v: VideoStream; choice: ExportChoice; set: (p: Partial<ExportChoice>) => void }) {
  const w = t.exportPanel;
  const gif = choice.codec === "gif";
  const [typing, setTyping] = useState(false);
  const rate = gif ? choice.gifRate : choice.frameRate;
  const setRate = (r: number | null) => set(gif ? { gifRate: r } : { frameRate: r });
  const b = rateButtons(v, rate, gif, typing);
  const source = sourceRate(v);
  const pick = (r: number | null) => {
    setTyping(false);
    setRate(r);
  };
  return (
    <div className="ed-group" role="group" aria-label={w.frameRate}>
      <span className="ed-label">{w.frameRate}</span>
      <div className="ed-seg tight">
        <button type="button" aria-pressed={b.sourcePressed} onClick={() => pick(null)}>
          {w.rateSource}
        </button>
        {b.presets.map((p) => (
          <button
            key={p.value}
            type="button"
            aria-pressed={p.pressed}
            disabled={p.disabled}
            title={p.disabled && source !== null ? w.rateTooHigh(rateText(source)) : undefined}
            onClick={() => pick(p.value)}
          >
            {p.value}
          </button>
        ))}
        <button
          type="button"
          aria-pressed={b.customPressed}
          onClick={
            b.customPressed
              ? undefined
              : () => {
                  setTyping(true);
                  setRate(b.start);
                }
          }
        >
          {w.rateCustom}
        </button>
      </div>
      {b.customPressed && (
        <div className="ed-fx-size">
          <NumberField label={w.rateField} value={rate ?? b.start} onCommit={(n) => setRate(clampRate(n, gif))} />
          <span aria-hidden="true">fps</span>
        </div>
      )}
      <span className="ed-help">{rateNote(t, v, rate, gif)}</span>
      {b.customPressed && <span className="ed-help">{w.rateRange(1, gif ? GIF_MAX_FRAME_RATE : MAX_FRAME_RATE)}</span>}
    </div>
  );
}

/** GIF 크기: 원본 · short side 480 / 360 / 240 (never larger), from the picture the 화면 효과 made. */
function GifSizeGroup({ t, shown, short, onShort }: { t: Messages; shown: [number, number]; short: number | null; onShort: (s: number | null) => void }) {
  const w = t.exportPanel;
  const b = gifSizeButtons(shown, short);
  return (
    <div className="ed-group" role="group" aria-label={w.gifSize}>
      <span className="ed-label">{w.gifSize}</span>
      <div className="ed-seg tight">
        <button type="button" aria-pressed={b.sourcePressed} onClick={() => onShort(null)}>
          {w.rateSource}
        </button>
        {b.presets.map((p) => (
          <button
            key={p.value}
            type="button"
            aria-pressed={p.pressed}
            disabled={p.disabled}
            title={p.disabled ? w.gifSizeTooBig(b.side, p.value) : undefined}
            onClick={() => onShort(p.value)}
          >
            {`${p.value}p`}
          </button>
        ))}
      </div>
      <span className="ed-help">{w.gifSizeNote(b.size[0], b.size[1])}</span>
    </div>
  );
}

/** The panel's fixed foot: the result's name and what to expect, 내보내기. */
export function ExportFooter(props: ExportProps) {
  const { info, choice, editor, length, whole, canExport, item, starting, error } = props;
  const t = useT();
  const w = t.exportPanel;
  const precise = choice.mode === "precise";
  const name = info ? predictedExportName(info, choice, whole, editor) : "—";
  const size = info && length !== null && !precise ? estimateCopySize(info, length) : null;
  const note = info ? saveNote(t, info, choice, props.picture, props.shown, size) : precise ? w.saveNotePrecise : w.saveNoteLossless(null);
  const unfinished = item !== null && (item.state === "waiting" || item.state === "running");
  return (
    <>
      <div className="ed-save">
        <span className="ed-label">{w.saveTo}</span>
        <span className="mono ed-save-name">{name}</span>
        <span className="ed-label">{note}</span>
      </div>

      <button type="button" className="ed-export" disabled={!canExport || starting} onClick={props.onExport}>
        {starting ? w.queuing : w.export}
      </button>
      {unfinished && <span className="ed-help">{w.again}</span>}
      {error && <p className="error small">{say(t, error)}</p>}
    </>
  );
}
