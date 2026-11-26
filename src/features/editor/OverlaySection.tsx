import { useEffect, useState } from "react";
import { useT } from "../../lib/i18n/I18n";
import { say } from "../../lib/i18n/text";
import type { Msg, Overlay, OverlaySource, VideoStream } from "../../lib/ipc";
import { formatTime, parseTime } from "../../lib/time";
import { cornerBox, MAX_OVERLAYS, type Orientation, seenSize, toSeen, uprightSize } from "./effects";
import { formatClock, frameStart, stepFrame, typedFrameStart } from "./format";
import { InfoIcon, PlusIcon, TrashIcon } from "./icons";

interface Props {
  video: VideoStream | null;
  hasFile: boolean;
  /** The source has sound to mix into. */
  hasAudio: boolean;
  duration: number;
  now: number;
  overlays: Overlay[];
  /** How the picture is turned and mirrored: the presets and sizes are the screen's. */
  orientation: Orientation;
  sources: Record<string, OverlaySource>;
  selected: number | null;
  remembered: number;
  adding: boolean;
  error: Msg | null;
  onAdd: () => void;
  onSelect: (index: number) => void;
  onDelete: (index: number) => void;
  onChange: (index: number, patch: Partial<Overlay>) => void;
  onRestore: () => void;
}

/**
 * 영상 · 이미지 올리기 (canvas Editor.dc.html): the files put over the picture, the selected one's
 * position preset, opacity, times, where its video starts and its sound. The box is dragged on the
 * stage, the times on the timeline's 올리기 row.
 */
export function OverlaySection(p: Props) {
  const t = useT();
  const w = t.overlays;
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
  const src = uprightSize(v);
  const sel = p.selected !== null ? p.overlays[p.selected] : undefined;
  const o = p.orientation;
  const seen = sel ? toSeen(sel, src, o) : null;
  const source = sel ? p.sources[sel.path] : undefined;
  const full = p.overlays.length >= MAX_OVERLAYS;
  const kind = (path: string) => (p.sources[path]?.kind === "image" ? w.image : w.video);
  const change = (patch: Partial<Overlay>) => p.selected !== null && p.onChange(p.selected, patch);
  const setTime = (edge: "start" | "end", value: number) => {
    if (!sel) return;
    const f = (x: number) => Math.max(0, Math.min(p.duration, x));
    if (edge === "start") {
      const start = f(value);
      change(start < sel.end ? { start } : { start, end: Math.min(p.duration, stepFrame(start, 1, v)) });
    } else {
      const end = value >= p.duration ? p.duration : f(value);
      change(end > sel.start ? { end } : { end, start: Math.max(0, stepFrame(end, -1, v)) });
    }
  };

  return (
    <div className="ed-section">
      {head}
      {p.remembered > 0 && p.overlays.length === 0 && (
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
      {p.overlays.length > 0 ? (
        <ol className="ed-cuts" aria-label={w.list}>
          {p.overlays.map((o, i) => (
            <li key={i} className={i === p.selected ? "ed-cut-row selected overlay" : "ed-cut-row overlay"}>
              <button type="button" className="ed-cut-pick" aria-pressed={i === p.selected} onClick={() => p.onSelect(i)}>
                <span className="ed-cut-no overlay">
                  {p.sources[o.path]?.kind === "image" ? "▣" : "▶"} {i + 1}
                </span>
                <span className="ed-cut-text">
                  <span className="ed-overlay-name">{w.line(p.sources[o.path]?.fileName ?? o.path, kind(o.path))}</span>
                  <span className="mono ed-cut-len">
                    {formatTime(o.start)} → {formatTime(o.end)}
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
        <div className="ed-mask-detail" role="group" aria-label={w.list}>
          <div className="ed-group">
            <span className="ed-label">{w.position}</span>
            <div className="ed-seg tight ed-corners">
              {w.corners.map((label, c) => (
                <button key={c} type="button" title={label} aria-label={label} onClick={() => change(cornerBox(src, o, sel, c as 0 | 1 | 2 | 3 | 4))}>
                  {label.split(" ")[0]}
                </button>
              ))}
            </div>
            {seen && <span className="ed-help">{w.sizeNote(Math.round((seen.width / seenSize(src, o.turn)[0]) * 100), seen.width, seen.height)}</span>}
          </div>
          <label className="ed-strength overlay">
            <span className="ed-strength-head">
              <span>{w.opacity}</span>
              <span className="mono">{sel.opacity} %</span>
            </span>
            <input type="range" min={0} max={100} step={1} value={sel.opacity} onChange={(e) => change({ opacity: Number(e.target.value) })} />
          </label>
          <div className="ed-mask-times">
            <TimeRow label={w.start} value={sel.start} max={p.duration} onCommit={(x) => setTime("start", typedFrameStart(x, v))} onPlayhead={() => setTime("start", frameStart(p.now, v))} />
            <TimeRow label={w.end} value={sel.end} max={p.duration} onCommit={(x) => setTime("end", x >= p.duration ? p.duration : typedFrameStart(x, v))} onPlayhead={() => setTime("end", frameStart(p.now, v))} />
            {source?.kind === "video" && (
              <>
                <TimeRow label={w.offset} value={sel.offset} max={Math.max(0, (source.duration ?? 0) - 0.001)} onCommit={(x) => change({ offset: Math.max(0, x) })} />
                {source.duration !== null && <span className="ed-help">{w.endsNote(formatClock(source.duration))}</span>}
              </>
            )}
          </div>
          {source?.kind === "video" && (
            <div className="ed-group">
              <div className="ed-fx-line">
                <span className="ed-label">{w.sound}</span>
                <button
                  type="button"
                  role="switch"
                  aria-checked={sel.audio && source.hasAudio && p.hasAudio}
                  className="ed-switch"
                  disabled={!source.hasAudio || !p.hasAudio}
                  onClick={() => change({ audio: !sel.audio })}
                >
                  <span aria-hidden="true" />
                  {sel.audio && source.hasAudio && p.hasAudio ? w.on : w.off}
                </button>
              </div>
              {!source.hasAudio ? (
                <span className="ed-help">{w.noSound}</span>
              ) : !p.hasAudio ? (
                <span className="ed-help">{w.noBaseSound}</span>
              ) : (
                sel.audio && (
                  <label className="ed-strength overlay">
                    <span className="ed-strength-head">
                      <span>{w.volume}</span>
                      <span className="mono">{sel.volume} %</span>
                    </span>
                    <input type="range" min={0} max={100} step={1} value={sel.volume} onChange={(e) => change({ volume: Number(e.target.value) })} />
                  </label>
                )
              )}
            </div>
          )}
        </div>
      )}
      {p.error && <p className="warn-text small">{say(t, p.error)}</p>}
      <button type="button" className="ed-add" disabled={full || p.adding} title={full ? w.full(MAX_OVERLAYS) : undefined} onClick={p.onAdd}>
        <PlusIcon />
        {p.adding ? w.adding : w.add(MAX_OVERLAYS)}
      </button>
    </div>
  );
}

/** A time typed (Enter or leaving commits, Escape restores), with a 재생 위치 button when given. */
function TimeRow({ label, value, max, onCommit, onPlayhead }: { label: string; value: number; max: number; onCommit: (t: number) => void; onPlayhead?: () => void }) {
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
        {onPlayhead && (
          <button type="button" className="ed-mask-now" title={t.masks.toPlayheadTitle} onClick={onPlayhead}>
            {t.masks.toPlayhead}
          </button>
        )}
      </span>
      {bad && <span className="warn-text">{t.trim.badTime}</span>}
    </label>
  );
}
