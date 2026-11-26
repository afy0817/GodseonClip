// Phase 1 step 1 checks on the verification screen: preview proxy, thumbnail strip, waveform.
// The editor (src/features/editor) uses the same IPC calls; the timeline data hook and the
// waveform canvas are shared with it.

import { useEffect, useRef, useState } from "react";
import { useT } from "../lib/i18n/I18n";
import { say } from "../lib/i18n/text";
import {
  api,
  type JobFinished,
  type JobProgress,
  type MediaInfo,
  type Msg,
  onJobFinished,
  onJobProgress,
  type ProbeResult,
  type ProxyJob,
  toMsg,
} from "../lib/ipc";
import { formatTime } from "../lib/time";
import { useTimelineData } from "../features/editor/useTimelineData";
import { WaveCanvas } from "../features/editor/WaveCanvas";

interface ProxyProps {
  info: MediaInfo;
  /** The webview could not play the original. */
  needed: boolean;
  /** The player is already showing the proxy. */
  active: boolean;
  /** Proxy file is ready; `note` says how it was obtained (for the report). */
  onReady: (path: string, note: string) => void;
}

export function ProxyControl({ info, needed, active, onReady }: ProxyProps) {
  const t = useT();
  const w = t.diag.proxy;
  const words = useRef(w);
  words.current = w;
  const [job, setJob] = useState<ProxyJob | null>(null);
  const [progress, setProgress] = useState<JobProgress | null>(null);
  const [finished, setFinished] = useState<JobFinished | null>(null);
  const [error, setError] = useState<Msg | null>(null);
  const jobId = useRef<number | null>(null);
  const readyCb = useRef(onReady);
  readyCb.current = onReady;

  // A different file: forget this one's proxy and stop building it.
  useEffect(() => {
    setJob(null);
    setProgress(null);
    setFinished(null);
    setError(null);
    return () => {
      if (jobId.current !== null) void api.cancelJob(jobId.current);
      jobId.current = null;
    };
  }, [info.path]);

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
        if (f.ok && f.output) readyCb.current(f.output, words.current.made((f.elapsedMs / 1000).toFixed(1)));
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

  const running = job !== null && !job.ready && finished === null;

  const start = async () => {
    setError(null);
    setFinished(null);
    setProgress(null);
    try {
      const j = await api.startProxy(info.path);
      setJob(j);
      if (j.ready) readyCb.current(j.path, w.reused);
      else jobId.current = j.id;
    } catch (e) {
      setError(toMsg(e));
    }
  };

  if (!needed && !job) return null;
  const percent = finished?.ok ? 100 : (progress?.percent ?? 0);

  return (
    <div className="proxy">
      {needed && !active && !running && (
        <p className="note small">{w.needed}</p>
      )}
      <div className="btn-row">
        {!active && !running && (
          <button className="btn primary" onClick={start}>
            {w.make}
          </button>
        )}
        {running && (
          <button className="btn danger" onClick={() => job?.id != null && void api.cancelJob(job.id)}>
            {t.stage.cancelProxy}
          </button>
        )}
        {active && <span className="ok-text small">{w.playing}</span>}
      </div>
      {running && (
        <div className="job">
          <div
            className="progress"
            role="progressbar"
            aria-label={t.stage.proxyProgress}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(percent)}
          >
            <div style={{ width: `${percent}%` }} />
          </div>
          <span className="mono small muted">
            {percent.toFixed(0)}%{progress?.speed ? ` · ${progress.speed.toFixed(1)}×` : ""}
          </span>
        </div>
      )}
      {finished?.canceled && <p className="muted small">{w.canceled}</p>}
      {finished?.error && <p className="error">{say(t, finished.error)}</p>}
      {error && <p className="error">{say(t, error)}</p>}
      {job?.commandLine && (
        <details>
          <summary>{w.command}</summary>
          <pre className="code">{job.commandLine}</pre>
        </details>
      )}
    </div>
  );
}

const TILE_COUNT = 16;
const WAVE_BUCKETS = 800;

interface TimelineProps {
  probe: ProbeResult;
  /** Playback position on the original's timeline. */
  now: number;
  onSeek: (t: number) => void;
  /** Timings for the report. */
  onNote: (note: string) => void;
}

export function TimelinePreview({ probe, now, onSeek, onNote }: TimelineProps) {
  const dict = useT();
  const w = dict.diag.timeline;
  const info = probe.info;
  const duration = info.duration > 0 ? info.duration : 1;
  const { strip, wave, thumbError, waveError, busy } = useTimelineData(info, () => ({
    tiles: TILE_COUNT,
    buckets: WAVE_BUCKETS,
  }));
  const noteCb = useRef(onNote);
  noteCb.current = onNote;

  const summary = [
    strip ? w.thumbs(strip.images.length, strip.elapsedMs) : info.video ? null : dict.timeline.noVideo,
    wave ? w.wave(wave.peaks.length, wave.elapsedMs) : info.audio.length ? null : dict.timeline.noAudio,
  ]
    .filter(Boolean)
    .join(" · ");

  useEffect(() => {
    if (!busy && summary) noteCb.current(summary);
  }, [busy, summary]);

  const pct = (t: number) => `${(t / duration) * 100}%`;
  const seekAt = (clientX: number, el: HTMLElement) => {
    const r = el.getBoundingClientRect();
    onSeek(Math.max(0, Math.min(duration, ((clientX - r.left) / r.width) * duration)));
  };

  return (
    <div className="timeline-preview">
      <div className="panel-head">
        <h3>{w.title}</h3>
        <span className="muted small mono">{busy ? w.making : summary}</span>
      </div>
      <div
        className="timeline"
        role="slider"
        tabIndex={0}
        aria-label={w.track}
        aria-valuemin={0}
        aria-valuemax={duration}
        aria-valuenow={now}
        aria-valuetext={formatTime(now)}
        onClick={(e) => seekAt(e.clientX, e.currentTarget)}
        onKeyDown={(e) => {
          if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
            e.preventDefault();
            onSeek(Math.max(0, Math.min(duration, now + (e.key === "ArrowLeft" ? -1 : 1))));
          }
        }}
      >
        <div className="thumbs">
          {strip?.images.map((src, k) => (
            <img
              key={k}
              src={src}
              alt=""
              draggable={false}
              style={{ left: pct(strip.start + k * strip.interval), width: pct(strip.interval) }}
            />
          ))}
        </div>
        <WaveCanvas wave={wave} duration={duration} />
        <div className="playhead" style={{ left: pct(now) }} />
      </div>
      <div className="timeline-scale mono small muted">
        <span>{formatTime(0)}</span>
        <span>{formatTime(duration)}</span>
      </div>
      {thumbError && <p className="error">{dict.timeline.thumbError(say(dict, thumbError))}</p>}
      {waveError && <p className="error">{dict.timeline.waveError(say(dict, waveError))}</p>}
    </div>
  );
}
