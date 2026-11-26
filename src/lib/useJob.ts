import { useCallback, useEffect, useRef, useState } from "react";
import { api, type JobFinished, type JobProgress, onJobFinished, onJobProgress } from "./ipc";

export interface JobTracker {
  /** Id of the followed job, until `reset`. */
  id: number | null;
  running: boolean;
  progress: JobProgress | null;
  finished: JobFinished | null;
  /** 0–100; 100 once finished successfully, 0 once canceled or failed. */
  percent: number;
  /** Follow this job (the id a start command returned). */
  track: (id: number) => void;
  /** Stop following (does not cancel). */
  reset: () => void;
  /** Ask Rust to cancel the followed job. */
  cancel: () => void;
}

const EARLY_LIMIT = 32;

/**
 * Follows one FFmpeg job at a time through the `job-progress` / `job-finished` events.
 *
 * A short job can finish before the start command's reply reaches the UI, so finish events of
 * jobs not followed yet are kept for a while and applied when `track` is called.
 */
export function useJob(onFinished?: (f: JobFinished) => void): JobTracker {
  const [id, setId] = useState<number | null>(null);
  const [progress, setProgress] = useState<JobProgress | null>(null);
  const [finished, setFinished] = useState<JobFinished | null>(null);
  const idRef = useRef<number | null>(null);
  const early = useRef(new Map<number, JobFinished>());
  const finishedCb = useRef(onFinished);
  finishedCb.current = onFinished;

  useEffect(() => {
    let alive = true;
    let unlisten: Array<() => void> = [];
    Promise.all([
      onJobProgress((p) => {
        if (p.id === idRef.current) setProgress(p);
      }),
      onJobFinished((f) => {
        if (f.id === idRef.current) {
          idRef.current = null;
          setFinished(f);
          finishedCb.current?.(f);
          return;
        }
        early.current.set(f.id, f);
        if (early.current.size > EARLY_LIMIT) {
          const oldest = early.current.keys().next().value;
          if (oldest !== undefined) early.current.delete(oldest);
        }
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

  const track = useCallback((jobId: number) => {
    const done = early.current.get(jobId) ?? null;
    early.current.delete(jobId);
    idRef.current = done ? null : jobId;
    setId(jobId);
    setProgress(null);
    setFinished(done);
    if (done) finishedCb.current?.(done);
  }, []);

  const reset = useCallback(() => {
    idRef.current = null;
    setId(null);
    setProgress(null);
    setFinished(null);
  }, []);

  const cancel = useCallback(() => {
    if (idRef.current !== null) void api.cancelJob(idRef.current);
  }, []);

  const running = id !== null && finished === null;
  // FFmpeg reports `progress=end` (100%) also after a cancel's `q`: only success fills the bar.
  const percent = finished ? (finished.ok ? 100 : 0) : (progress?.percent ?? 0);
  return { id, running, progress, finished, percent, track, reset, cancel };
}
