// Typed wrappers around the Rust commands in src-tauri/src/lib.rs.
// Field names mirror the Rust structs (serde camelCase) — change both sides together.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * msg::Msg — what Rust tells the user, as a code with arguments; the dictionaries (`msg` in
 * `i18n/ko.ts` · `en.ts`) word it (`say`). `raw`: text with no translation (an FFmpeg stderr line).
 * A Rust test checks every Rust variant is here.
 */
export type Msg =
  | { code: "raw"; text: string }
  // Files
  | { code: "fileNotFound"; path: string | null }
  | { code: "cannotOpenFolder"; name: string }
  | { code: "isFolder" }
  | { code: "fileUnreadable"; detail: string }
  | { code: "permissionDenied" }
  | { code: "notMedia" }
  | { code: "imageFile" }
  | { code: "noTracks" }
  | { code: "durationUnknown" }
  | { code: "emptyRange" }
  | { code: "noVideoTrack" }
  | { code: "noAudioTrack" }
  | { code: "probeFailed"; reason: string | null }
  | { code: "probeOutputUnreadable"; detail: string }
  | { code: "keyframesFailed" }
  | { code: "notProbed" }
  | { code: "pieceUnreadable" }
  | { code: "piecePacketsMissing" }
  // FFmpeg
  | { code: "ffmpegNotReady" }
  | { code: "ffmpegTooOld"; version: string; min: string }
  | { code: "ffprobeMissing"; path: string }
  | { code: "runFailed"; program: string; detail: string }
  | { code: "versionFailed"; program: string; detail: string | null }
  | { code: "encoderListFailed"; detail: string }
  | { code: "enteredPathUnusable"; why: Msg }
  | { code: "savedPathUnusable"; why: Msg }
  | { code: "ffmpegLaunchFailed"; detail: string }
  | { code: "ffmpegRelaunchFailed"; detail: string }
  | { code: "ffmpegFailed" }
  // Encoders
  | { code: "encoderTestTimedOut"; secs: number }
  | { code: "encoderTestFailed" }
  | { code: "encoderUnsupported"; encoder: string }
  | { code: "chosenEncoderUnavailable"; encoder: string }
  | { code: "noEncoder"; codec: VideoCodec }
  | { code: "retriedWith"; failed: string; next: string; reason: string | null }
  // Editor export
  | { code: "preciseNeedsVideo" }
  | { code: "losslessKeepsCodec" }
  | { code: "cutNeedsVideo" }
  | { code: "cutNeedsKeyframes" }
  | { code: "noRangesToRemove" }
  | { code: "removesEverything" }
  | { code: "nothingKept" }
  | { code: "nothingRemovableLossless" }
  | { code: "cutExtensionUnsupported"; extension: string }
  | { code: "openGopJoin"; at: number }
  | { code: "tempFolderFailed"; detail: string }
  | { code: "effectsNeedPrecise" }
  | { code: "frameRateNeedsPrecise" }
  | { code: "frameRateOutOfRange"; min: number; max: number }
  | { code: "cropOutsidePicture"; width: number; height: number }
  | { code: "cropTooSmall"; min: number }
  | { code: "outputSizeInvalid"; min: number; max: number }
  | { code: "pictureSizeUnknown" }
  | { code: "maskOutsidePicture"; width: number; height: number }
  | { code: "maskTooSmall"; min: number }
  | { code: "maskTimeInvalid" }
  | { code: "tooManyMasks"; max: number }
  | { code: "overlayOutsidePicture"; width: number; height: number }
  | { code: "overlayTooSmall"; min: number }
  | { code: "overlayTimeInvalid" }
  | { code: "tooManyOverlays"; max: number }
  | { code: "overlayUnreadable"; name: string; why: Msg }
  | { code: "concatListFailed"; detail: string }
  | { code: "renameFailed"; detail: string }
  // Convert
  | { code: "convertNeedsVideo" }
  | { code: "convertCodecUnsupported" }
  | { code: "codecNotInContainer"; codec: string; container: string }
  | { code: "webmNeedsOpus" }
  | { code: "outputFolderNotFound"; path: string }
  | { code: "suffixTooLong"; max: number }
  | { code: "suffixBadChar"; ch: string | null }
  | { code: "suffixPart" }
  | { code: "folderUnreadable"; path: string; detail: string }
  | { code: "noVideoFiles"; name: string }
  | { code: "tooManyFiles"; limit: number }
  // Estimate
  | { code: "estimateNeedsDuration" }
  | { code: "estimateFolderFailed"; detail: string }
  | { code: "sampleFailed"; why: Msg | null }
  | { code: "sampleEmpty" }
  // Preview
  | { code: "proxyNeedsVideo" }
  | { code: "proxyNeedsX264" }
  | { code: "cacheFolderFailed"; detail: string }
  | { code: "thumbnailsTimedOut" }
  | { code: "thumbnailsFailed" }
  | { code: "thumbnailsNoKeyframes" }
  | { code: "waveformTimedOut" }
  | { code: "waveformFailed" }
  // Queue
  | { code: "queueClosed" }
  | { code: "queueItemNotFound" }
  | { code: "itemFinished" }
  | { code: "cancelRunningFirst" }
  | { code: "onlyWaitingMoves" }
  | { code: "onlyStoppedRestart" }
  // Settings
  | { code: "settingsUnreadable"; path: string; detail: string }
  | { code: "settingsCorrupt"; path: string; detail: string }
  | { code: "settingsNotSaved"; path: string; detail: string }
  | { code: "noConfigFolder" }
  | { code: "thisRunOnly" }
  | { code: "concurrencyNotSaved" };

export type MsgCode = Msg["code"];
export type MsgOf<C extends MsgCode> = Extract<Msg, { code: C }>;

/** settings::Lang — the app's language. */
export type Lang = "ko" | "en";

/** settings::LanguageInfo — null setting: the system's language. */
export interface LanguageInfo {
  setting: Lang | null;
  effective: Lang;
  system: Lang;
  /** The pick could not be saved (it holds for this run). */
  notice: Msg | null;
}

export interface PlatformInfo {
  os: string; // "macos" | "windows" | "linux"
  arch: string;
}

export interface Features {
  libx264: boolean;
  libx265: boolean;
  libsvtav1: boolean;
  hevcHardware: string[];
  av1Hardware: string[];
  /** Opus audio encoder: 일괄 변환 to WebM needs it. */
  libopus: boolean;
}

export interface FfmpegInfo {
  ffmpegPath: string;
  ffprobePath: string;
  version: string;
  release: string | null;
  libavcodecMajor: number | null;
  meetsMinimum: boolean;
  /** A package manager's name ("Homebrew" …), or "system" (the OS's package) · "manual". */
  source: string;
  videoEncoders: string[];
  features: Features;
}

export interface LocateResult {
  found: boolean;
  info: FfmpegInfo | null;
  searched: string[];
  /** Why the found FFmpeg cannot be used (too old, no ffprobe, does not run). */
  problem: Msg | null;
  minVersion: string;
  /** The ffmpeg path in the settings file after this call; null: automatic search at start. */
  savedPath: string | null;
  /** For the user: why their own path was skipped, or that it could not be saved. */
  notices: Msg[];
}

export interface EncoderTest {
  name: string;
  codec: "hevc" | "av1" | "h264";
  hardware: boolean;
  ok: boolean;
  millis: number;
  error: Msg | null;
}

export interface EncoderReport {
  tests: EncoderTest[];
  preferredHevc: string | null;
  preferredAv1: string | null;
}

export interface VideoStream {
  index: number;
  codec: string;
  profile: string | null;
  width: number;
  height: number;
  startTime: number | null;
  hasBFrames: number;
  fps: number | null;
  avgFps: number | null;
  pixFmt: string | null;
  colorPrimaries: string | null;
  colorTransfer: string | null;
  colorSpace: string | null;
  hdr: boolean;
  rotation: number | null;
  /** Variable frame rate: the rates disagree, or the frames' own times are not steady. */
  vfrSuspect: boolean;
  /** Every frame's time, sorted, on a variable rate (probe::measure_frame_rate); absent otherwise. */
  frameTimes?: number[];
}

export interface AudioStream {
  index: number;
  codec: string;
  startTime: number | null;
  channels: number | null;
  sampleRate: number | null;
  language: string | null;
}

export interface MediaInfo {
  path: string;
  fileName: string;
  sizeBytes: number | null;
  duration: number;
  formatName: string;
  bitRate: number | null;
  video: VideoStream | null;
  audio: AudioStream[];
  subtitleCount: number;
  otherCount: number;
}

export interface ProbeResult {
  info: MediaInfo;
  keyframes: number[];
  keyframeError: Msg | null;
}

/** The file must have been opened with probeMedia first; Rust uses that probe. */
export interface TrimRequest {
  path: string;
  start: number;
  end: number;
}

export interface LosslessTrimPlan {
  seek: number;
  duration: number;
  keyframe: number | null;
  /** MP4/MOV: playback starts exactly at the requested start (edit lists). */
  exactStart: boolean;
  visibleStart: number;
}

export interface StartedJob {
  id: number;
  output: string;
  commandLine: string;
  plan: LosslessTrimPlan;
}

/** command::Quality — 고화질 · 균형 · 작은 용량. */
export type Quality = "high" | "balanced" | "small";
/** command::VideoCodec — the codec a re-encoded result has ("gif": the editor's export only). */
export type VideoCodec = "h264" | "hevc" | "av1" | "gif";
/** command::OutputCodec — "source" (원본 유지) re-encodes to the same codec, or H.264. */
export type OutputCodec = "source" | "hevc" | "av1" | "gif";
/** ExportMode in lib.rs: 무손실 (stream copy) or 정밀 (re-encode). */
export type ExportMode = "lossless" | "precise";
/** command::AudioPlan */
export type AudioPlan = "none" | "copy" | "aac";

/** encoders::EncoderChoice — the encoder a precise export uses, and its software fallback. */
export interface EncoderChoice {
  name: string;
  /** "VideoToolbox", "libx265", "SVT-AV1"… */
  label: string;
  hardware: boolean;
  fallback: string | null;
  fallbackLabel: string | null;
}

/** encoders::ExportEncoders — null: no working encoder for that codec (a GIF needs none of these). */
export interface ExportEncoders {
  h264: EncoderChoice | null;
  hevc: EncoderChoice | null;
  av1: EncoderChoice | null;
}

/** plan::Range — seconds on the original's timeline, end exclusive. */
export interface TimeRange {
  start: number;
  end: number;
}

/**
 * What the editor's mode asks for (lib.rs `Edit`). A cut sends only the ranges to remove; Rust works
 * out what stays (plan::keep_from_removed).
 */
export type Edit = { kind: "trim"; start: number; end: number } | { kind: "cut"; removed: TimeRange[] };

/** effects::Crop — the box in pixels of the upright source, and the 비율 it was locked to ([w, h] as seen). */
export interface Crop {
  x: number;
  y: number;
  width: number;
  height: number;
  ratio: [number, number] | null;
}

/** effects::Fit — 맞춤 (black bars) · 채움 (edges cut off) · 늘림. */
export type Fit = "contain" | "cover" | "stretch";

/** effects::OutputSize — 원본 · short side down to 1080 / 720 / 480 (never up) · exactly W × H. */
export type OutputSize =
  | { kind: "source" }
  | { kind: "short"; short: number }
  | { kind: "exact"; width: number; height: number; fit: Fit };

/** effects::MaskKind — 모자이크 (square cells) · 블러. */
export type MaskKind = "mosaic" | "blur";

/** effects::Mask — a box (upright source pixels) hidden from start to end (original timeline), strength 1–100. */
export interface Mask {
  x: number;
  y: number;
  width: number;
  height: number;
  start: number;
  end: number;
  kind: MaskKind;
  strength: number;
}

/** effects::Overlay — PiP: a video or image laid over a box (upright source pixels) from start to end
 * (original timeline); a video plays from `offset` in it and goes when it ends. */
export interface Overlay {
  path: string;
  x: number;
  y: number;
  width: number;
  height: number;
  start: number;
  end: number;
  offset: number;
  /** 0–100. */
  opacity: number;
  /** Mix the video's sound at `volume` (0–100). */
  audio: boolean;
  volume: number;
}

/** probe::OverlayKind */
export type OverlayKind = "video" | "image";

/** probe::OverlaySource — a file to put over the picture (probeOverlay). */
export interface OverlaySource {
  path: string;
  fileName: string;
  kind: OverlayKind;
  /** Upright size. */
  width: number;
  height: number;
  /** Video length; null for an image. */
  duration: number | null;
  hasAudio: boolean;
}

/** effects::Effects — 화면 효과: masks · PiP (source pixels) → crop → quarter turns clockwise · flips (as seen) → size. */
export interface Effects {
  crop: Crop | null;
  turn: number;
  flipH: boolean;
  flipV: boolean;
  size: OutputSize;
  masks: Mask[];
  overlays: Overlay[];
}

/** The file must have been opened with probeMedia first. Rust picks the encoder and every FFmpeg detail. */
export interface ExportRequest {
  path: string;
  edit: Edit;
  mode: ExportMode;
  codec: OutputCodec;
  quality: Quality;
  /** 화면 효과: 정밀 only (Rust refuses them with 무손실). */
  effects: Effects;
  /** 프레임레이트: 정밀 only, at most this many fps (never more than the source has); null: 원본. */
  frameRate: number | null;
  /** GIF 크기: a GIF's short side at most this (never larger); null: the picture's. Ignored for video. */
  gifShort: number | null;
}

export interface ExportSummary {
  /** Where the result's first visible frame comes from (original timeline). */
  visibleStart: number;
  /** Lossless MP4 / MOV: the edit list starts playback exactly at the requested start. */
  exactStart: boolean;
  length: number;
  /** null: stream copy. */
  codec: VideoCodec | null;
  encoder: string | null;
  /** Software encoder tried if the hardware one fails. */
  fallback: string | null;
  tenBit: boolean;
  audio: AudioPlan;
  /** Kept pieces joined in the result (1 for a trim). */
  pieces: number;
  /** Lossless 구간 제거: removed ranges stream copy cannot take out (between two keyframes). */
  keptRemovals: TimeRange[];
  /** The result's [width, height] when 화면 효과 or GIF 크기 change the picture; null: the source's. */
  picture: [number, number] | null;
  /** The result's fps when 프레임레이트 (or a GIF) changes it; null: the source's. */
  frameRate: number | null;
  /** 모자이크 · 블러 boxes that show in the result. */
  masks: number;
  /** PiP files that show in the result. */
  overlays: number;
}

/** export::EditKind — what the editor's mode made. */
export type EditKind = "trim" | "cut";

/** export::ExportTask — what the queue shows about an export (QueueItem.info). */
export interface ExportTask {
  /** The original's path. */
  source: string;
  sourceName: string;
  edit: EditKind;
  mode: ExportMode;
  codec: OutputCodec;
  quality: Quality;
  summary: ExportSummary;
}

/** command::Container — the 일괄 변환 컨테이너. H.265: mp4 · mkv · mov; AV1: mp4 · mkv · webm. */
export type Container = "mp4" | "mkv" | "mov" | "webm";
/** command::Resolution — keep, or bring the short side down to 1080 / 720 (never up). */
export type Resolution = "source" | "p1080" | "p720";
/** command::AudioChoice — "가능하면 복사, 아니면 AAC 192k" · "항상 AAC 192k" · "항상 AAC 128k". */
export type AudioChoice = "copyOrAac" | "aac192" | "aac128";
/** command::AudioOut — what a conversion does with the audio tracks (kbit/s per track). */
export type AudioOut = { kind: "none" } | { kind: "copy" } | { kind: "aac"; bitrate: number } | { kind: "opus"; bitrate: number };

/** convert::ConvertSettings — the 일괄 변환 screen's settings, the same for every file. */
export interface ConvertSettings {
  codec: "hevc" | "av1";
  container: Container;
  resolution: Resolution;
  audio: AudioChoice;
  /** 10 bits also for 8-bit sources (10-bit and HDR sources always keep 10 bits). */
  tenBit: boolean;
  /** 프레임레이트: at most this many fps (never more than the source has); null: 원본. */
  frameRate: number | null;
  /** null: "자동" (hardware first, then software); else an encoder name from encoderOptions. */
  encoder: string | null;
  quality: Quality;
  /** null: next to each original. */
  outputDir: string | null;
  /** Put after the original's name ("_h265"; may be empty). */
  suffix: string;
}

/** The files must have been probed with convertProbe (Rust probes again what it has not seen). */
export interface ConvertRequest {
  paths: string[];
  settings: ConvertSettings;
}

/** convert::ConvertSummary */
export interface ConvertSummary {
  length: number;
  codec: VideoCodec;
  encoder: string;
  fallback: string | null;
  tenBit: boolean;
  /** The source is HDR; its colors are kept. */
  hdr: boolean;
  /** Made smaller, to [width, height] as shown; null: the source size. */
  scaled: [number, number] | null;
  /** Brought down to this many fps; null: every frame of the source. */
  frameRate: number | null;
  audio: AudioOut;
  container: Container;
}

/** convert::ConvertTask — what the queue shows about one 일괄 변환 file. */
export interface ConvertTask {
  source: string;
  sourceName: string;
  quality: Quality;
  summary: ConvertSummary;
  /** 예상 크기 from the test encode, when it was ready as the file was queued. */
  estimatedBytes: number | null;
}

/** lib.rs TaskInfo — a queue item is an editor export or a 일괄 변환 file. */
export type TaskInfo = ({ kind: "export" } & ExportTask) | ({ kind: "convert" } & ConvertTask);

/** convert::ConvertQueued — what one 변환 시작 did. */
export interface ConvertQueued {
  /** The queue item made for each file, in list order. */
  queued: { path: string; id: number }[];
  /** Files that were not queued, and why. */
  refused: { path: string; error: Msg }[];
}

/** estimate::EstimateState — one file's 예상 크기 with the settings asked for. */
export type EstimateState =
  | { state: "waiting" }
  /** Share of all its stretches tried so far. */
  | { state: "running"; percent: number }
  /**
   * low · high: the size if the whole file were like its smallest · largest stretch. encoder: the one
   * the test ran with. whole: a short file tried whole (the conversion's size).
   */
  | { state: "done"; bytes: number; low: number; high: number; encoder: string; whole: boolean }
  | { state: "failed"; error: Msg };

/** estimate::EstimateEntry — `key` names the file's state and settings (empty: it could not be planned). */
export type EstimateEntry = { path: string; key: string } & EstimateState;

/** estimate::EstimateSnapshot — what is known now, one entry per path asked for. */
export interface EstimateSnapshot {
  /** The 작업 큐 runs an item: nothing is tried until it is done. */
  paused: boolean;
  entries: EstimateEntry[];
}

/** convert::ScanResult — the files to list for what was added. */
export interface ScanResult {
  files: string[];
  /** Folders without a video file, paths that are gone, the limit. */
  notices: Msg[];
}

/** encoders::EncoderOptions — the 일괄 변환 인코더 list. */
export interface EncoderOptions {
  /** What "자동" picks per codec. */
  auto: ExportEncoders;
  /** Every working encoder, hardware first. */
  hevc: EncoderChoice[];
  av1: EncoderChoice[];
}

/** queue::ItemState */
export type ItemState = "waiting" | "running" | "done" | "failed" | "canceled";

/** queue::QueueItem — one item of the 작업 큐. */
export interface QueueItem {
  id: number;
  info: TaskInfo;
  state: ItemState;
  /** Running, and asked to stop: FFmpeg is stopping. */
  canceling: boolean;
  /** The name the result gets (held for this item while it is in the queue); once done, the file made. */
  output: string;
  outputBytes: number | null;
  progress: JobProgress | null;
  /** An earlier attempt failed and the next one ran (why). */
  note: Msg | null;
  error: Msg | null;
  commandLine: string;
  /** End of FFmpeg's stderr, once finished. */
  log: string | null;
  /** Unix ms of the last start; null if it never ran. */
  startedAt: number | null;
  elapsedMs: number | null;
  /** Larger = finished later. */
  finishedOrder: number | null;
}

/** queue::QueueSnapshot — the whole queue, in queue order. */
export interface QueueSnapshot {
  /** Grows with every change: drop a snapshot older than the one shown. */
  version: number;
  concurrency: number;
  items: QueueItem[];
}

/** queue::ItemProgress */
export interface ItemProgress {
  id: number;
  progress: JobProgress;
}

/** Same rule as command::is_mov_family in Rust. */
export function isMovFamily(formatName: string): boolean {
  return formatName.split(",").some((f) => ["mov", "mp4", "m4a", "3gp", "3g2", "mj2"].includes(f.trim()));
}

export interface JobProgress {
  id: number;
  percent: number;
  outTime: number;
  speed: number | null;
  fps: number | null;
  /** Set once an attempt failed and the job started over with the next encoder (why). */
  note: Msg | null;
}

export interface JobFinished {
  id: number;
  ok: boolean;
  canceled: boolean;
  output: string | null;
  error: Msg | null;
  elapsedMs: number;
  /** An earlier attempt failed (e.g. the hardware encoder) and a later one ran. */
  note: Msg | null;
  /** End of FFmpeg's stderr (every run that wrote something). */
  log: string | null;
}

/** Preview proxy (540p H.264) of an opened file, in the app cache folder. */
export interface ProxyJob {
  /** Running job: progress and finish arrive as job events. null when an earlier proxy is reused. */
  id: number | null;
  /** Where the proxy is, or will be once the job succeeds (the finish event has the final path). */
  path: string;
  ready: boolean;
  commandLine: string | null;
}

/** Range defaults to the whole file. The file must have been opened with probeMedia first. */
export interface ThumbnailRequest {
  path: string;
  start?: number | null;
  end?: number | null;
  count: number;
  /** Pixels; default 90. */
  height?: number | null;
}

export interface ThumbnailStrip {
  /** Tile k covers [start + k·interval, start + (k+1)·interval) of the original's timeline. */
  start: number;
  interval: number;
  /** JPEG data URLs, one per tile; each shows the last keyframe at or before its tile's middle. */
  images: string[];
  elapsedMs: number;
}

export interface WaveformRequest {
  path: string;
  start?: number | null;
  end?: number | null;
  buckets: number;
}

export interface Waveform {
  /** Bucket k of n covers [start + k·(end−start)/n, start + (k+1)·(end−start)/n). */
  start: number;
  end: number;
  /** Peak |sample| per bucket of the first audio track, 0–1. */
  peaks: number[];
  elapsedMs: number;
}

export const api = {
  platformInfo: () => invoke<PlatformInfo>("platform_info"),
  /** The language to start in (the saved pick, else the system's). */
  language: () => invoke<LanguageInfo>("language"),
  /** 언어 · Language on the settings screen; null follows the system. Saved for the next start. */
  setLanguage: (setting: Lang | null) => invoke<LanguageInfo>("set_language", { setting }),
  /** App start and "다시 찾기": the saved path first, then PATH and the install folders. */
  locateFfmpeg: () => invoke<LocateResult>("locate_ffmpeg"),
  /** Uses `path` (the ffmpeg file or its folder) and saves it when it works; otherwise as locateFfmpeg. */
  chooseFfmpegPath: (path: string) => invoke<LocateResult>("choose_ffmpeg_path", { path: path.trim() }),
  /** Forgets the saved path and searches automatically. */
  forgetFfmpegPath: () => invoke<LocateResult>("forget_ffmpeg_path"),
  /** Runs the three-frame encoder test again (exports and conversions then pick from the new result). */
  testEncoders: () => invoke<EncoderReport>("test_encoders"),
  /** The encoder test exports and conversions pick from: the cached one, or run now the first time. */
  encoderResults: () => invoke<EncoderReport>("encoder_results"),
  probeMedia: (path: string) => invoke<ProbeResult>("probe_media", { path }),
  /** PiP: a video or image to put over the picture (the webview may then show it). */
  probeOverlay: (path: string) => invoke<OverlaySource>("probe_overlay", { path }),
  startLosslessTrim: (req: TrimRequest) => invoke<StartedJob>("start_lossless_trim", { req }),
  /** Which encoder a precise export to each codec uses (runs the encoder test the first time). */
  exportEncoders: () => invoke<ExportEncoders>("export_encoders"),
  /** The editor's 내보내기: plans it (a refusal rejects at once) and adds it to the 작업 큐. */
  queueExport: (req: ExportRequest) => invoke<QueueItem>("queue_export", { req }),
  /** 일괄 변환 인코더: "자동" and every working encoder (runs the encoder test the first time). */
  encoderOptions: () => invoke<EncoderOptions>("encoder_options"),
  /** 파일 추가 · 폴더 추가 · dropped paths → the files to list (a folder's video files, subfolders too). */
  convertScan: (paths: string[]) => invoke<ScanResult>("convert_scan", { paths }),
  /** One listed file's codec · size · rate · length (no keyframes); kept in Rust for 변환 시작. */
  convertProbe: (path: string) => invoke<MediaInfo>("convert_probe", { path }),
  /** 변환 시작: one queue item per file; settings that hold for all (name ending, folder, encoder) reject the call. */
  queueConvert: (req: ConvertRequest) => invoke<ConvertQueued>("queue_convert", { req }),
  /**
   * 예상 크기 of these files with these settings (list order): what is known now; the rest comes as
   * estimate-changed events, one file at a time. A test no longer asked for stops; [] stops all.
   */
  estimateSizes: (req: ConvertRequest) => invoke<EstimateSnapshot>("estimate_sizes", { req }),
  queueSnapshot: () => invoke<QueueSnapshot>("queue_snapshot"),
  /** Waiting: canceled before it starts. Running: FFmpeg stops, then the item is canceled. */
  queueCancel: (id: number) => invoke<void>("queue_cancel", { id }),
  queueCancelWaiting: () => invoke<number>("queue_cancel_waiting"),
  /** A waiting or finished item (a running one is canceled first). */
  queueRemove: (id: number) => invoke<void>("queue_remove", { id }),
  queueClearFinished: () => invoke<number>("queue_clear_finished"),
  /** `position` among the waiting items (0 runs next). */
  queueMove: (id: number, position: number) => invoke<void>("queue_move", { id, position }),
  /** A canceled or failed item waits again, at the end. */
  queueRestart: (id: number) => invoke<void>("queue_restart", { id }),
  /** 1–4, saved for the next start; rejects only when saving failed (it still applies). */
  queueSetConcurrency: (n: number) => invoke<number>("queue_set_concurrency", { n }),
  startProxy: (path: string) => invoke<ProxyJob>("start_proxy", { path }),
  thumbnailStrip: (req: ThumbnailRequest) => invoke<ThumbnailStrip>("thumbnail_strip", { req }),
  audioWaveform: (req: WaveformRequest) => invoke<Waveform>("audio_waveform", { req }),
  cancelJob: (id: number) => invoke<boolean>("cancel_job", { id }),
};

export const onJobProgress = (cb: (p: JobProgress) => void): Promise<UnlistenFn> =>
  listen<JobProgress>("job-progress", (e) => cb(e.payload));

export const onJobFinished = (cb: (f: JobFinished) => void): Promise<UnlistenFn> =>
  listen<JobFinished>("job-finished", (e) => cb(e.payload));

/** Anything but progress changed in the queue (whole snapshot). */
export const onQueueChanged = (cb: (s: QueueSnapshot) => void): Promise<UnlistenFn> =>
  listen<QueueSnapshot>("queue-changed", (e) => cb(e.payload));

export const onQueueProgress = (cb: (p: ItemProgress) => void): Promise<UnlistenFn> =>
  listen<ItemProgress>("queue-progress", (e) => cb(e.payload));

/** One file's 예상 크기 changed (drop it when its key is not the one asked for last). */
export const onEstimateChanged = (cb: (e: EstimateEntry) => void): Promise<UnlistenFn> =>
  listen<EstimateEntry>("estimate-changed", (e) => cb(e.payload));

/** The 작업 큐 started (true) or finished (false) running items: test encodes pause meanwhile. */
export const onEstimatePaused = (cb: (paused: boolean) => void): Promise<UnlistenFn> =>
  listen<boolean>("estimate-paused", (e) => cb(e.payload));

/**
 * A rejected command as a `Msg`: Rust rejects with one (`Result<_, Msg>`); anything else (a Tauri
 * error string, a JS Error) is raw text. Keep the Msg in state and word it when shown (`say`), so a
 * language switch rewords it too.
 */
export function toMsg(e: unknown): Msg {
  if (e && typeof e === "object" && typeof (e as { code?: unknown }).code === "string") return e as Msg;
  if (typeof e === "string") return { code: "raw", text: e };
  if (e instanceof Error) return { code: "raw", text: e.message };
  return { code: "raw", text: JSON.stringify(e) };
}
