import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { pickVideoFile } from "../lib/files";
import { useT } from "../lib/i18n/I18n";
import { say } from "../lib/i18n/text";
import type { Msg, ProbeResult } from "../lib/ipc";
import { formatBytes, formatTime } from "../lib/time";
import { type CodecProbe, type PlaybackStatus, playbackLabel } from "../lib/playback";
import { ProxyControl, TimelinePreview } from "./PreviewTools";

interface Props {
  enabled: boolean;
  busy: boolean;
  probe: ProbeResult | null;
  error: Msg | null;
  playback: PlaybackStatus;
  matrix: CodecProbe[];
  frameCallback: boolean;
  onOpen: (path: string) => void;
  /** Playback of the original file (not of the proxy). */
  onPlayback: (s: PlaybackStatus) => void;
  onProxyNote: (note: string | null) => void;
  onTimelineNote: (note: string | null) => void;
}

export function MediaPanel({
  enabled,
  busy,
  probe,
  error,
  playback,
  matrix,
  frameCallback,
  onOpen,
  onPlayback,
  onProxyNote,
  onTimelineNote,
}: Props) {
  const dict = useT();
  const w = dict.diag.media;
  const videoRef = useRef<HTMLVideoElement>(null);
  const [now, setNow] = useState(0);
  const [proxy, setProxy] = useState<{ path: string; note: string } | null>(null);
  const [proxyPlayback, setProxyPlayback] = useState<PlaybackStatus>({ state: "idle" });
  const info = probe?.info ?? null;
  const v = info?.video ?? null;
  const frame = v?.fps ? 1 / v.fps : 1 / 30;
  // The original could not be shown: offer a proxy. Edits still use the original's timeline.
  const needsProxy = v !== null && (playback.state === "error" || playback.state === "audio-only");
  const src = info ? convertFileSrc(proxy?.path ?? info.path) : "";
  const report = useRef({ onProxyNote, onTimelineNote });
  report.current = { onProxyNote, onTimelineNote };

  useEffect(() => {
    setProxy(null);
    setProxyPlayback({ state: "idle" });
    setNow(0);
    report.current.onProxyNote(null);
    report.current.onTimelineNote(null);
  }, [info?.path]);

  useEffect(() => {
    if (proxy) report.current.onProxyNote(`${proxy.note} → ${playbackLabel(dict, proxyPlayback)}`);
  }, [proxy, proxyPlayback, dict]);

  const setStatus = (s: PlaybackStatus) => (proxy ? setProxyPlayback(s) : onPlayback(s));
  const shown = proxy ? proxyPlayback : playback;

  const pick = async () => {
    const picked = await pickVideoFile(dict);
    if (picked) {
      onPlayback({ state: "loading" });
      onOpen(picked);
    }
  };

  const seek = (t: number) => {
    const el = videoRef.current;
    if (!el) return;
    el.currentTime = t;
    setNow(t);
  };

  const step = (delta: number) => {
    const el = videoRef.current;
    if (!el) return;
    el.pause();
    el.currentTime = Math.max(0, Math.min(el.duration || Infinity, el.currentTime + delta));
  };

  const kf = probe?.keyframes ?? [];
  const avgGop = kf.length > 1 ? (kf[kf.length - 1] - kf[0]) / (kf.length - 1) : null;

  return (
    <section className="panel" aria-labelledby="media-title">
      <div className="panel-head">
        <h2 id="media-title">{w.title}</h2>
        <button className="btn primary" onClick={pick} disabled={!enabled || busy}>
          {busy ? dict.editor.reading : w.open}
        </button>
      </div>
      {!enabled && <p className="muted small">{w.needsFfmpeg}</p>}
      {error && <p className="error">{say(dict, error)}</p>}

      {info && (
        <div className="media-grid">
          <dl className="kv">
            <dt>{w.file}</dt>
            <dd className="mono">{info.fileName}</dd>
            <dt>{w.container}</dt>
            <dd>{info.formatName}</dd>
            <dt>{w.lengthSize}</dt>
            <dd>
              {formatTime(info.duration)} · {formatBytes(info.sizeBytes)}
              {info.bitRate ? ` · ${(info.bitRate / 1e6).toFixed(1)}Mbps` : ""}
            </dd>
            <dt>{w.video}</dt>
            <dd>
              {v ? (
                <>
                  {v.codec}
                  {v.profile ? ` (${v.profile})` : ""} · {v.width}×{v.height}
                  {v.fps ? ` · ${v.fps.toFixed(3).replace(/\.?0+$/, "")}fps` : ""}
                  {v.pixFmt ? ` · ${v.pixFmt}` : ""}
                  {v.hdr && <span className="chip warn">HDR {v.colorTransfer}</span>}
                  {v.rotation != null && v.rotation !== 0 && <span className="chip">{w.rotation(v.rotation)}</span>}
                  {v.vfrSuspect && <span className="chip warn">{w.vfr}</span>}
                </>
              ) : (
                w.none
              )}
            </dd>
            <dt>{w.audio}</dt>
            <dd>
              {info.audio.length === 0
                ? w.none
                : info.audio
                    .map((a) => `${a.codec}${a.channels ? ` ${a.channels}ch` : ""}${a.sampleRate ? ` ${a.sampleRate / 1000}kHz` : ""}${a.language ? ` (${a.language})` : ""}`)
                    .join(", ")}
            </dd>
            <dt>{w.otherTracks}</dt>
            <dd>{w.otherCounts(info.subtitleCount, info.otherCount)}</dd>
            <dt>{dict.timeline.keyframe}</dt>
            <dd>
              {probe?.keyframeError
                ? w.keyframesFailed(say(dict, probe.keyframeError))
                : w.keyframes(kf.length, avgGop ? avgGop.toFixed(2) : null)}
            </dd>
          </dl>

          <div className="player">
            <video
              ref={videoRef}
              key={src}
              src={src}
              controls
              preload="auto"
              // With only metadata loaded WKWebView never fires loadeddata before play, so judge
              // from loadedmetadata: a video track the webview cannot decode reports width 0.
              onLoadedMetadata={(e) => {
                const el = e.currentTarget;
                if (el.videoWidth > 0) setStatus({ state: "ok", width: el.videoWidth, height: el.videoHeight });
                else if (v) setStatus({ state: "audio-only" });
              }}
              onError={(e) => {
                const code = e.currentTarget.error?.code ?? 0;
                setStatus({ state: "error", code });
              }}
              onTimeUpdate={(e) => setNow(e.currentTarget.currentTime)}
              onSeeked={(e) => setNow(e.currentTarget.currentTime)}
            />
            <div className="player-bar">
              <span className={shown.state === "ok" ? "ok-text" : shown.state === "error" || shown.state === "audio-only" ? "warn-text" : "muted"}>
                {proxy ? w.proxyPlayback(playbackLabel(dict, proxyPlayback)) : playbackLabel(dict, playback)}
              </span>
              <span className="mono">{formatTime(now)}</span>
              <div className="btn-row">
                <button className="btn" onClick={() => step(-frame)} aria-label={dict.transport.back1f}>
                  {w.minusFrame}
                </button>
                <button className="btn" onClick={() => step(frame)} aria-label={dict.transport.fwd1f}>
                  {w.plusFrame}
                </button>
              </div>
            </div>
            {proxy && <p className="muted small">{w.originalPlayback(playbackLabel(dict, playback))}</p>}
            <ProxyControl
              info={info}
              needed={needsProxy}
              active={proxy !== null}
              onReady={(path, note) => setProxy({ path, note })}
            />
          </div>
        </div>
      )}
      {probe && <TimelinePreview probe={probe} now={now} onSeek={seek} onNote={onTimelineNote} />}

      <details className="matrix">
        <summary>{w.matrix}</summary>
        <div className="table-wrap">
          <table className="data">
            <tbody>
              {matrix.map((m) => (
                <tr key={m.mime}>
                  <th scope="row">{m.label}</th>
                  <td className={m.result ? "ok-text" : "warn-text"}>{m.result || w.unsupported}</td>
                  <td className="mono small muted">{m.mime}</td>
                </tr>
              ))}
              <tr>
                <th scope="row">requestVideoFrameCallback</th>
                <td className={frameCallback ? "ok-text" : "warn-text"}>{frameCallback ? w.yes : w.none}</td>
                <td className="small muted">{w.frameCallback}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </details>
    </section>
  );
}
