// 진단: the Phase 0 verification screen, for checking a new OS or machine. Reached from the bottom
// of the FFmpeg settings screen; FFmpeg is found and set there (App) — this screen only shows which
// one it uses.

import { useEffect, useMemo, useState } from "react";
import { EncoderPanel } from "./EncoderPanel";
import { MediaPanel } from "./MediaPanel";
import { ReportPanel } from "./ReportPanel";
import { TrimPanel } from "./TrimPanel";
import { sourceLabel } from "../features/setup/setupView";
import { useT } from "../lib/i18n/I18n";
import { say } from "../lib/i18n/text";
import { api, type EncoderReport, type LocateResult, type Msg, type PlatformInfo, type ProbeResult, toMsg } from "../lib/ipc";
import { codecMatrix, hasFrameCallback, type PlaybackStatus } from "../lib/playback";

interface Props {
  platform: PlatformInfo | null;
  locate: LocateResult | null;
  locating: boolean;
  /** Back to the FFmpeg settings (finding · setting FFmpeg lives there). */
  onOpenSetup: () => void;
}

export function DiagnosticsScreen({ platform, locate, locating, onOpenSetup }: Props) {
  const t = useT();
  const w = t.diag;
  const [encoders, setEncoders] = useState<EncoderReport | null>(null);
  const [encBusy, setEncBusy] = useState(false);
  const [encError, setEncError] = useState<Msg | null>(null);
  const [probe, setProbe] = useState<ProbeResult | null>(null);
  const [probeBusy, setProbeBusy] = useState(false);
  const [probeError, setProbeError] = useState<Msg | null>(null);
  const [playback, setPlayback] = useState<PlaybackStatus>({ state: "idle" });
  const [trim, setTrim] = useState<string | null>(null);
  const [proxyNote, setProxyNote] = useState<string | null>(null);
  const [timelineNote, setTimelineNote] = useState<string | null>(null);
  const matrix = useMemo(codecMatrix, []);
  const frameCallback = useMemo(hasFrameCallback, []);

  const info = locate?.info ?? null;
  const ready = !!info?.meetsMinimum;

  // Encoder results belong to the FFmpeg they were measured with.
  const ffmpegPath = ready ? info!.ffmpegPath : null;
  useEffect(() => setEncoders(null), [ffmpegPath]);

  const runEncoders = async () => {
    setEncBusy(true);
    setEncError(null);
    try {
      setEncoders(await api.testEncoders());
    } catch (e) {
      setEncError(toMsg(e));
    } finally {
      setEncBusy(false);
    }
  };

  const openFile = async (path: string) => {
    setProbeBusy(true);
    setProbeError(null);
    setTrim(null);
    try {
      setProbe(await api.probeMedia(path));
    } catch (e) {
      setProbe(null);
      setPlayback({ state: "idle" });
      setProbeError(toMsg(e));
    } finally {
      setProbeBusy(false);
    }
  };

  return (
    <div className="app">
      <header className="top">
        <div className="brand">
          <svg aria-hidden="true" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
            <rect x="3" y="5" width="18" height="14" rx="2" />
            <path d="M9 5v14M15 5v14" />
          </svg>
          <span>Godseon</span>
          <span className="badge">{t.setup.diag}</span>
        </div>
        <div className="btn-row">
          <span className="muted small mono">{platform ? `${platform.os} · ${platform.arch}` : ""}</span>
          <button className="btn primary" onClick={onOpenSetup}>
            {w.back}
          </button>
        </div>
      </header>

      <main className="content">
        <p className="intro">{w.intro}</p>
        <section className="panel" aria-labelledby="ffmpeg-title">
          <div className="panel-head">
            <h2 id="ffmpeg-title">1 · FFmpeg</h2>
            <button className="btn" onClick={onOpenSetup}>
              {w.toSetup}
            </button>
          </div>
          <p className={ready ? "ok-text" : "warn-text"} role="status">
            {locating && !ready
              ? t.setup.searching
              : ready && info
                ? w.ffmpegReady(
                    `FFmpeg ${info.release ?? info.version} · ${sourceLabel(t, info.source)} · libavcodec ${info.libavcodecMajor ?? "?"}`,
                    locate?.minVersion ?? "6.0",
                  )
                : w.ffmpegMissing(locate?.problem ? say(t, locate.problem) : "")}
          </p>
          {ready && info && (
            <p className="mono small muted">
              {info.ffmpegPath} · ffprobe {info.ffprobePath}
            </p>
          )}
        </section>
        <EncoderPanel enabled={ready} busy={encBusy} report={encoders} error={encError} onRun={runEncoders} />
        <MediaPanel
          enabled={ready}
          busy={probeBusy}
          probe={probe}
          error={probeError}
          playback={playback}
          matrix={matrix}
          frameCallback={frameCallback}
          onOpen={openFile}
          onPlayback={setPlayback}
          onProxyNote={setProxyNote}
          onTimelineNote={setTimelineNote}
        />
        <TrimPanel enabled={ready} probe={probe} onFinished={setTrim} />
        <ReportPanel
          platform={platform}
          locate={locate}
          encoders={encoders}
          probe={probe}
          playback={playback}
          matrix={matrix}
          frameCallback={frameCallback}
          trim={trim}
          proxy={proxyNote}
          timeline={timelineNote}
        />
      </main>
    </div>
  );
}
