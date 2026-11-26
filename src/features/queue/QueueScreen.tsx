// 작업 큐 (canvas UI mockup Queue.dc.html): what runs, what waits (in order), what finished.
// Rust holds the queue (queue.rs); this screen only asks it to cancel, move, remove or restart.

import { useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useT } from "../../lib/i18n/I18n";
import { say } from "../../lib/i18n/text";
import type { QueueItem } from "../../lib/ipc";
import { type QueueState, useNow } from "../../lib/useQueue";
import { ArrowUpIcon, CheckIcon, CloseIcon, StopIcon, WarningIcon } from "../editor/icons";
import { TopBar, type WorkKind } from "../editor/TopBar";
import {
  activeCount,
  baseName,
  finishedLine,
  groupItems,
  itemEstimate,
  itemTags,
  logText,
  queueSummary,
  runningStats,
  waitingLine,
} from "./queueView";
import "../editor/editor.css";
import "./queue.css";

interface Props {
  /** False while another screen covers this one. */
  active: boolean;
  queue: QueueState;
  /** 자르기 · 구간 제거 · 일괄 변환 in the header: back to that screen. */
  onKind: (k: WorkKind) => void;
  onOpenSettings: () => void;
}

const CONCURRENCY = [1, 2, 3, 4];

export function QueueScreen({ active, queue, onKind, onOpenSettings }: Props) {
  const t = useT();
  const w = t.queue;
  const { items, snapshot, actions } = queue;
  const groups = groupItems(items);
  const now = useNow(active && groups.running.length > 0);
  // Items whose FFmpeg log is open.
  const [open, setOpen] = useState<ReadonlySet<number>>(new Set());
  const toggle = (id: number) =>
    setOpen((s) => {
      const next = new Set(s);
      if (!next.delete(id)) next.add(id);
      return next;
    });

  return (
    <div className="ed q">
      <TopBar
        kind={null}
        onKind={onKind}
        queueCount={activeCount(items)}
        queueCurrent
        onOpenQueue={() => window.scrollTo(0, 0)}
        onOpenSettings={onOpenSettings}
      />

      <main className="q-main">
        <div className="q-head">
          <div className="q-title">
            <h1>{t.topBar.queue}</h1>
            <span className="q-sub">{snapshot ? queueSummary(t, items) : t.stage.loading}</span>
          </div>
          <div className="q-controls">
            <label htmlFor="q-concurrency">{w.concurrency}</label>
            <select
              id="q-concurrency"
              className="q-select"
              value={snapshot?.concurrency ?? 1}
              title={w.concurrencyTitle}
              onChange={(e) => actions.setConcurrency(Number(e.target.value))}
            >
              {CONCURRENCY.map((n) => (
                <option key={n} value={n}>
                  {w.concurrencyN(n)}
                </option>
              ))}
            </select>
            <button type="button" className="ed-btn" disabled={groups.waiting.length === 0} onClick={actions.cancelWaiting}>
              {w.cancelWaiting}
            </button>
          </div>
        </div>
        {queue.error && (
          <p className="error small q-error">
            {say(t, queue.error)}{" "}
            <button type="button" className="link" onClick={queue.clearError}>
              {t.convertScreen.close}
            </button>
          </p>
        )}

        <section aria-label={w.running} className="q-section">
          <h2 className="q-h2">{w.running}</h2>
          {groups.running.length === 0 ? (
            <p className="q-empty">{w.noRunning}</p>
          ) : (
            groups.running.map((item) => (
              <RunningCard key={item.id} item={item} now={now} logOpen={open.has(item.id)} onLog={() => toggle(item.id)} onCancel={() => actions.cancel(item.id)} />
            ))
          )}
        </section>

        <section aria-label={w.waiting} className="q-section">
          <h2 className="q-h2">
            {w.waiting} · {groups.waiting.length}
          </h2>
          {groups.waiting.length === 0 ? (
            <p className="q-empty">{w.noWaiting}</p>
          ) : (
            <ol className="q-list">
              {groups.waiting.map((item, index) => (
                <li key={item.id} className="q-row">
                  <span className="q-num mono">{groups.running.length + index + 1}</span>
                  <div className="q-row-main">
                    <b>{item.info.sourceName}</b>
                    <span className="q-arrow">→</span>
                    <span className="mono q-out">{baseName(item.output)}</span>
                  </div>
                  <span className="q-desc">{waitingLine(t, item)}</span>
                  <div className="q-row-actions">
                    <button
                      type="button"
                      className="q-icon"
                      aria-label={w.moveUpLabel(item.info.sourceName)}
                      title={w.moveUp}
                      disabled={index === 0}
                      onClick={() => actions.move(item.id, index - 1)}
                    >
                      <ArrowUpIcon />
                    </button>
                    <button
                      type="button"
                      className="q-icon"
                      aria-label={w.removeLabel(item.info.sourceName)}
                      title={w.remove}
                      onClick={() => actions.remove(item.id)}
                    >
                      <CloseIcon />
                    </button>
                  </div>
                </li>
              ))}
            </ol>
          )}
        </section>

        <section aria-label={w.finishedLabel} className="q-section">
          <div className="q-section-head">
            <h2 className="q-h2">{w.finished}</h2>
            {groups.finished.length > 0 && (
              <button type="button" className="link small" onClick={actions.clearFinished}>
                {w.clearHistory}
              </button>
            )}
          </div>
          {groups.finished.length === 0 ? (
            <p className="q-empty">{w.noFinished}</p>
          ) : (
            <ul className="q-list">
              {groups.finished.map((item) => (
                <FinishedRow
                  key={item.id}
                  item={item}
                  logOpen={open.has(item.id)}
                  onLog={() => toggle(item.id)}
                  onRestart={() => actions.restart(item.id)}
                />
              ))}
            </ul>
          )}
        </section>
      </main>
    </div>
  );
}

function Names({ item }: { item: QueueItem }) {
  return (
    <div className="q-names">
      <b>{item.info.sourceName}</b>
      <span className="q-arrow">→</span>
      <span className="mono q-out">{baseName(item.output)}</span>
    </div>
  );
}

function RunningCard(props: { item: QueueItem; now: number; logOpen: boolean; onLog: () => void; onCancel: () => void }) {
  const { item, now, logOpen } = props;
  const t = useT();
  const w = t.queue;
  const percent = item.progress?.percent ?? 0;
  const estimate = itemEstimate(t, item);
  return (
    <article className="q-card">
      <div className="q-card-top">
        <div className="q-card-info">
          <Names item={item} />
          <div className="q-tags">
            {itemTags(t, item).map((tag) => (
              <span key={tag} className="q-tag">
                {tag}
              </span>
            ))}
          </div>
        </div>
        <div className="q-card-actions">
          <button type="button" className="ed-btn danger" disabled={item.canceling} onClick={props.onCancel}>
            {item.canceling ? t.convert.row.canceling : t.exportPanel.cancel}
          </button>
          <button type="button" className="ed-btn" aria-expanded={logOpen} onClick={props.onLog}>
            {w.ffmpegLog}
          </button>
        </div>
      </div>
      <div className="q-progress-block">
        <div
          className="q-bar"
          role="progressbar"
          aria-label={w.progressLabel(item.info.sourceName)}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(percent)}
        >
          <div style={{ width: `${percent}%` }} />
        </div>
        <div className="q-stats mono">
          {runningStats(t, item, now).map((s, i) => (i === 0 ? <b key={s}>{s}</b> : <span key={s}>{s}</span>))}
          {estimate && <span className="q-estimate">{estimate}</span>}
        </div>
      </div>
      {item.note && <p className="warn-text small">{say(t, item.note)}</p>}
      {logOpen && <pre className="q-log">{logText(t, item)}</pre>}
    </article>
  );
}

function FinishedRow(props: { item: QueueItem; logOpen: boolean; onLog: () => void; onRestart: () => void }) {
  const { item, logOpen } = props;
  const t = useT();
  const w = t.queue;
  const failed = item.state === "failed";
  const done = item.state === "done";
  return (
    <li className={`q-row finished ${item.state}`}>
      <span className="q-state" aria-label={done ? t.convert.row.done : failed ? t.convert.row.failed : t.convert.row.canceled}>
        {done ? <CheckIcon /> : failed ? <WarningIcon /> : <StopIcon />}
      </span>
      <div className="q-row-main col">
        {done ? <span className="mono">{baseName(item.output)}</span> : <Names item={item} />}
        <span className="q-desc">
          {finishedLine(t, item)}
          {item.note && <span className="q-note"> · {say(t, item.note)}</span>}
        </span>
      </div>
      <div className="q-row-actions">
        {done && (
          <button type="button" className="ed-btn" onClick={() => void revealItemInDir(item.output)}>
            {t.exportPanel.reveal}
          </button>
        )}
        {!done && (
          <button type="button" className="ed-btn" onClick={props.onRestart}>
            {w.restart}
          </button>
        )}
        {(failed || item.log) && (
          <button type="button" className={`ed-btn${failed ? " bad" : ""}`} aria-expanded={logOpen} onClick={props.onLog}>
            {logOpen ? w.hideLog : w.showLog}
          </button>
        )}
      </div>
      {logOpen && <pre className="q-log">{logText(t, item)}</pre>}
    </li>
  );
}
