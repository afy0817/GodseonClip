//! Preview data for the editor (canvas 03 · 3.5):
//! - a 540p H.264 proxy for files the webview cannot play (WKWebView on macOS cannot open MKV),
//!   cached by file identity. The job runs through `JobManager`; its arguments are
//!   `command::proxy_args`.
//! - a thumbnail strip for the timeline, decoded from keyframes only, so a long 4K file takes
//!   seconds instead of a full decode.
//! - an audio waveform: the peak of each time bucket of 8 kHz mono samples, streamed so memory
//!   stays small for long files.
//!
//! Every time is on the original's timeline (0 = start of the file, the clock `-ss` uses).
//! The proxy is only for viewing; edits always refer to the original.

use crate::command::secs;
use crate::plan;
use crate::probe::MediaInfo;
use crate::msg::Msg;
use crate::util::{last_line, run_capture, run_streaming};
use serde::Serialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

// ---------------------------------------------------------------- proxy

/// Bump when `command::proxy_args` changes, so proxies made by older versions are rebuilt.
/// 2: AAC with the `twoloop` coder (`command::AAC_ENCODER`).
const PROXY_VERSION: u32 = 2;

/// Final and `.part` paths of the proxy of `input` inside `cache_dir`.
///
/// The name is a hash of the file's identity (canonical path, size, modification time), so a
/// replaced or re-recorded file gets a new proxy while reopening the same file reuses the old one.
pub fn proxy_paths(cache_dir: &Path, input: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    let meta = std::fs::metadata(input)?;
    let canonical = std::fs::canonicalize(input).unwrap_or_else(|_| input.to_path_buf());
    let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok());
    let key = proxy_key(&canonical, meta.len(), modified);
    Ok((cache_dir.join(format!("{key}.mp4")), cache_dir.join(format!("{key}.part.mp4"))))
}

pub fn proxy_key(path: &Path, size: u64, modified: Option<Duration>) -> String {
    let modified = modified.unwrap_or_default();
    let mut h = Fnv64::new();
    h.write(&PROXY_VERSION.to_le_bytes());
    h.write(path.as_os_str().as_encoded_bytes());
    h.write(&[0]);
    h.write(&size.to_le_bytes());
    h.write(&modified.as_secs().to_le_bytes());
    h.write(&modified.subsec_nanos().to_le_bytes());
    format!("{:016x}", h.0)
}

/// FNV-1a 64. Unlike `DefaultHasher` it is stable across Rust releases, so cache names
/// survive app updates.
struct Fnv64(u64);

impl Fnv64 {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

// ---------------------------------------------------------------- thumbnails

pub const MAX_THUMBNAILS: u32 = 400;
const THUMBNAIL_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailStrip {
    /// Tile `k` covers `[start + k·interval, start + (k+1)·interval)` of the original's timeline.
    pub start: f64,
    pub interval: f64,
    /// JPEG data URLs, one per tile. Each shows the last keyframe at or before the middle of its tile.
    pub images: Vec<String>,
    pub elapsed_ms: u64,
}

/// One JPEG per tile, written to stdout as an MJPEG stream (`image2pipe`).
///
/// `-skip_frame nokey` decodes keyframes only, which is what makes long 4K files fast.
/// `-noaccurate_seek` keeps the keyframe before the seek point, and `fps … round=up` puts in
/// slot `k` the last frame at or before `seek + k·interval`. Seeking to the middle of the first
/// tile therefore gives every tile the last keyframe at or before its middle (measured with
/// FFmpeg 9.0.2, MP4 and MKV: see `thumbnails_and_waveform_follow_the_source_timeline`).
///
/// Without `tpad`, the stream ends one frame after the last keyframe, so tiles whose middle lies
/// beyond that got no image (a 30 s file with a keyframe every 2 s gave 15 of 16). Cloning the last
/// frame for the length of the range fills them; `-frames:v` stops FFmpeg once every tile is out.
pub fn thumbnail_args(
    input: &Path,
    video_index: u32,
    start: f64,
    interval_us: u64,
    count: u32,
    height: u32,
) -> Vec<OsString> {
    let seek = start + interval_us as f64 / 2e6;
    let mut a = os(&["-hide_banner", "-nostdin", "-loglevel", "error", "-skip_frame", "nokey", "-noaccurate_seek"]);
    a.extend(["-ss".into(), secs(seek).into(), "-i".into(), input.as_os_str().to_owned()]);
    a.extend(["-map".into(), format!("0:{video_index}").into()]);
    let span = secs(interval_us as f64 * f64::from(count) / 1e6);
    let filter = format!(
        "tpad=stop_mode=clone:stop_duration={span},fps=fps=1000000/{interval_us}:start_time=0:round=up,scale=-2:{height}"
    );
    a.extend(["-vf".into(), filter.into(), "-frames:v".into(), count.to_string().into()]);
    a.extend(os(&["-q:v", "5", "-f", "image2pipe", "-c:v", "mjpeg", "-"]));
    a
}

/// `count` thumbnails `height` px tall, evenly spread over `[start, end)` (default: whole file).
pub fn thumbnails(
    ffmpeg: &Path,
    input: &Path,
    info: &MediaInfo,
    start: Option<f64>,
    end: Option<f64>,
    count: u32,
    height: u32,
) -> Result<ThumbnailStrip, Msg> {
    let video = info.video.as_ref().ok_or(Msg::NoVideoTrack)?;
    let range = resolve_range(info, start, end)?;
    let count = count.clamp(1, MAX_THUMBNAILS);
    let height = (height.clamp(16, 360) & !1).max(2);
    let interval_us = ((range.len() / f64::from(count)) * 1e6).round().max(1.0) as u64;
    let args = thumbnail_args(input, video.index, range.start, interval_us, count, height);

    let started = Instant::now();
    let out = run_capture(ffmpeg, &args, THUMBNAIL_TIMEOUT).map_err(Msg::launch)?;
    if out.timed_out {
        return Err(Msg::ThumbnailsTimedOut);
    }
    if !out.success {
        return Err(Msg::line_or(last_line(&out.stderr, 300), Msg::ThumbnailsFailed));
    }
    let images: Vec<String> = split_jpegs(&out.stdout_raw)
        .into_iter()
        .map(|jpeg| format!("data:image/jpeg;base64,{}", base64(jpeg)))
        .collect();
    if images.is_empty() {
        return Err(Msg::ThumbnailsNoKeyframes);
    }
    Ok(ThumbnailStrip {
        start: range.start,
        interval: interval_us as f64 / 1e6,
        images,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// Splits an MJPEG stream into single JPEG files by walking the JPEG markers (length-prefixed
/// segments, then entropy-coded data up to EOI), so a stray `FF D9` inside a segment payload
/// cannot cut an image short. A truncated last image is dropped.
pub fn split_jpegs(data: &[u8]) -> Vec<&[u8]> {
    let mut images = Vec::new();
    let mut i = 0;
    while let Some(offset) = data[i..].windows(2).position(|w| w == [0xFF, 0xD8]) {
        let start = i + offset;
        match jpeg_len(&data[start..]) {
            Some(len) => {
                images.push(&data[start..start + len]);
                i = start + len;
            }
            None => break,
        }
    }
    images
}

/// Length of the JPEG at the start of `d` (which begins with SOI), or None if it is truncated.
fn jpeg_len(d: &[u8]) -> Option<usize> {
    let seg_len = |p: usize| -> Option<usize> {
        let len = usize::from(u16::from_be_bytes([*d.get(p + 2)?, *d.get(p + 3)?]));
        (len >= 2).then_some(len)
    };
    let mut p = 2;
    loop {
        if *d.get(p)? != 0xFF {
            return None; // not a marker: corrupt stream
        }
        match *d.get(p + 1)? {
            0xFF => p += 1,                       // fill byte
            0xD9 => return Some(p + 2),           // EOI
            0x01 | 0xD0..=0xD7 => p += 2,         // TEM, RSTn: no length
            0xDA => {
                p += 2 + seg_len(p)?;             // SOS header, then entropy-coded data
                loop {
                    if *d.get(p)? == 0xFF {
                        match *d.get(p + 1)? {
                            0x00 | 0xD0..=0xD7 => p += 2, // stuffed byte, restart marker
                            0xFF => p += 1,
                            _ => break,                   // next real marker
                        }
                    } else {
                        p += 1;
                    }
                }
            }
            _ => p += 2 + seg_len(p)?,
        }
    }
}

/// Standard base64 with padding (RFC 4648), for `data:` URLs.
pub fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for (k, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            s.push(if k <= c.len() { char::from(ABC[((n >> shift) & 63) as usize]) } else { '=' });
        }
    }
    s
}

// ---------------------------------------------------------------- waveform

/// Sample rate the waveform is computed at (canvas 03 · 3.5).
pub const WAVEFORM_RATE: u32 = 8000;
pub const MAX_WAVEFORM_BUCKETS: u32 = 20_000;
const WAVEFORM_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Waveform {
    /// Bucket `k` of `n` covers `[start + k·(end−start)/n, start + (k+1)·(end−start)/n)`.
    pub start: f64,
    pub end: f64,
    /// Peak |sample| of each bucket of the first audio track, 0–1 (1 = full scale).
    pub peaks: Vec<f32>,
    pub elapsed_ms: u64,
}

/// First audio track as 8 kHz mono 16-bit PCM on stdout.
///
/// `aresample … async=1:first_pts=0` pads the start with silence when the audio track begins after
/// the file start (or after `-ss`), so sample 0 is always at `start` on the original's timeline.
pub fn waveform_args(input: &Path, start: f64, duration: f64) -> Vec<OsString> {
    let mut a = os(&["-hide_banner", "-nostdin", "-loglevel", "error"]);
    if start > 0.0 {
        a.extend(["-ss".into(), secs(start).into()]);
    }
    a.extend(["-i".into(), input.as_os_str().to_owned(), "-t".into(), secs(duration).into()]);
    let filter = format!("aresample={WAVEFORM_RATE}:async=1:first_pts=0");
    a.extend(["-map".into(), "0:a:0".into(), "-af".into(), filter.into()]);
    a.extend(os(&["-ac", "1", "-f", "s16le", "-"]));
    a
}

/// `buckets` peaks of the first audio track over `[start, end)` (default: whole file).
pub fn waveform(
    ffmpeg: &Path,
    input: &Path,
    info: &MediaInfo,
    start: Option<f64>,
    end: Option<f64>,
    buckets: u32,
) -> Result<Waveform, Msg> {
    if info.audio.is_empty() {
        return Err(Msg::NoAudioTrack);
    }
    let range = resolve_range(info, start, end)?;
    let buckets = buckets.clamp(1, MAX_WAVEFORM_BUCKETS);
    let args = waveform_args(input, range.start, range.len());

    let started = Instant::now();
    let mut meter = PeakMeter::new(WAVEFORM_RATE, range.len(), buckets as usize);
    let out = run_streaming(ffmpeg, &args, WAVEFORM_TIMEOUT, |chunk| meter.feed(chunk))
        .map_err(Msg::launch)?;
    if out.timed_out {
        return Err(Msg::WaveformTimedOut);
    }
    if !out.success {
        return Err(Msg::line_or(last_line(&out.stderr, 300), Msg::WaveformFailed));
    }
    Ok(Waveform {
        start: range.start,
        end: range.end,
        peaks: meter.finish(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// Reduces a stream of 16-bit little-endian mono samples to the peak of each time bucket.
/// Chunks may split a sample in two; samples past the last bucket are ignored.
pub struct PeakMeter {
    samples_per_bucket: f64,
    peaks: Vec<u16>,
    seen: u64,
    odd_byte: Option<u8>,
}

impl PeakMeter {
    pub fn new(rate: u32, duration: f64, buckets: usize) -> Self {
        let buckets = buckets.max(1);
        Self {
            samples_per_bucket: (f64::from(rate) * duration / buckets as f64).max(f64::MIN_POSITIVE),
            peaks: vec![0; buckets],
            seen: 0,
            odd_byte: None,
        }
    }

    pub fn feed(&mut self, mut bytes: &[u8]) {
        if let Some(lo) = self.odd_byte.take() {
            match bytes.split_first() {
                Some((&hi, rest)) => {
                    self.push(i16::from_le_bytes([lo, hi]));
                    bytes = rest;
                }
                None => {
                    self.odd_byte = Some(lo);
                    return;
                }
            }
        }
        let (pairs, rest) = bytes.as_chunks::<2>();
        for pair in pairs {
            self.push(i16::from_le_bytes(*pair));
        }
        if let [last] = rest {
            self.odd_byte = Some(*last);
        }
    }

    fn push(&mut self, sample: i16) {
        let bucket = (self.seen as f64 / self.samples_per_bucket) as usize;
        self.seen += 1;
        if let Some(peak) = self.peaks.get_mut(bucket) {
            *peak = (*peak).max(sample.unsigned_abs());
        }
    }

    /// Peaks scaled to 0–1, rounded to 3 decimals to keep the IPC payload small.
    pub fn finish(self) -> Vec<f32> {
        self.peaks
            .iter()
            .map(|&p| ((f32::from(p) / 32768.0).min(1.0) * 1000.0).round() / 1000.0)
            .collect()
    }
}

// ---------------------------------------------------------------- shared

fn resolve_range(info: &MediaInfo, start: Option<f64>, end: Option<f64>) -> Result<plan::Range, Msg> {
    Ok(plan::keep_from_trim(start.unwrap_or(0.0), end.unwrap_or(info.duration), info.duration)?)
}

fn os(parts: &[&str]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(a: &[OsString]) -> Vec<String> {
        a.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn proxy_key_follows_file_identity() {
        let p = Path::new("/영상/OBS 녹화 01.mkv");
        let t = Some(Duration::new(1_763_000_000, 5));
        let k = proxy_key(p, 1000, t);
        assert_eq!(k.len(), 16);
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(k, proxy_key(p, 1000, t), "same file, same proxy");
        assert_ne!(k, proxy_key(p, 1001, t), "size changed");
        assert_ne!(k, proxy_key(p, 1000, Some(Duration::new(1_763_000_000, 6))), "modified");
        assert_ne!(k, proxy_key(Path::new("/영상/OBS 녹화 02.mkv"), 1000, t), "other file");
    }

    #[test]
    fn fnv_matches_reference_values() {
        let mut h = Fnv64::new();
        h.write(b"");
        assert_eq!(h.0, 0xcbf2_9ce4_8422_2325);
        let mut h = Fnv64::new();
        h.write(b"a");
        assert_eq!(h.0, 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn proxy_paths_live_in_the_cache_dir() {
        let dir = std::env::temp_dir().join(format!("godseon-proxy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("테스트 클립 01.mkv");
        std::fs::write(&input, b"x").unwrap();
        let cache = Path::new("/cache/proxies");
        let (fin, part) = proxy_paths(cache, &input).unwrap();
        assert_eq!(fin.parent().unwrap(), cache);
        let stem = fin.file_stem().unwrap().to_string_lossy().into_owned();
        assert_eq!(fin.extension().unwrap(), "mp4");
        assert_eq!(part.file_name().unwrap().to_string_lossy(), format!("{stem}.part.mp4"));
        assert_eq!(proxy_paths(cache, &input).unwrap().0, fin, "stable for the same file");
        std::fs::write(&input, b"xy").unwrap();
        assert_ne!(proxy_paths(cache, &input).unwrap().0, fin, "a changed file gets a new proxy");
        assert!(proxy_paths(cache, &dir.join("없음.mkv")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn thumbnail_snapshot() {
        let a = thumbnail_args(Path::new("/v/제주 여행.mkv"), 0, 60.0, 10_000_000, 30, 90);
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-nostdin", "-loglevel", "error", "-skip_frame", "nokey", "-noaccurate_seek",
                "-ss", "65.000", "-i", "/v/제주 여행.mkv", "-map", "0:0",
                "-vf",
                "tpad=stop_mode=clone:stop_duration=300.000,fps=fps=1000000/10000000:start_time=0:round=up,scale=-2:90",
                "-frames:v", "30",
                "-q:v", "5", "-f", "image2pipe", "-c:v", "mjpeg", "-",
            ]
        );
    }

    #[test]
    fn waveform_snapshot() {
        let a = waveform_args(Path::new("/v/제주 여행.mkv"), 0.0, 600.0);
        assert_eq!(
            strs(&a),
            vec![
                "-hide_banner", "-nostdin", "-loglevel", "error", "-i", "/v/제주 여행.mkv", "-t", "600.000",
                "-map", "0:a:0", "-af", "aresample=8000:async=1:first_pts=0", "-ac", "1", "-f", "s16le", "-",
            ]
        );
        let a = strs(&waveform_args(Path::new("a.mp4"), 65.4, 10.0));
        assert_eq!(a[4..8], ["-ss", "65.400", "-i", "a.mp4"]);
    }

    /// A minimal JPEG-shaped byte string: SOI, a COM segment, SOS, entropy data, EOI.
    fn fake_jpeg(comment: &[u8], entropy: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xFE];
        v.extend(((comment.len() + 2) as u16).to_be_bytes());
        v.extend(comment);
        v.extend([0xFF, 0xDA, 0x00, 0x04, 0x01, 0x02]);
        v.extend(entropy);
        v.extend([0xFF, 0xD9]);
        v
    }

    #[test]
    fn splits_mjpeg_stream() {
        // FF D9 inside the comment, a stuffed FF 00 and a restart marker inside the entropy data.
        let a = fake_jpeg(b"Lavc\xFF\xD9", &[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD3, 0x56]);
        let b = fake_jpeg(b"", &[0x99]);
        let mut stream = a.clone();
        stream.extend(&b);
        stream.extend(&a[..a.len() - 3]); // truncated third image
        let images = split_jpegs(&stream);
        assert_eq!(images, vec![&a[..], &b[..]]);
        assert!(split_jpegs(b"").is_empty());
        assert!(split_jpegs(&[0xFF, 0xD8, 0x00]).is_empty(), "corrupt");
    }

    #[test]
    fn base64_rfc4648_vectors() {
        for (input, want) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(input.as_bytes()), want);
        }
        assert_eq!(base64(&[0xFF, 0xD8, 0xFF]), "/9j/");
    }

    fn pcm(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn peak_meter_buckets_by_time() {
        // 1 s at 8 Hz into 4 buckets: 2 samples per bucket.
        let mut m = PeakMeter::new(8, 1.0, 4);
        m.feed(&pcm(&[100, -16384, 0, 0, 32767, 5, i16::MIN, 1]));
        assert_eq!(m.finish(), vec![0.5, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn peak_meter_handles_split_samples_and_overflow() {
        let bytes = pcm(&[0, 8192, 0, 0, 0, 0, 0, 0, 16384, 16384]);
        let mut m = PeakMeter::new(8, 1.0, 4);
        // Feed in odd-sized pieces, including an empty one.
        for piece in [&bytes[..3], &bytes[3..3], &bytes[3..8], &bytes[8..]] {
            m.feed(piece);
        }
        assert_eq!(m.finish(), vec![0.25, 0.0, 0.0, 0.0], "samples after the last bucket are ignored");
    }

    #[test]
    fn range_defaults_to_whole_file_and_is_clamped() {
        let info = MediaInfo {
            path: "a".into(),
            file_name: "a".into(),
            size_bytes: None,
            duration: 10.0,
            format_name: "matroska,webm".into(),
            bit_rate: None,
            video: None,
            audio: vec![],
            subtitle_count: 0,
            other_count: 0,
        };
        assert_eq!(resolve_range(&info, None, None).unwrap(), plan::Range::new(0.0, 10.0));
        assert_eq!(resolve_range(&info, Some(-1.0), Some(99.0)).unwrap(), plan::Range::new(0.0, 10.0));
        assert!(resolve_range(&info, Some(5.0), Some(5.0)).is_err());
    }
}
