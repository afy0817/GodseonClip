import { useEffect, useState } from "react";
import { useT } from "../../lib/i18n/I18n";
import type { Messages } from "../../lib/i18n/ko";
import type { Mask, MaskKind, VideoStream } from "../../lib/ipc";
import { formatTime, parseTime } from "../../lib/time";
import { blurSigma, MAX_MASKS, mosaicCell, uprightSize } from "./effects";
import { frameStart, stepFrame, typedFrameStart } from "./format";
import { InfoIcon, PlusIcon, TrashIcon } from "./icons";

interface Props {
  /** The opened file's video; null without a file or without video. */
  video: VideoStream | null;
  hasFile: boolean;
  duration: number;
  /** Playhead (original timeline). */
  now: number;
  masks: Mask[];
  selected: number | null;
  /** Boxes 무손실 turned off, ready to come back. */
  remembered: number;
  onAdd: () => void;
  onSelect: (index: number) => void;
  onDelete: (index: number) => void;
  onChange: (index: number, patch: Partial<Mask>) => void;
  onRestore: () => void;
}

const KINDS: MaskKind[] = ["mosaic", "blur"];

/**
 * 모자이크 · 블러 (canvas Editor.dc.html): the boxes to hide, the selected one's kind, strength and
 * times. The boxes are dragged on the stage, their times on the timeline's 가리기 row.
 */
export function MaskSection(p: Props) {
  const t = useT();
  const w = t.masks;
  const v = p.video;
  const head = (
    <div className="ed-section-head">
      <h2 className="ed-h2">{w.title}</h2>
      <p className="muted">{w.sub}</p>
    </div>
  );
  if (!v) {
    return (
      <div className="ed-section">
        {head}
        {p.hasFile && <span className="ed-help">{t.effects.noVideo}</span>}
      </div>
    );
  }
  const short = Math.min(...uprightSize(v));
  const sel = p.selected !== null ? p.masks[p.selected] : undefined;
  const full = p.masks.length >= MAX_MASKS;
  const kindLabel = (k: MaskKind) => (k === "mosaic" ? w.mosaic : w.blur);
  const setTime = (edge: "start" | "end", value: number) => {
    if (p.selected === null || !sel) return;
    const f = (x: number) => Math.max(0, Math.min(p.duration, x));
    if (edge === "start") {
      const start = f(value);
      // Keep at least a frame: push the end out when the start passes it.
      p.onChange(p.selected, start < sel.end ? { start } : { start, end: Math.min(p.duration, stepFrame(start, 1, v)) });
    } else {
      const end = value >= p.duration ? p.duration : f(value);
      p.onChange(p.selected, end > sel.start ? { end } : { end, start: Math.max(0, stepFrame(end, -1, v)) });
    }
  };

  return (
    <div className="ed-section">
      {head}
      {p.remembered > 0 && p.masks.length === 0 && (
        <div className="ed-note" role="status">
          <InfoIcon />
          <div className="ed-note-body">
            <span>{w.turnedOff(p.remembered)}</span>
            <button type="button" className="ed-link" onClick={p.onRestore}>
              {w.restore}
            </button>
          </div>
        </div>
      )}
      {p.masks.length > 0 ? (
        <ol className="ed-cuts" aria-label={w.list}>
          {p.masks.map((m, i) => (
            <li key={i} className={i === p.selected ? "ed-cut-row selected" : "ed-cut-row"}>
              <button type="button" className="ed-cut-pick" aria-pressed={i === p.selected} onClick={() => p.onSelect(i)}>
                <span className="ed-cut-no">#{i + 1}</span>
                <span className="ed-cut-text">
                  <span>{w.line(kindLabel(m.kind), m.strength)}</span>
                  <span className="mono ed-cut-len">
                    {formatTime(m.start)} → {formatTime(m.end)}
                  </span>
                </span>
              </button>
              <button type="button" className="ed-cut-del" aria-label={w.remove(i + 1)} title={w.remove(i + 1)} onClick={() => p.onDelete(i)}>
                <TrashIcon />
              </button>
            </li>
          ))}
        </ol>
      ) : (
        p.remembered === 0 && <span className="ed-help">{w.empty}</span>
      )}
      {sel && p.selected !== null && (
        <div className="ed-mask-detail" role="group" aria-label={kindLabel(sel.kind)}>
          <div className="ed-group">
            <span className="ed-label">{w.kind(p.selected + 1)}</span>
            <div className="ed-seg">
              {KINDS.map((k) => (
                <button key={k} type="button" aria-pressed={sel.kind === k} onClick={() => p.onChange(p.selected!, { kind: k })}>
                  <b>{kindLabel(k)}</b>
                  <span>{k === "mosaic" ? w.mosaicUse : w.blurUse}</span>
                </button>
              ))}
            </div>
          </div>
          <label className="ed-strength">
            <span className="ed-strength-head">
              <span>{w.strength}</span>
              <span className="mono">{sel.strength}</span>
            </span>
            <input
              type="range"
              min={1}
              max={100}
              step={1}
              value={sel.strength}
              onChange={(e) => p.onChange(p.selected!, { strength: Number(e.target.value) })}
            />
            <span className="ed-strength-foot">
              <span>{w.weak}</span>
              <span>{strengthNote(t, sel, short)}</span>
              <span>{w.strong}</span>
            </span>
          </label>
          <div className="ed-mask-times">
            <MaskTime label={w.start} value={sel.start} max={p.duration} onCommit={(x) => setTime("start", typedFrameStart(x, v))} onPlayhead={() => setTime("start", frameStart(p.now, v))} />
            <MaskTime label={w.end} value={sel.end} max={p.duration} onCommit={(x) => setTime("end", x >= p.duration ? p.duration : typedFrameStart(x, v))} onPlayhead={() => setTime("end", frameStart(p.now, v))} />
          </div>
          <span className="ed-help">{w.help}</span>
        </div>
      )}
      <button type="button" className="ed-add" disabled={full} title={full ? w.full(MAX_MASKS) : undefined} onClick={p.onAdd}>
        <PlusIcon />
        {w.add(MAX_MASKS)}
      </button>
    </div>
  );
}

/** "칸 35px" · "흐림 17px" — in pixels of the original. */
function strengthNote(t: Messages, m: Mask, short: number): string {
  return m.kind === "mosaic" ? t.masks.cellNote(mosaicCell(m.strength, short)) : t.masks.blurNote(blurSigma(m.strength, short));
}

/** A time typed (Enter or leaving commits, Escape restores), and a button that takes the playhead. */
function MaskTime({ label, value, max, onCommit, onPlayhead }: { label: string; value: number; max: number; onCommit: (t: number) => void; onPlayhead: () => void }) {
  const t = useT();
  const [draft, setDraft] = useState(formatTime(value));
  const [bad, setBad] = useState(false);
  useEffect(() => {
    setDraft(formatTime(value));
    setBad(false);
  }, [value]);
  const commit = () => {
    const x = parseTime(draft);
    if (x === null) {
      setBad(true);
      return;
    }
    setBad(false);
    const clamped = Math.max(0, Math.min(max, x));
    setDraft(formatTime(clamped));
    if (Math.abs(clamped - value) > 1e-6) onCommit(clamped);
  };
  return (
    <label className="ed-field">
      {label}
      <span className="ed-mask-time">
        <input
          type="text"
          className="ed-input"
          value={draft}
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
            else if (e.key === "Escape") setDraft(formatTime(value));
          }}
        />
        <button type="button" className="ed-mask-now" title={t.masks.toPlayheadTitle} onClick={onPlayhead}>
          {t.masks.toPlayhead}
        </button>
      </span>
      {bad && <span className="warn-text">{t.trim.badTime}</span>}
    </label>
  );
}
