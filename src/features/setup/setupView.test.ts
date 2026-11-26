// npm run test:ts — the FFmpeg 설정 · 첫 실행 screen's wording.
import assert from "node:assert/strict";
import { test } from "node:test";
import type { EncoderReport, FfmpegInfo, LocateResult } from "../../lib/ipc";
import { en } from "../../lib/i18n/en.ts";
import { ko } from "../../lib/i18n/ko.ts";
import { featureList, guideLine, installGuides, osOf, pathPlaceholder, savedLine, setupStatus, sourceLabel, usableInfo } from "./setupView.ts";

const INSTALL_GUIDES = installGuides(ko);

function info(patch: Partial<FfmpegInfo> = {}, features: Partial<FfmpegInfo["features"]> = {}): FfmpegInfo {
  return {
    ffmpegPath: "/opt/homebrew/bin/ffmpeg",
    ffprobePath: "/opt/homebrew/bin/ffprobe",
    version: "9.0.2",
    release: "9.0.2",
    libavcodecMajor: 63,
    meetsMinimum: true,
    source: "Homebrew",
    videoEncoders: [],
    features: { libx264: true, libx265: true, libsvtav1: true, hevcHardware: ["hevc_videotoolbox"], av1Hardware: [], libopus: true, ...features },
    ...patch,
  };
}

function located(i: FfmpegInfo | null, patch: Partial<LocateResult> = {}): LocateResult {
  return { found: !!i, info: i, searched: ["/usr/bin", "/opt/homebrew/bin", "/usr/local/bin"], problem: null, minVersion: "6.0", savedPath: null, notices: [], ...patch };
}

const REPORT: EncoderReport = {
  tests: [
    { name: "hevc_videotoolbox", codec: "hevc", hardware: true, ok: true, millis: 165, error: null },
    { name: "hevc_nvenc", codec: "hevc", hardware: true, ok: false, millis: 40, error: { code: "raw", text: "No NVENC capable devices found" } },
    { name: "libx265", codec: "hevc", hardware: false, ok: true, millis: 300, error: null },
  ],
  preferredHevc: "hevc_videotoolbox",
  preferredAv1: "libsvtav1",
};

test("status card: finding, found, too old, missing", () => {
  assert.equal(setupStatus(ko, null, true).state, "locating");
  assert.equal(setupStatus(ko, located(null), true).state, "locating", "a search again from not found shows that it searches");
  const ready = setupStatus(ko, located(info()), false);
  assert.deepEqual(ready, { state: "ready", title: "FFmpeg를 찾았습니다", detail: "FFmpeg 9.0.2 · /opt/homebrew/bin/ffmpeg · Homebrew로 설치됨", problem: null });
  assert.equal(setupStatus(ko, located(info()), true).state, "ready", "다시 검사 keeps the card while it searches");
  assert.equal(setupStatus(ko, located(info({ source: "PATH", release: null, version: "N-12345-g1a2b" })), false).detail, "FFmpeg N-12345-g1a2b · /opt/homebrew/bin/ffmpeg · PATH");

  const old = setupStatus(ko, located(info({ meetsMinimum: false, release: "5.1.2" }), { problem: { code: "ffmpegTooOld", version: "5.1.2", min: "6.0" } }), false);
  assert.deepEqual([old.state, old.title, old.problem], ["unusable", "이 FFmpeg는 쓸 수 없습니다", "FFmpeg 5.1.2은(는) 최소 버전 6.0보다 낮습니다."]);
  assert.match(old.detail!, /6\.0 이상으로 업데이트/);

  const missing = setupStatus(ko, located(null), false);
  assert.deepEqual([missing.state, missing.title, missing.detail], ["missing", "FFmpeg를 찾지 못했습니다", "아래 방법으로 설치한 뒤 ‘다시 찾기’를 누르세요."]);
  assert.equal(setupStatus(ko, located(null, { problem: { code: "ffprobeMissing", path: "/x/ffmpeg" } }), false).problem, "/x/ffmpeg 옆에서 ffprobe를 찾지 못했습니다.");
  assert.equal(usableInfo(located(info({ meetsMinimum: false }))), null);
});

test("feature list: what this FFmpeg does, hardware as measured", () => {
  const rows = featureList(ko, info(), "6.0", REPORT, "macos").map((f) => `${f.name} — ${f.what} [${f.tone}]`);
  assert.deepEqual(rows, [
    "ffmpeg · ffprobe 9.0.2 — 최소 6.0 [ok]",
    "libx264 — 미리보기 프록시 [ok]",
    "libx265 — H.265 (CPU) [ok]",
    "hevc_videotoolbox — H.265 하드웨어 (VideoToolbox) · 실측 통과 [ok]",
    "libsvtav1 — AV1 (CPU) [ok]",
    "AV1 하드웨어 — 이 Mac에는 없음, CPU로 처리 [off]",
    "libopus — WebM 오디오 (Opus) [ok]",
  ]);
  // While the test runs; a failed one says why; an encoder the app does not use yet.
  const pending = featureList(ko, info(), "6.0", null, "macos").find((f) => f.name === "hevc_videotoolbox")!;
  assert.deepEqual([pending.what, pending.tone], ["H.265 하드웨어 (VideoToolbox) · 실측 중…", "pending"]);
  const win = featureList(ko, info({}, { hevcHardware: ["hevc_nvenc", "hevc_mf"], av1Hardware: ["av1_nvenc"], libopus: false }), "6.0", REPORT, "windows");
  const nvenc = win.find((f) => f.name === "hevc_nvenc")!;
  assert.deepEqual([nvenc.what, nvenc.tone, nvenc.why], ["H.265 하드웨어 (NVENC) · 실측 실패, CPU로 처리", "warn", "No NVENC capable devices found"]);
  assert.equal(win.find((f) => f.name === "hevc_mf")!.tone, "off");
  assert.equal(win.find((f) => f.name === "av1_nvenc")!.what, "AV1 하드웨어 · 아직 쓰지 않음 (실기기 확인 전), CPU로 처리");
  assert.equal(win.find((f) => f.name === "libopus")!.what, "없음 — 일괄 변환에서 WebM을 고를 수 없음");
  // A build without encoders the app needs (an LGPL build).
  const lgpl = featureList(ko, info({}, { libx264: false, libx265: false, hevcHardware: [] }), "6.0", REPORT, "linux");
  assert.deepEqual(
    lgpl.filter((f) => f.tone === "warn").map((f) => f.name),
    ["libx264", "libx265"],
  );
  assert.equal(lgpl.find((f) => f.name === "H.265 하드웨어")!.what, "이 FFmpeg에는 없음, CPU(libx265)로 처리");
  assert.equal(lgpl.find((f) => f.name === "AV1 하드웨어")!.what, "이 FFmpeg에는 없음, CPU로 처리");
});

test("install guide per OS, saved path line", () => {
  assert.deepEqual([osOf("macos"), osOf("windows"), osOf("linux"), osOf(null)], ["macos", "windows", "linux", "macos"]);
  const commands = (os: "macos" | "windows" | "linux") => INSTALL_GUIDES[os].steps.flatMap((s) => (s.command ? [s.command] : []));
  assert.deepEqual([commands("macos"), commands("windows"), commands("linux")], [["brew install ffmpeg"], ["winget install --id Gyan.FFmpeg -e"], ["sudo apt install ffmpeg"]]);
  assert.match(INSTALL_GUIDES.linux.caution!.text, /22\.04|4\.4/);
  assert.equal(guideLine(ko, "macos", "macos"), "이 컴퓨터는 macOS로 감지되었습니다.");
  assert.equal(guideLine(ko, "macos", "linux"), "Ubuntu 안내를 보고 있습니다 (이 컴퓨터는 macOS).");
  assert.equal(pathPlaceholder("windows"), "C:\\ffmpeg\\bin\\ffmpeg.exe");

  assert.deepEqual(savedLine(ko, located(info())), { text: "저장된 경로 없음 — 앱을 켤 때 PATH와 기본 설치 위치에서 찾습니다.", saved: null });
  const mine = "/Users/me/내 FFmpeg 빌드/ffmpeg";
  assert.match(savedLine(ko, located(info({ ffmpegPath: mine }), { savedPath: mine })).text, /^앱을 켤 때 이 경로로 먼저 찾습니다/);
  assert.match(savedLine(ko, located(info(), { savedPath: mine })).text, /^지금은 쓸 수 없어 자동으로 찾았습니다/);
  assert.equal(savedLine(ko, located(info(), { savedPath: mine })).saved, mine);
});

test("English: status, sources, features, guides", () => {
  assert.deepEqual(setupStatus(en, located(info()), false), {
    state: "ready",
    title: "FFmpeg found",
    detail: "FFmpeg 9.0.2 · /opt/homebrew/bin/ffmpeg · installed with Homebrew",
    problem: null,
  });
  const old = setupStatus(en, located(info({ meetsMinimum: false }), { problem: { code: "ffmpegTooOld", version: "5.1.2", min: "6.0" } }), false);
  assert.deepEqual([old.title, old.problem], ["This FFmpeg can't be used", "FFmpeg 5.1.2 is older than the minimum version, 6.0."]);
  assert.deepEqual([sourceLabel(ko, "system"), sourceLabel(en, "system"), sourceLabel(en, "manual"), sourceLabel(en, "PATH")], ["시스템 패키지", "System package", "Installed by hand", "PATH"]);
  const rows = featureList(en, info(), "6.0", REPORT, "macos").map((f) => `${f.name} — ${f.what}`);
  assert.equal(rows[3], "hevc_videotoolbox — H.265 hardware (VideoToolbox) · test passed");
  assert.equal(rows[5], "AV1 hardware — Not on this Mac, uses the CPU");
  assert.deepEqual(installGuides(en).macos.steps.map((x) => x.command ?? null), [null, "brew install ffmpeg", null], "the same commands");
  assert.equal(guideLine(en, "macos", "linux"), "Showing the Ubuntu guide (this computer is macOS).");
  assert.match(savedLine(en, located(info())).text, /^No saved path/);
});
