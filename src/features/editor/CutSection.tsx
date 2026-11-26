import { useT } from "../../lib/i18n/I18n";
import type { Messages } from "../../lib/i18n/ko";
import { say } from "../../lib/i18n/text";
import { formatTime } from "../../lib/time";
import { type Range, total } from "./cutList";
import { CloseIcon, InfoIcon, PlusIcon, TrashIcon } from "./icons";
import { type LosslessCutPreview, removalOutcomes } from "./losslessCut";

/** Start / end marked with I · O in 구간 제거 mode, not yet added. */
export interface Marks {
  start: number | null;
  end: number | null;
}

export const NO_MARKS: Marks = { start: null, end: null };

interface Props {
  /** A file with video is open. */
  enabled: boolean;
  /** The open file has no video track (구간 제거 needs one, lossless or precise). */
  noVideo: boolean;
  duration: number;
  removed: Range[];
  /** What stays (cutList.keepFromRemoved). */
  keep: Range[];
  selected: number | null;
  marks: Marks;
  /** What 추가 (A) will add now, and whether it comes from the marks or the playhead. */
  next: { range: Range; fromMarks: boolean } | null;
  now: number;
  onSelect: (index: number) => void;
  onDelete: (index: number) => void;
  onAdd: () => void;
  onClearMarks: () => void;
  /** What a lossless export does (every piece from its keyframe); null when exporting precisely. */
  lossless: LosslessCutPreview | null;
  /** The source has B-frames: each lossless piece can end a few frames past its OUT. */
  bFrames: boolean;
  /** Switch the export to 정밀; null while it cannot be chosen (no video, exporting). */
  onPrecise: (() => void) | null;
}

/** 구간 제거 mode (canvas mockup Editor.dc.html): the ranges to remove and what is left. */
export function CutSection(props: Props) {
  const { enabled, noVideo, duration, removed, keep, selected, marks, next, now, onSelect, onDelete, onAdd, onClearMarks, lossless, bFrames, onPrecise } = props;
  // Lossless (once something is removed): the pieces start at their keyframes, so what stays is a
  // little longer. With nothing removed the one "piece" is the whole file: nothing to explain yet.
  const copied = lossless && removed.length > 0 && lossless.pieces.length > 0 ? lossless : null;
  const kept = copied ? copied.length : total(keep);
  const pieces = copied ? copied.pieces.length : keep.length;
  const marked = marks.start !== null || marks.end !== null;
  const outcomes = copied ? removalOutcomes(removed, copied) : null;
  const notRemoved = outcomes ? outcomes.flatMap((o, i) => (o.kept ? [i + 1] : [])) : [];
  const t = useT();
  const w = t.cut;
  return (
    <div className="ed-section">
      <div className="ed-section-head">
        <h2 className="ed-h2">{w.title}</h2>
        <p className="muted">{w.sub}</p>
      </div>

      {removed.length > 0 ? (
        <ol className="ed-cuts" aria-label={w.listLabel}>
          {removed.map((r, i) => (
            <li key={i} className={i === selected ? "ed-cut-row selected" : "ed-cut-row"}>
              <button
                type="button"
                className="ed-cut-pick"
                aria-pressed={i === selected}
                title={w.pickTitle}
                onClick={() => onSelect(i)}
              >
                <span className="ed-cut-no">#{i + 1}</span>
                <span className="ed-cut-text">
                  <span className="mono ed-cut-span">
                    {compactTime(r.start)} → {compactTime(r.end)}
                  </span>
                  <span className="mono ed-cut-len">{lengthText(t, r.end - r.start)}</span>
                  {outcomes?.[i]?.kept ? (
                    <span className="ed-cut-note warn">{w.cannotRemove}</span>
                  ) : outcomes && outcomes[i].back > 0.0005 ? (
                    <span className="ed-cut-note">{w.endStays(outcomes[i].back.toFixed(3))}</span>
                  ) : null}
                </span>
              </button>
              <button type="button" className="ed-cut-del" aria-label={w.deleteLabel(i + 1)} title={w.deleteTitle} onClick={() => onDelete(i)}>
                <TrashIcon />
              </button>
            </li>
          ))}
        </ol>
      ) : (
        enabled && (
          <p className="muted small">
            {w.empty[0]}
            <kbd className="ed-kbd">I</kbd>
            {w.empty[1]}
            <kbd className="ed-kbd">O</kbd>
            {w.empty[2]}
            <kbd className="ed-kbd">A</kbd>
            {w.empty[3]}
          </p>
        )
      )}

      {marked && (
        <div className="ed-mark-row">
          <span className="ed-label">{w.marked}</span>
          <span className="mono">
            {marks.start !== null ? formatTime(marks.start) : w.noStart} → {marks.end !== null ? formatTime(marks.end) : w.noEnd}
          </span>
          <button type="button" className="ed-cut-del small" aria-label={w.clearMarks} title={w.clearMarks} onClick={onClearMarks}>
            <CloseIcon />
          </button>
        </div>
      )}

      <button type="button" className="ed-add" disabled={!enabled || !next} onClick={onAdd} aria-keyshortcuts="A">
        <PlusIcon />
        {next?.fromMarks ? w.addMarked(formatTime(next.range.start), formatTime(next.range.end)) : w.addAtPlayhead(formatTime(now))}
      </button>

      <dl className="ed-sum mono">
        <dt>{w.original}</dt>
        <dd>{enabled ? formatTime(duration) : "—"}</dd>
        <dt>{w.removedCount(removed.length)}</dt>
        <dd className="ed-sum-cut">{enabled ? `−${formatTime(Math.max(0, duration - kept))}` : "—"}</dd>
        <dt className="ed-sum-total">{w.result(pieces, !!lossless)}</dt>
        <dd className="ed-sum-total">{enabled ? formatTime(kept) : "—"}</dd>
      </dl>

      {noVideo && <p className="warn-text small">{w.noVideo}</p>}
      {enabled && removed.length > 0 && keep.length === 0 && <p className="warn-text small">{say(t, { code: "removesEverything" })}</p>}
      {enabled && !lossless && (
        <div className="ed-note info">
          <InfoIcon />
          <div className="ed-note-body">
            <span>{w.precise}</span>
          </div>
        </div>
      )}
      {enabled && copied && (
        <div className="ed-note">
          <InfoIcon />
          <div className="ed-note-body">
            {copied.exactStart ? (
              <span>
                {w.editList[0]}
                <b className="mono">{formatTime(copied.pieces[0].keep.start)}</b>
                {w.editList[1]}
              </span>
            ) : (
              <span>{w.pulled(copied.pieces.length)}</span>
            )}
            {notRemoved.length > 0 && <span className="warn-text">{w.notRemoved(notRemoved.map((n) => `#${n}`).join(" · "))}</span>}
            {bFrames && <span>{w.bFrames}</span>}
            {copied.problem && <span className="warn-text">{say(t, copied.problem)}</span>}
            <button type="button" className="ed-link" disabled={!onPrecise} onClick={onPrecise ?? undefined}>
              {w.toPrecise}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

/** "0:02.000" — formatTime without the leading zero of the minutes, so a row fits the side panel. */
function compactTime(t: number): string {
  return formatTime(t).replace(/^0(\d:)/, "$1");
}

/** "1.033초" under a minute, else "1:05.400". */
function lengthText(t: Messages, sec: number): string {
  return sec < 59.9995 ? t.queueView.elapsed.s(sec.toFixed(3)) : compactTime(sec);
}
