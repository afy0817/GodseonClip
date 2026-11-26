import { useCallback, useEffect, useRef, useState } from "react";
import { api, toMsg, type MediaInfo, type Msg } from "../../lib/ipc";
import { type JobTracker, useJob } from "../../lib/useJob";

export interface ProxyState {
  /** Ready proxy file (540p H.264 in the app cache), or null. */
  path: string | null;
  job: JobTracker;
  error: Msg | null;
  /** Make (or reuse) the proxy of the open file. */
  start: () => void;
}

/**
 * Preview proxy of the open file. Starts by itself once `needed` turns true (the webview
 * could not show the original), at most once per file: after a cancel or a failure the
 * player offers a button instead. Opening another file cancels a proxy still being made.
 */
export function useProxy(info: MediaInfo | null, needed: boolean): ProxyState {
  const [path, setPath] = useState<string | null>(null);
  const [error, setError] = useState<Msg | null>(null);
  const [asked, setAsked] = useState(false);
  const file = info?.path ?? null;
  const fileRef = useRef(file);
  fileRef.current = file;
  const job = useJob((f) => {
    if (f.ok && f.output) setPath(f.output);
  });
  const { track, reset, cancel } = job;

  useEffect(() => {
    setPath(null);
    setError(null);
    setAsked(false);
    reset();
    return cancel;
  }, [file, reset, cancel]);

  const start = useCallback(() => {
    const p = fileRef.current;
    if (!p) return;
    setAsked(true);
    setError(null);
    api
      .startProxy(p)
      .then((j) => {
        if (fileRef.current !== p) {
          if (j.id !== null) void api.cancelJob(j.id);
          return;
        }
        if (j.ready) setPath(j.path);
        else if (j.id !== null) track(j.id);
      })
      .catch((e) => {
        if (fileRef.current === p) setError(toMsg(e));
      });
  }, [track]);

  useEffect(() => {
    if (needed && !asked) start();
  }, [needed, asked, start]);

  return { path, job, error, start };
}
