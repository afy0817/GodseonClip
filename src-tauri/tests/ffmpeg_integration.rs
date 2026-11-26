//! End-to-end checks against a real FFmpeg 6.0+ install. Ignored by default:
//!
//!     npm run test:ffmpeg            # or: cargo test -- --ignored
//!     GODSEON_FFMPEG=/path/to/ffmpeg npm run test:ffmpeg
//!
//! Every clip is generated with lavfi, so no sample files are needed.

use godseon_clip_lib::jobs::{Attempt, JobEvent, JobFinished, JobManager};
use godseon_clip_lib::convert::{self, ConvertRequest, ConvertSettings, ConvertTask};
use godseon_clip_lib::export::{self, Edit, ExportMode, ExportRequest, ExportTask};
use godseon_clip_lib::queue::{self, ItemState, Queue, QueueEvent, QueueSnapshot};
use godseon_clip_lib::msg::Msg;
use godseon_clip_lib::{command, effects, encoders, estimate, ffmpeg, jobs, plan, preview, probe, util};
use std::sync::Arc;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// The source clip has a 1 ms click at exactly this time (and the frame shown at this time is known).
const CLICK_AT: f64 = 3.0;

fn ffmpeg_or_panic() -> ffmpeg::FfmpegInfo {
    let custom = std::env::var("GODSEON_FFMPEG").ok();
    let r = ffmpeg::locate(custom.as_deref());
    match r.info {
        Some(info) if info.meets_minimum => info,
        _ => panic!("FFmpeg 6.0+ not found. problem={:?} searched={:?}", r.problem, r.searched),
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("godseon-it-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn os_args(parts: &[&str]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

fn run_ok(ff: &ffmpeg::FfmpegInfo, args: &[OsString]) -> util::Captured {
    let out = util::run_capture(Path::new(&ff.ffmpeg_path), args, Duration::from_secs(120)).unwrap();
    assert!(out.success, "ffmpeg failed: {}", out.stderr);
    out
}

/// 10 s test pattern, a keyframe every second, AAC audio that is silent except a click at CLICK_AT.
fn make_clip(ff: &ffmpeg::FfmpegInfo, path: &Path) {
    let vcodec = if ff.features.libx264 { "libx264" } else { "mpeg4" };
    let mut args = os_args(&[
        "-hide_banner", "-nostdin", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=10",
        "-f", "lavfi", "-i", r"aevalsrc=exprs='if(between(t\,3.0\,3.001)\,0.9\,0)':s=44100:d=10",
        "-c:v", vcodec, "-g", "25", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest",
    ]);
    args.push(path.as_os_str().to_owned());
    run_ok(ff, &args);
}

/// (presentation time in seconds, md5) of every video frame, as a player would show them.
fn video_frames(ff: &ffmpeg::FfmpegInfo, file: &Path, seek: Option<f64>) -> Vec<(f64, String)> {
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error"]);
    if let Some(t) = seek {
        args.extend(os_args(&["-ss", &format!("{t:.3}")]));
    }
    args.extend(os_args(&["-i"]));
    args.push(file.as_os_str().to_owned());
    args.extend(os_args(&["-map", "0:v:0", "-f", "framemd5", "-"]));
    let text = run_ok(ff, &args).stdout;
    let (mut num, mut den) = (1f64, 1f64);
    let mut frames = Vec::new();
    for line in text.lines() {
        if let Some(tb) = line.strip_prefix("#tb 0:") {
            let (n, d) = tb.trim().split_once('/').unwrap();
            num = n.parse().unwrap();
            den = d.parse().unwrap();
        } else if !line.starts_with('#') {
            let cols: Vec<&str> = line.split(',').map(str::trim).collect();
            if cols.len() >= 6 {
                frames.push((cols[2].parse::<f64>().unwrap() * num / den, cols[5].to_string()));
            }
        }
    }
    frames
}

/// When the output shows the frame that the source shows at `source_t`.
fn time_of_source_frame(ff: &ffmpeg::FfmpegInfo, source: &Path, out: &Path, source_t: f64) -> f64 {
    let wanted = video_frames(ff, source, Some(source_t)).first().expect("source frame").1.clone();
    video_frames(ff, out, None)
        .into_iter()
        .find(|(_, md5)| *md5 == wanted)
        .map(|(t, _)| t)
        .expect("source frame not found in output")
}

/// Time of the loudest audio sample (the click) on the output timeline.
fn click_time(ff: &ffmpeg::FfmpegInfo, out: &Path) -> f64 {
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-i"]);
    args.push(out.as_os_str().to_owned());
    args.extend(os_args(&["-map", "0:a:0", "-ac", "1", "-ar", "44100", "-f", "s16le", "-"]));
    let raw = run_ok(ff, &args).stdout_raw;
    let (idx, _) = raw
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs())
        .enumerate()
        .max_by_key(|(_, v)| *v)
        .expect("audio samples");
    let info = probe::probe(Path::new(&ff.ffprobe_path), out).unwrap();
    idx as f64 / 44100.0 + info.audio[0].start_time.unwrap_or(0.0)
}

fn run_job(ff: &ffmpeg::FfmpegInfo, args: Vec<OsString>, total: f64, part: &Path, fin: &Path) -> (JobFinished, usize) {
    let jobs = JobManager::default();
    let (tx, rx) = mpsc::channel();
    jobs.start(Path::new(&ff.ffmpeg_path), args, total, part.to_path_buf(), fin.to_path_buf(), move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    let mut progress_events = 0;
    loop {
        match rx.recv_timeout(Duration::from_secs(90)).expect("no job event within 90 s") {
            JobEvent::Progress(_) => progress_events += 1,
            JobEvent::Finished(f) => return (f, progress_events),
        }
    }
}

/// Lossless trim through the app's own code path (output_paths + lossless_trim_args + JobManager),
/// with a Korean file name, in MP4 and MKV, at a keyframe and between keyframes.
/// Checks A/V sync with the click and where playback really starts.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn lossless_trim_keeps_sync_and_lands_where_promised() {
    let ff = ffmpeg_or_panic();
    let ffprobe = PathBuf::from(&ff.ffprobe_path);
    let dir = temp_dir("trim");
    let mp4 = dir.join("테스트 클립 01.mp4");
    make_clip(&ff, &mp4);
    let mkv = dir.join("테스트 클립 01.mkv");
    let mut remux = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"]);
    remux.extend([mp4.as_os_str().to_owned(), "-c".into(), "copy".into(), mkv.as_os_str().to_owned()]);
    run_ok(&ff, &remux);

    for source in [&mp4, &mkv] {
        let info = probe::probe(&ffprobe, source).unwrap();
        assert!((info.duration - 10.0).abs() < 0.2, "duration {}", info.duration);
        let video = info.video.clone().expect("video stream");
        let kf = probe::keyframes(&ffprobe, source, video.index).unwrap();
        assert!(kf.len() >= 9, "expected a keyframe every second, got {kf:?}");
        let is_mp4 = source.extension().unwrap() == "mp4";

        for start in [2.0, 2.4] {
            let label = format!("{} from {start}", source.file_name().unwrap().to_string_lossy());
            let tp = command::plan_lossless_trim(&info, &kf, start, start + 3.0).unwrap();
            let (final_path, part) = command::output_paths(source, "_trim");
            let args = command::lossless_trim_args(source, Some(video.index), tp.seek, tp.duration, &part);
            let (finished, progress_events) = run_job(&ff, args, tp.duration, &part, &final_path);

            assert!(finished.ok, "{label}: job failed: {:?}", finished.error);
            assert!(progress_events > 0, "{label}: no -progress output was parsed");
            assert!(!part.exists(), "{label}: .part file must be renamed");
            let out = probe::probe(&ffprobe, &final_path).unwrap();
            assert_eq!(out.audio.len(), 1, "{label}: audio must be kept");

            let v = time_of_source_frame(&ff, source, &final_path, CLICK_AT);
            let a = click_time(&ff, &final_path);
            assert!((a - v).abs() < 0.015, "{label}: A/V offset {:.1} ms", (a - v) * 1000.0);

            let keyframe = plan::keyframe_at_or_before(&kf, start).unwrap();
            assert_eq!(tp.exact_start, is_mp4);
            assert_eq!(tp.visible_start, if is_mp4 { start } else { keyframe });
            if is_mp4 {
                // Edit lists: playback starts exactly at the requested time.
                assert!((v - (CLICK_AT - start)).abs() < 0.002, "{label}: MP4 should start at {start}, frame at {v}");
                assert!(out.duration < 3.3, "{label}: duration {}", out.duration);
            } else {
                // No edit lists: playback starts at the keyframe (plus a short audio lead-in).
                let expected = CLICK_AT - keyframe;
                assert!(v >= expected - 0.002 && v <= expected + 0.12, "{label}: frame at {v}, expected ≈{expected}");
            }
            std::fs::remove_file(&final_path).unwrap();
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Cut points on 30 fps frame starts that are not whole milliseconds (frame 62 = 2.0666… s,
/// frame 155 = 5.1666… s, frame 154 = 5.1333… s), as the editor sends them. MP4 must start on frame 62 exactly (the copy
/// once started a frame late because `-ss` was rounded up to 2.067); MKV starts at the keyframe.
/// Without B-frames the copy stops right before OUT. With B-frames stream copy cuts the end
/// by decode time, so up to `has_b_frames + 1` later frames trail OUT − 1, not always contiguous
/// (a known limit of lossless cuts).
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn lossless_trim_shows_exactly_the_frames_between_in_and_out() {
    let ff = ffmpeg_or_panic();
    let ffprobe = PathBuf::from(&ff.ffprobe_path);
    let dir = temp_dir("frames");
    let vcodec = if ff.features.libx264 { "libx264" } else { "mpeg4" };
    let first = 62usize;

    for bf in ["0", "2"] {
        let mp4 = dir.join(format!("프레임 확인 30fps B{bf}.mp4"));
        let mut make = os_args(&[
            "-hide_banner", "-nostdin", "-loglevel", "error", "-y",
            "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30:duration=6",
            "-c:v", vcodec, "-g", "30", "-bf", bf, "-pix_fmt", "yuv420p",
        ]);
        make.push(mp4.as_os_str().to_owned());
        run_ok(&ff, &make);
        let mkv = mp4.with_extension("mkv");
        let mut remux = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"]);
        remux.extend([mp4.as_os_str().to_owned(), "-c".into(), "copy".into(), mkv.as_os_str().to_owned()]);
        run_ok(&ff, &remux);

        // OUT #155 (5.1666…) is stored as 5.167 in MKV, #154 (5.1333…) as 5.133: rounded down, it
        // sat before the cut and came in until the copy ended CUT_END_MARGIN early.
        for (source, stop) in [&mp4, &mkv].into_iter().flat_map(|s| [(s, 155usize), (s, 154)]) {
            let label = format!("{} OUT #{stop}", source.file_name().unwrap().to_string_lossy());
            let shown = video_frames(&ff, source, None);
            assert_eq!(shown.len(), 180, "{label}: testsrc2 frames");
            let info = probe::probe(&ffprobe, source).unwrap();
            let video = info.video.clone().expect("video stream");
            let kf = probe::keyframes(&ffprobe, source, video.index).unwrap();
            let (start, end) = (first as f64 / 30.0, stop as f64 / 30.0);
            let tp = command::plan_lossless_trim(&info, &kf, start, end).unwrap();
            let (final_path, part) = command::output_paths(source, "_trim");
            let args = command::lossless_trim_args(source, Some(video.index), tp.seek, tp.duration, &part);
            let (finished, _) = run_job(&ff, args, tp.duration, &part, &final_path);
            assert!(finished.ok, "{label}: job failed: {:?}", finished.error);

            let out: Vec<String> = video_frames(&ff, &final_path, None).into_iter().map(|(_, md5)| md5).collect();
            let from = if tp.exact_start {
                first
            } else {
                // No edit lists: the frames from the keyframe at or before IN.
                let k = plan::keyframe_at_or_before(&kf, start).unwrap();
                shown.iter().position(|(t, _)| (t - k).abs() < 0.001).expect("keyframe among the shown frames")
            };
            let index_of = |md5: &String| shown.iter().position(|(_, m)| m == md5);
            assert_eq!(out.first().and_then(index_of), Some(from), "{label}: first frame");
            // Every frame up to OUT − 1, exactly and in order.
            let kept: Vec<String> = shown[from..stop].iter().map(|(_, md5)| md5.clone()).collect();
            assert!(out.len() >= kept.len() && out[..kept.len()] == kept[..], "{label}: frames {from}..{stop} are not all there in order");
            // Stream copy cuts by decode time, so with B-frames the P frame that the B frames
            // before it lean on decodes first and comes along, and a B frame decoded after the cut
            // can be missing: up to has_b_frames + 1 later source frames trail OUT − 1, in order
            // but not always contiguous (x264 -bf 2: OUT #155 → … 154 155 156, OUT #154 →
            // … 153 154 156 — the same with or without CUT_END_MARGIN). Frame-exact ends are 정밀's.
            let extra = if video.has_b_frames > 0 { video.has_b_frames as usize + 1 } else { 0 };
            let tail: Vec<usize> = out[kept.len()..].iter().map(|m| index_of(m).expect("trailing frame is a source frame")).collect();
            assert!(
                tail.windows(2).all(|w| w[0] < w[1]) && tail.iter().all(|&i| i >= stop && i <= stop - 1 + extra),
                "{label}: frames after OUT − 1: {tail:?} (B-frame delay {extra})"
            );
            std::fs::remove_file(&final_path).unwrap();
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn cancel_stops_job_and_removes_part_file() {
    let ff = ffmpeg_or_panic();
    let dir = temp_dir("cancel");
    let final_path = dir.join("long.mkv");
    let part = dir.join("long.part.mkv");
    // -re reads the 60 s source in real time, so the job is still running when we cancel.
    let mut args = os_args(&[
        "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
        "-re", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=60", "-c:v", "mpeg4",
    ]);
    args.push(part.as_os_str().to_owned());

    let jobs = JobManager::default();
    let (tx, rx) = mpsc::channel();
    let id = jobs
        .start(Path::new(&ff.ffmpeg_path), args, 60.0, part.clone(), final_path.clone(), move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(jobs.cancel(id));
    let finished = loop {
        if let JobEvent::Finished(f) = rx.recv_timeout(Duration::from_secs(30)).expect("job event") {
            break f;
        }
    };

    assert!(finished.canceled && !finished.ok);
    assert!(!part.exists() && !final_path.exists(), "cancel must leave no output behind");
    assert!(finished.elapsed_ms < 10_000);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn software_encoders_pass_the_three_frame_test() {
    let ff = ffmpeg_or_panic();
    let report = encoders::run(&ff);
    for (name, present) in [("libx265", ff.features.libx265), ("libsvtav1", ff.features.libsvtav1)] {
        if present {
            let t = report.tests.iter().find(|t| t.name == name).expect("tested");
            assert!(t.ok, "{name} failed: {:?}", t.error);
        }
    }
    if ff.features.libx265 {
        assert!(report.preferred_hevc.is_some());
    }
}

/// 1280×720 MKV whose brightness rises steadily (so a frame's time can be read back from its
/// brightness), B-frames, a keyframe about every second, and AAC audio that starts 1 s late with a
/// click at CLICK_AT on the file's timeline.
fn make_ramp_clip(ff: &ffmpeg::FfmpegInfo, path: &Path) {
    let mut args = os_args(&[
        "-hide_banner", "-nostdin", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", "nullsrc=s=64x36:r=25:d=10,format=yuv420p,geq=lum='40+16*T':cb=128:cr=128,scale=1280:720",
        "-itsoffset", "1",
        "-f", "lavfi", "-i", r"aevalsrc=exprs='if(between(t\,2.0\,2.001)\,0.9\,0)':s=44100:d=9",
        "-map", "0:v", "-map", "1:a", "-c:v", "libx264", "-g", "25", "-pix_fmt", "yuv420p", "-c:a", "aac",
    ]);
    args.push(path.as_os_str().to_owned());
    run_ok(ff, &args);
}

/// Decodes the first frame at or after `seek` (or of an image / data: URL) to RGB, as a viewer sees it.
/// (Not `-pix_fmt gray`: with FFmpeg 9.0.2 it expands limited-range video twice, so H.264 and JPEG
/// pictures of the same brightness come out different.)
fn rgb_frame(ff: &ffmpeg::FfmpegInfo, input: &OsStr, seek: Option<f64>, one_pixel: bool) -> Vec<u8> {
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error"]);
    if let Some(t) = seek {
        args.extend(os_args(&["-ss", &format!("{t:.3}")]));
    }
    args.push("-i".into());
    args.push(input.to_owned());
    args.extend(os_args(&["-map", "0:v:0", "-frames:v", "1"]));
    if one_pixel {
        args.extend(os_args(&["-vf", "scale=1:1:flags=area"]));
    }
    args.extend(os_args(&["-f", "rawvideo", "-pix_fmt", "rgb24", "-"]));
    run_ok(ff, &args).stdout_raw
}

/// Average brightness (red channel of the gray picture) of the frame shown at `t`.
fn brightness_at(ff: &ffmpeg::FfmpegInfo, file: &Path, t: f64) -> i32 {
    i32::from(rgb_frame(ff, file.as_os_str(), Some(t), true)[0])
}

/// The proxy runs through the app's code path (proxy_paths + proxy_args + JobManager) and keeps the
/// original's timeline: same frame times, same pictures at the same times, audio still in sync.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn proxy_is_playable_h264_on_the_source_timeline() {
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx264, "the proxy needs libx264");
    let ffprobe = PathBuf::from(&ff.ffprobe_path);
    let dir = temp_dir("proxy");
    let source = dir.join("테스트 클립 01.mkv");
    make_ramp_clip(&ff, &source);
    let cache = dir.join("캐시 폴더");
    std::fs::create_dir_all(&cache).unwrap();

    let info = probe::probe(&ffprobe, &source).unwrap();
    let video = info.video.clone().unwrap();
    let (final_path, part) = preview::proxy_paths(&cache, &source).unwrap();
    let args = command::proxy_args(&source, &video, &part);
    let (finished, progress_events) = run_job(&ff, args, info.duration, &part, &final_path);
    assert!(finished.ok, "proxy job failed: {:?}", finished.error);
    assert!(progress_events > 0, "no -progress output was parsed");
    assert!(!part.exists() && final_path.is_file());
    assert_eq!(preview::proxy_paths(&cache, &source).unwrap().0, final_path, "reopening finds the same proxy");

    let out = probe::probe(&ffprobe, &final_path).unwrap();
    assert!(command::is_mov_family(&out.format_name), "{}", out.format_name);
    let v = out.video.clone().unwrap();
    assert_eq!((v.codec.as_str(), v.width, v.height, v.pix_fmt.as_deref()), ("h264", 960, 540, Some("yuv420p")));
    assert_eq!(out.audio.len(), 1);
    assert_eq!(out.audio[0].codec, "aac");
    assert!((out.duration - info.duration).abs() < 0.1, "duration {} vs {}", out.duration, info.duration);

    // Same frame times as the original…
    let src_times: Vec<f64> = video_frames(&ff, &source, None).into_iter().map(|(t, _)| t).collect();
    let proxy_times: Vec<f64> = video_frames(&ff, &final_path, None).into_iter().map(|(t, _)| t).collect();
    assert_eq!(src_times.len(), proxy_times.len(), "no frames duplicated or dropped");
    for (a, b) in src_times.iter().zip(&proxy_times) {
        assert!((a - b).abs() < 0.001, "frame at {a} in the original is at {b} in the proxy");
    }
    // …showing the same picture at the same time (one frame ≈ 0.9 brightness steps)…
    for t in [1.5, 3.0, 5.24, 8.04] {
        let (a, b) = (brightness_at(&ff, &source, t), brightness_at(&ff, &final_path, t));
        assert!((a - b).abs() <= 3, "at {t}: original {a}, proxy {b}");
    }
    // …and the late-starting audio still lines up.
    let click = click_time(&ff, &final_path);
    assert!((click - CLICK_AT).abs() < 0.015, "click at {click}, expected {CLICK_AT}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Thumbnails show the last keyframe at or before each tile's middle, and the waveform peak lands
/// at the click on the original's timeline (late-starting audio padded), in MP4 and MKV.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn thumbnails_and_waveform_follow_the_source_timeline() {
    let ff = ffmpeg_or_panic();
    let ffmpeg_path = PathBuf::from(&ff.ffmpeg_path);
    let ffprobe = PathBuf::from(&ff.ffprobe_path);
    let dir = temp_dir("preview");
    let mkv = dir.join("테스트 클립 01.mkv");
    make_ramp_clip(&ff, &mkv);
    let mp4 = dir.join("테스트 클립 01.mp4");
    let mut remux = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"]);
    remux.extend([mkv.as_os_str().to_owned(), "-c".into(), "copy".into(), mp4.as_os_str().to_owned()]);
    run_ok(&ff, &remux);

    for source in [&mkv, &mp4] {
        let name = source.file_name().unwrap().to_string_lossy().into_owned();
        let info = probe::probe(&ffprobe, source).unwrap();
        let kf = probe::keyframes(&ffprobe, source, info.video.as_ref().unwrap().index).unwrap();
        assert!(kf.len() >= 9, "{name}: keyframes {kf:?}");
        let kf_brightness: Vec<i32> = kf.iter().map(|&k| brightness_at(&ff, source, k)).collect();

        // Whole file; then a range whose tile middles fall just after keyframes, where MKV's
        // B-frame seek margin (3/23 s) would show the previous keyframe if the args were wrong.
        // 20 tiles: the last tile's middle lies past the last keyframe (the tail needs tpad).
        let just_after = kf[2] + 0.05 - 0.5;
        let cases = [
            (None, None, 8u32),
            (None, None, 20),
            (Some(2.0), Some(6.0), 4),
            (Some(just_after), Some(just_after + 6.0), 6),
        ];
        for (start, end, count) in cases {
            let label = format!("{name} {start:?}..{end:?}");
            let strip = preview::thumbnails(&ffmpeg_path, source, &info, start, end, count, 90).unwrap();
            assert_eq!(strip.images.len(), count as usize, "{label}");
            let span = end.unwrap_or(info.duration) - start.unwrap_or(0.0);
            assert!((strip.interval - span / f64::from(count)).abs() < 1e-5, "{label}: interval {}", strip.interval);
            for (k, url) in strip.images.iter().enumerate() {
                assert!(url.starts_with("data:image/jpeg;base64,/9j/"), "{label}: not a JPEG data URL");
                let pixels = rgb_frame(&ff, OsStr::new(url), None, false);
                assert_eq!(pixels.len(), 160 * 90 * 3, "{label}: thumbnail size");
                let shown = i32::from(rgb_frame(&ff, OsStr::new(url), None, true)[0]);
                let middle = strip.start + (k as f64 + 0.5) * strip.interval;
                let expected = plan::keyframe_at_or_before(&kf, middle).unwrap();
                let closest = kf
                    .iter()
                    .zip(&kf_brightness)
                    .min_by_key(|(_, b)| (*b - shown).abs())
                    .map(|(k, _)| *k)
                    .unwrap();
                assert_eq!(closest, expected, "{label}: tile {k} (middle {middle:.2}) shows keyframe {closest}, expected {expected}");
            }
        }

        // Whole file, then a range. Bucket counts keep the 1 ms click away from bucket edges.
        for (start, end, n) in [(None, None, 64usize), (Some(2.55), Some(4.55), 20)] {
            let label = format!("{name} {start:?}..{end:?}");
            let w = preview::waveform(&ffmpeg_path, source, &info, start, end, n as u32).unwrap();
            assert_eq!((w.start, w.end), (start.unwrap_or(0.0), end.unwrap_or(info.duration)), "{label}");
            assert_eq!(w.peaks.len(), n, "{label}");
            let bucket_of = |t: f64| ((t - w.start) / (w.end - w.start) * n as f64) as usize;
            let click = bucket_of(CLICK_AT + 0.0005);
            let loudest = (0..n).max_by(|&a, &b| w.peaks[a].total_cmp(&w.peaks[b])).unwrap();
            assert_eq!(loudest, click, "{label}: the click at {CLICK_AT} s, peaks {:?}", w.peaks);
            assert!(w.peaks[click] > 0.5, "{label}: click peak {}", w.peaks[click]);
            let quiet = w.peaks.iter().enumerate().filter(|(i, _)| i.abs_diff(click) > 1).all(|(_, &p)| p < 0.05);
            assert!(quiet, "{label}: only the click is loud: {:?}", w.peaks);
            if start.is_none() {
                // The audio track starts at 1 s; padding keeps the click where it is on the timeline.
                assert!(w.peaks[..bucket_of(0.95)].iter().all(|&p| p < 0.01), "{label}: {:?}", w.peaks);
            }
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

// --- precise export (re-encode) ---

const SMALL: (usize, usize) = (160, 120);

/// Every video frame decoded to small RGB pictures, in display order.
fn small_frames(ff: &ffmpeg::FfmpegInfo, file: &Path) -> Vec<Vec<u8>> {
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-i"]);
    args.push(file.as_os_str().to_owned());
    let scale = format!("scale={}:{}:flags=area", SMALL.0, SMALL.1);
    args.extend(os_args(&[
        "-map", "0:v:0", "-vf", &scale, "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "rgb24", "-",
    ]));
    run_ok(ff, &args).stdout_raw.chunks_exact(SMALL.0 * SMALL.1 * 3).map(<[u8]>::to_vec).collect()
}

/// The source frame a re-encoded frame looks most like (sum of absolute differences).
fn nearest(source: &[Vec<u8>], frame: &[u8]) -> usize {
    let sad = |a: &[u8]| a.iter().zip(frame).map(|(x, y)| u64::from(x.abs_diff(*y))).sum::<u64>();
    (0..source.len()).min_by_key(|&i| sad(&source[i])).unwrap()
}

/// "codec_name=hevc codec_tag_string=hvc1 color_primaries=… color_space=… color_transfer=… pix_fmt=…"
/// of the first video stream, keys sorted (ffprobe prints them in its own order).
fn video_stream_line(ff: &ffmpeg::FfmpegInfo, file: &Path) -> String {
    let mut args = os_args(&[
        "-v", "error", "-select_streams", "v:0", "-show_entries",
        "stream=codec_name,codec_tag_string,pix_fmt,color_primaries,color_transfer,color_space", "-of", "default=nw=1",
    ]);
    args.push(file.as_os_str().to_owned());
    let out = util::run_capture(Path::new(&ff.ffprobe_path), &args, Duration::from_secs(30)).unwrap();
    assert!(out.success, "ffprobe failed: {}", out.stderr);
    let mut fields: Vec<&str> = out.stdout.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    fields.sort_unstable();
    fields.join(" ")
}

fn run_attempts(ff: &ffmpeg::FfmpegInfo, attempts: Vec<Attempt>, total: f64, part: &Path, fin: &Path) -> JobFinished {
    let jobs = JobManager::default();
    let (tx, rx) = mpsc::channel();
    jobs.start_attempts(Path::new(&ff.ffmpeg_path), attempts, total, part.to_path_buf(), fin.to_path_buf(), move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    loop {
        if let JobEvent::Finished(f) = rx.recv_timeout(Duration::from_secs(180)).expect("no job event within 180 s") {
            return f;
        }
    }
}

/// One precise export through the app's code path (plan → output_paths_as → args → JobManager).
fn precise_export(
    ff: &ffmpeg::FfmpegInfo,
    source: &Path,
    start: f64,
    end: f64,
    output: command::OutputCodec,
    encoder: &str,
) -> (PathBuf, command::PreciseTrimPlan) {
    let info = probe::probe(Path::new(&ff.ffprobe_path), source).unwrap();
    let video = info.video.clone().expect("video stream");
    let plan = command::plan_precise_trim(&info, start, end, output).unwrap();
    let (final_path, part) = command::output_paths_as(source, &plan.suffix, Some(&plan.extension));
    let args = command::precise_trim_args(source, &video, &plan, encoder, command::Quality::High, &part).unwrap();
    let finished = run_attempts(ff, vec![Attempt { args, label: encoder.into() }], plan.duration, &part, &final_path);
    assert!(finished.ok, "{encoder} on {}: {:?}", source.display(), finished.error);
    assert!(!part.exists());
    (final_path, plan)
}

/// Every encoder that passes the three-frame test here and that the export has settings for.
fn working_export_encoders(ff: &ffmpeg::FfmpegInfo) -> Vec<(String, String)> {
    encoders::run(ff)
        .tests
        .into_iter()
        .filter(|t| t.ok && command::exports_with(&t.name))
        .map(|t| (t.name, t.codec))
        .collect()
}

fn output_for(codec: &str) -> command::OutputCodec {
    match codec {
        "hevc" => command::OutputCodec::Hevc,
        "av1" => command::OutputCodec::Av1,
        _ => command::OutputCodec::Source,
    }
}

/// The frame-exact promise of 정밀: frames IN … OUT−1 of the source and nothing more — also with
/// B-frames, where stream copy keeps frames past OUT. Every encoder that works here, MP4 and MKV.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn precise_trim_shows_exactly_the_frames_between_in_and_out() {
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx264, "this test needs libx264 for its source clips");
    let dir = temp_dir("precise");
    let (first, stop) = (62usize, 155usize);
    let encoders = working_export_encoders(&ff);
    assert!(encoders.iter().any(|(n, _)| n == "libx264"), "working encoders: {encoders:?}");

    for bf in ["0", "2"] {
        let mp4 = dir.join(format!("정밀 확인 30fps B{bf}.mp4"));
        let mut make = os_args(&[
            "-hide_banner", "-nostdin", "-loglevel", "error", "-y",
            "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30:duration=6",
            "-c:v", "libx264", "-g", "30", "-bf", bf, "-pix_fmt", "yuv420p",
        ]);
        make.push(mp4.as_os_str().to_owned());
        run_ok(&ff, &make);
        let mkv = mp4.with_extension("mkv");
        let mut remux = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"]);
        remux.extend([mp4.as_os_str().to_owned(), "-c".into(), "copy".into(), mkv.as_os_str().to_owned()]);
        run_ok(&ff, &remux);

        for source in [&mp4, &mkv] {
            let shown = small_frames(&ff, source);
            assert_eq!(shown.len(), 180);
            for (encoder, codec) in &encoders {
                let label = format!("{} → {encoder}", source.file_name().unwrap().to_string_lossy());
                let (out, plan) =
                    precise_export(&ff, source, first as f64 / 30.0, stop as f64 / 30.0, output_for(codec), encoder);
                let matched: Vec<usize> = small_frames(&ff, &out).iter().map(|f| nearest(&shown, f)).collect();
                let expected: Vec<usize> = (first..stop).collect();
                assert_eq!(matched, expected, "{label}: output frames are not source frames {first}..{stop}");

                let line = video_stream_line(&ff, &out);
                let codec_fields = match codec.as_str() {
                    "hevc" if plan.extension == "mp4" => "codec_name=hevc codec_tag_string=hvc1",
                    "hevc" => "codec_name=hevc ",
                    "av1" => "codec_name=av1 ",
                    _ => "codec_name=h264 ",
                };
                assert!(line.starts_with(codec_fields), "{label}: {line}");
                let length = probe::probe(Path::new(&ff.ffprobe_path), &out).unwrap().duration;
                let want = (stop - first) as f64 / 30.0;
                assert!((length - want).abs() <= 1.0 / 30.0 + 1e-3, "{label}: length {length}, want {want}");
                std::fs::remove_file(&out).unwrap();
            }
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A cut re-encodes the audio and keeps it in sync with the picture; the whole file copies AAC.
/// The first attempt names an encoder that does not exist, so the job falls back the way it does
/// when a hardware encoder fails, under the same job.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn precise_export_keeps_sync_and_falls_back() {
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx265, "this test needs libx265");
    let dir = temp_dir("precise-sync");
    let source = dir.join("소리 확인 클립.mp4");
    make_clip(&ff, &source);
    let ffprobe = PathBuf::from(&ff.ffprobe_path);
    let info = probe::probe(&ffprobe, &source).unwrap();
    let video = info.video.clone().unwrap();

    // Cut 2.0 – 6.0 (25 fps frame starts): the click at 3.0 lands at 1.0.
    let plan = command::plan_precise_trim(&info, 2.0, 6.0, command::OutputCodec::Hevc).unwrap();
    assert_eq!(plan.audio, command::AudioPlan::Aac);
    let (final_path, part) = command::output_paths_as(&source, &plan.suffix, Some(&plan.extension));
    let good = command::precise_trim_args(&source, &video, &plan, "libx265", command::Quality::Balanced, &part).unwrap();
    let broken: Vec<OsString> =
        good.iter().map(|a| if a == "libx265" { OsString::from("hevc_does_not_exist") } else { a.clone() }).collect();
    let attempts =
        vec![Attempt { args: broken, label: "가짜 인코더".into() }, Attempt { args: good, label: "libx265".into() }];
    let finished = run_attempts(&ff, attempts, plan.duration, &part, &final_path);
    assert!(finished.ok, "{:?}", finished.error);
    let note = finished.note.expect("retry note");
    assert!(matches!(&note, Msg::RetriedWith { failed, next, .. } if failed == "가짜 인코더" && next == "libx265"), "{note}");
    assert_eq!(final_path.file_name().unwrap(), "소리 확인 클립_trim_h265.mp4");

    let out = probe::probe(&ffprobe, &final_path).unwrap();
    assert_eq!(out.audio.first().map(|a| a.codec.as_str()), Some("aac"));
    let click = click_time(&ff, &final_path);
    assert!((click - 1.0).abs() < 0.015, "click at {click:.4} s, expected 1.0");
    let shown = small_frames(&ff, &source);
    let frames = small_frames(&ff, &final_path);
    assert_eq!(frames.len(), 100, "4 s at 25 fps");
    assert_eq!((nearest(&shown, &frames[0]), nearest(&shown, &frames[99])), (50, 149));

    // Whole file to H.265: no -ss / -t, AAC copied, the click still at 3.0.
    let (whole, plan) = precise_export(&ff, &source, 0.0, info.duration, command::OutputCodec::Hevc, "libx265");
    assert_eq!((plan.audio, plan.seek), (command::AudioPlan::Copy, None));
    assert_eq!(whole.file_name().unwrap(), "소리 확인 클립_h265.mp4");
    let click = click_time(&ff, &whole);
    assert!((click - CLICK_AT).abs() < 0.015, "whole file: click at {click:.4} s");
    assert_eq!(small_frames(&ff, &whole).len(), 250);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// 10-bit HDR (PQ) sources: H.265 and AV1 stay 10-bit and keep the color description, with every
/// encoder that works here (on a Mac also VideoToolbox's Main 10 path). One source describes its
/// colors in the bitstream; the other only in the MKV container, so the decoded frames carry none
/// and only the app's `setparams` can pass them on.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn precise_export_keeps_ten_bit_and_hdr_tags() {
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx265, "this test needs libx265 for its source clips");
    let dir = temp_dir("precise-hdr");
    let tagged = dir.join("HDR 테스트 클립.mkv");
    let mut make = os_args(&[
        "-hide_banner", "-nostdin", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30:duration=2",
        // Output -color_* options do not tag frames lavfi left unspecified; setparams does.
        "-vf", "setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc",
        "-c:v", "libx265", "-x265-params", "log-level=error", "-pix_fmt", "yuv420p10le",
    ]);
    make.push(tagged.as_os_str().to_owned());
    run_ok(&ff, &make);
    let untagged_stream = dir.join("HDR 색 정보 없는 원본.mkv");
    let mut make = os_args(&[
        "-hide_banner", "-nostdin", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30:duration=2",
        "-c:v", "libx265", "-x265-params", "log-level=error", "-pix_fmt", "yuv420p10le",
    ]);
    make.push(untagged_stream.as_os_str().to_owned());
    run_ok(&ff, &make);
    let container_only = dir.join("HDR 컨테이너만 표시.mkv");
    let mut remux = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"]);
    remux.push(untagged_stream.as_os_str().to_owned());
    remux.extend(os_args(&["-c", "copy", "-color_primaries", "bt2020", "-color_trc", "smpte2084", "-colorspace", "bt2020nc"]));
    remux.push(container_only.as_os_str().to_owned());
    run_ok(&ff, &remux);

    let tested: Vec<(String, String)> = working_export_encoders(&ff).into_iter().filter(|(_, c)| c != "h264").collect();
    assert!(tested.iter().any(|(n, _)| n == "libx265"), "{tested:?}");
    for source in [&tagged, &container_only] {
        let info = probe::probe(Path::new(&ff.ffprobe_path), source).unwrap();
        assert!(info.video.as_ref().unwrap().hdr, "{}", source.display());
        for (encoder, codec) in &tested {
            let label = format!("{} → {encoder}", source.file_name().unwrap().to_string_lossy());
            let (out, plan) = precise_export(&ff, source, 0.5, 1.5, output_for(codec), encoder);
            assert!(plan.ten_bit);
            let tag = if codec == "hevc" { "codec_name=hevc codec_tag_string=hvc1" } else { "codec_name=av1 codec_tag_string=av01" };
            assert_eq!(
                video_stream_line(&ff, &out),
                format!("{tag} color_primaries=bt2020 color_space=bt2020nc color_transfer=smpte2084 pix_fmt=yuv420p10le"),
                "{label}"
            );
            std::fs::remove_file(&out).unwrap();
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

// --- precise cut-out (구간 제거) ---

/// One precise cut-out through the app's code path (plan_precise_cut → output_paths_as → args → JobManager).
fn precise_cut(
    ff: &ffmpeg::FfmpegInfo,
    source: &Path,
    removed: &[plan::Range],
    output: command::OutputCodec,
    encoder: &str,
) -> (PathBuf, command::PreciseCutPlan) {
    let info = probe::probe(Path::new(&ff.ffprobe_path), source).unwrap();
    let plan = command::plan_precise_cut(&info, removed, output).unwrap();
    let (final_path, part) = command::output_paths_as(source, &plan.suffix, Some(&plan.extension));
    let args = command::precise_cut_args(source, &info, &plan, encoder, command::Quality::High, &part).unwrap();
    let finished = run_attempts(ff, vec![Attempt { args, label: encoder.into() }], plan.duration, &part, &final_path);
    assert!(finished.ok, "{encoder} on {}: {:?}", source.display(), finished.error);
    assert!(!part.exists());
    (final_path, plan)
}

/// 30 fps frame `n` as a time, as the editor sends cut points (frame starts).
fn f30(n: usize) -> f64 {
    n as f64 / 30.0
}

/// The frame-exact promise of 구간 제거: the result is the kept frames of the source, in order, and
/// nothing else — B-frame MP4 and MKV, every encoder that works here. The removed ranges come
/// unsorted and overlapping (the editor merges them, but Rust must not depend on it), cut points sit
/// on frames whose start is not a whole millisecond (#41, #152), and the last range runs to the end.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn precise_cut_shows_exactly_the_kept_frames() {
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx264, "this test needs libx264 for its source clips");
    let dir = temp_dir("cut-frames");
    let encoders = working_export_encoders(&ff);
    assert!(encoders.iter().any(|(n, _)| n == "libx264"), "working encoders: {encoders:?}");
    let mp4 = dir.join("구간 제거 확인 30fps B2.mp4");
    let mut make = os_args(&[
        "-hide_banner", "-nostdin", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30:duration=8",
        "-c:v", "libx264", "-g", "30", "-bf", "2", "-pix_fmt", "yuv420p",
    ]);
    make.push(mp4.as_os_str().to_owned());
    run_ok(&ff, &make);
    let mkv = mp4.with_extension("mkv");
    let mut remux = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"]);
    remux.extend([mp4.as_os_str().to_owned(), "-c".into(), "copy".into(), mkv.as_os_str().to_owned()]);
    run_ok(&ff, &remux);

    // Keep #0–#40, #70–#151, #200–#225 (149 frames).
    let removed = [
        plan::Range::new(f30(165), f30(200)),
        plan::Range::new(f30(41), f30(70)),
        plan::Range::new(f30(226), 99.0),
        plan::Range::new(f30(152), f30(171)),
    ];
    let expected: Vec<usize> = (0..41).chain(70..152).chain(200..226).collect();
    for source in [&mp4, &mkv] {
        let shown = small_frames(&ff, source);
        assert_eq!(shown.len(), 240);
        for (encoder, codec) in &encoders {
            let label = format!("{} → {encoder}", source.file_name().unwrap().to_string_lossy());
            let (out, plan) = precise_cut(&ff, source, &removed, output_for(codec), encoder);
            assert_eq!(plan.keep.len(), 3, "{label}");
            let matched: Vec<usize> = small_frames(&ff, &out).iter().map(|f| nearest(&shown, f)).collect();
            assert_eq!(matched, expected, "{label}: output frames are not the kept source frames");
            let line = video_stream_line(&ff, &out);
            let codec_fields = match codec.as_str() {
                "hevc" if plan.extension == "mp4" => "codec_name=hevc codec_tag_string=hvc1",
                "hevc" => "codec_name=hevc ",
                "av1" => "codec_name=av1 ",
                _ => "codec_name=h264 ",
            };
            assert!(line.starts_with(codec_fields), "{label}: {line}");
            let length = probe::probe(Path::new(&ff.ffprobe_path), &out).unwrap().duration;
            let want = expected.len() as f64 / 30.0;
            assert!((length - want).abs() <= 1.0 / 30.0 + 1e-3, "{label}: length {length}, want {want}");
            assert!((plan.duration - want).abs() < 1e-9, "{label}: Σkeep {}", plan.duration);
            std::fs::remove_file(&out).unwrap();
        }
    }

    // Many small pieces (the first half of every second, 8 kept), one process: still exactly the kept frames.
    let removed: Vec<plan::Range> = (0..8).map(|k| plan::Range::new(f30(15 + 30 * k), f30(30 + 30 * k))).collect();
    let (out, plan) = precise_cut(&ff, &mkv, &removed, command::OutputCodec::Source, "libx264");
    assert_eq!(plan.keep.len(), 8);
    let expected: Vec<usize> = (0..8).flat_map(|k| (30 * k)..(30 * k + 15)).collect();
    let shown = small_frames(&ff, &mkv);
    let matched: Vec<usize> = small_frames(&ff, &out).iter().map(|f| nearest(&shown, f)).collect();
    assert_eq!(matched, expected, "8 pieces");
    std::fs::remove_file(&out).unwrap();

    // One piece left is a precise trim. OUT #226 is stored as 7.533 in MKV (rounded down from
    // 7.5333…): without CUT_END_MARGIN it came into the result (27 frames instead of 26).
    let removed = [plan::Range::new(0.0, f30(200)), plan::Range::new(f30(226), 99.0)];
    let (out, plan) = precise_cut(&ff, &mkv, &removed, command::OutputCodec::Source, "libx264");
    assert!(plan.as_trim().is_some());
    let matched: Vec<usize> = small_frames(&ff, &out).iter().map(|f| nearest(&shown, f)).collect();
    assert_eq!(matched, (200..226).collect::<Vec<_>>(), "one piece");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Times of the clicks (runs of samples above half the loudest) in audio track `track`, on the
/// output timeline.
fn click_times(ff: &ffmpeg::FfmpegInfo, out: &Path, track: usize) -> Vec<f64> {
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-i"]);
    args.push(out.as_os_str().to_owned());
    let map = format!("0:a:{track}");
    args.extend(os_args(&["-map", &map, "-ac", "1", "-ar", "44100", "-f", "s16le", "-"]));
    let samples: Vec<u16> =
        run_ok(ff, &args).stdout_raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs()).collect();
    let loudest = samples.iter().copied().max().unwrap_or(0);
    assert!(loudest > 1000, "no click in track {track}");
    let start = probe::probe(Path::new(&ff.ffprobe_path), out).unwrap().audio[track].start_time.unwrap_or(0.0);
    let mut clicks: Vec<f64> = Vec::new();
    let mut last: Option<usize> = None;
    for (i, v) in samples.iter().enumerate() {
        if *v > loudest / 2 {
            if last.is_none_or(|l| i - l > 2205) {
                clicks.push(i as f64 / 44100.0 + start);
            }
            last = Some(i);
        }
    }
    clicks
}

/// Picture and sound stay together across the joins: clicks in three kept pieces land where the
/// cut timeline puts them, in both audio tracks (the concat filter joins every track), the first
/// track keeps its language, and the job falls back from a failing encoder like a trim does.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn precise_cut_keeps_sync_across_joins_and_falls_back() {
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx264 && ff.features.libx265, "this test needs libx264 and libx265");
    let dir = temp_dir("cut-sync");
    let source = dir.join("소리 확인 구간 제거.mp4");
    let clicks = r"aevalsrc=exprs='if(between(t\,1.0\,1.001)+between(t\,4.0\,4.001)+between(t\,8.0\,8.001)\,0.9\,0)':s=44100:d=10";
    let mut make = os_args(&[
        "-hide_banner", "-nostdin", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=10",
        "-f", "lavfi", "-i", clicks,
        "-f", "lavfi", "-i", clicks,
        "-map", "0:v", "-map", "1:a", "-map", "2:a",
        "-c:v", "libx264", "-g", "25", "-bf", "2", "-pix_fmt", "yuv420p", "-c:a", "aac",
        "-metadata:s:a:0", "language=kor", "-shortest",
    ]);
    make.push(source.as_os_str().to_owned());
    run_ok(&ff, &make);
    let ffprobe = PathBuf::from(&ff.ffprobe_path);
    let info = probe::probe(&ffprobe, &source).unwrap();
    assert_eq!(info.audio.len(), 2);

    // Remove 2–3 and 5–7: keep 0–2, 3–5, 7–10 (7 s). Clicks 1 · 4 · 8 → 1 · 3 · 5.
    let removed = [plan::Range::new(5.0, 7.0), plan::Range::new(2.0, 3.0)];
    let plan = command::plan_precise_cut(&info, &removed, command::OutputCodec::Hevc).unwrap();
    assert_eq!((plan.duration, plan.audio), (7.0, command::AudioPlan::Aac));
    let (final_path, part) = command::output_paths_as(&source, &plan.suffix, Some(&plan.extension));
    let good = command::precise_cut_args(&source, &info, &plan, "libx265", command::Quality::Balanced, &part).unwrap();
    let broken: Vec<OsString> =
        good.iter().map(|a| if a == "libx265" { OsString::from("hevc_does_not_exist") } else { a.clone() }).collect();
    let attempts =
        vec![Attempt { args: broken, label: "가짜 인코더".into() }, Attempt { args: good, label: "libx265".into() }];
    let finished = run_attempts(&ff, attempts, plan.duration, &part, &final_path);
    assert!(finished.ok, "{:?}", finished.error);
    let note = finished.note.expect("retry note");
    assert!(matches!(&note, Msg::RetriedWith { failed, next, .. } if failed == "가짜 인코더" && next == "libx265"), "{note}");
    assert_eq!(final_path.file_name().unwrap(), "소리 확인 구간 제거_cut_h265.mp4");

    let out = probe::probe(&ffprobe, &final_path).unwrap();
    assert_eq!(out.audio.iter().map(|a| a.codec.as_str()).collect::<Vec<_>>(), ["aac", "aac"]);
    assert_eq!(out.audio[0].language.as_deref(), Some("kor"));
    assert!((out.duration - 7.0).abs() <= 0.04 + 0.03, "length {}", out.duration);
    for track in 0..2 {
        let got = click_times(&ff, &final_path, track);
        assert_eq!(got.len(), 3, "track {track}: clicks at {got:?}");
        for (t, want) in got.iter().zip([1.0, 3.0, 5.0]) {
            assert!((t - want).abs() < 0.015, "track {track}: click at {t:.4} s, expected {want}");
        }
    }
    let shown = small_frames(&ff, &source);
    let frames = small_frames(&ff, &final_path);
    assert_eq!(frames.len(), 175, "7 s at 25 fps");
    let firsts: Vec<usize> = [0, 49, 50, 99, 100, 174].iter().map(|&i| nearest(&shown, &frames[i])).collect();
    assert_eq!(firsts, [0, 49, 75, 124, 175, 249], "first and last frame of each piece");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The colors go through `setparams` after the concat: a 10-bit HDR source tagged only in its MKV
/// container stays 10-bit PQ / BT.2020 when cut, with every H.265 · AV1 encoder that works here.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn precise_cut_keeps_ten_bit_and_hdr_tags() {
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx265, "this test needs libx265 for its source clip");
    let dir = temp_dir("cut-hdr");
    let untagged = dir.join("HDR 색 정보 없는 원본.mkv");
    let mut make = os_args(&[
        "-hide_banner", "-nostdin", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30:duration=3",
        "-c:v", "libx265", "-x265-params", "log-level=error", "-pix_fmt", "yuv420p10le",
    ]);
    make.push(untagged.as_os_str().to_owned());
    run_ok(&ff, &make);
    let source = dir.join("HDR 컨테이너만 표시.mkv");
    let mut remux = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"]);
    remux.push(untagged.as_os_str().to_owned());
    remux.extend(os_args(&["-c", "copy", "-color_primaries", "bt2020", "-color_trc", "smpte2084", "-colorspace", "bt2020nc"]));
    remux.push(source.as_os_str().to_owned());
    run_ok(&ff, &remux);

    let removed = [plan::Range::new(f30(20), f30(40)), plan::Range::new(f30(60), f30(70))];
    let tested: Vec<(String, String)> = working_export_encoders(&ff).into_iter().filter(|(_, c)| c != "h264").collect();
    assert!(tested.iter().any(|(n, _)| n == "libx265"), "{tested:?}");
    for (encoder, codec) in &tested {
        let (out, plan) = precise_cut(&ff, &source, &removed, output_for(codec), encoder);
        assert!(plan.ten_bit && plan.keep.len() == 3);
        let tag = if codec == "hevc" { "codec_name=hevc codec_tag_string=hvc1" } else { "codec_name=av1 codec_tag_string=av01" };
        assert_eq!(
            video_stream_line(&ff, &out),
            format!("{tag} color_primaries=bt2020 color_space=bt2020nc color_transfer=smpte2084 pix_fmt=yuv420p10le"),
            "{encoder}"
        );
        assert_eq!(small_frames(&ff, &out).len(), 90 - 30, "{encoder}");
        std::fs::remove_file(&out).unwrap();
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

// --- lossless cut-out (구간 제거 · 무손실) ---

/// Starts a lossless cut-out through the app's code path (plan_lossless_cut → check_joins →
/// lossless_cut_job → start_steps); a single piece runs as a trim, as in `start_export`.
fn start_lossless_cut(
    ff: &ffmpeg::FfmpegInfo,
    jobs: &JobManager,
    source: &Path,
    removed: &[plan::Range],
) -> (u64, mpsc::Receiver<JobEvent>, command::LosslessCutPlan, PathBuf, Option<PathBuf>) {
    let ffprobe = PathBuf::from(&ff.ffprobe_path);
    let program = PathBuf::from(&ff.ffmpeg_path);
    let info = probe::probe(&ffprobe, source).unwrap();
    let video = info.video.clone().expect("video");
    let kf = probe::keyframes(&ffprobe, source, video.index).unwrap();
    let plan = command::plan_lossless_cut(&info, &kf, removed).unwrap();
    let (final_path, part) = command::output_paths(source, "_cut");
    let (tx, rx) = mpsc::channel();
    let send = move |e| {
        let _ = tx.send(e);
    };
    if let [piece] = plan.pieces.as_slice() {
        let args = command::lossless_trim_args(source, Some(video.index), piece.seek, piece.duration, &part);
        let id = jobs.start(&program, args, piece.duration, part, final_path.clone(), send).unwrap();
        return (id, rx, plan, final_path, None);
    }
    jobs::check_joins(&program, source, &video, &plan).unwrap();
    let job = jobs::lossless_cut_job(&program, &ffprobe, source, video.index, &plan, &final_path, &part, "it").unwrap();
    let scratch = job.scratch.clone();
    // The first step makes it, when the job runs (a queued cut leaves nothing on disk while it waits).
    assert!(!scratch.exists() && scratch.file_name().unwrap().to_str().unwrap().is_ascii());
    let id = jobs.start_steps(&program, job.steps, plan.length, job.scratch, part, final_path.clone(), send);
    (id, rx, plan, final_path, Some(scratch))
}

fn wait_for_end(rx: &mpsc::Receiver<JobEvent>) -> (JobFinished, Vec<f64>) {
    let mut percents = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(90)).expect("no job event within 90 s") {
            JobEvent::Progress(p) => percents.push(p.percent),
            JobEvent::Finished(f) => return (f, percents),
        }
    }
}

fn lossless_cut(ff: &ffmpeg::FfmpegInfo, source: &Path, removed: &[plan::Range]) -> (PathBuf, command::LosslessCutPlan) {
    let jobs = JobManager::default();
    let (_, rx, plan, final_path, scratch) = start_lossless_cut(ff, &jobs, source, removed);
    let (finished, percents) = wait_for_end(&rx);
    assert!(finished.ok, "{}: {:?}", source.display(), finished.error);
    assert_eq!(finished.output.as_deref(), Some(final_path.to_str().unwrap()));
    assert!(percents.windows(2).all(|w| w[0] <= w[1]) && percents.last() == Some(&100.0), "progress {percents:?}");
    assert!(scratch.is_none_or(|s| !s.exists()), "the scratch folder is removed");
    (final_path, plan)
}

/// md5 of every decoded frame in order, none dropped or doubled (`-fps_mode passthrough`).
fn frame_md5s(ff: &ffmpeg::FfmpegInfo, file: &Path) -> Vec<String> {
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-i"]);
    args.push(file.as_os_str().to_owned());
    args.extend(os_args(&["-map", "0:v:0", "-fps_mode", "passthrough", "-f", "framemd5", "-"]));
    run_ok(ff, &args).stdout.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.rsplit(',').next()).map(|m| m.trim().to_string()).collect()
}

/// Checks a lossless cut-out frame by frame: each piece is the source frames from its keyframe to the
/// one before its end, then (B-frame sources) later frames in order, below OUT + `extra`, as a trim's
/// end. With OUT on a keyframe that reaches OUT + has_b_frames + 1: x264 -bf 2, OUT #150 → … 149 150
/// 153 (the I-frame at OUT, and the P-frame its B-frames wait for).
fn assert_pieces(label: &str, shown: &[String], out: &[String], pieces: &[(usize, usize)], extra: usize) {
    let index_of = |m: &String| shown.iter().position(|s| s == m);
    let mut pos = 0;
    for &(from, stop) in pieces {
        let want = &shown[from..stop];
        assert!(out.len() >= pos + want.len() && out[pos..pos + want.len()] == *want, "{label}: piece #{from}..#{stop} is not all there in order (at output frame {pos}: {:?})", out.get(pos).and_then(index_of));
        pos += want.len();
        let mut last = stop - 1;
        while let Some(i) = out.get(pos).and_then(index_of).filter(|&i| i > last && i < stop + extra && extra > 0) {
            last = i;
            pos += 1;
        }
    }
    assert_eq!(pos, out.len(), "{label}: frames after the last piece: {:?}", out[pos..].iter().map(index_of).collect::<Vec<_>>());
}

/// What 구간 제거 · 무손실 promises: every kept piece starts at the keyframe at or before its start
/// (MP4 too — edit lists do not survive the join) and ends right before its OUT (B-frame sources: a
/// few frames more, as a lossless trim), nothing of the pieces is lost at the joins, and a removal
/// with no keyframe inside stays. x264 B0 · B2 × MP4 · MKV, 30 fps, a keyframe every 25 frames, so
/// the keyframes the pieces start from (#50 = 1.6666… s, #200) and cut points sit on frame starts
/// that are not whole milliseconds.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn lossless_cut_keeps_every_piece_from_its_keyframe() {
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx264, "this test needs libx264 for its source clips");
    let dir = temp_dir("lossless-cut");
    // Remove #41–69, #100–109 (it starts on a keyframe, but there is none after it before #110:
    // stays), #150–199 (OUT on a keyframe: in MP4 the frames past it overlapped the next piece until
    // the list gave each piece its duration) and #226 to the end.
    let removed = [
        plan::Range::new(f30(150), f30(200)),
        plan::Range::new(f30(100), f30(110)),
        plan::Range::new(f30(226), 99.0),
        plan::Range::new(f30(41), f30(70)),
    ];
    for bf in ["0", "2"] {
        let mp4 = dir.join(format!("무손실 구간 제거 B{bf}.mp4"));
        let mut make = os_args(&[
            "-hide_banner", "-nostdin", "-loglevel", "error", "-y",
            "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30:duration=8",
            "-c:v", "libx264", "-g", "25", "-keyint_min", "25", "-sc_threshold", "0", "-bf", bf, "-pix_fmt", "yuv420p",
        ]);
        make.push(mp4.as_os_str().to_owned());
        run_ok(&ff, &make);
        let mkv = mp4.with_extension("mkv");
        let mut remux = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"]);
        remux.extend([mp4.as_os_str().to_owned(), "-c".into(), "copy".into(), mkv.as_os_str().to_owned()]);
        run_ok(&ff, &remux);
        for source in [&mp4, &mkv] {
            let label = source.file_name().unwrap().to_string_lossy().into_owned();
            let shown = frame_md5s(&ff, source);
            assert_eq!(shown.len(), 240, "{label}");
            let (out, plan) = lossless_cut(&ff, source, &removed);
            assert_eq!(out.file_name().unwrap().to_string_lossy(), label.replace(".m", "_cut.m"));
            assert_eq!(plan.kept_removals, vec![plan::Range::new(f30(100), f30(110))], "{label}");
            let starts: Vec<f64> = plan.pieces.iter().map(|p| (p.visible_start * 30.0).round()).collect();
            assert_eq!(starts, [0.0, 50.0, 200.0], "{label}: keyframes #0 · #50 · #200");
            let extra = if bf == "0" { 0 } else { 2 + 2 };
            let frames = frames_by_pts(&ff, &out);
            let md5s: Vec<String> = frames.iter().map(|f| f.1.clone()).collect();
            assert_pieces(&label, &shown, &md5s, &[(0, 41), (50, 150), (200, 226)], extra);
            // In the order they are shown, every frame comes later than the one before: no piece
            // overlaps the next (players drop or reorder frames whose times go back).
            let back: Vec<usize> = frames.windows(2).enumerate().filter(|(_, w)| w[1].0 <= w[0].0 + 1e-6).map(|(i, _)| i + 1).collect();
            assert!(back.is_empty(), "{label}: frame times go back at output frames {back:?}");
            if bf == "0" {
                let length = probe::probe(Path::new(&ff.ffprobe_path), &out).unwrap().duration;
                assert!((length - plan.length).abs() <= 1.0 / 30.0, "{label}: length {length}, planned {}", plan.length);
            }
            std::fs::remove_file(&out).unwrap();
        }
    }

    // One piece left: a trim, and the MP4 edit list starts it exactly at #80 (mid-GOP).
    let mp4 = dir.join("무손실 구간 제거 B2.mp4");
    let (out, plan) = lossless_cut(&ff, &mp4, &[plan::Range::new(0.0, f30(80))]);
    assert!(plan.exact_start && plan.pieces.len() == 1);
    let (shown, got) = (frame_md5s(&ff, &mp4), frame_md5s(&ff, &out));
    assert_eq!(got.first(), shown.get(80));
    assert_eq!(got.len(), 240 - 80);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Clicks of `track` on the output timeline as its timestamps place them: gaps between the pieces'
/// audio are filled with silence (`aresample` async), as players that follow timestamps play them.
fn click_times_by_pts(ff: &ffmpeg::FfmpegInfo, out: &Path, track: usize) -> Vec<f64> {
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-copyts", "-i"]);
    args.push(out.as_os_str().to_owned());
    let map = format!("0:a:{track}");
    args.extend(os_args(&["-map", &map, "-af", "aresample=async=1:min_hard_comp=0.0005:first_pts=0", "-ac", "1", "-ar", "44100", "-f", "s16le", "-"]));
    let samples: Vec<u16> =
        run_ok(ff, &args).stdout_raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs()).collect();
    let loudest = samples.iter().copied().max().unwrap_or(0);
    assert!(loudest > 1000, "no click in track {track}");
    let mut clicks = Vec::new();
    let mut last: Option<usize> = None;
    for (i, v) in samples.iter().enumerate() {
        if *v > loudest / 2 {
            if last.is_none_or(|l| i - l > 2205) {
                clicks.push(i as f64 / 44100.0);
            }
            last = Some(i);
        }
    }
    clicks
}

/// (time on the output timeline, md5) of every frame, by its own timestamp.
fn frames_by_pts(ff: &ffmpeg::FfmpegInfo, file: &Path) -> Vec<(f64, String)> {
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-copyts", "-i"]);
    args.push(file.as_os_str().to_owned());
    args.extend(os_args(&["-map", "0:v:0", "-fps_mode", "passthrough", "-enc_time_base:v", "demux", "-f", "framemd5", "-"]));
    let text = run_ok(ff, &args).stdout;
    let mut tb = 1.0;
    let mut frames = Vec::new();
    for line in text.lines() {
        if let Some(t) = line.strip_prefix("#tb 0:") {
            let (n, d) = t.trim().split_once('/').unwrap();
            tb = n.parse::<f64>().unwrap() / d.parse::<f64>().unwrap();
        } else if !line.starts_with('#') {
            let cols: Vec<&str> = line.split(',').map(str::trim).collect();
            frames.push((cols[2].parse::<f64>().unwrap() * tb, cols[5].to_string()));
        }
    }
    frames
}

/// Picture and sound stay together across the joins: in each of three pieces (two of them pulled
/// back to their keyframe), the click lands where the frame that shows its source time does — both
/// audio tracks, B0 · B2 × MP4 · MKV. In B0 MP4 the audio also runs on without gaps, so players that
/// just play the samples stay in sync too. The first track keeps its language, the file its title.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn lossless_cut_keeps_sync_across_joins() {
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx264, "this test needs libx264");
    let dir = temp_dir("lossless-cut-sync");
    let clicks = r"aevalsrc=exprs='if(between(t\,1.0\,1.001)+between(t\,4.0\,4.001)+between(t\,8.0\,8.001)\,0.9\,0)':s=44100:d=10";
    // Remove 2.0–3.4 and 5.0–7.6: pieces 0–2, 3.0–5 and 7.0–10 (keyframes every second).
    let removed = [plan::Range::new(5.0, 7.6), plan::Range::new(2.0, 3.4)];
    for bf in ["0", "2"] {
        let mp4 = dir.join(format!("소리 확인 무손실 B{bf}.mp4"));
        let mut make = os_args(&[
            "-hide_banner", "-nostdin", "-loglevel", "error", "-y",
            "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=10",
            "-f", "lavfi", "-i", clicks,
            "-f", "lavfi", "-i", clicks,
            "-map", "0:v", "-map", "1:a", "-map", "2:a",
            "-c:v", "libx264", "-g", "25", "-keyint_min", "25", "-sc_threshold", "0", "-bf", bf, "-pix_fmt", "yuv420p",
            "-c:a", "aac", "-metadata:s:a:0", "language=kor", "-metadata", "title=제주 여행 0412", "-shortest",
        ]);
        make.push(mp4.as_os_str().to_owned());
        run_ok(&ff, &make);
        let mkv = mp4.with_extension("mkv");
        let mut remux = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"]);
        remux.extend([mp4.as_os_str().to_owned(), "-map".into(), "0".into(), "-c".into(), "copy".into(), mkv.as_os_str().to_owned()]);
        run_ok(&ff, &remux);
        for source in [&mp4, &mkv] {
            let label = source.file_name().unwrap().to_string_lossy().into_owned();
            let (out, plan) = lossless_cut(&ff, source, &removed);
            assert_eq!(plan.pieces.iter().map(|p| p.visible_start).collect::<Vec<_>>(), [0.0, 3.0, 7.0], "{label}");
            let info = probe::probe(Path::new(&ff.ffprobe_path), &out).unwrap();
            assert_eq!(info.audio.len(), 2, "{label}");
            assert_eq!(info.audio[0].language.as_deref(), Some("kor"), "{label}");
            let title = util::run_capture(
                Path::new(&ff.ffprobe_path),
                &[OsStr::new("-v"), "error".as_ref(), "-show_entries".as_ref(), "format_tags=title".as_ref(), "-of".as_ref(), "default=nw=1:nk=1".as_ref(), out.as_os_str()],
                Duration::from_secs(30),
            )
            .unwrap()
            .stdout;
            assert_eq!(title.trim(), "제주 여행 0412", "{label}: the file's metadata comes from the original");

            // Where the frame showing each click's source time lands (25 fps: #25 · #100 · #200).
            let source_frames = frame_md5s(&ff, source);
            let frames = frames_by_pts(&ff, &out);
            let at = |n: usize| frames.iter().find(|(_, m)| *m == source_frames[n]).map(|f| f.0).expect("frame in the output");
            let want = [at(25), at(100), at(200)];
            for track in 0..2 {
                let got = click_times_by_pts(&ff, &out, track);
                assert_eq!(got.len(), 3, "{label} track {track}: clicks at {got:?}");
                for (t, w) in got.iter().zip(want) {
                    assert!((t - w).abs() < 0.003, "{label} track {track}: click at {t:.4} s, its frame at {w:.4}");
                }
            }
            if bf == "0" && source == &mp4 {
                // Played sample after sample from the audio's start, without timestamps.
                for (t, w) in click_times(&ff, &out, 0).iter().zip(want) {
                    assert!((t - w).abs() < 0.003, "{label}: gapless audio, click at {t:.4} s, its frame at {w:.4}");
                }
            }
            std::fs::remove_file(&out).unwrap();
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A piece after a join has to start at an IDR frame: x265's default open GOP (CRA keyframes) is
/// refused before anything is written (joined, the decoder lost the frames up to the next keyframe
/// after each join), and with closed GOPs H.265 joins frame-exactly like H.264.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn lossless_cut_refuses_open_gop_joins() {
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx265, "this test needs libx265");
    let dir = temp_dir("lossless-cut-gop");
    let ffprobe = PathBuf::from(&ff.ffprobe_path);
    let removed = [plan::Range::new(f30(41), f30(70)), plan::Range::new(f30(152), f30(200))];
    for (name, params) in [("열린 GOP.mkv", "keyint=30:min-keyint=30:scenecut=0:log-level=error"), ("닫힌 GOP.mp4", "keyint=30:min-keyint=30:scenecut=0:open-gop=0:log-level=error")] {
        let source = dir.join(name);
        let mut make = os_args(&[
            "-hide_banner", "-nostdin", "-loglevel", "error", "-y",
            "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30:duration=8",
            "-c:v", "libx265", "-preset", "ultrafast", "-x265-params", params, "-pix_fmt", "yuv420p",
        ]);
        if name.ends_with("mp4") {
            make.extend(os_args(&["-tag:v", "hvc1"]));
        }
        make.push(source.as_os_str().to_owned());
        run_ok(&ff, &make);
        let info = probe::probe(&ffprobe, &source).unwrap();
        let video = info.video.clone().unwrap();
        let kf = probe::keyframes(&ffprobe, &source, video.index).unwrap();
        let plan = command::plan_lossless_cut(&info, &kf, &removed).unwrap();
        let checked = jobs::check_joins(Path::new(&ff.ffmpeg_path), &source, &video, &plan);
        if name.starts_with("열린") {
            let e = checked.unwrap_err();
            assert!(matches!(e, Msg::OpenGopJoin { at } if (at - 2.0).abs() < 1e-6), "{e}");
        } else {
            checked.unwrap();
            let (out, plan) = lossless_cut(&ff, &source, &removed);
            let extra = video.has_b_frames as usize + 2;
            assert_eq!(plan.pieces.iter().map(|p| p.visible_start).collect::<Vec<_>>(), [0.0, 2.0, 6.0]);
            assert_pieces(name, &frame_md5s(&ff, &source), &frame_md5s(&ff, &out), &[(0, 41), (60, 152), (180, 240)], extra);
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Canceled right after it starts: the job ends canceled and leaves neither the scratch folder nor
/// the `.part` file nor a result behind.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn lossless_cut_cancel_leaves_nothing() {
    let ff = ffmpeg_or_panic();
    let dir = temp_dir("lossless-cut-cancel");
    let source = dir.join("취소 확인 클립.mp4");
    make_clip(&ff, &source);
    let jobs = JobManager::default();
    let removed = [plan::Range::new(2.4, 3.5), plan::Range::new(6.2, 7.0)];
    let (id, rx, plan, final_path, scratch) = start_lossless_cut(&ff, &jobs, &source, &removed);
    assert!(jobs.cancel(id));
    assert_eq!(plan.pieces.len(), 3);
    let (finished, _) = wait_for_end(&rx);
    assert!(finished.canceled && !finished.ok, "{finished:?}");
    let scratch = scratch.unwrap();
    assert!(!scratch.exists() && !final_path.exists());
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left, [source.file_name().unwrap()], "only the original is left");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A 6 s B-frame (x264 `-bf 2`) source at `rate` in MP4, and the same packets in MKV. `select`
/// thins the frames into a variable rate (every 5 frames of 60 fps keep 3: gaps of 1/60, 1/60 and
/// 3/60 s, like a phone recording that drops frames).
fn make_rate_sources(ff: &ffmpeg::FfmpegInfo, dir: &Path, label: &str, rate: &str, select: Option<&str>, gop: &str) -> [PathBuf; 2] {
    let mp4 = dir.join(format!("프레임레이트 {label} B2.mp4"));
    let lavfi = format!("testsrc2=size=320x240:rate={rate}:duration=6");
    let mut make = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", &lavfi]);
    if let Some(s) = select {
        make.extend(os_args(&["-vf", s, "-fps_mode", "vfr"]));
    }
    make.extend(os_args(&["-c:v", "libx264", "-g", gop, "-bf", "2", "-pix_fmt", "yuv420p"]));
    make.push(mp4.as_os_str().to_owned());
    run_ok(ff, &make);
    let mkv = mp4.with_extension("mkv");
    let mut remux = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"]);
    remux.extend([mp4.as_os_str().to_owned(), "-c".into(), "copy".into(), mkv.as_os_str().to_owned()]);
    run_ok(ff, &remux);
    [mp4, mkv]
}

/// Phase 2 완료 기준 at NTSC 29.97, film 23.976 and a variable frame rate, B-frame MP4 and MKV:
/// 정밀 자르기 (every encoder that works here) and 정밀 구간 제거 give exactly the frames between the
/// cut points and a length within one frame of the request; 무손실 자르기 starts where it promises
/// and keeps every frame up to OUT − 1 (its B-frame tail is the known limit).
///
/// Cut points are what the editor sends: frame starts k / fps for constant rates (`frameStart` in
/// format.ts — at 29.97 and 23.976 almost none is a whole millisecond, and MKV rounds them to one),
/// and for the variable rate the frame times `probe_media` hands it (ffprobe's packet times, which
/// it prints rounded to the µs). The variable rate must be found in MKV too, where r_frame_rate and
/// avg_frame_rate agree and only the frame times tell.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn exports_stay_frame_exact_at_ntsc_film_and_variable_rates() {
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx264, "this test needs libx264 for its source clips");
    let ffprobe = PathBuf::from(&ff.ffprobe_path);
    let dir = temp_dir("rates");
    let encoders = working_export_encoders(&ff);
    let sources = [
        ("29.97fps", "30000/1001", None, "30"),
        ("23.976fps", "24000/1001", None, "24"),
        ("가변", "60", Some(r"select='lt(mod(n\,5)\,3)'"), "36"),
    ];
    for (rate_label, rate, select, gop) in sources {
        for source in make_rate_sources(&ff, &dir, rate_label, rate, select, gop) {
            let name = source.file_name().unwrap().to_string_lossy().into_owned();
            // As probe_media does: probe, then one packet listing for keyframes and frame times.
            let mut info = probe::probe(&ffprobe, &source).unwrap();
            let times = probe::frame_times(&ffprobe, &source, info.video.as_ref().expect("video stream").index).unwrap();
            probe::measure_frame_rate(info.video.as_mut().unwrap(), times.frames.clone());
            let video = info.video.clone().unwrap();
            let vfr = select.is_some();
            assert_eq!(video.vfr_suspect, vfr, "{name}: vfr_suspect");
            assert_eq!(video.frame_times.is_some(), vfr, "{name}: frame times for the editor");
            // Every frame at its own time: plain framemd5 drops frames of the variable-rate MKV
            // that land on one tick of its average-rate time base (PROGRESS 알게 된 것 42).
            let shown = frames_by_pts(&ff, &source);
            let small = small_frames(&ff, &source);
            assert_eq!(shown.len(), small.len(), "{name}: frame count");
            let n = shown.len();
            let gaps: Vec<f64> = shown.windows(2).map(|w| w[1].0 - w[0].0).collect();
            let frame = gaps.iter().cloned().fold(0.0, f64::max);
            let frame_times = video.frame_times.clone().unwrap_or_default();
            if vfr {
                assert_eq!(frame_times.len(), n, "{name}: one time per frame");
                assert!(frame_times.iter().zip(&shown).all(|(t, s)| (t - s.0).abs() < 1e-5), "{name}: frame times");
            }
            // Where frame k starts, as the editor computes it.
            let fps = video.fps.expect("fps");
            let at = |k: usize| if vfr { frame_times[k] } else { k as f64 / fps };
            let span = |a: usize, b: usize| if b < n { shown[b].0 - shown[a].0 } else { info.duration - shown[a].0 };
            // IN and OUT mid-GOP, away from keyframes.
            let (first, stop) = (n * 35 / 100, n * 86 / 100);

            for (encoder, codec) in &encoders {
                let label = format!("{name} 정밀 자르기 → {encoder}");
                let (out, _) = precise_export(&ff, &source, at(first), at(stop), output_for(codec), encoder);
                let matched: Vec<usize> = small_frames(&ff, &out).iter().map(|f| nearest(&small, f)).collect();
                assert_eq!(matched, (first..stop).collect::<Vec<_>>(), "{label}: frames");
                let length = probe::probe(&ffprobe, &out).unwrap().duration;
                let want = span(first, stop);
                assert!((length - want).abs() <= frame + 1e-3, "{label}: length {length:.4}, want {want:.4} ± {frame:.4}");
                std::fs::remove_file(&out).unwrap();
            }

            // 구간 제거: two removed ranges, the second running to the end. The first kept piece ends on
            // a frame after a long gap at the variable rate (a multiple of 3: gaps 1, 1, 3 / 60 s).
            let (a, b, c) = (n * 20 / 100 / 3 * 3, n * 45 / 100, n * 70 / 100);
            let removed = [plan::Range { start: at(a), end: at(b) }, plan::Range { start: at(c), end: info.duration }];
            let label = format!("{name} 정밀 구간 제거");
            let (out, plan) = precise_cut(&ff, &source, &removed, command::OutputCodec::Source, "libx264");
            assert_eq!(plan.keep.len(), 2, "{label}: keep {:?}", plan.keep);
            let matched: Vec<usize> = small_frames(&ff, &out).iter().map(|f| nearest(&small, f)).collect();
            let expected: Vec<usize> = (0..a).chain(b..c).collect();
            assert_eq!(matched, expected, "{label}: frames");
            let length = probe::probe(&ffprobe, &out).unwrap().duration;
            let want = span(0, a) + span(b, c);
            assert!((length - want).abs() <= frame + 1e-3, "{label}: length {length:.4}, want {want:.4} ± {frame:.4}");
            // The second piece starts where the first ends: never early (without sound it came 23 ms
            // early at the variable rate — concat reckoned the first piece's end from its frames'
            // average spacing), less than a frame late, and its frames keep their spacing (MKV keeps
            // whole milliseconds, in the source and in the result).
            let joined: Vec<f64> = frames_by_pts(&ff, &out).into_iter().map(|(t, _)| t).collect();
            let first_late = joined[a] - (shown[b].0 - at(b) + at(a));
            assert!(first_late > -0.0015 && first_late < frame, "{label}: the second piece starts {first_late:+.4} s off");
            for (i, t) in joined.iter().enumerate() {
                let (planned, late) = if i < a { (shown[i].0, 0.0) } else { (shown[b + i - a].0 - at(b) + at(a), first_late) };
                assert!((t - planned - late).abs() < 0.0015, "{label}: frame {i} at {t:.4}, want {:.4}", planned + late);
            }
            std::fs::remove_file(&out).unwrap();

            // 무손실 자르기: MP4 from IN exactly (edit list), MKV from the keyframe; every frame to OUT − 1.
            let label = format!("{name} 무손실 자르기");
            let kf = times.keyframes.clone();
            let tp = command::plan_lossless_trim(&info, &kf, at(first), at(stop)).unwrap();
            let (final_path, part) = command::output_paths(&source, "_trim");
            let args = command::lossless_trim_args(&source, Some(video.index), tp.seek, tp.duration, &part);
            let (finished, _) = run_job(&ff, args, tp.duration, &part, &final_path);
            assert!(finished.ok, "{label}: job failed: {:?}", finished.error);
            let out: Vec<String> = frames_by_pts(&ff, &final_path).into_iter().map(|(_, md5)| md5).collect();
            let from = if tp.exact_start {
                first
            } else {
                let k = plan::keyframe_at_or_before(&kf, at(first)).unwrap();
                shown.iter().position(|(t, _)| (t - k).abs() < 0.001).expect("keyframe among the shown frames")
            };
            let index_of = |md5: &String| shown.iter().position(|(_, m)| m == md5);
            assert_eq!(out.first().and_then(index_of), Some(from), "{label}: first frame");
            let kept: Vec<String> = shown[from..stop].iter().map(|(_, md5)| md5.clone()).collect();
            assert!(out.len() >= kept.len() && out[..kept.len()] == kept[..], "{label}: frames {from}..{stop} not all there in order");
            // When the frame at OUT is an anchor (I or P) its own B frames wait for the next anchor,
            // which comes along too: one more than has_b_frames + 1 (23.976 #123 P → 123 126; the
            // keyframe case of PROGRESS 알게 된 것 41 is the same thing).
            let extra = video.has_b_frames as usize + 2;
            let tail: Vec<usize> = out[kept.len()..].iter().map(|m| index_of(m).expect("trailing frame is a source frame")).collect();
            assert!(tail.iter().all(|&i| i >= stop && i <= stop - 1 + extra), "{label}: frames after OUT − 1: {tail:?}");
            std::fs::remove_file(&final_path).unwrap();

            if vfr {
                // Why the editor snaps to frame times on a variable rate: a time inside the frame on
                // screen (what it sent before) makes FFmpeg start at the first frame starting at or
                // after IN — one frame after the frame shown — and keep the frame shown at OUT.
                let mid = |k: usize| (shown[k].0 + shown[k + 1].0) / 2.0;
                let label = format!("{name} 정밀 자르기 (프레임 가운데 시각)");
                let (out, _) = precise_export(&ff, &source, mid(first), mid(stop), command::OutputCodec::Source, "libx264");
                let matched: Vec<usize> = small_frames(&ff, &out).iter().map(|f| nearest(&small, f)).collect();
                assert_eq!(matched, (first + 1..=stop).collect::<Vec<_>>(), "{label}: frames");
                std::fs::remove_file(&out).unwrap();
            }
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---------------------------------------------------------------------------------------------
// 작업 큐 (Phase 3): exports through the app's own path — export::prepare on the probed file, then
// Queue::add with the names the queue leaves free — mixed with cancels and failures.

type QueueEvents = mpsc::Receiver<QueueEvent<ExportTask>>;

fn new_queue(concurrency: usize) -> (Queue<ExportTask>, Arc<JobManager>, QueueEvents) {
    let jobs = Arc::new(JobManager::default());
    let (tx, rx) = mpsc::channel();
    let queue = Queue::new(jobs.clone(), concurrency, move |e| {
        let _ = tx.send(e);
    });
    (queue, jobs, rx)
}

/// The probe the editor makes when a file is opened (info, keyframes).
fn opened(ff: &ffmpeg::FfmpegInfo, path: &Path) -> (probe::MediaInfo, Vec<f64>) {
    let ffprobe = Path::new(&ff.ffprobe_path);
    let info = probe::probe(ffprobe, path).unwrap();
    let kf = probe::keyframes(ffprobe, path, info.video.as_ref().unwrap().index).unwrap();
    (info, kf)
}

/// A report where only libx265 works: a precise H.265 export runs on the CPU (slow enough to cancel).
fn x265_only() -> encoders::EncoderReport {
    encoders::EncoderReport {
        tests: vec![encoders::EncoderTest { name: "libx265".into(), codec: "hevc".into(), hardware: false, ok: true, millis: 1, error: None }],
        preferred_hevc: Some("libx265".into()),
        preferred_av1: None,
    }
}

fn queue_export(
    ff: &ffmpeg::FfmpegInfo,
    queue: &Queue<ExportTask>,
    source: &Path,
    edit: Edit,
    mode: ExportMode,
    codec: command::OutputCodec,
) -> godseon_clip_lib::queue::QueueItem<ExportTask> {
    let (info, kf) = opened(ff, source);
    let req = ExportRequest {
        path: source.display().to_string(),
        edit,
        mode,
        codec,
        quality: command::Quality::Balanced,
        effects: Default::default(),
        frame_rate: None,
        gif_short: None,
    };
    let prepared = export::prepare(ff, &info, &kf, &req, x265_only).unwrap();
    queue.add(|taken| prepared.into_task(taken)).unwrap()
}

fn wait_queue(rx: &QueueEvents, what: &str, done: impl Fn(&QueueSnapshot<ExportTask>) -> bool) -> QueueSnapshot<ExportTask> {
    loop {
        match rx.recv_timeout(Duration::from_secs(120)).unwrap_or_else(|_| panic!("queue: no change within 120 s ({what})")) {
            QueueEvent::Changed(s) if done(&s) => return s,
            _ => {}
        }
    }
}

fn item_state(s: &QueueSnapshot<ExportTask>, id: u64) -> ItemState {
    s.items.iter().find(|i| i.id == id).expect("item in the queue").state
}

/// Everything in `dir` that is neither a source nor a result: `.part` files, scratch folders.
fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".part.") || n.starts_with(".godseon-cut-") || n.starts_with(".godseon-gif-"))
        .collect()
}

/// A 30 s 720p clip: a precise x265 export of it takes long enough to cancel while it runs.
fn make_long_clip(ff: &ffmpeg::FfmpegInfo, path: &Path) {
    let mut args = os_args(&[
        "-hide_banner", "-nostdin", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30:duration=30",
        "-f", "lavfi", "-i", "sine=frequency=440:duration=30",
        "-c:v", "libx264", "-preset", "ultrafast", "-g", "30", "-c:a", "aac", "-shortest",
    ]);
    args.push(path.as_os_str().to_owned());
    run_ok(ff, &args);
}

#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn queue_keeps_states_right_through_cancels_and_failures_and_leaves_nothing() {
    let ff = ffmpeg_or_panic();
    let dir = temp_dir("queue-mix");
    let clip = dir.join("큐 테스트 클립 01.mp4");
    let long = dir.join("긴 인코딩 원본.mp4");
    let doomed = dir.join("지워질 원본.mp4");
    make_clip(&ff, &clip);
    make_long_clip(&ff, &long);
    std::fs::copy(&clip, &doomed).unwrap();
    let (queue, jobs, rx) = new_queue(2);
    use command::OutputCodec::{Hevc, Source};
    use ExportMode::{Lossless, Precise};
    let cut = |removed: &[(f64, f64)]| Edit::Cut { removed: removed.iter().map(|&(a, b)| plan::Range::new(a, b)).collect() };

    // Two long encodes take both slots; the rest wait behind them.
    let encode = queue_export(&ff, &queue, &long, Edit::Trim { start: 0.0, end: 30.0 }, Precise, Hevc);
    let encode2 = queue_export(&ff, &queue, &long, Edit::Trim { start: 2.0, end: 28.0 }, Precise, Hevc);
    let trim = queue_export(&ff, &queue, &clip, Edit::Trim { start: 2.0, end: 6.0 }, Lossless, Source);
    let trim_again = queue_export(&ff, &queue, &clip, Edit::Trim { start: 2.0, end: 6.0 }, Lossless, Source);
    let pieces = queue_export(&ff, &queue, &clip, cut(&[(1.5, 2.5), (6.2, 7.0)]), Lossless, Source);
    let pieces_canceled = queue_export(&ff, &queue, &clip, cut(&[(3.0, 4.0)]), Lossless, Source);
    // Planned while the file was there; it is gone by the time the item runs.
    let fails = queue_export(&ff, &queue, &doomed, Edit::Trim { start: 1.0, end: 5.0 }, Lossless, Source);
    std::fs::remove_file(&doomed).unwrap();

    let s = queue.snapshot();
    let names: Vec<String> = s.items.iter().map(|i| Path::new(&i.output).file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names[2], "큐 테스트 클립 01_trim.mp4");
    assert_eq!(names[3], "큐 테스트 클립 01_trim (1).mp4", "the same export twice: two names, two .part files");
    assert_eq!(names[4], "큐 테스트 클립 01_cut.mp4");
    assert_eq!(names[5], "큐 테스트 클립 01_cut (1).mp4");
    assert_eq!(queue.active(), (2, 5));

    // Waiting: canceled before it starts (its scratch folder was never made).
    queue.cancel(pieces_canceled.id).unwrap();
    // Running: canceled once FFmpeg reported progress.
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).expect("progress of the long encode") {
            QueueEvent::Progress(p) if p.id == encode.id && p.progress.out_time > 0.0 => break,
            _ => {}
        }
    }
    assert!(leftovers(&dir).iter().any(|n| n.starts_with("긴 인코딩 원본_h265.part")), "{:?}", leftovers(&dir));
    queue.cancel(encode.id).unwrap();
    let s = wait_queue(&rx, "first encode canceled", |s| item_state(s, encode.id) == ItemState::Canceled);
    assert_eq!(item_state(&s, trim.id), ItemState::Running, "the freed slot goes to the next waiting item");
    // The second encode too, while it runs: the short ones finish two at a time after it.
    queue.cancel(encode2.id).unwrap();

    let s = wait_queue(&rx, "everything finished", |s| s.items.iter().all(|i| i.state.finished()));
    let get = |id: u64| s.items.iter().find(|i| i.id == id).unwrap().clone();
    for (item, want) in [
        (&encode, ItemState::Canceled),
        (&encode2, ItemState::Canceled),
        (&trim, ItemState::Done),
        (&trim_again, ItemState::Done),
        (&pieces, ItemState::Done),
        (&pieces_canceled, ItemState::Canceled),
        (&fails, ItemState::Failed),
    ] {
        let now = get(item.id);
        assert_eq!(now.state, want, "{}: {:?} {:?}", now.output, now.error, now.log);
        if want == ItemState::Done {
            assert!(Path::new(&now.output).is_file() && now.output_bytes.unwrap() > 0, "{now:?}");
        } else {
            assert!(!Path::new(&now.output).exists(), "{} must not exist", now.output);
        }
    }
    assert!(get(pieces_canceled.id).started_at.is_none());
    let failed = get(fails.id);
    assert!(matches!(&failed.error, Some(Msg::Raw { text }) if text.contains("No such file")), "{failed:?}");
    assert!(failed.log.as_deref().unwrap_or("").contains("No such file"), "the log keeps FFmpeg's stderr");
    let made = probe::probe(Path::new(&ff.ffprobe_path), Path::new(&get(trim.id).output)).unwrap();
    assert!((made.duration - 4.0).abs() < 0.1, "{}", made.duration);
    assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));

    // Run again after a cancel: the same names, now made.
    queue.restart(pieces_canceled.id).unwrap();
    let s = wait_queue(&rx, "restarted cut", |s| item_state(s, pieces_canceled.id).finished());
    let again = s.items.iter().find(|i| i.id == pieces_canceled.id).unwrap();
    assert_eq!(again.state, ItemState::Done, "{again:?}");
    assert!(again.output.ends_with("큐 테스트 클립 01_cut (1).mp4"));
    assert!(leftovers(&dir).is_empty() && jobs.running_count() == 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// App exit (lib.rs `shutdown`): running encodes are canceled, waiting items never start, and no
/// `.part` is left once the jobs say they are idle (a lossless cut's scratch folder on cancel:
/// `lossless_cut_cancel_leaves_nothing`).
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn closing_the_queue_cancels_and_cleans_up_everything() {
    let ff = ffmpeg_or_panic();
    let dir = temp_dir("queue-close");
    let long = dir.join("종료 테스트 원본.mp4");
    make_long_clip(&ff, &long);
    let (queue, jobs, rx) = new_queue(2);
    let encode = queue_export(&ff, &queue, &long, Edit::Trim { start: 0.0, end: 30.0 }, ExportMode::Precise, command::OutputCodec::Hevc);
    let removed = vec![plan::Range::new(5.0, 10.0), plan::Range::new(20.0, 22.0)];
    let cut = queue_export(&ff, &queue, &long, Edit::Cut { removed }, ExportMode::Precise, command::OutputCodec::Hevc);
    let waiting = queue_export(&ff, &queue, &long, Edit::Trim { start: 1.0, end: 9.0 }, ExportMode::Lossless, command::OutputCodec::Source);
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).expect("progress of the encode") {
            QueueEvent::Progress(p) if p.id == encode.id && p.progress.out_time > 0.0 => break,
            _ => {}
        }
    }
    assert_eq!(queue.active(), (2, 1));
    assert!(!leftovers(&dir).is_empty(), "something is being written");
    assert_eq!(queue.close(), 2);
    jobs.cancel_all();
    assert!(jobs.wait_idle(Duration::from_secs(8)), "every job ends within the exit wait");
    assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
    let s = queue.snapshot();
    for id in [encode.id, cut.id, waiting.id] {
        assert_eq!(item_state(&s, id), ItemState::Canceled);
    }
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left, [long.file_name().unwrap()], "only the original is left");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---------------------------------------------------------------------------------------------
// 일괄 변환 (Phase 3 step 2): files listed by `convert::scan`, probed as the convert screen does,
// planned and queued by `convert::queue_files` — the app's own path — then the results checked.

/// `key=value` of the entries ffprobe prints for one stream, sorted (its own order is fixed).
fn stream_fields(ff: &ffmpeg::FfmpegInfo, file: &Path, stream: &str, entries: &str) -> String {
    let mut args = os_args(&["-v", "error", "-select_streams", stream, "-show_entries", entries, "-of", "default=nw=1"]);
    args.push(file.as_os_str().to_owned());
    let out = util::run_capture(Path::new(&ff.ffprobe_path), &args, Duration::from_secs(30)).unwrap();
    assert!(out.success, "ffprobe failed: {}", out.stderr);
    let mut fields: Vec<&str> = out.stdout.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('[')).collect();
    fields.sort_unstable();
    fields.join(" ")
}

fn convert_settings(
    codec: command::VideoCodec,
    container: command::Container,
    resolution: command::Resolution,
    audio: command::AudioChoice,
    ten_bit: bool,
    encoder: Option<&str>,
    output_dir: Option<&Path>,
    suffix: &str,
) -> ConvertSettings {
    ConvertSettings {
        options: command::ConvertOptions { codec, container, resolution, audio, ten_bit, frame_rate: None },
        encoder: encoder.map(String::from),
        quality: command::Quality::Balanced,
        output_dir: output_dir.map(|d| d.display().to_string()),
        suffix: suffix.into(),
    }
}

/// 변환 시작 as lib.rs `queue_convert` does it: the settings checked once, the encoder picked once.
fn start_conversion(
    ff: &ffmpeg::FfmpegInfo,
    queue: &Queue<ConvertTask>,
    report: &encoders::EncoderReport,
    paths: &[&Path],
    settings: ConvertSettings,
) -> convert::ConvertQueued {
    convert::check_settings(&settings).unwrap();
    let choice = encoders::pick(report, settings.options.codec, settings.encoder.as_deref()).unwrap();
    let req = ConvertRequest { paths: paths.iter().map(|p| p.display().to_string()).collect(), settings };
    let ffprobe = PathBuf::from(&ff.ffprobe_path);
    convert::queue_files(queue, ff, &req, &choice, |p| probe::probe(&ffprobe, Path::new(p)), |_, _| None, |t| t)
}

/// Mean difference per byte between two small RGB frames (0–255).
fn mean_difference(a: &[u8], b: &[u8]) -> f64 {
    a.iter().zip(b).map(|(x, y)| f64::from(x.abs_diff(*y))).sum::<f64>() / a.len() as f64
}

#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn convert_makes_every_file_as_set_and_leaves_nothing() {
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx264 && ff.features.libx265 && ff.features.libsvtav1 && ff.features.libopus, "needs x264 · x265 · SVT-AV1 · libopus");
    let dir = temp_dir("convert");
    let out_dir = dir.join("변환 결과");
    std::fs::create_dir_all(&out_dir).unwrap();
    let make = |name: &str, args: &[&str]| -> PathBuf {
        let path = dir.join(name);
        let mut a = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y"]);
        a.extend(os_args(args));
        a.push(path.as_os_str().to_owned());
        run_ok(&ff, &a);
        path
    };
    // A phone's HDR video: HEVC 10-bit PQ 1920×1080 stored, shown upright (rotation), AAC.
    let stored = make("가로 저장.mov", &[
        "-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=30:duration=2",
        "-f", "lavfi", "-i", "sine=frequency=440:duration=2",
        "-vf", "format=yuv420p10le,setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc",
        "-c:v", "libx265", "-preset", "ultrafast", "-x265-params", "log-level=error", "-tag:v", "hvc1",
        "-c:a", "aac", "-shortest",
    ]);
    let phone = dir.join("아이폰 가족 HDR.mov");
    let mut rotate = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-display_rotation", "90", "-i"]);
    rotate.push(stored.as_os_str().to_owned());
    rotate.extend(os_args(&["-c", "copy"]));
    rotate.push(phone.as_os_str().to_owned());
    run_ok(&ff, &rotate);
    std::fs::remove_file(&stored).unwrap();
    // A meeting recording: H.264 8-bit 1080p, Opus.
    let meeting = make("회의 녹화 01.mkv", &[
        "-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=30:duration=2",
        "-f", "lavfi", "-i", "sine=frequency=440:duration=2",
        "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "libopus", "-shortest",
    ]);
    // A screen recording: H.264 720p with 5.1(side) AC-3, which libopus refuses as it is.
    let screen = make("화면 녹화 데모.mp4", &[
        "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30:duration=2",
        "-f", "lavfi", "-i", "sine=frequency=440:duration=2",
        "-af", "pan=5.1(side)|FL=c0|FR=c0|FC=c0|LFE=c0|SL=c0|SR=c0",
        "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "ac3", "-shortest",
    ]);
    let voice = make("음성 메모.m4a", &["-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:a", "aac"]);

    // Added as a folder, the way the screen does: video files only, sorted, the m4a left out.
    let listed = convert::scan(&[dir.clone()], convert::SCAN_LIMIT);
    let names: Vec<String> = listed.files.iter().map(|f| Path::new(f).file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["아이폰 가족 HDR.mov", "화면 녹화 데모.mp4", "회의 녹화 01.mkv"], "{:?}", listed.notices);

    use command::{AudioChoice, Container, Resolution, VideoCodec};
    let report = encoders::run(&ff);
    let (queue, jobs, rx) = {
        let jobs = Arc::new(JobManager::default());
        let (tx, rx) = mpsc::channel();
        let queue = Queue::new(jobs.clone(), 2, move |e: QueueEvent<ConvertTask>| {
            let _ = tx.send(e);
        });
        (queue, jobs, rx)
    };
    // 1. AV1 MKV 720p into the chosen folder with its own ending; the AAC fits MKV and is copied.
    let a = start_conversion(&ff, &queue, &report, &[&phone, &voice], convert_settings(
        VideoCodec::Av1, Container::Mkv, Resolution::P720, AudioChoice::CopyOrAac, false, None, Some(&out_dir), " 작게",
    ));
    assert_eq!(a.queued.len(), 1);
    assert_eq!(a.refused.len(), 1, "the voice memo has no video");
    assert_eq!(a.refused[0].error, Msg::ConvertNeedsVideo, "{:?}", a.refused);
    // 2 · 3. H.265 MP4 720p in 10 bits on libx265 (picked), next to the original: Opus → AAC.
    //       The same again while the first is queued: the next free name.
    let h265 = convert_settings(VideoCodec::Hevc, Container::Mp4, Resolution::P720, AudioChoice::CopyOrAac, true, Some("libx265"), None, "_h265");
    let b = start_conversion(&ff, &queue, &report, &[&meeting], h265.clone());
    let b2 = start_conversion(&ff, &queue, &report, &[&meeting], h265);
    // 4. AV1 WebM: 5.1(side) AC-3 → Opus 5.1. 5. H.265 MOV, AAC 128k, the automatic encoder.
    let c = start_conversion(&ff, &queue, &report, &[&screen], convert_settings(
        VideoCodec::Av1, Container::Webm, Resolution::P720, AudioChoice::CopyOrAac, false, None, None, "_av1",
    ));
    let d = start_conversion(&ff, &queue, &report, &[&screen], convert_settings(
        VideoCodec::Hevc, Container::Mov, Resolution::Source, AudioChoice::Aac128, false, None, None, "_h265",
    ));
    let ids: Vec<u64> = [&a, &b, &b2, &c, &d].iter().map(|q| q.queued[0].id).collect();
    let s = wait_queue_of(&rx, "every conversion", |s| ids.iter().all(|id| s.items.iter().any(|i| i.id == *id && i.state.finished())));
    let get = |id: u64| s.items.iter().find(|i| i.id == id).unwrap().clone();
    for id in &ids {
        let item = get(*id);
        assert_eq!(item.state, ItemState::Done, "{}: {:?} {:?}", item.output, item.error, item.log);
    }
    let output = |q: &convert::ConvertQueued| PathBuf::from(get(q.queued[0].id).output);
    let name = |q: &convert::ConvertQueued| output(q).file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(output(&a), out_dir.join("아이폰 가족 HDR 작게.mkv"));
    assert_eq!((name(&b), name(&b2)), ("회의 녹화 01_h265.mp4".into(), "회의 녹화 01_h265 (1).mp4".into()));
    assert_eq!((name(&c), name(&d)), ("화면 녹화 데모_av1.webm".into(), "화면 녹화 데모_h265.mov".into()));
    assert_eq!(get(a.queued[0].id).info.summary.scaled, Some([720, 1280]));

    let video = "stream=codec_name,codec_tag_string,width,height,pix_fmt,color_primaries,color_transfer,color_space";
    assert_eq!(
        stream_fields(&ff, &output(&a), "v:0", video),
        "codec_name=av1 codec_tag_string=[0][0][0][0] color_primaries=bt2020 color_space=bt2020nc color_transfer=smpte2084 height=1280 pix_fmt=yuv420p10le width=720",
        "upright, short side 720, 10-bit HDR kept"
    );
    assert_eq!(stream_fields(&ff, &output(&a), "a:0", "stream=codec_name"), "codec_name=aac", "copied");
    for out in [output(&b), output(&b2)] {
        assert_eq!(
            stream_fields(&ff, &out, "v:0", "stream=codec_name,codec_tag_string,width,height,pix_fmt"),
            "codec_name=hevc codec_tag_string=hvc1 height=720 pix_fmt=yuv420p10le width=1280"
        );
        assert_eq!(stream_fields(&ff, &out, "a:0", "stream=codec_name"), "codec_name=aac");
    }
    assert_eq!(stream_fields(&ff, &output(&c), "v:0", "stream=codec_name,width,height"), "codec_name=av1 height=720 width=1280");
    assert_eq!(stream_fields(&ff, &output(&c), "a:0", "stream=codec_name,channels,channel_layout"), "channel_layout=5.1 channels=6 codec_name=opus");
    assert_eq!(stream_fields(&ff, &output(&d), "v:0", "stream=codec_name,codec_tag_string,width,height"), "codec_name=hevc codec_tag_string=hvc1 height=720 width=1280");
    let aac = stream_fields(&ff, &output(&d), "a:0", "stream=codec_name,bit_rate");
    let bits: u64 = aac.split("bit_rate=").nth(1).and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse().ok()).unwrap();
    assert!(aac.contains("codec_name=aac") && (100_000..=140_000).contains(&bits), "{aac}");
    let formats = |p: &Path| probe::probe(Path::new(&ff.ffprobe_path), p).unwrap();
    assert_eq!(formats(&output(&c)).format_name, "matroska,webm");
    for out in [output(&a), output(&b), output(&c), output(&d)] {
        let info = formats(&out);
        assert!((info.duration - 2.0).abs() <= 1.0 / 30.0 + 0.03, "{}: {}", out.display(), info.duration);
    }

    // The whole file, frame for frame, and the colors not shifted by the downscale.
    let source = small_frames(&ff, &phone);
    let made = small_frames(&ff, &output(&a));
    assert_eq!(made.len(), source.len(), "every frame");
    for k in [0, 17, 59] {
        assert_eq!(nearest(&source, &made[k]), k);
        let diff = mean_difference(&source[k], &made[k]);
        assert!(diff < 8.0, "frame {k} differs by {diff:.1} per byte");
    }
    assert_eq!(small_frames(&ff, &output(&b)).len(), 60);

    for d in [&dir, &out_dir] {
        assert!(leftovers(d).is_empty(), "{:?}", leftovers(d));
    }
    assert_eq!(jobs.running_count(), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

fn wait_queue_of<D: Clone>(rx: &mpsc::Receiver<QueueEvent<D>>, what: &str, done: impl Fn(&QueueSnapshot<D>) -> bool) -> QueueSnapshot<D> {
    loop {
        match rx.recv_timeout(Duration::from_secs(180)).unwrap_or_else(|_| panic!("queue: no change within 180 s ({what})")) {
            QueueEvent::Changed(s) if done(&s) => return s,
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 예상 크기 (Phase 3 step 3): the estimator's test encodes against the size the conversion makes.

/// `seconds` of 720p30 test pattern whose content changes by thirds — clean, heavy noise, light
/// noise — with AAC. The front of it says little about the rest, as an intro or a still opening does.
fn make_changing_clip(ff: &ffmpeg::FfmpegInfo, path: &Path, seconds: u32, audio: &str) {
    let (a, b) = (seconds / 3, 2 * seconds / 3);
    let vf = format!("noise=alls=28:allf=t:enable='between(t,{a},{b})',noise=alls=10:allf=t:enable='gte(t,{b})'");
    let mut args = os_args(&[
        "-hide_banner", "-nostdin", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", &format!("testsrc2=size=1280x720:rate=30:duration={seconds}"),
        "-f", "lavfi", "-i", &format!("sine=frequency=440:duration={seconds}"),
        "-vf", &vf, "-c:v", "libx264", "-preset", "ultrafast", "-crf", "14", "-pix_fmt", "yuv420p",
        "-c:a", audio, "-shortest",
    ]);
    args.push(path.as_os_str().to_owned());
    run_ok(ff, &args);
}

fn estimator_in(jobs: &Arc<JobManager>, dir: &Path) -> (estimate::Estimator, mpsc::Receiver<estimate::EstimateEvent>) {
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let e = estimate::Estimator::new(jobs.clone(), dir.to_path_buf(), move |ev| {
        let _ = tx.lock().unwrap().send(ev);
    });
    (e, rx)
}

/// The next estimate event `pick` takes.
fn wait_estimate<T>(rx: &mpsc::Receiver<estimate::EstimateEvent>, what: &str, pick: impl Fn(&estimate::EstimateEvent) -> Option<T>) -> T {
    loop {
        let ev = rx.recv_timeout(Duration::from_secs(240)).unwrap_or_else(|_| panic!("estimate: nothing within 240 s ({what})"));
        if let Some(t) = pick(&ev) {
            return t;
        }
    }
}

fn estimate_done(rx: &mpsc::Receiver<estimate::EstimateEvent>, path: &str) -> estimate::EstimateState {
    wait_estimate(rx, path, |ev| match ev {
        estimate::EstimateEvent::Entry(e) if e.path == path && matches!(e.state, estimate::EstimateState::Done { .. } | estimate::EstimateState::Failed { .. }) => Some(e.state.clone()),
        _ => None,
    })
}

/// The conversion itself, as the queue runs it; the size it made.
fn converted_size(ff: &ffmpeg::FfmpegInfo, prepared: convert::Prepared) -> u64 {
    let task = prepared.into_task(&|_| false).unwrap();
    let queue::Work::Attempts { attempts, total_secs } = task.work else { panic!("one run") };
    let jobs = JobManager::default();
    let (tx, rx) = mpsc::channel();
    jobs.start_attempts(Path::new(&ff.ffmpeg_path), attempts, total_secs, task.part, task.final_path, move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    loop {
        if let JobEvent::Finished(f) = rx.recv_timeout(Duration::from_secs(300)).expect("conversion") {
            assert!(f.ok, "{:?} {:?}", f.error, f.log);
            let out = PathBuf::from(f.output.unwrap());
            let size = std::fs::metadata(&out).unwrap().len();
            std::fs::remove_file(&out).unwrap();
            return size;
        }
    }
}

fn files_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
}

#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn estimate_is_close_to_the_size_the_conversion_makes() {
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx264 && ff.features.libx265 && ff.features.libsvtav1, "needs x264 · x265 · SVT-AV1");
    let dir = temp_dir("estimate");
    let scratch = dir.join("시험 인코딩");
    let long = dir.join("복잡도 바뀌는 클립 01.mp4");
    make_changing_clip(&ff, &long, 90, "aac");
    // Under 45 s: tried whole, so the estimate is the conversion's own size.
    let short = dir.join("짧은 클립 02.mkv");
    make_changing_clip(&ff, &short, 20, "libopus");
    let ffprobe = PathBuf::from(&ff.ffprobe_path);
    let report = encoders::run(&ff);
    let jobs = Arc::new(JobManager::default());
    let (estimator, rx) = estimator_in(&jobs, &scratch);

    use command::{AudioChoice, Container, Resolution, VideoCodec};
    let cases = [
        (VideoCodec::Hevc, Container::Mp4, "libx265"),
        (VideoCodec::Hevc, Container::Mp4, "hevc_videotoolbox"),
        (VideoCodec::Av1, Container::Mkv, "libsvtav1"),
    ];
    let mut measured = Vec::new();
    for (codec, container, encoder) in cases {
        let Ok(choice) = encoders::pick(&report, codec, Some(encoder)) else {
            eprintln!("{encoder}: not on this machine, skipped");
            continue;
        };
        let settings = convert_settings(codec, container, Resolution::Source, AudioChoice::CopyOrAac, false, Some(encoder), None, "_시험");
        for file in [&long, &short] {
            let path = file.display().to_string();
            let info = probe::probe(&ffprobe, file).unwrap();
            let plan = || convert::prepare(&ff, &path, &info, &settings, &choice).unwrap();
            let snap = estimator.want(vec![Ok(estimate::want_for(&path, plan()).unwrap())]);
            assert_eq!(snap.entries.len(), 1);
            let estimate::EstimateState::Done { bytes, low, high, whole, encoder: used } = estimate_done(&rx, &path) else { panic!("{encoder}: no estimate") };
            assert_eq!(used, choice.label);
            assert_eq!(estimator.bytes(&path, &estimate::key_for(&path, &plan()).unwrap()), Some(bytes), "kept for the queue");
            let actual = converted_size(&ff, plan());
            let error = bytes as f64 / actual as f64 - 1.0;
            measured.push(format!("{encoder} {}: estimate {bytes} · actual {actual} ({:+.1} %) · stretches {low}–{high}", file.file_name().unwrap().to_string_lossy(), error * 100.0));
            if file == &short {
                assert!(whole, "a 20 s file is tried whole");
                assert!(error.abs() < 0.02, "{encoder}: tried whole, {bytes} vs {actual}");
            } else {
                assert!(!whole);
                assert!(error.abs() < 0.15, "{encoder}: {bytes} vs {actual} ({:+.1} %)", error * 100.0);
                assert!(high > low * 2, "{encoder}: the clip's thirds differ ({low}–{high})");
                // Why three stretches: the clean first third alone says much less.
                let front = {
                    let out = scratch.join("front.part.mp4");
                    let prepared = plan();
                    let attempts = prepared.sample_attempts(plan::Range::new(0.0, 30.0), &out, false).unwrap();
                    run_ok(&ff, &attempts[0].args);
                    let size = std::fs::metadata(&out).unwrap().len() * 3;
                    std::fs::remove_file(&out).unwrap();
                    size
                };
                let front_error = front as f64 / actual as f64 - 1.0;
                measured.push(format!("{encoder}: front 30 s would say {front} ({:+.1} %)", front_error * 100.0));
                assert!(front_error < -0.3, "{encoder}: front 30 s {front} vs {actual}");
            }
            assert!(files_in(&scratch).is_empty(), "no test file left: {:?}", files_in(&scratch));
        }
    }
    eprintln!("{}", measured.join("\n"));
    assert!(measured.len() >= 4, "x265 and SVT-AV1 at least");
    estimator.close();
    estimate::clean(&scratch);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn estimate_pauses_for_the_queue_and_a_canceled_test_leaves_nothing() {
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx264 && ff.features.libx265, "needs x264 · x265");
    let dir = temp_dir("estimate-cancel");
    let scratch = dir.join("시험 인코딩");
    let clip = dir.join("취소할 클립 01.mov");
    make_changing_clip(&ff, &clip, 60, "aac");
    let path = clip.display().to_string();
    let info = probe::probe(Path::new(&ff.ffprobe_path), &clip).unwrap();
    let report = encoders::run(&ff);
    let choice = encoders::pick(&report, command::VideoCodec::Hevc, Some("libx265")).unwrap();
    let settings = convert_settings(
        command::VideoCodec::Hevc, command::Container::Mov, command::Resolution::Source, command::AudioChoice::CopyOrAac, true, Some("libx265"), None, "_h265",
    );
    let want = || Ok(estimate::want_for(&path, convert::prepare(&ff, &path, &info, &settings, &choice).unwrap()).unwrap());
    let jobs = Arc::new(JobManager::default());
    let (estimator, rx) = estimator_in(&jobs, &scratch);
    let running = |rx: &mpsc::Receiver<estimate::EstimateEvent>| {
        wait_estimate(rx, "running", |ev| match ev {
            estimate::EstimateEvent::Entry(e) if matches!(e.state, estimate::EstimateState::Running { percent } if percent > 0.0) => Some(()),
            _ => None,
        })
    };

    // The queue starts an item during the test: FFmpeg stops, its .part goes, the file waits.
    estimator.want(vec![want()]);
    running(&rx);
    assert_eq!(jobs.running_count(), 1);
    estimator.set_busy(true);
    assert!(jobs.wait_idle(Duration::from_secs(8)));
    assert!(files_in(&scratch).is_empty(), "{:?}", files_in(&scratch));
    let snap = estimator.want(vec![want()]);
    assert!(snap.paused);
    assert_eq!(snap.entries[0].state, estimate::EstimateState::Waiting);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(jobs.running_count(), 0, "nothing runs while the queue does");

    // The queue is done: tried again; then the list is cleared mid-test and the app quits.
    estimator.set_busy(false);
    running(&rx);
    estimator.want(Vec::new());
    assert!(jobs.wait_idle(Duration::from_secs(8)));
    assert!(files_in(&scratch).is_empty(), "{:?}", files_in(&scratch));
    assert_eq!(estimator.bytes(&path, &snap.entries[0].key), None);
    estimator.close();
    estimate::clean(&scratch);
    assert!(!scratch.exists());
    let left = files_in(&dir);
    assert_eq!(left, [clip.file_name().unwrap().to_string_lossy().into_owned()], "only the original");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---------------------------------------------------------------------------------------------
// Phase 3 완료 기준 (step 6): 20 files converted the app's way — `scan`, probe, `queue_files`,
// the estimator tied to the queue as lib.rs ties them — with cancels (running · waiting · all
// waiting), failures (source deleted · source garbled · folder read-only), restarts, moves and
// concurrency changes mixed in; then the app's exit (`close_and_clean_up`) while a test encode,
// two conversions and a waiting one are on.

/// The queue's events, every snapshot kept in order for the checks at the end.
struct Watched<D> {
    rx: mpsc::Receiver<QueueEvent<D>>,
    seen: Vec<QueueSnapshot<D>>,
}

impl<D: Clone> Watched<D> {
    fn next(&mut self, what: &str) -> QueueEvent<D> {
        let event = self.rx.recv_timeout(Duration::from_secs(180)).unwrap_or_else(|_| panic!("queue: nothing within 180 s ({what})"));
        if let QueueEvent::Changed(s) = &event {
            self.seen.push(s.clone());
        }
        event
    }

    /// The first snapshot from version `from` on where `done` holds (seen already or still to come).
    fn until(&mut self, from: u64, what: &str, done: impl Fn(&QueueSnapshot<D>) -> bool) -> QueueSnapshot<D> {
        if let Some(s) = self.seen.iter().find(|s| s.version >= from && done(s)) {
            return s.clone();
        }
        loop {
            if let QueueEvent::Changed(s) = self.next(what) {
                if s.version >= from && done(&s) {
                    return s;
                }
            }
        }
    }

    /// Until FFmpeg of item `id` reported encoded time.
    fn progress(&mut self, id: u64, what: &str) {
        loop {
            if let QueueEvent::Progress(p) = self.next(what) {
                if p.id == id && p.progress.out_time > 0.0 {
                    return;
                }
            }
        }
    }
}

fn item_of<D>(s: &QueueSnapshot<D>, id: u64) -> &godseon_clip_lib::queue::QueueItem<D> {
    s.items.iter().find(|i| i.id == id).unwrap_or_else(|| panic!("item {id} in the queue"))
}

fn all_finished<D>(s: &QueueSnapshot<D>) -> bool {
    s.items.iter().all(|i| i.state.finished())
}

/// testsrc2 at `size`, 30 fps, `secs` long, a keyframe every second; AAC unless `audio` is false.
fn make_batch_clip(ff: &ffmpeg::FfmpegInfo, path: &Path, size: &str, secs: u32, audio: bool) {
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y"]);
    args.extend(os_args(&["-f", "lavfi", "-i", &format!("testsrc2=size={size}:rate=30:duration={secs}")]));
    if audio {
        args.extend(os_args(&["-f", "lavfi", "-i", &format!("sine=frequency=440:duration={secs}")]));
    }
    args.extend(os_args(&["-c:v", "libx264", "-preset", "ultrafast", "-g", "30", "-pix_fmt", "yuv420p"]));
    if audio {
        args.extend(os_args(&["-c:a", "aac", "-shortest"]));
    }
    args.push(path.as_os_str().to_owned());
    run_ok(ff, &args);
}

/// What decoding the whole file says (empty: it plays from start to end without an error).
fn decode_errors(ff: &ffmpeg::FfmpegInfo, file: &Path) -> String {
    let mut args = os_args(&["-hide_banner", "-nostdin", "-v", "error", "-i"]);
    args.push(file.as_os_str().to_owned());
    args.extend(os_args(&["-f", "null", "-"]));
    let out = util::run_capture(Path::new(&ff.ffmpeg_path), &args, Duration::from_secs(120)).unwrap();
    if out.success { out.stderr.trim().to_string() } else { format!("decoding failed: {}", out.stderr) }
}

/// ffprobe's one-value-per-line answer for the streams `select` picks.
fn probe_lines(ff: &ffmpeg::FfmpegInfo, file: &Path, select: &str, extra: &[&str], entries: &str) -> Vec<String> {
    let mut args = os_args(&["-v", "error", "-select_streams", select]);
    args.extend(os_args(extra));
    args.extend(os_args(&["-show_entries", entries, "-of", "default=nw=1:nk=1"]));
    args.push(file.as_os_str().to_owned());
    let out = util::run_capture(Path::new(&ff.ffprobe_path), &args, Duration::from_secs(60)).unwrap();
    assert!(out.success, "ffprobe failed: {}", out.stderr);
    out.stdout.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// 변환 시작 as lib.rs `queue_convert` does it, the 예상 크기 measured so far attached.
fn queue_batch(
    ff: &ffmpeg::FfmpegInfo,
    queue: &Queue<ConvertTask>,
    estimator: &estimate::Estimator,
    choice: &encoders::EncoderChoice,
    paths: &[PathBuf],
    settings: ConvertSettings,
) -> convert::ConvertQueued {
    convert::check_settings(&settings).unwrap();
    let req = ConvertRequest { paths: paths.iter().map(|p| p.display().to_string()).collect(), settings };
    let ffprobe = PathBuf::from(&ff.ffprobe_path);
    let estimated = |path: &str, p: &convert::Prepared| estimate::key_for(path, p).and_then(|k| estimator.bytes(path, &k));
    convert::queue_files(queue, ff, &req, choice, |p| probe::probe(&ffprobe, Path::new(p)), estimated, |t| t)
}

/// Until `dir` holds a test encode's `.part` (or not), within `secs`.
fn wait_for_part(dir: &Path, present: bool, secs: u64) {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while files_in(dir).iter().any(|n| n.contains(".part.")) != present {
        assert!(std::time::Instant::now() < deadline, "{}: .part {} within {secs} s: {:?}", dir.display(), if present { "made" } else { "gone" }, files_in(dir));
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn twenty_conversions_through_cancels_and_failures_end_right_and_leave_nothing() {
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx264 && ff.features.libx265 && ff.features.libsvtav1, "needs x264 · x265 · SVT-AV1");
    let started = std::time::Instant::now();
    let dir = temp_dir("일괄 변환 20개");
    let sources = dir.join("원본 모음");
    let out_dir = dir.join("변환 결과");
    let locked = dir.join("잠긴 폴더");
    let later = dir.join("나중에 고른 파일");
    let scratch = dir.join("앱 캐시").join("estimates");
    for d in [sources.join("1일차"), sources.join("2일차"), out_dir.clone(), locked.clone(), later.clone()] {
        std::fs::create_dir_all(d).unwrap();
    }
    const SMALL: &str = "640x360";
    // (path, size, seconds, audio). Two long 720p encodes are slow enough on x265 to cancel.
    let clips: Vec<(PathBuf, &str, u32, bool)> = vec![
        (sources.join("가을 축제 A.mp4"), "1280x720", 30, true),
        (sources.join("가을 축제 B.mp4"), "1280x720", 30, true),
        (sources.join("가족 여행 01.mp4"), SMALL, 3, true),
        (sources.join("가족 여행 02.mp4"), SMALL, 3, true),
        (sources.join("가족 여행 03.mp4"), SMALL, 3, true),
        (sources.join("가족 여행 04.mp4"), SMALL, 3, true),
        (sources.join("대기 취소될 클립.mp4"), SMALL, 3, true),
        (sources.join("망가질 원본.mp4"), SMALL, 3, true),
        (sources.join("무음 타임랩스.mkv"), SMALL, 3, false),
        (sources.join("무음 화면 녹화.mp4"), SMALL, 3, false),
        (sources.join("이미 변환됨.mp4"), SMALL, 3, true),
        (sources.join("지워질 원본.mp4"), SMALL, 3, true),
        (sources.join("회의 녹화 (최종).mov"), SMALL, 3, true),
        (sources.join("1일차").join("해변 산책.mp4"), SMALL, 2, true),
        (sources.join("2일차").join("해변 산책.mp4"), SMALL, 4, true),
        (locked.join("드론 영상 1.mp4"), SMALL, 3, true),
        (locked.join("드론 영상 2.mp4"), SMALL, 3, true),
        (later.join("그대로 둘 클립 1.mp4"), SMALL, 3, true),
        (later.join("그대로 둘 클립 2.mp4"), SMALL, 3, true),
        (later.join("AV1로 바꿀 클립.mp4"), SMALL, 3, true),
    ];
    for (path, size, secs, audio) in &clips {
        make_batch_clip(&ff, path, size, *secs, *audio);
    }
    // The user's own file where the first result would go: never written over.
    let mine = out_dir.join("이미 변환됨_h265.mp4");
    std::fs::write(&mine, "내가 만든 파일").unwrap();
    let clip_of = |source: &str| clips.iter().find(|c| c.0.display().to_string() == source).unwrap_or_else(|| panic!("{source}"));
    let made_in = started.elapsed();

    // The convert screen: the folder is added (subfolders too) and the list probed.
    let listed = convert::scan(&[sources.clone()], convert::SCAN_LIMIT);
    let rel: Vec<String> = listed.files.iter().map(|f| Path::new(f).strip_prefix(&sources).unwrap().to_string_lossy().replace('\\', "/")).collect();
    assert_eq!(
        rel,
        [
            "가을 축제 A.mp4", "가을 축제 B.mp4", "가족 여행 01.mp4", "가족 여행 02.mp4", "가족 여행 03.mp4", "가족 여행 04.mp4",
            "대기 취소될 클립.mp4", "망가질 원본.mp4", "무음 타임랩스.mkv", "무음 화면 녹화.mp4", "이미 변환됨.mp4", "지워질 원본.mp4",
            "회의 녹화 (최종).mov", "1일차/해변 산책.mp4", "2일차/해변 산책.mp4",
        ],
        "{:?}",
        listed.notices
    );
    let listed: Vec<PathBuf> = listed.files.iter().map(PathBuf::from).collect();

    use command::{AudioChoice, Container, Resolution, VideoCodec};
    let report = encoders::run(&ff);
    let x265 = encoders::pick(&report, VideoCodec::Hevc, Some("libx265")).unwrap();
    let av1 = encoders::pick(&report, VideoCodec::Av1, None).unwrap();
    let to_folder = convert_settings(VideoCodec::Hevc, Container::Mp4, Resolution::Source, AudioChoice::CopyOrAac, false, Some("libx265"), Some(&out_dir), "_h265");
    let next_to = convert_settings(VideoCodec::Hevc, Container::Mp4, Resolution::Source, AudioChoice::CopyOrAac, false, Some("libx265"), None, "_h265");
    let as_av1 = convert_settings(VideoCodec::Av1, Container::Mkv, Resolution::P720, AudioChoice::CopyOrAac, false, None, None, "_av1");

    // The queue and the test encodes, tied as lib.rs ties them (emit_queue_event → set_busy).
    let jobs = Arc::new(JobManager::default());
    let (estimator, est_rx) = estimator_in(&jobs, &scratch);
    let (tx, rx) = mpsc::channel();
    let est = estimator.clone();
    let queue = Queue::new(jobs.clone(), 1, move |e: QueueEvent<ConvertTask>| {
        if let QueueEvent::Changed(s) = &e {
            est.set_busy(s.items.iter().any(|i| i.state == ItemState::Running));
        }
        let _ = tx.send(e);
    });
    let mut watch = Watched { rx, seen: Vec::new() };
    let version = |q: &Queue<ConvertTask>| q.snapshot().version;

    // 예상 크기 of the list starts first (the queue is idle): the first file is being tried.
    let ffprobe = PathBuf::from(&ff.ffprobe_path);
    let wants = listed
        .iter()
        .map(|p| {
            let path = p.display().to_string();
            let info = probe::probe(&ffprobe, p).unwrap();
            Ok(estimate::want_for(&path, convert::prepare(&ff, &path, &info, &to_folder, &x265).unwrap()).unwrap())
        })
        .collect();
    estimator.want(wants);
    let first = listed[0].display().to_string();
    wait_estimate(&est_rx, "first test encode", |ev| match ev {
        estimate::EstimateEvent::Entry(e) if e.path == first && matches!(e.state, estimate::EstimateState::Running { percent } if percent > 0.0) => Some(()),
        _ => None,
    });
    wait_for_part(&scratch, true, 10);

    // ---- 1. 변환 시작 (15), one at a time; failures set up once they are queued.
    let phase1 = std::time::Instant::now();
    let a = queue_batch(&ff, &queue, &estimator, &x265, &listed, to_folder.clone());
    assert_eq!((a.queued.len(), a.refused.len()), (15, 0), "{:?}", a.refused);
    let id = |q: &convert::ConvertQueued, name: &str| -> u64 {
        q.queued.iter().find(|f| Path::new(&f.path).ends_with(name)).unwrap_or_else(|| panic!("{name} queued")).id
    };
    let s = queue.snapshot();
    let output_name = |s: &QueueSnapshot<ConvertTask>, id: u64| Path::new(&item_of(s, id).output).file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(output_name(&s, id(&a, "1일차/해변 산책.mp4")), "해변 산책_h265.mp4");
    assert_eq!(output_name(&s, id(&a, "2일차/해변 산책.mp4")), "해변 산책_h265 (1).mp4", "same name from two folders into one");
    assert_eq!(output_name(&s, id(&a, "이미 변환됨.mp4")), "이미 변환됨_h265 (1).mp4", "a file of that name is there already");
    assert!(s.items.iter().all(|i| Path::new(&i.output).parent() == Some(out_dir.as_path())));
    assert_eq!(queue.active(), (1, 14));
    // The queue took the CPU: the test encode stopped and its .part went.
    wait_estimate(&est_rx, "paused for the queue", |ev| (*ev == estimate::EstimateEvent::Paused(true)).then_some(()));
    wait_for_part(&scratch, false, 8);
    std::fs::remove_file(sources.join("지워질 원본.mp4")).unwrap();
    std::fs::write(sources.join("망가질 원본.mp4"), vec![0x5a_u8; 64 * 1024]).unwrap();

    let (long_a, long_b) = (id(&a, "가을 축제 A.mp4"), id(&a, "가을 축제 B.mp4"));
    queue.cancel(id(&a, "대기 취소될 클립.mp4")).unwrap();
    queue.move_to(id(&a, "회의 녹화 (최종).mov"), 0).unwrap();
    queue.move_to(id(&a, "무음 화면 녹화.mp4"), 1).unwrap();
    let names_in = |s: &QueueSnapshot<ConvertTask>, state: ItemState| -> Vec<String> {
        s.items.iter().filter(|i| i.state == state).map(|i| i.info.source_name.clone()).collect()
    };
    assert_eq!(
        names_in(&queue.snapshot(), ItemState::Waiting),
        [
            "회의 녹화 (최종).mov", "무음 화면 녹화.mp4", "가을 축제 B.mp4", "가족 여행 01.mp4", "가족 여행 02.mp4", "가족 여행 03.mp4",
            "가족 여행 04.mp4", "망가질 원본.mp4", "무음 타임랩스.mkv", "이미 변환됨.mp4", "지워질 원본.mp4", "해변 산책.mp4", "해변 산책.mp4",
        ]
    );
    watch.progress(long_a, "first long encode");
    assert!(leftovers(&out_dir).iter().any(|n| n.starts_with("가을 축제 A_h265.part")), "{:?}", leftovers(&out_dir));
    let v = version(&queue);
    assert_eq!(queue.set_concurrency(4), 4);
    let s = watch.until(v, "four at once", |s| s.concurrency == 4);
    assert_eq!(names_in(&s, ItemState::Running), ["가을 축제 A.mp4", "회의 녹화 (최종).mov", "무음 화면 녹화.mp4", "가을 축제 B.mp4"]);
    watch.progress(long_b, "second long encode");
    let v = version(&queue);
    queue.cancel(long_a).unwrap();
    queue.cancel(long_b).unwrap();
    watch.until(v, "both canceling", |s| item_of(s, long_a).canceling || item_of(s, long_a).state == ItemState::Canceled);
    let v = version(&queue);
    assert_eq!(queue.set_concurrency(2), 2);
    let lowered = watch.until(v, "two at once", |s| s.concurrency == 2).version;
    let s = watch.until(lowered, "the first batch", all_finished);
    let phase1 = phase1.elapsed();
    let state_of = |s: &QueueSnapshot<ConvertTask>, name: &str| item_of(s, id(&a, name)).state;
    for (name, want) in [
        ("가을 축제 A.mp4", ItemState::Canceled),
        ("가을 축제 B.mp4", ItemState::Canceled),
        ("대기 취소될 클립.mp4", ItemState::Canceled),
        ("망가질 원본.mp4", ItemState::Failed),
        ("지워질 원본.mp4", ItemState::Failed),
    ] {
        assert_eq!(state_of(&s, name), want, "{name}: {:?}", item_of(&s, id(&a, name)));
    }
    assert_eq!(names_in(&s, ItemState::Done).len(), 10);
    assert!(item_of(&s, id(&a, "대기 취소될 클립.mp4")).started_at.is_none(), "canceled while waiting: never ran");
    // Waiting items ran in queue order (moved ones first), never more at once than allowed.
    let mut ran: Vec<u64> = Vec::new();
    let mut before: Option<&QueueSnapshot<ConvertTask>> = None;
    let mut most = 0;
    for snap in &watch.seen {
        let running: Vec<u64> = snap.items.iter().filter(|i| i.state == ItemState::Running).map(|i| i.id).collect();
        let was = |id: &u64| before.is_some_and(|b| b.items.iter().any(|i| i.id == *id && i.state == ItemState::Running));
        if running.iter().any(|i| !was(i)) {
            assert!(running.len() <= snap.concurrency, "v{}: started an item with {} running of {}", snap.version, running.len(), snap.concurrency);
        }
        let new: Vec<u64> = running.iter().copied().filter(|i| !ran.contains(i)).collect();
        ran.extend(new);
        most = most.max(running.len());
        before = Some(snap);
    }
    assert_eq!(most, 4);
    let ran: Vec<String> = ran.iter().map(|i| Path::new(&item_of(&s, *i).info.source).strip_prefix(&sources).unwrap().to_string_lossy().replace('\\', "/")).collect();
    assert_eq!(
        ran,
        [
            "가을 축제 A.mp4", "회의 녹화 (최종).mov", "무음 화면 녹화.mp4", "가을 축제 B.mp4", "가족 여행 01.mp4", "가족 여행 02.mp4",
            "가족 여행 03.mp4", "가족 여행 04.mp4", "망가질 원본.mp4", "무음 타임랩스.mkv", "이미 변환됨.mp4", "지워질 원본.mp4",
            "1일차/해변 산책.mp4", "2일차/해변 산책.mp4",
        ]
    );
    for d in [&sources, &out_dir] {
        assert!(leftovers(d).is_empty(), "{:?}", leftovers(d));
    }
    // The queue is idle: the test encodes go on.
    wait_estimate(&est_rx, "test encodes again", |ev| (*ev == estimate::EstimateEvent::Paused(false)).then_some(()));

    // ---- 2. One at a time again: B restarted; three more files queued and all waiting canceled;
    //         two into a folder made read-only after they were queued; one of the three restarted.
    let phase2 = std::time::Instant::now();
    assert_eq!(queue.set_concurrency(1), 1);
    let v = version(&queue);
    queue.restart(long_b).unwrap();
    let s = watch.until(v, "B again", |s| item_of(s, long_b).state == ItemState::Running);
    assert_eq!(s.items.last().unwrap().id, long_b, "a restarted item goes to the end");
    let c = queue_batch(&ff, &queue, &estimator, &av1, &[later.join("그대로 둘 클립 1.mp4"), later.join("그대로 둘 클립 2.mp4"), later.join("AV1로 바꿀 클립.mp4")], as_av1);
    assert_eq!(c.queued.len(), 3, "{:?}", c.refused);
    assert_eq!(queue.cancel_waiting(), 3);
    let s = queue.snapshot();
    for f in &c.queued {
        let i = item_of(&s, f.id);
        assert!(i.state == ItemState::Canceled && i.started_at.is_none(), "{i:?}");
    }
    let b = queue_batch(&ff, &queue, &estimator, &x265, &[locked.join("드론 영상 1.mp4"), locked.join("드론 영상 2.mp4")], next_to);
    assert_eq!(b.queued.len(), 2, "{:?}", b.refused);
    // A share or drive that became read-only (on Windows the attribute does not stop writing).
    let lock_works = !cfg!(windows);
    let open_permissions = std::fs::metadata(&locked).unwrap().permissions();
    let mut read_only = open_permissions.clone();
    read_only.set_readonly(true);
    std::fs::set_permissions(&locked, read_only).unwrap();
    let av1_clip = id(&c, "AV1로 바꿀 클립.mp4");
    queue.restart(av1_clip).unwrap();
    assert_eq!(names_in(&queue.snapshot(), ItemState::Waiting), ["드론 영상 1.mp4", "드론 영상 2.mp4", "AV1로 바꿀 클립.mp4"]);
    let v = version(&queue);
    let s = watch.until(v, "B, the read-only folder, AV1", all_finished);
    std::fs::set_permissions(&locked, open_permissions).unwrap();
    assert_eq!(item_of(&s, long_b).state, ItemState::Done, "{:?}", item_of(&s, long_b));
    assert_eq!(item_of(&s, av1_clip).state, ItemState::Done, "{:?}", item_of(&s, av1_clip));
    let (drone1, drone2) = (id(&b, "드론 영상 1.mp4"), id(&b, "드론 영상 2.mp4"));
    if lock_works {
        for drone in [drone1, drone2] {
            let i = item_of(&s, drone);
            assert_eq!(i.state, ItemState::Failed, "{i:?}");
            assert!(matches!(&i.error, Some(Msg::Raw { text }) if text.contains("Permission denied")), "{:?} {:?}", i.error, i.log);
        }
        // Writable again: run again, under the same name.
        let v = version(&queue);
        queue.restart(drone1).unwrap();
        queue.restart(id(&a, "지워질 원본.mp4")).unwrap();
        watch.until(v, "restarted after the fix", all_finished);
    }
    let s = watch.until(version(&queue), "phase 2 done", all_finished);
    let phase2 = phase2.elapsed();

    // The 20: each state, its file, and the finish order the queue kept.
    let expected: Vec<(u64, ItemState)> = vec![
        (long_a, ItemState::Canceled),
        (long_b, ItemState::Done),
        (id(&a, "가족 여행 01.mp4"), ItemState::Done),
        (id(&a, "가족 여행 02.mp4"), ItemState::Done),
        (id(&a, "가족 여행 03.mp4"), ItemState::Done),
        (id(&a, "가족 여행 04.mp4"), ItemState::Done),
        (id(&a, "대기 취소될 클립.mp4"), ItemState::Canceled),
        (id(&a, "망가질 원본.mp4"), ItemState::Failed),
        (id(&a, "무음 타임랩스.mkv"), ItemState::Done),
        (id(&a, "무음 화면 녹화.mp4"), ItemState::Done),
        (id(&a, "이미 변환됨.mp4"), ItemState::Done),
        (id(&a, "지워질 원본.mp4"), ItemState::Failed),
        (id(&a, "회의 녹화 (최종).mov"), ItemState::Done),
        (id(&a, "1일차/해변 산책.mp4"), ItemState::Done),
        (id(&a, "2일차/해변 산책.mp4"), ItemState::Done),
        (drone1, ItemState::Done),
        (drone2, if lock_works { ItemState::Failed } else { ItemState::Done }),
        (id(&c, "그대로 둘 클립 1.mp4"), ItemState::Canceled),
        (id(&c, "그대로 둘 클립 2.mp4"), ItemState::Canceled),
        (av1_clip, ItemState::Done),
    ];
    assert_eq!(s.items.len(), 20);
    let count = |want: ItemState| s.items.iter().filter(|i| i.state == want).count();
    let failed = if lock_works { 3 } else { 2 };
    assert_eq!((count(ItemState::Done), count(ItemState::Failed), count(ItemState::Canceled)), (16 - failed, failed, 4));
    for (id, want) in &expected {
        let i = item_of(&s, *id);
        assert_eq!(i.state, *want, "{}: {:?} {:?}", i.info.source, i.error, i.log);
        let out = Path::new(&i.output);
        if *want != ItemState::Done {
            assert!(!out.exists(), "{} must not exist", out.display());
            assert!(i.output_bytes.is_none() && i.progress.as_ref().is_none_or(|p| p.percent < 100.0), "{i:?}");
            continue;
        }
        let (source, _, secs, audio) = clip_of(&i.info.source);
        assert_eq!(i.output_bytes, Some(std::fs::metadata(out).unwrap().len()), "{}", out.display());
        assert_eq!(decode_errors(&ff, out), "", "{} plays to the end", out.display());
        let av1_out = *id == av1_clip;
        let codec = probe_lines(&ff, out, "v:0", &[], "stream=codec_name,codec_tag_string");
        assert_eq!(codec, if av1_out { ["av1", "[0][0][0][0]"] } else { ["hevc", "hvc1"] }, "{}", out.display());
        let frames = probe_lines(&ff, out, "v:0", &["-count_packets"], "stream=nb_read_packets");
        assert_eq!(frames, [(secs * 30).to_string()], "{}: every frame", out.display());
        let tracks = probe_lines(&ff, out, "a", &[], "stream=codec_name");
        assert_eq!(tracks, if *audio { vec!["aac".to_string()] } else { vec![] }, "{}", out.display());
        let length = probe::probe(&ffprobe, out).unwrap().duration;
        assert!((length - f64::from(*secs)).abs() <= 1.0 / 30.0 + 0.03, "{}: {length}", out.display());
        assert_ne!(out, source.as_path());
    }
    let error_of = |id: u64| match &item_of(&s, id).error {
        Some(Msg::Raw { text }) => text.clone(),
        other => format!("{other:?}"),
    };
    assert!(error_of(id(&a, "지워질 원본.mp4")).contains("No such file"), "{}", error_of(id(&a, "지워질 원본.mp4")));
    assert!(error_of(id(&a, "망가질 원본.mp4")).contains("Invalid data"), "{}", error_of(id(&a, "망가질 원본.mp4")));
    assert_eq!(std::fs::read_to_string(&mine).unwrap(), "내가 만든 파일", "the user's file is as it was");
    // finishedOrder: every item has one, no two alike, and it follows when each item last finished.
    let mut last_finish: Vec<u64> = Vec::new();
    let mut known: std::collections::HashMap<u64, Option<u64>> = std::collections::HashMap::new();
    for snap in &watch.seen {
        for i in &snap.items {
            if i.state.finished() && known.get(&i.id) != Some(&i.finished_order) {
                last_finish.retain(|x| *x != i.id);
                last_finish.push(i.id);
            }
            known.insert(i.id, i.finished_order);
        }
    }
    let mut by_order: Vec<(u64, u64)> = s.items.iter().map(|i| (i.finished_order.expect("finished order"), i.id)).collect();
    by_order.sort_unstable();
    by_order.dedup_by_key(|x| x.0);
    assert_eq!(by_order.len(), 20, "no two items share a finish order");
    assert_eq!(by_order.iter().map(|x| x.1).collect::<Vec<_>>(), last_finish, "finish order = the order items ended in");
    for d in [&sources, &out_dir, &locked, &later] {
        assert!(leftovers(d).is_empty(), "{}: {:?}", d.display(), leftovers(d));
    }

    // ---- 3. Quit: a test encode is being written once the queue is idle; then two conversions run
    //         and one waits (their names skip the ones the record holds), and the app exits.
    wait_for_part(&scratch, true, 60);
    assert_eq!(queue.set_concurrency(2), 2);
    let d = queue_batch(&ff, &queue, &estimator, &x265, &[listed[0].clone(), listed[1].clone(), listed[2].clone()], to_folder.clone());
    assert_eq!(d.queued.len(), 3, "{:?}", d.refused);
    let now = queue.snapshot();
    let names: Vec<String> = d.queued.iter().map(|f| output_name(&now, f.id)).collect();
    assert_eq!(names, ["가을 축제 A_h265 (1).mp4", "가을 축제 B_h265 (1).mp4", "가족 여행 01_h265 (1).mp4"], "the canceled A keeps its name for 다시 시작");
    watch.progress(d.queued[0].id, "quit: first encode");
    watch.progress(d.queued[1].id, "quit: second encode");
    assert_eq!(queue.active(), (2, 1));
    assert!(leftovers(&out_dir).len() >= 2, "{:?}", leftovers(&out_dir));
    let quit = std::time::Instant::now();
    assert!(godseon_clip_lib::close_and_clean_up(&queue, &estimator, &jobs, &scratch, godseon_clip_lib::EXIT_WAIT), "every job ends within the exit wait");
    let quit = quit.elapsed();
    let end = watch.until(0, "after quit", |snap| snap.items.len() == 23 && all_finished(snap));
    for f in &d.queued {
        assert_eq!(item_of(&end, f.id).state, ItemState::Canceled, "{:?}", item_of(&end, f.id));
    }
    assert!(item_of(&end, d.queued[2].id).started_at.is_none());
    for i in &s.items {
        let now = item_of(&end, i.id);
        assert_eq!((now.state, now.finished_order, &now.output), (i.state, i.finished_order, &i.output), "quitting leaves the record as it was");
    }
    assert!(!scratch.exists(), "the test encodes' folder is gone: {:?}", files_in(&scratch));
    for d in [&sources, &out_dir, &locked, &later] {
        assert!(leftovers(d).is_empty(), "{}: {:?}", d.display(), leftovers(d));
    }
    assert_eq!(jobs.running_count(), 0);
    assert_eq!(queue.restart(long_a), Err(Msg::QueueClosed));
    let refused = queue_batch(&ff, &queue, &estimator, &x265, &[listed[2].clone()], to_folder);
    assert_eq!(refused.refused.iter().map(|r| r.error.clone()).collect::<Vec<_>>(), [Msg::QueueClosed]);
    let mut results = files_in(&out_dir);
    results.sort();
    let mut want: Vec<String> = [
        "가을 축제 B_h265.mp4", "가족 여행 01_h265.mp4", "가족 여행 02_h265.mp4", "가족 여행 03_h265.mp4", "가족 여행 04_h265.mp4",
        "무음 타임랩스_h265.mp4", "무음 화면 녹화_h265.mp4", "이미 변환됨_h265.mp4", "이미 변환됨_h265 (1).mp4", "회의 녹화 (최종)_h265.mp4",
        "해변 산책_h265.mp4", "해변 산책_h265 (1).mp4",
    ]
    .iter()
    .map(|n| n.to_string())
    .collect();
    want.sort();
    assert_eq!(results, want, "only the results and the user's file");
    eprintln!(
        "20 conversions: clips {:.1} s · batch 1 (15, concurrency 1 → 4 → 2) {:.1} s · batch 2 + restarts {:.1} s · quit {:.2} s · all {:.1} s",
        made_in.as_secs_f64(),
        phase1.as_secs_f64(),
        phase2.as_secs_f64(),
        quit.as_secs_f64(),
        started.elapsed().as_secs_f64()
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// --- 화면 효과 (Phase 4 step 1): crop · turn · flip · size ---

/// The block clip's pictures are 32-pixel squares, each its own color.
const BLOCK: f64 = 32.0;

fn block_color(bx: usize, by: usize) -> [u8; 3] {
    [(40 + (bx * 47 + by * 13) % 180) as u8, (40 + (by * 59 + bx * 7) % 180) as u8, (40 + ((bx + by) * 83) % 180) as u8]
}

/// The 640 × 480 block picture as a PPM.
fn block_ppm() -> Vec<u8> {
    let (w, h) = (640usize, 480usize);
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    for y in 0..h {
        for x in 0..w {
            ppm.extend(block_color(x / BLOCK as usize, y / BLOCK as usize));
        }
    }
    ppm
}

/// 2 s of one 640 × 480 still made of colored blocks (lossless H.264, 4:2:0), so where each pixel
/// of a result came from can be checked. `rotation`: a display rotation as a phone writes it.
fn make_block_clip(ff: &ffmpeg::FfmpegInfo, path: &Path, rotation: Option<i32>) {
    let still = path.with_extension("ppm");
    std::fs::write(&still, block_ppm()).unwrap();
    let coded = if rotation.is_some() { path.with_extension("coded.mp4") } else { path.to_path_buf() };
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-loop", "1", "-framerate", "30", "-i"]);
    args.push(still.as_os_str().to_owned());
    args.extend(os_args(&["-t", "2", "-c:v", "libx264", "-qp", "0", "-pix_fmt", "yuv420p", "-g", "30"]));
    args.push(coded.as_os_str().to_owned());
    run_ok(ff, &args);
    if let Some(r) = rotation {
        let mut remux = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-display_rotation", &r.to_string(), "-i"]);
        remux.push(coded.as_os_str().to_owned());
        remux.extend(os_args(&["-c", "copy"]));
        remux.push(path.as_os_str().to_owned());
        run_ok(ff, &remux);
        std::fs::remove_file(&coded).unwrap();
    }
    std::fs::remove_file(&still).unwrap();
}

/// What the app would run for this export (prepare → queue names → the first attempt), run to the end.
fn export_with_effects(
    ff: &ffmpeg::FfmpegInfo,
    source: &Path,
    edit: Edit,
    codec: command::OutputCodec,
    e: effects::Effects,
    report: impl FnOnce() -> encoders::EncoderReport,
) -> (PathBuf, export::ExportSummary, Option<effects::EffectsPlan>) {
    let (info, kf) = opened(ff, source);
    let planned = effects::plan(info.video.as_ref().unwrap(), &e).unwrap();
    let req = ExportRequest {
        path: source.display().to_string(),
        edit,
        mode: ExportMode::Precise,
        codec,
        quality: command::Quality::High,
        effects: e,
        frame_rate: None,
        gif_short: None,
    };
    let prepared = export::prepare(ff, &info, &kf, &req, report).unwrap();
    let summary = prepared.summary().clone();
    let task = prepared.into_task(&|_| false).unwrap();
    // GODSEON_SHOW_ARGS=1 prints the FFmpeg command line (to rerun or vary a failing export by hand).
    if std::env::var("GODSEON_SHOW_ARGS").is_ok() {
        eprintln!("{}", task.command_line);
    }
    let queue::Work::Attempts { mut attempts, total_secs } = task.work else { panic!("one encode") };
    attempts.truncate(1);
    let finished = run_attempts(ff, attempts, total_secs, &task.part, &task.final_path);
    assert!(finished.ok, "{}: {:?}", source.display(), finished.error);
    assert!(!task.part.exists());
    (task.final_path, summary, planned)
}

fn software_report() -> encoders::EncoderReport {
    let test = |name: &str, codec: &str| encoders::EncoderTest { name: name.into(), codec: codec.into(), hardware: false, ok: true, millis: 1, error: None };
    encoders::EncoderReport { tests: vec![test("libx264", "h264"), test("libx265", "hevc")], preferred_hevc: Some("libx265".into()), preferred_av1: None }
}

/// Where the result's pixel (`ox`, `oy`) comes from in the upright source, by the plan's numbers —
/// the same picture the editor draws the box on. None inside 맞춤's bars.
fn source_point(plan: &effects::EffectsPlan, src: (u32, u32), ox: f64, oy: f64) -> Option<(f64, f64)> {
    let crop = plan.crop.unwrap_or(effects::Rect { x: 0, y: 0, width: src.0, height: src.1 });
    let quarter = plan.turn % 2 == 1;
    let (cw, ch) = if quarter { (crop.height as f64, crop.width as f64) } else { (crop.width as f64, crop.height as f64) };
    let [w, h] = plan.picture_size(src);
    let (px, py) = plan.pad.map_or((0.0, 0.0), |p| (p.x as f64, p.y as f64));
    let (pw, ph) = (w as f64, h as f64);
    if ox < px || oy < py || ox >= px + pw || oy >= py + ph {
        return None;
    }
    let mut sx = (ox - px) * cw / pw;
    let mut sy = (oy - py) * ch / ph;
    if plan.flip_h {
        sx = cw - sx;
    }
    if plan.flip_v {
        sy = ch - sy;
    }
    let (wc, hc) = (crop.width as f64, crop.height as f64);
    let (x, y) = match plan.turn {
        1 => (sy, hc - sx),
        3 => (wc - sy, sx),
        _ => (sx, sy),
    };
    Some((crop.x as f64 + x, crop.y as f64 + y))
}

/// Every 4th pixel of the result against the source pixel the plan says it shows (away from the
/// blocks' edges, where scaling and 4:2:0 blend neighbours), and 맞춤's bars black. Returns how many
/// pixels were compared and the largest difference.
fn assert_shows_the_drawn_area(label: &str, source_rgb: &[u8], src: (u32, u32), out_rgb: &[u8], plan: &effects::EffectsPlan) -> (usize, u8) {
    let [w, h] = plan.size;
    assert_eq!(out_rgb.len(), (w * h * 3) as usize, "{label}: the result is {w} × {h}");
    let px = |rgb: &[u8], width: u32, x: u32, y: u32| -> [u8; 3] {
        let i = ((y * width + x) * 3) as usize;
        [rgb[i], rgb[i + 1], rgb[i + 2]]
    };
    let (cw, pw) = match (plan.scale, plan.crop) {
        (Some([sw, _]), Some(c)) => (if plan.turn % 2 == 1 { c.height } else { c.width } as f64, sw as f64),
        (Some([sw, _]), None) => (if plan.turn % 2 == 1 { src.1 } else { src.0 } as f64, sw as f64),
        _ => (1.0, 1.0),
    };
    let margin = (3.0 * cw / pw).max(6.0);
    let (mut compared, mut worst, mut bars) = (0, 0u8, 0);
    for oy in (0..h).step_by(4) {
        for ox in (0..w).step_by(4) {
            let got = px(out_rgb, w, ox, oy);
            let Some((sx, sy)) = source_point(plan, src, ox as f64 + 0.5, oy as f64 + 0.5) else {
                // A bar: black, away from the picture's edge.
                let p = plan.pad.unwrap();
                let [iw, ih] = plan.picture_size(src);
                let far = |o: u32, start: u32, len: u32| o + 4 < start || o >= start + len + 4;
                if far(ox, p.x, iw) || far(oy, p.y, ih) {
                    assert!(got.iter().all(|c| *c < 24), "{label}: bar at ({ox}, {oy}) is {got:?}");
                    bars += 1;
                }
                continue;
            };
            let (bx, by) = (sx % BLOCK, sy % BLOCK);
            if bx < margin || bx > BLOCK - margin || by < margin || by > BLOCK - margin {
                continue;
            }
            let want = px(source_rgb, src.0, sx as u32, sy as u32);
            let diff = got.iter().zip(want).map(|(a, b)| a.abs_diff(b)).max().unwrap();
            assert!(diff <= 24, "{label}: ({ox}, {oy}) shows {got:?}, the source at ({sx:.1}, {sy:.1}) is {want:?}");
            worst = worst.max(diff);
            compared += 1;
        }
    }
    assert!(compared > 200, "{label}: only {compared} pixels compared");
    if plan.pad.is_some() {
        assert!(bars > 50, "{label}: only {bars} bar pixels");
    }
    (compared, worst)
}

/// The promise of 화면 효과: the export shows exactly the box drawn on the picture, turned and
/// sized as chosen — the result's pixels are the source's at the place the plan (the same numbers
/// the editor draws with) says. Crop alone, crop + turn + mirror + 1080p-style preset, 맞춤 with bars,
/// 채움 with a half turn, a phone video stored sideways (box in the upright picture), and 구간 제거.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn effects_export_shows_exactly_the_drawn_area() {
    use effects::{Crop, Effects, Fit, OutputSize};
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx264, "this test needs libx264");
    let dir = temp_dir("effects-area");
    let flat = dir.join("색 블록 원본.mp4");
    make_block_clip(&ff, &flat, None);
    let phone = dir.join("세로로 찍은 폰 영상.mp4");
    make_block_clip(&ff, &phone, Some(90));
    let crop = |x, y, width, height, ratio| Some(Crop { x, y, width, height, ratio });
    let trim = || Edit::Trim { start: 0.5, end: 1.5 };
    let cases: Vec<(&str, &Path, Edit, Effects)> = vec![
        ("크롭만 (홀수 → 짝수)", &flat, trim(), Effects { crop: crop(97, 65, 321, 257, None), ..Effects::default() }),
        (
            "크롭 + 오른쪽 90° + 좌우 + 짧은 변 192",
            &flat,
            trim(),
            Effects { crop: crop(64, 32, 384, 288, Some([3, 4])), turn: 1, flip_h: true, size: OutputSize::Short { short: 192 }, ..Effects::default() },
        ),
        (
            "왼쪽 90° + 상하 + 맞춤 480 × 480",
            &flat,
            trim(),
            Effects { turn: 3, flip_v: true, size: OutputSize::Exact { width: 480, height: 480, fit: Fit::Contain }, ..Effects::default() },
        ),
        ("180° + 채움 320 × 320", &flat, trim(), Effects { turn: 2, size: OutputSize::Exact { width: 320, height: 320, fit: Fit::Cover }, ..Effects::default() }),
        (
            "맞춤 여백만 (크기 그대로)",
            &flat,
            trim(),
            Effects { crop: crop(160, 0, 320, 480, None), size: OutputSize::Exact { width: 480, height: 480, fit: Fit::Contain }, ..Effects::default() },
        ),
        ("늘림 400 × 200", &flat, trim(), Effects { crop: crop(0, 96, 640, 288, None), size: OutputSize::Exact { width: 400, height: 200, fit: Fit::Stretch }, ..Effects::default() }),
        ("폰 영상: 세운 화면의 크롭 + 회전", &phone, trim(), Effects { crop: crop(32, 96, 352, 448, None), turn: 1, ..Effects::default() }),
        (
            "구간 제거 + 크롭 + 회전",
            &flat,
            Edit::Cut { removed: vec![plan::Range::new(0.4, 1.0)] },
            Effects { crop: crop(128, 64, 256, 320, None), turn: 3, flip_h: true, ..Effects::default() },
        ),
    ];
    for (label, source, edit, e) in cases {
        let source_rgb = rgb_frame(&ff, source.as_os_str(), Some(0.0), false);
        let info = probe::probe(Path::new(&ff.ffprobe_path), source).unwrap();
        let src = info.video.as_ref().unwrap().upright_size();
        assert_eq!(source_rgb.len(), (src.0 * src.1 * 3) as usize, "{label}: the reference is the upright picture");
        let (out, summary, plan) = export_with_effects(&ff, source, edit, command::OutputCodec::Source, e, software_report);
        let plan = plan.expect("the effects change the picture");
        assert_eq!(summary.picture, Some(plan.size), "{label}");
        assert_eq!(stream_fields(&ff, &out, "v:0", "stream=width,height"), format!("height={} width={}", plan.size[1], plan.size[0]), "{label}");
        let sar = stream_fields(&ff, &out, "v:0", "stream=sample_aspect_ratio");
        assert!(sar == "sample_aspect_ratio=1:1" || sar == "sample_aspect_ratio=N/A", "{label}: square pixels, {sar}");
        let (compared, worst) = assert_shows_the_drawn_area(label, &source_rgb, src, &rgb_frame(&ff, out.as_os_str(), None, false), &plan);
        eprintln!("{label}: {} × {} · {compared} pixels, largest difference {worst}", plan.size[0], plan.size[1]);
        std::fs::remove_file(&out).unwrap();
    }
    assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The first frame's luma of a 10-bit file, `w` × `h` values 0–1023.
fn luma10(ff: &ffmpeg::FfmpegInfo, file: &Path, w: usize, h: usize) -> Vec<u16> {
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-i"]);
    args.push(file.as_os_str().to_owned());
    args.extend(os_args(&["-map", "0:v:0", "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "yuv420p10le", "-"]));
    let raw = run_ok(ff, &args).stdout_raw;
    raw[..w * h * 2].chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect()
}

/// How far the luma of rows `rows` × `columns` strays from `want(x)`, pixel by pixel (RMS, in 10-bit
/// steps). An 8-bit stage on the way leaves each pixel on a 4-step grid, dithered: the encoders keep
/// part of that noise — x265 · SVT-AV1 about 1.5 against 0.5 for ten bits all the way; VideoToolbox
/// smooths it to its own noise level (0.6 either way), so it cannot tell (measured).
fn ramp_error(luma: &[u16], w: usize, rows: std::ops::Range<usize>, columns: std::ops::Range<usize>, want: impl Fn(usize) -> f64) -> f64 {
    let (mut sum, mut n) = (0.0, 0.0);
    for y in rows {
        for x in columns.clone() {
            sum += (f64::from(luma[y * w + x]) - want(x)).powi(2);
            n += 1.0;
        }
    }
    (sum / n).sqrt()
}

/// 화면 효과 keep 10 bits and the HDR description (crop · turn · scale · pad come before setparams,
/// 모자이크 · 블러 · PiP go through `overlay`, which works in 8 bits unless told otherwise), for every
/// encoder that works here, 자르기 and 구간 제거: H.265 · AV1 stay 10-bit — really, not only in name:
/// a smooth 10-bit ramp comes out without 8-bit stairs — and H.264 (원본 유지 of a 10-bit H.264) is
/// 8-bit by design but keeps the colors' description; 맞춤's bars stay black.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn effects_keep_ten_bit_and_hdr_tags() {
    use effects::{Crop, Effects, Fit, Mask, MaskKind, OutputSize, Overlay};
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx265 && ff.features.libx264, "this test needs libx265 and libx264 for its source clips");
    let dir = temp_dir("effects-hdr");
    // A 10-bit PQ picture whose luma climbs one step every 2 pixels (256 … 575), stored losslessly.
    let ramp = "nullsrc=s=640x360:r=30:d=2,format=yuv420p10le,geq=lum='256+X/2':cb=512:cr=512,setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc";
    let hevc = dir.join("HDR 효과 원본.mkv");
    let mut make = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", ramp]);
    make.extend(os_args(&["-c:v", "libx265", "-x265-params", "lossless=1:log-level=error", "-pix_fmt", "yuv420p10le"]));
    make.push(hevc.as_os_str().to_owned());
    run_ok(&ff, &make);
    let avc = dir.join("HDR 효과 원본 H.264.mp4");
    let mut make = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", ramp]);
    make.extend(os_args(&["-c:v", "libx264", "-qp", "0", "-pix_fmt", "yuv420p10le"]));
    make.push(avc.as_os_str().to_owned());
    run_ok(&ff, &make);
    let pip = dir.join("카메라 화면 번호.mp4");
    make_coded_pip(&ff, &pip);
    let logo = dir.join("채널 로고.png");
    make_logo(&ff, &logo);
    // Everything that goes through `overlay` sits in the top 280 rows; the ramp is read below.
    let layers = Effects {
        masks: vec![
            Mask { x: 200, y: 40, width: 64, height: 64, start: 0.0, end: 2.0, kind: MaskKind::Mosaic, strength: 60 },
            Mask { x: 300, y: 200, width: 80, height: 80, start: 0.0, end: 2.0, kind: MaskKind::Blur, strength: 60 },
        ],
        overlays: vec![
            Overlay { path: pip.display().to_string(), x: 400, y: 20, width: 64, height: 48, start: 0.2, end: 1.8, offset: 0.0, opacity: 80, audio: false, volume: 100 },
            Overlay { path: logo.display().to_string(), x: 180, y: 120, width: 32, height: 32, start: 0.0, end: 2.0, offset: 0.0, opacity: 50, audio: false, volume: 100 },
        ],
        ..Effects::default()
    };
    let crop = Some(Crop { x: 160, y: 0, width: 320, height: 360, ratio: None });
    // Turned and fitted (bars); and as it is but mirrored, the ramp running along the rows.
    let turned = Effects { crop, turn: 1, size: OutputSize::Exact { width: 400, height: 400, fit: Fit::Contain }, ..layers.clone() };
    let mirrored = Effects { crop, flip_h: true, ..layers.clone() };
    let mut tested: Vec<(String, String)> = working_export_encoders(&ff);
    assert!(tested.iter().any(|(n, _)| n == "libx265") && tested.iter().any(|(n, _)| n == "libx264"), "{tested:?}");
    tested.sort();
    for (encoder, codec) in &tested {
        let source = if codec == "h264" { &avc } else { &hevc };
        let report = || encoders::EncoderReport {
            tests: vec![encoders::EncoderTest { name: encoder.clone(), codec: codec.clone(), hardware: false, ok: true, millis: 1, error: None }],
            preferred_hevc: None,
            preferred_av1: None,
        };
        for edit in [Edit::Trim { start: 0.5, end: 1.5 }, Edit::Cut { removed: vec![plan::Range::new(0.5, 1.0)] }] {
            let label = format!("{encoder} {edit:?}");
            let (out, summary, plan) = export_with_effects(&ff, source, edit.clone(), output_for(codec), turned.clone(), report);
            let plan = plan.unwrap();
            assert_eq!((plan.size, summary.masks, summary.overlays), ([400, 400], 2, 2), "{label}");
            let (tag, pix) = match codec.as_str() {
                "hevc" => ("codec_name=hevc codec_tag_string=hvc1", "yuv420p10le"),
                "av1" => ("codec_name=av1 codec_tag_string=av01", "yuv420p10le"),
                // 원본 유지 of H.264 is 8-bit H.264 (10-bit H.264 plays almost nowhere).
                _ => ("codec_name=h264 codec_tag_string=avc1", "yuv420p"),
            };
            assert_eq!(
                video_stream_line(&ff, &out),
                format!("{tag} color_primaries=bt2020 color_space=bt2020nc color_transfer=smpte2084 pix_fmt={pix}"),
                "{label}"
            );
            assert_eq!(stream_fields(&ff, &out, "v:0", "stream=width,height"), "height=400 width=400", "{label}");
            // The turned box is seen 360 × 320: it fits as 400 × 356 (355.6) at y 22, bars above and below.
            assert_eq!((plan.scale, plan.pad.map(|p| (p.x, p.y))), (Some([400, 356]), Some((0, 22))));
            let rgb = rgb_frame(&ff, out.as_os_str(), None, false);
            for (x, y) in [(200u32, 4u32), (100, 395), (20, 8)] {
                let i = ((y * 400 + x) * 3) as usize;
                assert!(rgb[i..i + 3].iter().all(|c| *c < 24), "{label}: bar at ({x}, {y}) is {:?}", &rgb[i..i + 3]);
            }
            std::fs::remove_file(&out).unwrap();
            if codec == "h264" {
                continue;
            }
            // Ten bits all the way: the mirrored ramp (320 × 360; source column 160 + 319 − x) comes
            // out as it went in, not on an 8-bit grid (without `overlay`'s format=yuv420p10: 1.5).
            let (out, _, _) = export_with_effects(&ff, source, edit, output_for(codec), mirrored.clone(), report);
            let error = ramp_error(&luma10(&ff, &out, 320, 360), 320, 290..350, 8..312, |x| 256.0 + ((479 - x) / 2) as f64);
            eprintln!("{label}: the ramp is off by {error:.2} (RMS, 10-bit steps)");
            assert!(error < 1.0, "{label}: the ramp is off by {error:.2} (RMS) — cut to 8 bits on the way?");
            std::fs::remove_file(&out).unwrap();
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The pixels of the box `r` (x, y, w, h) of an RGB frame `width` wide.
fn box_pixels(rgb: &[u8], width: u32, r: (u32, u32, u32, u32)) -> Vec<[u8; 3]> {
    let mut out = Vec::new();
    for y in r.1..r.1 + r.3 {
        for x in r.0..r.0 + r.2 {
            let i = ((y * width + x) * 3) as usize;
            out.push([rgb[i], rgb[i + 1], rgb[i + 2]]);
        }
    }
    out
}

/// Mean absolute difference per channel between two pixel lists (0–255).
fn mean_diff(a: &[[u8; 3]], b: &[[u8; 3]]) -> f64 {
    a.iter().zip(b).map(|(p, q)| (0..3).map(|c| f64::from(p[c].abs_diff(q[c]))).sum::<f64>()).sum::<f64>() / (a.len() * 3) as f64
}

/// The sharpest step between neighbouring pixels along rows: a block edge in the source, much less
/// once blurred (a blur spreads the step over a few sigmas; the sum of the steps would not change).
fn peak_edge(px: &[[u8; 3]], w: u32) -> u8 {
    let w = w as usize;
    (0..px.len()).filter(|i| (i + 1) % w != 0).map(|i| (0..3).map(|c| px[i][c].abs_diff(px[i + 1][c])).max().unwrap()).max().unwrap_or(0)
}

/// The promise of 모자이크 · 블러: the box is hidden exactly while it shows (its times moved into the
/// result — 자르기 shifts them, 구간 제거 joins them across the cut), and nothing outside the box or
/// outside its times changes. 모자이크 = flat cells larger than the source's blocks, 블러 = soft edges.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn masks_hide_the_box_only_at_their_times() {
    use effects::{Effects, Mask, MaskKind};
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx264, "this test needs libx264");
    let dir = temp_dir("effects-masks");
    let source = dir.join("가릴 영역 원본.mp4");
    make_block_clip(&ff, &source, None);
    let src_rgb = rgb_frame(&ff, source.as_os_str(), Some(0.0), false);
    let (a, b) = ((64u32, 64u32, 192u32, 128u32), (384u32, 256u32, 192u32, 160u32));
    let outside = (448u32, 32u32, 128u32, 96u32);
    let masks = vec![
        Mask { x: a.0, y: a.1, width: a.2, height: a.3, start: 0.8, end: 1.2, kind: MaskKind::Mosaic, strength: 100 },
        Mask { x: b.0, y: b.1, width: b.2, height: b.3, start: 0.0, end: 2.0, kind: MaskKind::Blur, strength: 60 },
    ];
    let src_a = box_pixels(&src_rgb, 640, a);
    let src_b = box_pixels(&src_rgb, 640, b);
    let src_out = box_pixels(&src_rgb, 640, outside);
    let check = |label: &str, out: &Path, t: f64, a_hidden: bool| {
        let rgb = rgb_frame(&ff, out.as_os_str(), Some(t), false);
        assert_eq!(rgb.len(), 640 * 480 * 3, "{label}");
        let got_a = box_pixels(&rgb, 640, a);
        let got_b = box_pixels(&rgb, 640, b);
        let diff_a = mean_diff(&got_a, &src_a);
        if a_hidden {
            // 29 px cells averaged across the 32 px blocks: other colors than the source.
            assert!(diff_a > 8.0, "{label} t={t}: box A should be a mosaic (diff {diff_a:.1})");
        } else {
            assert!(diff_a < 3.0, "{label} t={t}: box A should be untouched (diff {diff_a:.1})");
        }
        let (blurred, sharp) = (peak_edge(&got_b, b.2), peak_edge(&src_b, b.2));
        assert!(f64::from(blurred) < f64::from(sharp) * 0.5, "{label} t={t}: box B should be blurred (step {blurred} vs {sharp})");
        let diff_out = mean_diff(&box_pixels(&rgb, 640, outside), &src_out);
        assert!(diff_out < 3.0, "{label} t={t}: outside the boxes untouched (diff {diff_out:.1})");
    };
    // `-ss` drops the frames before it: just under frame k's start gets frame k.
    let frame = |k: u32| (k as f64 / 30.0 - 0.002).max(0.0);

    // 자르기 0.5–1.5: box A shows 0.8–1.2 → result 0.3–0.7 (frames 9–20).
    let e = Effects { masks: masks.clone(), ..Effects::default() };
    let (out, summary, _) = export_with_effects(&ff, &source, Edit::Trim { start: 0.5, end: 1.5 }, command::OutputCodec::Source, e, software_report);
    assert_eq!((summary.masks, summary.picture), (2, Some([640, 480])));
    for (k, hidden) in [(3, false), (8, false), (9, true), (15, true), (20, true), (21, false), (27, false)] {
        check("자르기", &out, frame(k), hidden);
    }
    std::fs::remove_file(&out).unwrap();

    // 구간 제거 0.9–1.1: kept 0–0.9 · 1.1–2, box A 0.8–1.2 → result 0.8–1.0 (frames 24–29), joined.
    let e = Effects { masks: masks.clone(), ..Effects::default() };
    let (out, summary, _) = export_with_effects(&ff, &source, Edit::Cut { removed: vec![plan::Range::new(0.9, 1.1)] }, command::OutputCodec::Source, e, software_report);
    assert_eq!(summary.masks, 2);
    for (k, hidden) in [(20, false), (23, false), (24, true), (26, true), (29, true), (30, false), (40, false)] {
        check("구간 제거", &out, frame(k), hidden);
    }
    std::fs::remove_file(&out).unwrap();

    // A box only in what is cut away does nothing: the summary counts the one left.
    let gone = Effects { masks: vec![Mask { start: 0.92, end: 1.08, ..masks[0] }, masks[1]], ..Effects::default() };
    let (out, summary, _) = export_with_effects(&ff, &source, Edit::Cut { removed: vec![plan::Range::new(0.9, 1.1)] }, command::OutputCodec::Source, gone, software_report);
    assert_eq!(summary.masks, 1);
    check("잘린 영역의 상자", &out, frame(27), false);
    std::fs::remove_file(&out).unwrap();
    assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The pixel at (`x`, `y`) of a 640-wide RGB frame.
fn pixel(rgb: &[u8], x: u32, y: u32) -> [u8; 3] {
    let i = ((y * 640 + x) * 3) as usize;
    [rgb[i], rgb[i + 1], rgb[i + 2]]
}

fn near(a: [u8; 3], b: [u8; 3], tolerance: u8) -> bool {
    (0..3).all(|c| a[c].abs_diff(b[c]) <= tolerance)
}

/// The promise of PiP: a video put on top shows in its box exactly from its first to its last frame
/// (moved into the result: 자르기 shifts it, 구간 제거 jumps it with the original), its sound lands at
/// the matching time in the first audio track, and an image with transparency shows only where it is
/// opaque, at its opacity. Nothing else changes.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn pip_shows_in_its_box_at_its_times_with_its_sound() {
    use effects::{Effects, Overlay};
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx264, "this test needs libx264");
    let dir = temp_dir("effects-pip");
    // The source: the block clip with its own click at 0.7 s (quieter than the PiP's).
    let blocks = dir.join("블록.mp4");
    make_block_clip(&ff, &blocks, None);
    let source = dir.join("PiP 원본 화면.mp4");
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"]);
    args.push(blocks.as_os_str().to_owned());
    args.extend(os_args(&[
        "-f", "lavfi", "-i", r"aevalsrc=exprs='if(between(t\,0.7\,0.701)\,0.6\,0)':s=44100:d=2",
        "-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", "aac",
    ]));
    args.push(source.as_os_str().to_owned());
    run_ok(&ff, &args);
    // The video put on top: solid red, 25 fps, a click at 1.2 s into it.
    let camera = dir.join("카메라 화면.mp4");
    let mut args = os_args(&[
        "-hide_banner", "-nostdin", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", "color=c=red:s=320x240:r=25:d=3",
        "-f", "lavfi", "-i", r"aevalsrc=exprs='if(between(t\,1.2\,1.201)\,0.9\,0)':s=44100:d=3",
        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest",
    ]);
    args.push(camera.as_os_str().to_owned());
    run_ok(&ff, &args);
    // A logo: a blue square in a transparent border.
    let logo = dir.join("채널 로고.png");
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=blue:s=32x32,format=rgba,pad=64:64:16:16:color=white@0", "-frames:v", "1"]);
    args.push(logo.as_os_str().to_owned());
    run_ok(&ff, &args);

    let src_rgb = rgb_frame(&ff, source.as_os_str(), Some(0.0), false);
    let (video_at, logo_at, logo_edge, untouched) = ((400u32, 300u32), (72u32, 72u32), (44u32, 44u32), (600u32, 20u32));
    let effects = || Effects {
        overlays: vec![
            Overlay { path: camera.display().to_string(), x: 320, y: 240, width: 160, height: 120, start: 0.6, end: 1.6, offset: 0.5, opacity: 100, audio: true, volume: 100 },
            Overlay { path: logo.display().to_string(), x: 40, y: 40, width: 64, height: 64, start: 0.0, end: 2.0, offset: 0.0, opacity: 50, audio: false, volume: 100 },
        ],
        ..Effects::default()
    };
    let frame = |k: u32| (k as f64 / 30.0 - 0.002).max(0.0);
    let check = |label: &str, out: &Path, k: u32, red: bool| {
        let rgb = rgb_frame(&ff, out.as_os_str(), Some(frame(k)), false);
        let v = pixel(&rgb, video_at.0, video_at.1);
        if red {
            assert!(near(v, [255, 0, 0], 40), "{label} frame {k}: the PiP video should show, got {v:?}");
        } else {
            assert!(near(v, pixel(&src_rgb, video_at.0, video_at.1), 6), "{label} frame {k}: no PiP video yet / any more, got {v:?}");
        }
        // The logo at 50 %: half way between the picture and blue; its transparent border shows the picture.
        let under = pixel(&src_rgb, logo_at.0, logo_at.1);
        let want = [under[0] / 2, under[1] / 2, ((u16::from(under[2]) + 255) / 2) as u8];
        let got = pixel(&rgb, logo_at.0, logo_at.1);
        assert!(near(got, want, 14), "{label} frame {k}: logo {got:?}, want about {want:?}");
        assert!(near(pixel(&rgb, logo_edge.0, logo_edge.1), pixel(&src_rgb, logo_edge.0, logo_edge.1), 8), "{label}: the logo's transparent border");
        assert!(near(pixel(&rgb, untouched.0, untouched.1), pixel(&src_rgb, untouched.0, untouched.1), 6), "{label}: elsewhere untouched");
    };

    // 자르기 0.5–1.8: the video (original 0.6–1.6, from 0.5 s in) → result 0.1–1.1 (frames 3–32);
    // its click at 1.2 s → result 0.1 + 0.7 = 0.8 s, the source's own at 0.7 → 0.2 s.
    let (out, summary, _) = export_with_effects(&ff, &source, Edit::Trim { start: 0.5, end: 1.8 }, command::OutputCodec::Source, effects(), software_report);
    assert_eq!((summary.overlays, summary.audio), (2, command::AudioPlan::Aac));
    for (k, red) in [(2, false), (3, true), (17, true), (32, true), (33, false), (36, false)] {
        check("자르기", &out, k, red);
    }
    let clicks = click_times(&ff, &out, 0);
    assert!(clicks.len() == 2 && (clicks[0] - 0.2).abs() < 0.015 && (clicks[1] - 0.8).abs() < 0.015, "자르기: clicks at {clicks:?}, want [0.2, 0.8]");
    std::fs::remove_file(&out).unwrap();

    // 구간 제거 0.9–1.1: the video jumps with the original — original 0.6–0.9 → result 0.6–0.9 from
    // 0.5 s, 1.1–1.6 → result 0.9–1.4 from 1.0 s; its click at 1.2 s → result 0.9 + 0.2 = 1.1 s,
    // the source's own stays at 0.7 s. (Mixed by samples, the PiP click came 23 ms late before the
    // source's sound was put at 0 — `command::AUDIO_FROM_ZERO`.)
    let (out, summary, _) = export_with_effects(&ff, &source, Edit::Cut { removed: vec![plan::Range::new(0.9, 1.1)] }, command::OutputCodec::Source, effects(), software_report);
    assert_eq!(summary.overlays, 2);
    for (k, red) in [(17, false), (18, true), (27, true), (41, true), (42, false)] {
        check("구간 제거", &out, k, red);
    }
    let clicks = click_times(&ff, &out, 0);
    assert!(clicks.len() == 2 && (clicks[0] - 0.7).abs() < 0.015 && (clicks[1] - 1.1).abs() < 0.015, "구간 제거: clicks at {clicks:?}, want [0.7, 1.1]");
    std::fs::remove_file(&out).unwrap();
    assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A PiP stands upright in the result whatever the picture's turn and mirror: an image of four
/// colored quarters (red · blue over green · white) shows them in the same places inside its box as
/// seen — the box given in the original's pixels, like the editor sends it.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn pip_stands_upright_when_the_picture_turns_or_mirrors() {
    use effects::{Effects, Overlay};
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx264, "this test needs libx264");
    let dir = temp_dir("effects-pip-upright");
    let source = dir.join("돌릴 원본 화면.mp4");
    make_block_clip(&ff, &source, None);
    let quarters = dir.join("네 칸 로고.png");
    let mut args = os_args(&[
        "-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-filter_complex",
        "color=c=red:s=32x16:d=1[a];color=c=blue:s=32x16:d=1[b];color=c=lime:s=32x16:d=1[c];color=c=white:s=32x16:d=1[d];[a][b]hstack[t];[c][d]hstack[u];[t][u]vstack",
        "-frames:v", "1",
    ]);
    args.push(quarters.as_os_str().to_owned());
    run_ok(&ff, &args);
    // (turn, flipH, flipV, the box in the original, where it is seen, the seen width)
    let cases = [
        (1, true, false, (100, 200, 32, 64), (200, 100), 480usize),
        (3, false, false, (100, 200, 32, 64), (200, 508), 480),
        (0, false, true, (100, 200, 64, 32), (100, 248), 640),
        (2, false, false, (100, 200, 64, 32), (476, 248), 640),
    ];
    for (turn, flip_h, flip_v, (x, y, width, height), (sx, sy), seen_w) in cases {
        let e = Effects {
            turn,
            flip_h,
            flip_v,
            overlays: vec![Overlay { path: quarters.display().to_string(), x, y, width, height, start: 0.0, end: 1.0, offset: 0.0, opacity: 100, audio: false, volume: 100 }],
            ..Effects::default()
        };
        let (out, _, _) = export_with_effects(&ff, &source, Edit::Trim { start: 0.0, end: 1.0 }, command::OutputCodec::Source, e, software_report);
        let rgb = rgb_frame(&ff, out.as_os_str(), Some(0.0), false);
        let at = |px: usize, py: usize| {
            let i = (py * seen_w + px) * 3;
            [rgb[i], rgb[i + 1], rgb[i + 2]]
        };
        let label = format!("turn {turn} flipH {flip_h} flipV {flip_v}");
        for ((dx, dy), want) in [((16, 8), [255, 0, 0]), ((48, 8), [0, 0, 255]), ((16, 24), [0, 255, 0]), ((48, 24), [255, 255, 255])] {
            let got = at(sx + dx, sy + dy);
            assert!(near(got, want, 48), "{label}: quarter at +{dx},+{dy} is {got:?}, want {want:?}");
        }
        std::fs::remove_file(&out).unwrap();
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

// --- 완료 기준 측정 (Phase 4 step 4): every 화면 효과 at once, with 자르기 · 구간 제거 ---

/// The coded clips carry their frame number as 8 squares, black = 0 and white = 1 (bit i is square i).
const CODE_BITS: u32 = 8;
/// The source's code squares: block row 2 from block column 5 (x 160 … 416, y 64 … 96).
const SOURCE_CODE: (u32, u32) = (160, 64);
/// A PiP video's: 32 px squares 38 px apart along its top from (8, 8); the rest is red.
const PIP_CODE: (u32, u32) = (8, 8);
const PIP_SIZE: (f64, f64) = (320.0, 240.0);
const PIP_FPS: f64 = 25.0;
/// Clicks in the source's sound, and in the PiP video's (on its own timeline).
const SOURCE_CLICKS: [f64; 4] = [0.3, 1.7, 2.6, 3.7];
const PIP_CLICKS: [f64; 3] = [1.0, 1.6, 3.2];
/// Boxes in the coded source (upright pixels), inside every case's crop and apart from the code
/// squares and each other (x, y, w, h): 모자이크, 블러, the logo, and an untouched box the blur is
/// measured against. The PiP video's is [`pip_box`].
const MOSAIC_BOX: (u32, u32, u32, u32) = (168, 112, 120, 96);
const BLUR_BOX: (u32, u32, u32, u32) = (304, 112, 128, 96);
const LOGO_BOX: (u32, u32, u32, u32) = (368, 240, 64, 64);
const CONTROL_BOX: (u32, u32, u32, u32) = (368, 312, 96, 64);

/// The PiP video's box: 4:3 as seen, so 3:4 in the source when the picture turns a quarter.
fn pip_box(quarter: bool) -> (u32, u32, u32, u32) {
    if quarter { (176, 224, 120, 160) } else { (176, 224, 160, 120) }
}

/// `drawbox` filters writing the frame number `n` as squares of `size` from `origin`, `step` apart.
fn code_filter(origin: (u32, u32), step: u32, size: u32) -> String {
    (0..CODE_BITS)
        .map(|i| {
            let (x, y) = (origin.0 + i * step, origin.1);
            let b = format!("drawbox=x={x}:y={y}:w={size}:h={size}:t=fill:color=");
            format!(r"{b}black,{b}white:enable='mod(floor(n/{})\,2)'", 1u32 << i)
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// 4 s of silence with 1 ms clicks at `times`.
fn clicks_source(times: &[f64]) -> String {
    let parts: Vec<String> = times.iter().map(|t| format!(r"between(t\,{t:.3}\,{:.3})", t + 0.001)).collect();
    format!(r"aevalsrc=exprs='if({}\,0.6\,0)':s=44100:d=4", parts.join("+"))
}

/// 4 s of the block picture (640 × 480) with its frame number in the code squares at `rate` —
/// thinned by `select` to a variable rate, the number staying the frame's before thinning — and AAC
/// with [`SOURCE_CLICKS`]. Lossless H.264.
fn make_coded_clip(ff: &ffmpeg::FfmpegInfo, path: &Path, rate: &str, select: Option<&str>) {
    let still = path.with_extension("ppm");
    std::fs::write(&still, block_ppm()).unwrap();
    let mut vf = code_filter(SOURCE_CODE, 32, 32);
    if let Some(s) = select {
        vf = format!("{vf},{s}");
    }
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-loop", "1", "-framerate", rate, "-i"]);
    args.push(still.as_os_str().to_owned());
    args.extend(os_args(&["-f", "lavfi", "-i", &clicks_source(&SOURCE_CLICKS), "-vf", &vf]));
    if select.is_some() {
        args.extend(os_args(&["-fps_mode", "vfr"]));
    }
    args.extend(os_args(&["-t", "4", "-c:v", "libx264", "-qp", "0", "-pix_fmt", "yuv420p", "-g", "30", "-c:a", "aac"]));
    args.push(path.as_os_str().to_owned());
    run_ok(ff, &args);
    std::fs::remove_file(&still).unwrap();
}

/// The PiP video: red, 320 × 240 at 25 fps for 4 s, its frame number along the top, [`PIP_CLICKS`].
fn make_coded_pip(ff: &ffmpeg::FfmpegInfo, path: &Path) {
    let video = format!("color=c=red:s=320x240:r=25:d=4,{}", code_filter(PIP_CODE, 38, 32));
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", &video]);
    args.extend(os_args(&["-f", "lavfi", "-i", &clicks_source(&PIP_CLICKS), "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"]));
    args.push(path.as_os_str().to_owned());
    run_ok(ff, &args);
}

/// A logo: a blue 32 px square in a 16 px transparent border.
fn make_logo(ff: &ffmpeg::FfmpegInfo, path: &Path) {
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=blue:s=32x32,format=rgba,pad=64:64:16:16:color=white@0", "-frames:v", "1"]);
    args.push(path.as_os_str().to_owned());
    run_ok(ff, &args);
}

fn px_at(rgb: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * w + x) * 3) as usize;
    [rgb[i], rgb[i + 1], rgb[i + 2]]
}

/// The number in code squares centered at `centers` (pixels of a `w` × `h` frame): white = 1.
fn read_code(rgb: &[u8], w: u32, h: u32, centers: &[(f64, f64)]) -> u32 {
    let mut code = 0;
    for (i, &(cx, cy)) in centers.iter().enumerate() {
        let (cx, cy) = (cx.round() as i64, cy.round() as i64);
        let mut sum = 0u32;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let p = px_at(rgb, w, (cx + dx).clamp(0, w as i64 - 1) as u32, (cy + dy).clamp(0, h as i64 - 1) as u32);
                sum += (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3;
            }
        }
        if sum / 9 > 128 {
            code |= 1 << i;
        }
    }
    code
}

/// Every frame of `file` as RGB (upright size) with its time, as a player shows them.
fn decoded_frames(ff: &ffmpeg::FfmpegInfo, file: &Path) -> (Vec<f64>, Vec<Vec<u8>>, (u32, u32)) {
    let info = probe::probe(Path::new(&ff.ffprobe_path), file).unwrap();
    let (w, h) = info.video.as_ref().unwrap().upright_size();
    let mut args = os_args(&["-hide_banner", "-nostdin", "-loglevel", "error", "-i"]);
    args.push(file.as_os_str().to_owned());
    args.extend(os_args(&["-map", "0:v:0", "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]));
    let frames: Vec<Vec<u8>> = run_ok(ff, &args).stdout_raw.chunks_exact((w * h * 3) as usize).map(<[u8]>::to_vec).collect();
    let times: Vec<f64> = frames_by_pts(ff, file).into_iter().map(|(t, _)| t).collect();
    assert_eq!(times.len(), frames.len(), "{}: frame times", file.display());
    (times, frames, (w, h))
}

/// Where the upright source's point (`sx`, `sy`) lands in the result, by the plan's numbers — the
/// inverse of [`source_point`].
fn result_point(plan: &effects::EffectsPlan, src: (u32, u32), sx: f64, sy: f64) -> (f64, f64) {
    let crop = plan.crop.unwrap_or(effects::Rect { x: 0, y: 0, width: src.0, height: src.1 });
    let (x, y) = (sx - crop.x as f64, sy - crop.y as f64);
    let (wc, hc) = (crop.width as f64, crop.height as f64);
    let (cw, ch) = if plan.turn % 2 == 1 { (hc, wc) } else { (wc, hc) };
    let (mut tx, mut ty) = match plan.turn {
        1 => (hc - y, x),
        3 => (y, wc - x),
        _ => (x, y),
    };
    if plan.flip_h {
        tx = cw - tx;
    }
    if plan.flip_v {
        ty = ch - ty;
    }
    let [pw, ph] = plan.picture_size(src);
    let (px, py) = plan.pad.map_or((0.0, 0.0), |p| (p.x as f64, p.y as f64));
    (px + tx * pw as f64 / cw, py + ty * ph as f64 / ch)
}

/// The box `r` of the source as seen in the result: (x0, y0, x1, y1).
fn seen_rect(plan: &effects::EffectsPlan, src: (u32, u32), r: (u32, u32, u32, u32)) -> (f64, f64, f64, f64) {
    let corners = [(r.0, r.1), (r.0 + r.2, r.1), (r.0, r.1 + r.3), (r.0 + r.2, r.1 + r.3)].map(|(x, y)| result_point(plan, src, x as f64, y as f64));
    let (xs, ys) = (corners.map(|c| c.0), corners.map(|c| c.1));
    (xs.iter().cloned().fold(f64::MAX, f64::min), ys.iter().cloned().fold(f64::MAX, f64::min), xs.iter().cloned().fold(f64::MIN, f64::max), ys.iter().cloned().fold(f64::MIN, f64::max))
}

/// A point (`u`, `v`) of an upright PiP file of `size` shown in the seen rect `r`.
fn in_pip(r: (f64, f64, f64, f64), size: (f64, f64), u: f64, v: f64) -> (f64, f64) {
    (r.0 + u / size.0 * (r.2 - r.0), r.1 + v / size.1 * (r.3 - r.1))
}

fn inside(r: (u32, u32, u32, u32), x: f64, y: f64, grow: f64) -> bool {
    x >= r.0 as f64 - grow && x < (r.0 + r.2) as f64 + grow && y >= r.1 as f64 - grow && y < (r.1 + r.3) as f64 + grow
}

/// The kept ranges of `edit` on a source `duration` long (original timeline).
fn kept_ranges(edit: &Edit, duration: f64) -> Vec<plan::Range> {
    match edit {
        Edit::Trim { start, end } => vec![plan::Range::new(*start, *end)],
        Edit::Cut { removed } => {
            let mut keep = Vec::new();
            let mut at = 0.0;
            for r in removed {
                if r.start > at {
                    keep.push(plan::Range::new(at, r.start));
                }
                at = r.end;
            }
            if at < duration {
                keep.push(plan::Range::new(at, duration));
            }
            keep
        }
    }
}

/// Where source time `t` lands in the result made of `keep` (None: cut away). Frame-exact: a frame
/// starting at a kept range's start is in, one starting at its end is out.
fn result_time(keep: &[plan::Range], t: f64) -> Option<(f64, plan::Range)> {
    let mut before = 0.0;
    for k in keep {
        if t >= k.start - 1e-4 && t < k.end - 1e-4 {
            return Some((before + t - k.start, *k));
        }
        before += k.len();
    }
    None
}

/// Mean difference to the source's blocks of the result's pixels that show the source box `r`
/// (shrunk by `inset`), away from the blocks' edges: about 0 when the box is untouched.
#[allow(clippy::too_many_arguments)]
fn box_change(rgb: &[u8], size: (u32, u32), plan: &effects::EffectsPlan, src: (u32, u32), r: (u32, u32, u32, u32), inset: f64, margin: f64) -> f64 {
    let seen = seen_rect(plan, src, r);
    let (mut sum, mut n) = (0.0, 0usize);
    for oy in (seen.1.floor().max(0.0) as u32..(seen.3.ceil() as u32).min(size.1)).step_by(2) {
        for ox in (seen.0.floor().max(0.0) as u32..(seen.2.ceil() as u32).min(size.0)).step_by(2) {
            let Some((sx, sy)) = source_point(plan, src, ox as f64 + 0.5, oy as f64 + 0.5) else { continue };
            let inner = (r.0 + inset as u32, r.1 + inset as u32, r.2 - 2 * inset as u32, r.3 - 2 * inset as u32);
            let (bx, by) = (sx % BLOCK, sy % BLOCK);
            if !inside(inner, sx, sy, 0.0) || bx < margin || bx > BLOCK - margin || by < margin || by > BLOCK - margin {
                continue;
            }
            let want = block_color((sx / BLOCK) as usize, (sy / BLOCK) as usize);
            let got = px_at(rgb, size.0, ox, oy);
            sum += (0..3).map(|c| f64::from(got[c].abs_diff(want[c]))).sum::<f64>() / 3.0;
            n += 1;
        }
    }
    assert!(n > 20, "only {n} pixels of the box {r:?}");
    sum / n as f64
}

/// The sharpest step between neighbours along the rows of the result's seen rect of `r`, inside it.
fn seen_peak_edge(rgb: &[u8], size: (u32, u32), plan: &effects::EffectsPlan, src: (u32, u32), r: (u32, u32, u32, u32)) -> u8 {
    let s = seen_rect(plan, src, r);
    let (x0, y0, x1, y1) = (s.0.ceil() as u32 + 3, s.1.ceil() as u32 + 3, (s.2.floor() as u32).min(size.0) - 3, (s.3.floor() as u32).min(size.1) - 3);
    let px: Vec<[u8; 3]> = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| px_at(rgb, size.0, x, y)).collect();
    peak_edge(&px, x1 - x0)
}

/// What one combo export got right, for the log: (pixels compared, largest difference, largest
/// frame-time error, largest click error).
struct ComboStats {
    compared: usize,
    worst: u8,
    time_error: f64,
    click_error: f64,
}

/// The promise of 화면 효과 together, frame by frame: every result frame is the source frame the
/// keep list says (its code squares), and every kept piece keeps its frames' spacing, starting where
/// the plan puts it — at a constant rate exactly, at a variable one never early and less than a
/// frame late (`concat` ends a piece by its frames' average spacing); the picture outside the boxes
/// is the source's at the place the plan says (crop · turn · mirror · size · bars); 모자이크 · 블러 ·
/// the PiP video · the logo show exactly on the source frames inside their times (the PiP at the
/// frame of its own that follows the original, gone once it ends), upright; and the clicks of both
/// sounds land where the picture shows their source time.
#[allow(clippy::too_many_arguments)]
fn check_combo(
    label: &str,
    ff: &ffmpeg::FfmpegInfo,
    source: &(Vec<f64>, Vec<u32>),
    src: (u32, u32),
    duration: f64,
    edit: &Edit,
    e: &effects::Effects,
    plan: &effects::EffectsPlan,
    out: &Path,
    pip_len: f64,
    tolerance: u8,
) -> ComboStats {
    let keep = kept_ranges(edit, duration);
    let expected: Vec<(usize, f64)> = source.0.iter().enumerate().filter_map(|(n, t)| result_time(&keep, *t).map(|(r, _)| (n, r))).collect();
    let (times, frames, size) = decoded_frames(ff, out);
    assert_eq!([size.0, size.1], plan.size, "{label}: size");
    let code_at: Vec<(f64, f64)> = (0..CODE_BITS).map(|i| result_point(plan, src, (SOURCE_CODE.0 + 32 * i + 16) as f64, (SOURCE_CODE.1 + 16) as f64)).collect();
    if frames.len() != expected.len() {
        let got: Vec<(String, u32)> = frames.iter().zip(&times).map(|(f, t)| (format!("{t:.3}"), read_code(f, size.0, size.1, &code_at))).collect();
        panic!("{label}: {} frames, want {} — (time, source frame): {got:?}", frames.len(), expected.len());
    }
    let quarter = plan.turn % 2 == 1;
    let (mosaic, blur, pip, logo) = (&e.masks[0], &e.masks[1], &e.overlays[0], &e.overlays[1]);
    let pip_r = pip_box(quarter);
    let gaps: Vec<f64> = source.0.windows(2).map(|w| w[1] - w[0]).collect();
    let (longest, constant) = (gaps.iter().cloned().fold(0.0, f64::max), gaps.iter().all(|g| (g - gaps[0]).abs() < 1e-3));
    let (mut piece, mut shift) = (None::<f64>, 0.0);
    let pip_seen = seen_rect(plan, src, pip_r);
    let pip_code_at: Vec<(f64, f64)> = (0..CODE_BITS).map(|i| in_pip(pip_seen, PIP_SIZE, (PIP_CODE.0 + 38 * i + 16) as f64, (PIP_CODE.1 + 16) as f64)).collect();
    let logo_seen = seen_rect(plan, src, LOGO_BOX);
    // Source pixels per result pixel: how far from a block edge a pixel must be to compare.
    let crop = plan.crop.unwrap_or(effects::Rect { x: 0, y: 0, width: src.0, height: src.1 });
    let (cw, ch) = if quarter { (crop.height, crop.width) } else { (crop.width, crop.height) };
    let [pw, ph] = plan.picture_size(src);
    let per = (cw as f64 / pw as f64).max(ch as f64 / ph as f64);
    let margin = (3.0 * per).max(6.0);
    let boxes = [MOSAIC_BOX, BLUR_BOX, pip_r, LOGO_BOX, (SOURCE_CODE.0, SOURCE_CODE.1, 32 * CODE_BITS, 32)];
    let mut stats = ComboStats { compared: 0, worst: 0, time_error: 0.0, click_error: 0.0 };
    let on = |range: (f64, f64), t: f64| t >= range.0 - 1e-4 && t < range.1 - 1e-4;
    for (j, (&(n, r), rgb)) in expected.iter().zip(&frames).enumerate() {
        let t = source.0[n];
        let at = format!("{label}: result frame {j} (source frame {n}, {t:.4} s)");
        // The right frame at the right time.
        assert_eq!(read_code(rgb, size.0, size.1, &code_at), source.1[n] & 0xff, "{at}: code");
        let (_, k) = result_time(&keep, t).unwrap();
        let off = times[j] - r;
        if piece == Some(k.start) {
            assert!((off - shift).abs() < 0.0005, "{at}: at {:.4}, {off:+.4} s off the plan, the piece's earlier frames {shift:+.4}", times[j]);
        } else {
            assert!(off > shift - 0.0005 && off < shift + longest, "{at}: the piece starts at {:.4}, {off:+.4} s off the plan (the one before {shift:+.4})", times[j]);
            assert!(!constant || off.abs() < 0.0005, "{at}: at {:.4}, want {r:.4}", times[j]);
            (piece, shift) = (Some(k.start), off);
        }
        stats.time_error = stats.time_error.max(off.abs());
        // Outside the boxes: the source at the plan's place; 맞춤's bars black.
        for oy in (0..size.1).step_by(3) {
            for ox in (0..size.0).step_by(3) {
                let got = px_at(rgb, size.0, ox, oy);
                let Some((sx, sy)) = source_point(plan, src, ox as f64 + 0.5, oy as f64 + 0.5) else {
                    let p = plan.pad.unwrap();
                    let far = |o: u32, start: u32, len: u32| o + 4 < start || o >= start + len + 4;
                    if far(ox, p.x, pw) || far(oy, p.y, ph) {
                        assert!(got.iter().all(|c| *c < 24), "{at}: bar at ({ox}, {oy}) is {got:?}");
                    }
                    continue;
                };
                let (bx, by) = (sx % BLOCK, sy % BLOCK);
                if boxes.iter().any(|b| inside(*b, sx, sy, margin)) || bx < margin || bx > BLOCK - margin || by < margin || by > BLOCK - margin {
                    continue;
                }
                let want = block_color((sx / BLOCK) as usize, (sy / BLOCK) as usize);
                let diff = got.iter().zip(want).map(|(a, b)| a.abs_diff(b)).max().unwrap();
                assert!(diff <= tolerance, "{at}: ({ox}, {oy}) shows {got:?}, the source at ({sx:.1}, {sy:.1}) is {want:?}");
                stats.worst = stats.worst.max(diff);
                stats.compared += 1;
            }
        }
        // 모자이크: cells of other colors while on, the blocks as they are otherwise.
        let change = box_change(rgb, size, plan, src, MOSAIC_BOX, 4.0, margin);
        if on((mosaic.start, mosaic.end), t) {
            assert!(change > 10.0, "{at}: 모자이크 should show (change {change:.1})");
        } else {
            assert!(change < 4.0, "{at}: no 모자이크 (change {change:.1})");
        }
        // 블러: soft edges while on, as sharp as the untouched box otherwise.
        let (blurred, sharp) = (seen_peak_edge(rgb, size, plan, src, BLUR_BOX), seen_peak_edge(rgb, size, plan, src, CONTROL_BOX));
        if on((blur.start, blur.end), t) {
            assert!(f64::from(blurred) < f64::from(sharp) * 0.5, "{at}: 블러 should show (step {blurred} vs {sharp})");
        } else {
            assert!(f64::from(blurred) > f64::from(sharp) * 0.7, "{at}: no 블러 (step {blurred} vs {sharp})");
        }
        // The PiP video: red with its own frame number, upright, following the original.
        let played = pip.offset + (t - pip.start);
        if on((pip.start, pip.end), t) && played < pip_len - 1e-4 {
            let red = px_at(rgb, size.0, in_pip(pip_seen, PIP_SIZE, 160.0, 170.0).0 as u32, in_pip(pip_seen, PIP_SIZE, 160.0, 170.0).1 as u32);
            assert!(near(red, [255, 0, 0], 48), "{at}: the PiP video should show, got {red:?}");
            // Its first frame from where the piece starts in it (after `-ss`), then one per 1/25 s.
            let (_, k) = result_time(&keep, t).unwrap();
            let from = k.start.max(pip.start);
            let offset = pip.offset + (from - pip.start);
            let a = (played * PIP_FPS - 0.01).floor() as i64;
            let b = (offset * PIP_FPS - 0.01).ceil() as i64 + ((t - from) * PIP_FPS + 0.01).floor() as i64;
            let last = (pip_len * PIP_FPS).round() as i64 - 1;
            let got = i64::from(read_code(rgb, size.0, size.1, &pip_code_at));
            assert!((a.min(b).min(last)..=a.max(b).min(last)).contains(&got), "{at}: PiP frame {got}, want {a} (or {b})");
        } else {
            let change = box_change(rgb, size, plan, src, pip_r, 4.0, margin);
            assert!(change < 4.0, "{at}: no PiP video now (change {change:.1})");
        }
        // The logo at 50 %: half way to blue in its middle, the picture through its transparent border.
        let mid = in_pip(logo_seen, (64.0, 64.0), 32.0, 32.0);
        let (sx, sy) = source_point(plan, src, mid.0, mid.1).unwrap();
        let under = block_color((sx / BLOCK) as usize, (sy / BLOCK) as usize);
        let got = px_at(rgb, size.0, mid.0 as u32, mid.1 as u32);
        if on((logo.start, logo.end), t) {
            let want = [under[0] / 2, under[1] / 2, ((u16::from(under[2]) + 255) / 2) as u8];
            assert!(near(got, want, 24), "{at}: logo {got:?}, want about {want:?}");
        } else {
            assert!(near(got, under, tolerance), "{at}: no logo now, got {got:?} over {under:?}");
        }
        let corner = in_pip(logo_seen, (64.0, 64.0), 8.0, 8.0);
        let (sx, sy) = source_point(plan, src, corner.0, corner.1).unwrap();
        let under = block_color((sx / BLOCK) as usize, (sy / BLOCK) as usize);
        assert!(near(px_at(rgb, size.0, corner.0 as u32, corner.1 as u32), under, tolerance), "{at}: the logo's transparent border");
    }
    // The sound: the source's clicks and the PiP video's (at its own time, while it plays), each
    // where the picture shows its source time — the frame showing it, plus how far into that frame.
    let mut heard_at: Vec<f64> = SOURCE_CLICKS.to_vec();
    if pip.audio {
        heard_at.extend(PIP_CLICKS.iter().filter(|p| **p < pip_len).map(|p| pip.start + (p - pip.offset)).filter(|s| on((pip.start, pip.end), *s)));
    }
    let mut want: Vec<f64> = heard_at
        .iter()
        .filter(|s| result_time(&keep, **s).is_some())
        .map(|s| {
            let j = expected.iter().rposition(|&(n, _)| source.0[n] <= s + 1e-4).unwrap();
            times[j] + (s - source.0[expected[j].0])
        })
        .collect();
    want.sort_by(f64::total_cmp);
    // (A piece concat starts late moves its sound with it. Before the encoder kept the frames'
    // times at 90 kHz, the picture was rounded onto its 1/60 s grid and the sound was not: 5.6 ms
    // apart — PROGRESS 알게 된 것 90.)
    let clicks = click_times(ff, out, 0);
    assert_eq!(clicks.len(), want.len(), "{label}: clicks at {clicks:?}, want {want:?}");
    for (got, w) in clicks.iter().zip(&want) {
        stats.click_error = stats.click_error.max((got - w).abs());
        assert!((got - w).abs() < 0.005, "{label}: clicks at {clicks:?}, want {want:?}");
    }
    stats
}

/// Phase 4 완료 기준: 크롭 + 회전 · 뒤집기 + 출력 크기 + 모자이크 · 블러 + PiP 영상(소리) + 로고 in one
/// export, mixed with 자르기 and 구간 제거 (joins in and around every effect's times), at 30, 29.97 and
/// a variable frame rate, through every encoder that works here — the result shows each where and
/// when it was drawn (see [`check_combo`]).
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn all_effects_together_land_where_and_when_drawn() {
    use effects::{Crop, Effects, Fit, Mask, MaskKind, OutputSize, Overlay};
    let ff = ffmpeg_or_panic();
    assert!(ff.features.libx264, "this test needs libx264");
    let ffprobe = PathBuf::from(&ff.ffprobe_path);
    let dir = temp_dir("effects-all");
    let pip = dir.join("카메라 화면 번호.mp4");
    make_coded_pip(&ff, &pip);
    let pip_len = probe::probe_overlay(&ffprobe, &pip).unwrap().duration.unwrap();
    let logo = dir.join("채널 로고.png");
    make_logo(&ff, &logo);
    let layers = |quarter: bool, mosaic: (f64, f64), blur: (f64, f64), pip_at: (f64, f64, f64), logo_at: (f64, f64)| {
        let (px, py, pw, ph) = pip_box(quarter);
        let (lx, ly, lw, lh) = LOGO_BOX;
        let masks = vec![
            Mask { x: MOSAIC_BOX.0, y: MOSAIC_BOX.1, width: MOSAIC_BOX.2, height: MOSAIC_BOX.3, start: mosaic.0, end: mosaic.1, kind: MaskKind::Mosaic, strength: 80 },
            Mask { x: BLUR_BOX.0, y: BLUR_BOX.1, width: BLUR_BOX.2, height: BLUR_BOX.3, start: blur.0, end: blur.1, kind: MaskKind::Blur, strength: 60 },
        ];
        let overlays = vec![
            Overlay { path: pip.display().to_string(), x: px, y: py, width: pw, height: ph, start: pip_at.0, end: pip_at.1, offset: pip_at.2, opacity: 100, audio: true, volume: 100 },
            Overlay { path: logo.display().to_string(), x: lx, y: ly, width: lw, height: lh, start: logo_at.0, end: logo_at.1, offset: 0.0, opacity: 50, audio: false, volume: 100 },
        ];
        (masks, overlays)
    };
    let sources = [("30fps", "30", None), ("29.97fps", "30000/1001", None), ("가변", "60", Some(r"select='lt(mod(n\,5)\,3)'"))];
    let encoders = working_export_encoders(&ff);
    for (rate_label, rate, select) in sources {
        let source = dir.join(format!("효과 원본 {rate_label}.mp4"));
        make_coded_clip(&ff, &source, rate, select);
        let info = probe::probe(&ffprobe, &source).unwrap();
        let src = info.video.as_ref().unwrap().upright_size();
        let (times, frames, _) = decoded_frames(&ff, &source);
        let at: Vec<(f64, f64)> = (0..CODE_BITS).map(|i| ((SOURCE_CODE.0 + 32 * i + 16) as f64, (SOURCE_CODE.1 + 16) as f64)).collect();
        let codes: Vec<u32> = frames.iter().map(|f| read_code(f, src.0, src.1, &at)).collect();
        if select.is_none() {
            assert_eq!(codes, (0..codes.len() as u32).collect::<Vec<_>>(), "{rate_label}: the source's codes");
        }
        // Cut points as the editor sends them: frame starts (the packet times it is given).
        let frame_at = probe::frame_times(&ffprobe, &source, 0).unwrap().frames;
        let f = |k: usize| frame_at[k];
        let cut = |ranges: &[(usize, usize)]| Edit::Cut { removed: ranges.iter().map(|&(a, b)| plan::Range::new(f(a), f(b))).collect() };
        let cases: Vec<(&str, Edit, Effects)> = match rate_label {
            "30fps" => vec![
                (
                    "자르기 0.4–3.6 · 크롭 + 오른쪽 90° + 좌우 + 맞춤 360 × 360",
                    Edit::Trim { start: 0.4, end: 3.6 },
                    {
                        // The PiP's last frame (result 3.17) comes after the picture's last (3.1667).
                        let (masks, overlays) = layers(true, (1.0, 2.0), (0.0, 4.0), (0.61, 3.6, 0.4), (1.4, 4.0));
                        Effects { crop: Some(Crop { x: 64, y: 32, width: 544, height: 416, ratio: None }), turn: 1, flip_h: true, size: OutputSize::Exact { width: 360, height: 360, fit: Fit::Contain }, masks, overlays, ..Effects::default() }
                    },
                ),
                (
                    "구간 제거 3곳 · 4:3 크롭 + 왼쪽 90° + 상하 + 채움 240 × 240",
                    Edit::Cut { removed: vec![plan::Range::new(0.8, 1.2), plan::Range::new(2.0, 2.2), plan::Range::new(3.0, 3.4)] },
                    {
                        let (masks, overlays) = layers(true, (0.6, 1.6), (1.8, 3.2), (0.4, 3.8, 0.8), (0.0, 4.0));
                        Effects { crop: Some(Crop { x: 96, y: 48, width: 448, height: 336, ratio: Some([3, 4]) }), turn: 3, flip_v: true, size: OutputSize::Exact { width: 240, height: 240, fit: Fit::Cover }, masks, overlays, ..Effects::default() }
                    },
                ),
            ],
            "29.97fps" => vec![(
                // The last kept piece starts on a keyframe (frame 60, 2.002 s): MP4 · MOV seeking
                // then starts its sound at the next AAC packet, up to 23 ms late. The PiP's sound is
                // off here, so the source's sound joins the pieces as it is (PROGRESS 알게 된 것 92).
                "구간 제거 2곳 · 16:9 크롭 + 180° + 짧은 변 240 · 키프레임에서 시작하는 조각",
                cut(&[(12, 27), (45, 60)]),
                {
                    let (masks, mut overlays) = layers(false, (0.5, 2.0), (1.2, 3.5), (0.3, 3.3, 0.3), (1.0, 3.0));
                    overlays[0].audio = false;
                    Effects { crop: Some(Crop { x: 0, y: 60, width: 640, height: 360, ratio: Some([16, 9]) }), turn: 2, size: OutputSize::Short { short: 240 }, masks, overlays, ..Effects::default() }
                },
            )],
            _ => vec![(
                // The first kept piece ends after a short gap (1/60 s, the average is 1/36): concat
                // reckons it longer and starts the next one later. The second ends after a long one
                // (3/60 s): reckoned shorter, it ended early before the silent guide track.
                "구간 제거 2곳 · 오른쪽 90° + 짧은 변 240",
                cut(&[(31, 50), (90, 105)]),
                {
                    let (masks, overlays) = layers(true, (0.8, 2.4), (0.0, 4.0), (0.5, 3.5, 0.0), (0.2, 3.9));
                    Effects { turn: 1, size: OutputSize::Short { short: 240 }, masks, overlays, ..Effects::default() }
                },
            )],
        };
        for (case, edit, e) in cases {
            for (encoder, codec) in &encoders {
                let label = format!("{rate_label} {case} → {encoder}");
                let report = || encoders::EncoderReport {
                    tests: vec![encoders::EncoderTest { name: encoder.clone(), codec: codec.clone(), hardware: false, ok: true, millis: 1, error: None }],
                    preferred_hevc: None,
                    preferred_av1: None,
                };
                let (out, summary, plan) = export_with_effects(&ff, &source, edit.clone(), output_for(codec), e.clone(), report);
                let plan = plan.expect("the effects change the picture");
                assert_eq!((summary.picture, summary.masks, summary.overlays, summary.audio), (Some(plan.size), 2, 2, command::AudioPlan::Aac), "{label}");
                let stats = check_combo(&label, &ff, &(times.clone(), codes.clone()), src, info.duration, &edit, &e, &plan, &out, pip_len, 32);
                eprintln!(
                    "{label}: {} × {}, {} pixels, largest difference {}, frame times ±{:.4} s, clicks ±{:.4} s",
                    plan.size[0], plan.size[1], stats.compared, stats.worst, stats.time_error, stats.click_error
                );
                std::fs::remove_file(&out).unwrap();
            }
        }
        std::fs::remove_file(&source).unwrap();
    }
    assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---------------------------------------------------------------------------------------------
// GIF · 프레임레이트

fn export_request(source: &Path, edit: Edit, codec: command::OutputCodec, quality: command::Quality, frame_rate: Option<u32>, gif_short: Option<u32>) -> ExportRequest {
    ExportRequest {
        path: source.display().to_string(),
        edit,
        mode: ExportMode::Precise,
        codec,
        quality,
        effects: Default::default(),
        frame_rate,
        gif_short,
    }
}

/// Which source frame each output frame is (the nearest of every source frame, as shown).
fn picked_frames(ff: &ffmpeg::FfmpegInfo, shown: &[Vec<u8>], out: &Path) -> Vec<usize> {
    picked_every(ff, shown, out, 1)
}

/// [`picked_frames`] for every `stride`-th output frame only (the comparison is slow in a debug build).
fn picked_every(ff: &ffmpeg::FfmpegInfo, shown: &[Vec<u8>], out: &Path, stride: usize) -> Vec<usize> {
    small_frames(ff, out).iter().step_by(stride).map(|f| nearest(shown, f)).collect()
}

/// 편집기 → GIF through the app's queue, in a folder whose name has a `%` and Hangul: 자르기 at 5 fps,
/// 구간 제거 at 5 fps made smaller, the whole file at 원본 (25 fps). Every GIF frame is the source
/// frame at its time (frame-exact like 정밀), lasts 1/fps, the GIF loops, and neither the palette's
/// scratch folder nor a `.part` stays.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn gif_exports_show_the_frames_asked_for_at_their_times_and_leave_nothing() {
    let ff = ffmpeg_or_panic();
    let dir = temp_dir("gif").join("움짤 100%");
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("여행 클립.mp4");
    make_clip(&ff, &source);
    let shown = small_frames(&ff, &source);
    assert_eq!(shown.len(), 250);
    let (info, kf) = opened(&ff, &source);
    let (queue, _jobs, rx) = new_queue(2);
    let add = |edit: Edit, quality, frame_rate, gif_short| {
        let req = export_request(&source, edit, command::OutputCodec::Gif, quality, frame_rate, gif_short);
        let prepared = export::prepare(&ff, &info, &kf, &req, || panic!("a GIF needs no encoder test")).unwrap();
        queue.add(|taken| prepared.into_task(taken)).unwrap()
    };
    let trim = add(Edit::Trim { start: 2.0, end: 5.0 }, command::Quality::Balanced, Some(5), None);
    let cut = add(Edit::Cut { removed: vec![plan::Range::new(3.0, 6.0)] }, command::Quality::Small, Some(5), Some(120));
    let whole = add(Edit::Trim { start: 0.0, end: 10.0 }, command::Quality::High, None, None);
    assert_eq!((trim.info.summary.frame_rate, cut.info.summary.picture, whole.info.summary.frame_rate), (Some(5.0), Some([160, 120]), Some(25.0)));
    let s = wait_queue(&rx, "three GIFs", |s| s.items.iter().all(|i| i.state.finished()));
    for item in &s.items {
        assert_eq!(item.state, ItemState::Done, "{:?} {:?}", item.error, item.log);
    }
    let output = |id: u64| PathBuf::from(&item_of(&s, id).output);
    let check = |out: &Path, name: &str, size: &str, expected: Vec<usize>, step: f64, stride: usize| {
        assert_eq!(out.file_name().unwrap().to_string_lossy(), name);
        assert_eq!(stream_fields(&ff, out, "v:0", "stream=codec_name,width,height"), format!("codec_name=gif {size}"));
        let times: Vec<f64> = video_frames(&ff, out, None).into_iter().map(|(t, _)| t).collect();
        assert_eq!(times.len(), expected.len(), "{name}: frames");
        for (k, t) in times.iter().enumerate() {
            assert!((t - step * k as f64).abs() < 0.006, "{name}: frame {k} at {t}");
        }
        let sampled: Vec<usize> = expected.iter().copied().step_by(stride).collect();
        assert_eq!(picked_every(&ff, &shown, out, stride), sampled, "{name}: which source frames");
        let bytes = std::fs::read(out).unwrap();
        assert!(bytes.windows(11).any(|w| w == b"NETSCAPE2.0"), "{name}: loops");
        bytes.len()
    };
    // 2.0 – 5.0 s at 5 fps: source frames #50, #55 … #120.
    check(&output(trim.id), "여행 클립_trim.gif", "height=240 width=320", (0..15).map(|k| 50 + 5 * k).collect(), 0.2, 1);
    // 3.0 – 6.0 s removed: #0 … #70, then #150 … #245, at 160 × 120.
    let joined: Vec<usize> = (0..15).map(|k| 5 * k).chain((0..20).map(|k| 150 + 5 * k)).collect();
    check(&output(cut.id), "여행 클립_cut.gif", "height=120 width=160", joined, 0.2, 1);
    // The whole file at its own 25 fps: every frame, named after the original.
    check(&output(whole.id), "여행 클립.gif", "height=240 width=320", (0..250).collect(), 0.04, 23);

    // With 화면 효과: a crop, a mosaic box and a logo on the 자르기, 10 fps — the same graph as 정밀
    // with the palette after it.
    let logo = dir.join("로고.png");
    run_ok(&ff, &[os_args(&["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=red:s=64x32", "-frames:v", "1"]), vec![logo.as_os_str().to_owned()]].concat());
    let mut req = export_request(&source, Edit::Trim { start: 1.0, end: 3.0 }, command::OutputCodec::Gif, command::Quality::Balanced, Some(10), None);
    req.effects = effects::Effects {
        crop: Some(effects::Crop { x: 40, y: 0, width: 240, height: 240, ratio: None }),
        masks: vec![effects::Mask { x: 100, y: 100, width: 64, height: 64, start: 0.0, end: 10.0, kind: effects::MaskKind::Mosaic, strength: 50 }],
        overlays: vec![effects::Overlay {
            path: logo.display().to_string(), x: 200, y: 8, width: 64, height: 32, start: 0.0, end: 10.0, offset: 0.0, opacity: 100, audio: false, volume: 100,
        }],
        ..Default::default()
    };
    let prepared = export::prepare(&ff, &info, &kf, &req, || panic!("no encoder test")).unwrap();
    assert_eq!((prepared.summary().picture, prepared.summary().masks, prepared.summary().overlays), (Some([240, 240]), 1, 1));
    let fx = queue.add(|taken| prepared.into_task(taken)).unwrap();
    let s = wait_queue(&rx, "GIF with effects", |s| item_of(s, fx.id).state.finished());
    assert_eq!(item_of(&s, fx.id).state, ItemState::Done, "{:?} {:?}", item_of(&s, fx.id).error, item_of(&s, fx.id).log);
    // (`output` reads the snapshot before this item finished.)
    let out = PathBuf::from(&item_of(&s, fx.id).output);
    assert_eq!(stream_fields(&ff, &out, "v:0", "stream=codec_name,width,height"), "codec_name=gif height=240 width=240");
    assert_eq!(video_frames(&ff, &out, None).len(), 20);
    // The logo's red is at its spot after the crop (x 200 − 40).
    let px = rgb_frame(&ff, out.as_os_str(), None, false);
    let at = |x: usize, y: usize| &px[(y * 240 + x) * 3..(y * 240 + x) * 3 + 3];
    assert!(at(160 + 32, 8 + 16)[0] > 200 && at(160 + 32, 8 + 16)[1] < 60, "{:?}", at(192, 24));
    assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

/// 프레임레이트 on a video: 정밀 at 5 fps keeps every fifth source frame at its own time and the
/// sound in sync; 일괄 변환 at 5 fps the same over the whole file; a cap at the source's rate
/// changes nothing.
#[test]
#[ignore = "needs FFmpeg 6.0+ installed"]
fn frame_rate_cap_keeps_every_nth_frame_and_the_sound_in_sync() {
    let ff = ffmpeg_or_panic();
    let dir = temp_dir("fps");
    let source = dir.join("클릭 25fps.mp4");
    make_clip(&ff, &source);
    let shown = small_frames(&ff, &source);
    let (info, kf) = opened(&ff, &source);
    let (queue, _jobs, rx) = new_queue(1);
    let req = export_request(&source, Edit::Trim { start: 1.0, end: 6.0 }, command::OutputCodec::Source, command::Quality::High, Some(5), None);
    let prepared = export::prepare(&ff, &info, &kf, &req, software_report).unwrap();
    assert_eq!(prepared.summary().frame_rate, Some(5.0));
    let item = queue.add(|taken| prepared.into_task(taken)).unwrap();
    let s = wait_queue(&rx, "export at 5 fps", |s| item_of(s, item.id).state.finished());
    assert_eq!(item_of(&s, item.id).state, ItemState::Done, "{:?}", item_of(&s, item.id).log);
    let out = PathBuf::from(&item_of(&s, item.id).output);
    assert_eq!(picked_frames(&ff, &shown, &out), (0..25).map(|k| 25 + 5 * k).collect::<Vec<_>>());
    assert!(stream_fields(&ff, &out, "v:0", "stream=r_frame_rate").contains("r_frame_rate=5/1"));
    let click = click_time(&ff, &out);
    assert!((click - (CLICK_AT - 1.0)).abs() < 0.03, "click at {click}");

    // A cap at the source's 25 fps: the export as it was.
    let same = export_request(&source, Edit::Trim { start: 1.0, end: 6.0 }, command::OutputCodec::Source, command::Quality::High, Some(25), None);
    assert_eq!(export::prepare(&ff, &info, &kf, &same, software_report).unwrap().summary().frame_rate, None);

    // 일괄 변환 to H.265 at 5 fps.
    let mut settings = convert_settings(
        command::VideoCodec::Hevc, command::Container::Mkv, command::Resolution::Source, command::AudioChoice::CopyOrAac, false, Some("libx265"), None, "_5fps",
    );
    settings.options.frame_rate = Some(5);
    let choice = encoders::pick(&software_report(), command::VideoCodec::Hevc, Some("libx265")).unwrap();
    let prepared = convert::prepare(&ff, &source.display().to_string(), &info, &settings, &choice).unwrap();
    assert_eq!(prepared.task().summary.frame_rate, Some(5.0));
    let task = prepared.into_task(&|_| false).unwrap();
    let queue::Work::Attempts { attempts, total_secs } = task.work else { panic!("one run") };
    let finished = run_attempts(&ff, attempts, total_secs, &task.part, &task.final_path);
    assert!(finished.ok, "{:?} {:?}", finished.error, finished.log);
    let converted = PathBuf::from(finished.output.unwrap());
    assert_eq!(picked_frames(&ff, &shown, &converted), (0..50).map(|k| 5 * k).collect::<Vec<_>>());
    let click = click_time(&ff, &converted);
    assert!((click - CLICK_AT).abs() < 0.03, "click at {click}");
    std::fs::remove_dir_all(&dir).unwrap();
}
