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
  targetCodec,
} from "./exportOptions";
import { codecLabel, estimateCopySize } from "./format";
import {
  clampRate,
  GIF_MAX_FRAME_RATE,
  GIF_RATES,
  GIF_SHORT_SIDES,
  gifScale,
  MAX_FRAME_RATE,
  ratePossible,
  rateText,
  resultRate,
  sourceRate,
  VIDEO_RATES,
} from "./frameRate";
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

      {precise && v && <FrameRateGroup t={t} v={v} choice={choice} set={set} />}

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
  const presets = gif ? GIF_RATES : VIDEO_RATES;
  const rate = gif ? choice.gifRate : choice.frameRate;
  const setRate = (r: number | null) => set(gif ? { gifRate: r } : { frameRate: r });
  const custom = typing || (rate !== null && !presets.includes(rate));
  const source = sourceRate(v);
  const sourceText = source !== null ? rateText(source) : null;
  const out = resultRate(v, rate, gif);
  const sourcePressed = !custom && (rate === null || !ratePossible(v, rate, gif));
  const pick = (r: number | null) => {
    setTyping(false);
    setRate(r);
  };
  let note: string;
  if (gif) note = rate === null && source !== null && source > GIF_MAX_FRAME_RATE ? w.rateGifMax(sourceText ?? "") : w.rateGif(rateText(out ?? 0), sourceText);
  else note = out === null ? w.rateKept(sourceText) : w.rateDown(rateText(out), sourceText);
  return (
    <div className="ed-group" role="group" aria-label={w.frameRate}>
      <span className="ed-label">{w.frameRate}</span>
      <div className="ed-seg tight">
        <button type="button" aria-pressed={sourcePressed} onClick={() => pick(null)}>
          {w.rateSource}
        </button>
        {presets.map((n) => {
          const possible = ratePossible(v, n, gif);
          return (
            <button
              key={n}
              type="button"
              aria-pressed={possible && !custom && rate === n}
              disabled={!possible}
              title={possible ? undefined : w.rateTooHigh(sourceText ?? "")}
              onClick={() => pick(n)}
            >
              {n}
            </button>
          );
        })}
        <button
          type="button"
          aria-pressed={custom}
          onClick={
            custom
              ? undefined
              : () => {
                  setTyping(true);
                  setRate(clampRate(out ?? source ?? 30, gif));
                }
          }
        >
          {w.rateCustom}
        </button>
      </div>
      {custom && (
        <div className="ed-fx-size">
          <NumberField label={w.rateField} value={rate ?? clampRate(out ?? source ?? 30, gif)} onCommit={(n) => setRate(clampRate(n, gif))} />
          <span aria-hidden="true">fps</span>
        </div>
      )}
      <span className="ed-help">{note}</span>
      {custom && <span className="ed-help">{w.rateRange(1, gif ? GIF_MAX_FRAME_RATE : MAX_FRAME_RATE)}</span>}
    </div>
  );
}

/** GIF 크기: 원본 · short side 480 / 360 / 240 (never larger), from the picture the 화면 효과 made. */
function GifSizeGroup({ t, shown, short, onShort }: { t: Messages; shown: [number, number]; short: number | null; onShort: (s: number | null) => void }) {
  const w = t.exportPanel;
  const side = Math.min(shown[0], shown[1]);
  const possible = (n: number) => side > n;
  const [fw, fh] = gifScale(shown, short) ?? shown;
  return (
    <div className="ed-group" role="group" aria-label={w.gifSize}>
      <span className="ed-label">{w.gifSize}</span>
      <div className="ed-seg tight">
        <button type="button" aria-pressed={short === null || !possible(short)} onClick={() => onShort(null)}>
          {w.rateSource}
        </button>
        {GIF_SHORT_SIDES.map((n) => (
          <button
            key={n}
            type="button"
            aria-pressed={possible(n) && short === n}
            disabled={!possible(n)}
            title={possible(n) ? undefined : w.gifSizeTooBig(side, n)}
            onClick={() => onShort(n)}
          >
            {`${n}p`}
          </button>
        ))}
      </div>
      <span className="ed-help">{w.gifSizeNote(fw, fh)}</span>
    </div>
  );
}

/** "1280 × 720 · 30fps" — what 정밀 changes about the picture; empty when nothing. */
function pictureNote(t: Messages, size: [number, number] | null, rate: number | null): string {
  const parts: string[] = [];
  if (size) parts.push(`${size[0]} × ${size[1]}`);
  if (rate !== null) parts.push(t.queueView.fps(rateText(rate)));
  return parts.join(" · ");
}

/** The panel's fixed foot: the result's name and what to expect, 내보내기. */
export function ExportFooter(props: ExportProps) {
  const { info, choice, editor, length, whole, canExport, item, starting, error } = props;
  const t = useT();
  const w = t.exportPanel;
  const precise = choice.mode === "precise";
  const gif = choice.codec === "gif";
  const name = info ? predictedExportName(info, choice, whole, editor) : "—";
  const size = info && length !== null && !precise ? estimateCopySize(info, length) : null;
  const v = info?.video ?? null;
  const rate = v && precise ? resultRate(v, gif ? choice.gifRate : choice.frameRate, gif) : null;
  const picture = gif && props.shown ? (gifScale(props.shown, choice.gifShort) ?? props.shown) : props.picture;
  const what = pictureNote(t, picture, rate);
  const note = !precise ? w.saveNoteLossless(size) : gif ? w.saveNoteGif(what) : what ? w.saveNotePreciseWith(what) : w.saveNotePrecise;
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
