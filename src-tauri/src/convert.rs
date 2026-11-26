//! 일괄 변환 (canvas Convert.dc.html): every file of the convert screen becomes its own 작업 큐
//! item — the whole file re-encoded to H.265 or AV1 with the screen's settings.
//!
//! As for the editor's export ([`crate::export`]), [`prepare`] plans a file before it is queued (a
//! refusal comes back for that file at once) and [`Prepared::into_task`] picks the names when the
//! item joins the queue. [`scan`] lists the video files in what was added, folders included. Every
//! FFmpeg argument still comes from `command.rs`.

use crate::command::{self, AudioOut, Container, ConvertOptions, ConvertPlan, Quality, VideoCodec};
use crate::encoders::EncoderChoice;
use crate::ffmpeg::FfmpegInfo;
use crate::jobs::Attempt;
use crate::msg::Msg;
use crate::plan;
use crate::probe::{MediaInfo, VideoStream};
use crate::queue::{Queue, Task, Work};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The convert screen's settings, the same for every file of one 변환 시작.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConvertSettings {
    /// 출력 코덱 · 컨테이너 · 해상도 · 오디오 · 10비트.
    #[serde(flatten)]
    pub options: ConvertOptions,
    /// None: "자동" (hardware first, then software). Some: an encoder `encoder_options` offered.
    pub encoder: Option<String>,
    pub quality: Quality,
    /// None: next to each original (원본과 같은 폴더).
    pub output_dir: Option<String>,
    /// Put after the original's name ("_h265"; may be empty).
    pub suffix: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConvertRequest {
    /// Files the screen listed and probed, in list order.
    pub paths: Vec<String>,
    pub settings: ConvertSettings,
}

/// What a conversion makes, for the queue and the convert screen.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConvertSummary {
    /// The whole file's length.
    pub length: f64,
    pub codec: VideoCodec,
    pub encoder: String,
    /// Software encoder tried if the hardware one fails.
    pub fallback: Option<String>,
    pub ten_bit: bool,
    /// The source is HDR; its colors are kept.
    pub hdr: bool,
    /// Made smaller, to [width, height] as shown; None: the source size.
    pub scaled: Option<[u32; 2]>,
    pub audio: AudioOut,
    pub container: Container,
}

/// What the queue shows about a conversion (`QueueItem.info`).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConvertTask {
    /// The original's path.
    pub source: String,
    pub source_name: String,
    pub quality: Quality,
    pub summary: ConvertSummary,
    /// 예상 크기 from the test encode ([`crate::estimate`]), when it was ready as the file was queued.
    pub estimated_bytes: Option<u64>,
}

/// One planned file, waiting for its names ([`Prepared::into_task`]).
pub struct Prepared {
    task: ConvertTask,
    input: PathBuf,
    program: PathBuf,
    video: VideoStream,
    plan: ConvertPlan,
    choice: EncoderChoice,
    quality: Quality,
    dir: PathBuf,
    suffix: String,
}

/// What is the same for every file: the name ending and the folder to save in.
pub fn check_settings(settings: &ConvertSettings) -> Result<(), Msg> {
    command::check_name_suffix(&settings.suffix)?;
    if let Some(dir) = &settings.output_dir {
        if !Path::new(dir).is_dir() {
            return Err(Msg::OutputFolderNotFound { path: dir.to_string() });
        }
    }
    Ok(())
}

/// Plans the file at `path` (`info` from its probe) with `choice`, the encoder picked once for the
/// whole 변환 시작 (`encoders::pick`).
pub fn prepare(ff: &FfmpegInfo, path: &str, info: &MediaInfo, settings: &ConvertSettings, choice: &EncoderChoice) -> Result<Prepared, Msg> {
    let plan = command::plan_convert(info, &settings.options)?;
    if matches!(plan.audio, AudioOut::Opus { .. }) && !ff.features.libopus {
        return Err(Msg::WebmNeedsOpus);
    }
    let video = info.video.clone().ok_or(Msg::ConvertNeedsVideo)?;
    let input = PathBuf::from(path);
    let dir = match &settings.output_dir {
        Some(d) => PathBuf::from(d),
        None => input.parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    let scaled = plan.scale.is_some().then_some(plan.size);
    let task = ConvertTask {
        source: path.to_string(),
        source_name: info.file_name.clone(),
        quality: settings.quality,
        estimated_bytes: None,
        summary: ConvertSummary {
            length: plan.duration,
            codec: plan.codec,
            encoder: choice.label.clone(),
            fallback: choice.fallback_label.clone(),
            ten_bit: plan.ten_bit,
            hdr: video.hdr,
            scaled,
            audio: plan.audio,
            container: plan.container,
        },
    };
    Ok(Prepared {
        task,
        input,
        program: PathBuf::from(&ff.ffmpeg_path),
        video,
        plan,
        choice: choice.clone(),
        quality: settings.quality,
        dir,
        suffix: settings.suffix.clone(),
    })
}

impl Prepared {
    pub fn task(&self) -> &ConvertTask {
        &self.task
    }

    /// What the conversion depends on besides the file: the encoders, the quality and the plan. Two
    /// files' tests with the same settings but other sizes (a 720p file under 1080p로 축소) differ
    /// only where their plans differ.
    pub fn settings_key(&self) -> String {
        let p = &self.plan;
        format!(
            "{}|{:?}|{:?}|{:?}|{}|{:?}|{:?}|{:?}",
            self.choice.name, self.choice.fallback, self.quality, p.codec, p.ten_bit, p.scale, p.audio, p.container
        )
    }

    /// The whole file's length.
    pub fn duration(&self) -> f64 {
        self.plan.duration
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    pub fn container(&self) -> Container {
        self.plan.container
    }

    /// The first encoder's name, and the software one tried after it.
    pub fn encoder_labels(&self) -> (String, Option<String>) {
        (self.choice.label.clone(), self.choice.fallback_label.clone())
    }

    /// The conversion over `sample` only, written to `output` ([`command::convert_sample_args`]):
    /// hardware then software as [`Prepared::into_task`] runs it, or the software encoder alone once
    /// the hardware one failed on an earlier stretch.
    pub fn sample_attempts(&self, sample: plan::Range, output: &Path, software_only: bool) -> Result<Vec<Attempt>, Msg> {
        let args = |encoder: &str| command::convert_sample_args(&self.input, &self.video, &self.plan, encoder, self.quality, sample, output);
        let fallback = self.choice.fallback.as_ref().zip(self.choice.fallback_label.as_ref());
        match fallback {
            Some((name, label)) if software_only => Ok(vec![Attempt { args: args(name)?, label: label.clone() }]),
            _ => {
                let mut attempts = vec![Attempt { args: args(&self.choice.name)?, label: self.choice.label.clone() }];
                if let Some((name, label)) = fallback {
                    attempts.push(Attempt { args: args(name)?, label: label.clone() });
                }
                Ok(attempts)
            }
        }
    }

    /// Puts the 예상 크기 measured for this file and these settings on the queue item.
    pub fn set_estimate(&mut self, bytes: Option<u64>) {
        self.task.estimated_bytes = bytes;
    }

    /// The queue item, named `{original}{suffix}.{container}` in its folder under a name no file and
    /// no name in `taken` (the queue's) holds. Nothing is written yet.
    pub fn into_task(self, taken: &dyn Fn(&Path) -> bool) -> Result<Task<ConvertTask>, Msg> {
        let Prepared { task, input, program, video, plan, choice, quality, dir, suffix } = self;
        let (final_path, part) = command::output_paths_in(&dir, &input, &suffix, Some(plan.container.extension()), taken);
        let args = |encoder: &str| command::convert_args(&input, &video, &plan, encoder, quality, &part);
        let mut attempts = vec![Attempt { args: args(&choice.name)?, label: choice.label.clone() }];
        if let (Some(name), Some(label)) = (&choice.fallback, &choice.fallback_label) {
            attempts.push(Attempt { args: args(name)?, label: label.clone() });
        }
        let command_line = command::display_command(&program, &attempts[0].args);
        let work = Work::Attempts { attempts, total_secs: plan.duration };
        Ok(Task { info: task, program, work, final_path, part, command_line })
    }
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueuedFile {
    pub path: String,
    /// The queue item made for it.
    pub id: u64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RefusedFile {
    pub path: String,
    pub error: Msg,
}

/// What one 변환 시작 did: an item per file, or why a file was not queued.
#[derive(Serialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConvertQueued {
    pub queued: Vec<QueuedFile>,
    pub refused: Vec<RefusedFile>,
}

/// Plans and queues every file of `req` in order (`media` gives a file's probe). A file that cannot
/// be converted is refused on its own; the others still go in. `estimated` gives the 예상 크기
/// already measured for a file and these settings; `wrap` makes the queue's info.
pub fn queue_files<D: Clone + Serialize + Send + 'static>(
    queue: &Queue<D>,
    ff: &FfmpegInfo,
    req: &ConvertRequest,
    choice: &EncoderChoice,
    media: impl Fn(&str) -> Result<MediaInfo, Msg>,
    estimated: impl Fn(&str, &Prepared) -> Option<u64>,
    wrap: impl Fn(ConvertTask) -> D,
) -> ConvertQueued {
    let mut out = ConvertQueued::default();
    for path in &req.paths {
        let added = media(path)
            .and_then(|info| prepare(ff, path, &info, &req.settings, choice))
            .and_then(|mut prepared| {
                prepared.set_estimate(estimated(path, &prepared));
                queue.add(|taken| prepared.into_task(taken).map(|t| t.map_info(&wrap)))
            });
        match added {
            Ok(item) => out.queued.push(QueuedFile { path: path.clone(), id: item.id }),
            Err(error) => out.refused.push(RefusedFile { path: path.clone(), error }),
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// 파일 · 폴더 추가

/// Extensions a folder's files need to be listed (FFmpeg reads more; a file added on its own is
/// listed whatever its name and the probe decides).
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "asf", "flv", "f4v", "ts", "m2ts", "mts", "m2t", "mpg", "mpeg",
    "vob", "3gp", "3g2", "ogv", "mxf", "dv", "divx",
];

/// At most this many files per 추가 (a folder tree can hold thousands).
pub const SCAN_LIMIT: usize = 1000;

#[derive(Serialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    /// In the order added; a folder's own files by name, then its subfolders'.
    pub files: Vec<String>,
    /// For the user: folders without a video file, paths that are gone, the limit.
    pub notices: Vec<Msg>,
}

fn is_video_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Hidden files and folders (`.DS_Store`, `._…`, a lossless cut's `.godseon-cut-…`) and the `.part`
/// files of jobs that are running or were cut short.
fn skipped_name(name: &str) -> bool {
    name.starts_with('.') || name.to_lowercase().contains(".part.")
}

fn take_digits(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut s = String::new();
    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
        s.push(c);
        it.next();
    }
    s
}

/// "클립 2" before "클립 10"; letters without case, then the plain order.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let (n, m) = (take_digits(&mut x), take_digits(&mut y));
                let (n, m) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let o = n.len().cmp(&m.len()).then_with(|| n.cmp(m));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_ascii_lowercase().cmp(&d.to_ascii_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

struct Walk {
    files: Vec<PathBuf>,
    seen: HashSet<PathBuf>,
    notices: Vec<Msg>,
    limit: usize,
}

impl Walk {
    /// False once the limit is reached.
    fn push(&mut self, path: PathBuf) -> bool {
        if self.files.len() >= self.limit {
            return false;
        }
        if self.seen.insert(path.clone()) {
            self.files.push(path);
        }
        true
    }

    /// The video files under `dir`, its own first. Linked folders are not followed (no loops).
    fn folder(&mut self, dir: &Path) -> bool {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                self.notices.push(Msg::FolderUnreadable { path: dir.display().to_string(), detail: e.to_string() });
                return true;
            }
        };
        let mut files = Vec::new();
        let mut folders = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if skipped_name(&name) {
                continue;
            }
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => folders.push((name, path)),
                // A link counts when it leads to a file (followed), never to a folder.
                Ok(t) if (t.is_file() || (t.is_symlink() && path.is_file())) && is_video_name(&name) => files.push((name, path)),
                _ => {}
            }
        }
        files.sort_by(|a, b| natural_cmp(&a.0, &b.0));
        folders.sort_by(|a, b| natural_cmp(&a.0, &b.0));
        for (_, path) in files {
            if !self.push(path) {
                return false;
            }
        }
        for (_, path) in folders {
            if !self.folder(&path) {
                return false;
            }
        }
        true
    }
}

fn display_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// The files to list for what was added (파일 추가 · 폴더 추가 · dropped paths): a file as it is,
/// a folder's video files down through its subfolders. Duplicates once; at most `limit`.
pub fn scan(paths: &[PathBuf], limit: usize) -> ScanResult {
    let mut walk = Walk { files: Vec::new(), seen: HashSet::new(), notices: Vec::new(), limit };
    let mut full = false;
    for path in paths {
        if full {
            break;
        }
        if path.is_dir() {
            let (files, notices) = (walk.files.len(), walk.notices.len());
            full = !walk.folder(path);
            // A folder that could not be read says so already.
            if !full && walk.files.len() == files && walk.notices.len() == notices {
                walk.notices.push(Msg::NoVideoFiles { name: display_name(path) });
            }
        } else if path.is_file() {
            full = !walk.push(path.clone());
        } else {
            walk.notices.push(Msg::file_not_found(path.display()));
        }
    }
    if full {
        walk.notices.push(Msg::TooManyFiles { limit });
    }
    ScanResult { files: walk.files.iter().map(|p| p.display().to_string()).collect(), notices: walk.notices }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{AudioChoice, Resolution};
    use crate::probe::AudioStream;

    #[test]
    fn natural_order_counts_numbers() {
        let mut names = vec!["클립 10.mp4", "클립 2.mp4", "클립 1.mp4", "Clip b.mp4", "clip A.mp4", "클립 02.mp4"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["clip A.mp4", "Clip b.mp4", "클립 1.mp4", "클립 02.mp4", "클립 2.mp4", "클립 10.mp4"]);
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn scan_lists_video_files_down_through_subfolders() {
        let root = std::env::temp_dir().join(format!("godseon 훑기 {}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for f in [
            "여행 2.mp4",
            "여행 10.MOV",
            "여행 1.mkv",
            "메모.txt",
            "사진.jpg",
            ".숨김.mp4",
            "여행 1_h265.part.mp4",
            "하위 폴더/클립 A.webm",
            "하위 폴더/더 깊은 곳/클립 B.avi",
            ".godseon-cut-123/piece-000.mp4",
            "빈 폴더/설명.txt",
        ] {
            touch(&root.join(f));
        }
        let names = |r: &ScanResult| -> Vec<String> {
            let rel = |f: &String| {
                let p = Path::new(f).strip_prefix(&root).unwrap().components();
                p.map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
            };
            r.files.iter().map(rel).collect()
        };
        let r = scan(
            &[root.clone(), root.join("여행 2.mp4"), root.join("메모.txt"), root.join("없는 파일.mp4"), root.join("빈 폴더")],
            SCAN_LIMIT,
        );
        assert_eq!(
            names(&r),
            ["여행 1.mkv", "여행 2.mp4", "여행 10.MOV", "하위 폴더/클립 A.webm", "하위 폴더/더 깊은 곳/클립 B.avi", "메모.txt"],
            "own files first by name, then subfolders; a file added on its own is listed whatever it is"
        );
        assert_eq!(r.notices.len(), 2, "{:?}", r.notices);
        assert!(matches!(&r.notices[0], Msg::FileNotFound { path: Some(p) } if p.ends_with("없는 파일.mp4")), "{:?}", r.notices);
        assert_eq!(r.notices[1], Msg::NoVideoFiles { name: "빈 폴더".into() });

        let few = scan(&[root.clone()], 2);
        assert_eq!(names(&few), ["여행 1.mkv", "여행 2.mp4"]);
        assert_eq!(few.notices, [Msg::TooManyFiles { limit: 2 }]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn ff(libopus: bool) -> FfmpegInfo {
        FfmpegInfo {
            ffmpeg_path: "/opt/homebrew/bin/ffmpeg".into(),
            ffprobe_path: "/opt/homebrew/bin/ffprobe".into(),
            version: "9.0.2".into(),
            release: Some("9.0.2".into()),
            libavcodec_major: Some(63),
            meets_minimum: true,
            source: "test".into(),
            video_encoders: vec![],
            features: crate::ffmpeg::Features { libopus, ..Default::default() },
        }
    }

    fn info(path: &str, width: u32, height: u32, audio: &[&str]) -> MediaInfo {
        MediaInfo {
            path: path.into(),
            file_name: Path::new(path).file_name().unwrap().to_string_lossy().into_owned(),
            size_bytes: Some(1_000_000),
            duration: 12.5,
            format_name: "mov,mp4,m4a,3gp,3g2,mj2".into(),
            bit_rate: None,
            video: Some(VideoStream {
                index: 0,
                codec: "h264".into(),
                profile: None,
                width,
                height,
                start_time: Some(0.0),
                has_b_frames: 2,
                fps: Some(30.0),
                avg_fps: Some(30.0),
                pix_fmt: Some("yuv420p".into()),
                color_primaries: None,
                color_transfer: None,
                color_space: None,
                hdr: false,
                rotation: None,
                vfr_suspect: false,
                frame_times: None,
            }),
            audio: audio
                .iter()
                .enumerate()
                .map(|(i, c)| AudioStream { index: i as u32 + 1, codec: c.to_string(), start_time: Some(0.0), channels: Some(2), sample_rate: Some(48000), language: None })
                .collect(),
            subtitle_count: 0,
            other_count: 0,
        }
    }

    fn settings(codec: VideoCodec, container: Container, output_dir: Option<&str>, suffix: &str) -> ConvertSettings {
        ConvertSettings {
            options: ConvertOptions { codec, container, resolution: Resolution::P1080, audio: AudioChoice::CopyOrAac, ten_bit: false },
            encoder: None,
            quality: Quality::Balanced,
            output_dir: output_dir.map(String::from),
            suffix: suffix.into(),
        }
    }

    fn videotoolbox() -> EncoderChoice {
        EncoderChoice {
            name: "hevc_videotoolbox".into(),
            label: "VideoToolbox".into(),
            hardware: true,
            fallback: Some("libx265".into()),
            fallback_label: Some("libx265".into()),
        }
    }

    fn file_name(p: &Path) -> String {
        p.file_name().unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn settings_from_the_ui() {
        let s: ConvertSettings = serde_json::from_str(
            r#"{"codec":"hevc","container":"mov","resolution":"source","audio":"copyOrAac","tenBit":false,
                "encoder":"libx265","quality":"high","outputDir":"/변환 결과","suffix":" 작게"}"#,
        )
        .unwrap();
        assert_eq!((s.options.codec, s.options.container, s.encoder.as_deref()), (VideoCodec::Hevc, Container::Mov, Some("libx265")));
        assert_eq!((s.quality, s.output_dir.as_deref(), s.suffix.as_str()), (Quality::High, Some("/변환 결과"), " 작게"));
        let auto: ConvertSettings = serde_json::from_str(
            r#"{"codec":"av1","container":"webm","resolution":"p720","audio":"aac128","tenBit":true,"encoder":null,"quality":"small","outputDir":null,"suffix":""}"#,
        )
        .unwrap();
        assert_eq!((auto.encoder, auto.output_dir), (None, None));
    }

    #[test]
    fn a_file_becomes_an_item_with_hardware_then_software() {
        let path = "/영상/제주 여행 4K.mp4";
        let media = info(path, 3840, 2160, &["aac"]);
        let s = settings(VideoCodec::Hevc, Container::Mp4, None, "_h265");
        let prepared = prepare(&ff(false), path, &media, &s, &videotoolbox()).unwrap();
        let summary = prepared.task().summary.clone();
        assert_eq!(
            (summary.codec, summary.encoder.as_str(), summary.fallback.as_deref(), summary.scaled, summary.audio, summary.length),
            (VideoCodec::Hevc, "VideoToolbox", Some("libx265"), Some([1920, 1080]), AudioOut::Copy, 12.5)
        );
        let task = prepared.into_task(&|_| false).unwrap();
        assert_eq!((file_name(&task.final_path), file_name(&task.part)), ("제주 여행 4K_h265.mp4".into(), "제주 여행 4K_h265.part.mp4".into()));
        assert_eq!(task.final_path.parent().unwrap(), Path::new("/영상"), "next to the original");
        let plan = command::plan_convert(&media, &s.options).unwrap();
        let video = media.video.as_ref().unwrap();
        let Work::Attempts { attempts, total_secs } = &task.work else { panic!("one FFmpeg run per attempt") };
        assert_eq!(attempts.iter().map(|a| a.label.as_str()).collect::<Vec<_>>(), ["VideoToolbox", "libx265"]);
        for (attempt, encoder) in attempts.iter().zip(["hevc_videotoolbox", "libx265"]) {
            assert_eq!(attempt.args, command::convert_args(Path::new(path), video, &plan, encoder, Quality::Balanced, &task.part).unwrap());
        }
        assert_eq!(*total_secs, 12.5);
        assert_eq!(task.command_line, command::display_command(Path::new("/opt/homebrew/bin/ffmpeg"), &attempts[0].args));
        assert_eq!((task.info.source.as_str(), task.info.source_name.as_str()), (path, "제주 여행 4K.mp4"));
    }

    #[test]
    fn the_size_test_runs_the_same_conversion_over_a_stretch() {
        let path = "/영상/제주 여행 4K.mp4";
        let media = info(path, 3840, 2160, &["aac"]);
        let s = settings(VideoCodec::Hevc, Container::Mp4, None, "_h265");
        let prepared = prepare(&ff(false), path, &media, &s, &videotoolbox()).unwrap();
        let out = Path::new("/캐시/estimates/sample-1.part.mp4");
        let stretch = plan::Range::new(1.25, 11.25);
        let plan = command::plan_convert(&media, &s.options).unwrap();
        let video = media.video.as_ref().unwrap();
        let args = |encoder: &str| command::convert_sample_args(Path::new(path), video, &plan, encoder, Quality::Balanced, stretch, out).unwrap();
        let attempts = prepared.sample_attempts(stretch, out, false).unwrap();
        assert_eq!(attempts.iter().map(|a| (a.label.as_str(), a.args.clone())).collect::<Vec<_>>(), [("VideoToolbox", args("hevc_videotoolbox")), ("libx265", args("libx265"))]);
        let software = prepared.sample_attempts(stretch, out, true).unwrap();
        assert_eq!(software.iter().map(|a| a.label.as_str()).collect::<Vec<_>>(), ["libx265"], "after the hardware encoder failed");
        assert_eq!((prepared.duration(), prepared.container(), prepared.encoder_labels()), (12.5, Container::Mp4, ("VideoToolbox".into(), Some("libx265".into()))));

        // The key follows what changes the result, not the name or the folder.
        let key = |s: &ConvertSettings, m: &MediaInfo| prepare(&ff(true), path, m, s, &videotoolbox()).unwrap().settings_key();
        let named = ConvertSettings { suffix: " 작게".into(), output_dir: Some("/변환 결과".into()), ..s.clone() };
        assert_eq!(key(&s, &media), key(&named, &media));
        assert_ne!(key(&s, &media), key(&ConvertSettings { quality: Quality::High, ..s.clone() }, &media));
        let mut ten = s.clone();
        ten.options.ten_bit = true;
        assert_ne!(key(&s, &media), key(&ten, &media));
        // A 720p file is not made smaller by "1080p로 축소": the same test as "원본 유지".
        let small = info(path, 1280, 720, &["aac"]);
        let mut source = s.clone();
        source.options.resolution = Resolution::Source;
        assert_eq!(key(&s, &small), key(&source, &small));
        assert_ne!(key(&s, &media), key(&source, &media));
    }

    #[test]
    fn another_folder_and_ending_and_the_names_the_queue_holds() {
        let path = "/영상/회의 녹화.mkv";
        let media = info(path, 1280, 720, &["opus"]);
        let s = settings(VideoCodec::Av1, Container::Webm, Some("/변환 결과"), " 작게");
        let svt = EncoderChoice { name: "libsvtav1".into(), label: "SVT-AV1".into(), hardware: false, fallback: None, fallback_label: None };
        let held = [PathBuf::from("/변환 결과/회의 녹화 작게.webm")];
        let task = prepare(&ff(false), path, &media, &s, &svt).unwrap().into_task(&|p| held.iter().any(|h| h == p)).unwrap();
        assert_eq!(task.final_path, Path::new("/변환 결과/회의 녹화 작게 (1).webm"));
        assert_eq!(task.part, Path::new("/변환 결과/회의 녹화 작게 (1).part.webm"));
        let Work::Attempts { attempts, .. } = &task.work else { panic!() };
        assert_eq!(attempts.len(), 1, "software alone");
        assert_eq!(task.info.summary.audio, AudioOut::Copy, "Opus stays as it is in WebM");
        assert_eq!(task.info.summary.scaled, None);

        // AAC into WebM is Opus, which this FFmpeg cannot write.
        let aac = info("/영상/a.mp4", 1280, 720, &["aac"]);
        let e = prepare(&ff(false), "/영상/a.mp4", &aac, &s, &svt).err().unwrap();
        assert_eq!(e, Msg::WebmNeedsOpus);
        assert!(prepare(&ff(true), "/영상/a.mp4", &aac, &s, &svt).is_ok());
    }

    #[test]
    fn settings_checked_once_for_every_file() {
        assert!(check_settings(&settings(VideoCodec::Hevc, Container::Mp4, None, "_h265")).is_ok());
        assert!(check_settings(&settings(VideoCodec::Hevc, Container::Mp4, None, "a/b")).is_err());
        let e = check_settings(&settings(VideoCodec::Hevc, Container::Mp4, Some("/없는 폴더/결과"), "_h265")).unwrap_err();
        assert_eq!(e, Msg::OutputFolderNotFound { path: "/없는 폴더/결과".into() });
    }
}
