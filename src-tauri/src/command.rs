//! FFmpeg argument builders. Output is an argument list, never a shell string.

use crate::effects::{self, EffectsPlan, MaskFilter, OverlayPlan};
use crate::probe::OverlayKind;
use crate::msg::Msg;
use crate::plan;
use crate::probe::{MediaInfo, VideoStream};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// For inputs whose demuxer seeks by DTS (not the mov/mp4 demuxer), FFmpeg moves an input `-ss`
/// target 3/23 s earlier when the video has B-frames. Cutting at (or just after) a keyframe would
/// then land one whole GOP early, so the seek point is pushed past this margin.
pub const DTS_SEEK_BACKOFF: f64 = 3.0 / 23.0;

/// The mov/mp4 demuxer seeks by PTS, and its muxer writes edit lists so a copy starts exactly at `-ss`.
pub fn is_mov_family(format_name: &str) -> bool {
    format_name.split(',').any(|f| matches!(f.trim(), "mov" | "mp4" | "m4a" | "3gp" | "3g2" | "mj2"))
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LosslessTrimPlan {
    /// Value passed to `-ss`.
    pub seek: f64,
    /// Value passed to `-t`: up to the requested end, [`CUT_END_MARGIN`] early unless that is the
    /// file's end.
    pub duration: f64,
    /// Keyframe the copy starts from.
    pub keyframe: Option<f64>,
    /// MP4/MOV output: playback starts exactly at the requested start (edit lists).
    /// Otherwise playback starts at `keyframe`.
    pub exact_start: bool,
    /// Where playback is expected to start.
    pub visible_start: f64,
}

/// Seek point and duration for a stream-copy trim of [start, end), given what ffprobe found.
pub fn plan_lossless_trim(
    info: &MediaInfo,
    keyframes: &[f64],
    start: f64,
    end: f64,
) -> Result<LosslessTrimPlan, Msg> {
    let range = plan::keep_from_trim(start, end, info.duration)?;
    let mov = is_mov_family(&info.format_name);
    let b_frames = info.video.as_ref().is_some_and(|v| v.has_b_frames > 0);
    let keyframe = plan::keyframe_at_or_before(keyframes, range.start);
    let end = lossless_end(range, info.duration);
    let mut seek = range.start;
    if let Some(k) = keyframe {
        if !mov && b_frames {
            // 2 ms past the margin; never past the end of the range.
            seek = seek.max(k + DTS_SEEK_BACKOFF + 0.002).min(end - 0.0005);
        }
    }
    let visible_start = if mov { range.start } else { keyframe.unwrap_or(range.start) };
    Ok(LosslessTrimPlan {
        seek,
        duration: end - seek,
        keyframe,
        exact_start: mov,
        visible_start,
    })
}

/// Where a stream copy of `range` stops: [`CUT_END_MARGIN`] before OUT, so the frame at OUT stays
/// out even where the container rounded its time down. At the file's end there is no such frame,
/// and the copy keeps every last audio packet.
fn lossless_end(range: plan::Range, file_duration: f64) -> f64 {
    if range.end < file_duration - WHOLE_FILE_SLACK {
        (range.end - CUT_END_MARGIN).max(range.start + range.len() * 0.5)
    } else {
        range.end
    }
}

/// Seconds with millisecond precision, for times that need not hit a frame (thumbnails, waveform).
pub(crate) fn secs(t: f64) -> String {
    format!("{:.3}", t.max(0.0))
}

/// Seconds for cut points, rounded DOWN to the microsecond. Frame starts are rarely whole
/// milliseconds (30 fps: 245.1666… s), and rounding one up to the next millisecond (245.167) puts
/// that frame a fraction of a millisecond before the cut: MP4 edit lists then hide it and the copy
/// starts one frame late, and an end rounded up keeps the frame at OUT. Measured with FFmpeg 9.0.2
/// on a 4K 30 fps MP4 (docs/PROGRESS.md, Phase 1 completion check). The 1 ns nudge keeps float
/// noise (2.4 × 10⁶ = 2399999.9999…) from losing a whole microsecond.
pub(crate) fn exact_secs(t: f64) -> String {
    let micros = (t.max(0.0) * 1e6 + 1e-3).floor();
    format!("{:.6}", micros / 1e6)
}

/// A cut ends this long before the frame at OUT. Containers round frame times: MKV stores
/// milliseconds, so 30 fps frame #226 (7.5333… s) is stored as 7.533 and, with the cut at exactly
/// 7.533333, came into the result — re-encoded with every encoder, and stream-copied too (FFmpeg
/// 9.0.2). The frame before OUT is a whole frame earlier and stays. A re-encoded piece's audio ends
/// as much earlier; the concat filter pads it with silence to the video's length, so the next piece
/// stays in sync.
pub const CUT_END_MARGIN: f64 = 0.001;

/// `-t` of a re-encoded piece `len` long (see [`CUT_END_MARGIN`]).
fn precise_len(len: f64) -> String {
    exact_secs((len - CUT_END_MARGIN).max(len * 0.5))
}

/// Arguments every job starts with: quiet log, machine-readable progress on stdout.
fn job_prefix() -> Vec<OsString> {
    ["-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y"]
        .into_iter()
        .map(OsString::from)
        .collect()
}

fn is_mp4_family(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
        Some("mp4" | "mov" | "m4v")
    )
}

/// Lossless (stream copy) trim of one range.
///
/// `-ss` before `-i` seeks fast; stream copy has to start at the keyframe at or before `start`.
/// What viewers see then depends on the container (measured with FFmpeg 9.0.2, see the
/// A/V sync integration test):
/// - MP4 / MOV: the muxer writes edit lists, so playback starts exactly at `start` and audio stays
///   within ~1 ms. Players that ignore edit lists show the frames from the keyframe.
/// - MKV / WebM and others: the frames from the keyframe are shown (start is pulled earlier).
///
/// `-avoid_negative_ts make_zero` is deliberately NOT used: it disables the MP4 edit-list start
/// and added ~17 ms of A/V offset in MP4 in the same measurement.
///
/// `-ss` and `-t` go out with [`exact_secs`] so a frame start passed in stays the first frame and
/// the frame at the end stays out.
///
/// Maps the chosen video stream and every audio stream; data tracks (e.g. iPhone metadata) are
/// left out because many muxers reject them.
pub fn lossless_trim_args(
    input: &Path,
    video_index: Option<u32>,
    start: f64,
    duration: f64,
    output: &Path,
) -> Vec<OsString> {
    stream_copy_args(input, video_index, start, duration, output, is_mp4_family(output))
}

/// [`lossless_trim_args`]; `faststart` moves the MP4 index to the front (a second pass over the file).
fn stream_copy_args(
    input: &Path,
    video_index: Option<u32>,
    start: f64,
    duration: f64,
    output: &Path,
    faststart: bool,
) -> Vec<OsString> {
    let mut a = job_prefix();
    a.extend(["-ss".into(), exact_secs(start).into()]);
    a.extend(["-i".into(), input.as_os_str().to_owned()]);
    a.extend(["-t".into(), exact_secs(duration).into()]);
    let video_map = match video_index {
        Some(i) => format!("0:{i}"),
        None => "0:v:0?".to_string(),
    };
    a.extend(["-map".into(), video_map.into(), "-map".into(), "0:a?".into()]);
    a.extend(["-c".into(), "copy".into(), "-map_metadata".into(), "0".into()]);
    if faststart {
        a.extend(["-movflags".into(), "+faststart".into()]);
    }
    a.push(output.as_os_str().to_owned());
    a
}

// ---------------------------------------------------------------------------------------------
// Lossless 구간 제거 (canvas 03 · 3.2 B): every kept piece is stream-copied to its own file, then
// the concat demuxer joins them, stream copy again. Measured with FFmpeg 9.0.2 (docs/PROGRESS.md,
// Phase 2 step 3):
// - MP4 edit lists do not survive the concat demuxer: a piece cut mid-GOP brings its frames from the
//   keyframe with times before the piece's start, and the join comes out with non-monotonic DTS and
//   shifted frames. So in MP4 too every piece is copied from its keyframe (`-ss` = the keyframe), and
//   its first frame is that keyframe, as in MKV.
// - The concat demuxer places each piece after the one before by the piece's duration, which in MP4
//   leaves out the frames a B-frame source brings past OUT (they then cover the next keyframe), and
//   a piece's audio can end after its video or start before it. The list gives every piece its
//   duration from its packets instead ([`concat_durations`]), so no stream overlaps the next piece.
// - A piece after a join must start at an IDR frame (H.264 · H.265): from a CRA or a non-IDR I-frame
//   (open GOP, x265's default) the decoder carries on the picture order of the piece before and loses
//   up to a whole GOP. FFmpeg has no filter to end the sequence there, so such sources are refused
//   ([`random_access_check_args`]).

/// One kept piece of a lossless 구간 제거.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LosslessPiece {
    /// What stays of the original, as asked (after the joins [`plan_lossless_cut`] makes).
    pub keep: plan::Range,
    /// `-ss`
    pub seek: f64,
    /// `-t`
    pub duration: f64,
    /// The keyframe the copy starts from — where the piece's first frame comes from (the piece's
    /// start itself only for a single piece in MP4 / MOV, as in a trim).
    pub visible_start: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LosslessCutPlan {
    /// In order; never empty.
    pub pieces: Vec<LosslessPiece>,
    /// Removed ranges that stay in the result: there is no keyframe between their start and their
    /// end, so the piece after them would start before them.
    pub kept_removals: Vec<plan::Range>,
    /// A single piece in MP4 / MOV: the edit list starts it exactly at its start (a trim).
    pub exact_start: bool,
    /// Playing length of the result, without the frames a B-frame source brings past OUT.
    pub length: f64,
}

/// 구간 제거 by stream copy: the kept ranges ([`plan::keep_from_removed`], as [`plan_precise_cut`]),
/// each starting at the keyframe at or before its start. A piece whose keyframe is not past the end
/// of the piece before would bring back what was removed between them (and repeat what came before):
/// the two become one piece and that removal stays (`kept_removals`, shown to the user beforehand).
/// A single piece left is a trim ([`plan_lossless_trim`]).
pub fn plan_lossless_cut(info: &MediaInfo, keyframes: &[f64], removed: &[plan::Range]) -> Result<LosslessCutPlan, Msg> {
    if !(info.duration.is_finite() && info.duration > 0.0) {
        return Err(Msg::DurationUnknown);
    }
    let video = info.video.as_ref().ok_or(Msg::CutNeedsVideo)?;
    if keyframes.is_empty() {
        return Err(Msg::CutNeedsKeyframes);
    }
    let min_len = min_keep_len(video);
    if plan::normalize(removed, info.duration).iter().all(|r| r.len() < min_len) {
        return Err(Msg::NoRangesToRemove);
    }
    let keep = plan::keep_from_removed(removed, info.duration, min_len);
    if keep.is_empty() {
        return Err(Msg::RemovesEverything);
    }
    // (kept range, keyframe it is copied from)
    let mut joined: Vec<(plan::Range, f64)> = Vec::with_capacity(keep.len());
    let mut kept_removals = Vec::new();
    for k in keep {
        let from = plan::keyframe_at_or_before(keyframes, k.start).unwrap_or(0.0);
        match joined.last_mut() {
            Some((prev, _)) if from <= prev.end + 1e-6 => {
                kept_removals.push(plan::Range::new(prev.end, k.start));
                prev.end = k.end;
            }
            _ => joined.push((k, from)),
        }
    }
    let mov = is_mov_family(&info.format_name);
    let (pieces, exact_start) = if let [(k, _)] = joined.as_slice() {
        let t = plan_lossless_trim(info, keyframes, k.start, k.end)?;
        (vec![LosslessPiece { keep: *k, seek: t.seek, duration: t.duration, visible_start: t.visible_start }], t.exact_start)
    } else {
        let mut pieces = Vec::with_capacity(joined.len());
        for (k, from) in &joined {
            pieces.push(if mov {
                // The mov demuxer seeks by PTS: `-ss` at the keyframe copies from it with nothing before.
                LosslessPiece { keep: *k, seek: *from, duration: lossless_end(*k, info.duration) - from, visible_start: *from }
            } else {
                // Same seek as a trim (past the B-frame DTS margin); the copy starts at the keyframe.
                let t = plan_lossless_trim(info, keyframes, k.start, k.end)?;
                LosslessPiece { keep: *k, seek: t.seek, duration: t.duration, visible_start: t.visible_start }
            });
        }
        (pieces, false)
    };
    if let [p] = pieces.as_slice() {
        if p.visible_start <= WHOLE_FILE_SLACK && p.keep.end >= info.duration - WHOLE_FILE_SLACK {
            return Err(Msg::NothingRemovableLossless);
        }
    }
    let length = pieces.iter().map(|p| (p.seek + p.duration - p.visible_start).max(0.0)).sum();
    Ok(LosslessCutPlan { pieces, kept_removals, exact_start, length })
}

/// Stream copy of one piece into the scratch folder. As [`lossless_trim_args`], without faststart
/// (the piece is read once, by the concat step).
pub fn lossless_piece_args(input: &Path, video_index: Option<u32>, piece: &LosslessPiece, output: &Path) -> Vec<OsString> {
    stream_copy_args(input, video_index, piece.seek, piece.duration, output, false)
}

/// Scratch folder of a lossless cut, next to the result (the same disk, which holds the result
/// anyway): an ASCII name, hidden on macOS · Linux. `tag` tells jobs apart.
pub fn cut_scratch_dir(final_path: &Path, tag: &str) -> PathBuf {
    let dir = final_path.parent().map(Path::to_path_buf).unwrap_or_default();
    dir.join(format!(".godseon-cut-{tag}"))
}

/// `piece-000.mp4`: plain ASCII, so the concat list holds it as it is, relative to the list (the
/// demuxer's safe mode accepts it). The container is the result's; Err if its extension is not ASCII.
pub fn piece_name(index: usize, extension: &str) -> Result<String, Msg> {
    if extension.is_empty() || !extension.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(Msg::CutExtensionUnsupported { extension: extension.to_string() });
    }
    Ok(format!("piece-{index:03}.{}", extension.to_ascii_lowercase()))
}

/// Where one piece's packets lie (from [`crate::probe::piece_bounds`]), in the piece's own times.
#[derive(Debug, Clone, PartialEq)]
pub struct PieceBounds {
    /// The file's start time, which the concat demuxer puts at the piece's place in the result.
    pub start: f64,
    /// End of the last video frame (pts + duration), B-frame tail included.
    pub video_end: Option<f64>,
    /// (stream index, first pts, end) of each audio stream.
    pub audio: Vec<(u32, f64, f64)>,
}

/// The `duration` of every piece in the concat list: far enough that nothing of the piece overlaps
/// the next one — its video's end (frames past OUT included), and each audio track's end measured
/// against where that track starts in the next piece (MP4 pieces keep an audio packet from just
/// before the keyframe). Where a track ends earlier, the result has that much silence; the
/// timestamps keep every piece in sync.
pub fn concat_durations(bounds: &[PieceBounds]) -> Vec<f64> {
    bounds
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let audio_end = b.audio.iter().map(|a| a.2).fold(f64::NEG_INFINITY, f64::max);
            let mut d = b.video_end.unwrap_or(audio_end) - b.start;
            match bounds.get(i + 1) {
                Some(next) => {
                    for (index, _, end) in &b.audio {
                        if let Some((_, first, _)) = next.audio.iter().find(|a| a.0 == *index) {
                            d = d.max((end - b.start) - (first - next.start));
                        }
                    }
                }
                None => d = d.max(audio_end - b.start),
            }
            d.max(0.0)
        })
        .collect()
}

/// The concat list (UTF-8): every piece by its ASCII name, relative to the list, with its duration
/// rounded up to the microsecond so the next piece never starts inside it.
pub fn concat_list(pieces: &[(String, f64)]) -> String {
    let mut s = String::from("ffconcat version 1.0\n");
    for (name, duration) in pieces {
        let micros = (duration.max(0.0) * 1e6 - 1e-3).ceil();
        s.push_str(&format!("file '{name}'\nduration {:.6}\n", micros / 1e6));
    }
    s
}

/// Joins the pieces in `list` by stream copy. The concat demuxer keeps each stream's metadata (the
/// audio languages) but not the file's (title, creation date): those come from the original, added
/// as a second input that is never read past its header. Chapters are left out (their times would
/// point at the wrong places after the cuts).
pub fn lossless_concat_args(list: &Path, source: &Path, output: &Path) -> Vec<OsString> {
    let mut a = job_prefix();
    a.extend(["-f".into(), "concat".into(), "-i".into(), list.as_os_str().to_owned()]);
    a.extend(["-i".into(), source.as_os_str().to_owned()]);
    a.extend(["-map".into(), "0".into(), "-c".into(), "copy".into()]);
    a.extend(["-map_metadata".into(), "1".into(), "-map_chapters".into(), "-1".into()]);
    if is_mp4_family(output) {
        a.extend(["-movflags".into(), "+faststart".into()]);
    }
    a.push(output.as_os_str().to_owned());
    a
}

/// Copies the one video packet at `seek` through `trace_headers`, whose log names its NAL unit (or
/// AV1 frame) type: [`starts_a_sequence`] reads it.
pub fn random_access_check_args(input: &Path, video_index: u32, seek: f64) -> Vec<OsString> {
    let mut a: Vec<OsString> = ["-hide_banner", "-nostdin", "-loglevel", "info"].into_iter().map(OsString::from).collect();
    a.extend(["-ss".into(), exact_secs(seek).into(), "-i".into(), input.as_os_str().to_owned()]);
    a.extend(["-map".into(), format!("0:{video_index}").into(), "-c".into(), "copy".into(), "-frames:v".into(), "1".into()]);
    a.extend(["-bsf:v".into(), "trace_headers".into(), "-f".into(), "null".into(), "-".into()]);
    a
}

/// Whether the packet in a [`random_access_check_args`] log can start a piece after a join: an IDR
/// frame (H.264 type 5, H.265 IDR · BLA 16–20) or a shown AV1 key frame. A CRA (H.265 type 21) or a
/// non-IDR H.264 I-frame (open GOP) cannot. None when the log says nothing about it (other codecs:
/// VP9 · ProRes · MPEG-2 … are not checked).
pub fn starts_a_sequence(codec: &str, log: &str) -> Option<bool> {
    let value = |line: &str| line.rsplit_once('=').and_then(|(_, v)| v.trim().parse::<u32>().ok());
    // Parameter sets of the extradata come first; the packet's own units follow its "Packet:" line.
    let packet = log.lines().skip_while(|l| !l.contains("] Packet: ")).skip(1);
    match codec {
        "h264" | "hevc" => {
            let vcl = |t: u32| if codec == "h264" { (1..=5).contains(&t) } else { t < 32 };
            let nal = packet.filter(|l| l.contains(" nal_unit_type ")).filter_map(value).find(|t| vcl(*t))?;
            Some(if codec == "h264" { nal == 5 } else { (16..=20).contains(&nal) })
        }
        "av1" => {
            let mut lines = packet.skip_while(|l| !l.contains(" frame_type "));
            let frame_type = lines.next().and_then(value)?;
            let shown = lines.find(|l| l.contains(" show_frame ")).and_then(value)?;
            Some(frame_type == 0 && shown == 1)
        }
        _ => None,
    }
}

/// Short side of the preview proxy in pixels ("540p"). Smaller sources are not upscaled.
pub const PROXY_SHORT_SIDE: u32 = 540;

/// `scale` filter for the proxy. FFmpeg rotates phone video upright before filters run
/// (autorotate), so the short side is chosen on the displayed picture, not the stored one.
pub fn proxy_scale_filter(video: &VideoStream) -> String {
    let quarter_turn = video.rotation.is_some_and(|r| ((r / 90.0).round() as i64).rem_euclid(2) == 1);
    let (w, h) = if quarter_turn { (video.height, video.width) } else { (video.width, video.height) };
    let short = match w.min(h) {
        0 => PROXY_SHORT_SIDE,
        s => s.min(PROXY_SHORT_SIDE),
    };
    let short = (short & !1).max(2); // H.264 4:2:0 needs even sizes
    if w >= h { format!("scale=-2:{short}") } else { format!("scale={short}:-2") }
}

/// 540p H.264 + AAC MP4 proxy for previewing a file the webview cannot play (canvas 03 · 3.5).
///
/// Only for viewing: edits always use the original's timeline, so the proxy keeps the source
/// timestamps (`-fps_mode vfr`: no frames duplicated or dropped to force a constant rate).
/// A 15-frame GOP keeps scrubbing smooth. `yuv420p` makes 10-bit sources playable as 8-bit H.264.
/// Only the first audio track is kept, downmixed to stereo.
pub fn proxy_args(input: &Path, video: &VideoStream, output: &Path) -> Vec<OsString> {
    let mut a = job_prefix();
    a.extend(["-i".into(), input.as_os_str().to_owned()]);
    a.extend(["-map".into(), format!("0:{}", video.index).into(), "-map".into(), "0:a:0?".into()]);
    a.extend(["-vf".into(), proxy_scale_filter(video).into(), "-pix_fmt".into(), "yuv420p".into()]);
    for s in ["-c:v", "libx264", "-preset", "ultrafast", "-crf", "28", "-g", "15", "-fps_mode", "vfr"] {
        a.push(s.into());
    }
    for s in ["-c:a", "aac", "-b:a", "96k", "-ac", "2", "-movflags", "+faststart"] {
        a.push(s.into());
    }
    a.push(output.as_os_str().to_owned());
    a
}

// ---------------------------------------------------------------------------------------------
// Precise export: re-encode one kept range (canvas 03 · 3.2 C, 3.3).

/// Quality preset; the per-encoder values are the table in canvas 03 · 3.3 (and the ffmpeg-args skill).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Quality {
    High,
    Balanced,
    Small,
}

impl Quality {
    /// The value for this preset out of [high, balanced, small].
    fn pick(self, values: [u32; 3]) -> u32 {
        match self {
            Quality::High => values[0],
            Quality::Balanced => values[1],
            Quality::Small => values[2],
        }
    }
}

/// Video codec of a re-encoded result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
}

/// What the user picked as the output video codec. `Source` ("원본 유지") re-encodes to the same
/// codec when the app can write it (H.264 · H.265 · AV1), else H.264 (2026-11-23 decision).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OutputCodec {
    Source,
    Hevc,
    Av1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioPlan {
    /// No audio track.
    None,
    /// The whole file, AAC already: copied untouched.
    Copy,
    /// AAC 192k. A cut decodes the audio anyway (sample-exact start and end).
    Aac,
}

pub const AUDIO_AAC_BITRATE: &str = "192k";

/// `-fps_mode passthrough` keeps every frame with its own time, and this keeps those times as they
/// are: by default the encoder counts in 1/frame rate, which rounds a variable rate's times onto
/// that grid — a variable-rate MKV (frame rate 6480/179) put two frames on one time (measured, FFmpeg
/// 9.0.2). 90 kHz, the usual video clock, holds MKV milliseconds, 1/600 s (iPhone) and 24 · 25 · 30 ·
/// 60 · 29.97 fps frames exactly, and anything else within 6 µs. A number, so FFmpeg 6.0 takes it
/// (its `filter` / `demux` words came later).
const KEEP_FRAME_TIMES: [&str; 4] = ["-fps_mode", "passthrough", "-enc_time_base:v", "1/90000"];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreciseTrimPlan {
    /// `-ss`; None for the whole file (no `-ss` · `-t`).
    pub seek: Option<f64>,
    /// Length of the result.
    pub duration: f64,
    pub codec: VideoCodec,
    /// 10-bit output (10-bit or HDR source, H.265 · AV1 only).
    pub ten_bit: bool,
    pub audio: AudioPlan,
    /// Output file name suffix (`_trim`, `_trim_h265`, `_h265`…).
    pub suffix: String,
    /// Output container extension.
    pub extension: String,
    /// 화면 효과 (crop · turn · size) before the colors; None: the picture as it is. Planned by
    /// [`effects::plan`] and set by the caller (`export::prepare`).
    pub effects: Option<EffectsPlan>,
    /// The first audio track's stream index (PiP sound is mixed into it).
    pub first_audio: Option<u32>,
}

/// Within this of the file's start and end, a range counts as the whole file.
const WHOLE_FILE_SLACK: f64 = 0.001;

/// Containers each re-encoded codec may keep when the user chose "원본 유지"; otherwise MP4
/// (H.265 · AV1 chosen explicitly always go to MP4, as in the editor mockup).
fn keeps_container(codec: VideoCodec, ext: &str) -> bool {
    match codec {
        VideoCodec::H264 | VideoCodec::Hevc => matches!(ext, "mp4" | "m4v" | "mov" | "mkv"),
        VideoCodec::Av1 => matches!(ext, "mp4" | "mkv"),
    }
}

/// Codec, bit depth and container of a re-encoded result; the same for 자르기 and 구간 제거.
struct PreciseTarget {
    codec: VideoCodec,
    ten_bit: bool,
    extension: String,
}

fn precise_target(info: &MediaInfo, output: OutputCodec) -> Result<PreciseTarget, Msg> {
    let video = info.video.as_ref().ok_or(Msg::PreciseNeedsVideo)?;
    let codec = match output {
        OutputCodec::Hevc => VideoCodec::Hevc,
        OutputCodec::Av1 => VideoCodec::Av1,
        OutputCodec::Source => match video.codec.as_str() {
            "hevc" => VideoCodec::Hevc,
            "av1" => VideoCodec::Av1,
            _ => VideoCodec::H264,
        },
    };
    let source_ext = Path::new(&info.path)
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = if output == OutputCodec::Source && keeps_container(codec, &source_ext.to_ascii_lowercase()) {
        source_ext
    } else {
        "mp4".to_string()
    };
    Ok(PreciseTarget { codec, ten_bit: codec != VideoCodec::H264 && video.wants_ten_bit(), extension })
}

/// `_h265` · `_av1` after `_trim` / `_cut`; nothing for "원본 유지".
fn codec_suffix(output: OutputCodec) -> &'static str {
    match output {
        OutputCodec::Source => "",
        OutputCodec::Hevc => "_h265",
        OutputCodec::Av1 => "_av1",
    }
}

/// Re-encodes [start, end) frame-exactly: the first frame is the one starting at `start`, the last
/// the one before `end` (FFmpeg decodes from the keyframe before `-ss` and drops what comes before).
pub fn plan_precise_trim(info: &MediaInfo, start: f64, end: f64, output: OutputCodec) -> Result<PreciseTrimPlan, Msg> {
    let range = plan::keep_from_trim(start, end, info.duration)?;
    let target = precise_target(info, output)?;
    let whole = range.start <= WHOLE_FILE_SLACK && range.end >= info.duration - WHOLE_FILE_SLACK;
    let audio = if info.audio.is_empty() {
        AudioPlan::None
    } else if whole && info.audio.iter().all(|a| a.codec == "aac") {
        AudioPlan::Copy
    } else {
        AudioPlan::Aac
    };
    let suffix = match (whole, codec_suffix(output)) {
        (true, "") => "_trim".to_string(),
        (true, s) => s.to_string(),
        (false, s) => format!("_trim{s}"),
    };
    Ok(PreciseTrimPlan {
        seek: (!whole).then_some(range.start),
        duration: if whole { info.duration } else { range.len() },
        codec: target.codec,
        ten_bit: target.ten_bit,
        audio,
        suffix,
        extension: target.extension,
        effects: None,
        first_audio: info.audio.first().map(|a| a.index),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreciseCutPlan {
    /// Ranges of the original that stay, in order; never empty.
    pub keep: Vec<plan::Range>,
    /// Length of the result: Σkeep.
    pub duration: f64,
    pub codec: VideoCodec,
    pub ten_bit: bool,
    /// Never `Copy`: the cuts decode the audio anyway.
    pub audio: AudioPlan,
    /// `_cut`, `_cut_h265`, `_cut_av1`.
    pub suffix: String,
    pub extension: String,
    /// 화면 효과, applied to the joined picture (they do not change with time).
    pub effects: Option<EffectsPlan>,
    pub first_audio: Option<u32>,
}

impl PreciseCutPlan {
    /// One piece left (a cut at the start or the end only): the same job as a precise trim.
    pub fn as_trim(&self) -> Option<PreciseTrimPlan> {
        match self.keep.as_slice() {
            [k] => Some(PreciseTrimPlan {
                seek: Some(k.start),
                duration: k.len(),
                codec: self.codec,
                ten_bit: self.ten_bit,
                audio: self.audio,
                suffix: self.suffix.clone(),
                extension: self.extension.clone(),
                effects: self.effects.clone(),
                first_audio: self.first_audio,
            }),
            _ => None,
        }
    }
}

/// Kept pieces shorter than this are dropped. Just under one frame (1/30 s when the rate is unknown,
/// like `frameDuration` in the editor): the editor puts every cut on a frame start, so a piece of
/// exactly one frame stays, and a piece this long always holds a frame start at a constant rate.
pub fn min_keep_len(video: &VideoStream) -> f64 {
    let frame = video.fps.filter(|f| *f > 0.0).map_or(1.0 / 30.0, |f| 1.0 / f);
    frame * 0.999
}

/// 구간 제거: what stays of the file once `removed` is taken out (`plan::keep_from_removed`), and
/// how it is re-encoded. Codec, container and bit depth follow [`plan_precise_trim`].
pub fn plan_precise_cut(info: &MediaInfo, removed: &[plan::Range], output: OutputCodec) -> Result<PreciseCutPlan, Msg> {
    if !(info.duration.is_finite() && info.duration > 0.0) {
        return Err(Msg::DurationUnknown);
    }
    let target = precise_target(info, output)?;
    let video = info.video.as_ref().expect("precise_target checked the video");
    let min_len = min_keep_len(video);
    if plan::normalize(removed, info.duration).iter().all(|r| r.len() < min_len) {
        return Err(Msg::NoRangesToRemove);
    }
    let keep = plan::keep_from_removed(removed, info.duration, min_len);
    if keep.is_empty() {
        return Err(Msg::RemovesEverything);
    }
    Ok(PreciseCutPlan {
        duration: plan::total(&keep),
        keep,
        codec: target.codec,
        ten_bit: target.ten_bit,
        audio: if info.audio.is_empty() { AudioPlan::None } else { AudioPlan::Aac },
        suffix: format!("_cut{}", codec_suffix(output)),
        extension: target.extension,
        effects: None,
        first_audio: info.audio.first().map(|a| a.index),
    })
}

/// Encoders the precise export knows settings for. AV1 hardware encoders (values still to be
/// calibrated, canvas 03 · 3.3), VAAPI (needs a hardware upload filter) and Media Foundation are
/// left out until they are checked on Windows · Ubuntu.
pub fn exports_with(encoder: &str) -> bool {
    matches!(
        encoder,
        "libx264" | "libx265" | "libsvtav1" | "hevc_videotoolbox" | "hevc_nvenc" | "hevc_qsv" | "hevc_amf"
    )
}

/// The quality value of each preset [고화질, 균형, 작은 용량] for one encoder (canvas 03 · 3.3,
/// the ffmpeg-args skill): CRF for the software encoders, `-q:v` for VideoToolbox (higher = better),
/// `-cq` · `-global_quality` · `-qp_p` for NVENC · QSV · AMF. None for an encoder without settings.
///
/// x265 CRF 20 · 24 · 28 is the anchor (mean VMAF 96.4 · 92.5 · 86.0 over six 1080p clips); the
/// others are the values whose mean VMAF on the same clips is closest to it (2026-11-24, Apple M5,
/// FFmpeg 9.0.2, `examples/vmaf_calibrate.rs`). NVENC · QSV · AMF are not measured yet (Windows).
pub fn quality_values(encoder: &str) -> Option<[u32; 3]> {
    Some(match encoder {
        "libx265" => [20, 24, 28],
        "libx264" => [20, 24, 27],
        "libsvtav1" => [28, 38, 48],
        "hevc_videotoolbox" => [68, 60, 54],
        "hevc_nvenc" | "hevc_qsv" | "hevc_amf" => [22, 26, 30],
        _ => return None,
    })
}

/// `-c:v` and the quality, pixel format and profile options of one encoder (canvas 03 · 3.3).
/// Output is always 4:2:0, in 10 bits when `ten_bit` (H.265 Main 10, AV1 10-bit).
pub fn video_encoder_args(encoder: &str, quality: Quality, ten_bit: bool) -> Result<Vec<OsString>, Msg> {
    let values = quality_values(encoder).ok_or_else(|| Msg::EncoderUnsupported { encoder: encoder.to_string() })?;
    video_encoder_args_at(encoder, quality.pick(values), ten_bit)
}

/// [`video_encoder_args`] with the quality `value` given directly instead of a preset — what the
/// presets resolve to, and what the VMAF calibration (`examples/vmaf_calibrate.rs`) sweeps.
pub fn video_encoder_args_at(encoder: &str, value: u32, ten_bit: bool) -> Result<Vec<OsString>, Msg> {
    let q = value.to_string();
    let main10 = |hw_8bit: &'static str| -> Vec<String> {
        if ten_bit {
            vec!["-profile:v".into(), "main10".into(), "-pix_fmt".into(), "p010le".into()]
        } else {
            vec!["-pix_fmt".into(), hw_8bit.into()]
        }
    };
    let yuv420 = if ten_bit { "yuv420p10le" } else { "yuv420p" };
    let mut a: Vec<String> = vec!["-c:v".into(), encoder.into()];
    match encoder {
        "libx265" => {
            a.extend(["-crf".into(), q.clone(), "-preset".into(), "medium".into()]);
            // x265 prints its own banner to stderr; keep only errors there.
            a.extend(["-x265-params".into(), "log-level=error".into(), "-pix_fmt".into(), yuv420.into()]);
        }
        // H.264 for "원본 유지": always 8-bit (10-bit H.264 plays almost nowhere).
        "libx264" => a.extend(["-crf".into(), q.clone(), "-preset".into(), "medium".into(), "-pix_fmt".into(), "yuv420p".into()]),
        "libsvtav1" => a.extend(["-crf".into(), q.clone(), "-preset".into(), "8".into(), "-pix_fmt".into(), yuv420.into()]),
        // Constant quality needs Apple Silicon; on Intel Macs this fails and the job falls back to libx265.
        "hevc_videotoolbox" => {
            a.extend(["-q:v".into(), q.clone()]);
            a.extend(main10("yuv420p"));
        }
        "hevc_nvenc" => {
            a.extend(["-preset".into(), "p5".into(), "-rc".into(), "vbr".into(), "-cq".into(), q.clone()]);
            a.extend(["-b:v".into(), "0".into()]);
            a.extend(main10("yuv420p"));
        }
        "hevc_qsv" => {
            a.extend(["-global_quality".into(), q.clone()]);
            a.extend(main10("nv12"));
        }
        // Canvas: `-qp_i 24 -qp_p 26` at 균형, so I frames two steps finer than P frames.
        "hevc_amf" => {
            let qp_i = value.saturating_sub(2).to_string();
            a.extend(["-rc".into(), "cqp".into(), "-qp_i".into(), qp_i, "-qp_p".into(), q.clone()]);
            a.extend(main10("nv12"));
        }
        other => return Err(Msg::EncoderUnsupported { encoder: other.to_string() }),
    }
    Ok(a.into_iter().map(OsString::from).collect())
}

/// Color descriptions `setparams` accepts under the names ffprobe prints. Anything else is left
/// to the decoder (a name FFmpeg rejects would fail the whole job).
const PRIMARIES: &[&str] = &["bt709", "bt470m", "bt470bg", "smpte170m", "smpte240m", "film", "bt2020", "smpte431", "smpte432"];
const TRANSFERS: &[&str] = &[
    "bt709", "gamma22", "gamma28", "smpte170m", "smpte240m", "linear", "iec61966-2-1", "iec61966-2-4", "bt1361e",
    "bt2020-10", "bt2020-12", "smpte2084", "arib-std-b67",
];
/// YUV matrices only: an RGB source ("gbr") is converted to YUV, so its matrix does not carry over.
const MATRICES: &[&str] = &["bt709", "fcc", "bt470bg", "smpte170m", "smpte240m", "bt2020nc", "bt2020c"];

/// `-vf setparams=…` with the source's color primaries, transfer (PQ · HLG for HDR) and matrix, so
/// the result is described the same. A filter, not the `-color_*` output options: with FFmpeg 9.0.2
/// the encoder takes the frames' own description, and the options did not replace an unspecified
/// one (measured: `-color_trc smpte2084` on untagged frames → `unknown`; setparams → `smpte2084`).
pub fn color_args(video: &VideoStream) -> Vec<OsString> {
    match color_filter(video) {
        Some(f) => vec!["-vf".into(), f.into()],
        None => Vec::new(),
    }
}

/// The 화면 효과 filters in order: `crop=w:h:x:y` (upright source pixels, after autorotate) →
/// `transpose=clock` · `transpose=cclock` (a half turn is `hflip,vflip`) → `hflip` · `vflip` (as seen)
/// → `scale=W:H` + `setsar=1` (square pixels: a locked ratio or 맞춤 / 채움 may move the ratio by a
/// pixel, which `scale` would otherwise keep as a pixel aspect) → `pad=W:H:x:y` (맞춤, black). Every
/// number is even ([`effects::plan`]); the colors' `setparams` goes after these.
pub fn effect_filters(p: &EffectsPlan) -> Vec<String> {
    let mut f = Vec::new();
    if let Some(c) = p.crop {
        f.push(format!("crop={}:{}:{}:{}", c.width, c.height, c.x, c.y));
    }
    match p.turn {
        1 => f.push("transpose=clock".into()),
        3 => f.push("transpose=cclock".into()),
        _ => {}
    }
    if p.flip_h {
        f.push("hflip".into());
    }
    if p.flip_v {
        f.push("vflip".into());
    }
    if let Some([w, h]) = p.scale {
        f.push(format!("scale={w}:{h}"));
        f.push("setsar=1".into());
    }
    if let Some(pad) = p.pad {
        f.push(format!("pad={}:{}:{}:{}", pad.width, pad.height, pad.x, pad.y));
    }
    f
}

/// Half a millisecond: a mask's times are frame starts, and a frame whose time prints a hair under
/// its start must still count (the same reason as [`CUT_END_MARGIN`]).
const MASK_MARGIN: f64 = 0.0005;

/// `enable='gte(t,1.4995)*lt(t,3.9995)+…'`: on for frames starting in any of the result ranges
/// (start in, end out).
fn mask_enable(times: &[plan::Range]) -> String {
    let parts: Vec<String> = times
        .iter()
        .map(|r| format!("gte(t,{:.4})*lt(t,{:.4})", (r.start - MASK_MARGIN).max(0.0), r.end - MASK_MARGIN))
        .collect();
    format!("'{}'", parts.join("+"))
}

/// The inputs the PiP files add, in the order [`picture_graph`] numbers them: a video piece is
/// `-ss` · `-t` before its own `-i` (frame-exact like the cuts, so a cut that jumps the original
/// jumps the video too); an image is its one frame, which `overlay` holds (`enable` shows it).
/// A looped image (`-loop 1` at the rounded frame rate for the result's length) could have a frame
/// after the picture's last, and `overlay` then made one more picture frame (29.97 fps: 121 for
/// 120, measured with FFmpeg 9.0.2).
fn overlay_input_args(effects: Option<&EffectsPlan>) -> Vec<OsString> {
    let mut a: Vec<OsString> = Vec::new();
    for o in effects.map(|e| e.overlays.as_slice()).unwrap_or_default() {
        if is_video(o) {
            for p in &o.pieces {
                a.extend(["-ss".into(), exact_secs(p.source_offset).into(), "-t".into(), exact_secs(p.len).into()]);
                a.extend(["-i".into(), OsString::from(&o.path)]);
            }
        } else {
            a.extend(["-i".into(), OsString::from(&o.path)]);
        }
    }
    a
}

fn is_video(o: &OverlayPlan) -> bool {
    o.source.as_ref().is_some_and(|s| s.kind == OverlayKind::Video)
}

/// `scale`'s RGB → YUV matrix for a PiP so it matches the picture under it (a PNG would otherwise
/// be converted as BT.601); nothing when the source does not say.
fn overlay_matrix(video: &VideoStream) -> &'static str {
    match video.color_space.as_deref() {
        Some("bt709") => ":out_color_matrix=bt709:out_range=tv",
        Some("bt2020nc") | Some("bt2020c") => ":out_color_matrix=bt2020:out_range=tv",
        Some("smpte170m") | Some("bt470bg") => ":out_color_matrix=bt601:out_range=tv",
        _ => "",
    }
}

/// The filters that do not change with time: the 화면 효과's crop · turn · flip · scale · pad, then
/// the source's colors (`setparams`).
fn picture_layer(video: &VideoStream, effects: Option<&EffectsPlan>) -> Vec<String> {
    let mut chain = effects.map(effect_filters).unwrap_or_default();
    chain.extend(color_filter(video));
    chain
}

/// How many inputs [`overlay_input_args`] adds for `effects`.
fn overlay_inputs(effects: Option<&EffectsPlan>) -> usize {
    effects.map_or(0, |e| e.overlays.iter().map(|o| if is_video(o) { o.pieces.len() } else { 1 }).sum())
}

/// The re-encoded picture's filter graph, from `from` (a label, or None for `-vf`'s own input) to
/// `to` (None for `-vf`'s output): 모자이크 · 블러, PiP, then — when `picture` — the other 화면 효과
/// and the source's colors ([`picture_layer`]). None when there is nothing to do. The PiP inputs are
/// numbered from `first_input` ([`overlay_input_args`]); with PiP, `from` must be a label (a
/// `-filter_complex`). `tag` keeps the labels of several graphs in one `-filter_complex` apart
/// (one per kept piece of a 구간 제거).
///
/// Each mask (source pixels, before the crop): `split` off a copy, `crop` the box, shrink it to its
/// cells (`flags=area` — the average) and blow it back up (`flags=neighbor` — square cells) or
/// `gblur` it, and `overlay` it back on its spot while `enable` says (result timeline). `overlay`
/// works in 8-bit `yuva420p` by default, which would cut a 10-bit picture down: `format=yuv420p10`
/// for 10-bit · HDR sources keeps the picture as it is and converts only the box (measured, FFmpeg
/// 9.0.2). Labels are `fx…`, so they cannot meet the concat's.
///
/// Each PiP: its input scaled to the box (the picture's matrix), with alpha (`lutyuv` for the
/// opacity), laid over while `enable` says. A video piece is moved to its result time with `setpts`
/// and held on its last frame to the end of its window, so it neither shows early nor drops its
/// last frame a step early (measured): `tpad` clones that frame on and on, and `shortest=1` ends
/// the picture with the original's last frame. With `eof_action=repeat` instead, a piece whose last
/// frame came after the original's last (a video running to the result's end) made `overlay`
/// repeat the original's last frame once more — it goes on while any input has frames.
fn picture_graph(
    video: &VideoStream,
    effects: Option<&EffectsPlan>,
    from: Option<&str>,
    to: Option<&str>,
    first_input: usize,
    picture: bool,
    tag: &str,
) -> Option<String> {
    let chain = if picture { picture_layer(video, effects) } else { Vec::new() };
    let label = |l: Option<&str>| l.map(|l| format!("[{l}]")).unwrap_or_default();
    let masks = effects.map(|e| e.masks.as_slice()).unwrap_or_default();
    let overlays = effects.map(|e| e.overlays.as_slice()).unwrap_or_default();
    if masks.is_empty() && overlays.is_empty() {
        return (!chain.is_empty()).then(|| format!("{}{}{}", label(from), chain.join(","), label(to)));
    }
    let ten = video.wants_ten_bit();
    let format = if ten { "yuv420p10" } else { "yuv420" };
    let mut stmts: Vec<String> = Vec::new();
    let mut cur: Option<String> = from.map(String::from);
    let mut n = 0;
    if !masks.is_empty() {
        let regions: String = (0..masks.len()).map(|i| format!("[fxr{tag}{i}]")).collect();
        stmts.push(format!("{}split={}[fx{tag}0]{regions}", label(cur.as_deref()), masks.len() + 1));
        for (i, m) in masks.iter().enumerate() {
            let r = m.rect;
            let process = match m.filter {
                MaskFilter::Mosaic { cells: [cw, ch] } => format!("scale={cw}:{ch}:flags=area,scale={}:{}:flags=neighbor", r.width, r.height),
                MaskFilter::Blur { sigma } => format!("gblur=sigma={sigma}:steps=3"),
            };
            stmts.push(format!("[fxr{tag}{i}]crop={}:{}:{}:{},{process}[fxp{tag}{i}]", r.width, r.height, r.x, r.y));
            stmts.push(format!("[fx{tag}{i}][fxp{tag}{i}]overlay={}:{}:format={format}:enable={}[fx{tag}{}]", r.x, r.y, mask_enable(&m.times), i + 1));
        }
        n = masks.len();
        cur = Some(format!("fx{tag}{n}"));
    }
    let alpha_format = if ten { "yuva420p10le" } else { "yuva420p" };
    let matrix = overlay_matrix(video);
    // A PiP stands upright in the result: the picture is turned and mirrored after it, so each file
    // is mirrored the same way and turned back first (flips undo themselves), scaled to the box as
    // seen — its height and width swapped by a quarter turn.
    let (turn, flip_h, flip_v) = effects.map_or((0, false, false), |e| (e.turn, e.flip_h, e.flip_v));
    let upright = [flip_h.then_some(",hflip"), flip_v.then_some(",vflip"), (turn == 1).then_some(",transpose=cclock"), (turn == 3).then_some(",transpose=clock")]
        .into_iter()
        .flatten()
        .collect::<String>();
    let mut input = first_input;
    for (j, o) in overlays.iter().enumerate() {
        let r = o.rect;
        let alpha = if o.opacity < 100 { format!(",lutyuv=a='val*{:.2}'", o.opacity as f64 / 100.0) } else { String::new() };
        let (sw, sh) = if turn % 2 == 1 { (r.height, r.width) } else { (r.width, r.height) };
        let fit = format!("scale={sw}:{sh}{matrix},format={alpha_format}{alpha}{upright}");
        if is_video(o) {
            for (p, piece) in o.pieces.iter().enumerate() {
                stmts.push(format!("[{input}:v]setpts=PTS-STARTPTS+{:.6}/TB,{fit},tpad=stop_mode=clone:stop=-1[fxo{tag}{j}_{p}]", piece.start));
                let window = mask_enable(&[plan::Range::new(piece.start, piece.start + piece.len)]);
                n += 1;
                stmts.push(format!("{}[fxo{tag}{j}_{p}]overlay={}:{}:format={format}:shortest=1:enable={window}[fx{tag}{n}]", label(cur.as_deref()), r.x, r.y));
                cur = Some(format!("fx{tag}{n}"));
                input += 1;
            }
        } else {
            stmts.push(format!("[{input}:v]{fit}[fxo{tag}{j}]"));
            n += 1;
            stmts.push(format!("{}[fxo{tag}{j}]overlay={}:{}:format={format}:enable={}[fx{tag}{n}]", label(cur.as_deref()), r.x, r.y, mask_enable(&o.times)));
            cur = Some(format!("fx{tag}{n}"));
            input += 1;
        }
    }
    let rest = if chain.is_empty() { "null".to_string() } else { chain.join(",") };
    stmts.push(format!("{}{rest}{}", label(cur.as_deref()), label(to)));
    Some(stmts.join(";"))
}

/// Puts a sound's first sample at time 0 of its timeline: silence fills a start that is late (an AAC
/// opened with `-ss 0` can lose its first frame and begin at 0.023 s), a start that is early is cut.
/// `amix` mixes sample by sample and ignores timestamps, so every input to it goes through this.
const AUDIO_FROM_ZERO: &str = "aresample=async=1:min_hard_comp=0.001:first_pts=0";

/// The PiP videos' sound mixed into `base` (an audio label) as `out`: each piece delayed to its
/// result time, at its volume, then `amix` without normalizing (the original keeps its level) for
/// as long as the original. None when no PiP mixes sound. Inputs numbered and labels tagged as in
/// [`picture_graph`].
fn overlay_audio_graph(effects: Option<&EffectsPlan>, base: &str, out: &str, first_input: usize, tag: &str) -> Option<String> {
    let mut stmts = Vec::new();
    let mut mixed = Vec::new();
    let mut input = first_input;
    for (j, o) in effects.map(|e| e.overlays.as_slice()).unwrap_or_default().iter().enumerate() {
        if !is_video(o) {
            input += 1;
            continue;
        }
        for (p, piece) in o.pieces.iter().enumerate() {
            if let Some(volume) = o.volume {
                let delay = (piece.start * 1000.0).round() as u64;
                stmts.push(format!(
                    "[{input}:a]{AUDIO_FROM_ZERO},adelay=delays={delay}:all=1,volume={:.2}[fxa{tag}{j}_{p}]",
                    volume as f64 / 100.0
                ));
                mixed.push(format!("[fxa{tag}{j}_{p}]"));
            }
            input += 1;
        }
    }
    if mixed.is_empty() {
        return None;
    }
    stmts.push(format!("[{base}]{AUDIO_FROM_ZERO}[fxb{tag}]"));
    stmts.push(format!("[fxb{tag}]{}amix=inputs={}:duration=first:dropout_transition=0:normalize=0[{out}]", mixed.join(""), mixed.len() + 1));
    Some(stmts.join(";"))
}

/// The PiP sound is mixed into the result (some PiP video with sound has `volume`).
pub fn mixes_overlay_audio(effects: Option<&EffectsPlan>) -> bool {
    effects.is_some_and(|e| e.overlays.iter().any(|o| is_video(o) && o.volume.is_some()))
}

/// The `setparams=…` filter of [`color_args`]; None when the source describes no known colors.
fn color_filter(video: &VideoStream) -> Option<String> {
    let pick = |v: &Option<String>, allowed: &[&str]| v.clone().filter(|s| allowed.contains(&s.as_str()));
    let params: Vec<String> = [
        ("color_primaries", pick(&video.color_primaries, PRIMARIES)),
        ("color_trc", pick(&video.color_transfer, TRANSFERS)),
        ("colorspace", pick(&video.color_space, MATRICES)),
    ]
    .into_iter()
    .filter_map(|(key, value)| value.map(|v| format!("{key}={v}")))
    .collect();
    (!params.is_empty()).then(|| format!("setparams={}", params.join(":")))
}

/// Re-encodes the range of `plan` with `encoder` (one FFmpeg process, canvas 03 · 3.2 C).
///
/// `-ss` goes before `-i` and `-t` (an output option) after the last input, with [`exact_secs`]
/// (`-t` ends [`CUT_END_MARGIN`] early, for containers that round frame times): when
/// transcoding, FFmpeg decodes from the keyframe before `-ss` and drops the frames before it, so
/// the result starts on the frame at IN and ends before the frame at OUT — also for B-frame sources,
/// where stream copy cannot (measured with FFmpeg 9.0.2: x264 `-bf 2` MKV, IN #62 · OUT #155 → 93
/// frames). `-fps_mode passthrough` keeps every source timestamp (no frames added or dropped for
/// variable-frame-rate phone video). All audio tracks go along (AAC 192k unless copied).
pub fn precise_trim_args(
    input: &Path,
    video: &VideoStream,
    plan: &PreciseTrimPlan,
    encoder: &str,
    quality: Quality,
    output: &Path,
) -> Result<Vec<OsString>, Msg> {
    let mut a = job_prefix();
    if let Some(seek) = plan.seek {
        a.extend(["-ss".into(), exact_secs(seek).into()]);
    }
    a.extend(["-i".into(), input.as_os_str().to_owned()]);
    let effects = plan.effects.as_ref();
    let pip = effects.is_some_and(|e| !e.overlays.is_empty());
    // PiP brings more inputs: a -filter_complex with the source's streams by number.
    a.extend(overlay_input_args(effects));
    // The output's length, after the last input: right after the source's `-i` it would belong to
    // the next input — a PiP file, whose own `-t` replaced it, and the result ran on to the end of
    // the source (measured; the 완료 기준 test counts the frames).
    if plan.seek.is_some() {
        a.extend(["-t".into(), precise_len(plan.duration).into()]);
    }
    let mix = match (plan.audio, plan.first_audio) {
        (AudioPlan::None, _) | (_, None) => None,
        (_, Some(first)) => overlay_audio_graph(effects, &format!("0:{first}"), "fxmix", 1, ""),
    };
    if pip {
        let v = format!("0:{}", video.index);
        let mut graph = picture_graph(video, effects, Some(&v), Some("v"), 1, true, "").expect("PiP is something to do");
        if let Some(m) = &mix {
            graph.push(';');
            graph.push_str(m);
        }
        a.extend(["-filter_complex".into(), graph.into(), "-map".into(), "[v]".into()]);
    } else {
        a.extend(["-map".into(), format!("0:{}", video.index).into()]);
    }
    if plan.audio != AudioPlan::None {
        if mix.is_some() {
            // The first track mixed with the PiP sound, the others as they are.
            a.extend(["-map".into(), "[fxmix]".into(), "-map".into(), "0:a?".into(), "-map".into(), "-0:a:0".into()]);
        } else {
            a.extend(["-map".into(), "0:a?".into()]);
        }
    }
    a.extend(video_encoder_args(encoder, quality, plan.ten_bit)?);
    let mp4 = is_mp4_family(output);
    if plan.codec == VideoCodec::Hevc && mp4 {
        a.extend(["-tag:v".into(), "hvc1".into()]);
    }
    if !pip && let Some(graph) = picture_graph(video, effects, None, None, 1, true, "") {
        a.extend(["-vf".into(), graph.into()]);
    }
    a.extend(KEEP_FRAME_TIMES.map(OsString::from));
    match plan.audio {
        AudioPlan::None => {}
        AudioPlan::Copy => a.extend(["-c:a".into(), "copy".into()]),
        AudioPlan::Aac => a.extend(["-c:a".into(), "aac".into(), "-b:a".into(), AUDIO_AAC_BITRATE.into()]),
    }
    a.extend(["-map_metadata".into(), "0".into()]);
    if mp4 {
        a.extend(["-movflags".into(), "+faststart".into()]);
    }
    a.push(output.as_os_str().to_owned());
    Ok(a)
}

/// The time layers (masks · PiP) of each kept piece on its own timeline ([`EffectsPlan::in_piece`]);
/// None for a piece they leave alone.
fn piece_layers(keep: &[plan::Range], effects: Option<&EffectsPlan>) -> Vec<Option<EffectsPlan>> {
    let mut start = 0.0;
    keep.iter()
        .map(|k| {
            let local = effects.map(|e| e.in_piece(start, k.len())).filter(|l| !l.masks.is_empty() || !l.overlays.is_empty());
            start += k.len();
            local
        })
        .collect()
}

/// `[0:0][fa0_0][fz0][1:0][fa1_0][fz1]concat=n=2:v=1:a=2[v][a0][fz]` (+ `;[vc]crop=…,setparams=…[v]`
/// for the 화면 효과 and the colors): the video and every audio track of each kept piece, joined in
/// order. Related streams go through one concat so a piece whose audio ends early is padded with
/// silence and the next piece stays in sync.
///
/// Each piece's sound is put at 0 first ([`AUDIO_FROM_ZERO`] → `[fa i_t]`): a piece that starts on a
/// keyframe gets its sound from the next AAC packet after it (MP4 · MOV seeking — measured 5.3 and
/// 10.7 ms late), `concat` keeps that as a gap in the timestamps only, and a player playing the
/// samples back to back heard every later piece that much early (4K: 26.7 ms after three joins).
///
/// `[fz i]` is silence exactly as long as the piece (`anullsrc,atrim`), joined along and dropped
/// (`anullsink`): `concat` ends a piece where its longest stream ends, and it reckons a picture's
/// end from the average frame spacing — right for a constant rate, not for a variable one, where a
/// piece ended early (measured, FFmpeg 9.0.2: by the 1 ms the sound is cut short — `-t`,
/// [`CUT_END_MARGIN`] — and by 23 ms in a file without sound), and everything after a join moved.
///
/// Masks and PiP run on each piece before the join, on the piece's own timeline (`layers`, numbered
/// from the inputs after the pieces, PiP sound mixed into the piece's first track); crop · turn ·
/// size and the colors run once, on the joined picture.
fn concat_graph(keep: &[plan::Range], video: &VideoStream, audio: &[crate::probe::AudioStream], effects: Option<&EffectsPlan>, layers: &[Option<EffectsPlan>]) -> String {
    let pieces = keep.len();
    let mut stmts = Vec::new();
    let mut join = String::new();
    let mut input = pieces;
    for (i, (k, local)) in keep.iter().zip(layers).enumerate() {
        let tag = format!("s{i}_");
        let own = format!("{i}:{}", video.index);
        match picture_graph(video, local.as_ref(), Some(&own), Some(&format!("fv{i}")), input, false, &tag) {
            Some(g) => {
                stmts.push(g);
                join.push_str(&format!("[fv{i}]"));
            }
            None => join.push_str(&format!("[{own}]")),
        }
        for (t, a) in audio.iter().enumerate() {
            let own = format!("{i}:{}", a.index);
            match (t, overlay_audio_graph(local.as_ref(), &own, &format!("fm{i}"), input, &tag)) {
                (0, Some(m)) => {
                    stmts.push(m);
                    join.push_str(&format!("[fm{i}]"));
                }
                _ => {
                    stmts.push(format!("[{own}]{AUDIO_FROM_ZERO}[fa{i}_{t}]"));
                    join.push_str(&format!("[fa{i}_{t}]"));
                }
            }
        }
        stmts.push(format!("anullsrc=r=48000:cl=mono,atrim=end={}[fz{i}]", exact_secs(k.len())));
        join.push_str(&format!("[fz{i}]"));
        input += overlay_inputs(local.as_ref());
    }
    let layer = picture_layer(video, effects);
    let outs: String = (0..audio.len()).map(|t| format!("[a{t}]")).collect();
    let picture = if layer.is_empty() { "[v]" } else { "[vc]" };
    stmts.push(format!("{join}concat=n={pieces}:v=1:a={}{picture}{outs}[fz]", audio.len() + 1));
    stmts.push("[fz]anullsink".to_string());
    if !layer.is_empty() {
        stmts.push(format!("[vc]{}[v]", layer.join(",")));
    }
    stmts.join(";")
}

/// 구간 제거 in one FFmpeg process (canvas 03 · 3.2 C): every kept piece is its own input with
/// `-ss` · `-t` before `-i`, exactly like [`precise_trim_args`], so each piece is frame-exact and
/// the removed parts are never decoded; one `concat` filter joins them. A single piece is a trim.
///
/// Filter outputs carry no stream metadata, so the audio languages are set again; chapters are left
/// out (their times would point at the wrong places after the cuts). Encoder, quality, `hvc1`,
/// `-fps_mode passthrough` and AAC are the trim's.
pub fn precise_cut_args(
    input: &Path,
    info: &MediaInfo,
    plan: &PreciseCutPlan,
    encoder: &str,
    quality: Quality,
    output: &Path,
) -> Result<Vec<OsString>, Msg> {
    let video = info.video.as_ref().ok_or(Msg::PreciseNeedsVideo)?;
    if let Some(trim) = plan.as_trim() {
        return precise_trim_args(input, video, &trim, encoder, quality, output);
    }
    if plan.keep.is_empty() {
        return Err(Msg::NothingKept);
    }
    let audio: &[crate::probe::AudioStream] = if plan.audio == AudioPlan::None { &[] } else { &info.audio };
    let mut a = job_prefix();
    for k in &plan.keep {
        a.extend(["-ss".into(), exact_secs(k.start).into(), "-t".into(), precise_len(k.len()).into()]);
        a.extend(["-i".into(), input.as_os_str().to_owned()]);
    }
    // The pieces' PiP inputs, piece by piece, as concat_graph numbers them.
    let layers = piece_layers(&plan.keep, plan.effects.as_ref());
    for local in &layers {
        a.extend(overlay_input_args(local.as_ref()));
    }
    a.extend(["-filter_complex".into(), concat_graph(&plan.keep, video, audio, plan.effects.as_ref(), &layers).into()]);
    a.extend(["-map".into(), "[v]".into()]);
    for k in 0..audio.len() {
        a.extend(["-map".into(), format!("[a{k}]").into()]);
    }
    a.extend(video_encoder_args(encoder, quality, plan.ten_bit)?);
    let mp4 = is_mp4_family(output);
    if plan.codec == VideoCodec::Hevc && mp4 {
        a.extend(["-tag:v".into(), "hvc1".into()]);
    }
    a.extend(KEEP_FRAME_TIMES.map(OsString::from));
    if !audio.is_empty() {
        a.extend(["-c:a".into(), "aac".into(), "-b:a".into(), AUDIO_AAC_BITRATE.into()]);
    }
    for (k, track) in audio.iter().enumerate() {
        if let Some(lang) = track.language.as_deref().filter(|l| !l.is_empty() && *l != "und") {
            a.extend([format!("-metadata:s:a:{k}").into(), format!("language={lang}").into()]);
        }
    }
    a.extend(["-map_metadata".into(), "0".into(), "-map_chapters".into(), "-1".into()]);
    if mp4 {
        a.extend(["-movflags".into(), "+faststart".into()]);
    }
    a.push(output.as_os_str().to_owned());
    Ok(a)
}

// ---------------------------------------------------------------------------------------------
// 일괄 변환 (Phase 3 step 2, canvas Convert.dc.html): a whole file re-encoded to H.265 or AV1 with
// the convert screen's settings. It is the precise export of a whole file (no `-ss` · `-t`,
// `-fps_mode passthrough`, colors, 10 bits, `hvc1`, faststart) plus the container, a downscale and
// the audio choice.

/// The convert screen's 컨테이너. H.265: MP4 · MKV · MOV; AV1: MP4 · MKV · WebM (canvas).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Container {
    Mp4,
    Mkv,
    Mov,
    Webm,
}

impl Container {
    pub fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
            Container::Mov => "mov",
            Container::Webm => "webm",
        }
    }

    fn holds(self, codec: VideoCodec) -> bool {
        match codec {
            VideoCodec::Hevc => matches!(self, Container::Mp4 | Container::Mkv | Container::Mov),
            VideoCodec::Av1 => matches!(self, Container::Mp4 | Container::Mkv | Container::Webm),
            VideoCodec::H264 => self != Container::Webm,
        }
    }

    /// Audio codecs (ffprobe names) a stream copy may put in this container. MP4 keeps to what
    /// Apple players and browsers take; MOV adds the PCM cameras record; WebM takes Opus · Vorbis only.
    fn copies(self, audio_codec: &str) -> bool {
        match self {
            Container::Mp4 => matches!(audio_codec, "aac" | "mp3" | "ac3" | "eac3" | "alac"),
            Container::Mov => matches!(
                audio_codec,
                "aac" | "mp3" | "ac3" | "eac3" | "alac" | "pcm_s16le" | "pcm_s16be" | "pcm_s24le" | "pcm_s24be"
            ),
            Container::Mkv => matches!(
                audio_codec,
                "aac" | "mp3" | "ac3" | "eac3" | "opus" | "vorbis" | "flac" | "alac" | "dts" | "truehd" | "pcm_s16le" | "pcm_s24le"
            ),
            Container::Webm => matches!(audio_codec, "opus" | "vorbis"),
        }
    }

    fn mp4_family(self) -> bool {
        matches!(self, Container::Mp4 | Container::Mov)
    }
}

/// 해상도: keep it, or bring the short side down to 1080 / 720 (never up).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Resolution {
    Source,
    P1080,
    P720,
}

impl Resolution {
    fn short_side(self) -> Option<u32> {
        match self {
            Resolution::Source => None,
            Resolution::P1080 => Some(1080),
            Resolution::P720 => Some(720),
        }
    }
}

/// 오디오: "가능하면 복사, 아니면 AAC 192k" · "항상 AAC 192k" · "항상 AAC 128k".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioChoice {
    CopyOrAac,
    Aac192,
    Aac128,
}

/// What a conversion does with the audio tracks (all of them alike).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AudioOut {
    None,
    Copy,
    /// kbit/s per track.
    Aac { bitrate: u32 },
    Opus { bitrate: u32 },
}

/// Opus for WebM, whatever bitrate was picked for AAC (canvas: "WebM은 오디오를 Opus 128k로").
pub const OPUS_BITRATE: u32 = 128;

/// Channel layouts libopus takes (the Vorbis ones). It refuses others — 5.1(side), common in AC-3
/// and camera audio, failed to open the encoder (FFmpeg 9.0.2) — so `aformat` converts to the
/// nearest of these first (5.1(side) → 5.1: the side channels go to the back ones, same level).
const OPUS_LAYOUTS: &str = "aformat=channel_layouts=mono|stereo|3.0|quad|5.0|5.1|6.1|7.1";

/// The convert screen's settings that shape the FFmpeg command (the encoder and quality are
/// chosen next to it, as for the precise export).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertOptions {
    pub codec: VideoCodec,
    pub container: Container,
    pub resolution: Resolution,
    pub audio: AudioChoice,
    /// 10비트 also for 8-bit sources (10-bit and HDR sources are always kept in 10 bits).
    pub ten_bit: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertPlan {
    pub codec: VideoCodec,
    pub ten_bit: bool,
    /// `scale=…` when the picture is made smaller; None keeps the source size.
    pub scale: Option<String>,
    /// The picture as shown, after the downscale: [width, height].
    pub size: [u32; 2],
    pub audio: AudioOut,
    pub container: Container,
    /// The whole file's length.
    pub duration: f64,
}

/// `scale=W:H` to `size` (the size [`effects::short_side_size`] works out for 해상도: the same
/// numbers `scale=-2:{short}` makes, written out).
fn scale_filter(size: [u32; 2]) -> String {
    format!("scale={}:{}", size[0], size[1])
}

/// 일괄 변환 of one file: codec, bit depth, size and audio from `opts`.
pub fn plan_convert(info: &MediaInfo, opts: &ConvertOptions) -> Result<ConvertPlan, Msg> {
    let video = info.video.as_ref().ok_or(Msg::ConvertNeedsVideo)?;
    if opts.codec == VideoCodec::H264 {
        return Err(Msg::ConvertCodecUnsupported);
    }
    if !opts.container.holds(opts.codec) {
        let codec = if opts.codec == VideoCodec::Hevc { "H.265" } else { "AV1" };
        return Err(Msg::CodecNotInContainer {
            codec: codec.to_string(),
            container: opts.container.extension().to_uppercase(),
        });
    }
    if !(info.duration.is_finite() && info.duration > 0.0) {
        return Err(Msg::DurationUnknown);
    }
    let (w, h) = video.upright_size();
    let (scale, size) = match opts.resolution.short_side().and_then(|s| effects::short_side_size(w, h, s)) {
        Some(size) => (Some(scale_filter(size)), size),
        None => (None, [w, h]),
    };
    let copyable = !info.audio.is_empty() && info.audio.iter().all(|a| opts.container.copies(&a.codec));
    let audio = if info.audio.is_empty() {
        AudioOut::None
    } else if opts.audio == AudioChoice::CopyOrAac && copyable {
        AudioOut::Copy
    } else if opts.container == Container::Webm {
        AudioOut::Opus { bitrate: OPUS_BITRATE }
    } else {
        AudioOut::Aac { bitrate: if opts.audio == AudioChoice::Aac128 { 128 } else { 192 } }
    };
    Ok(ConvertPlan {
        codec: opts.codec,
        ten_bit: opts.ten_bit || video.wants_ten_bit(),
        scale,
        size,
        audio,
        container: opts.container,
        duration: info.duration,
    })
}

/// One FFmpeg process for the whole file: [`precise_trim_args`] without `-ss` · `-t`, with the
/// downscale before the colors in one `-vf` (measured: the same pixels and color tags in either
/// order, HDR 10-bit through libx265 and VideoToolbox), the container's tag and flags, and the
/// planned audio. Video and every audio track; subtitles, data tracks and attached pictures stay out.
pub fn convert_args(
    input: &Path,
    video: &VideoStream,
    plan: &ConvertPlan,
    encoder: &str,
    quality: Quality,
    output: &Path,
) -> Result<Vec<OsString>, Msg> {
    convert_args_over(input, video, plan, encoder, quality, None, output)
}

/// 예상 크기 (Phase 3 step 3): [`convert_args`] for the stretch `sample` of the file only — the same
/// conversion with `-ss` before `-i` and `-t` after it, as the precise trim seeks (FFmpeg decodes from
/// the keyframe before and drops what is before `-ss`). A stretch that covers the whole file is the
/// whole conversion. The test's result is only measured, so it goes to a scratch `.part` too.
pub fn convert_sample_args(
    input: &Path,
    video: &VideoStream,
    plan: &ConvertPlan,
    encoder: &str,
    quality: Quality,
    sample: plan::Range,
    output: &Path,
) -> Result<Vec<OsString>, Msg> {
    let whole = sample.start <= 0.0 && sample.end >= plan.duration;
    convert_args_over(input, video, plan, encoder, quality, (!whole).then_some(sample), output)
}

fn convert_args_over(
    input: &Path,
    video: &VideoStream,
    plan: &ConvertPlan,
    encoder: &str,
    quality: Quality,
    sample: Option<plan::Range>,
    output: &Path,
) -> Result<Vec<OsString>, Msg> {
    let mut a = job_prefix();
    if let Some(s) = sample {
        a.extend(["-ss".into(), exact_secs(s.start.max(0.0)).into()]);
    }
    a.extend(["-i".into(), input.as_os_str().to_owned()]);
    if let Some(s) = sample {
        a.extend(["-t".into(), exact_secs(s.len()).into()]);
    }
    a.extend(["-map".into(), format!("0:{}", video.index).into()]);
    if plan.audio != AudioOut::None {
        a.extend(["-map".into(), "0:a?".into()]);
    }
    a.extend(video_encoder_args(encoder, quality, plan.ten_bit)?);
    let mp4 = plan.container.mp4_family();
    if plan.codec == VideoCodec::Hevc && mp4 {
        a.extend(["-tag:v".into(), "hvc1".into()]);
    }
    let filters: Vec<String> = plan.scale.iter().cloned().chain(color_filter(video)).collect();
    if !filters.is_empty() {
        a.extend(["-vf".into(), filters.join(",").into()]);
    }
    a.extend(KEEP_FRAME_TIMES.map(OsString::from));
    match plan.audio {
        AudioOut::None => {}
        AudioOut::Copy => a.extend(["-c:a".into(), "copy".into()]),
        AudioOut::Aac { bitrate } => a.extend(["-c:a".into(), "aac".into(), "-b:a".into(), format!("{bitrate}k").into()]),
        AudioOut::Opus { bitrate } => {
            a.extend(["-af".into(), OPUS_LAYOUTS.into()]);
            a.extend(["-c:a".into(), "libopus".into(), "-b:a".into(), format!("{bitrate}k").into()]);
        }
    }
    a.extend(["-map_metadata".into(), "0".into()]);
    if mp4 {
        a.extend(["-movflags".into(), "+faststart".into()]);
    }
    a.push(output.as_os_str().to_owned());
    Ok(a)
}

/// Characters a name ending may not hold: path separators, what Windows refuses in file names, and
/// control characters. ".part" would read as a job's unfinished file.
pub fn check_name_suffix(suffix: &str) -> Result<(), Msg> {
    if suffix.chars().count() > 60 {
        return Err(Msg::SuffixTooLong { max: 60 });
    }
    if let Some(c) = suffix.chars().find(|c| c.is_control() || "/\\:*?\"<>|".contains(*c)) {
        return Err(Msg::SuffixBadChar { ch: (!c.is_control()).then(|| c.to_string()) });
    }
    if suffix.to_lowercase().contains(".part") {
        return Err(Msg::SuffixPart);
    }
    Ok(())
}

/// Final output path `{stem}{suffix}.{ext}` next to the input, never overwriting an existing file,
/// and the `.part` path FFmpeg writes to until the job succeeds.
pub fn output_paths(input: &Path, suffix: &str) -> (PathBuf, PathBuf) {
    output_paths_as(input, suffix, None)
}

/// [`output_paths`] with another extension (container) than the input's.
pub fn output_paths_as(input: &Path, suffix: &str, extension: Option<&str>) -> (PathBuf, PathBuf) {
    output_paths_avoiding(input, suffix, extension, &|_| false)
}

/// [`output_paths_as`] that also skips names `taken` holds (the final or `.part` path of a queued
/// job), so two queued exports of the same file never write the same `.part`.
pub fn output_paths_avoiding(input: &Path, suffix: &str, extension: Option<&str>, taken: &dyn Fn(&Path) -> bool) -> (PathBuf, PathBuf) {
    let dir = input.parent().map(Path::to_path_buf).unwrap_or_default();
    output_paths_in(&dir, input, suffix, extension, taken)
}

/// [`output_paths_avoiding`] in another folder than the input's (일괄 변환's 지정 폴더). With an
/// empty suffix in the input's own folder the name is the original's, which exists: the result then
/// gets ` (1)`, so the original is never written over.
pub fn output_paths_in(dir: &Path, input: &Path, suffix: &str, extension: Option<&str>, taken: &dyn Fn(&Path) -> bool) -> (PathBuf, PathBuf) {
    let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "output".into());
    let ext = extension
        .map(str::to_string)
        .or_else(|| input.extension().map(|e| e.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "mkv".into());
    let mut n = 0;
    loop {
        let stem = if n == 0 { format!("{stem}{suffix}") } else { format!("{stem}{suffix} ({n})") };
        let final_path = dir.join(format!("{stem}.{ext}"));
        let part = dir.join(format!("{stem}.part.{ext}"));
        if !final_path.exists() && !taken(&final_path) && !taken(&part) {
            return (final_path, part);
        }
        n += 1;
    }
}

/// For display only: the argument list as one line, quoting what needs quoting.
pub fn display_command(program: &Path, args: &[OsString]) -> String {
    let quote = |s: &str| {
        if s.is_empty() || s.chars().any(|c| c.is_whitespace() || "\"'$`\\;&|<>()*?[]{}".contains(c)) {
            format!("\"{}\"", s.replace('"', "\\\""))
        } else {
            s.to_string()
        }
    };
    std::iter::once(program.as_os_str())
        .chain(args.iter().map(OsString::as_os_str))
        .map(|s| quote(&s.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(a: &[OsString]) -> Vec<String> {
        a.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn lossless_trim_snapshot() {
        let a = lossless_trim_args(
            Path::new("/v/제주 여행.mov"),
            Some(0),
            65.4,
            434.6,
            Path::new("/v/제주 여행_trim.part.mov"),
        );
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-ss", "65.400000", "-i", "/v/제주 여행.mov", "-t", "434.600000",
                "-map", "0:0", "-map", "0:a?", "-c", "copy", "-map_metadata", "0",
                "-movflags", "+faststart",
                "/v/제주 여행_trim.part.mov",
            ]
        );
    }

    #[test]
    fn cut_times_never_round_past_a_frame_start() {
        // 30 fps frame #7355 starts at 245.1666… s: the old millisecond rounding sent 245.167.
        assert_eq!(exact_secs(7355.0 / 30.0), "245.166666");
        assert_eq!(exact_secs(13705.0 / 30.0 - 7355.0 / 30.0), "211.666666");
        // Whole values survive float noise; negatives clamp like `secs`.
        assert_eq!(exact_secs(2.4), "2.400000");
        assert_eq!(exact_secs(0.1 + 0.2), "0.300000");
        assert_eq!(exact_secs(65.4), "65.400000");
        assert_eq!(exact_secs(-0.5), "0.000000");
        for fps in [23.976, 24.0, 25.0, 29.97, 30.0, 50.0, 59.94, 60.0] {
            for frame in [1u32, 2, 62, 155, 7355, 13705, 107_892] {
                let t = f64::from(frame) / fps;
                let sent: f64 = exact_secs(t).parse().unwrap();
                assert!(sent <= t && t - sent < 1.001e-6, "{fps} fps frame {frame}: {t} → {sent}");
            }
        }
    }

    #[test]
    fn lossless_trim_sends_frame_exact_times() {
        let a = strs(&lossless_trim_args(Path::new("a.mp4"), Some(0), 7355.0 / 30.0, 6350.0 / 30.0, Path::new("a_trim.part.mp4")));
        assert_eq!(a[7..9], ["-ss", "245.166666"]);
        assert_eq!(a[11..13], ["-t", "211.666666"]);
    }

    #[test]
    fn mkv_output_has_no_movflags() {
        let a = strs(&lossless_trim_args(Path::new("a.mkv"), None, 0.0, 5.0, Path::new("a_trim.part.mkv")));
        assert!(!a.iter().any(|s| s == "-movflags"));
        assert!(!a.iter().any(|s| s == "-avoid_negative_ts"), "see the lossless_trim_args doc comment");
        assert!(a.contains(&"0:v:0?".to_string()));
    }

    #[test]
    fn output_paths_avoid_collisions() {
        let dir = std::env::temp_dir().join(format!("godseon-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("클립.mp4");
        let (f1, p1) = output_paths(&input, "_trim");
        assert_eq!(f1.file_name().unwrap(), "클립_trim.mp4");
        assert_eq!(p1.file_name().unwrap(), "클립_trim.part.mp4");
        std::fs::write(&f1, b"x").unwrap();
        let (f2, p2) = output_paths(&input, "_trim");
        assert_eq!(f2.file_name().unwrap(), "클립_trim (1).mp4");
        assert_eq!(p2.file_name().unwrap(), "클립_trim (1).part.mp4");
        // Names a queued job holds are skipped too: its final name, or only its `.part`.
        let held = [dir.join("클립_trim (1).mp4"), dir.join("클립_trim (2).part.mp4")];
        let (f3, p3) = output_paths_avoiding(&input, "_trim", None, &|p| held.iter().any(|h| h == p));
        assert_eq!((f3.file_name().unwrap(), p3.file_name().unwrap()), ("클립_trim (3).mp4".as_ref(), "클립_trim (3).part.mp4".as_ref()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn media(format_name: &str, has_b_frames: u32) -> MediaInfo {
        MediaInfo {
            path: "a".into(),
            file_name: "a".into(),
            size_bytes: None,
            duration: 10.0,
            format_name: format_name.into(),
            bit_rate: None,
            video: Some(crate::probe::VideoStream {
                index: 0,
                codec: "h264".into(),
                profile: None,
                width: 320,
                height: 240,
                start_time: Some(0.0),
                has_b_frames,
                fps: Some(25.0),
                avg_fps: Some(25.0),
                pix_fmt: None,
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

    const KF: [f64; 4] = [0.0, 1.0, 2.0, 3.0];
    const MOV: &str = "mov,mp4,m4a,3gp,3g2,mj2";

    #[test]
    fn mp4_seeks_exactly_where_asked() {
        let p = plan_lossless_trim(&media(MOV, 2), &KF, 2.0, 5.0).unwrap();
        assert_eq!((p.seek, p.keyframe, p.exact_start, p.visible_start), (2.0, Some(2.0), true, 2.0));
        assert!((p.duration - (3.0 - CUT_END_MARGIN)).abs() < 1e-9, "ends 1 ms before OUT");
        let p = plan_lossless_trim(&media(MOV, 2), &KF, 2.4, 5.4).unwrap();
        assert_eq!((p.seek, p.visible_start), (2.4, 2.4));
    }

    #[test]
    fn mkv_with_b_frames_pushes_seek_past_the_dts_margin() {
        let p = plan_lossless_trim(&media("matroska,webm", 2), &KF, 2.0, 5.0).unwrap();
        assert!((p.seek - (2.0 + DTS_SEEK_BACKOFF + 0.002)).abs() < 1e-9);
        assert!((p.seek + p.duration - (5.0 - CUT_END_MARGIN)).abs() < 1e-9, "still ends at the requested end");
        assert_eq!((p.keyframe, p.exact_start, p.visible_start), (Some(2.0), false, 2.0));
        // Far enough after the keyframe: left alone.
        let p = plan_lossless_trim(&media("matroska,webm", 2), &KF, 2.4, 5.4).unwrap();
        assert_eq!((p.seek, p.visible_start), (2.4, 2.0));
    }

    #[test]
    fn mkv_without_b_frames_is_left_alone() {
        let p = plan_lossless_trim(&media("matroska,webm", 0), &KF, 2.0, 5.0).unwrap();
        assert!(p.seek == 2.0 && (p.duration - 2.999).abs() < 1e-9);
    }

    #[test]
    fn lossless_trim_to_the_end_of_the_file_keeps_every_packet() {
        // OUT at the file's end (10 s): no frame to keep out, so no margin.
        let p = plan_lossless_trim(&media("matroska,webm", 0), &KF, 2.0, 10.0).unwrap();
        assert_eq!((p.seek, p.duration), (2.0, 8.0));
        let p = plan_lossless_trim(&media("matroska,webm", 0), &KF, 2.0, 9.9995).unwrap();
        assert!((p.duration - 7.9995).abs() < 1e-9, "within 1 ms of the end counts as the end");
    }

    #[test]
    fn short_range_never_seeks_past_its_end() {
        let p = plan_lossless_trim(&media("matroska,webm", 2), &KF, 2.0, 2.05).unwrap();
        assert!(p.seek < 2.05 && p.duration > 0.0 && p.seek + p.duration < 2.05);
    }

    #[test]
    fn mov_family_detection() {
        assert!(is_mov_family(MOV));
        assert!(!is_mov_family("matroska,webm"));
        assert!(!is_mov_family("mpegts"));
    }

    fn video(width: u32, height: u32, rotation: Option<f64>) -> VideoStream {
        let mut v = media(MOV, 0).video.unwrap();
        (v.width, v.height, v.rotation) = (width, height, rotation);
        v
    }

    #[test]
    fn proxy_snapshot() {
        let mut v = video(3840, 2160, None);
        v.index = 1;
        let a = proxy_args(
            Path::new("/v/OBS 녹화 01.mkv"),
            &v,
            Path::new("/cache/proxies/0123456789abcdef.part.mp4"),
        );
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-i", "/v/OBS 녹화 01.mkv",
                "-map", "0:1", "-map", "0:a:0?",
                "-vf", "scale=-2:540", "-pix_fmt", "yuv420p",
                "-c:v", "libx264", "-preset", "ultrafast", "-crf", "28", "-g", "15", "-fps_mode", "vfr",
                "-c:a", "aac", "-b:a", "96k", "-ac", "2", "-movflags", "+faststart",
                "/cache/proxies/0123456789abcdef.part.mp4",
            ]
        );
    }

    #[test]
    fn proxy_scale_keeps_the_short_side_at_540() {
        assert_eq!(proxy_scale_filter(&video(3840, 2160, None)), "scale=-2:540");
        assert_eq!(proxy_scale_filter(&video(1080, 1920, None)), "scale=540:-2", "portrait");
        // Stored landscape, shown portrait (iPhone held upright).
        assert_eq!(proxy_scale_filter(&video(3840, 2160, Some(-90.0))), "scale=540:-2");
        assert_eq!(proxy_scale_filter(&video(3840, 2160, Some(180.0))), "scale=-2:540");
        assert_eq!(proxy_scale_filter(&video(640, 360, None)), "scale=-2:360", "never upscales");
        assert_eq!(proxy_scale_filter(&video(721, 481, None)), "scale=-2:480", "even height");
        assert_eq!(proxy_scale_filter(&video(0, 0, None)), "scale=-2:540", "unknown size");
    }

    #[test]
    fn display_quotes_spaces() {
        let s = display_command(Path::new("/opt/homebrew/bin/ffmpeg"), &["-i".into(), "a b.mp4".into()]);
        assert_eq!(s, "/opt/homebrew/bin/ffmpeg -i \"a b.mp4\"");
    }

    // --- precise export ---

    fn audio(codec: &str) -> crate::probe::AudioStream {
        crate::probe::AudioStream {
            index: 1,
            codec: codec.into(),
            start_time: Some(0.0),
            channels: Some(2),
            sample_rate: Some(48000),
            language: None,
        }
    }

    /// 10 s file at `path` with `codec` video and the given audio tracks.
    fn clip(path: &str, codec: &str, audio_codecs: &[&str]) -> MediaInfo {
        let mut m = media(MOV, 2);
        m.path = path.into();
        m.video.as_mut().unwrap().codec = codec.into();
        m.audio = audio_codecs.iter().map(|c| audio(c)).collect();
        m
    }

    #[test]
    fn precise_trim_snapshot_x265_mp4() {
        let info = clip("/v/제주 여행.mov", "h264", &["aac"]);
        let plan = plan_precise_trim(&info, 7355.0 / 30.0 - 240.0, 7.5, OutputCodec::Hevc).unwrap();
        assert_eq!((plan.suffix.as_str(), plan.extension.as_str(), plan.audio), ("_trim_h265", "mp4", AudioPlan::Aac));
        let v = info.video.as_ref().unwrap();
        let a = precise_trim_args(
            Path::new("/v/제주 여행.mov"),
            v,
            &plan,
            "libx265",
            Quality::Balanced,
            Path::new("/v/제주 여행_trim_h265.part.mp4"),
        )
        .unwrap();
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-ss", "5.166666", "-i", "/v/제주 여행.mov", "-t", "2.332333",
                "-map", "0:0", "-map", "0:a?",
                "-c:v", "libx265", "-crf", "24", "-preset", "medium", "-x265-params", "log-level=error",
                "-pix_fmt", "yuv420p", "-tag:v", "hvc1",
                "-fps_mode", "passthrough", "-enc_time_base:v", "1/90000",
                "-c:a", "aac", "-b:a", "192k",
                "-map_metadata", "0", "-movflags", "+faststart",
                "/v/제주 여행_trim_h265.part.mp4",
            ]
        );
    }

    #[test]
    fn precise_snapshot_videotoolbox_hdr_whole_file() {
        let mut info = clip("/v/아이폰 가족.MOV", "hevc", &["aac"]);
        {
            let v = info.video.as_mut().unwrap();
            v.pix_fmt = Some("yuv420p10le".into());
            v.color_primaries = Some("bt2020".into());
            v.color_transfer = Some("arib-std-b67".into());
            v.color_space = Some("bt2020nc".into());
            v.hdr = true;
        }
        // 원본 유지 on an HEVC MOV: H.265 again, same container, whole file → no -ss/-t, AAC copied.
        let plan = plan_precise_trim(&info, 0.0, 10.0, OutputCodec::Source).unwrap();
        assert_eq!(
            (plan.codec, plan.ten_bit, plan.audio, plan.seek, plan.suffix.as_str(), plan.extension.as_str()),
            (VideoCodec::Hevc, true, AudioPlan::Copy, None, "_trim", "MOV")
        );
        let a = precise_trim_args(
            Path::new("/v/아이폰 가족.MOV"),
            info.video.as_ref().unwrap(),
            &plan,
            "hevc_videotoolbox",
            Quality::High,
            Path::new("/v/아이폰 가족_trim.part.MOV"),
        )
        .unwrap();
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-i", "/v/아이폰 가족.MOV",
                "-map", "0:0", "-map", "0:a?",
                "-c:v", "hevc_videotoolbox", "-q:v", "68", "-profile:v", "main10", "-pix_fmt", "p010le",
                "-tag:v", "hvc1",
                "-vf", "setparams=color_primaries=bt2020:color_trc=arib-std-b67:colorspace=bt2020nc",
                "-fps_mode", "passthrough", "-enc_time_base:v", "1/90000",
                "-c:a", "copy",
                "-map_metadata", "0", "-movflags", "+faststart",
                "/v/아이폰 가족_trim.part.MOV",
            ]
        );
    }

    #[test]
    fn precise_snapshot_svtav1_mkv_without_audio() {
        let info = clip("/v/OBS 녹화 01.mkv", "h264", &[]);
        let plan = plan_precise_trim(&info, 2.0, 10.0, OutputCodec::Av1).unwrap();
        assert_eq!((plan.suffix.as_str(), plan.extension.as_str(), plan.audio), ("_trim_av1", "mp4", AudioPlan::None));
        let a = precise_trim_args(
            Path::new("/v/OBS 녹화 01.mkv"),
            info.video.as_ref().unwrap(),
            &plan,
            "libsvtav1",
            Quality::Small,
            Path::new("/v/OBS 녹화 01_trim_av1.part.mp4"),
        )
        .unwrap();
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-ss", "2.000000", "-i", "/v/OBS 녹화 01.mkv", "-t", "7.999000",
                "-map", "0:0",
                "-c:v", "libsvtav1", "-crf", "48", "-preset", "8", "-pix_fmt", "yuv420p",
                "-fps_mode", "passthrough", "-enc_time_base:v", "1/90000",
                "-map_metadata", "0", "-movflags", "+faststart",
                "/v/OBS 녹화 01_trim_av1.part.mp4",
            ]
        );
    }

    fn fx(e: crate::effects::Effects, v: &VideoStream) -> Option<EffectsPlan> {
        crate::effects::plan(v, &e).unwrap()
    }

    /// 화면 효과 on a 4K HLG iPhone clip: a 9:16 box, 1080p → crop, scale to the ratio, square
    /// pixels, then the colors — one `-vf`, effects before `setparams`.
    #[test]
    fn precise_trim_snapshot_with_crop_and_size() {
        use crate::effects::{Crop, Effects, OutputSize};
        let mut info = sized(clip("/v/아이폰 세로 크롭.MOV", "hevc", &["aac"]), 3840, 2160, None);
        {
            let v = info.video.as_mut().unwrap();
            v.pix_fmt = Some("yuv420p10le".into());
            v.color_primaries = Some("bt2020".into());
            v.color_transfer = Some("arib-std-b67".into());
            v.color_space = Some("bt2020nc".into());
            v.hdr = true;
        }
        let v = info.video.clone().unwrap();
        let mut plan = plan_precise_trim(&info, 1.0, 4.0, OutputCodec::Hevc).unwrap();
        let crop = Crop { x: 1312, y: 0, width: 1216, height: 2160, ratio: Some([9, 16]) };
        plan.effects = fx(Effects { crop: Some(crop), size: OutputSize::Short { short: 1080 }, ..Effects::default() }, &v);
        assert_eq!(plan.effects.as_ref().unwrap().size, [1080, 1920]);
        let a = precise_trim_args(Path::new("/v/아이폰 세로 크롭.MOV"), &v, &plan, "hevc_videotoolbox", Quality::Balanced, Path::new("/v/아이폰 세로 크롭_trim_h265.part.mp4")).unwrap();
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-ss", "1.000000", "-i", "/v/아이폰 세로 크롭.MOV", "-t", "2.999000",
                "-map", "0:0", "-map", "0:a?",
                "-c:v", "hevc_videotoolbox", "-q:v", "60", "-profile:v", "main10", "-pix_fmt", "p010le",
                "-tag:v", "hvc1",
                "-vf", "crop=1216:2160:1312:0,scale=1080:1920,setsar=1,setparams=color_primaries=bt2020:color_trc=arib-std-b67:colorspace=bt2020nc",
                "-fps_mode", "passthrough", "-enc_time_base:v", "1/90000",
                "-c:a", "aac", "-b:a", "192k",
                "-map_metadata", "0", "-movflags", "+faststart",
                "/v/아이폰 세로 크롭_trim_h265.part.mp4",
            ]
        );
    }

    /// Turned right and mirrored, then 맞춤 into a square: the turned 1080 × 1920 picture is scaled to
    /// 608 × 1080 (607.5) and centered on black.
    #[test]
    fn precise_trim_snapshot_with_turn_flip_and_contain() {
        use crate::effects::{Effects, Fit, OutputSize};
        let info = sized(clip("/v/옆으로 찍은 영상.mp4", "h264", &[]), 1920, 1080, None);
        let v = info.video.clone().unwrap();
        let mut plan = plan_precise_trim(&info, 0.0, 10.0, OutputCodec::Source).unwrap();
        let e = Effects { turn: 1, flip_h: true, size: OutputSize::Exact { width: 1080, height: 1080, fit: Fit::Contain }, ..Effects::default() };
        plan.effects = fx(e, &v);
        let a = precise_trim_args(Path::new("/v/옆으로 찍은 영상.mp4"), &v, &plan, "libx264", Quality::High, Path::new("/v/옆으로 찍은 영상_trim.part.mp4")).unwrap();
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-i", "/v/옆으로 찍은 영상.mp4",
                "-map", "0:0",
                "-c:v", "libx264", "-crf", "20", "-preset", "medium", "-pix_fmt", "yuv420p",
                "-vf", "transpose=clock,hflip,scale=608:1080,setsar=1,pad=1080:1080:236:0",
                "-fps_mode", "passthrough", "-enc_time_base:v", "1/90000",
                "-map_metadata", "0", "-movflags", "+faststart",
                "/v/옆으로 찍은 영상_trim.part.mp4",
            ]
        );
    }

    /// 구간 제거 with 화면 효과: the effects run once on the joined picture, before the colors; each
    /// piece's sound is put at 0 and carries silence exactly its length (`[fz i]`, dropped after the
    /// join).
    #[test]
    fn precise_cut_snapshot_with_effects_after_the_join() {
        use crate::effects::{Crop, Effects, Fit, OutputSize};
        let mut info = sized(clip("/v/회의 녹화 화면.mkv", "h264", &["aac"]), 1920, 1080, None);
        info.video.as_mut().unwrap().color_primaries = Some("bt709".into());
        let v = info.video.clone().unwrap();
        let mut plan = plan_precise_cut(&info, &[rr(2.0, 3.0)], OutputCodec::Av1).unwrap();
        assert!(plan.as_trim().is_none());
        let crop = Crop { x: 200, y: 100, width: 1280, height: 720, ratio: Some([16, 9]) };
        let e = Effects { crop: Some(crop), turn: 3, flip_v: true, size: OutputSize::Exact { width: 480, height: 480, fit: Fit::Cover }, ..Effects::default() };
        plan.effects = fx(e, &v);
        let a = precise_cut_args(Path::new("/v/회의 녹화 화면.mkv"), &info, &plan, "libsvtav1", Quality::Balanced, Path::new("/v/회의 녹화 화면_cut_av1.part.mp4")).unwrap();
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-ss", "0.000000", "-t", "1.999000", "-i", "/v/회의 녹화 화면.mkv",
                "-ss", "3.000000", "-t", "6.999000", "-i", "/v/회의 녹화 화면.mkv",
                "-filter_complex",
                "[0:1]aresample=async=1:min_hard_comp=0.001:first_pts=0[fa0_0];anullsrc=r=48000:cl=mono,atrim=end=2.000000[fz0]\
                 ;[1:1]aresample=async=1:min_hard_comp=0.001:first_pts=0[fa1_0];anullsrc=r=48000:cl=mono,atrim=end=7.000000[fz1]\
                 ;[0:0][fa0_0][fz0][1:0][fa1_0][fz1]concat=n=2:v=1:a=2[vc][a0][fz];[fz]anullsink\
                 ;[vc]crop=720:720:480:100,transpose=cclock,vflip,scale=480:480,setsar=1,setparams=color_primaries=bt709[v]",
                "-map", "[v]", "-map", "[a0]",
                "-c:v", "libsvtav1", "-crf", "38", "-preset", "8", "-pix_fmt", "yuv420p",
                "-fps_mode", "passthrough", "-enc_time_base:v", "1/90000",
                "-c:a", "aac", "-b:a", "192k",
                "-map_metadata", "0", "-map_chapters", "-1", "-movflags", "+faststart",
                "/v/회의 녹화 화면_cut_av1.part.mp4",
            ]
        );
        // One piece left: the trim's arguments, effects included.
        let mut one = plan_precise_cut(&info, &[rr(0.0, 3.0)], OutputCodec::Av1).unwrap();
        one.effects = plan.effects;
        let trim = precise_cut_args(Path::new("/v/a.mkv"), &info, &one, "libsvtav1", Quality::Balanced, Path::new("/v/b.mp4")).unwrap();
        assert!(strs(&trim).contains(&"crop=720:720:480:100,transpose=cclock,vflip,scale=480:480,setsar=1,setparams=color_primaries=bt709".to_string()));
    }

    /// 모자이크 + 블러 on a 10-bit HDR clip with a crop after them: split, each box cropped,
    /// processed and laid back while its result times last, `format=yuv420p10` to keep 10 bits,
    /// then the crop and the colors — one `-vf`.
    #[test]
    fn precise_trim_snapshot_with_masks() {
        use crate::effects::{Crop, Effects, Mask, MaskKind};
        let mut info = sized(clip("/v/얼굴 가리기 HDR.mov", "hevc", &[]), 1920, 1080, None);
        {
            let v = info.video.as_mut().unwrap();
            v.pix_fmt = Some("yuv420p10le".into());
            v.color_primaries = Some("bt2020".into());
            v.color_transfer = Some("smpte2084".into());
            v.color_space = Some("bt2020nc".into());
            v.hdr = true;
        }
        let v = info.video.clone().unwrap();
        let mut plan = plan_precise_trim(&info, 2.0, 8.0, OutputCodec::Hevc).unwrap();
        let e = Effects {
            masks: vec![
                Mask { x: 100, y: 50, width: 350, height: 210, start: 3.0, end: 5.5, kind: MaskKind::Mosaic, strength: 50 },
                Mask { x: 1200, y: 600, width: 400, height: 300, start: 0.0, end: 10.0, kind: MaskKind::Blur, strength: 50 },
            ],
            crop: Some(Crop { x: 0, y: 0, width: 1600, height: 900, ratio: Some([16, 9]) }),
            ..Effects::default()
        };
        plan.effects = crate::effects::plan(&v, &e).unwrap().unwrap().in_result(&[plan::Range::new(2.0, 8.0)]);
        let a = precise_trim_args(Path::new("/v/얼굴 가리기 HDR.mov"), &v, &plan, "libx265", Quality::Balanced, Path::new("/v/b.part.mp4")).unwrap();
        let vf = strs(&a)[strs(&a).iter().position(|x| x == "-vf").unwrap() + 1].clone();
        assert_eq!(
            vf,
            "split=3[fx0][fxr0][fxr1]\
             ;[fxr0]crop=350:210:100:50,scale=10:6:flags=area,scale=350:210:flags=neighbor[fxp0]\
             ;[fx0][fxp0]overlay=100:50:format=yuv420p10:enable='gte(t,0.9995)*lt(t,3.4995)'[fx1]\
             ;[fxr1]crop=400:300:1200:600,gblur=sigma=17:steps=3[fxp1]\
             ;[fx1][fxp1]overlay=1200:600:format=yuv420p10:enable='gte(t,0.0000)*lt(t,5.9995)'[fx2]\
             ;[fx2]crop=1600:900:0:0,setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc"
        );
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-ss", "2.000000", "-i", "/v/얼굴 가리기 HDR.mov", "-t", "5.999000",
                "-map", "0:0",
                "-c:v", "libx265", "-crf", "24", "-preset", "medium", "-x265-params", "log-level=error",
                "-pix_fmt", "yuv420p10le", "-tag:v", "hvc1",
                "-vf", vf.as_str(),
                "-fps_mode", "passthrough", "-enc_time_base:v", "1/90000",
                "-map_metadata", "0", "-movflags", "+faststart",
                "/v/b.part.mp4",
            ]
        );
    }

    /// 구간 제거 with a mask that spans a cut: it runs on each piece it covers before the join, on
    /// the piece's own timeline (the joined range 1–5 is 1–2 of the first piece and 0–3 of the
    /// second; the third piece is left alone), so a join that `concat` places a hair off cannot
    /// move it a frame.
    #[test]
    fn precise_cut_snapshot_with_a_mask_across_the_join() {
        use crate::effects::{Effects, Mask, MaskKind};
        let info = sized(clip("/v/번호판 가리기.mp4", "h264", &[]), 1280, 720, None);
        let v = info.video.clone().unwrap();
        let mut plan = plan_precise_cut(&info, &[rr(2.0, 3.0), rr(6.0, 7.0)], OutputCodec::Source).unwrap();
        let e = Effects {
            masks: vec![Mask { x: 600, y: 400, width: 160, height: 64, start: 1.0, end: 6.5, kind: MaskKind::Mosaic, strength: 1 }],
            ..Effects::default()
        };
        plan.effects = crate::effects::plan(&v, &e).unwrap().unwrap().in_result(&plan.keep);
        let a = precise_cut_args(Path::new("/v/번호판 가리기.mp4"), &info, &plan, "libx264", Quality::High, Path::new("/v/c.part.mp4")).unwrap();
        let graph = strs(&a)[strs(&a).iter().position(|x| x == "-filter_complex").unwrap() + 1].clone();
        // 1–2 → 1–2, 3–6 → 2–5: one range 1–5. Cells of 4 px (720 × 0.5 %).
        assert_eq!(
            graph,
            "[0:0]split=2[fxs0_0][fxrs0_0];[fxrs0_0]crop=160:64:600:400,scale=40:16:flags=area,scale=160:64:flags=neighbor[fxps0_0]\
             ;[fxs0_0][fxps0_0]overlay=600:400:format=yuv420:enable='gte(t,0.9995)*lt(t,1.9995)'[fxs0_1];[fxs0_1]null[fv0]\
             ;anullsrc=r=48000:cl=mono,atrim=end=2.000000[fz0]\
             ;[1:0]split=2[fxs1_0][fxrs1_0];[fxrs1_0]crop=160:64:600:400,scale=40:16:flags=area,scale=160:64:flags=neighbor[fxps1_0]\
             ;[fxs1_0][fxps1_0]overlay=600:400:format=yuv420:enable='gte(t,0.0000)*lt(t,2.9995)'[fxs1_1];[fxs1_1]null[fv1]\
             ;anullsrc=r=48000:cl=mono,atrim=end=3.000000[fz1];anullsrc=r=48000:cl=mono,atrim=end=3.000000[fz2]\
             ;[fv0][fz0][fv1][fz1][2:0][fz2]concat=n=3:v=1:a=1[v][fz];[fz]anullsink"
        );
    }

    fn pip(path: &str, kind: crate::probe::OverlayKind, duration: Option<f64>, audio: bool) -> crate::probe::OverlaySource {
        crate::probe::OverlaySource { path: path.into(), file_name: path.into(), kind, width: 640, height: 360, duration, has_audio: audio }
    }

    /// A PiP video with sound and a logo on a 자르기: a -filter_complex — the source's video by
    /// number, the video moved to its result time and held to its window's end, the logo shown while
    /// its times last; the sound delayed, at its volume, mixed into the first track (the others kept).
    #[test]
    fn precise_trim_snapshot_with_pip_and_sound() {
        use crate::effects::{Effects, Overlay};
        use crate::probe::OverlayKind;
        let mut info = sized(clip("/v/강의 녹화.mp4", "h264", &["aac", "aac"]), 1920, 1080, None);
        info.audio[1].index = 2;
        info.video.as_mut().unwrap().color_space = Some("bt709".into());
        let v = info.video.clone().unwrap();
        let mut plan = plan_precise_trim(&info, 1.0, 9.0, OutputCodec::Source).unwrap();
        let e = Effects {
            overlays: vec![
                Overlay { path: "/v/카메라 1.mp4".into(), x: 1280, y: 720, width: 576, height: 324, start: 2.0, end: 8.0, offset: 5.0, opacity: 100, audio: true, volume: 80 },
                Overlay { path: "/로고/채널 로고.png".into(), x: 40, y: 40, width: 200, height: 60, start: 0.0, end: 10.0, offset: 0.0, opacity: 70, audio: false, volume: 100 },
            ],
            ..Effects::default()
        };
        let mut p = crate::effects::plan(&v, &e).unwrap().unwrap();
        p.attach_sources(vec![pip("/v/카메라 1.mp4", OverlayKind::Video, Some(60.0), true), pip("/로고/채널 로고.png", OverlayKind::Image, None, false)]);
        plan.effects = p.in_result(&[plan::Range::new(1.0, 9.0)]);
        assert!(mixes_overlay_audio(plan.effects.as_ref()));
        let a = precise_trim_args(Path::new("/v/강의 녹화.mp4"), &v, &plan, "libx264", Quality::Balanced, Path::new("/v/o.part.mp4")).unwrap();
        let graph = strs(&a)[strs(&a).iter().position(|x| x == "-filter_complex").unwrap() + 1].clone();
        assert_eq!(
            graph,
            "[1:v]setpts=PTS-STARTPTS+1.000000/TB,scale=576:324:out_color_matrix=bt709:out_range=tv,format=yuva420p,tpad=stop_mode=clone:stop=-1[fxo0_0]\
             ;[0:0][fxo0_0]overlay=1280:720:format=yuv420:shortest=1:enable='gte(t,0.9995)*lt(t,6.9995)'[fx1]\
             ;[2:v]scale=200:60:out_color_matrix=bt709:out_range=tv,format=yuva420p,lutyuv=a='val*0.70'[fxo1]\
             ;[fx1][fxo1]overlay=40:40:format=yuv420:enable='gte(t,0.0000)*lt(t,7.9995)'[fx2]\
             ;[fx2]setparams=colorspace=bt709[v]\
             ;[1:a]aresample=async=1:min_hard_comp=0.001:first_pts=0,adelay=delays=1000:all=1,volume=0.80[fxa0_0]\
             ;[0:1]aresample=async=1:min_hard_comp=0.001:first_pts=0[fxb]\
             ;[fxb][fxa0_0]amix=inputs=2:duration=first:dropout_transition=0:normalize=0[fxmix]"
        );
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-ss", "1.000000", "-i", "/v/강의 녹화.mp4",
                "-ss", "5.000000", "-t", "6.000000", "-i", "/v/카메라 1.mp4",
                "-i", "/로고/채널 로고.png",
                // The result's length after the last input (before it, the camera's -t replaced it).
                "-t", "7.999000",
                "-filter_complex", graph.as_str(), "-map", "[v]",
                "-map", "[fxmix]", "-map", "0:a?", "-map", "-0:a:0",
                "-c:v", "libx264", "-crf", "24", "-preset", "medium", "-pix_fmt", "yuv420p",
                "-fps_mode", "passthrough", "-enc_time_base:v", "1/90000",
                "-c:a", "aac", "-b:a", "192k",
                "-map_metadata", "0", "-movflags", "+faststart",
                "/v/o.part.mp4",
            ]
        );
    }

    /// 구간 제거 with a PiP video across the cut on a 10-bit source: one input per kept stretch of
    /// the video (it jumps with the original), each laid on its piece before the join at the piece's
    /// own time, 10-bit alpha, its sound mixed into that piece's first track.
    #[test]
    fn precise_cut_snapshot_with_pip_across_the_join() {
        use crate::effects::{Effects, Overlay};
        use crate::probe::OverlayKind;
        let mut info = sized(clip("/v/여행 HDR.mov", "hevc", &["aac"]), 1920, 1080, None);
        info.video.as_mut().unwrap().pix_fmt = Some("yuv420p10le".into());
        let v = info.video.clone().unwrap();
        let mut plan = plan_precise_cut(&info, &[rr(3.0, 4.0)], OutputCodec::Source).unwrap();
        let e = Effects {
            overlays: vec![Overlay { path: "/v/카메라 1.mp4".into(), x: 0, y: 0, width: 640, height: 360, start: 2.0, end: 6.0, offset: 0.0, opacity: 100, audio: true, volume: 100 }],
            ..Effects::default()
        };
        let mut p = crate::effects::plan(&v, &e).unwrap().unwrap();
        p.attach_sources(vec![pip("/v/카메라 1.mp4", OverlayKind::Video, Some(30.0), true)]);
        plan.effects = p.in_result(&plan.keep);
        let a = precise_cut_args(Path::new("/v/여행 HDR.mov"), &info, &plan, "libx265", Quality::Balanced, Path::new("/v/o.part.mov")).unwrap();
        let s = strs(&a);
        let inputs: Vec<&str> = s.iter().zip(s.iter().skip(1)).filter(|(k, _)| k.as_str() == "-i").map(|(_, v)| v.as_str()).collect();
        assert_eq!(inputs, ["/v/여행 HDR.mov", "/v/여행 HDR.mov", "/v/카메라 1.mp4", "/v/카메라 1.mp4"]);
        // Original 2–3 → result 2–3 from 0 s; 4–6 → result 3–5 from 2 s (the cut second is skipped).
        assert!(s.windows(4).any(|w| w == ["-ss", "0.000000", "-t", "1.000000"]));
        assert!(s.windows(4).any(|w| w == ["-ss", "2.000000", "-t", "2.000000"]));
        let graph = s[s.iter().position(|x| x == "-filter_complex").unwrap() + 1].clone();
        assert_eq!(
            graph,
            "[2:v]setpts=PTS-STARTPTS+2.000000/TB,scale=640:360,format=yuva420p10le,tpad=stop_mode=clone:stop=-1[fxos0_0_0]\
             ;[0:0][fxos0_0_0]overlay=0:0:format=yuv420p10:shortest=1:enable='gte(t,1.9995)*lt(t,2.9995)'[fxs0_1];[fxs0_1]null[fv0]\
             ;[2:a]aresample=async=1:min_hard_comp=0.001:first_pts=0,adelay=delays=2000:all=1,volume=1.00[fxas0_0_0]\
             ;[0:1]aresample=async=1:min_hard_comp=0.001:first_pts=0[fxbs0_]\
             ;[fxbs0_][fxas0_0_0]amix=inputs=2:duration=first:dropout_transition=0:normalize=0[fm0]\
             ;anullsrc=r=48000:cl=mono,atrim=end=3.000000[fz0]\
             ;[3:v]setpts=PTS-STARTPTS+0.000000/TB,scale=640:360,format=yuva420p10le,tpad=stop_mode=clone:stop=-1[fxos1_0_0]\
             ;[1:0][fxos1_0_0]overlay=0:0:format=yuv420p10:shortest=1:enable='gte(t,0.0000)*lt(t,1.9995)'[fxs1_1];[fxs1_1]null[fv1]\
             ;[3:a]aresample=async=1:min_hard_comp=0.001:first_pts=0,adelay=delays=0:all=1,volume=1.00[fxas1_0_0]\
             ;[1:1]aresample=async=1:min_hard_comp=0.001:first_pts=0[fxbs1_]\
             ;[fxbs1_][fxas1_0_0]amix=inputs=2:duration=first:dropout_transition=0:normalize=0[fm1]\
             ;anullsrc=r=48000:cl=mono,atrim=end=6.000000[fz1]\
             ;[fv0][fm0][fz0][fv1][fm1][fz1]concat=n=2:v=1:a=2[v][a0][fz];[fz]anullsink"
        );
    }

    /// A PiP stays upright when the picture turns and mirrors: the file is mirrored the same way and
    /// turned back before it goes on, scaled to the box as seen (a quarter turn swaps its sides).
    #[test]
    fn pip_is_turned_back_against_the_pictures_turn_and_mirror() {
        use crate::effects::{Effects, Overlay};
        use crate::probe::OverlayKind;
        let info = sized(clip("/v/폰 영상.mp4", "h264", &["aac"]), 1920, 1080, None);
        let v = info.video.clone().unwrap();
        let graph = |turn: u8, flip_h: bool, flip_v: bool| {
            let mut plan = plan_precise_trim(&info, 0.0, 4.0, OutputCodec::Source).unwrap();
            let e = Effects {
                turn,
                flip_h,
                flip_v,
                overlays: vec![Overlay { path: "/로고/채널 로고.png".into(), x: 100, y: 200, width: 60, height: 140, start: 0.0, end: 4.0, offset: 0.0, opacity: 100, audio: false, volume: 100 }],
                ..Effects::default()
            };
            let mut p = crate::effects::plan(&v, &e).unwrap().unwrap();
            p.attach_sources(vec![pip("/로고/채널 로고.png", OverlayKind::Image, None, false)]);
            plan.effects = p.in_result(&[plan::Range::new(0.0, 4.0)]);
            let a = precise_trim_args(Path::new("/v/폰 영상.mp4"), &v, &plan, "libx264", Quality::Balanced, Path::new("/v/o.part.mp4")).unwrap();
            let s = strs(&a);
            let g = s[s.iter().position(|x| x == "-filter_complex").unwrap() + 1].clone();
            g.split(';').find(|x| x.starts_with("[1:v]")).unwrap().to_string()
        };
        // 오른쪽 90° + 좌우: the box (60 × 140 in the original) is 140 × 60 as seen.
        assert_eq!(graph(1, true, false), "[1:v]scale=140:60,format=yuva420p,hflip,transpose=cclock[fxo0]");
        assert_eq!(graph(3, false, true), "[1:v]scale=140:60,format=yuva420p,vflip,transpose=clock[fxo0]");
        // 180° is the two mirrors.
        assert_eq!(graph(2, false, false), "[1:v]scale=60:140,format=yuva420p,hflip,vflip[fxo0]");
        assert_eq!(graph(0, false, false), "[1:v]scale=60:140,format=yuva420p[fxo0]");
    }

    #[test]
    fn mask_enable_covers_every_range() {
        assert_eq!(mask_enable(&[rr(0.0, 1.0), rr(2.5, 3.0)]), "'gte(t,0.0000)*lt(t,0.9995)+gte(t,2.4995)*lt(t,2.9995)'");
    }

    #[test]
    fn effect_filters_in_order() {
        use crate::effects::{Pad, Rect};
        let p = EffectsPlan {
            masks: vec![],
            overlays: vec![],
            crop: Some(Rect { x: 2, y: 4, width: 640, height: 360 }),
            turn: 3,
            flip_h: true,
            flip_v: true,
            scale: Some([320, 180]),
            pad: Some(Pad { width: 320, height: 320, x: 0, y: 70 }),
            size: [320, 320],
        };
        assert_eq!(effect_filters(&p), ["crop=640:360:2:4", "transpose=cclock", "hflip", "vflip", "scale=320:180", "setsar=1", "pad=320:320:0:70"]);
        let mirror = EffectsPlan { masks: vec![], overlays: vec![], crop: None, turn: 0, flip_h: true, flip_v: false, scale: None, pad: None, size: [320, 240] };
        assert_eq!(effect_filters(&mirror), ["hflip"]);
    }

    #[test]
    fn precise_snapshot_x264_keeps_mkv() {
        // 원본 유지 on an H.264 MKV with Opus: H.264 again, still MKV, Opus → AAC, no hvc1 / movflags.
        let info = clip("/v/회의 녹화.mkv", "h264", &["opus"]);
        let plan = plan_precise_trim(&info, 1.0, 4.0, OutputCodec::Source).unwrap();
        assert_eq!((plan.codec, plan.suffix.as_str(), plan.extension.as_str()), (VideoCodec::H264, "_trim", "mkv"));
        let a = precise_trim_args(
            Path::new("/v/회의 녹화.mkv"),
            info.video.as_ref().unwrap(),
            &plan,
            "libx264",
            Quality::Balanced,
            Path::new("/v/회의 녹화_trim.part.mkv"),
        )
        .unwrap();
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-ss", "1.000000", "-i", "/v/회의 녹화.mkv", "-t", "2.999000",
                "-map", "0:0", "-map", "0:a?",
                "-c:v", "libx264", "-crf", "24", "-preset", "medium", "-pix_fmt", "yuv420p",
                "-fps_mode", "passthrough", "-enc_time_base:v", "1/90000",
                "-c:a", "aac", "-b:a", "192k",
                "-map_metadata", "0",
                "/v/회의 녹화_trim.part.mkv",
            ]
        );
    }

    #[test]
    fn hardware_encoder_settings_follow_the_canvas_table() {
        let s = |enc: &str, q: Quality, ten: bool| strs(&video_encoder_args(enc, q, ten).unwrap()).join(" ");
        assert_eq!(
            s("hevc_nvenc", Quality::Balanced, false),
            "-c:v hevc_nvenc -preset p5 -rc vbr -cq 26 -b:v 0 -pix_fmt yuv420p"
        );
        assert_eq!(
            s("hevc_nvenc", Quality::High, true),
            "-c:v hevc_nvenc -preset p5 -rc vbr -cq 22 -b:v 0 -profile:v main10 -pix_fmt p010le"
        );
        assert_eq!(s("hevc_qsv", Quality::Small, false), "-c:v hevc_qsv -global_quality 30 -pix_fmt nv12");
        assert_eq!(s("hevc_amf", Quality::Balanced, false), "-c:v hevc_amf -rc cqp -qp_i 24 -qp_p 26 -pix_fmt nv12");
        assert_eq!(s("hevc_videotoolbox", Quality::Balanced, false), "-c:v hevc_videotoolbox -q:v 60 -pix_fmt yuv420p");
        assert_eq!(s("libsvtav1", Quality::High, true), "-c:v libsvtav1 -crf 28 -preset 8 -pix_fmt yuv420p10le");
        assert_eq!(
            s("libx265", Quality::Small, true),
            "-c:v libx265 -crf 28 -preset medium -x265-params log-level=error -pix_fmt yuv420p10le"
        );
        assert_eq!(s("libx264", Quality::High, true), "-c:v libx264 -crf 20 -preset medium -pix_fmt yuv420p", "8-bit H.264");
        for enc in ["libx264", "libx265", "libsvtav1", "hevc_videotoolbox", "hevc_nvenc", "hevc_qsv", "hevc_amf"] {
            assert!(exports_with(enc) && video_encoder_args(enc, Quality::Balanced, false).is_ok(), "{enc}");
        }
        for enc in ["av1_nvenc", "av1_qsv", "av1_amf", "hevc_vaapi", "hevc_mf"] {
            assert!(!exports_with(enc) && video_encoder_args(enc, Quality::Balanced, false).is_err(), "{enc}");
        }
    }

    /// The presets are the table values fed through `video_encoder_args_at` (what the VMAF
    /// calibration sweeps), so a calibrated value means the same arguments in the app.
    #[test]
    fn presets_are_values_of_the_swept_encoder_args() {
        for enc in ["libx264", "libx265", "libsvtav1", "hevc_videotoolbox", "hevc_nvenc", "hevc_qsv", "hevc_amf"] {
            let values = quality_values(enc).unwrap();
            for (q, v) in [Quality::High, Quality::Balanced, Quality::Small].into_iter().zip(values) {
                for ten in [false, true] {
                    assert_eq!(video_encoder_args(enc, q, ten), video_encoder_args_at(enc, v, ten), "{enc} {q:?}");
                }
            }
        }
        assert_eq!(
            strs(&video_encoder_args_at("hevc_amf", 31, false).unwrap()).join(" "),
            "-c:v hevc_amf -rc cqp -qp_i 29 -qp_p 31 -pix_fmt nv12"
        );
        assert!(quality_values("hevc_vaapi").is_none() && video_encoder_args_at("hevc_vaapi", 24, false).is_err());
    }

    #[test]
    fn precise_plan_names_and_containers() {
        let name = |path: &str, codec: &str, start: f64, end: f64, out: OutputCodec| {
            let p = plan_precise_trim(&clip(path, codec, &["aac"]), start, end, out).unwrap();
            format!("{}.{}", p.suffix, p.extension)
        };
        assert_eq!(name("a.mov", "h264", 0.0, 10.0, OutputCodec::Hevc), "_h265.mp4", "whole file: a conversion");
        assert_eq!(name("a.mov", "h264", 0.0, 10.0, OutputCodec::Av1), "_av1.mp4");
        assert_eq!(name("a.mov", "h264", 1.0, 10.0, OutputCodec::Av1), "_trim_av1.mp4");
        assert_eq!(name("a.mov", "h264", 1.0, 10.0, OutputCodec::Source), "_trim.mov");
        assert_eq!(name("a.webm", "vp9", 1.0, 10.0, OutputCodec::Source), "_trim.mp4", "VP9 → H.264 needs MP4");
        assert_eq!(name("a.avi", "mpeg4", 1.0, 10.0, OutputCodec::Source), "_trim.mp4");
        assert_eq!(name("a.mov", "av1", 1.0, 10.0, OutputCodec::Source), "_trim.mp4", "no AV1 in MOV");
        assert_eq!(name("a.mkv", "av1", 1.0, 10.0, OutputCodec::Source), "_trim.mkv");

        let p = |codec: &str, out: OutputCodec| plan_precise_trim(&clip("a.mp4", codec, &["aac"]), 1.0, 2.0, out).unwrap().codec;
        assert_eq!(p("h264", OutputCodec::Source), VideoCodec::H264);
        assert_eq!(p("hevc", OutputCodec::Source), VideoCodec::Hevc);
        assert_eq!(p("av1", OutputCodec::Source), VideoCodec::Av1);
        assert_eq!(p("prores", OutputCodec::Source), VideoCodec::H264, "other codecs → H.264");
        assert_eq!(p("hevc", OutputCodec::Av1), VideoCodec::Av1);
    }

    #[test]
    fn precise_plan_audio_and_bit_depth() {
        let plan = |audio: &[&str], start: f64, end: f64| plan_precise_trim(&clip("a.mp4", "h264", audio), start, end, OutputCodec::Hevc).unwrap();
        assert_eq!(plan(&["aac", "aac"], 0.0, 10.0).audio, AudioPlan::Copy);
        assert_eq!(plan(&["aac", "ac3"], 0.0, 10.0).audio, AudioPlan::Aac, "every track must be AAC to copy");
        assert_eq!(plan(&["aac"], 0.0005, 9.9995).audio, AudioPlan::Copy, "within 1 ms of the ends is the whole file");
        assert_eq!(plan(&["aac"], 0.5, 10.0).audio, AudioPlan::Aac, "a cut re-encodes the audio");
        let whole = plan(&["aac"], 0.0, 10.0);
        assert_eq!((whole.seek, whole.duration), (None, 10.0));
        let cut = plan(&["aac"], 2.5, 7.0);
        assert_eq!((cut.seek, cut.duration), (Some(2.5), 4.5));

        let mut ten = clip("a.mp4", "hevc", &[]);
        ten.video.as_mut().unwrap().pix_fmt = Some("yuv420p10le".into());
        assert!(plan_precise_trim(&ten, 1.0, 2.0, OutputCodec::Av1).unwrap().ten_bit);
        assert!(!plan_precise_trim(&ten, 1.0, 2.0, OutputCodec::Source).map(|p| p.codec == VideoCodec::H264).unwrap());
        ten.video.as_mut().unwrap().codec = "prores".into();
        assert!(!plan_precise_trim(&ten, 1.0, 2.0, OutputCodec::Source).unwrap().ten_bit, "H.264 stays 8-bit");

        let mut no_video = clip("a.m4a", "h264", &["aac"]);
        no_video.video = None;
        assert!(plan_precise_trim(&no_video, 0.0, 1.0, OutputCodec::Hevc).is_err());
        assert!(plan_precise_trim(&clip("a.mp4", "h264", &[]), 5.0, 5.0, OutputCodec::Hevc).is_err());
    }

    #[test]
    fn color_tags_skip_unknown_and_rgb_matrix() {
        let mut v = video(1920, 1080, None);
        (v.color_primaries, v.color_transfer, v.color_space) =
            (Some("bt709".into()), Some("unknown".into()), Some("gbr".into()));
        assert_eq!(strs(&color_args(&v)), vec!["-vf", "setparams=color_primaries=bt709"]);
        (v.color_primaries, v.color_transfer, v.color_space) =
            (Some("bt2020".into()), Some("smpte2084".into()), Some("bt2020nc".into()));
        assert_eq!(
            strs(&color_args(&v)),
            vec!["-vf", "setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc"]
        );
        (v.color_primaries, v.color_transfer, v.color_space) = (Some("jedec-p22".into()), None, Some("reserved".into()));
        assert!(color_args(&v).is_empty(), "names setparams may not know are left to the decoder");
    }

    // --- precise cut-out (구간 제거) ---

    fn rr(start: f64, end: f64) -> plan::Range {
        plan::Range::new(start, end)
    }

    #[test]
    fn precise_cut_snapshot_x265_two_audio_tracks() {
        // 10 s, 25 fps: remove 2.0–3.5 and 7.0–8.0 → keep 0–2, 3.5–7, 8–10 (7.5 s).
        let mut info = clip("/v/제주 여행 0412.mov", "h264", &["aac", "ac3"]);
        info.audio[0].index = 1;
        info.audio[0].language = Some("kor".into());
        info.audio[1].index = 2;
        info.audio[1].language = Some("und".into());
        let plan = plan_precise_cut(&info, &[rr(7.0, 8.0), rr(2.0, 3.5)], OutputCodec::Hevc).unwrap();
        assert_eq!(plan.keep, vec![rr(0.0, 2.0), rr(3.5, 7.0), rr(8.0, 10.0)]);
        assert_eq!((plan.duration, plan.audio), (7.5, AudioPlan::Aac));
        assert_eq!((plan.suffix.as_str(), plan.extension.as_str()), ("_cut_h265", "mp4"));
        let a = precise_cut_args(
            Path::new("/v/제주 여행 0412.mov"),
            &info,
            &plan,
            "libx265",
            Quality::Balanced,
            Path::new("/v/제주 여행 0412_cut_h265.part.mp4"),
        )
        .unwrap();
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-ss", "0.000000", "-t", "1.999000", "-i", "/v/제주 여행 0412.mov",
                "-ss", "3.500000", "-t", "3.499000", "-i", "/v/제주 여행 0412.mov",
                "-ss", "8.000000", "-t", "1.999000", "-i", "/v/제주 여행 0412.mov",
                "-filter_complex",
                "[0:1]aresample=async=1:min_hard_comp=0.001:first_pts=0[fa0_0];[0:2]aresample=async=1:min_hard_comp=0.001:first_pts=0[fa0_1]\
                 ;anullsrc=r=48000:cl=mono,atrim=end=2.000000[fz0]\
                 ;[1:1]aresample=async=1:min_hard_comp=0.001:first_pts=0[fa1_0];[1:2]aresample=async=1:min_hard_comp=0.001:first_pts=0[fa1_1]\
                 ;anullsrc=r=48000:cl=mono,atrim=end=3.500000[fz1]\
                 ;[2:1]aresample=async=1:min_hard_comp=0.001:first_pts=0[fa2_0];[2:2]aresample=async=1:min_hard_comp=0.001:first_pts=0[fa2_1]\
                 ;anullsrc=r=48000:cl=mono,atrim=end=2.000000[fz2]\
                 ;[0:0][fa0_0][fa0_1][fz0][1:0][fa1_0][fa1_1][fz1][2:0][fa2_0][fa2_1][fz2]concat=n=3:v=1:a=3[v][a0][a1][fz];[fz]anullsink",
                "-map", "[v]", "-map", "[a0]", "-map", "[a1]",
                "-c:v", "libx265", "-crf", "24", "-preset", "medium", "-x265-params", "log-level=error",
                "-pix_fmt", "yuv420p", "-tag:v", "hvc1",
                "-fps_mode", "passthrough", "-enc_time_base:v", "1/90000",
                "-c:a", "aac", "-b:a", "192k",
                "-metadata:s:a:0", "language=kor",
                "-map_metadata", "0", "-map_chapters", "-1", "-movflags", "+faststart",
                "/v/제주 여행 0412_cut_h265.part.mp4",
            ]
        );
    }

    #[test]
    fn precise_cut_snapshot_videotoolbox_hdr_frame_exact_mkv() {
        // 원본 유지 on a 10-bit HDR HEVC MKV without audio: H.265 again, still MKV, colors through
        // setparams after the concat. 30 fps cuts at frames #62 – #155 and #240 – #250.
        let mut info = clip("/v/OBS 녹화 HDR.mkv", "hevc", &[]);
        {
            let v = info.video.as_mut().unwrap();
            (v.fps, v.pix_fmt) = (Some(30.0), Some("yuv420p10le".into()));
            v.color_primaries = Some("bt2020".into());
            v.color_transfer = Some("smpte2084".into());
            v.color_space = Some("bt2020nc".into());
            v.hdr = true;
        }
        let removed = [rr(62.0 / 30.0, 155.0 / 30.0), rr(240.0 / 30.0, 250.0 / 30.0)];
        let plan = plan_precise_cut(&info, &removed, OutputCodec::Source).unwrap();
        assert_eq!(
            (plan.codec, plan.ten_bit, plan.audio, plan.suffix.as_str(), plan.extension.as_str()),
            (VideoCodec::Hevc, true, AudioPlan::None, "_cut", "mkv")
        );
        let a = precise_cut_args(
            Path::new("/v/OBS 녹화 HDR.mkv"),
            &info,
            &plan,
            "hevc_videotoolbox",
            Quality::High,
            Path::new("/v/OBS 녹화 HDR_cut.part.mkv"),
        )
        .unwrap();
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-ss", "0.000000", "-t", "2.065666", "-i", "/v/OBS 녹화 HDR.mkv",
                "-ss", "5.166666", "-t", "2.832333", "-i", "/v/OBS 녹화 HDR.mkv",
                "-ss", "8.333333", "-t", "1.665666", "-i", "/v/OBS 녹화 HDR.mkv",
                "-filter_complex",
                "anullsrc=r=48000:cl=mono,atrim=end=2.066666[fz0];anullsrc=r=48000:cl=mono,atrim=end=2.833333[fz1]\
                 ;anullsrc=r=48000:cl=mono,atrim=end=1.666666[fz2]\
                 ;[0:0][fz0][1:0][fz1][2:0][fz2]concat=n=3:v=1:a=1[vc][fz];[fz]anullsink\
                 ;[vc]setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc[v]",
                "-map", "[v]",
                "-c:v", "hevc_videotoolbox", "-q:v", "68", "-profile:v", "main10", "-pix_fmt", "p010le",
                "-fps_mode", "passthrough", "-enc_time_base:v", "1/90000",
                "-map_metadata", "0", "-map_chapters", "-1",
                "/v/OBS 녹화 HDR_cut.part.mkv",
            ]
        );
    }

    #[test]
    fn precise_pieces_end_a_millisecond_before_out() {
        // MKV stores 30 fps frame #226 (7.5333… s) as 7.533: a piece #200 – #226 must stop before it.
        assert_eq!(precise_len(226.0 / 30.0 - 200.0 / 30.0), "0.865666");
        assert_eq!(precise_len(1.0 / 30.0), "0.032333", "a one-frame piece keeps its frame");
        assert_eq!(precise_len(0.0015), "0.000750", "never below half the piece");
    }

    #[test]
    fn precise_cut_with_one_piece_left_is_a_trim() {
        // Removing the first 2 s leaves 2–10: the trim's arguments, under the cut's name.
        let info = clip("/v/회의 녹화.mp4", "h264", &["aac"]);
        let plan = plan_precise_cut(&info, &[rr(-1.0, 2.0)], OutputCodec::Av1).unwrap();
        assert_eq!((plan.keep.clone(), plan.suffix.as_str(), plan.audio), (vec![rr(2.0, 10.0)], "_cut_av1", AudioPlan::Aac));
        let trim = plan.as_trim().unwrap();
        assert_eq!((trim.seek, trim.duration), (Some(2.0), 8.0));
        let out = Path::new("/v/회의 녹화_cut_av1.part.mp4");
        let video = info.video.as_ref().unwrap();
        assert_eq!(
            precise_cut_args(Path::new("/v/회의 녹화.mp4"), &info, &plan, "libsvtav1", Quality::Small, out).unwrap(),
            precise_trim_args(Path::new("/v/회의 녹화.mp4"), video, &trim, "libsvtav1", Quality::Small, out).unwrap()
        );
    }

    #[test]
    fn precise_cut_plan_rules() {
        let cut = |removed: &[plan::Range]| plan_precise_cut(&clip("a.mp4", "h264", &["aac"]), removed, OutputCodec::Source);
        assert_eq!(cut(&[]).unwrap_err(), Msg::NoRangesToRemove);
        assert_eq!(cut(&[rr(4.0, 4.02)]).unwrap_err(), Msg::NoRangesToRemove, "shorter than a 25 fps frame");
        assert_eq!(cut(&[rr(0.0, 6.0), rr(5.0, 10.0)]).unwrap_err(), Msg::RemovesEverything);
        assert!(cut(&[rr(0.0, 4.0), rr(4.03, 10.0)]).is_err(), "the 0.03 s left is under one frame");
        // A piece of exactly one frame stays.
        assert_eq!(cut(&[rr(0.0, 4.0), rr(4.04, 10.0)]).unwrap().keep, vec![rr(4.0, 4.04)]);
        // Overlapping and unsorted ranges merge; AAC is re-encoded even though it could be copied.
        let p = cut(&[rr(6.0, 8.0), rr(1.0, 3.0), rr(2.0, 4.0)]).unwrap();
        assert_eq!((p.keep, p.duration, p.audio), (vec![rr(0.0, 1.0), rr(4.0, 6.0), rr(8.0, 10.0)], 5.0, AudioPlan::Aac));
        assert_eq!((p.suffix.as_str(), p.extension.as_str()), ("_cut", "mp4"));

        let name = |path: &str, codec: &str, out: OutputCodec| {
            let p = plan_precise_cut(&clip(path, codec, &[]), &[rr(1.0, 2.0)], out).unwrap();
            format!("{}.{}", p.suffix, p.extension)
        };
        assert_eq!(name("a.mov", "h264", OutputCodec::Source), "_cut.mov");
        assert_eq!(name("a.mov", "h264", OutputCodec::Hevc), "_cut_h265.mp4");
        assert_eq!(name("a.mkv", "h264", OutputCodec::Av1), "_cut_av1.mp4");
        assert_eq!(name("a.webm", "vp9", OutputCodec::Source), "_cut.mp4", "VP9 → H.264 needs MP4");

        let mut no_video = clip("a.m4a", "h264", &["aac"]);
        no_video.video = None;
        assert!(plan_precise_cut(&no_video, &[rr(1.0, 2.0)], OutputCodec::Hevc).is_err());
        let mut unknown_rate = clip("a.mp4", "h264", &[]);
        unknown_rate.video.as_mut().unwrap().fps = None;
        assert!((min_keep_len(unknown_rate.video.as_ref().unwrap()) - 0.999 / 30.0).abs() < 1e-12);
    }

    // --- lossless cut-out (구간 제거 · 무손실) ---

    const GOP2: [f64; 5] = [0.0, 2.0, 4.0, 6.0, 8.0];

    #[test]
    fn lossless_cut_copies_every_piece_from_its_keyframe() {
        // 10 s MP4, keyframes every 2 s. Remove 1.0–2.4 (keyframe 2.0 is inside: the piece after it
        // starts at 2.0), 5.0–5.5 (no keyframe after 5.0 before 5.5: stays) and 8.5–10.
        let removed = [rr(8.5, 10.0), rr(5.0, 5.5), rr(1.0, 2.4)];
        let p = plan_lossless_cut(&media(MOV, 2), &GOP2, &removed).unwrap();
        assert_eq!(p.kept_removals, vec![rr(5.0, 5.5)]);
        assert!(!p.exact_start);
        let got: Vec<_> = p.pieces.iter().map(|x| (x.keep, x.seek, x.visible_start)).collect();
        assert_eq!(got, vec![(rr(0.0, 1.0), 0.0, 0.0), (rr(2.4, 8.5), 2.0, 2.0)], "MP4: -ss at the keyframe");
        assert!((p.pieces[0].duration - 0.999).abs() < 1e-9 && (p.pieces[1].duration - 6.499).abs() < 1e-9, "1 ms before OUT");
        assert!((p.length - 7.498).abs() < 1e-9);

        // MKV with B-frames: the trim's seek (past the DTS margin), still copied from the keyframe.
        let p = plan_lossless_cut(&media("matroska,webm", 2), &GOP2, &removed).unwrap();
        let second = p.pieces[1];
        assert_eq!((second.seek, second.visible_start), (2.4, 2.0));
        assert!((second.seek + second.duration - 8.499).abs() < 1e-9);
        assert!((p.length - 7.498).abs() < 1e-9, "the same frames as in MP4");
        let p = plan_lossless_cut(&media("matroska,webm", 2), &GOP2, &[rr(1.0, 2.05), rr(8.5, 10.0)]).unwrap();
        assert!((p.pieces[1].seek - (2.0 + DTS_SEEK_BACKOFF + 0.002)).abs() < 1e-9);
    }

    #[test]
    fn lossless_cut_with_one_piece_left_is_a_trim() {
        // Removing 0–3.3 of an MP4 leaves 3.3–10: the edit list starts it exactly there.
        let p = plan_lossless_cut(&media(MOV, 2), &GOP2, &[rr(0.0, 3.3)]).unwrap();
        let only = p.pieces[0];
        assert_eq!((only.seek, only.duration, only.visible_start, p.exact_start), (3.3, 6.7, 3.3, true));
        assert_eq!(p.pieces.len(), 1);
        // In MKV it plays from the keyframe 2.0.
        let p = plan_lossless_cut(&media("matroska,webm", 0), &GOP2, &[rr(0.0, 3.3)]).unwrap();
        assert_eq!((p.pieces[0].visible_start, p.exact_start), (2.0, false));
        assert!((p.length - 8.0).abs() < 1e-9);
        // Two kept ranges joined into one by a removal stream copy cannot do.
        let p = plan_lossless_cut(&media(MOV, 0), &GOP2, &[rr(5.0, 5.5), rr(9.0, 10.0)]).unwrap();
        assert_eq!((p.pieces.len(), p.pieces[0].keep, p.kept_removals.clone()), (1, rr(0.0, 9.0), vec![rr(5.0, 5.5)]));
    }

    #[test]
    fn lossless_cut_refuses_what_it_cannot_do() {
        let cut = |info: &MediaInfo, kf: &[f64], removed: &[plan::Range]| plan_lossless_cut(info, kf, removed).unwrap_err();
        assert_eq!(cut(&media(MOV, 0), &GOP2, &[rr(5.0, 5.5)]), Msg::NothingRemovableLossless);
        assert_eq!(cut(&media("matroska,webm", 0), &GOP2, &[rr(0.0, 1.5)]), Msg::NothingRemovableLossless, "pulled back to 0");
        assert_eq!(cut(&media(MOV, 0), &[], &[rr(1.0, 2.0)]), Msg::CutNeedsKeyframes);
        assert_eq!(cut(&media(MOV, 0), &GOP2, &[]), Msg::NoRangesToRemove);
        assert_eq!(cut(&media(MOV, 0), &GOP2, &[rr(0.0, 10.0)]), Msg::RemovesEverything);
        let mut no_video = media(MOV, 0);
        no_video.video = None;
        assert_eq!(cut(&no_video, &GOP2, &[rr(1.0, 2.0)]), Msg::CutNeedsVideo);
    }

    #[test]
    fn lossless_piece_and_concat_snapshot() {
        let p = plan_lossless_cut(&media(MOV, 2), &GOP2, &[rr(1.0, 2.4)]).unwrap();
        let scratch = cut_scratch_dir(Path::new("/v/제주 여행 0412_cut.mp4"), "77-1");
        assert_eq!(scratch, Path::new("/v/.godseon-cut-77-1"));
        let piece = scratch.join(piece_name(1, "MP4").unwrap());
        // Joined paths take the platform's separator (`/v\.godseon-cut-77-1\piece-001.mp4` on Windows).
        let list = scratch.join("list.txt");
        assert_eq!(
            strs(&lossless_piece_args(Path::new("/v/제주 여행 0412.mp4"), Some(0), &p.pieces[1], &piece)),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-ss", "2.000000", "-i", "/v/제주 여행 0412.mp4", "-t", "8.000000",
                "-map", "0:0", "-map", "0:a?", "-c", "copy", "-map_metadata", "0",
                piece.to_str().unwrap(),
            ],
            "no faststart for a piece; the piece runs to the file's end"
        );
        assert_eq!(
            strs(&lossless_concat_args(&list, Path::new("/v/제주 여행 0412.mp4"), Path::new("/v/제주 여행 0412_cut.part.mp4"))),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-f", "concat", "-i", list.to_str().unwrap(),
                "-i", "/v/제주 여행 0412.mp4",
                "-map", "0", "-c", "copy", "-map_metadata", "1", "-map_chapters", "-1",
                "-movflags", "+faststart",
                "/v/제주 여행 0412_cut.part.mp4",
            ]
        );
        assert!(!strs(&lossless_concat_args(Path::new("l.txt"), Path::new("a.mkv"), Path::new("a_cut.part.mkv"))).contains(&"-movflags".to_string()));
        assert_eq!(
            concat_list(&[("piece-000.mp4".into(), 2.4), ("piece-001.mp4".into(), 7.0 / 3.0)]),
            "ffconcat version 1.0\nfile 'piece-000.mp4'\nduration 2.400000\nfile 'piece-001.mp4'\nduration 2.333334\n",
            "rounded up to the microsecond"
        );
        assert_eq!(piece_name(12, "mkv").unwrap(), "piece-012.mkv");
        assert!(piece_name(0, "").is_err() && piece_name(0, "мп4").is_err());
    }

    #[test]
    fn concat_durations_keep_every_stream_clear_of_the_next_piece() {
        let b = |start: f64, video_end: Option<f64>, audio: &[(u32, f64, f64)]| PieceBounds { start, video_end, audio: audio.to_vec() };
        let pieces = [
            // Audio runs 13 ms past the video; the next piece's audio starts 16 ms before its video
            // (MP4: the packet holding the keyframe's time): 2.3467 + 0.016.
            b(0.0, Some(7.0 / 3.0), &[(1, -0.021333, 2.346667)]),
            // B-frame tail: the video ends 0.133 s past the audio.
            b(0.0, Some(4.133333), &[(1, -0.016, 4.005333)]),
            // MKV-like: times start late, audio starts 21 ms after the video.
            b(0.5, Some(3.5), &[(1, 0.521, 3.48)]),
            b(0.0, Some(6.0), &[(1, 0.0, 6.01)]),
        ];
        let d = concat_durations(&pieces);
        let want = [2.346667 + 0.016, 4.133333, 3.0, 6.01];
        for (got, want) in d.iter().zip(want) {
            assert!((got - want).abs() < 1e-9, "{d:?}");
        }
        // Video only, and a track the next piece lacks.
        assert_eq!(concat_durations(&[b(0.0, Some(2.0), &[(2, 0.0, 2.5)]), b(0.0, Some(1.0), &[])]), vec![2.0, 1.0]);
    }

    #[test]
    fn random_access_from_the_trace_headers_log() {
        let log = |lines: &[&str]| lines.iter().map(|l| format!("[trace_headers @ 0x1] {l}")).collect::<Vec<_>>().join("\n");
        let hevc = |nal: &str| {
            log(&[
                "1           nal_unit_type                                          100000 = 32",
                "1           nal_unit_type                                          100111 = 39",
                "Packet: 9452 bytes, key frame, pts 0, dts -2560, duration 512.",
                "1           nal_unit_type                                          100111 = 39",
                nal,
            ])
        };
        let cra = "1           nal_unit_type                                          010101 = 21";
        let idr = "1           nal_unit_type                                          010100 = 20";
        assert_eq!(starts_a_sequence("hevc", &hevc(cra)), Some(false), "x265 open GOP");
        assert_eq!(starts_a_sequence("hevc", &hevc(idr)), Some(true));
        let h264 = |nal: &str| log(&["3   nal_unit_type    00111 = 7", "Packet: 10201 bytes, key frame", "3   nal_unit_type    00110 = 6", nal]);
        assert_eq!(starts_a_sequence("h264", &h264("3   nal_unit_type    00101 = 5")), Some(true));
        assert_eq!(starts_a_sequence("h264", &h264("3   nal_unit_type    00001 = 1")), Some(false), "x264 open-gop");
        let av1 = |shown: &str| {
            log(&["1   obu_type   0001 = 1", "Packet: 12463 bytes, key frame", "1   obu_type   0110 = 6", "26  show_existing_frame   0 = 0", "25  frame_type   00 = 0", shown])
        };
        assert_eq!(starts_a_sequence("av1", &av1("27  show_frame   1 = 1")), Some(true));
        assert_eq!(starts_a_sequence("av1", &av1("27  show_frame   0 = 0")), Some(false));
        assert_eq!(starts_a_sequence("hevc", "Unknown bitstream filter trace_headers"), None);
        assert_eq!(starts_a_sequence("vp9", &hevc(idr)), None);
        let a = strs(&random_access_check_args(Path::new("/v/제주 여행.mkv"), 1, 2.1344));
        assert_eq!(
            a.join(" "),
            "-hide_banner -nostdin -loglevel info -ss 2.134400 -i /v/제주 여행.mkv -map 0:1 -c copy -frames:v 1 -bsf:v trace_headers -f null -"
        );
    }

    #[test]
    fn output_paths_with_another_container() {
        let (f, p) = output_paths_as(Path::new("/없는 폴더/클립 01.mov"), "_trim_h265", Some("mp4"));
        assert_eq!(f, Path::new("/없는 폴더/클립 01_trim_h265.mp4"));
        assert_eq!(p, Path::new("/없는 폴더/클립 01_trim_h265.part.mp4"));
    }

    // --- 일괄 변환 ---

    fn sized(mut m: MediaInfo, width: u32, height: u32, rotation: Option<f64>) -> MediaInfo {
        let v = m.video.as_mut().unwrap();
        (v.width, v.height, v.rotation) = (width, height, rotation);
        m
    }

    fn opts(codec: VideoCodec, container: Container, resolution: Resolution, audio: AudioChoice, ten_bit: bool) -> ConvertOptions {
        ConvertOptions { codec, container, resolution, audio, ten_bit }
    }

    #[test]
    fn convert_snapshot_x265_mp4_downscaled_to_ten_bits() {
        // H.264 MKV with Opus → H.265 MP4, 720p, 10 bits asked for: Opus cannot be copied into MP4.
        let info = sized(clip("/v/회의 녹화 01.mkv", "h264", &["opus"]), 1920, 1080, None);
        let o = opts(VideoCodec::Hevc, Container::Mp4, Resolution::P720, AudioChoice::CopyOrAac, true);
        let plan = plan_convert(&info, &o).unwrap();
        assert_eq!(
            (plan.scale.as_deref(), plan.size, plan.audio, plan.ten_bit, plan.duration),
            (Some("scale=1280:720"), [1280, 720], AudioOut::Aac { bitrate: 192 }, true, 10.0)
        );
        let a = convert_args(
            Path::new("/v/회의 녹화 01.mkv"),
            info.video.as_ref().unwrap(),
            &plan,
            "libx265",
            Quality::Balanced,
            Path::new("/변환 결과/회의 녹화 01_h265.part.mp4"),
        )
        .unwrap();
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-i", "/v/회의 녹화 01.mkv",
                "-map", "0:0", "-map", "0:a?",
                "-c:v", "libx265", "-crf", "24", "-preset", "medium", "-x265-params", "log-level=error",
                "-pix_fmt", "yuv420p10le", "-tag:v", "hvc1",
                "-vf", "scale=1280:720",
                "-fps_mode", "passthrough", "-enc_time_base:v", "1/90000",
                "-c:a", "aac", "-b:a", "192k",
                "-map_metadata", "0", "-movflags", "+faststart",
                "/변환 결과/회의 녹화 01_h265.part.mp4",
            ]
        );
    }

    #[test]
    fn convert_snapshot_videotoolbox_hdr_portrait_mov_copies_audio() {
        // A 4K HDR phone video held upright (stored landscape, rotated): 1080p means 1080 wide.
        let mut info = sized(clip("/v/아이폰 가족.MOV", "hevc", &["aac"]), 3840, 2160, Some(-90.0));
        {
            let v = info.video.as_mut().unwrap();
            v.pix_fmt = Some("yuv420p10le".into());
            (v.color_primaries, v.color_transfer, v.color_space) =
                (Some("bt2020".into()), Some("arib-std-b67".into()), Some("bt2020nc".into()));
            v.hdr = true;
        }
        let o = opts(VideoCodec::Hevc, Container::Mov, Resolution::P1080, AudioChoice::CopyOrAac, false);
        let plan = plan_convert(&info, &o).unwrap();
        assert_eq!((plan.scale.as_deref(), plan.size, plan.audio, plan.ten_bit), (Some("scale=1080:1920"), [1080, 1920], AudioOut::Copy, true));
        let a = convert_args(
            Path::new("/v/아이폰 가족.MOV"),
            info.video.as_ref().unwrap(),
            &plan,
            "hevc_videotoolbox",
            Quality::High,
            Path::new("/v/아이폰 가족_작게.part.mov"),
        )
        .unwrap();
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-i", "/v/아이폰 가족.MOV",
                "-map", "0:0", "-map", "0:a?",
                "-c:v", "hevc_videotoolbox", "-q:v", "68", "-profile:v", "main10", "-pix_fmt", "p010le",
                "-tag:v", "hvc1",
                "-vf", "scale=1080:1920,setparams=color_primaries=bt2020:color_trc=arib-std-b67:colorspace=bt2020nc",
                "-fps_mode", "passthrough", "-enc_time_base:v", "1/90000",
                "-c:a", "copy",
                "-map_metadata", "0", "-movflags", "+faststart",
                "/v/아이폰 가족_작게.part.mov",
            ]
        );
    }

    #[test]
    fn convert_snapshot_svtav1_webm_with_opus() {
        // Already 720p: nothing to scale. AAC cannot go in WebM: Opus 128k, laid out for libopus.
        let info = sized(clip("/v/화면 녹화 데모.mp4", "h264", &["aac"]), 1280, 720, None);
        let o = opts(VideoCodec::Av1, Container::Webm, Resolution::P720, AudioChoice::CopyOrAac, false);
        let plan = plan_convert(&info, &o).unwrap();
        assert_eq!((plan.scale.as_deref(), plan.size, plan.audio, plan.ten_bit), (None, [1280, 720], AudioOut::Opus { bitrate: 128 }, false));
        let a = convert_args(
            Path::new("/v/화면 녹화 데모.mp4"),
            info.video.as_ref().unwrap(),
            &plan,
            "libsvtav1",
            Quality::Small,
            Path::new("/v/화면 녹화 데모_av1.part.webm"),
        )
        .unwrap();
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-i", "/v/화면 녹화 데모.mp4",
                "-map", "0:0", "-map", "0:a?",
                "-c:v", "libsvtav1", "-crf", "48", "-preset", "8", "-pix_fmt", "yuv420p",
                "-fps_mode", "passthrough", "-enc_time_base:v", "1/90000",
                "-af", "aformat=channel_layouts=mono|stereo|3.0|quad|5.0|5.1|6.1|7.1",
                "-c:a", "libopus", "-b:a", "128k",
                "-map_metadata", "0",
                "/v/화면 녹화 데모_av1.part.webm",
            ]
        );
        // No audio track: no audio map or options at all.
        let silent = plan_convert(&sized(clip("/v/무음.mkv", "h264", &[]), 1280, 720, None), &o).unwrap();
        assert_eq!(silent.audio, AudioOut::None);
        let a = strs(&convert_args(Path::new("/v/무음.mkv"), info.video.as_ref().unwrap(), &silent, "libsvtav1", Quality::Small, Path::new("/v/o.webm")).unwrap());
        assert!(!a.iter().any(|s| s == "0:a?" || s == "-c:a" || s == "-af"), "{a:?}");
    }

    #[test]
    fn convert_sample_snapshot_is_the_conversion_over_a_stretch() {
        // 예상 크기: the same conversion, 10 s from 1/6 of a 10-minute file, into the scratch folder.
        let mut info = sized(clip("/v/회의 녹화 01.mkv", "h264", &["opus"]), 1920, 1080, None);
        info.duration = 600.0;
        let o = opts(VideoCodec::Hevc, Container::Mp4, Resolution::P720, AudioChoice::CopyOrAac, true);
        let plan = plan_convert(&info, &o).unwrap();
        let video = info.video.as_ref().unwrap();
        let out = Path::new("/캐시/예상 크기/sample-1.part.mp4");
        let a = convert_sample_args(Path::new("/v/회의 녹화 01.mkv"), video, &plan, "libx265", Quality::Balanced, plan::Range::new(95.0, 105.0), out).unwrap();
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y",
                "-ss", "95.000000", "-i", "/v/회의 녹화 01.mkv", "-t", "10.000000",
                "-map", "0:0", "-map", "0:a?",
                "-c:v", "libx265", "-crf", "24", "-preset", "medium", "-x265-params", "log-level=error",
                "-pix_fmt", "yuv420p10le", "-tag:v", "hvc1",
                "-vf", "scale=1280:720",
                "-fps_mode", "passthrough", "-enc_time_base:v", "1/90000",
                "-c:a", "aac", "-b:a", "192k",
                "-map_metadata", "0", "-movflags", "+faststart",
                "/캐시/예상 크기/sample-1.part.mp4",
            ]
        );
        // A stretch over the whole file is the conversion itself (a short file is tried whole).
        let whole = convert_sample_args(Path::new("/v/회의 녹화 01.mkv"), video, &plan, "libx265", Quality::Balanced, plan::Range::new(0.0, 600.0), out).unwrap();
        assert_eq!(whole, convert_args(Path::new("/v/회의 녹화 01.mkv"), video, &plan, "libx265", Quality::Balanced, out).unwrap());
    }

    #[test]
    fn convert_audio_follows_the_container() {
        let audio = |codecs: &[&str], container: Container, choice: AudioChoice| {
            let codec = if container == Container::Webm || container == Container::Mkv { VideoCodec::Av1 } else { VideoCodec::Hevc };
            plan_convert(&clip("/v/a.mov", "h264", codecs), &opts(codec, container, Resolution::Source, choice, false)).unwrap().audio
        };
        use AudioChoice::*;
        assert_eq!(audio(&["aac", "ac3"], Container::Mp4, CopyOrAac), AudioOut::Copy);
        assert_eq!(audio(&["aac", "dts"], Container::Mp4, CopyOrAac), AudioOut::Aac { bitrate: 192 }, "every track must fit");
        assert_eq!(audio(&["opus"], Container::Mp4, CopyOrAac), AudioOut::Aac { bitrate: 192 });
        assert_eq!(audio(&["pcm_s16le"], Container::Mov, CopyOrAac), AudioOut::Copy, "camera PCM stays in MOV");
        assert_eq!(audio(&["pcm_s16le"], Container::Mp4, CopyOrAac), AudioOut::Aac { bitrate: 192 });
        assert_eq!(audio(&["opus", "flac"], Container::Mkv, CopyOrAac), AudioOut::Copy);
        assert_eq!(audio(&["opus"], Container::Webm, CopyOrAac), AudioOut::Copy);
        assert_eq!(audio(&["aac"], Container::Webm, CopyOrAac), AudioOut::Opus { bitrate: OPUS_BITRATE });
        assert_eq!(audio(&["opus"], Container::Webm, Aac192), AudioOut::Opus { bitrate: OPUS_BITRATE }, "always re-encoded");
        assert_eq!(audio(&["aac"], Container::Mp4, Aac192), AudioOut::Aac { bitrate: 192 });
        assert_eq!(audio(&["aac"], Container::Mkv, Aac128), AudioOut::Aac { bitrate: 128 });
        assert_eq!(audio(&[], Container::Mp4, Aac128), AudioOut::None);
    }

    #[test]
    fn convert_scales_the_short_side_down_only() {
        let size = |w: u32, h: u32, rotation: Option<f64>, r: Resolution| {
            let p = plan_convert(&sized(clip("/v/a.mp4", "h264", &[]), w, h, rotation), &opts(VideoCodec::Av1, Container::Mp4, r, AudioChoice::CopyOrAac, false)).unwrap();
            (p.scale, p.size)
        };
        let s = |f: &str| Some(f.to_string());
        assert_eq!(size(3840, 2160, None, Resolution::P1080), (s("scale=1920:1080"), [1920, 1080]));
        assert_eq!(size(1920, 1080, None, Resolution::P1080), (None, [1920, 1080]), "already 1080p");
        assert_eq!(size(1280, 720, None, Resolution::P1080), (None, [1280, 720]), "never up");
        assert_eq!(size(1440, 1080, None, Resolution::P720), (s("scale=960:720"), [960, 720]));
        assert_eq!(size(1918, 1080, None, Resolution::P720), (s("scale=1278:720"), [1278, 720]), "the numbers scale=-2:720 makes");
        assert_eq!(size(1080, 1920, None, Resolution::P720), (s("scale=720:1280"), [720, 1280]), "portrait");
        assert_eq!(size(1920, 1080, Some(90.0), Resolution::P720), (s("scale=720:1280"), [720, 1280]), "stored landscape, shown upright");
        assert_eq!(size(1920, 1080, Some(180.0), Resolution::P720), (s("scale=1280:720"), [1280, 720]));
        assert_eq!(size(3840, 2160, None, Resolution::Source), (None, [3840, 2160]));
        assert_eq!(size(720, 480, None, Resolution::P720), (None, [720, 480]));
    }

    #[test]
    fn convert_plan_refuses_what_it_cannot_make() {
        let info = clip("/v/a.mp4", "h264", &["aac"]);
        let plan = |codec, container| plan_convert(&info, &opts(codec, container, Resolution::Source, AudioChoice::CopyOrAac, false));
        assert_eq!(plan(VideoCodec::Av1, Container::Mov).unwrap_err(), Msg::CodecNotInContainer { codec: "AV1".into(), container: "MOV".into() });
        assert_eq!(plan(VideoCodec::Hevc, Container::Webm).unwrap_err(), Msg::CodecNotInContainer { codec: "H.265".into(), container: "WEBM".into() });
        assert_eq!(plan(VideoCodec::H264, Container::Mp4).unwrap_err(), Msg::ConvertCodecUnsupported);
        for (codec, container) in [(VideoCodec::Hevc, Container::Mp4), (VideoCodec::Hevc, Container::Mkv), (VideoCodec::Hevc, Container::Mov), (VideoCodec::Av1, Container::Mp4), (VideoCodec::Av1, Container::Mkv), (VideoCodec::Av1, Container::Webm)] {
            assert!(plan(codec, container).is_ok(), "{codec:?} {container:?}");
        }
        let mut no_video = info.clone();
        no_video.video = None;
        assert_eq!(plan_convert(&no_video, &opts(VideoCodec::Hevc, Container::Mp4, Resolution::Source, AudioChoice::CopyOrAac, false)).unwrap_err(), Msg::ConvertNeedsVideo);
        let mut no_length = info.clone();
        no_length.duration = 0.0;
        assert!(plan_convert(&no_length, &opts(VideoCodec::Hevc, Container::Mp4, Resolution::Source, AudioChoice::CopyOrAac, false)).is_err());

        // 10 bits: asked for, or kept from a 10-bit / HDR source.
        let ten = |pix: &str, asked: bool| {
            let mut m = clip("/v/a.mp4", "h264", &[]);
            m.video.as_mut().unwrap().pix_fmt = Some(pix.into());
            plan_convert(&m, &opts(VideoCodec::Hevc, Container::Mp4, Resolution::Source, AudioChoice::CopyOrAac, asked)).unwrap().ten_bit
        };
        assert_eq!((ten("yuv420p", false), ten("yuv420p", true), ten("yuv420p10le", false)), (false, true, true));
    }

    #[test]
    fn convert_options_from_the_ui() {
        let o: ConvertOptions = serde_json::from_str(r#"{"codec":"av1","container":"webm","resolution":"p720","audio":"aac128","tenBit":true}"#).unwrap();
        assert_eq!(o, opts(VideoCodec::Av1, Container::Webm, Resolution::P720, AudioChoice::Aac128, true));
        let json = serde_json::to_value(AudioOut::Aac { bitrate: 192 }).unwrap();
        assert_eq!(json, serde_json::json!({"kind": "aac", "bitrate": 192}));
        assert_eq!(serde_json::to_value(AudioOut::Copy).unwrap(), serde_json::json!({"kind": "copy"}));
    }

    #[test]
    fn name_suffix_rules() {
        for ok in ["_h265", "", " 작게", "_v2", "-보관용 (2026)"] {
            assert!(check_name_suffix(ok).is_ok(), "{ok}");
        }
        for bad in ["a/b", "a\\b", "a:b", "제목?", "x\u{7}", "_작게.part", ".PART2"] {
            assert!(check_name_suffix(bad).is_err(), "{bad}");
        }
        assert!(check_name_suffix(&"가".repeat(60)).is_ok());
        assert_eq!(check_name_suffix(&"가".repeat(61)), Err(Msg::SuffixTooLong { max: 60 }));
        assert_eq!(check_name_suffix("제목?"), Err(Msg::SuffixBadChar { ch: Some("?".into()) }));
        assert_eq!(check_name_suffix("x\u{7}"), Err(Msg::SuffixBadChar { ch: None }), "a control character");
        assert_eq!(check_name_suffix(".PART2"), Err(Msg::SuffixPart));
    }

    #[test]
    fn output_paths_in_another_folder_never_reach_the_original() {
        let dir = std::env::temp_dir().join(format!("godseon 변환 이름 {}", std::process::id()));
        let out = dir.join("변환 결과");
        std::fs::create_dir_all(&out).unwrap();
        let input = dir.join("여행 01.mp4");
        std::fs::write(&input, b"x").unwrap();
        let free = |_: &Path| false;
        // An empty ending in the original's folder: the original's name is taken by the original.
        let (f, p) = output_paths_in(&dir, &input, "", Some("mp4"), &free);
        assert_eq!((f.file_name().unwrap(), p.file_name().unwrap()), ("여행 01 (1).mp4".as_ref(), "여행 01 (1).part.mp4".as_ref()));
        let (f, _) = output_paths_in(&out, &input, "", Some("mp4"), &free);
        assert_eq!(f, out.join("여행 01.mp4"));
        let (f, p) = output_paths_in(&out, &input, "_av1", Some("webm"), &|q| q == out.join("여행 01_av1.part.webm"));
        assert_eq!((f, p), (out.join("여행 01_av1 (1).webm"), out.join("여행 01_av1 (1).part.webm")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
