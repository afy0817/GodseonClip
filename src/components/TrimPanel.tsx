import { useEffect, useRef, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useT } from "../lib/i18n/I18n";
import { say } from "../lib/i18n/text";
import {
  api,
  isMovFamily,
  type JobFinished,
  type JobProgress,
  type Msg,
  onJobFinished,
  onJobProgress,
  type ProbeResult,
  type StartedJob,
  toMsg,
} from "../lib/ipc";
import { formatTime, keyframeAtOrBefore, parseTime } from "../lib/time";

interface Props {
  enabled: boolean;
  probe: ProbeResult | null;
  onFinished: (summary: string) => void;
}

export function TrimPanel({ enabled, probe, onFinished }: Props) {
  const t = useT();
  const w = t.diag.trim;
  const dict = useRef(t);
  dict.current = t;
  const info = probe?.info ?? null;
  const [startText, setStartText] = useState("00:00.000");
  const [endText, setEndText] = useState("00:00.000");
  const [job, setJob] = useState<StartedJob | null>(null);
  const [progress, setProgress] = useState<JobProgress | null>(null);
  const [finished, setFinished] = useState<JobFinished | null>(null);
  const [error, setError] = useState<Msg | null>(null);
  const jobId = useRef<number | null>(null);
  const finishedCb = useRef(onFinished);
  finishedCb.current = onFinished;

  useEffect(() => {
    if (!info) return;
    setStartText("00:00.000");
    setEndText(formatTime(info.duration));
    setJob(null);
    setProgress(null);
    setFinished(null);
    setError(null);
    // Reset only when a different file is opened, not on every probe object change.
  }, [info?.path]);

  useEffect(() => {
    let alive = true;
    let unlisten: Array<() => void> = [];
    Promise.all([
      onJobProgress((p) => {
        if (p.id === jobId.current) setProgress(p);
      }),
      onJobFinished((f) => {
        if (f.id !== jobId.current) return;
        jobId.current = null;
        setFinished(f);
        const d = dict.current;
        const r = d.diag.trim.report;
        finishedCb.current(
          f.ok
            ? r.ok((f.elapsedMs / 1000).toFixed(1), f.output ?? "")
            : f.canceled
              ? r.canceled
              : r.failed(f.error ? say(d, f.error) : ""),
        );
      }),
    ]).then((fns) => {
      if (alive) unlisten = fns;
      else fns.forEach((f) => f());
    });
    return () => {
      alive = false;
      unlisten.forEach((f) => f());
    };
  }, []);

  const start = parseTime(startText);
  const end = parseTime(endText);
  const valid = info !== null && start !== null && end !== null && end > start && start < info.duration;
  const snapped = valid && probe ? keyframeAtOrBefore(probe.keyframes, start!) : null;
  const exact = info !== null && isMovFamily(info.formatName);
  const pulledBy = valid && snapped !== null ? start! - snapped : 0;
  // MP4/MOV play from the requested start; other containers show the frames from the keyframe.
  const visibleStart = valid ? (exact || snapped === null ? start! : snapped) : 0;
  const running = job !== null && finished === null;

  const run = async () => {
    if (!info || !valid) return;
    setError(null);
    setFinished(null);
    setProgress(null);
    try {
      const started = await api.startLosslessTrim({ path: info.path, start: start!, end: end! });
      jobId.current = started.id;
      setJob(started);
    } catch (e) {
      setError(toMsg(e));
    }
  };

  const cancel = () => {
    if (job) void api.cancelJob(job.id);
  };

  return (
    <section className="panel" aria-labelledby="trim-title">
      <div className="panel-head">
        <h2 id="trim-title">{w.title}</h2>
      </div>
      {!info ? (
        <p className="muted small">{w.empty}</p>
      ) : (
        <>
          <div className="field-row">
            <label className="field">
              <span>{t.trim.start}</span>
              <input className="text mono" value={startText} onChange={(e) => setStartText(e.target.value)} />
            </label>
            <label className="field">
              <span>{t.trim.end}</span>
              <input className="text mono" value={endText} onChange={(e) => setEndText(e.target.value)} />
            </label>
            <div className="field">
              <span>{t.trim.length}</span>
              <span className="mono value">{valid ? formatTime(end! - visibleStart) : "—"}</span>
            </div>
          </div>
          {!valid && <p className="warn-text small">{w.invalid}</p>}
          {valid && snapped !== null && pulledBy > 0.0005 && (
            <p className="note">
              {exact ? (
                <>
                  {t.trim.editList[0]}
                  <b className="mono">{formatTime(snapped)}</b>
                  {t.trim.editList[1]}
                </>
              ) : (
                <>
                  {t.trim.pulled[0]}
                  <b className="mono">{formatTime(snapped)}</b>
                  {t.trim.pulled[1](pulledBy.toFixed(3))}
                </>
              )}
            </p>
          )}
          <div className="btn-row">
            <button className="btn primary" onClick={run} disabled={!enabled || !valid || running}>
              {w.run}
            </button>
            {running && (
              <button className="btn danger" onClick={cancel}>
                {t.exportPanel.cancel}
              </button>
            )}
          </div>

          {job && (
            <div className="job">
              <div
                className="progress"
                role="progressbar"
                aria-label={w.progress}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={Math.round(finished?.ok ? 100 : (progress?.percent ?? 0))}
              >
                <div style={{ width: `${finished?.ok ? 100 : (progress?.percent ?? 0)}%` }} />
              </div>
              <div className="mono small muted">
                {finished
                  ? finished.ok
                    ? w.done((finished.elapsedMs / 1000).toFixed(1))
                    : finished.canceled
                      ? t.diag.proxy.canceled
                      : t.convert.row.failed
                  : `${(progress?.percent ?? 0).toFixed(0)}%${progress?.speed ? ` · ${progress.speed.toFixed(1)}×` : ""}`}
              </div>
              {finished?.ok && finished.output && (
                <div className="btn-row">
                  <span className="mono small">{finished.output}</span>
                  <button className="btn" onClick={() => revealItemInDir(finished.output!)}>
                    {t.exportPanel.reveal}
                  </button>
                </div>
              )}
              {finished?.error && <p className="error">{say(t, finished.error)}</p>}
              <p className="muted small">
                {w.startsAt} <span className="mono">{formatTime(job.plan.visibleStart)}</span>
                {job.plan.exactStart ? w.editList : w.keyframe}
                {Math.abs(job.plan.seek - (start ?? job.plan.seek)) > 0.0005 && w.seekFix(job.plan.seek.toFixed(3))}
              </p>
              <details>
                <summary>{w.command}</summary>
                <pre className="code">{job.commandLine}</pre>
              </details>
            </div>
          )}
          {error && <p className="error">{say(t, error)}</p>}
        </>
      )}
    </section>
  );
}
