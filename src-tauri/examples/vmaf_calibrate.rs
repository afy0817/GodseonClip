//! VMAF calibration of the quality presets (Phase 2 4단계). Not a test: it needs FFmpeg with the
//! `libvmaf` filter and a folder of sample clips (never in the repository — see docs/PROGRESS.md for
//! the public clips used).
//!
//!     cargo run --example vmaf_calibrate -- <샘플 폴더> [결과.csv]
//!     GODSEON_FFMPEG=/path/to/ffmpeg cargo run --example vmaf_calibrate -- …
//!     GODSEON_VMAF_ENCODERS=hevc_nvenc,hevc_qsv cargo run --example vmaf_calibrate -- …
//!
//! Every sample is encoded by every encoder (default: each export encoder that passes the
//! three-frame test here; libx265 is always added as the anchor) across a sweep of quality values,
//! with the app's own arguments (`command::video_encoder_args_at`), then scored with VMAF
//! (vmaf_v0.6.1, both sides scaled to 1920 wide when the sample is narrower). One CSV row per encode is
//! appended as it finishes; a rerun skips rows that are already there, so new samples or encoders
//! can be added later (on Windows for NVENC · QSV · AMF). Finally, for each preset, it prints the
//! x265 anchor (the table's CRF 20 · 24 · 28) and, for every other encoder, the value whose VMAF
//! is closest to x265 on the same samples.

use godseon_clip_lib::{command, encoders, ffmpeg, probe, util};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const ANCHOR: &str = "libx265";
const PRESETS: [&str; 3] = ["고화질", "균형", "작은 용량"];

/// Quality values to try per encoder. Each must include that encoder's current preset values.
fn sweep(encoder: &str) -> Vec<u32> {
    match encoder {
        "libx265" => (16..=32).step_by(2).collect(),
        "libx264" => (14..=28).collect(),
        "libsvtav1" => (20..=52).step_by(2).collect(),
        "hevc_videotoolbox" => (36..=84).step_by(2).collect(),
        "hevc_nvenc" | "hevc_qsv" | "hevc_amf" => (14..=38).step_by(2).collect(),
        _ => Vec::new(),
    }
}

struct Sample {
    name: String,
    path: PathBuf,
    duration: f64,
    width: u32,
}

#[derive(Clone, Copy)]
struct Score {
    vmaf: f64,
    kbps: f64,
}

type Key = (String, String, u32); // (encoder, sample, value)

fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(dir) = args.next().map(PathBuf::from) else {
        eprintln!("usage: cargo run --example vmaf_calibrate -- <샘플 폴더> [결과.csv]");
        std::process::exit(2);
    };
    let csv = args.next().map(PathBuf::from).unwrap_or_else(|| dir.join("vmaf.csv"));

    let located = ffmpeg::locate(std::env::var("GODSEON_FFMPEG").ok().as_deref());
    let ff = match located.info {
        Some(info) if info.meets_minimum => info,
        _ => panic!("FFmpeg 6.0+ not found: {:?}", located.problem),
    };
    let ffmpeg_path = PathBuf::from(&ff.ffmpeg_path);

    let samples = load_samples(Path::new(&ff.ffprobe_path), &dir);
    assert!(!samples.is_empty(), "no video files in {}", dir.display());
    let encoders = pick_encoders(&ff);
    println!("FFmpeg {} · samples {} · encoders {}", ff.version, samples.len(), encoders.join(", "));

    let mut scores = read_csv(&csv);
    let work = std::env::temp_dir().join(format!("godseon-vmaf-{}", std::process::id()));
    std::fs::create_dir_all(&work).unwrap();
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(&csv).unwrap();
    if out.metadata().unwrap().len() == 0 {
        writeln!(out, "encoder,sample,value,kbps,vmaf,encode_secs").unwrap();
    }

    for encoder in &encoders {
        for value in sweep(encoder) {
            for s in &samples {
                let key = (encoder.clone(), s.name.clone(), value);
                if scores.contains_key(&key) {
                    continue;
                }
                let encoded = work.join(format!("{encoder}-{value}.mp4"));
                let started = Instant::now();
                if let Err(e) = encode(&ffmpeg_path, s, encoder, value, &encoded) {
                    eprintln!("{encoder} {value} {}: encode failed: {e}", s.name);
                    continue;
                }
                let secs = started.elapsed().as_secs_f64();
                let kbps = std::fs::metadata(&encoded).unwrap().len() as f64 * 8.0 / s.duration / 1000.0;
                let vmaf = match vmaf(&ffmpeg_path, &encoded, s) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("{encoder} {value} {}: VMAF failed: {e}", s.name);
                        continue;
                    }
                };
                let _ = std::fs::remove_file(&encoded);
                writeln!(out, "{encoder},{},{value},{kbps:.0},{vmaf:.3},{secs:.2}", s.name).unwrap();
                out.flush().unwrap();
                println!("{encoder:>18} {value:>3} {:<18} {kbps:>8.0} kbps  VMAF {vmaf:6.2}  {secs:5.1} s", s.name);
                scores.insert(key, Score { vmaf, kbps });
            }
        }
    }
    let _ = std::fs::remove_dir_all(&work);

    let names: Vec<String> = samples.iter().map(|s| s.name.clone()).collect();
    report(&scores, &encoders, &names);
}

fn load_samples(ffprobe: &Path, dir: &Path) -> Vec<Sample> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            matches!(ext.as_str(), "mkv" | "mp4" | "mov" | "y4m" | "webm")
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let info = probe::probe(ffprobe, &path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let v = info.video.expect("video stream");
            Sample {
                name: path.file_stem().unwrap().to_string_lossy().into_owned(),
                duration: info.duration,
                width: v.width,
                path,
            }
        })
        .collect()
}

fn pick_encoders(ff: &ffmpeg::FfmpegInfo) -> Vec<String> {
    let mut list: Vec<String> = match std::env::var("GODSEON_VMAF_ENCODERS") {
        Ok(v) => v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Err(_) => encoders::run(ff)
            .tests
            .into_iter()
            .filter(|t| t.ok && command::exports_with(&t.name))
            .map(|t| t.name)
            .collect(),
    };
    list.retain(|e| e != ANCHOR);
    list.insert(0, ANCHOR.to_string());
    for e in &list {
        let values = command::quality_values(e).unwrap_or_else(|| panic!("{e}: no quality values in command.rs"));
        assert!(values.iter().all(|v| sweep(e).contains(v)), "{e}: sweep misses a preset value {values:?}");
    }
    list
}

/// The export's video settings for one value (8-bit 4:2:0: the samples are 8-bit), MP4 like the app.
fn encode(ffmpeg: &Path, s: &Sample, encoder: &str, value: u32, out: &Path) -> Result<(), String> {
    let mut a: Vec<OsString> = ["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"].map(OsString::from).to_vec();
    a.push(s.path.as_os_str().to_owned());
    a.extend(["-map", "0:v:0"].map(OsString::from));
    a.extend(command::video_encoder_args_at(encoder, value, false).map_err(|e| e.to_string())?);
    if encoder.starts_with("hevc") || encoder == "libx265" {
        a.extend(["-tag:v", "hvc1"].map(OsString::from));
    }
    a.extend(["-fps_mode", "passthrough", "-an"].map(OsString::from));
    a.push(out.as_os_str().to_owned());
    let r = util::run_capture(ffmpeg, &a, Duration::from_secs(1800)).map_err(|e| e.to_string())?;
    if r.success { Ok(()) } else { Err(r.stderr.lines().last().unwrap_or("").to_string()) }
}

/// Mean VMAF (vmaf_v0.6.1, a 1080p viewing model) of `encoded` against the sample. Narrower samples
/// are scaled up to 1920 wide on both sides (aspect kept: letterboxed films stay as they are).
fn vmaf(ffmpeg: &Path, encoded: &Path, s: &Sample) -> Result<f64, String> {
    let fit = if s.width >= 1920 { "" } else { "scale=1920:-2:flags=bicubic," };
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    // Pair frames by number, not time: the export keeps every frame (`-fps_mode passthrough`), but
    // MKV stores times in whole ms (24 fps frame 1 = 0.042) and MP4 exactly (0.041667), so pairing
    // by time matched each encoded frame with the reference frame before it (measured: VMAF 72–77
    // instead of 95+ at x265 CRF 20).
    let graph = format!(
        "[0:v]{fit}format=yuv420p,settb=1,setpts=N[d];[1:v]{fit}format=yuv420p,settb=1,setpts=N[r];\
         [d][r]libvmaf=model=version=vmaf_v0.6.1:n_threads={threads}"
    );
    let mut a: Vec<OsString> = ["-hide_banner", "-loglevel", "info", "-nostdin", "-i"].map(OsString::from).to_vec();
    a.push(encoded.as_os_str().to_owned());
    a.push("-i".into());
    a.push(s.path.as_os_str().to_owned());
    a.extend(["-lavfi", &graph, "-f", "null", "-"].map(OsString::from));
    let r = util::run_capture(ffmpeg, &a, Duration::from_secs(1800)).map_err(|e| e.to_string())?;
    r.stderr
        .lines()
        .find_map(|l| l.split("VMAF score:").nth(1).and_then(|v| v.trim().parse().ok()))
        .ok_or_else(|| r.stderr.lines().last().unwrap_or("no VMAF score").to_string())
}

fn read_csv(path: &Path) -> BTreeMap<Key, Score> {
    let Ok(text) = std::fs::read_to_string(path) else { return BTreeMap::new() };
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let c: Vec<&str> = l.split(',').collect();
            let score = Score { kbps: c.get(3)?.parse().ok()?, vmaf: c.get(4)?.parse().ok()? };
            Some(((c[0].to_string(), c[1].to_string(), c[2].parse().ok()?), score))
        })
        .collect()
}

fn report(scores: &BTreeMap<Key, Score>, encoders: &[String], samples: &[String]) {
    let get = |e: &str, s: &str, v: u32| scores.get(&(e.to_string(), s.to_string(), v)).copied();
    let complete = |e: &str, v: u32| samples.iter().all(|s| get(e, s, v).is_some());
    let mean = |xs: &[f64]| xs.iter().sum::<f64>() / xs.len() as f64;

    println!("\n## 평균 VMAF · 비트레이트 (샘플 {}개)\n", samples.len());
    for e in encoders {
        println!("{e}:");
        for v in sweep(e).into_iter().filter(|&v| complete(e, v)) {
            let vm: Vec<f64> = samples.iter().map(|s| get(e, s, v).unwrap().vmaf).collect();
            let kb: Vec<f64> = samples.iter().map(|s| get(e, s, v).unwrap().kbps).collect();
            let lo = vm.iter().cloned().fold(f64::INFINITY, f64::min);
            println!("  {v:>3}  VMAF {:6.2} (최저 {lo:6.2})  {:>7.0} kbps", mean(&vm), mean(&kb));
        }
    }

    let anchors = command::quality_values(ANCHOR).unwrap();
    println!("\n## 프리셋 보정 (기준 {ANCHOR} CRF {anchors:?})\n");
    println!("| 프리셋 | 인코더 | 지금 값 | 지금 ΔVMAF | 맞춘 값 | ΔVMAF (샘플별 범위) | 용량 (x265 = 1) |");
    println!("|---|---|---|---|---|---|---|");
    let current_sets: BTreeSet<&str> = encoders.iter().map(String::as_str).collect();
    for (i, preset) in PRESETS.iter().enumerate() {
        let a = anchors[i];
        if !complete(ANCHOR, a) {
            continue;
        }
        let target: Vec<Score> = samples.iter().map(|s| get(ANCHOR, s, a).unwrap()).collect();
        let tv: Vec<f64> = target.iter().map(|t| t.vmaf).collect();
        println!("| {preset} | {ANCHOR} (기준) | {a} | VMAF {:.2} | {a} | 0 | 1.00 |", mean(&tv));
        for e in encoders.iter().filter(|e| e.as_str() != ANCHOR && current_sets.contains(e.as_str())) {
            let delta = |v: u32| -> Option<(f64, f64, f64, f64)> {
                if !complete(e, v) {
                    return None;
                }
                let d: Vec<f64> = samples.iter().zip(&target).map(|(s, t)| get(e, s, v).unwrap().vmaf - t.vmaf).collect();
                let r: Vec<f64> = samples.iter().zip(&target).map(|(s, t)| get(e, s, v).unwrap().kbps / t.kbps).collect();
                let lo = d.iter().cloned().fold(f64::INFINITY, f64::min);
                let hi = d.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                Some((mean(&d), lo, hi, mean(&r)))
            };
            let now = command::quality_values(e).unwrap()[i];
            let now_d = delta(now).map(|(d, ..)| format!("{d:+.2}")).unwrap_or_else(|| "—".into());
            let best = sweep(e)
                .into_iter()
                .filter_map(|v| delta(v).map(|d| (v, d)))
                .min_by(|x, y| x.1.0.abs().total_cmp(&y.1.0.abs()));
            match best {
                Some((v, (d, lo, hi, r))) => {
                    println!("| {preset} | {e} | {now} | {now_d} | **{v}** | {d:+.2} ({lo:+.2} ~ {hi:+.2}) | {r:.2} |")
                }
                None => println!("| {preset} | {e} | {now} | {now_d} | — | — | — |"),
            }
        }
    }
}
