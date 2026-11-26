// What the FFmpeg 설정 · 첫 실행 screen says (canvas UI mockup Setup.dc.html): the status card, the
// feature list, the install guide per OS. Pure functions, no IPC (`npm run test:ts` runs them:
// imports are types or `.ts` modules). Rust finds and checks FFmpeg (ffmpeg.rs) and tests the
// encoders (encoders.rs); this only explains the result.

import type { EncoderReport, FfmpegInfo, LocateResult } from "../../lib/ipc";
import type { Messages } from "../../lib/i18n/ko.ts";
import { say } from "../../lib/i18n/text.ts";

/** The three FFmpeg requests; Rust decides what is saved (`settings.rs`). App.tsx runs them. */
export interface FfmpegActions {
  /** "다시 찾기" (and app start): the saved path first, then the automatic search. */
  locate: () => void;
  /** "이 경로로 확인" / "찾아보기": use this path and save it when it works. */
  choose: (path: string) => void;
  /** "자동 탐색으로 되돌리기": forget the saved path. */
  forget: () => void;
}

export type Os = "macos" | "windows" | "linux";

/** `std::env::consts::OS` → the install guide shown first. */
export function osOf(platform: string | null | undefined): Os {
  return platform === "windows" ? "windows" : platform === "linux" ? "linux" : "macos";
}

export const OS_LABELS: Record<Os, string> = { macos: "macOS", windows: "Windows", linux: "Ubuntu" };

// ---------------------------------------------------------------------------------------------
// The status card

export type SetupState = "locating" | "missing" | "unusable" | "ready";

export interface SetupStatus {
  state: SetupState;
  title: string;
  /** Under the title: the path and how it was installed, or what to do. */
  detail: string | null;
  /** Why the FFmpeg found cannot be used, or what went wrong. */
  problem: string | null;
}

/** The FFmpeg the app uses: found and 6.0 or newer. */
export function usableInfo(locate: LocateResult | null): FfmpegInfo | null {
  return locate?.info?.meetsMinimum ? locate.info : null;
}

/** "9.0.2" (the release when FFmpeg says one, else its version string). */
export function versionOf(info: FfmpegInfo): string {
  return info.release ?? info.version;
}

export function setupStatus(t: Messages, locate: LocateResult | null, locating: boolean): SetupStatus {
  const w = t.setupView.status;
  if (!locate || (locating && !usableInfo(locate))) {
    return { state: "locating", title: w.locatingTitle, detail: w.locatingDetail, problem: null };
  }
  const info = locate.info;
  const problem = locate.problem ? say(t, locate.problem) : null;
  if (info?.meetsMinimum) {
    return {
      state: "ready",
      title: w.readyTitle,
      detail: `FFmpeg ${versionOf(info)} · ${info.ffmpegPath} · ${sourceLabel(t, info.source)}`,
      problem: null,
    };
  }
  if (info) return { state: "unusable", title: w.unusableTitle, detail: w.unusableDetail(locate.minVersion), problem };
  return { state: "missing", title: w.missingTitle, detail: w.missingDetail, problem };
}

/** ffmpeg::guess_source: "Homebrew로 설치됨" for a package manager, "시스템 패키지" · "직접 설치". */
export function sourceLabel(t: Messages, source: string): string {
  const w = t.setupView.source;
  if (source === "system") return w.system;
  if (source === "manual") return w.manual;
  return ["Homebrew", "MacPorts", "winget", "Scoop", "Chocolatey", "snap"].includes(source) ? w.installedWith(source) : source;
}

// ---------------------------------------------------------------------------------------------
// The feature list

export type FeatureTone = "ok" | "off" | "warn" | "pending";

export interface Feature {
  /** Shown in mono: an encoder or program name; else plain words. */
  name: string;
  mono: boolean;
  what: string;
  tone: FeatureTone;
  /** Why it failed (the encoder test's error). */
  why: string | null;
}

/** Encoders the app's exports and conversions use (command::exports_with, H.264 left out). */
const USED_HARDWARE = ["hevc_videotoolbox", "hevc_nvenc", "hevc_qsv", "hevc_amf"];

const HARDWARE_LABELS: Record<string, string> = {
  hevc_videotoolbox: "VideoToolbox",
  hevc_nvenc: "NVENC",
  hevc_qsv: "Quick Sync",
  hevc_amf: "AMF",
  hevc_vaapi: "VA-API",
  hevc_mf: "Media Foundation",
};

/**
 * What this FFmpeg can do for the app, in the mockup's order. Hardware encoders carry the three-frame
 * test (`report`; null while it runs): listed in FFmpeg is not the same as working on this machine.
 */
export function featureList(t: Messages, info: FfmpegInfo, minVersion: string, report: EncoderReport | null, os: Os): Feature[] {
  const w = t.setupView.features;
  const f = info.features;
  const tested = (name: string) => report?.tests.find((x) => x.name === name) ?? null;
  const list: Feature[] = [
    { name: `ffmpeg · ffprobe ${versionOf(info)}`, mono: true, what: w.minimum(minVersion), tone: "ok", why: null },
    f.libx264
      ? { name: "libx264", mono: true, what: w.x264, tone: "ok", why: null }
      : { name: "libx264", mono: true, what: w.noX264, tone: "warn", why: null },
    f.libx265
      ? { name: "libx265", mono: true, what: "H.265 (CPU)", tone: "ok", why: null }
      : { name: "libx265", mono: true, what: w.noX265, tone: "warn", why: null },
  ];
  for (const name of f.hevcHardware) {
    const hw = w.hevcHardware(HARDWARE_LABELS[name] ? ` (${HARDWARE_LABELS[name]})` : "");
    if (!USED_HARDWARE.includes(name)) {
      list.push({ name, mono: true, what: `${hw} · ${w.notUsedYet}`, tone: "off", why: null });
      continue;
    }
    const r = tested(name);
    if (!report) list.push({ name, mono: true, what: `${hw} · ${w.testing}`, tone: "pending", why: null });
    else if (r?.ok) list.push({ name, mono: true, what: `${hw} · ${w.passed}`, tone: "ok", why: null });
    else list.push({ name, mono: true, what: `${hw} · ${w.failed}`, tone: "warn", why: r?.error ? say(t, r.error) : w.noResult });
  }
  if (f.hevcHardware.length === 0) {
    list.push({ name: w.hevcHardware(""), mono: false, what: w.noHevcHardware, tone: "off", why: null });
  }
  list.push(
    f.libsvtav1
      ? { name: "libsvtav1", mono: true, what: "AV1 (CPU)", tone: "ok", why: null }
      : { name: "libsvtav1", mono: true, what: w.noSvtAv1, tone: "warn", why: null },
  );
  if (f.av1Hardware.length === 0) {
    list.push({ name: w.av1Hardware, mono: false, what: os === "macos" ? w.noAv1HardwareMac : w.noAv1Hardware, tone: "off", why: null });
  } else {
    list.push({ name: f.av1Hardware.join(" · "), mono: true, what: w.av1HardwareUnused, tone: "off", why: null });
  }
  list.push(
    f.libopus
      ? { name: "libopus", mono: true, what: w.opus, tone: "ok", why: null }
      : { name: "libopus", mono: true, what: w.noOpus, tone: "off", why: null },
  );
  return list;
}

// ---------------------------------------------------------------------------------------------
// Install guides (the same commands the Phase 0 plan settled on: canvas 01 · 설치 가이드)

export interface InstallStep {
  text: string;
  /** A command to run, with a copy button. */
  command?: string;
  /** Smaller line under the command. */
  more?: string;
  link?: { label: string; url: string };
}

export interface InstallGuide {
  /** Above the steps (Ubuntu names its release). */
  heading?: string;
  steps: InstallStep[];
  /** Under the steps, after a line. */
  note?: string;
  /** An orange box after the steps (Ubuntu 22.04). */
  caution?: { title: string; text: string; link: { label: string; url: string } };
}

/** The install guide of each OS: the commands here, the words from the dictionary. */
export function installGuides(t: Messages): Record<Os, InstallGuide> {
  const g = t.setupView.guide;
  return {
    macos: {
      steps: [
        { text: g.macos.brewFirst, link: { label: g.macos.brewLink, url: "https://brew.sh" } },
        { text: g.runInTerminal, command: "brew install ffmpeg" },
        { text: g.macos.comeBack },
      ],
      note: g.macos.note,
    },
    windows: {
      steps: [
        { text: g.windows.run, command: "winget install --id Gyan.FFmpeg -e", more: g.windows.more },
        { text: g.windows.thenLocate },
      ],
      note: g.windows.note,
    },
    linux: {
      heading: "Ubuntu 24.04 LTS",
      steps: [{ text: g.runInTerminal, command: "sudo apt install ffmpeg" }, { text: g.linux.hardware }],
      caution: {
        title: "Ubuntu 22.04 LTS",
        text: g.linux.jammy,
        link: { label: g.linux.jammyLink, url: "https://github.com/BtbN/FFmpeg-Builds/releases" },
      },
    },
  };
}

/** "이 컴퓨터는 macOS로 감지되었습니다." · or the guide picked by hand. */
export function guideLine(t: Messages, detected: Os, shown: Os): string {
  const g = t.setupView.guide;
  return detected === shown ? g.detected(OS_LABELS[detected]) : g.viewing(OS_LABELS[shown], OS_LABELS[detected]);
}

/** The input's example path for this OS. */
export function pathPlaceholder(os: Os): string {
  return os === "windows" ? "C:\\ffmpeg\\bin\\ffmpeg.exe" : os === "linux" ? "/opt/ffmpeg/bin/ffmpeg" : "/opt/homebrew/bin/ffmpeg";
}

/** The line under 직접 경로 지정: what is saved, and what happens at the next start. */
export function savedLine(t: Messages, locate: LocateResult | null): { text: string; saved: string | null } {
  const w = t.setupView.saved;
  const saved = locate?.savedPath ?? null;
  if (!saved) return { text: w.none, saved: null };
  if (locate?.info?.meetsMinimum && locate.info.ffmpegPath === saved) return { text: w.inUse, saved };
  return { text: w.unusable, saved };
}
