//! Load measurement of 화면 효과 on a long, large source (Phase 4 4단계 완료 기준). Not a test: it
//! needs a big clip (never in the repository — docs/PROGRESS.md says how the 4K one was made).
//!
//!     cargo run --example effects_load -- <4K 원본> <PiP 영상> [시나리오 이름 일부…]
//!     GODSEON_FFMPEG=/path/to/ffmpeg cargo run --example effects_load -- …
//!
//! Each scenario is an export the editor could send, run through the app's own path
//! (`export::prepare` → `into_task` → `JobManager`, first attempt only) with the encoders this
//! machine passes. Printed per scenario: wall time and speed, FFmpeg's peak memory (resident, sampled
//! every 0.5 s), how far the progress bar was at a quarter · half · three quarters of the time, the
//! result's length · frames · size, and the sync over the whole length: the source's clicks (every
//! 10 s at 3 s past) and the PiP video's (every 10 s at 7 s past, mixed in) must land where the cut
//! timeline puts them. The results are deleted afterwards.

use godseon_clip_lib::effects::{Crop, Effects, Mask, MaskKind, OutputSize, Overlay};
use godseon_clip_lib::export::{self, Edit, ExportMode, ExportRequest};
use godseon_clip_lib::jobs::{JobEvent, JobManager};
use godseon_clip_lib::{command, encoders, ffmpeg, plan, probe, queue, util};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

fn os(parts: &[&str]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

/// FFmpeg children of this process: their resident memory, in KiB.
fn ffmpeg_rss_kib() -> u64 {
    let me = std::process::id();
    let out = util::run_capture(Path::new("/bin/ps"), &["-A", "-o", "pid=,ppid=,rss=,comm="], Duration::from_secs(5));
    let Ok(out) = out else { return 0 };
    out.stdout
        .lines()
        .filter_map(|l| {
            let cols: Vec<&str> = l.split_whitespace().collect();
            (cols.len() >= 4 && cols[1].parse::<u32>().ok() == Some(me) && cols[3].ends_with("ffmpeg")).then(|| cols[2].parse::<u64>().unwrap_or(0))
        })
        .sum()
}

/// Clicks (runs of samples above half the loudest) in the first audio track, in seconds.
fn clicks(ff: &ffmpeg::FfmpegInfo, file: &Path) -> Vec<f64> {
    let mut args = os(&["-hide_banner", "-nostdin", "-loglevel", "error", "-i"]);
    args.push(file.as_os_str().to_owned());
    args.extend(os(&["-map", "0:a:0", "-ac", "1", "-ar", "44100", "-f", "s16le", "-"]));
    let out = util::run_capture(Path::new(&ff.ffmpeg_path), &args, Duration::from_secs(600)).unwrap();
    let samples: Vec<u16> = out.stdout_raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs()).collect();
    let loudest = samples.iter().copied().max().unwrap_or(0);
    let mut found = Vec::new();
    let mut last: Option<usize> = None;
    for (i, v) in samples.iter().enumerate() {
        if *v > loudest / 2 {
            if last.is_none_or(|l| i - l > 22050) {
                found.push(i as f64 / 44100.0);
            }
            last = Some(i);
        }
    }
    found
}

fn result_time(keep: &[plan::Range], t: f64) -> Option<f64> {
    let mut before = 0.0;
    for k in keep {
        if t >= k.start && t < k.end {
            return Some(before + t - k.start);
        }
        before += k.len();
    }
    None
}

fn kept(removed: &[plan::Range], duration: f64) -> Vec<plan::Range> {
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

struct Scenario {
    name: &'static str,
    removed: Vec<plan::Range>,
    codec: command::OutputCodec,
    effects: Effects,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (source, camera) = match args.as_slice() {
        [s, c, ..] => (PathBuf::from(s), PathBuf::from(c)),
        _ => {
            eprintln!("usage: effects_load <4K 원본> <PiP 영상> [시나리오 이름 일부…]");
            std::process::exit(2);
        }
    };
    let only: Vec<&str> = args[2..].iter().map(String::as_str).collect();
    let ff = {
        let custom = std::env::var("GODSEON_FFMPEG").ok();
        ffmpeg::locate(custom.as_deref()).info.expect("FFmpeg")
    };
    let ffprobe = PathBuf::from(&ff.ffprobe_path);
    // As the editor opens a file (probe_media): probe, then one packet listing.
    let mut info = probe::probe(&ffprobe, &source).unwrap();
    let times = probe::frame_times(&ffprobe, &source, info.video.as_ref().unwrap().index).unwrap();
    probe::measure_frame_rate(info.video.as_mut().unwrap(), times.frames);
    let keyframes = times.keyframes;
    let v = info.video.clone().unwrap();
    println!("원본: {} × {} {} {:?} {:.0} s, {:.2} GB", v.width, v.height, v.codec, v.pix_fmt, info.duration, std::fs::metadata(&source).unwrap().len() as f64 / 1e9);
    let report = encoders::run(&ff);
    let logo = source.with_file_name("효과 측정 로고.png");
    let mut make = os(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=blue:s=240x240,format=rgba,pad=320:320:40:40:color=white@0", "-frames:v", "1"]);
    make.push(logo.as_os_str().to_owned());
    assert!(util::run_capture(Path::new(&ff.ffmpeg_path), &make, Duration::from_secs(30)).unwrap().success);

    let pip = |x: u32, y: u32, audio: bool| Overlay {
        path: camera.display().to_string(),
        x,
        y,
        width: 640,
        height: 360,
        start: 0.0,
        end: info.duration,
        offset: 0.0,
        opacity: 100,
        audio,
        volume: 100,
    };
    // 넷 모두: a 9:16 box down to 1080 × 1920, a mosaic for 4 minutes and a blur throughout, the
    // camera (with its sound) and a logo over the whole length.
    let all_four = Effects {
        crop: Some(Crop { x: 1312, y: 0, width: 1216, height: 2160, ratio: Some([9, 16]) }),
        size: OutputSize::Short { short: 1080 },
        masks: vec![
            Mask { x: 1500, y: 400, width: 400, height: 300, start: 60.0, end: 300.0, kind: MaskKind::Mosaic, strength: 60 },
            Mask { x: 1400, y: 1500, width: 600, height: 400, start: 0.0, end: info.duration, kind: MaskKind::Blur, strength: 60 },
        ],
        overlays: vec![
            Overlay { x: 1400, y: 1000, ..pip(0, 0, true) },
            Overlay { path: logo.display().to_string(), x: 2200, y: 80, width: 240, height: 240, start: 0.0, end: info.duration, offset: 0.0, opacity: 70, audio: false, volume: 100 },
        ],
        ..Effects::default()
    };
    let three = vec![plan::Range::new(100.0, 130.0), plan::Range::new(250.0, 260.0), plan::Range::new(400.0, 460.0)];
    // 20 cuts of 3 s, one every 30 s: 21 pieces, each with its own input — and each PiP video one
    // more per piece (4 videos: 84).
    let twenty: Vec<plan::Range> = (0..20).map(|k| plan::Range::new(15.0 + 30.0 * k as f64, 18.0 + 30.0 * k as f64)).collect();
    let four_pips = Effects { overlays: vec![pip(0, 0, true), pip(3200, 0, false), pip(0, 1800, false), pip(3200, 1800, false)], ..Effects::default() };
    let scenarios = vec![
        Scenario { name: "기준: 구간 제거 3곳, 효과 없음 → H.265", removed: three.clone(), codec: command::OutputCodec::Hevc, effects: Effects::default() },
        Scenario { name: "넷 모두 + 구간 제거 3곳 → H.265", removed: three.clone(), codec: command::OutputCodec::Hevc, effects: all_four.clone() },
        Scenario { name: "넷 모두 + 구간 제거 3곳 → AV1", removed: three.clone(), codec: command::OutputCodec::Av1, effects: all_four.clone() },
        Scenario { name: "기준: 구간 제거 20곳, 효과 없음 → H.265", removed: twenty.clone(), codec: command::OutputCodec::Hevc, effects: Effects::default() },
        Scenario { name: "구간 제거 20곳 × PiP 영상 4개 → H.265", removed: twenty.clone(), codec: command::OutputCodec::Hevc, effects: four_pips },
    ];

    for s in scenarios.into_iter().filter(|s| only.is_empty() || only.iter().any(|o| s.name.contains(o))) {
        let req = ExportRequest {
            path: source.display().to_string(),
            edit: Edit::Cut { removed: s.removed.clone() },
            mode: ExportMode::Precise,
            codec: s.codec,
            quality: command::Quality::Balanced,
            effects: s.effects.clone(),
            frame_rate: None,
            gif_short: None,
        };
        let prepared = export::prepare(&ff, &info, &keyframes, &req, || report.clone()).unwrap();
        let summary = prepared.summary().clone();
        let task = prepared.into_task(&|_| false).unwrap();
        let queue::Work::Attempts { mut attempts, total_secs } = task.work else { panic!("one encode") };
        attempts.truncate(1);
        let inputs = attempts[0].args.iter().filter(|a| *a == "-i").count();
        println!("\n== {} — {} ({} 입력, 결과 {:.0} s, {:?})", s.name, attempts[0].label, inputs, total_secs, summary.picture);

        let jobs = JobManager::default();
        let (tx, rx) = mpsc::channel();
        let progress: Arc<Mutex<Vec<(f64, f64)>>> = Arc::default();
        let started = Instant::now();
        let peak = Arc::new(AtomicU64::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let sampler = {
            let (peak, done) = (peak.clone(), done.clone());
            std::thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    peak.fetch_max(ffmpeg_rss_kib(), Ordering::Relaxed);
                    std::thread::sleep(Duration::from_millis(500));
                }
            })
        };
        let seen = progress.clone();
        jobs.start_attempts(Path::new(&ff.ffmpeg_path), attempts, total_secs, task.part.clone(), task.final_path.clone(), move |e| {
            if let JobEvent::Progress(p) = &e {
                seen.lock().unwrap().push((started.elapsed().as_secs_f64(), p.percent));
            }
            let _ = tx.send(e);
        })
        .unwrap();
        let finished = loop {
            if let JobEvent::Finished(f) = rx.recv_timeout(Duration::from_secs(3600)).expect("no event within an hour") {
                break f;
            }
        };
        let wall = started.elapsed().as_secs_f64();
        done.store(true, Ordering::Relaxed);
        sampler.join().unwrap();
        assert!(finished.ok, "{}: {:?}", s.name, finished.error);
        let at = |share: f64| {
            let p = progress.lock().unwrap();
            p.iter().rev().find(|(t, _)| *t <= wall * share).map_or(0.0, |(_, pc)| *pc)
        };
        let out = &task.final_path;
        let probed = probe::probe(&ffprobe, out).unwrap();
        let frames = probe::frame_times(&ffprobe, out, probed.video.as_ref().unwrap().index).unwrap().frames.len();
        println!(
            "시간 {:.0} s (결과 길이의 {:.2}배속), FFmpeg 최대 메모리 {:.0} MB, 진행률 1/4 · 1/2 · 3/4 시점 {:.0} · {:.0} · {:.0} %",
            wall,
            total_secs / wall,
            peak.load(Ordering::Relaxed) as f64 / 1024.0,
            at(0.25),
            at(0.5),
            at(0.75)
        );
        let size = std::fs::metadata(out).unwrap().len();
        println!("결과: {:.3} s, {frames} 프레임, {:.0} MB, {}", probed.duration, size as f64 / 1e6, probed.video.as_ref().map(|v| format!("{} × {} {} {:?}", v.width, v.height, v.codec, v.pix_fmt)).unwrap_or_default());
        // Sync over the whole length: every click where the cut timeline puts it.
        let keep = kept(&s.removed, info.duration);
        let mut want: Vec<f64> = (0..60).map(|k| 3.0 + 10.0 * k as f64).filter_map(|t| result_time(&keep, t)).collect();
        if s.effects.overlays.iter().any(|o| o.audio) {
            want.extend((0..60).map(|k| 7.0 + 10.0 * k as f64).filter_map(|t| result_time(&keep, t)));
        }
        want.sort_by(f64::total_cmp);
        let got = clicks(&ff, out);
        let worst = got.iter().zip(&want).map(|(g, w)| (g - w).abs()).fold(0.0, f64::max);
        println!("딸깍 {} / {}개, 가장 큰 차이 {:.1} ms", got.len(), want.len(), worst * 1000.0);
        assert_eq!(got.len(), want.len(), "{}: clicks", s.name);
        assert!(worst < 0.015, "{}: a click is {:.1} ms off", s.name, worst * 1000.0);
        std::fs::remove_file(out).unwrap();
    }
    let _ = std::fs::remove_file(&logo);
}
