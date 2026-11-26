// FFmpeg 설정 · 첫 실행 (canvas UI mockup Setup.dc.html): whether the app found a usable FFmpeg and
// what it can do, how to install one per OS, a path of the user's own, why FFmpeg is not bundled,
// and the way to the Phase 0 diagnostics. The app starts here when FFmpeg is missing or too old.
// Rust finds, checks and saves the path (ffmpeg.rs · settings.rs); this screen only asks.

import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useRef, useState } from "react";
import { useLanguage, useT } from "../../lib/i18n/I18n";
import { LANGUAGE_HEADING, LANGUAGE_NAMES, say } from "../../lib/i18n/text";
import { api, type EncoderReport, type Lang, type LocateResult, type Msg, type PlatformInfo, toMsg } from "../../lib/ipc";
import type { QueueState } from "../../lib/useQueue";
import { CheckIcon, WarningIcon } from "../editor/icons";
import { TopBar, type WorkKind } from "../editor/TopBar";
import { activeCount } from "../queue/queueView";
import {
  type FfmpegActions,
  type Feature,
  featureList,
  guideLine,
  installGuides,
  type Os,
  OS_LABELS,
  osOf,
  pathPlaceholder,
  savedLine,
  setupStatus,
  usableInfo,
} from "./setupView";
import "../editor/editor.css";
import "./setup.css";

interface Props {
  /** False while another screen covers this one. */
  active: boolean;
  platform: PlatformInfo | null;
  locate: LocateResult | null;
  locating: boolean;
  ffmpegActions: FfmpegActions;
  queue: QueueState;
  onKind: (k: WorkKind) => void;
  onOpenQueue: () => void;
  onOpenEditor: () => void;
  onOpenDiagnostics: () => void;
}

const OSES: Os[] = ["macos", "windows", "linux"];

function FeatureMark({ tone }: { tone: Feature["tone"] }) {
  if (tone === "ok") return <CheckIcon />;
  if (tone === "warn") return <WarningIcon />;
  return (
    <svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
      {tone === "pending" ? <path d="M12 3a9 9 0 1 0 9 9" /> : <path d="M6 12h12" />}
    </svg>
  );
}

export function SetupScreen(props: Props) {
  const { active, platform, locate, locating, ffmpegActions, queue } = props;
  const t = useT();
  const w = t.setup;
  const detected = osOf(platform?.os);
  const [shownOs, setShownOs] = useState<Os | null>(null);
  const guideOs = shownOs ?? detected;
  const guide = installGuides(t)[guideOs];
  const [copied, setCopied] = useState<string | null>(null);
  const [custom, setCustom] = useState("");
  const manualRef = useRef<HTMLInputElement>(null);

  const status = setupStatus(t, locate, locating);
  const info = usableInfo(locate);
  const ffmpegPath = info?.ffmpegPath ?? null;
  const saved = savedLine(t, locate);

  // The encoder test of the FFmpeg in use (the one exports and conversions pick from). "다시 검사"
  // finds FFmpeg again and then runs the test again.
  const [report, setReport] = useState<{ path: string; report: EncoderReport } | null>(null);
  const [reportError, setReportError] = useState<Msg | null>(null);
  const retest = useRef(false);
  useEffect(() => {
    if (!active || !ffmpegPath || locating) return;
    if (report?.path === ffmpegPath && !retest.current) return;
    const again = retest.current;
    retest.current = false;
    let alive = true;
    setReportError(null);
    (again ? api.testEncoders() : api.encoderResults())
      .then((r) => alive && setReport({ path: ffmpegPath, report: r }))
      .catch((e) => alive && setReportError(toMsg(e)));
    return () => {
      alive = false;
    };
  }, [active, ffmpegPath, locating, report]);
  const currentReport = report && report.path === ffmpegPath ? report.report : null;

  const checkAgain = () => {
    retest.current = true;
    setReport(null);
    ffmpegActions.locate();
  };

  const copy = async (command: string) => {
    try {
      await navigator.clipboard.writeText(command);
      setCopied(command);
      setTimeout(() => setCopied((c) => (c === command ? null : c)), 1500);
    } catch {
      setCopied(null);
    }
  };

  const browse = async () => {
    const picked = await open({ multiple: false, directory: false, title: w.browseTitle });
    if (typeof picked === "string") {
      setCustom(picked);
      ffmpegActions.choose(picked);
    }
  };

  const toManual = () => {
    manualRef.current?.scrollIntoView({ behavior: "smooth", block: "center" });
    manualRef.current?.focus({ preventScroll: true });
  };

  return (
    <div className="ed st">
      <TopBar
        kind={null}
        onKind={props.onKind}
        queueCount={activeCount(queue.items)}
        queueCurrent={false}
        settingsCurrent
        onOpenQueue={props.onOpenQueue}
        onOpenSettings={() => window.scrollTo(0, 0)}
      />

      <main className="st-main">
        <div className="st-top">
          <div className="st-intro">
            <span className="st-eyebrow">{w.eyebrow}</span>
            <h1>{w.title}</h1>
            <p>{w.intro}</p>
          </div>
          <LanguagePicker />
        </div>

        {status.state === "ready" && info ? (
          <section className="st-card st-found" aria-label={w.statusLabel}>
            <div className="st-card-head">
              <span className="st-mark ok">
                <CheckIcon />
              </span>
              <div className="st-card-text">
                <h2>{status.title}</h2>
                <p className="mono">{status.detail}</p>
              </div>
              <div className="st-card-actions">
                <button type="button" className="ed-btn primary" onClick={props.onOpenEditor}>
                  {w.openEditor}
                </button>
                <button type="button" className="ed-btn" disabled={locating} onClick={checkAgain}>
                  {locating ? w.searching : w.checkAgain}
                </button>
              </div>
            </div>
            <ul className="st-features" aria-label={w.featuresLabel}>
              {featureList(t, info, locate?.minVersion ?? "6.0", currentReport, detected).map((f) => (
                <li key={f.name} className={f.tone} title={f.why ?? undefined}>
                  <FeatureMark tone={f.tone} />
                  <span>
                    {f.mono ? <code>{f.name}</code> : <span className="st-plain">{f.name}</span>} <span className="st-what">— {f.what}</span>
                  </span>
                </li>
              ))}
            </ul>
            {reportError && <p className="error small">{w.testFailed(say(t, reportError))}</p>}
          </section>
        ) : status.state === "locating" ? (
          <section className="st-card" aria-label={w.statusLabel} aria-busy="true">
            <div className="st-card-head">
              <div className="st-card-text">
                <h2>{status.title}</h2>
                <p>{status.detail}</p>
              </div>
            </div>
          </section>
        ) : (
          <section className="st-card st-missing" aria-label={w.statusLabel}>
            <div className="st-card-head">
              <span className="st-mark warn">
                <WarningIcon />
              </span>
              <div className="st-card-text">
                <h2>{status.title}</h2>
                {status.problem && <p className="st-problem">{status.problem}</p>}
                <p>{status.detail}</p>
                {locate && locate.searched.length > 0 && (
                  <details className="st-searched">
                    <summary>{w.searched(locate.searched.length)}</summary>
                    <ul className="mono">
                      {locate.searched.map((s) => (
                        <li key={s}>{s}</li>
                      ))}
                    </ul>
                  </details>
                )}
              </div>
              <div className="st-card-actions">
                <button type="button" className="ed-btn primary" disabled={locating} onClick={ffmpegActions.locate}>
                  {locating ? w.searching : w.installedSearch}
                </button>
                <button type="button" className="ed-btn st-warn-btn" onClick={toManual}>
                  {w.manual}
                </button>
              </div>
            </div>
          </section>
        )}

        {locate?.notices.map((n, i) => (
          <p key={i} className="st-notice" role="status">
            {say(t, n)}
          </p>
        ))}

        <section className="st-section" aria-label={w.install}>
          <div className="st-section-head">
            <div>
              <h2>{w.install}</h2>
              <span className="st-dim">{guideLine(t, detected, guideOs)}</span>
            </div>
            <div className="ed-seg st-tabs" role="group" aria-label={w.os}>
              {OSES.map((os) => (
                <button key={os} type="button" aria-pressed={guideOs === os} onClick={() => setShownOs(os)}>
                  <b>{OS_LABELS[os]}</b>
                </button>
              ))}
            </div>
          </div>
          <div className="st-guide">
            {guide.heading && <h3>{guide.heading}</h3>}
            <ol>
              {guide.steps.map((step, i) => (
                <li key={step.text}>
                  <span className="st-step">{i + 1}</span>
                  <div className="st-step-body">
                    <span>{step.text}</span>
                    {step.link && (
                      <button type="button" className="link" onClick={() => void openUrl(step.link!.url)}>
                        {step.link.label}
                      </button>
                    )}
                    {step.command && (
                      <div className="st-command">
                        <code>{step.command}</code>
                        <button type="button" className="ed-btn" onClick={() => void copy(step.command!)}>
                          {copied === step.command ? w.copied : w.copy}
                        </button>
                      </div>
                    )}
                    {step.more && <span className="st-dim small">{step.more}</span>}
                  </div>
                </li>
              ))}
            </ol>
            {guide.caution && (
              <div className="st-caution">
                <h3>{guide.caution.title}</h3>
                <p>{guide.caution.text}</p>
                <button type="button" className="link" onClick={() => void openUrl(guide.caution!.link.url)}>
                  {guide.caution.link.label}
                </button>
              </div>
            )}
            {guide.note && <p className="st-guide-note">{guide.note}</p>}
          </div>
        </section>

        <section className="st-card st-manual" aria-label={w.manual}>
          <h2>{w.manual}</h2>
          <p className="st-dim">{w.manualSub}</p>
          <div className="st-manual-row">
            <label className="st-field">
              {w.pathLabel}
              <input
                ref={manualRef}
                className="ed-input mono"
                value={custom}
                spellCheck={false}
                placeholder={pathPlaceholder(detected)}
                onChange={(e) => setCustom(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && custom.trim() && ffmpegActions.choose(custom)}
              />
            </label>
            <button type="button" className="ed-btn" disabled={locating} onClick={() => void browse()}>
              {w.browse}
            </button>
            <button type="button" className="ed-btn" disabled={locating || !custom.trim()} onClick={() => ffmpegActions.choose(custom)}>
              {w.checkPath}
            </button>
          </div>
          {saved.saved ? (
            <div className="st-saved">
              <p>
                {w.savedPath} <code>{saved.saved}</code> — {saved.text}
              </p>
              <button type="button" className="ed-btn" disabled={locating} onClick={ffmpegActions.forget}>
                {w.forget}
              </button>
            </div>
          ) : (
            locate && <p className="st-dim small">{saved.text}</p>
          )}
        </section>

        <section className="st-section st-why" aria-label={w.whyLabel}>
          <h2>{w.why}</h2>
          <p>{w.whyText}</p>
        </section>

        <section className="st-card st-diag" aria-label={w.diag}>
          <div>
            <h2>{w.diag}</h2>
            <p className="st-dim">{w.diagText}</p>
          </div>
          <button type="button" className="ed-btn" onClick={props.onOpenDiagnostics}>
            {w.openDiag}
          </button>
        </section>
      </main>
    </div>
  );
}

/** 언어 · Language: the system's language, or one picked; both names in their own language. */
function LanguagePicker() {
  const t = useT();
  const lang = useLanguage();
  const options: { value: Lang | null; label: string; sub: string | null }[] = [
    { value: null, label: t.setup.language.system, sub: LANGUAGE_NAMES[lang.system] },
    { value: "ko", label: LANGUAGE_NAMES.ko, sub: null },
    { value: "en", label: LANGUAGE_NAMES.en, sub: null },
  ];
  return (
    <section className="st-lang" aria-label={LANGUAGE_HEADING}>
      <span className="ed-label">{LANGUAGE_HEADING}</span>
      <div className="ed-seg" role="group" aria-label={LANGUAGE_HEADING}>
        {options.map((o) => (
          <button key={o.value ?? "system"} type="button" aria-pressed={lang.setting === o.value} lang={o.value ?? undefined} onClick={() => lang.choose(o.value)}>
            <b>{o.label}</b>
            {o.sub && <span>{o.sub}</span>}
          </button>
        ))}
      </div>
      <span className="st-dim small">{t.setup.language.note}</span>
      {lang.notice && <span className="warn-text small">{say(t, lang.notice)}</span>}
    </section>
  );
}
