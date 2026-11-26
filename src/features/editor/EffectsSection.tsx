import { useEffect, useState } from "react";
import { useT } from "../../lib/i18n/I18n";
import type { Messages } from "../../lib/i18n/ko";
import { say } from "../../lib/i18n/text";
import type { Crop, Effects, Fit, VideoStream } from "../../lib/ipc";
import {
  type BoxField,
  cropOf,
  defaultBox,
  evenRect,
  exactSize,
  fromSeen,
  isNone,
  MAX_SIDE,
  pictureSize,
  MIN_SIDE,
  type Planned,
  presetPossible,
  RATIOS,
  ratioBox,
  sameRatio,
  type Size,
  SHORT_SIDES,
  seenCropSize,
  seenSize,
  toSeen,
  turned,
  typedBox,
  uprightSize,
} from "./effects";
import { FlipHIcon, FlipVIcon, InfoIcon, TurnLeftIcon, TurnRightIcon } from "./icons";

/** Why the section says something above its rows: effects switched the export to 정밀, or 무손실 turned them off. */
export type EffectsNote = "precise" | "off" | null;

interface Props {
  /** The opened file's video; null without a file or without video. */
  video: VideoStream | null;
  hasFile: boolean;
  effects: Effects;
  /** The crop box last used, brought back when crop is turned on again. */
  remembered: Crop | null;
  planned: Planned;
  note: EffectsNote;
  onChange: (e: Effects) => void;
}

const FITS: Fit[] = ["contain", "cover", "stretch"];

/**
 * 화면 (canvas Editor.dc.html): 크롭 (switch, 비율, the box's numbers as seen), 회전 · 뒤집기, 출력
 * 크기 (원본 · 1080p · 720p · 480p · 직접 with 맞춤 / 채움 / 늘림). The box itself is dragged on the
 * stage (CropBox). Every change goes through `onChange`, which switches the export to 정밀.
 */
export function EffectsSection({ video, hasFile, effects: e, remembered, planned, note, onChange }: Props) {
  const t = useT();
  const w = t.effects;
  if (!video) {
    return (
      <div className="ed-section">
        <Head t={t} any={false} onReset={() => {}} />
        {hasFile && <span className="ed-help">{w.noVideo}</span>}
      </div>
    );
  }
  const src = uprightSize(video);
  const frame = seenSize(src, e.turn);
  const cropOn = e.crop !== null;
  const set = (patch: Partial<Effects>) => onChange({ ...e, ...patch });
  const setCrop = (box: { x: number; y: number; width: number; height: number }, ratio: [number, number] | null) =>
    set({ crop: cropOf(evenRect(box, src), ratio) });

  const toggleCrop = () => set({ crop: cropOn ? null : (remembered ?? cropOf(defaultBox(src), null)) });
  const pickRatio = (ratio: [number, number] | null) => {
    if (ratio) setCrop(ratioBox(src, e, ratio), ratio);
    else set({ crop: e.crop ? { ...e.crop, ratio: null } : (remembered ?? cropOf(defaultBox(src), null)) });
  };
  const seenBox = e.crop ? toSeen(e.crop, src, e) : null;
  const typed = (field: BoxField, value: number) => {
    if (!e.crop || !seenBox) return;
    setCrop(fromSeen(typedBox(seenBox, field, value, frame, e.crop.ratio), src, e), e.crop.ratio);
  };

  const [cw, ch] = seenCropSize(src, e);
  const plan = planned.plan;
  const outSize: Size = plan?.size ?? [cw, ch];
  const pickCustom = () => set({ size: exactSize(outSize[0], outSize[1], e.size.kind === "exact" ? e.size.fit : "contain") });
  const exact = e.size.kind === "exact" ? e.size : null;
  const sourcePressed = e.size.kind === "source" || (e.size.kind === "short" && !presetPossible(src, e, e.size.short));

  return (
    <div className="ed-section">
      <Head t={t} any={!isNone({ ...e, masks: [], overlays: [] })} onReset={() => onChange({ ...e, crop: null, turn: 0, flipH: false, flipV: false, size: { kind: "source" } })} />

      {note && (
        <div className={note === "precise" ? "ed-note info" : "ed-note"} role="status">
          <InfoIcon />
          <span>{note === "precise" ? w.toPrecise : w.turnedOff}</span>
        </div>
      )}

      <div className="ed-group" role="group" aria-label={w.crop}>
        <div className="ed-fx-line">
          <span className="ed-label">{w.crop}</span>
          <button type="button" role="switch" aria-checked={cropOn} className="ed-switch" onClick={toggleCrop}>
            <span aria-hidden="true" />
            {cropOn ? w.on : w.off}
          </button>
        </div>
        <div className={cropOn ? "ed-seg tight" : "ed-seg tight dim"} role="group" aria-label={w.ratio}>
          <button type="button" aria-pressed={cropOn && e.crop?.ratio === null} onClick={() => pickRatio(null)}>
            {w.free}
          </button>
          {RATIOS.map((r) => (
            <button key={r.join(":")} type="button" className="mono" aria-pressed={cropOn && sameRatio(e.crop?.ratio ?? null, r)} onClick={() => pickRatio(r)}>
              {r.join(":")}
            </button>
          ))}
        </div>
        {seenBox && (
          <>
            <div className="ed-fx-fields">
              {(["x", "y", "width", "height"] as const).map((f) => (
                <NumberField key={f} label={w[f]} value={Math.round(seenBox[f])} onCommit={(n) => typed(f, n)} />
              ))}
            </div>
            <span className="ed-help">{w.cropHelp}</span>
          </>
        )}
      </div>

      <div className="ed-group" role="group" aria-label={w.turnFlip}>
        <span className="ed-label">{w.turnFlip}</span>
        <div className="ed-seg tight">
          <button type="button" className="ed-icon-btn" aria-label={w.turnLeft} title={w.turnLeft} onClick={() => onChange(turned(e, -1))}>
            <TurnLeftIcon />
            90°
          </button>
          <button type="button" className="ed-icon-btn" aria-label={w.turnRight} title={w.turnRight} onClick={() => onChange(turned(e, 1))}>
            <TurnRightIcon />
            90°
          </button>
          <button type="button" className="ed-icon-btn" aria-label={w.flipHLabel} aria-pressed={e.flipH} onClick={() => set({ flipH: !e.flipH })}>
            <FlipHIcon />
            {w.flipH}
          </button>
          <button type="button" className="ed-icon-btn" aria-label={w.flipVLabel} aria-pressed={e.flipV} onClick={() => set({ flipV: !e.flipV })}>
            <FlipVIcon />
            {w.flipV}
          </button>
        </div>
        <span className="ed-help">{turnText(t, e)}</span>
      </div>

      <div className="ed-group" role="group" aria-label={w.size}>
        <span className="ed-label">{w.size}</span>
        <div className="ed-seg tight">
          <button type="button" aria-pressed={sourcePressed} onClick={() => set({ size: { kind: "source" } })}>
            {w.source}
          </button>
          {SHORT_SIDES.map((n) => {
            const possible = presetPossible(src, e, n);
            return (
              <button
                key={n}
                type="button"
                aria-pressed={possible && e.size.kind === "short" && e.size.short === n}
                disabled={!possible}
                title={possible ? undefined : w.presetTooBig(Math.min(cw, ch), n)}
                onClick={() => set({ size: { kind: "short", short: n } })}
              >
                {`${n}p`}
              </button>
            );
          })}
          <button type="button" aria-pressed={exact !== null} onClick={exact ? undefined : pickCustom}>
            {w.custom}
          </button>
        </div>
        {exact && (
          <>
            <div className="ed-fx-size">
              <NumberField label={w.width} value={exact.width} onCommit={(n) => set({ size: { ...exact, width: side(n) } })} />
              <span aria-hidden="true">×</span>
              <NumberField label={w.height} value={exact.height} onCommit={(n) => set({ size: { ...exact, height: side(n) } })} />
            </div>
            <div className="ed-seg" role="group" aria-label={w.fit}>
              {FITS.map((f) => (
                <button key={f} type="button" aria-pressed={exact.fit === f} onClick={() => set({ size: { ...exact, fit: f } })}>
                  <b>{w[f]}</b>
                  <span>{fitUse(t, f)}</span>
                </button>
              ))}
            </div>
          </>
        )}
        <span className={planned.error ? "warn-text small" : "ed-label"}>{planned.error ? say(t, planned.error) : sizeText(t, e, planned, [cw, ch], src)}</span>
        {exact && <span className="ed-help">{w.sizeRange(MIN_SIDE, MAX_SIDE)}</span>}
      </div>
    </div>
  );
}

function fitUse(t: Messages, f: Fit): string {
  return f === "contain" ? t.effects.containUse : f === "cover" ? t.effects.coverUse : t.effects.stretchUse;
}

/** An output side as typed: whole, even, in range. */
function side(n: number): number {
  return Math.max(MIN_SIDE, Math.min(MAX_SIDE, Math.floor(n / 2) * 2));
}

function Head({ t, any, onReset }: { t: Messages; any: boolean; onReset: () => void }) {
  const w = t.effects;
  return (
    <div className="ed-section-head with-action">
      <div>
        <h2 className="ed-h2">{w.title}</h2>
        <p className="muted">{w.sub}</p>
      </div>
      {any && (
        <button type="button" className="ed-link blue" onClick={onReset}>
          {w.allOff}
        </button>
      )}
    </div>
  );
}

/** "오른쪽으로 90° · 좌우 뒤집음 — 크롭 상자는 보이는 방향 그대로". */
function turnText(t: Messages, e: Effects): string {
  const w = t.effects;
  const turn = ((e.turn % 4) + 4) % 4;
  const parts: string[] = [];
  if (turn) parts.push(w.turnedBy(turn));
  if (e.flipH) parts.push(w.flippedH);
  if (e.flipV) parts.push(w.flippedV);
  return parts.length ? w.turnNote(parts.join(" · ")) : w.turnNone;
}

/** What the result will be, from the plan (Rust's numbers). */
function sizeText(t: Messages, e: Effects, planned: Planned, crop: Size, src: Size): string {
  const w = t.effects;
  const plan = planned.plan;
  const [cw, ch] = crop;
  if (!plan) return e.crop ? w.resultCropped(cw, ch) : w.resultSource(cw, ch);
  const [ow, oh] = plan.size;
  const s = e.size;
  let text: string;
  if (s.kind === "short" && plan.scale) text = w.resultShort(ow, oh, s.short, cw, ch);
  else if (s.kind === "exact") {
    if (s.fit === "stretch") text = w.resultStretch(ow, oh);
    else if (s.fit === "cover") text = w.resultCover(ow, oh);
    else if (plan.pad) {
      const [pw, ph] = pictureSize(plan, src);
      text = w.resultContain(ow, oh, pw, ph);
    } else text = w.resultCropped(ow, oh);
    // Enlarged: the picture (not the frame with its bars) is bigger than what it is made from.
    const [pw, ph] = pictureSize(plan, src);
    const from = plan.crop ?? { width: src[0], height: src[1] };
    const [fw, fh] = seenSize([from.width, from.height], plan.turn);
    if (pw > fw || ph > fh) text += ` · ${w.larger}`;
  } else text = e.crop ? w.resultCropped(ow, oh) : w.resultSource(ow, oh);
  return text;
}

/** A whole number typed; Enter or leaving the field commits it, Escape restores it. */
export function NumberField({ label, value, onCommit }: { label: string; value: number; onCommit: (n: number) => void }) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);
  const commit = () => {
    const n = Number.parseInt(draft, 10);
    if (Number.isFinite(n) && n !== value) onCommit(n);
    else setDraft(String(value));
  };
  return (
    <label className="ed-fx-field">
      {label}
      <input
        type="text"
        inputMode="numeric"
        spellCheck={false}
        value={draft}
        onChange={(ev) => setDraft(ev.target.value)}
        onBlur={commit}
        onKeyDown={(ev) => {
          if (ev.key === "Enter") commit();
          else if (ev.key === "Escape") setDraft(String(value));
        }}
      />
    </label>
  );
}
