import { useEffect, useState } from "react";
import { useT } from "../../lib/i18n/I18n";
import { formatTime, parseTime } from "../../lib/time";
import { InfoIcon } from "./icons";
import type { LosslessPreview } from "./trim";

interface Props {
  enabled: boolean;
  start: number;
  end: number;
  duration: number;
  onStart: (t: number) => void;
  onEnd: (t: number) => void;
  /** What a lossless cut of the current range does; null while the range is invalid. */
  lossless: LosslessPreview | null;
  problem: string | null;
  /** Export mode is 정밀 (re-encode): the cut is frame-exact, no keyframe notes. */
  precise: boolean;
  /** The source has B-frames: a lossless cut can keep a few frames past OUT. */
  bFrames: boolean;
  /** Switch the export to 정밀; null while it cannot be chosen (no video, exporting). */
  onPrecise: (() => void) | null;
}

/** Trim mode: one kept range, typed or set from the playhead. */
export function TrimSection({ enabled, start, end, duration, onStart, onEnd, lossless, problem, precise, bFrames, onPrecise }: Props) {
  const pulled = !precise && lossless !== null && lossless.snapped !== null && lossless.pulledBy > 0.0005;
  const longEnd = !precise && lossless !== null && bFrames;
  const length = problem ? null : precise ? end - start : (lossless?.length ?? null);
  const w = useT().trim;
  return (
    <div className="ed-section">
      <div className="ed-section-head">
        <h2 className="ed-h2">{w.title}</h2>
        <p className="muted">{w.sub}</p>
      </div>
      <div className="ed-fields">
        <TimeField label={w.start} value={start} max={duration} enabled={enabled} onCommit={onStart} />
        <TimeField label={w.end} value={end} max={duration} enabled={enabled} onCommit={onEnd} />
      </div>
      <div className="ed-result">
        <span className="muted">{w.length}</span>
        <span className="mono ed-result-value">{length !== null ? formatTime(length) : "—"}</span>
      </div>
      {problem && <p className="warn-text small">{problem}</p>}
      {precise && !problem && enabled && (
        <div className="ed-note info">
          <InfoIcon />
          <div className="ed-note-body">
            <span>{w.precise}</span>
          </div>
        </div>
      )}
      {(pulled || longEnd) && lossless && (
        <div className="ed-note">
          <InfoIcon />
          <div className="ed-note-body">
            {pulled &&
              (lossless.exactStart ? (
                <span>
                  {w.editList[0]}
                  <b className="mono">{formatTime(lossless.snapped!)}</b>
                  {w.editList[1]}
                </span>
              ) : (
                <span>
                  {w.pulled[0]}
                  <b className="mono">{formatTime(lossless.snapped!)}</b>
                  {w.pulled[1](lossless.pulledBy.toFixed(3))}
                </span>
              ))}
            {longEnd && <span>{w.bFrames}</span>}
            <button type="button" className="ed-link" disabled={!onPrecise} onClick={onPrecise ?? undefined}>
              {w.toPrecise}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

interface FieldProps {
  label: string;
  value: number;
  max: number;
  enabled: boolean;
  onCommit: (t: number) => void;
}

/**
 * Text field over a number of seconds. Typing edits a draft; Enter or leaving the field
 * commits it (clamped to the file), Escape restores the value.
 */
function TimeField({ label, value, max, enabled, onCommit }: FieldProps) {
  const w = useT().trim;
  const [draft, setDraft] = useState(formatTime(value));
  const [bad, setBad] = useState(false);

  useEffect(() => {
    setDraft(formatTime(value));
    setBad(false);
  }, [value]);

  const commit = () => {
    const t = parseTime(draft);
    if (t === null) {
      setBad(true);
      return;
    }
    const clamped = Math.max(0, Math.min(max, t));
    setBad(false);
    setDraft(formatTime(clamped));
    if (Math.abs(clamped - value) > 1e-6) onCommit(clamped);
  };

  return (
    <label className="ed-field">
      {label}
      <input
        type="text"
        className="ed-input"
        value={draft}
        disabled={!enabled}
        spellCheck={false}
        inputMode="decimal"
        aria-invalid={bad}
        onChange={(e) => {
          setDraft(e.target.value);
          setBad(false);
        }}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          else if (e.key === "Escape") {
            setDraft(formatTime(value));
            setBad(false);
          }
        }}
      />
      {bad && <span className="warn-text">{w.badTime}</span>}
    </label>
  );
}
