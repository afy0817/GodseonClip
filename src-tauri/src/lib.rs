//! GodseonClip — Tauri entry point and IPC commands.
//!
//! The UI only decides *what* to do; every FFmpeg argument is built here in Rust.

pub mod command;
pub mod convert;
pub mod effects;
pub mod encoders;
pub mod estimate;
pub mod export;
pub mod ffmpeg;
pub mod jobs;
pub mod msg;
pub mod plan;
pub mod preview;
pub mod probe;
pub mod queue;
pub mod settings;
pub mod util;

use convert::{ConvertQueued, ConvertRequest, ConvertTask, ScanResult};
use export::{ExportRequest, ExportTask};
use msg::Msg;
use queue::{Queue, QueueEvent, QueueItem, QueueSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};

/// What the queue shows about an item: the editor's export or one file of a 일괄 변환.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TaskInfo {
    Export(ExportTask),
    Convert(ConvertTask),
}

struct AppState {
    ffmpeg: Mutex<Option<ffmpeg::FfmpegInfo>>,
    /// Last probe of each opened file, so edit commands only need the path from the UI.
    probes: Mutex<HashMap<String, ProbeResult>>,
    /// Last probe of each file on the 일괄 변환 list (no keyframes: a conversion takes the whole file).
    media: Mutex<HashMap<String, probe::MediaInfo>>,
    /// Every FFmpeg job: queue items, proxies, the diagnostics trim.
    jobs: Arc<jobs::JobManager>,
    /// 작업 큐: the editor's exports and the 일괄 변환 files.
    queue: Queue<TaskInfo>,
    /// Proxy jobs still running, by final proxy path, so opening the same file twice shares one job.
    proxy_jobs: Mutex<HashMap<PathBuf, ProxyJob>>,
    /// Held while FFmpeg is located and the settings file is read and written, one request at a time.
    settings_io: Mutex<()>,
    /// Last three-frame encoder test, for the FFmpeg it ran with. Precise exports pick from it.
    encoder_report: Mutex<Option<(String, encoders::EncoderReport)>>,
    /// The quit question is on screen (one at a time).
    asking_to_quit: AtomicBool,
    /// 예상 크기: test encodes of the 일괄 변환 list, paused while the queue runs.
    estimates: estimate::Estimator,
    /// Where the test encodes write (emptied at start and exit).
    estimate_dir: PathBuf,
    /// The language in use (the settings' pick, else the system's): for the native quit dialog.
    lang: Mutex<settings::Lang>,
    /// The system's language, read once at start.
    system_lang: settings::Lang,
}

fn settings_file(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join(settings::FILE_NAME))
}

fn emit_queue_event(app: &AppHandle, event: QueueEvent<TaskInfo>) {
    let _ = match event {
        QueueEvent::Changed(s) => {
            // Test encodes give the CPU to the queue while it runs an item.
            if let Some(state) = app.try_state::<AppState>() {
                state.estimates.set_busy(s.items.iter().any(|i| i.state == queue::ItemState::Running));
            }
            app.emit("queue-changed", s)
        }
        QueueEvent::Progress(p) => app.emit("queue-progress", p),
    };
}

fn emit_estimate_event(app: &AppHandle, event: estimate::EstimateEvent) {
    let _ = match event {
        estimate::EstimateEvent::Entry(e) => app.emit("estimate-changed", e),
        estimate::EstimateEvent::Paused(p) => app.emit("estimate-paused", p),
    };
}

/// The encoder test of this FFmpeg: the cached one, or run now (a few seconds). Held under the
/// cache lock so two callers never run it at once.
fn encoder_report(app: &AppHandle, ff: &ffmpeg::FfmpegInfo, rerun: bool) -> encoders::EncoderReport {
    let state = app.state::<AppState>();
    let mut cache = state.encoder_report.lock().unwrap();
    if !rerun {
        if let Some((path, report)) = cache.as_ref() {
            if *path == ff.ffmpeg_path {
                return report.clone();
            }
        }
    }
    let report = encoders::run(ff);
    *cache = Some((ff.ffmpeg_path.clone(), report.clone()));
    report
}

fn current_ffmpeg(state: &State<'_, AppState>) -> Result<ffmpeg::FfmpegInfo, Msg> {
    state
        .ffmpeg
        .lock()
        .unwrap()
        .clone()
        .ok_or(Msg::FfmpegNotReady)
}

fn probed(state: &State<'_, AppState>, path: &str) -> Result<ProbeResult, Msg> {
    state
        .probes
        .lock()
        .unwrap()
        .get(path)
        .cloned()
        .ok_or(Msg::NotProbed)
}

fn emit_job_event(app: &AppHandle, event: jobs::JobEvent) {
    let _ = match event {
        jobs::JobEvent::Progress(p) => app.emit("job-progress", p),
        jobs::JobEvent::Finished(f) => app.emit("job-finished", f),
    };
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, Msg> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| Msg::raw(e.to_string()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlatformInfo {
    os: String,
    arch: String,
}

#[tauri::command]
fn platform_info() -> PlatformInfo {
    PlatformInfo { os: std::env::consts::OS.into(), arch: std::env::consts::ARCH.into() }
}

/// The language at start (before the first screen) and for the settings screen.
#[tauri::command]
async fn language(app: AppHandle) -> Result<settings::LanguageInfo, Msg> {
    blocking(move || {
        let state = app.state::<AppState>();
        let _one_at_a_time = state.settings_io.lock().unwrap();
        let chosen = settings_file(&app).and_then(|f| settings::chosen_lang(&settings::load(&f).settings));
        settings::language_info(chosen, state.system_lang, None)
    })
    .await
}

/// The settings screen's 언어 · Language: None follows the system. Applies at once (the UI
/// switches its dictionary itself), and is saved for the next start.
#[tauri::command]
async fn set_language(app: AppHandle, setting: Option<settings::Lang>) -> Result<settings::LanguageInfo, Msg> {
    blocking(move || {
        let state = app.state::<AppState>();
        let _one_at_a_time = state.settings_io.lock().unwrap();
        let notice = match settings_file(&app) {
            Some(file) => settings::save_language(&file, setting).err(),
            None => Some(Msg::NoConfigFolder),
        };
        let info = settings::language_info(setting, state.system_lang, notice);
        *state.lang.lock().unwrap() = info.effective;
        info
    })
    .await
}

/// Locates FFmpeg for one settings request, saves the user's path when it changed, and makes the
/// result the FFmpeg every other command uses.
async fn run_ffmpeg_request(app: AppHandle, request: settings::FfmpegRequest) -> Result<ffmpeg::LocateResult, Msg> {
    blocking(move || {
        let state = app.state::<AppState>();
        let _one_at_a_time = state.settings_io.lock().unwrap();
        let file = settings_file(&app);
        let result =
            settings::apply_ffmpeg_request(file.as_deref(), &request, ffmpeg::check_path, || ffmpeg::locate(None));
        *state.ffmpeg.lock().unwrap() = result.info.clone().filter(|i| i.meets_minimum);
        result
    })
    .await
}

/// App start and "다시 찾기": the saved path first, then PATH and the install folders.
#[tauri::command]
async fn locate_ffmpeg(app: AppHandle) -> Result<ffmpeg::LocateResult, Msg> {
    run_ffmpeg_request(app, settings::FfmpegRequest::UseSaved).await
}

/// "이 경로로 확인": uses and saves `path` (the ffmpeg file or its folder) when it works.
#[tauri::command]
async fn choose_ffmpeg_path(app: AppHandle, path: String) -> Result<ffmpeg::LocateResult, Msg> {
    run_ffmpeg_request(app, settings::FfmpegRequest::Choose(path)).await
}

/// "자동 탐색으로 되돌리기": forgets the saved path.
#[tauri::command]
async fn forget_ffmpeg_path(app: AppHandle) -> Result<ffmpeg::LocateResult, Msg> {
    run_ffmpeg_request(app, settings::FfmpegRequest::Forget).await
}

/// "인코더 실측" on the settings screen: always runs again, and precise exports use the new result.
#[tauri::command]
async fn test_encoders(app: AppHandle, state: State<'_, AppState>) -> Result<encoders::EncoderReport, Msg> {
    let info = current_ffmpeg(&state)?;
    blocking(move || encoder_report(&app, &info, true)).await
}

/// The encoder test exports and conversions pick from (runs it the first time; "다시 검사" on the
/// settings screen reruns it through `test_encoders`). The settings screen's feature list shows it.
#[tauri::command]
async fn encoder_results(app: AppHandle, state: State<'_, AppState>) -> Result<encoders::EncoderReport, Msg> {
    let info = current_ffmpeg(&state)?;
    blocking(move || encoder_report(&app, &info, false)).await
}

/// Which encoder a precise export to each codec will use (tests the encoders the first time).
#[tauri::command]
async fn export_encoders(app: AppHandle, state: State<'_, AppState>) -> Result<encoders::ExportEncoders, Msg> {
    let info = current_ffmpeg(&state)?;
    blocking(move || encoders::export_choices(&encoder_report(&app, &info, false))).await
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProbeResult {
    info: probe::MediaInfo,
    keyframes: Vec<f64>,
    keyframe_error: Option<Msg>,
}

#[tauri::command]
async fn probe_media(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<ProbeResult, Msg> {
    let ff = current_ffmpeg(&state)?;
    let file = PathBuf::from(&path);
    // Dropped paths can be anything Finder / Explorer holds. The name is enough (a full path
    // wraps over several lines above the player).
    if file.is_dir() {
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
        return Err(Msg::CannotOpenFolder { name });
    }
    if !file.is_file() {
        return Err(Msg::file_not_found(&path));
    }
    let probed = file.clone();
    let result = blocking(move || {
        let ffprobe = Path::new(&ff.ffprobe_path);
        let mut info = probe::probe(ffprobe, &probed)?;
        // One packet listing gives the keyframes and every frame's time (variable rate or not).
        let (keyframes, keyframe_error) = match info.video.as_mut() {
            Some(v) => match probe::frame_times(ffprobe, &probed, v.index) {
                Ok(t) => {
                    probe::measure_frame_rate(v, t.frames);
                    (t.keyframes, None)
                }
                Err(e) => (Vec::new(), Some(e)),
            },
            None => (Vec::new(), None),
        };
        Ok::<_, Msg>(ProbeResult { info, keyframes, keyframe_error })
    })
    .await??;
    // Let the webview play exactly this file through the asset protocol, nothing else — and only
    // once it opened as media (not every file that was dropped).
    app.asset_protocol_scope().allow_file(&file).map_err(|e| Msg::raw(e.to_string()))?;
    state.probes.lock().unwrap().insert(path, result.clone());
    Ok(result)
}

/// PiP: reads a video or image to put over the picture and lets the webview show exactly that file
/// (the editor previews it over the playing source).
#[tauri::command]
async fn probe_overlay(app: AppHandle, state: State<'_, AppState>, path: String) -> Result<probe::OverlaySource, Msg> {
    let ff = current_ffmpeg(&state)?;
    let file = PathBuf::from(&path);
    if file.is_dir() {
        return Err(Msg::IsFolder);
    }
    if !file.is_file() {
        return Err(Msg::file_not_found(&path));
    }
    let probed = file.clone();
    let source = blocking(move || probe::probe_overlay(Path::new(&ff.ffprobe_path), &probed)).await??;
    app.asset_protocol_scope().allow_file(&file).map_err(|e| Msg::raw(e.to_string()))?;
    Ok(source)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrimRequest {
    path: String,
    start: f64,
    end: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartedJob {
    id: u64,
    output: String,
    command_line: String,
    plan: command::LosslessTrimPlan,
}

#[tauri::command]
fn start_lossless_trim(
    app: AppHandle,
    state: State<'_, AppState>,
    req: TrimRequest,
) -> Result<StartedJob, Msg> {
    let ff = current_ffmpeg(&state)?;
    let probed = probed(&state, &req.path)?;
    let plan = command::plan_lossless_trim(&probed.info, &probed.keyframes, req.start, req.end)?;
    let input = PathBuf::from(&req.path);
    let (final_path, part) = command::output_paths(&input, "_trim");
    let video_index = probed.info.video.as_ref().map(|v| v.index);
    let args = command::lossless_trim_args(&input, video_index, plan.seek, plan.duration, &part);
    let program = PathBuf::from(&ff.ffmpeg_path);
    let command_line = command::display_command(&program, &args);
    let id = state
        .jobs
        .start(&program, args, plan.duration, part, final_path.clone(), move |event| emit_job_event(&app, event))
        .map_err(Msg::launch)?;
    Ok(StartedJob { id, output: final_path.display().to_string(), command_line, plan })
}

/// The editor's 내보내기: plans the export (a refusal comes back at once) and adds it to the queue.
/// The item may already be running when this returns.
#[tauri::command]
async fn queue_export(app: AppHandle, state: State<'_, AppState>, req: ExportRequest) -> Result<QueueItem<TaskInfo>, Msg> {
    let ff = current_ffmpeg(&state)?;
    let probed = probed(&state, &req.path)?;
    let prepared = {
        let app = app.clone();
        blocking(move || export::prepare(&ff, &probed.info, &probed.keyframes, &req, || encoder_report(&app, &ff, false))).await??
    };
    state.queue.add(|taken| prepared.into_task(taken).map(|t| t.map_info(TaskInfo::Export)))
}

/// The encoders the 일괄 변환 screen offers: "자동" and every one that passed the test (runs the
/// encoder test the first time).
#[tauri::command]
async fn encoder_options(app: AppHandle, state: State<'_, AppState>) -> Result<encoders::EncoderOptions, Msg> {
    let info = current_ffmpeg(&state)?;
    blocking(move || encoders::encoder_options(&encoder_report(&app, &info, false))).await
}

/// 파일 추가 · 폴더 추가 · dropped paths: the files to list (a folder's video files, subfolders too).
#[tauri::command]
async fn convert_scan(paths: Vec<String>) -> Result<ScanResult, Msg> {
    blocking(move || convert::scan(&paths.iter().map(PathBuf::from).collect::<Vec<_>>(), convert::SCAN_LIMIT)).await
}

fn probe_for_list(ff: &ffmpeg::FfmpegInfo, path: &str) -> Result<probe::MediaInfo, Msg> {
    let file = Path::new(path);
    if file.is_dir() {
        return Err(Msg::IsFolder);
    }
    if !file.is_file() {
        return Err(Msg::FileNotFound { path: None });
    }
    probe::probe(Path::new(&ff.ffprobe_path), file)
}

/// One file of the 일괄 변환 list: what ffprobe says (codec · size · rate · length), kept for 변환 시작.
#[tauri::command]
async fn convert_probe(app: AppHandle, state: State<'_, AppState>, path: String) -> Result<probe::MediaInfo, Msg> {
    let ff = current_ffmpeg(&state)?;
    blocking(move || {
        let info = probe_for_list(&ff, &path)?;
        app.state::<AppState>().media.lock().unwrap().insert(path, info.clone());
        Ok(info)
    })
    .await?
}

/// 변환 시작: every file becomes a queue item (in list order), or comes back refused with why.
/// Settings that hold for every file (name ending, folder, encoder) refuse the whole request.
#[tauri::command]
async fn queue_convert(app: AppHandle, state: State<'_, AppState>, req: ConvertRequest) -> Result<ConvertQueued, Msg> {
    let ff = current_ffmpeg(&state)?;
    convert::check_settings(&req.settings)?;
    blocking(move || {
        let state = app.state::<AppState>();
        let report = encoder_report(&app, &ff, false);
        let choice = encoders::pick(&report, req.settings.options.codec, req.settings.encoder.as_deref())?;
        let cached = |path: &str| state.media.lock().unwrap().get(path).cloned();
        let media = |path: &str| cached(path).map_or_else(|| probe_for_list(&ff, path), Ok);
        let estimated = |path: &str, p: &convert::Prepared| estimate::key_for(path, p).and_then(|k| state.estimates.bytes(path, &k));
        Ok(convert::queue_files(&state.queue, &ff, &req, &choice, media, estimated, TaskInfo::Convert))
    })
    .await?
}

/// 예상 크기 of the files the convert screen will convert, with its settings (in list order): what
/// is known now. The rest is tried one file at a time and comes as `estimate-changed` events; a
/// test of a file or settings no longer asked for stops. An empty list stops everything.
#[tauri::command]
async fn estimate_sizes(app: AppHandle, state: State<'_, AppState>, req: ConvertRequest) -> Result<estimate::EstimateSnapshot, Msg> {
    let ff = current_ffmpeg(&state)?;
    blocking(move || {
        let state = app.state::<AppState>();
        if req.paths.is_empty() {
            return Ok(state.estimates.want(Vec::new()));
        }
        let report = encoder_report(&app, &ff, false);
        let choice = encoders::pick(&report, req.settings.options.codec, req.settings.encoder.as_deref())?;
        let cached = |path: &str| state.media.lock().unwrap().get(path).cloned();
        let files = req
            .paths
            .iter()
            .map(|path| {
                cached(path)
                    .map_or_else(|| probe_for_list(&ff, path), Ok)
                    .and_then(|info| convert::prepare(&ff, path, &info, &req.settings, &choice))
                    .and_then(|prepared| estimate::want_for(path, prepared))
                    .map_err(|e| (path.clone(), e))
            })
            .collect();
        Ok(state.estimates.want(files))
    })
    .await?
}

#[tauri::command]
fn queue_snapshot(state: State<'_, AppState>) -> QueueSnapshot<TaskInfo> {
    state.queue.snapshot()
}

#[tauri::command]
fn queue_cancel(state: State<'_, AppState>, id: u64) -> Result<(), Msg> {
    state.queue.cancel(id)
}

#[tauri::command]
fn queue_cancel_waiting(state: State<'_, AppState>) -> usize {
    state.queue.cancel_waiting()
}

#[tauri::command]
fn queue_remove(state: State<'_, AppState>, id: u64) -> Result<(), Msg> {
    state.queue.remove(id)
}

#[tauri::command]
fn queue_clear_finished(state: State<'_, AppState>) -> usize {
    state.queue.clear_finished()
}

/// `position` among the waiting items (0 = runs next).
#[tauri::command]
fn queue_move(state: State<'_, AppState>, id: u64, position: usize) -> Result<(), Msg> {
    state.queue.move_to(id, position)
}

#[tauri::command]
fn queue_restart(state: State<'_, AppState>, id: u64) -> Result<(), Msg> {
    state.queue.restart(id)
}

/// "동시 작업": applies at once and is saved for the next start. Err only when saving failed
/// (the number still applies to this run).
#[tauri::command]
async fn queue_set_concurrency(app: AppHandle, n: usize) -> Result<usize, Msg> {
    blocking(move || {
        let state = app.state::<AppState>();
        let n = state.queue.set_concurrency(n);
        let _one_at_a_time = state.settings_io.lock().unwrap();
        match settings_file(&app) {
            Some(file) => settings::save_queue_concurrency(&file, n).map(|()| n),
            None => Err(Msg::ConcurrencyNotSaved),
        }
    })
    .await?
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProxyJob {
    /// Running job (progress and finish come as job events); None when an earlier proxy is reused.
    id: Option<u64>,
    /// Where the proxy is, or will be once the job succeeds.
    path: String,
    ready: bool,
    command_line: Option<String>,
}

/// Starts (or reuses) the 540p preview proxy of an opened file. The proxy lives in the app cache
/// folder, never next to the original, and only it is added to the asset protocol scope.
#[tauri::command]
fn start_proxy(app: AppHandle, state: State<'_, AppState>, path: String) -> Result<ProxyJob, Msg> {
    let ff = current_ffmpeg(&state)?;
    let probed = probed(&state, &path)?;
    let video = probed.info.video.clone().ok_or(Msg::ProxyNeedsVideo)?;
    let input = PathBuf::from(&path);
    let cache = app.path().app_cache_dir().map_err(|e| Msg::raw(e.to_string()))?.join("proxies");
    std::fs::create_dir_all(&cache).map_err(|e| Msg::CacheFolderFailed { detail: e.to_string() })?;
    let (final_path, part) =
        preview::proxy_paths(&cache, &input).map_err(|e| Msg::FileUnreadable { detail: e.to_string() })?;

    // Held until the job is registered: the finish callback removes the entry under the same lock.
    let mut running = state.proxy_jobs.lock().unwrap();
    if let Some(job) = running.get(&final_path) {
        return Ok(job.clone());
    }
    if final_path.is_file() {
        app.asset_protocol_scope().allow_file(&final_path).map_err(|e| Msg::raw(e.to_string()))?;
        return Ok(ProxyJob { id: None, path: final_path.display().to_string(), ready: true, command_line: None });
    }
    if !ff.features.libx264 {
        return Err(Msg::ProxyNeedsX264);
    }
    let args = command::proxy_args(&input, &video, &part);
    let program = PathBuf::from(&ff.ffmpeg_path);
    let command_line = command::display_command(&program, &args);
    let key = final_path.clone();
    let events = app.clone();
    let id = state
        .jobs
        .start(&program, args, probed.info.duration, part, final_path.clone(), move |event| {
            if let jobs::JobEvent::Finished(f) = &event {
                if let Some(out) = f.output.as_deref() {
                    let _ = events.asset_protocol_scope().allow_file(out);
                }
                events.state::<AppState>().proxy_jobs.lock().unwrap().remove(&key);
            }
            emit_job_event(&events, event);
        })
        .map_err(Msg::launch)?;
    let job = ProxyJob {
        id: Some(id),
        path: final_path.display().to_string(),
        ready: false,
        command_line: Some(command_line),
    };
    running.insert(final_path, job.clone());
    Ok(job)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThumbnailRequest {
    path: String,
    /// Defaults: the whole file.
    start: Option<f64>,
    end: Option<f64>,
    count: u32,
    /// Pixels; default 90.
    height: Option<u32>,
}

#[tauri::command]
async fn thumbnail_strip(
    state: State<'_, AppState>,
    req: ThumbnailRequest,
) -> Result<preview::ThumbnailStrip, Msg> {
    let ff = current_ffmpeg(&state)?;
    let probed = probed(&state, &req.path)?;
    blocking(move || {
        preview::thumbnails(
            Path::new(&ff.ffmpeg_path),
            Path::new(&req.path),
            &probed.info,
            req.start,
            req.end,
            req.count,
            req.height.unwrap_or(90),
        )
    })
    .await?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WaveformRequest {
    path: String,
    /// Defaults: the whole file.
    start: Option<f64>,
    end: Option<f64>,
    buckets: u32,
}

#[tauri::command]
async fn audio_waveform(state: State<'_, AppState>, req: WaveformRequest) -> Result<preview::Waveform, Msg> {
    let ff = current_ffmpeg(&state)?;
    let probed = probed(&state, &req.path)?;
    blocking(move || {
        preview::waveform(Path::new(&ff.ffmpeg_path), Path::new(&req.path), &probed.info, req.start, req.end, req.buckets)
    })
    .await?
}

#[tauri::command]
fn cancel_job(state: State<'_, AppState>, id: u64) -> bool {
    state.jobs.cancel(id)
}

const QUIT_MENU_ID: &str = "godseon-quit";

/// macOS: the default app menu's Quit ends the app without asking it (no exit request reaches
/// Tauri), so it is replaced with an item of ours that asks first ([`request_quit`]).
fn install_quit_menu(app: &AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, MenuItemKind};
    let menu = Menu::default(app)?;
    if let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.into_iter().next() {
        let items = app_menu.items()?;
        if let Some(MenuItemKind::Predefined(_)) = items.last() {
            app_menu.remove_at(items.len() - 1)?;
            let label = format!("Quit {}", app.package_info().name);
            app_menu.append(&MenuItem::with_id(app, QUIT_MENU_ID, label, true, Some("CmdOrCtrl+Q"))?)?;
        }
    }
    app.set_menu(menu)?;
    Ok(())
}

/// Quits at once when no queue item runs or waits; otherwise asks first ("종료" / "계속 작업").
/// The jobs are canceled and cleaned up on the way out ([`shutdown`]).
fn request_quit(app: &AppHandle) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let state = app.state::<AppState>();
    let (running, waiting) = state.queue.active();
    if running + waiting == 0 {
        app.exit(0);
        return;
    }
    if state.asking_to_quit.swap(true, Ordering::SeqCst) {
        return;
    }
    let handle = app.clone();
    let texts = queue::quit_dialog(*state.lang.lock().unwrap(), running, waiting);
    let mut dialog = app
        .dialog()
        .message(texts.message)
        .title(texts.title)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(texts.quit, texts.keep));
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.parent(&window);
    }
    dialog.show(move |quit| {
        handle.state::<AppState>().asking_to_quit.store(false, Ordering::SeqCst);
        if quit {
            handle.exit(0);
        }
    });
}

/// How long quitting waits for the jobs to clean up. FFmpeg stops on `q`; one that does not is
/// killed after 3 s (JobManager::cancel).
pub const EXIT_WAIT: Duration = Duration::from_secs(8);

/// On the way out: nothing new starts, every job (queue items, test encodes, proxies) is canceled,
/// and this waits (`wait` at most) until each removed its `.part` and scratch folder; then the test
/// encodes' folder goes. False if a job still ran when the wait ran out. The app's own state is not
/// needed, so the integration tests run the same exit on their own queue.
pub fn close_and_clean_up<D: Clone + Serialize + Send + 'static>(
    queue: &Queue<D>,
    estimates: &estimate::Estimator,
    jobs: &jobs::JobManager,
    estimate_dir: &Path,
    wait: Duration,
) -> bool {
    queue.close();
    estimates.close();
    jobs.cancel_all();
    let idle = jobs.wait_idle(wait);
    estimate::clean(estimate_dir);
    idle
}

fn shutdown(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    close_and_clean_up(&state.queue, &state.estimates, &state.jobs, &state.estimate_dir, EXIT_WAIT);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let saved = settings_file(&handle).map(|f| settings::load(&f).settings).unwrap_or_default();
            let concurrency = saved.queue_concurrency.unwrap_or(1);
            let system_lang = settings::system_lang(sys_locale::get_locales());
            let lang = settings::chosen_lang(&saved).unwrap_or(system_lang);
            let jobs = Arc::new(jobs::JobManager::default());
            let events = handle.clone();
            let queue = Queue::new(jobs.clone(), concurrency, move |e| emit_queue_event(&events, e));
            // In the app cache, never next to the originals; a crash may have left a test behind.
            let estimate_dir = handle
                .path()
                .app_cache_dir()
                .map(|d| d.join("estimates"))
                .unwrap_or_else(|_| std::env::temp_dir().join("godseon-clip-estimates"));
            estimate::clean(&estimate_dir);
            let events = handle.clone();
            let estimates = estimate::Estimator::new(jobs.clone(), estimate_dir.clone(), move |e| emit_estimate_event(&events, e));
            app.manage(AppState {
                ffmpeg: Mutex::default(),
                probes: Mutex::default(),
                media: Mutex::default(),
                jobs,
                queue,
                proxy_jobs: Mutex::default(),
                settings_io: Mutex::default(),
                encoder_report: Mutex::default(),
                asking_to_quit: AtomicBool::new(false),
                estimates,
                estimate_dir,
                lang: Mutex::new(lang),
                system_lang,
            });
            if cfg!(target_os = "macos") {
                install_quit_menu(&handle)?;
            }
            Ok(())
        })
        .on_menu_event(|app, event| {
            if event.id() == QUIT_MENU_ID {
                request_quit(app);
            }
        })
        .on_window_event(|window, event| {
            // Closing the window quits the app: ask first while queue items run or wait.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let (running, waiting) = app.state::<AppState>().queue.active();
                if running + waiting > 0 {
                    api.prevent_close();
                    request_quit(app);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            platform_info,
            language,
            set_language,
            locate_ffmpeg,
            choose_ffmpeg_path,
            forget_ffmpeg_path,
            test_encoders,
            encoder_results,
            probe_media,
            probe_overlay,
            start_lossless_trim,
            export_encoders,
            queue_export,
            encoder_options,
            convert_scan,
            convert_probe,
            queue_convert,
            estimate_sizes,
            queue_snapshot,
            queue_cancel,
            queue_cancel_waiting,
            queue_remove,
            queue_clear_finished,
            queue_move,
            queue_restart,
            queue_set_concurrency,
            start_proxy,
            thumbnail_strip,
            audio_waveform,
            cancel_job
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    app.run(|app, event| {
        // Every way out ends here, the Dock's Quit and logging out included.
        if let RunEvent::Exit = event {
            shutdown(app);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `src/lib/ipc.ts` tells the two kinds apart by `kind`, next to the task's own fields.
    #[test]
    fn queue_info_is_tagged_with_its_kind() {
        let task = ConvertTask {
            source: "/영상/제주 여행.mov".into(),
            source_name: "제주 여행.mov".into(),
            quality: command::Quality::Balanced,
            summary: convert::ConvertSummary {
                length: 12.5,
                codec: command::VideoCodec::Av1,
                encoder: "SVT-AV1".into(),
                fallback: None,
                ten_bit: true,
                hdr: false,
                scaled: Some([1280, 720]),
                audio: command::AudioOut::Opus { bitrate: 128 },
                container: command::Container::Webm,
            },
            estimated_bytes: Some(1_500_000_000),
        };
        let json = serde_json::to_value(TaskInfo::Convert(task)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "convert",
                "source": "/영상/제주 여행.mov",
                "sourceName": "제주 여행.mov",
                "quality": "balanced",
                "summary": {
                    "length": 12.5, "codec": "av1", "encoder": "SVT-AV1", "fallback": null, "tenBit": true, "hdr": false,
                    "scaled": [1280, 720], "audio": {"kind": "opus", "bitrate": 128}, "container": "webm"
                },
                "estimatedBytes": 1_500_000_000u64
            })
        );
    }
}
