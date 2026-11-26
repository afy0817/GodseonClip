import { useState } from "react";
import { sourceLabel } from "../features/setup/setupView";
import { useT } from "../lib/i18n/I18n";
import type { Messages } from "../lib/i18n/ko";
import { say } from "../lib/i18n/text";
import type { EncoderReport, LocateResult, PlatformInfo, ProbeResult } from "../lib/ipc";
import { type CodecProbe, type PlaybackStatus, playbackLabel } from "../lib/playback";
import { formatTime } from "../lib/time";

interface Props {
  platform: PlatformInfo | null;
  locate: LocateResult | null;
  encoders: EncoderReport | null;
  probe: ProbeResult | null;
  playback: PlaybackStatus;
  matrix: CodecProbe[];
  frameCallback: boolean;
  trim: string | null;
  /** How the preview proxy was obtained and whether it plays. */
  proxy: string | null;
  /** Thumbnail strip and waveform timings. */
  timeline: string | null;
}

/** One Markdown block per machine, pasted into the Phase 0 playback matrix. */
export function buildReport(t: Messages, p: Props): string {
  const w = t.diag.report;
  const none = t.diag.encoders.none;
  const lines: string[] = [];
  lines.push(`## Godseon Phase 0 — ${p.platform?.os ?? "?"} ${p.platform?.arch ?? ""}`.trim());
  lines.push(`- ${w.webview}: ${navigator.userAgent}`);
  const i = p.locate?.info;
  if (i) {
    lines.push(
      `- FFmpeg: ${i.release ?? i.version} (${sourceLabel(t, i.source)}) · libavcodec ${i.libavcodecMajor ?? "?"} · ${w.minimum(p.locate!.minVersion, i.meetsMinimum)}`,
    );
    lines.push(`  - ${w.path}: ${i.ffmpegPath}${p.locate?.savedPath === i.ffmpegPath ? ` ${w.saved}` : ""}`);
    const f = i.features;
    lines.push(
      `  - ${w.features}: libx264 ${f.libx264 ? "✓" : "✗"} · libx265 ${f.libx265 ? "✓" : "✗"} · libsvtav1 ${f.libsvtav1 ? "✓" : "✗"} · HW ${[...f.hevcHardware, ...f.av1Hardware].join(", ") || none}`,
    );
  } else {
    lines.push(`- FFmpeg: ${w.notFound}${p.locate?.problem ? ` (${say(t, p.locate.problem)})` : ""}`);
  }
  for (const n of p.locate?.notices ?? []) lines.push(`  - ${w.notice}: ${say(t, n)}`);
  if (p.encoders) {
    lines.push(`- ${w.encoders}: ${p.encoders.tests.map((x) => `${x.name} ${x.ok ? "✓" : "✗"}`).join(" · ") || w.noCandidates}`);
    lines.push(`  - ${w.preferred}: H.265 ${p.encoders.preferredHevc ?? none} · AV1 ${p.encoders.preferredAv1 ?? none}`);
  }
  lines.push(`- canPlayType: ${p.matrix.map((m) => `${m.label} ${m.result || "no"}`).join(" · ")}`);
  lines.push(`- requestVideoFrameCallback: ${p.frameCallback ? t.diag.media.yes : none}`);
  if (p.probe) {
    const v = p.probe.info.video;
    lines.push(
      `- ${w.playback}: ${p.probe.info.fileName} (${p.probe.info.formatName}${v ? `, ${v.codec} ${v.width}×${v.height}` : ""}, ${formatTime(p.probe.info.duration)}) → ${playbackLabel(t, p.playback)}`,
    );
    lines.push(`  - ${w.keyframes(p.probe.keyframes.length)}`);
    if (p.proxy) lines.push(`  - ${w.proxy}: ${p.proxy}`);
    if (p.timeline) lines.push(`  - ${w.timeline}: ${p.timeline}`);
  }
  if (p.trim) lines.push(`- ${w.trim}: ${p.trim}`);
  return lines.join("\n");
}

export function ReportPanel(props: Props) {
  const t = useT();
  const [copied, setCopied] = useState(false);
  const text = buildReport(t, props);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  };
  return (
    <section className="panel" aria-labelledby="report-title">
      <div className="panel-head">
        <h2 id="report-title">{t.diag.report.title}</h2>
        <button className="btn" onClick={copy}>
          {copied ? t.setup.copied : t.diag.report.copy}
        </button>
      </div>
      <p className="muted small">{t.diag.report.sub}</p>
      <pre className="code">{text}</pre>
    </section>
  );
}
