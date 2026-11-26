import { useCallback, useEffect, useRef, useState } from "react";
import { api, toMsg } from "../../lib/ipc";
import { addPaths, type ConvertRow, type ListNotice } from "./convertView";

/** Files read by ffprobe at once while the list fills (each takes ~50 ms; a folder may hold hundreds). */
const PROBES_AT_ONCE = 4;

export interface ConvertList {
  rows: ConvertRow[];
  /** A 파일 추가 · 폴더 추가 · drop is being listed. */
  adding: boolean;
  /** What the last add said: folders without video, paths that are gone, the limit, duplicates. */
  notices: ListNotice[];
  dismissNotices: () => void;
  add: (paths: string[]) => Promise<void>;
  clear: () => void;
  toggle: (path: string) => void;
  /** Checks (or clears) every row in `paths`. */
  setChecked: (paths: string[], checked: boolean) => void;
  /** 변환 시작 made these items: the rows follow them and are unchecked. */
  markQueued: (queued: { path: string; id: number }[]) => void;
}

/**
 * The 일괄 변환 list: what was added (Rust lists a folder's video files, `convert_scan`), each file
 * then read by ffprobe a few at a time (`convert_probe`, kept in Rust for 변환 시작).
 */
export function useConvertList(): ConvertList {
  const [rows, setRowsState] = useState<ConvertRow[]>([]);
  // Every change goes through `update`, so `current` is always the list as it is now (an add needs
  // to know which paths are new before React renders).
  const current = useRef<ConvertRow[]>([]);
  const update = useCallback((f: (rs: ConvertRow[]) => ConvertRow[]) => {
    current.current = f(current.current);
    setRowsState(current.current);
  }, []);
  const [adding, setAdding] = useState(false);
  const [notices, setNotices] = useState<ListNotice[]>([]);
  // Paths waiting for ffprobe, and how many are being read.
  const pending = useRef<string[]>([]);
  const reading = useRef(0);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const pump = useCallback(() => {
    while (reading.current < PROBES_AT_ONCE && pending.current.length > 0) {
      const path = pending.current.shift()!;
      reading.current++;
      api
        .convertProbe(path)
        .then((info) => ({ state: "ready" as const, info }))
        .catch((e) => ({ state: "error" as const, error: toMsg(e) }))
        .then((probe) => {
          reading.current--;
          if (!alive.current) return;
          // A row cleared meanwhile stays gone.
          update((rs) => rs.map((r) => (r.path === path && r.probe.state === "loading" ? { ...r, probe } : r)));
          pump();
        });
    }
  }, [update]);

  const add = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) return;
      setAdding(true);
      try {
        const scan = await api.convertScan(paths);
        if (!alive.current) return;
        const next = addPaths(current.current, scan.files);
        update(() => next.rows);
        // New rows go at the end.
        pending.current.push(...next.rows.slice(next.rows.length - next.added).map((r) => r.path));
        const already = scan.files.length - next.added;
        setNotices([...scan.notices, ...(already > 0 ? [{ code: "alreadyListed" as const, count: already }] : [])]);
        pump();
      } catch (e) {
        setNotices([toMsg(e)]);
      } finally {
        if (alive.current) setAdding(false);
      }
    },
    [pump, update],
  );

  const clear = useCallback(() => {
    pending.current = [];
    update(() => []);
    setNotices([]);
  }, [update]);

  const toggle = useCallback(
    (path: string) => update((rs) => rs.map((r) => (r.path === path ? { ...r, checked: !r.checked } : r))),
    [update],
  );

  const setChecked = useCallback(
    (paths: string[], checked: boolean) => {
      const set = new Set(paths);
      update((rs) => rs.map((r) => (set.has(r.path) ? { ...r, checked } : r)));
    },
    [update],
  );

  const markQueued = useCallback(
    (queued: { path: string; id: number }[]) => {
      const ids = new Map(queued.map((q) => [q.path, q.id]));
      update((rs) => rs.map((r) => (ids.has(r.path) ? { ...r, checked: false, itemId: ids.get(r.path)! } : r)));
    },
    [update],
  );

  return { rows, adding, notices, dismissNotices: () => setNotices([]), add, clear, toggle, setChecked, markQueued };
}
