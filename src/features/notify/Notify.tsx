// The header's notification center (2026-11-24, user request): a bell in every screen's header that
// opens a panel over the screen with the queue's running items (progress · cancel), the waiting ones
// and the last that finished (show in folder). Finished items the user has not opened the panel for
// mark the bell (orange when one failed). The queue screen keeps the full list and the logs.

import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { createContext, type ReactNode, useCallback, useContext, useEffect, useRef, useState } from "react";
import { useT } from "../../lib/i18n/I18n";
import type { QueueItem } from "../../lib/ipc";
import { type QueueState, useNow } from "../../lib/useQueue";
import { BellIcon, CheckIcon, StopIcon, WarningIcon } from "../editor/icons";
import { baseName, finishedLine, runningStats, waitingLine } from "../queue/queueView";
import { lastFinishedOrder, notifyGroups, overallPercent, unseenOf } from "./notifyView";
import "./notify.css";

/** Waiting items listed at most (the rest: a count). */
const WAITING_LIMIT = 3;

interface NotifyState {
  queue: QueueState;
  openQueue: () => void;
  open: boolean;
  setOpen: (open: boolean) => void;
  /** The last `finishedOrder` the user has seen (the panel was open). */
  seenOrder: number;
}

const NotifyContext = createContext<NotifyState | null>(null);

/** One per app (App.tsx): every screen's header shares what has been seen and whether it is open. */
export function NotifyProvider({ queue, openQueue, children }: { queue: QueueState; openQueue: () => void; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  const [seenOrder, setSeenOrder] = useState(0);
  const last = lastFinishedOrder(queue.items);
  // While the panel is open, what finishes is seen.
  useEffect(() => {
    if (open) setSeenOrder((s) => Math.max(s, last));
  }, [open, last]);
  const value: NotifyState = { queue, openQueue, open, setOpen, seenOrder };
  return <NotifyContext.Provider value={value}>{children}</NotifyContext.Provider>;
}

/** The bell and its panel, a cell of the header strip (TopBar). */
export function NotifyButton() {
  const s = useContext(NotifyContext);
  const t = useT();
  const w = t.notify;
  const ref = useRef<HTMLDivElement>(null);
  const close = useCallback(() => s?.setOpen(false), [s]);

  // Clicking outside or Escape closes it.
  useEffect(() => {
    if (!s?.open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [s?.open, close]);

  if (!s) return null;
  const { items } = s.queue;
  const g = notifyGroups(items);
  const unseen = unseenOf(items, s.seenOrder);
  const percent = overallPercent(items);

  return (
    <div className="ed-notify" ref={ref}>
      <button
        type="button"
        className="ed-btn icon ed-notify-btn"
        aria-label={w.label(g.running.length + g.waiting.length, unseen.count)}
        aria-expanded={s.open}
        aria-haspopup="dialog"
        title={w.title}
        onClick={() => s.setOpen(!s.open)}
      >
        <BellIcon />
        {unseen.count > 0 && <span className={unseen.failed ? "ed-notify-dot bad" : "ed-notify-dot"} aria-hidden="true" />}
        {percent !== null && <span className="ed-notify-bar" style={{ width: `${percent}%` }} aria-hidden="true" />}
      </button>
      {s.open && <NotifyPanel items={items} queue={s.queue} onOpenQueue={() => (close(), s.openQueue())} />}
    </div>
  );
}

function NotifyPanel({ items, queue, onOpenQueue }: { items: QueueItem[]; queue: QueueState; onOpenQueue: () => void }) {
  const t = useT();
  const w = t.notify;
  const g = notifyGroups(items);
  const now = useNow(g.running.length > 0);
  const empty = g.running.length + g.waiting.length + g.recent.length === 0;
  return (
    <div className="nt-panel" role="dialog" aria-label={w.title}>
      <div className="nt-head">
        <b>{w.title}</b>
        <button type="button" className="link small" onClick={onOpenQueue}>
          {t.exportPanel.seeQueue}
        </button>
      </div>
      {empty && <p className="nt-empty">{w.empty}</p>}

      {g.running.length > 0 && <div className="nt-group">{w.running}</div>}
      {g.running.map((item) => {
        const percent = item.progress?.percent ?? 0;
        return (
          <div key={item.id} className="nt-item running">
            <Names item={item} />
            <div className="q-bar" role="progressbar" aria-label={t.queue.progressLabel(item.info.sourceName)} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(percent)}>
              <div style={{ width: `${percent}%` }} />
            </div>
            <div className="nt-row">
              {/* One span per stat: the line breaks between stats, never inside one ("speed 0.9×" split in two at 20 jobs, Phase 3 step 6). */}
              <span className="nt-stats mono">
                {item.canceling ? t.convert.row.canceling : runningStats(t, item, now).map((s) => <span key={s}>{s}</span>)}
              </span>
              <button type="button" className="ed-btn danger" disabled={item.canceling} onClick={() => queue.actions.cancel(item.id)}>
                {t.exportPanel.cancel}
              </button>
            </div>
          </div>
        );
      })}

      {g.waiting.length > 0 && <div className="nt-group">{w.waiting(g.waiting.length)}</div>}
      {g.waiting.slice(0, WAITING_LIMIT).map((item) => (
        <div key={item.id} className="nt-item">
          <Names item={item} />
          <span className="nt-desc">{waitingLine(t, item)}</span>
        </div>
      ))}
      {g.waiting.length > WAITING_LIMIT && <p className="nt-more">{w.more(g.waiting.length - WAITING_LIMIT)}</p>}

      {g.recent.length > 0 && <div className="nt-group">{w.recent}</div>}
      {g.recent.map((item) => (
        <div key={item.id} className={`nt-item finished ${item.state}`}>
          <div className="nt-row">
            <span className="nt-state" aria-label={item.state === "done" ? t.convert.row.done : item.state === "failed" ? t.convert.row.failed : t.convert.row.canceled}>
              {item.state === "done" ? <CheckIcon /> : item.state === "failed" ? <WarningIcon /> : <StopIcon />}
            </span>
            <Names item={item} />
            {item.state === "done" && (
              <button type="button" className="ed-btn" onClick={() => void revealItemInDir(item.output)}>
                {t.exportPanel.reveal}
              </button>
            )}
          </div>
          <span className="nt-desc">{finishedLine(t, item)}</span>
        </div>
      ))}
    </div>
  );
}

function Names({ item }: { item: QueueItem }) {
  return (
    <div className="nt-names">
      <b title={item.info.source}>{item.info.sourceName}</b>
      <span className="q-arrow">→</span>
      <span className="mono" title={item.output}>
        {baseName(item.output)}
      </span>
    </div>
  );
}
