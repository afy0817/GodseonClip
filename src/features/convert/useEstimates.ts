import { useCallback, useEffect, useRef, useState } from "react";
import { api, type ConvertRequest, type ConvertSettings, type EstimateEntry, toMsg, onEstimateChanged, onEstimatePaused, type Msg } from "../../lib/ipc";

/** Settings or the list must stay put this long before the tests are asked for again. */
const SETTLE_MS = 400;

export interface Estimates {
  /** By path: the test encode with the settings asked for last. */
  entries: Map<string, EstimateEntry>;
  /** The 작업 큐 runs an item: tests wait. */
  paused: boolean;
  /** The request itself failed (no encoder, FFmpeg gone). */
  error: Msg | null;
  /** `entries` are for other settings than the ones given now (the new ones are on their way). */
  stale: boolean;
}

/**
 * 예상 크기 of the files going in at the next 변환 시작 (estimate.rs): asked again whenever those
 * files or the settings change (once they settle), Rust trying one file at a time and sending each
 * result as an event. Events of an older request (another key) are left out. `paths` empty stops
 * the tests. One request at a time — the first one may wait seconds for the encoder test — so Rust
 * always ends with the last one; a reply with a newer request behind it is dropped.
 */
export function useEstimates(enabled: boolean, paths: string[], settings: ConvertSettings, settingsKey: string): Estimates {
  const [entries, setEntries] = useState<Map<string, EstimateEntry>>(() => new Map());
  const [paused, setPaused] = useState(false);
  const [error, setError] = useState<Msg | null>(null);
  // The settings key of the request whose reply `entries` holds.
  const [appliedKey, setAppliedKey] = useState<string | null>(null);
  const latest = useRef(settings);
  latest.current = settings;

  useEffect(() => {
    let alive = true;
    let unlisten: Array<() => void> = [];
    Promise.all([
      onEstimateChanged((e) =>
        setEntries((m) => {
          const known = m.get(e.path);
          if (!known || known.key !== e.key) return m;
          return new Map(m).set(e.path, e);
        }),
      ),
      onEstimatePaused(setPaused),
    ]).then((fns) => {
      if (alive) unlisten = fns;
      else fns.forEach((f) => f());
    });
    return () => {
      alive = false;
      unlisten.forEach((f) => f());
    };
  }, []);

  const alive = useRef(true);
  const inFlight = useRef(false);
  const next = useRef<{ req: ConvertRequest; key: string } | null>(null);
  const send = useCallback((req: ConvertRequest, key: string) => {
    if (inFlight.current) {
      next.current = { req, key };
      return;
    }
    inFlight.current = true;
    api
      .estimateSizes(req)
      .then((snap) => {
        if (!alive.current || next.current) return;
        setEntries(new Map(snap.entries.map((e) => [e.path, e])));
        setPaused(snap.paused);
        setError(null);
        setAppliedKey(key);
      })
      .catch((e) => alive.current && !next.current && setError(toMsg(e)))
      .finally(() => {
        inFlight.current = false;
        const queued = next.current;
        next.current = null;
        if (queued && alive.current) send(queued.req, queued.key);
      });
  }, []);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const pathsKey = paths.join("\n");
  useEffect(() => {
    if (!enabled) return;
    const wanted = pathsKey ? pathsKey.split("\n") : [];
    const timer = setTimeout(() => send({ paths: wanted, settings: latest.current }, settingsKey), wanted.length ? SETTLE_MS : 0);
    return () => clearTimeout(timer);
  }, [enabled, pathsKey, settingsKey, send]);

  return { entries, paused, error, stale: appliedKey !== null && appliedKey !== settingsKey };
}
