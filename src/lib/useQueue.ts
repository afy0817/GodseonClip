import { useEffect, useMemo, useState } from "react";
import { newer, withProgress } from "../features/queue/queueView";
import { api, toMsg, onQueueChanged, onQueueProgress, type QueueItem, type QueueSnapshot, type Msg } from "./ipc";

export interface QueueActions {
  cancel: (id: number) => void;
  cancelWaiting: () => void;
  remove: (id: number) => void;
  clearFinished: () => void;
  /** `position` among the waiting items (0 runs next). */
  move: (id: number, position: number) => void;
  restart: (id: number) => void;
  setConcurrency: (n: number) => void;
}

export interface QueueState {
  /** null until the first snapshot arrives. */
  snapshot: QueueSnapshot | null;
  items: QueueItem[];
  /** The last action Rust refused (or a concurrency that could not be saved). */
  error: Msg | null;
  clearError: () => void;
  actions: QueueActions;
}

/**
 * The 작업 큐 as Rust holds it: a snapshot on every change (`queue-changed`, newest version wins)
 * and progress in between (`queue-progress`). One per app (App.tsx); the editor and the queue
 * screen read the same state.
 */
export function useQueue(): QueueState {
  const [snapshot, setSnapshot] = useState<QueueSnapshot | null>(null);
  const [error, setError] = useState<Msg | null>(null);

  useEffect(() => {
    let alive = true;
    let unlisten: Array<() => void> = [];
    // Listen first, then ask: a change between the two is never missed (versions sort them out).
    Promise.all([
      onQueueChanged((s) => setSnapshot((prev) => newer(prev, s))),
      onQueueProgress((p) => setSnapshot((prev) => withProgress(prev, p))),
    ]).then((fns) => {
      if (!alive) {
        fns.forEach((f) => f());
        return;
      }
      unlisten = fns;
      api
        .queueSnapshot()
        .then((s) => setSnapshot((prev) => newer(prev, s)))
        .catch((e) => setError(toMsg(e)));
    });
    return () => {
      alive = false;
      unlisten.forEach((f) => f());
    };
  }, []);

  const actions = useMemo<QueueActions>(() => {
    const run = (p: Promise<unknown>) => {
      setError(null);
      p.catch((e) => setError(toMsg(e)));
    };
    return {
      cancel: (id) => run(api.queueCancel(id)),
      cancelWaiting: () => run(api.queueCancelWaiting()),
      remove: (id) => run(api.queueRemove(id)),
      clearFinished: () => run(api.queueClearFinished()),
      move: (id, position) => run(api.queueMove(id, position)),
      restart: (id) => run(api.queueRestart(id)),
      setConcurrency: (n) => run(api.queueSetConcurrency(n)),
    };
  }, []);

  return { snapshot, items: snapshot?.items ?? [], error, clearError: () => setError(null), actions };
}

/** The current Unix ms, ticking every second while `on` (elapsed times of running items). */
export function useNow(on: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    setNow(Date.now());
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [on]);
  return now;
}
