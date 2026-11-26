//! Media information from ffprobe, plus the keyframe list used to show where a lossless cut lands.

use crate::msg::Msg;
use crate::util::{last_line, run_capture};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VideoStream {
    pub index: u32,
    pub codec: String,
    pub profile: Option<String>,
    pub width: u32,
    pub height: u32,
    pub start_time: Option<f64>,
    /// Reorder delay in frames; > 0 means B-frames (decode order ≠ display order).
    pub has_b_frames: u32,
    /// Nominal frame rate (r_frame_rate).
    pub fps: Option<f64>,
    pub avg_fps: Option<f64>,
    pub pix_fmt: Option<String>,
    /// Color description (ffprobe names, e.g. bt709 · bt2020), kept when re-encoding.
    pub color_primaries: Option<String>,
    pub color_transfer: Option<String>,
    pub color_space: Option<String>,
    pub hdr: bool,
    pub rotation: Option<f64>,
    /// Variable frame rate: r_frame_rate and avg_frame_rate disagree (a phone recording), or — set
    /// by [`measure_frame_rate`] — the frames' own times do not come at a steady pace (MKV gives both
    /// rates the same average, so only the times tell).
    pub vfr_suspect: bool,
    /// Every frame's time, sorted, when the rate is variable: the editor puts IN / OUT and frame
    /// steps on these instead of a k / fps grid. None for a steady rate (and before measuring).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_times: Option<Vec<f64>>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioStream {
    pub index: u32,
    pub codec: String,
    pub start_time: Option<f64>,
    pub channels: Option<u32>,
    pub sample_rate: Option<u32>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub path: String,
    pub file_name: String,
    pub size_bytes: Option<u64>,
    pub duration: f64,
    pub format_name: String,
    pub bit_rate: Option<u64>,
    pub video: Option<VideoStream>,
    pub audio: Vec<AudioStream>,
    pub subtitle_count: usize,
    pub other_count: usize,
}

pub fn probe(ffprobe: &Path, file: &Path) -> Result<MediaInfo, Msg> {
    let args = [
        "-v".as_ref(),
        "error".as_ref(),
        "-print_format".as_ref(),
        "json".as_ref(),
        "-show_format".as_ref(),
        "-show_streams".as_ref(),
        file.as_os_str(),
    ];
    let out = run_capture(ffprobe, &args, Duration::from_secs(60)).map_err(|e| ran(ffprobe, e))?;
    if !out.success {
        return Err(probe_error_text(last_line(&out.stderr, 300).as_deref(), file));
    }
    let json: Value =
        serde_json::from_str(&out.stdout).map_err(|e| Msg::ProbeOutputUnreadable { detail: e.to_string() })?;
    parse_probe(&json, file)
}

fn ran(ffprobe: &Path, e: std::io::Error) -> Msg {
    Msg::RunFailed { program: ffprobe.display().to_string(), detail: e.to_string() }
}

/// ffprobe's last stderr line ("{path}: {reason}") as a message for the user. The path is already on
/// screen, so only the reason is kept, as its own message when it is a common one.
pub fn probe_error_text(last_line: Option<&str>, file: &Path) -> Msg {
    let line = last_line.unwrap_or("").trim();
    let prefix = format!("{}: ", file.display());
    let reason = line.strip_prefix(prefix.as_str()).unwrap_or(line).trim();
    if reason.is_empty() {
        Msg::ProbeFailed { reason: None }
    } else if reason.contains("Invalid data found when processing input") {
        Msg::NotMedia
    } else if reason.contains("Permission denied") || reason.contains("Operation not permitted") {
        Msg::PermissionDenied
    } else if reason.contains("No such file or directory") {
        Msg::FileNotFound { path: None }
    } else {
        Msg::ProbeFailed { reason: Some(reason.to_string()) }
    }
}

/// A file put over the picture (PiP): a video or a still image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OverlayKind {
    Video,
    Image,
}

/// What the editor and the export need to know about a file put over the picture.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlaySource {
    pub path: String,
    pub file_name: String,
    pub kind: OverlayKind,
    /// Upright size (a phone video turned as players show it).
    pub width: u32,
    pub height: u32,
    /// Video length in seconds; None for an image.
    pub duration: Option<f64>,
    /// The video has sound that can be mixed in.
    pub has_audio: bool,
}

/// Reads a file to put over the picture: a video (any [`probe`]-able file with a video track) or a
/// still image (PNG — transparency kept — JPEG, WebP… that [`probe`] refuses as media to edit).
pub fn probe_overlay(ffprobe: &Path, file: &Path) -> Result<OverlaySource, Msg> {
    let args = ["-v".as_ref(), "error".as_ref(), "-print_format".as_ref(), "json".as_ref(), "-show_format".as_ref(), "-show_streams".as_ref(), file.as_os_str()];
    let out = run_capture(ffprobe, &args, Duration::from_secs(60)).map_err(|e| ran(ffprobe, e))?;
    if !out.success {
        return Err(probe_error_text(last_line(&out.stderr, 300).as_deref(), file));
    }
    let json: Value = serde_json::from_str(&out.stdout).map_err(|e| Msg::ProbeOutputUnreadable { detail: e.to_string() })?;
    parse_overlay(&json, file)
}

pub fn parse_overlay(json: &Value, file: &Path) -> Result<OverlaySource, Msg> {
    let file_name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let format_name = json.get("format").and_then(|f| str_of(f, "format_name")).unwrap_or_default();
    if is_still_image(&format_name) {
        let streams = json.get("streams").and_then(Value::as_array).cloned().unwrap_or_default();
        let s = streams.iter().find(|s| s.get("codec_type").and_then(Value::as_str) == Some("video")).ok_or(Msg::NoVideoTrack)?;
        let size = |k: &str| s.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
        return Ok(OverlaySource {
            path: file.display().to_string(),
            file_name,
            kind: OverlayKind::Image,
            width: size("width"),
            height: size("height"),
            duration: None,
            has_audio: false,
        });
    }
    let info = parse_probe(json, file)?;
    let v = info.video.as_ref().ok_or(Msg::NoVideoTrack)?;
    let (width, height) = v.upright_size();
    Ok(OverlaySource {
        path: info.path.clone(),
        file_name,
        kind: OverlayKind::Video,
        width,
        height,
        duration: Some(info.duration),
        has_audio: !info.audio.is_empty(),
    })
}

/// Still-image demuxers: `image2` (JPEG, PNG sequences…) and the single-image `*_pipe` readers
/// (`png_pipe`, `webp_pipe`…). ffprobe reads a JPEG as a one-frame, 0.04-second "video".
fn is_still_image(format_name: &str) -> bool {
    format_name == "image2" || format_name.ends_with("_pipe")
}

fn num_str<T: std::str::FromStr>(v: &Value) -> Option<T> {
    match v {
        Value::String(s) => s.trim().parse().ok(),
        Value::Number(n) => n.to_string().parse().ok(),
        _ => None,
    }
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty() && *s != "unknown").map(String::from)
}

/// "30000/1001" → 29.97; "0/0" → None.
pub fn parse_rate(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/').unwrap_or((s, "1"));
    let n: f64 = n.trim().parse().ok()?;
    let d: f64 = d.trim().parse().ok()?;
    (d > 0.0 && n > 0.0).then(|| n / d)
}

/// More than 8 bits per sample ("yuv420p10le", "p010le", "yuv422p12le"…): re-encode in 10 bits.
pub fn is_high_bit_depth(pix_fmt: &str) -> bool {
    let f = pix_fmt.trim_end_matches("le").trim_end_matches("be");
    ["p10", "p12", "p14", "p16"].iter().any(|d| f.contains(d))
        || ["p010", "p016", "p210", "p216", "p410", "p416", "gray1"].iter().any(|d| f.starts_with(d))
}

impl VideoStream {
    /// 10-bit or more, or HDR: H.265 · AV1 output keeps 10 bits.
    pub fn wants_ten_bit(&self) -> bool {
        self.hdr || self.pix_fmt.as_deref().is_some_and(is_high_bit_depth)
    }

    /// Width and height as players show them: FFmpeg turns phone video upright (autorotate) before
    /// any filter runs, so crops and sizes are in this picture.
    pub fn upright_size(&self) -> (u32, u32) {
        let quarter_turn = self.rotation.is_some_and(|r| ((r / 90.0).round() as i64).rem_euclid(2) == 1);
        if quarter_turn { (self.height, self.width) } else { (self.width, self.height) }
    }
}

pub fn parse_probe(json: &Value, file: &Path) -> Result<MediaInfo, Msg> {
    let format = json.get("format").cloned().unwrap_or(Value::Null);
    let streams = json.get("streams").and_then(Value::as_array).cloned().unwrap_or_default();

    let mut video = None;
    let mut audio = Vec::new();
    let mut subtitle_count = 0;
    let mut other_count = 0;
    let mut stream_duration: Option<f64> = None;

    for s in &streams {
        let index = s.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
        let kind = s.get("codec_type").and_then(Value::as_str).unwrap_or("");
        let attached_pic = s
            .get("disposition")
            .and_then(|d| d.get("attached_pic"))
            .and_then(Value::as_u64)
            == Some(1);
        match kind {
            "video" if !attached_pic && video.is_none() => {
                let fps = s.get("r_frame_rate").and_then(Value::as_str).and_then(parse_rate);
                let avg_fps = s.get("avg_frame_rate").and_then(Value::as_str).and_then(parse_rate);
                let vfr_suspect = match (fps, avg_fps) {
                    (Some(a), Some(b)) => (a - b).abs() / a > 0.01,
                    _ => false,
                };
                let color_transfer = str_of(s, "color_transfer");
                let hdr = matches!(color_transfer.as_deref(), Some("smpte2084") | Some("arib-std-b67"));
                let rotation = s
                    .get("side_data_list")
                    .and_then(Value::as_array)
                    .and_then(|l| l.iter().find_map(|d| d.get("rotation").and_then(Value::as_f64)))
                    .or_else(|| s.get("tags").and_then(|t| t.get("rotate")).and_then(num_str));
                stream_duration = s.get("duration").and_then(num_str);
                video = Some(VideoStream {
                    index,
                    codec: str_of(s, "codec_name").unwrap_or_else(|| "?".into()),
                    profile: str_of(s, "profile"),
                    width: s.get("width").and_then(Value::as_u64).unwrap_or(0) as u32,
                    height: s.get("height").and_then(Value::as_u64).unwrap_or(0) as u32,
                    start_time: s.get("start_time").and_then(num_str),
                    has_b_frames: s.get("has_b_frames").and_then(Value::as_u64).unwrap_or(0) as u32,
                    fps,
                    avg_fps,
                    pix_fmt: str_of(s, "pix_fmt"),
                    color_primaries: str_of(s, "color_primaries"),
                    color_transfer,
                    color_space: str_of(s, "color_space"),
                    hdr,
                    rotation,
                    vfr_suspect,
                    frame_times: None,
                });
            }
            "audio" => audio.push(AudioStream {
                index,
                codec: str_of(s, "codec_name").unwrap_or_else(|| "?".into()),
                start_time: s.get("start_time").and_then(num_str),
                channels: s.get("channels").and_then(Value::as_u64).map(|c| c as u32),
                sample_rate: s.get("sample_rate").and_then(num_str),
                // "und" (undetermined) carries no information.
                language: s.get("tags").and_then(|t| str_of(t, "language")).filter(|l| l != "und"),
            }),
            "subtitle" => subtitle_count += 1,
            _ => other_count += 1,
        }
    }

    let format_name = str_of(&format, "format_name").unwrap_or_default();
    if is_still_image(&format_name) {
        return Err(Msg::ImageFile);
    }
    if video.is_none() && audio.is_empty() {
        return Err(Msg::NoTracks);
    }

    let duration = format
        .get("duration")
        .and_then(num_str)
        .or(stream_duration)
        .ok_or(Msg::DurationUnknown)?;

    Ok(MediaInfo {
        path: file.display().to_string(),
        file_name: file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        size_bytes: format.get("size").and_then(num_str),
        duration,
        format_name,
        bit_rate: format.get("bit_rate").and_then(num_str),
        video,
        audio,
        subtitle_count,
        other_count,
    })
}

/// Keyframe times (seconds) of one stream, read from packet flags — no decoding, so it is fast.
pub fn keyframes(ffprobe: &Path, file: &Path, stream_index: u32) -> Result<Vec<f64>, Msg> {
    frame_times(ffprobe, file, stream_index).map(|t| t.keyframes)
}

/// Keyframe times and every frame's time of one video stream, sorted (display order).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameTimes {
    pub keyframes: Vec<f64>,
    pub frames: Vec<f64>,
}

/// [`FrameTimes`] of one stream from its packets' times and flags — no decoding, so it is fast.
pub fn frame_times(ffprobe: &Path, file: &Path, stream_index: u32) -> Result<FrameTimes, Msg> {
    let spec = stream_index.to_string();
    let args = [
        "-v".as_ref(),
        "error".as_ref(),
        "-select_streams".as_ref(),
        spec.as_ref(),
        "-show_entries".as_ref(),
        "packet=pts_time,flags".as_ref(),
        "-of".as_ref(),
        "csv=p=0".as_ref(),
        file.as_os_str(),
    ];
    let out = run_capture(ffprobe, &args, Duration::from_secs(180)).map_err(|e| ran(ffprobe, e))?;
    if !out.success {
        return Err(Msg::line_or(last_line(&out.stderr, 300), Msg::KeyframesFailed));
    }
    Ok(FrameTimes { keyframes: parse_keyframes(&out.stdout), frames: parse_frame_times(&out.stdout) })
}

/// Marks `video` variable-rate when its frames' times are not steady, and gives a variable-rate
/// stream its frame times (for the editor). Keeps an earlier r_frame_rate ≠ avg_frame_rate verdict.
pub fn measure_frame_rate(video: &mut VideoStream, frames: Vec<f64>) {
    video.vfr_suspect |= is_variable_rate(&frames);
    video.frame_times = (video.vfr_suspect && frames.len() > 1).then_some(frames);
}

/// Whether sorted frame times leave a gap far from the usual one: more than 10 % of the median gap,
/// and more than 1.5 ms. The margins keep containers that store rounded times steady — MKV keeps
/// whole milliseconds (29.97 fps: 33 and 34 ms), iPhone MOV 1/600 s ticks (20 and 21).
pub fn is_variable_rate(frames: &[f64]) -> bool {
    let gaps: Vec<f64> = frames.windows(2).map(|w| w[1] - w[0]).collect();
    if gaps.len() < 2 {
        return false;
    }
    let mut sorted = gaps.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    let margin = (median * 0.1).max(0.0015);
    gaps.iter().any(|g| (g - median).abs() > margin)
}

/// Every packet's time from the `pts_time,flags` lines of [`frame_times`], sorted (packets come in
/// decode order) and without duplicates.
pub fn parse_frame_times(csv: &str) -> Vec<f64> {
    let mut v: Vec<f64> = csv
        .lines()
        .filter_map(|line| line.trim().split(',').next()?.trim().parse::<f64>().ok())
        .filter(|t| t.is_finite())
        .collect();
    v.sort_by(f64::total_cmp);
    v.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    v
}

/// Seconds of a piece read at each end by [`piece_bounds`]: enough for the first packet of every
/// stream (muxers interleave within a second or two) and for the frames past OUT at the end.
const PIECE_EDGE: f64 = 10.0;

/// ffprobe arguments listing the packets at both ends of a lossless-cut piece `length` seconds long
/// (all of it when it is short), and the file's start time.
pub fn piece_bounds_args(piece: &Path, length: f64) -> Vec<std::ffi::OsString> {
    let mut a: Vec<std::ffi::OsString> = vec!["-v".into(), "error".into()];
    if length > 2.0 * PIECE_EDGE {
        // `%+10` = the first 10 s; `{t}%` = from the keyframe before t to the end.
        a.extend(["-read_intervals".into(), format!("%+{PIECE_EDGE},{:.3}%", length - PIECE_EDGE).into()]);
    }
    for s in ["-show_entries", "packet=codec_type,stream_index,pts_time,duration_time:format=start_time", "-of", "compact=p=0"] {
        a.push(s.into());
    }
    a.push(piece.as_os_str().to_owned());
    a
}

/// Where the packets of a lossless-cut piece lie, for its `duration` in the concat list.
pub fn piece_bounds(ffprobe: &Path, piece: &Path, length: f64) -> Result<crate::command::PieceBounds, Msg> {
    let out = run_capture(ffprobe, &piece_bounds_args(piece, length), Duration::from_secs(120)).map_err(|e| ran(ffprobe, e))?;
    if !out.success {
        return Err(Msg::line_or(last_line(&out.stderr, 300), Msg::PieceUnreadable));
    }
    parse_piece_bounds(&out.stdout).ok_or(Msg::PiecePacketsMissing)
}

/// Parses `key=value|…` lines of [`piece_bounds_args`]: packets, and the format's `start_time`.
pub fn parse_piece_bounds(compact: &str) -> Option<crate::command::PieceBounds> {
    let mut start = None;
    let mut video_end: Option<f64> = None;
    let mut audio: Vec<(u32, f64, f64)> = Vec::new();
    for line in compact.lines() {
        let fields: HashMap<&str, &str> = line.split('|').filter_map(|kv| kv.split_once('=')).collect();
        let num = |k: &str| fields.get(k).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite());
        if !fields.contains_key("codec_type") {
            if let Some(s) = num("start_time") {
                start = Some(s);
            }
            continue;
        }
        let Some(pts) = num("pts_time") else { continue };
        let end = pts + num("duration_time").unwrap_or(0.0).max(0.0);
        match fields["codec_type"] {
            "video" => video_end = Some(video_end.map_or(end, |e| e.max(end))),
            "audio" => {
                let Some(index) = fields.get("stream_index").and_then(|v| v.parse::<u32>().ok()) else { continue };
                match audio.iter_mut().find(|a| a.0 == index) {
                    Some(a) => (a.1, a.2) = (a.1.min(pts), a.2.max(end)),
                    None => audio.push((index, pts, end)),
                }
            }
            _ => {}
        }
    }
    if video_end.is_none() && audio.is_empty() {
        return None;
    }
    audio.sort_by_key(|a| a.0);
    Some(crate::command::PieceBounds { start: start.unwrap_or(0.0), video_end, audio })
}

pub fn parse_keyframes(csv: &str) -> Vec<f64> {
    let mut v: Vec<f64> = csv
        .lines()
        .filter_map(|line| {
            let (t, flags) = line.trim().split_once(',')?;
            if !flags.contains('K') {
                return None;
            }
            t.trim().parse::<f64>().ok()
        })
        .collect();
    v.sort_by(f64::total_cmp);
    v.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rates() {
        assert!((parse_rate("30000/1001").unwrap() - 29.97).abs() < 0.01);
        assert_eq!(parse_rate("25/1"), Some(25.0));
        assert_eq!(parse_rate("0/0"), None);
        assert_eq!(parse_rate("x"), None);
    }

    #[test]
    fn bit_depth() {
        for f in ["yuv420p10le", "yuv422p10be", "p010le", "p016le", "yuv444p12le", "gray10le"] {
            assert!(is_high_bit_depth(f), "{f}");
        }
        for f in ["yuv420p", "yuvj420p", "nv12", "yuv444p", "rgb24", "bgra"] {
            assert!(!is_high_bit_depth(f), "{f}");
        }
    }

    #[test]
    fn keyframe_csv() {
        let csv = "0.000000,K__\n0.033333,___\n1.001000,K_\n1.034000,__\nN/A,K__\n2.002000,K__,\n";
        assert_eq!(parse_keyframes(csv), vec![0.0, 1.001, 2.002]);
        // Every frame, in display order (packets list in decode order), N/A skipped.
        let decode_order = "0.000000,K__\n0.100000,___\n0.033333,___\n0.066667,___\nN/A,___\n0.100000,___\n";
        assert_eq!(parse_frame_times(decode_order), vec![0.0, 0.033333, 0.066667, 0.1]);
    }

    #[test]
    fn variable_rate_from_frame_times() {
        let grid = |n: usize, step: f64, round: f64| (0..n).map(|k| (k as f64 * step / round).round() * round).collect::<Vec<_>>();
        assert!(!is_variable_rate(&grid(300, 1001.0 / 30000.0, 1e-6)), "29.97 exact");
        assert!(!is_variable_rate(&grid(300, 1001.0 / 30000.0, 0.001)), "29.97 in MKV ms: 33 / 34 ms");
        assert!(!is_variable_rate(&grid(300, 1.0 / 120.0, 0.001)), "120 fps in MKV ms: 8 / 9 ms");
        assert!(!is_variable_rate(&grid(300, 1001.0 / 30000.0, 1.0 / 600.0)), "iPhone 1/600 ticks: 20 / 21");
        assert!(!is_variable_rate(&[0.0, 0.04]) && !is_variable_rate(&[]), "too few frames to tell");
        // 60 fps keeping 3 frames of every 5 (the integration test's source), and one dropped frame.
        let thinned: Vec<f64> = (0..300).filter(|n| n % 5 < 3).map(|n| n as f64 / 60.0).collect();
        assert!(is_variable_rate(&thinned));
        let mut dropped = grid(300, 1.0 / 30.0, 1e-6);
        dropped.remove(150);
        assert!(is_variable_rate(&dropped));
    }

    #[test]
    fn measured_rate_marks_variable_streams_and_hands_them_their_frames() {
        let mut v = parse_probe(&json!({"streams": [{"index": 0, "codec_type": "video", "r_frame_rate": "6480/179", "avg_frame_rate": "6480/179"}], "format": {"duration": "6.0"}}), Path::new("a.mkv")).unwrap().video.unwrap();
        assert!(!v.vfr_suspect, "both rates agree: MKV cannot tell from them");
        let thinned: Vec<f64> = (0..30).filter(|n| n % 5 < 3).map(|n| n as f64 / 60.0).collect();
        measure_frame_rate(&mut v, thinned.clone());
        assert!(v.vfr_suspect);
        assert_eq!(v.frame_times.as_deref(), Some(&thinned[..]));
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["frameTimes"].as_array().unwrap().len(), thinned.len());

        let steady: Vec<f64> = (0..30).map(|n| n as f64 / 30.0).collect();
        let mut v = parse_probe(&json!({"streams": [{"index": 0, "codec_type": "video", "r_frame_rate": "30/1", "avg_frame_rate": "30/1"}], "format": {"duration": "1.0"}}), Path::new("a.mp4")).unwrap().video.unwrap();
        measure_frame_rate(&mut v, steady.clone());
        assert!(!v.vfr_suspect && v.frame_times.is_none());
        assert!(serde_json::to_value(&v).unwrap().get("frameTimes").is_none(), "no list for a steady rate");
        // A phone recording the rates already flag keeps its verdict and gets its frames.
        let mut v = parse_probe(&json!({"streams": [{"index": 0, "codec_type": "video", "r_frame_rate": "30/1", "avg_frame_rate": "2850/100"}], "format": {"duration": "1.0"}}), Path::new("a.mov")).unwrap().video.unwrap();
        assert!(v.vfr_suspect);
        measure_frame_rate(&mut v, steady.clone());
        assert!(v.vfr_suspect && v.frame_times.is_some());
    }

    #[test]
    fn parses_iphone_like_probe() {
        let j = json!({
            "streams": [
                {"index": 0, "codec_type": "video", "codec_name": "hevc", "profile": "Main 10",
                 "width": 3840, "height": 2160, "r_frame_rate": "30/1", "avg_frame_rate": "2997/100",
                 "pix_fmt": "yuv420p10le", "color_primaries": "bt2020", "color_transfer": "arib-std-b67",
                 "color_space": "bt2020nc", "duration": "12.5", "start_time": "0.000000", "has_b_frames": 2,
                 "side_data_list": [{"side_data_type": "Display Matrix", "rotation": -90}]},
                {"index": 1, "codec_type": "audio", "codec_name": "aac", "channels": 2,
                 "sample_rate": "44100", "tags": {"language": "und"}},
                {"index": 2, "codec_type": "data", "codec_name": "none"},
                {"index": 3, "codec_type": "video", "codec_name": "mjpeg", "disposition": {"attached_pic": 1}}
            ],
            "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2", "duration": "12.512", "size": "52428800", "bit_rate": "33554432"}
        });
        let m = parse_probe(&j, Path::new("/tmp/가족 영상.mov")).unwrap();
        assert_eq!(m.file_name, "가족 영상.mov");
        assert_eq!(m.duration, 12.512);
        assert_eq!(m.size_bytes, Some(52_428_800));
        let v = m.video.unwrap();
        assert_eq!((v.index, v.codec.as_str(), v.width, v.height), (0, "hevc", 3840, 2160));
        assert!(v.hdr && v.wants_ten_bit());
        assert_eq!(
            (v.color_primaries.as_deref(), v.color_transfer.as_deref(), v.color_space.as_deref()),
            (Some("bt2020"), Some("arib-std-b67"), Some("bt2020nc"))
        );
        assert_eq!(v.start_time, Some(0.0));
        assert_eq!(v.has_b_frames, 2);
        assert_eq!(v.rotation, Some(-90.0));
        assert!(!v.vfr_suspect, "29.97 vs 30 is within 1%");
        assert_eq!(m.audio.len(), 1);
        assert_eq!(m.audio[0].sample_rate, Some(44100));
        assert_eq!(m.audio[0].language, None, "\"und\" is dropped");
        assert_eq!(m.other_count, 2, "data stream + cover art");
    }

    #[test]
    fn flags_variable_frame_rate() {
        let j = json!({
            "streams": [{"index": 0, "codec_type": "video", "codec_name": "h264", "width": 1920, "height": 1080,
                          "r_frame_rate": "60/1", "avg_frame_rate": "3721/125"}],
            "format": {"duration": "5"}
        });
        assert!(parse_probe(&j, Path::new("a.mp4")).unwrap().video.unwrap().vfr_suspect);
    }

    #[test]
    fn missing_duration_is_an_error() {
        let j = json!({"streams": [{"index": 0, "codec_type": "audio", "codec_name": "aac"}], "format": {}});
        assert_eq!(parse_probe(&j, Path::new("a.mp4")).unwrap_err(), Msg::DurationUnknown);
    }

    #[test]
    fn overlays_are_videos_or_still_images() {
        let png = serde_json::json!({
            "streams": [{"index": 0, "codec_type": "video", "codec_name": "png", "width": 400, "height": 120}],
            "format": {"format_name": "png_pipe"}
        });
        let o = parse_overlay(&png, Path::new("/로고/채널 로고.png")).unwrap();
        assert_eq!((o.kind, o.width, o.height, o.duration, o.has_audio, o.file_name.as_str()), (OverlayKind::Image, 400, 120, None, false, "채널 로고.png"));
        let phone = serde_json::json!({
            "streams": [
                {"index": 0, "codec_type": "video", "codec_name": "hevc", "width": 1920, "height": 1080, "r_frame_rate": "30/1", "avg_frame_rate": "30/1",
                 "side_data_list": [{"side_data_type": "Display Matrix", "rotation": -90}]},
                {"index": 1, "codec_type": "audio", "codec_name": "aac"}
            ],
            "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2", "duration": "12.5"}
        });
        let o = parse_overlay(&phone, Path::new("/v/카메라 1.mov")).unwrap();
        assert_eq!((o.kind, o.width, o.height, o.duration, o.has_audio), (OverlayKind::Video, 1080, 1920, Some(12.5), true));
        let music = serde_json::json!({"streams": [{"index": 0, "codec_type": "audio", "codec_name": "mp3"}], "format": {"format_name": "mp3", "duration": "3"}});
        assert_eq!(parse_overlay(&music, Path::new("a.mp3")).err(), Some(Msg::NoVideoTrack));
    }

    #[test]
    fn rejects_still_images_and_files_without_tracks() {
        // What ffprobe 9 reports for a JPEG and a PNG.
        let jpeg = json!({
            "streams": [{"index": 0, "codec_type": "video", "codec_name": "mjpeg", "width": 320, "height": 180,
                          "r_frame_rate": "25/1", "duration": "0.040000"}],
            "format": {"format_name": "image2", "duration": "0.040000"}
        });
        let png = json!({
            "streams": [{"index": 0, "codec_type": "video", "codec_name": "png", "width": 320, "height": 180}],
            "format": {"format_name": "png_pipe"}
        });
        for j in [jpeg, png] {
            assert_eq!(
                parse_probe(&j, Path::new("/tmp/사진 파일.jpg")).unwrap_err(),
                Msg::ImageFile
            );
        }
        let subtitles = json!({
            "streams": [{"index": 0, "codec_type": "subtitle", "codec_name": "subrip"}],
            "format": {"format_name": "srt", "duration": "60"}
        });
        assert_eq!(
            parse_probe(&subtitles, Path::new("자막 파일.srt")).unwrap_err(),
            Msg::NoTracks
        );
        // Audio only still opens (the editor plays the sound).
        let mp3 = json!({
            "streams": [{"index": 0, "codec_type": "audio", "codec_name": "mp3"}],
            "format": {"format_name": "mp3", "duration": "2.000000"}
        });
        assert!(parse_probe(&mp3, Path::new("소리 파일.mp3")).is_ok());
    }

    #[test]
    fn probe_errors_as_codes_without_the_path() {
        let file = Path::new("/tmp/테스트 폴더/메모 파일.txt");
        let text = |line: &str| probe_error_text(Some(line), file);
        assert_eq!(
            text("/tmp/테스트 폴더/메모 파일.txt: Invalid data found when processing input"),
            Msg::NotMedia
        );
        assert_eq!(text("/tmp/테스트 폴더/메모 파일.txt: Permission denied"), Msg::PermissionDenied);
        assert_eq!(text("/tmp/테스트 폴더/메모 파일.txt: No such file or directory"), Msg::FileNotFound { path: None });
        assert_eq!(
            text("/tmp/테스트 폴더/메모 파일.txt: Something new"),
            Msg::ProbeFailed { reason: Some("Something new".into()) }
        );
        // A line that does not start with the path is kept whole.
        assert_eq!(text("moov atom not found"), Msg::ProbeFailed { reason: Some("moov atom not found".into()) });
        assert_eq!(probe_error_text(None, file), Msg::ProbeFailed { reason: None });
    }

    #[test]
    fn piece_bounds_from_both_ends_of_a_piece() {
        // An MP4 piece of a B-frame source: an audio packet before the keyframe, frames past OUT.
        let compact = "codec_type=audio|stream_index=1|pts_time=-0.016000|duration_time=0.021333|\n\
            codec_type=video|stream_index=0|pts_time=0.000000|duration_time=0.033333\n\
            codec_type=audio|stream_index=1|pts_time=0.005333|duration_time=0.021333\n\
            codec_type=audio|stream_index=2|pts_time=0.010000|duration_time=N/A\n\
            codec_type=video|stream_index=0|pts_time=N/A|duration_time=0.033333\n\
            codec_type=audio|stream_index=1|pts_time=3.984000|duration_time=0.021333\n\
            codec_type=video|stream_index=0|pts_time=4.100000|duration_time=0.033333\n\
            codec_type=video|stream_index=0|pts_time=4.000000|duration_time=0.033333\n\
            codec_type=data|stream_index=3|pts_time=9.000000|duration_time=1.000000\n\
            start_time=0.000000\n";
        let b = parse_piece_bounds(compact).unwrap();
        assert_eq!(b.start, 0.0);
        assert!((b.video_end.unwrap() - 4.133333).abs() < 1e-9, "the last frame shown, not the last read");
        assert_eq!(b.audio.len(), 2);
        assert_eq!((b.audio[0].0, b.audio[0].1), (1, -0.016));
        assert!((b.audio[0].2 - 4.005333).abs() < 1e-9);
        assert_eq!(b.audio[1], (2, 0.01, 0.01), "no duration: its pts");
        assert!(parse_piece_bounds("start_time=0.000000\n").is_none());
        assert_eq!(parse_piece_bounds("codec_type=video|stream_index=0|pts_time=0.5|duration_time=0.04\nstart_time=N/A").unwrap().start, 0.0);
    }

    #[test]
    fn long_pieces_are_read_at_both_ends_only() {
        let s = |a: Vec<std::ffi::OsString>| a.iter().map(|x| x.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
        assert_eq!(
            s(piece_bounds_args(Path::new("/v/.godseon-cut-1/piece-000.mp4"), 125.5)),
            "-v error -read_intervals %+10,115.500% -show_entries packet=codec_type,stream_index,pts_time,duration_time:format=start_time -of compact=p=0 /v/.godseon-cut-1/piece-000.mp4"
        );
        assert!(!s(piece_bounds_args(Path::new("p.mkv"), 20.0)).contains("-read_intervals"), "short: all of it");
    }
}
