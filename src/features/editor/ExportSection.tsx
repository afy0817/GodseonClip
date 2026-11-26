import { useT } from "../../lib/i18n/I18n";
import { say } from "../../lib/i18n/text";
import type { MediaInfo, Msg, OutputCodec, QueueItem } from "../../lib/ipc";
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
import type { ExportEncodersState } from "./useExportEncoders";

export interface ExportProps {
  info: MediaInfo | null;
  choice: ExportChoice;
  onChoice: (c: ExportChoice) => void;
  /** 화면 효과 are on: the export is 정밀 (무손실 would turn them off). */
  effectsOn: boolean;
  /** The result's size with 화면 효과 (정밀); null: the source's. */
  picture: [number, number] | null;
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
 * Export: lossless stream copy, or precise re-encoding to the same codec, H.265 or AV1 (canvas
 * Editor.dc.html). The choices scroll with the panel; what the result will be and 내보내기 are its
 * fixed foot (`ExportFooter`, the same props). 내보내기 adds a 작업 큐 item — its progress, cancel
 * and result are in the header's notification center (features/notify) — and pressing it again
 * while the item runs adds another one.
 */
export function ExportSection(props: ExportProps) {
  const { info, choice, onChoice, editor, encoders } = props;
  const t = useT();
  const w = t.exportPanel;
  const v = info?.video ?? null;
  const cut = editor === "cut";
  const precise = choice.mode === "precise";

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
        <div className="ed-seg">
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
        </div>
        <span className="ed-help">
          {precise
            ? encoders.loading && !encoders.encoders
              ? w.checkingEncoders
              : encoders.error
                ? say(t, encoders.error)
                : (line ?? "")
            : w.codecHint}
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
        </div>
      )}

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
  const unfinished = item !== null && (item.state === "waiting" || item.state === "running");
  return (
    <>
      <div className="ed-save">
        <span className="ed-label">{w.saveTo}</span>
        <span className="mono ed-save-name">{name}</span>
        <span className="ed-label">
          {precise ? (props.picture ? w.saveNotePreciseSized(props.picture[0], props.picture[1]) : w.saveNotePrecise) : w.saveNoteLossless(size)}
        </span>
      </div>

      <button type="button" className="ed-export" disabled={!canExport || starting} onClick={props.onExport}>
        {starting ? w.queuing : w.export}
      </button>
      {unfinished && <span className="ed-help">{w.again}</span>}
      {error && <p className="error small">{say(t, error)}</p>}
    </>
  );
}
