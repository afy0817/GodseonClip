//! The editor's 내보내기 as a queue item: the edit (자르기 · 구간 제거) and the way (무손실 ·
//! 정밀, codec — H.265 · AV1 · GIF too —, quality, 프레임레이트) planned into FFmpeg work, the
//! result's names and a summary for the UI.
//!
//! [`prepare`] plans before anything is queued, so a refused export shows in the editor at once;
//! [`Prepared::into_task`] picks the names when the item joins the queue (never a name another
//! queued item holds). Every FFmpeg argument still comes from `command.rs`.

use crate::command::{self, AudioPlan, Finish, LosslessCutPlan, OutputCodec, PreciseCutPlan, PreciseTrimPlan, Quality, VideoCodec};
use crate::effects::{self, Effects};
use crate::encoders::{self, EncoderChoice, EncoderReport};
use crate::ffmpeg::FfmpegInfo;
use crate::jobs::{self, Attempt};
use crate::msg::Msg;
use crate::plan;
use crate::probe::MediaInfo;
use crate::queue::{Task, Work};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ExportMode {
    /// Stream copy (키프레임 단위).
    Lossless,
    /// Re-encode (프레임 단위), also what H.265 · AV1 need.
    Precise,
}

/// What the editor's mode asks for. The UI never sends kept ranges for a cut: Rust works them out.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Edit {
    /// 자르기: the one range to keep.
    Trim { start: f64, end: f64 },
    /// 구간 제거: the ranges to take out (any order, may overlap).
    Cut { removed: Vec<plan::Range> },
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum EditKind {
    Trim,
    Cut,
}

/// The editor's 내보내기: the opened file, the edit and what to do with it. Rust decides every
/// FFmpeg detail.
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub path: String,
    pub edit: Edit,
    pub mode: ExportMode,
    pub codec: OutputCodec,
    pub quality: Quality,
    /// 화면 효과 (정밀 only); absent = none.
    #[serde(default)]
    pub effects: Effects,
    /// 프레임레이트 (정밀 only): at most this many frames per second; absent or None = 원본 (a GIF:
    /// the source's, at most [`command::GIF_MAX_FRAME_RATE`]).
    #[serde(default)]
    pub frame_rate: Option<u32>,
    /// GIF 크기: the GIF's short side at most this (never larger); absent or None = the picture's.
    /// Ignored for other codecs.
    #[serde(default)]
    pub gif_short: Option<u32>,
}

/// What the export will produce, for the UI.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummary {
    /// Where the result's first visible frame comes from (original timeline).
    pub visible_start: f64,
    /// Lossless MP4 / MOV: an edit list hides the frames before the requested start.
    pub exact_start: bool,
    /// Playing length of the result.
    pub length: f64,
    /// Re-encoding only (None: stream copy).
    pub codec: Option<VideoCodec>,
    pub encoder: Option<String>,
    /// Software encoder tried if the hardware one fails.
    pub fallback: Option<String>,
    pub ten_bit: bool,
    pub audio: AudioPlan,
    /// Kept pieces joined in the result (1 for a trim).
    pub pieces: usize,
    /// Lossless 구간 제거: removed ranges stream copy cannot take out (between two keyframes).
    pub kept_removals: Vec<plan::Range>,
    /// The result's width × height when 화면 효과 or GIF 크기 change the picture; None: the source's.
    pub picture: Option<[u32; 2]>,
    /// The result's frames per second when 프레임레이트 (or a GIF) changes it; None: the source's.
    pub frame_rate: Option<f64>,
    /// 모자이크 · 블러 boxes that show in the result.
    pub masks: usize,
    /// PiP files that show in the result.
    pub overlays: usize,
}

impl ExportSummary {
    /// A stream copy's summary, before its start, length and audio are filled in.
    fn stream_copy() -> Self {
        ExportSummary {
            visible_start: 0.0,
            exact_start: false,
            length: 0.0,
            codec: None,
            encoder: None,
            fallback: None,
            ten_bit: false,
            audio: AudioPlan::None,
            pieces: 1,
            kept_removals: Vec::new(),
            picture: None,
            frame_rate: None,
            masks: 0,
            overlays: 0,
        }
    }
}

/// What the queue shows about an export (`QueueItem.info`).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExportTask {
    /// The original's path.
    pub source: String,
    pub source_name: String,
    pub edit: EditKind,
    pub mode: ExportMode,
    pub codec: OutputCodec,
    pub quality: Quality,
    pub summary: ExportSummary,
}

/// A planned export, waiting for its names ([`Prepared::into_task`]).
pub struct Prepared {
    task: ExportTask,
    input: PathBuf,
    program: PathBuf,
    how: How,
}

enum PrecisePlan {
    Trim(PreciseTrimPlan),
    Cut(PreciseCutPlan),
}

enum How {
    /// One stream copy: 자르기, or a 구간 제거 that keeps one piece.
    Copy { suffix: &'static str, video_index: Option<u32>, seek: f64, duration: f64 },
    /// One stream copy per kept piece, then the join (무손실 구간 제거).
    Pieces { plan: LosslessCutPlan, video_index: u32, ffprobe: PathBuf, tag: String },
    /// Re-encoding with the chosen encoder, then its software fallback.
    Encode { plan: PrecisePlan, info: Box<MediaInfo>, choice: EncoderChoice, quality: Quality },
    /// A GIF: the palette, then the frames (`jobs::gif_job`); `tag` names its scratch folder.
    Gif { plan: PrecisePlan, info: Box<MediaInfo>, quality: Quality, tag: String },
}

/// FFmpeg's own GIF encoder: always there, nothing to test, no fallback.
fn gif_encoder() -> EncoderChoice {
    EncoderChoice { name: "gif".into(), label: "GIF".into(), hardware: false, fallback: None, fallback_label: None }
}

fn copied_audio(info: &MediaInfo) -> AudioPlan {
    if info.audio.is_empty() { AudioPlan::None } else { AudioPlan::Copy }
}

/// Names a lossless cut's scratch folder: unique per export, ASCII.
fn scratch_tag() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}-{nanos}", std::process::id())
}

/// Plans `req` on the opened file (`info` and `keyframes` from its probe). A lossless cut of
/// several pieces runs FFmpeg once per join (the IDR check, `jobs::check_joins`) and a precise
/// export asks `encoder_report` (a few seconds the first time), so call this off the UI thread.
pub fn prepare(
    ff: &FfmpegInfo,
    info: &MediaInfo,
    keyframes: &[f64],
    req: &ExportRequest,
    encoder_report: impl FnOnce() -> EncoderReport,
) -> Result<Prepared, Msg> {
    let input = PathBuf::from(&req.path);
    let program = PathBuf::from(&ff.ffmpeg_path);
    let (summary, how) = match (req.mode, &req.edit) {
        (ExportMode::Lossless, _) if req.codec != OutputCodec::Source => {
            return Err(Msg::LosslessKeepsCodec);
        }
        (ExportMode::Lossless, _) if !req.effects.is_none() => {
            return Err(Msg::EffectsNeedPrecise);
        }
        (ExportMode::Lossless, _) if req.frame_rate.is_some() => {
            return Err(Msg::FrameRateNeedsPrecise);
        }
        (ExportMode::Lossless, Edit::Trim { start, end }) => {
            let plan = command::plan_lossless_trim(info, keyframes, *start, *end)?;
            let summary = ExportSummary {
                visible_start: plan.visible_start,
                exact_start: plan.exact_start,
                length: (plan.seek + plan.duration - plan.visible_start).max(0.0),
                audio: copied_audio(info),
                ..ExportSummary::stream_copy()
            };
            let video_index = info.video.as_ref().map(|v| v.index);
            (summary, How::Copy { suffix: "_trim", video_index, seek: plan.seek, duration: plan.duration })
        }
        (ExportMode::Lossless, Edit::Cut { removed }) => {
            let plan = command::plan_lossless_cut(info, keyframes, removed)?;
            let video = info.video.as_ref().ok_or(Msg::NoVideoTrack)?;
            let summary = ExportSummary {
                visible_start: plan.pieces[0].visible_start,
                exact_start: plan.exact_start,
                length: plan.length,
                audio: copied_audio(info),
                pieces: plan.pieces.len(),
                kept_removals: plan.kept_removals.clone(),
                ..ExportSummary::stream_copy()
            };
            let how = if let [piece] = plan.pieces.as_slice() {
                How::Copy { suffix: "_cut", video_index: Some(video.index), seek: piece.seek, duration: piece.duration }
            } else {
                // Checked before anything is written: every piece after a join starts a new coded sequence.
                jobs::check_joins(&program, &input, video, &plan)?;
                How::Pieces { plan, video_index: video.index, ffprobe: PathBuf::from(&ff.ffprobe_path), tag: scratch_tag() }
            };
            (summary, how)
        }
        (ExportMode::Precise, edit) => {
            let mut plan = match edit {
                Edit::Trim { start, end } => PrecisePlan::Trim(command::plan_precise_trim(info, *start, *end, req.codec)?),
                Edit::Cut { removed } => PrecisePlan::Cut(command::plan_precise_cut(info, removed, req.codec)?),
            };
            let video = info.video.as_ref().ok_or(Msg::NoVideoTrack)?;
            // Masks show at their times in the result: the filters run on the trimmed / joined picture.
            let keep = match &plan {
                PrecisePlan::Trim(p) => {
                    let start = p.seek.unwrap_or(0.0);
                    vec![plan::Range::new(start, start + p.duration)]
                }
                PrecisePlan::Cut(p) => p.keep.clone(),
            };
            let mut picture = effects::plan(video, &req.effects)?;
            if let Some(p) = picture.as_mut() {
                // The PiP files: what each is (video · image, length, sound) decides its inputs.
                let ffprobe = Path::new(&ff.ffprobe_path);
                let sources = p
                    .overlays
                    .iter()
                    .map(|o| {
                        let path = Path::new(&o.path);
                        crate::probe::probe_overlay(ffprobe, path).map_err(|why| Msg::OverlayUnreadable {
                            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| o.path.clone()),
                            why: Box::new(why),
                        })
                    })
                    .collect::<Result<Vec<_>, Msg>>()?;
                p.attach_sources(sources);
            }
            let picture = picture.and_then(|p| p.in_result(&keep));
            let (picture_size, masks, overlays) = picture.as_ref().map_or((None, 0, 0), |p| (Some(p.size), p.masks.len(), p.overlays.len()));
            let mixes = command::mixes_overlay_audio(picture.as_ref());
            let gif = VideoCodec::Gif == match &plan {
                PrecisePlan::Trim(p) => p.codec,
                PrecisePlan::Cut(p) => p.codec,
            };
            // 프레임레이트, and GIF 크기 from the size the 화면 효과 made.
            let (w, h) = video.upright_size();
            let shown = picture_size.unwrap_or([w, h]);
            let finish = Finish {
                fps: command::plan_frame_rate(video, req.frame_rate, gif)?,
                scale: if gif { command::plan_gif_scale(shown, req.gif_short)? } else { None },
            };
            match &mut plan {
                PrecisePlan::Trim(p) => {
                    p.effects = picture;
                    p.finish = finish;
                    // Mixing PiP sound re-encodes the audio.
                    if mixes && p.audio == AudioPlan::Copy {
                        p.audio = AudioPlan::Aac;
                    }
                }
                PrecisePlan::Cut(p) => {
                    p.effects = picture;
                    p.finish = finish;
                }
            }
            let (codec, ten_bit, audio, visible_start, length, pieces) = match &plan {
                PrecisePlan::Trim(p) => (p.codec, p.ten_bit, p.audio, p.seek.unwrap_or(0.0), p.duration, 1),
                PrecisePlan::Cut(p) => (p.codec, p.ten_bit, p.audio, p.keep[0].start, p.duration, p.keep.len()),
            };
            // A GIF needs no encoder test: FFmpeg always has its own.
            let choice = if gif {
                gif_encoder()
            } else {
                let choices = encoders::export_choices(&encoder_report());
                choices.for_codec(codec).ok_or(Msg::NoEncoder { codec })?.clone()
            };
            let summary = ExportSummary {
                visible_start,
                exact_start: true,
                length,
                codec: Some(codec),
                encoder: Some(choice.label.clone()),
                fallback: choice.fallback_label.clone(),
                ten_bit,
                audio,
                pieces,
                kept_removals: Vec::new(),
                picture: finish.scale.or(picture_size),
                frame_rate: finish.fps,
                masks,
                overlays,
            };
            let how = if gif {
                How::Gif { plan, info: Box::new(info.clone()), quality: req.quality, tag: scratch_tag() }
            } else {
                How::Encode { plan, info: Box::new(info.clone()), choice, quality: req.quality }
            };
            (summary, how)
        }
    };
    let edit = match req.edit {
        Edit::Trim { .. } => EditKind::Trim,
        Edit::Cut { .. } => EditKind::Cut,
    };
    let task = ExportTask {
        source: req.path.clone(),
        source_name: info.file_name.clone(),
        edit,
        mode: req.mode,
        codec: req.codec,
        quality: req.quality,
        summary,
    };
    Ok(Prepared { task, input, program, how })
}

impl Prepared {
    pub fn summary(&self) -> &ExportSummary {
        &self.task.summary
    }

    /// The queue item, with names next to the original that no file and no name in `taken` (the
    /// queue's) holds. Nothing is written yet.
    pub fn into_task(self, taken: &dyn Fn(&Path) -> bool) -> Result<Task<ExportTask>, Msg> {
        let Prepared { task, input, program, how } = self;
        match how {
            How::Copy { suffix, video_index, seek, duration } => {
                let (final_path, part) = command::output_paths_avoiding(&input, suffix, None, taken);
                let args = command::lossless_trim_args(&input, video_index, seek, duration, &part);
                let command_line = command::display_command(&program, &args);
                let work = Work::Attempts { attempts: vec![Attempt { args, label: String::new() }], total_secs: duration };
                Ok(Task { info: task, program, work, final_path, part, command_line })
            }
            How::Pieces { plan, video_index, ffprobe, tag } => {
                let (final_path, part) = command::output_paths_avoiding(&input, "_cut", None, taken);
                let length = plan.length;
                let build = {
                    let (program, input, final_path, part) = (program.clone(), input.clone(), final_path.clone(), part.clone());
                    move || jobs::lossless_cut_job(&program, &ffprobe, &input, video_index, &plan, &final_path, &part, &tag)
                };
                // Building only lists the commands: the scratch folder is made when the job runs.
                let command_line = build()?.command_line;
                let work = Work::Steps { build: Arc::new(build), length };
                Ok(Task { info: task, program, work, final_path, part, command_line })
            }
            How::Encode { plan, info, choice, quality } => {
                let (suffix, extension, length) = match &plan {
                    PrecisePlan::Trim(p) => (&p.suffix, &p.extension, p.duration),
                    PrecisePlan::Cut(p) => (&p.suffix, &p.extension, p.duration),
                };
                let (final_path, part) = command::output_paths_avoiding(&input, suffix, Some(extension), taken);
                let video = info.video.as_ref().ok_or(Msg::NoVideoTrack)?;
                let args = |encoder: &str| match &plan {
                    PrecisePlan::Trim(p) => command::precise_trim_args(&input, video, p, encoder, quality, &part),
                    PrecisePlan::Cut(p) => command::precise_cut_args(&input, &info, p, encoder, quality, &part),
                };
                let mut attempts = vec![Attempt { args: args(&choice.name)?, label: choice.label.clone() }];
                if let (Some(name), Some(label)) = (&choice.fallback, &choice.fallback_label) {
                    attempts.push(Attempt { args: args(name)?, label: label.clone() });
                }
                let command_line = command::display_command(&program, &attempts[0].args);
                let work = Work::Attempts { attempts, total_secs: length };
                Ok(Task { info: task, program, work, final_path, part, command_line })
            }
            How::Gif { plan, info, quality, tag } => {
                let (suffix, extension, length) = match &plan {
                    PrecisePlan::Trim(p) => (&p.suffix, &p.extension, p.duration),
                    PrecisePlan::Cut(p) => (&p.suffix, &p.extension, p.duration),
                };
                let (final_path, part) = command::output_paths_avoiding(&input, suffix, Some(extension), taken);
                let video = info.video.clone().ok_or(Msg::NoVideoTrack)?;
                let build = {
                    let (program, input, final_path, part) = (program.clone(), input.clone(), final_path.clone(), part.clone());
                    move || {
                        let source = match &plan {
                            PrecisePlan::Trim(p) => jobs::GifSource::Trim { video: &video, plan: p },
                            PrecisePlan::Cut(p) => jobs::GifSource::Cut { info: &info, plan: p },
                        };
                        jobs::gif_job(&program, &input, source, quality, &final_path, &part, &tag)
                    }
                };
                // Building only lists the commands: the scratch folder is made when the job runs.
                let command_line = build()?.command_line;
                let work = Work::Steps { build: Arc::new(build), length };
                Ok(Task { info: task, program, work, final_path, part, command_line })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shapes `src/lib/ipc.ts` sends.
    #[test]
    fn export_request_edits_from_the_ui() {
        let cut: ExportRequest = serde_json::from_str(
            r#"{"path":"/v/제주 여행.mov","edit":{"kind":"cut","removed":[{"start":130,"end":185},{"start":460,"end":500}]},
                "mode":"precise","codec":"hevc","quality":"balanced"}"#,
        )
        .unwrap();
        assert_eq!(cut.edit, Edit::Cut { removed: vec![plan::Range::new(130.0, 185.0), plan::Range::new(460.0, 500.0)] });
        assert_eq!((cut.mode, cut.codec, cut.quality), (ExportMode::Precise, OutputCodec::Hevc, Quality::Balanced));
        let trim: ExportRequest = serde_json::from_str(
            r#"{"path":"a.mp4","edit":{"kind":"trim","start":1.5,"end":4},"mode":"lossless","codec":"source","quality":"high"}"#,
        )
        .unwrap();
        assert_eq!(trim.edit, Edit::Trim { start: 1.5, end: 4.0 });
        assert!(serde_json::from_str::<ExportRequest>(r#"{"path":"a","start":1,"end":2,"mode":"precise","codec":"source","quality":"high"}"#).is_err());
    }

    fn ff() -> FfmpegInfo {
        FfmpegInfo {
            // Never runs: the join check skips what it cannot run.
            ffmpeg_path: "/없는 경로/ffmpeg".into(),
            ffprobe_path: "/없는 경로/ffprobe".into(),
            version: "9.0.2".into(),
            release: Some("9.0.2".into()),
            libavcodec_major: Some(63),
            meets_minimum: true,
            source: "test".into(),
            video_encoders: vec![],
            features: Default::default(),
        }
    }

    fn info(path: &Path) -> MediaInfo {
        MediaInfo {
            path: path.display().to_string(),
            file_name: path.file_name().unwrap().to_string_lossy().into_owned(),
            size_bytes: None,
            duration: 10.0,
            format_name: "mov,mp4,m4a,3gp,3g2,mj2".into(),
            bit_rate: None,
            video: Some(crate::probe::VideoStream {
                index: 0,
                codec: "h264".into(),
                profile: None,
                width: 320,
                height: 240,
                start_time: Some(0.0),
                has_b_frames: 0,
                fps: Some(25.0),
                avg_fps: Some(25.0),
                pix_fmt: Some("yuv420p".into()),
                color_primaries: None,
                color_transfer: None,
                color_space: None,
                hdr: false,
                rotation: None,
                vfr_suspect: false,
                frame_times: None,
            }),
            audio: vec![],
            subtitle_count: 0,
            other_count: 0,
        }
    }

    const KF: [f64; 10] = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];

    fn request(path: &Path, edit: Edit, mode: ExportMode, codec: OutputCodec) -> ExportRequest {
        ExportRequest {
            path: path.display().to_string(),
            edit,
            mode,
            codec,
            quality: Quality::Balanced,
            effects: Effects::default(),
            frame_rate: None,
            gif_short: None,
        }
    }

    fn report(tests: &[(&str, &str, bool)]) -> EncoderReport {
        EncoderReport {
            tests: tests
                .iter()
                .map(|(name, codec, hardware)| encoders::EncoderTest {
                    name: name.to_string(),
                    codec: codec.to_string(),
                    hardware: *hardware,
                    ok: true,
                    millis: 10,
                    error: None,
                })
                .collect(),
            preferred_hevc: None,
            preferred_av1: None,
        }
    }

    fn no_report() -> EncoderReport {
        panic!("a lossless export never needs the encoder test")
    }

    fn name(p: &Path) -> String {
        p.file_name().unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn lossless_trim_is_one_stream_copy_under_a_free_name() {
        let input = Path::new("/영상/제주 여행 01.mp4");
        let media = info(input);
        let req = request(input, Edit::Trim { start: 2.0, end: 5.0 }, ExportMode::Lossless, OutputCodec::Source);
        let prepared = prepare(&ff(), &media, &KF, &req, no_report).unwrap();
        assert_eq!((prepared.summary().visible_start, prepared.summary().exact_start, prepared.summary().codec), (2.0, true, None));
        let task = prepared.into_task(&|_| false).unwrap();
        assert_eq!((name(&task.final_path), name(&task.part)), ("제주 여행 01_trim.mp4".into(), "제주 여행 01_trim.part.mp4".into()));
        let plan = command::plan_lossless_trim(&media, &KF, 2.0, 5.0).unwrap();
        let expected = command::lossless_trim_args(input, Some(0), plan.seek, plan.duration, &task.part);
        let Work::Attempts { attempts, total_secs } = &task.work else { panic!("one run") };
        assert_eq!((attempts.len(), &attempts[0].args, *total_secs), (1, &expected, plan.duration));
        assert_eq!(task.command_line, command::display_command(Path::new("/없는 경로/ffmpeg"), &expected));
        assert_eq!((task.info.edit, task.info.mode, task.info.source_name.as_str()), (EditKind::Trim, ExportMode::Lossless, "제주 여행 01.mp4"));

        // The same export again while the first is queued: the next free name.
        let held = [task.final_path.clone(), task.part.clone()];
        let again = prepare(&ff(), &media, &KF, &req, no_report).unwrap().into_task(&|p| held.iter().any(|h| h == p)).unwrap();
        assert_eq!(name(&again.final_path), "제주 여행 01_trim (1).mp4");
        let Work::Attempts { attempts, .. } = &again.work else { panic!() };
        assert_eq!(attempts[0].args.last().unwrap(), again.part.as_os_str(), "writes its own .part");
    }

    #[test]
    fn lossless_cut_of_several_pieces_runs_as_steps_and_writes_nothing_yet() {
        let dir = std::env::temp_dir().join(format!("godseon-export-steps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("회의 녹화.mp4");
        let removed = vec![plan::Range::new(1.2, 2.5), plan::Range::new(5.5, 6.5)];
        let req = request(&input, Edit::Cut { removed }, ExportMode::Lossless, OutputCodec::Source);
        let prepared = prepare(&ff(), &info(&input), &KF, &req, no_report).unwrap();
        assert_eq!(prepared.summary().pieces, 3);
        let task = prepared.into_task(&|_| false).unwrap();
        assert_eq!(name(&task.final_path), "회의 녹화_cut.mp4");
        let Work::Steps { length, .. } = &task.work else { panic!("pieces, then the join") };
        assert!((length - task.info.summary.length).abs() < 1e-9);
        assert_eq!(task.command_line.lines().count(), 4, "three pieces and the join");
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert!(left.is_empty(), "no scratch folder until the job runs");
        std::fs::remove_dir_all(&dir).unwrap();

        // Keeping a single piece is a trim, named as a cut.
        let one = request(&input, Edit::Cut { removed: vec![plan::Range::new(0.0, 3.0)] }, ExportMode::Lossless, OutputCodec::Source);
        let task = prepare(&ff(), &info(&input), &KF, &one, no_report).unwrap().into_task(&|_| false).unwrap();
        assert!(matches!(&task.work, Work::Attempts { attempts, .. } if attempts.len() == 1));
        assert_eq!(name(&task.final_path), "회의 녹화_cut.mp4");
    }

    #[test]
    fn precise_export_tries_the_hardware_encoder_then_software() {
        let input = Path::new("/영상/출장 브이로그.mp4");
        let media = info(input);
        let req = request(input, Edit::Trim { start: 1.0, end: 4.0 }, ExportMode::Precise, OutputCodec::Hevc);
        let prepared =
            prepare(&ff(), &media, &KF, &req, || report(&[("hevc_videotoolbox", "hevc", true), ("libx265", "hevc", false)])).unwrap();
        let s = prepared.summary().clone();
        assert_eq!((s.codec, s.encoder.as_deref(), s.fallback.as_deref()), (Some(VideoCodec::Hevc), Some("VideoToolbox"), Some("libx265")));
        let task = prepared.into_task(&|_| false).unwrap();
        assert!(name(&task.final_path).ends_with("_h265.mp4"), "{}", name(&task.final_path));
        let plan = command::plan_precise_trim(&media, 1.0, 4.0, OutputCodec::Hevc).unwrap();
        let video = media.video.as_ref().unwrap();
        let Work::Attempts { attempts, total_secs } = &task.work else { panic!() };
        let labels: Vec<&str> = attempts.iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, ["VideoToolbox", "libx265"]);
        for (attempt, encoder) in attempts.iter().zip(["hevc_videotoolbox", "libx265"]) {
            let expected = command::precise_trim_args(input, video, &plan, encoder, Quality::Balanced, &task.part).unwrap();
            assert_eq!(attempt.args, expected);
        }
        assert_eq!(*total_secs, plan.duration);
    }

    #[test]
    fn effects_come_with_the_request_and_shape_the_precise_export() {
        let req: ExportRequest = serde_json::from_str(
            r#"{"path":"/v/세로 영상.mp4","edit":{"kind":"trim","start":1,"end":4},"mode":"precise","codec":"hevc","quality":"balanced",
                "effects":{"crop":{"x":40,"y":0,"width":180,"height":240,"ratio":null},"turn":1,"flipH":false,"flipV":false,"size":{"kind":"source"}}}"#,
        )
        .unwrap();
        assert_eq!(req.effects.crop.map(|c| (c.x, c.width)), Some((40, 180)));
        let older: ExportRequest = serde_json::from_str(
            r#"{"path":"a.mp4","edit":{"kind":"trim","start":1.5,"end":4},"mode":"lossless","codec":"source","quality":"high"}"#,
        )
        .unwrap();
        assert!(older.effects.is_none(), "no effects field: none");

        let input = Path::new(&req.path);
        let media = info(input);
        let prepared = prepare(&ff(), &media, &KF, &req, || report(&[("libx265", "hevc", false)])).unwrap();
        assert_eq!(prepared.summary().picture, Some([240, 180]), "the 180 × 240 box turned");
        let task = prepared.into_task(&|_| false).unwrap();
        let Work::Attempts { attempts, .. } = &task.work else { panic!() };
        let args: Vec<String> = attempts[0].args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.windows(2).any(|w| w == ["-vf", "crop=180:240:40:0,transpose=clock"]), "{args:?}");

        // Without effects the summary has no picture size.
        let plain = request(input, Edit::Trim { start: 1.0, end: 4.0 }, ExportMode::Precise, OutputCodec::Hevc);
        let s = prepare(&ff(), &media, &KF, &plain, || report(&[("libx265", "hevc", false)])).unwrap().summary().clone();
        assert_eq!(s.picture, None);
    }

    #[test]
    fn lossless_export_refuses_effects_and_bad_effects_are_refused() {
        let input = Path::new("/영상/a.mp4");
        let mut req = request(input, Edit::Trim { start: 1.0, end: 4.0 }, ExportMode::Lossless, OutputCodec::Source);
        req.effects.flip_h = true;
        assert_eq!(prepare(&ff(), &info(input), &KF, &req, no_report).err().unwrap(), Msg::EffectsNeedPrecise);
        req.mode = ExportMode::Precise;
        req.effects.crop = Some(effects::Crop { x: 300, y: 0, width: 64, height: 64, ratio: None });
        let e = prepare(&ff(), &info(input), &KF, &req, || report(&[("libx264", "h264", false)])).err().unwrap();
        assert_eq!(e, Msg::CropOutsidePicture { width: 320, height: 240 });
    }

    #[test]
    fn refused_exports_say_why() {
        let input = Path::new("/영상/a.mp4");
        let lossless_hevc = request(input, Edit::Trim { start: 1.0, end: 4.0 }, ExportMode::Lossless, OutputCodec::Hevc);
        assert_eq!(prepare(&ff(), &info(input), &KF, &lossless_hevc, no_report).err().unwrap(), Msg::LosslessKeepsCodec);
        let av1 = request(input, Edit::Trim { start: 1.0, end: 4.0 }, ExportMode::Precise, OutputCodec::Av1);
        let e = prepare(&ff(), &info(input), &KF, &av1, || report(&[("libx265", "hevc", false)])).err().unwrap();
        assert_eq!(e, Msg::NoEncoder { codec: VideoCodec::Av1 });
        let nothing = request(input, Edit::Cut { removed: vec![] }, ExportMode::Lossless, OutputCodec::Source);
        assert!(prepare(&ff(), &info(input), &KF, &nothing, no_report).is_err());
    }

    /// What the editor sends for a GIF: GIF 크기 and 프레임레이트 next to the codec.
    #[test]
    fn gif_export_runs_as_two_steps_without_an_encoder_test() {
        let dir = std::env::temp_dir().join(format!("godseon-export-gif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("출장 브이로그.mp4");
        let req: ExportRequest = serde_json::from_str(&format!(
            r#"{{"path":{},"edit":{{"kind":"trim","start":1,"end":4}},"mode":"precise","codec":"gif","quality":"balanced","frameRate":15,"gifShort":120}}"#,
            serde_json::to_string(&input.display().to_string()).unwrap()
        ))
        .unwrap();
        assert_eq!((req.codec, req.frame_rate, req.gif_short), (OutputCodec::Gif, Some(15), Some(120)));
        let prepared = prepare(&ff(), &info(&input), &KF, &req, no_report).unwrap();
        let s = prepared.summary().clone();
        assert_eq!(
            (s.codec, s.encoder.as_deref(), s.fallback, s.audio, s.ten_bit, s.picture, s.frame_rate, s.length),
            (Some(VideoCodec::Gif), Some("GIF"), None, AudioPlan::None, false, Some([160, 120]), Some(15.0), 3.0)
        );
        let task = prepared.into_task(&|_| false).unwrap();
        assert_eq!((name(&task.final_path), name(&task.part)), ("출장 브이로그_trim.gif".into(), "출장 브이로그_trim.part.gif".into()));
        let Work::Steps { length, .. } = &task.work else { panic!("palette, then frames") };
        assert_eq!(*length, 3.0);
        let lines: Vec<&str> = task.command_line.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("palettegen") && lines[1].contains("paletteuse"), "{lines:?}");
        assert!(lines[0].contains("fps=15:round=up,scale=160:120:flags=lanczos"), "{}", lines[0]);
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none(), "no scratch folder until the job runs");
        std::fs::remove_dir_all(&dir).unwrap();

        // A cut as a GIF, 원본 frame rate (the source's 25), the picture's own size.
        let cut = ExportRequest {
            frame_rate: None,
            ..request(&input, Edit::Cut { removed: vec![plan::Range::new(1.2, 2.5), plan::Range::new(5.5, 6.5)] }, ExportMode::Precise, OutputCodec::Gif)
        };
        let prepared = prepare(&ff(), &info(&input), &KF, &cut, no_report).unwrap();
        assert_eq!((prepared.summary().frame_rate, prepared.summary().picture, prepared.summary().pieces), (Some(25.0), None, 3));
        let task = prepared.into_task(&|_| false).unwrap();
        assert_eq!(name(&task.final_path), "출장 브이로그_cut.gif");
        assert!(task.command_line.contains("concat=n=3"), "{}", task.command_line);
    }

    #[test]
    fn frame_rate_needs_precise_and_shapes_the_video() {
        let input = Path::new("/영상/a.mp4");
        let mut req = request(input, Edit::Trim { start: 1.0, end: 4.0 }, ExportMode::Lossless, OutputCodec::Source);
        req.frame_rate = Some(24);
        assert_eq!(prepare(&ff(), &info(input), &KF, &req, no_report).err().unwrap(), Msg::FrameRateNeedsPrecise);
        req.mode = ExportMode::Precise;
        req.codec = OutputCodec::Hevc;
        let prepared = prepare(&ff(), &info(input), &KF, &req, || report(&[("libx265", "hevc", false)])).unwrap();
        assert_eq!((prepared.summary().frame_rate, prepared.summary().picture), (Some(24.0), None));
        let task = prepared.into_task(&|_| false).unwrap();
        let Work::Attempts { attempts, .. } = &task.work else { panic!() };
        let args: Vec<String> = attempts[0].args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.windows(2).any(|w| w == ["-vf", "fps=24:round=up"]), "{args:?}");
        // GIF 크기 means nothing for a video; a cap at the source's 25 fps changes nothing.
        req.gif_short = Some(120);
        req.frame_rate = Some(25);
        let s = prepare(&ff(), &info(input), &KF, &req, || report(&[("libx265", "hevc", false)])).unwrap().summary().clone();
        assert_eq!((s.frame_rate, s.picture), (None, None));
        req.frame_rate = Some(0);
        assert_eq!(prepare(&ff(), &info(input), &KF, &req, || report(&[("libx265", "hevc", false)])).err().unwrap(), Msg::FrameRateOutOfRange { min: 1, max: 120 });
    }
}
