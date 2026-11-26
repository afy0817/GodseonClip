//! Checks which H.265 / AV1 encoders actually work on this machine.
//!
//! Being listed in `ffmpeg -encoders` only means the build supports an encoder; a missing
//! driver or GPU still makes it fail. Each candidate encodes three frames of a test pattern.

use crate::command::{exports_with, VideoCodec};
use crate::ffmpeg::FfmpegInfo;
use crate::msg::Msg;
use crate::util::{last_line, run_capture};
use serde::Serialize;
use std::path::Path;
use std::time::{Duration, Instant};

/// (encoder, codec, hardware). Order inside a codec is the preference order.
const CANDIDATES: &[(&str, &str, bool)] = &[
    ("hevc_videotoolbox", "hevc", true),
    ("hevc_nvenc", "hevc", true),
    ("hevc_qsv", "hevc", true),
    ("hevc_amf", "hevc", true),
    ("hevc_vaapi", "hevc", true),
    ("hevc_mf", "hevc", true),
    ("av1_nvenc", "av1", true),
    ("av1_qsv", "av1", true),
    ("av1_amf", "av1", true),
    ("av1_vaapi", "av1", true),
    ("av1_mf", "av1", true),
    ("libx265", "hevc", false),
    ("libsvtav1", "av1", false),
    ("libx264", "h264", false),
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderTest {
    pub name: String,
    pub codec: String,
    pub hardware: bool,
    pub ok: bool,
    pub millis: u64,
    pub error: Option<Msg>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderReport {
    pub tests: Vec<EncoderTest>,
    pub preferred_hevc: Option<String>,
    pub preferred_av1: Option<String>,
}

/// Arguments for a three-frame test encode with `encoder`.
pub fn test_args(encoder: &str) -> Vec<String> {
    let mut a: Vec<&str> = vec!["-hide_banner", "-nostdin", "-loglevel", "error"];
    let vaapi = encoder.ends_with("_vaapi");
    if vaapi {
        a.extend(["-vaapi_device", "/dev/dri/renderD128"]);
    }
    a.extend(["-f", "lavfi", "-i", "color=c=black:s=256x256:r=30:d=0.2"]);
    if vaapi {
        a.extend(["-vf", "format=nv12,hwupload"]);
    } else if encoder.ends_with("_qsv") || encoder.ends_with("_amf") || encoder.ends_with("_mf") {
        a.extend(["-pix_fmt", "nv12"]);
    } else {
        a.extend(["-pix_fmt", "yuv420p"]);
    }
    a.extend(["-frames:v", "3", "-c:v", encoder, "-f", "null", "-"]);
    a.into_iter().map(String::from).collect()
}

pub fn run(info: &FfmpegInfo) -> EncoderReport {
    let ffmpeg = Path::new(&info.ffmpeg_path);
    let tests: Vec<EncoderTest> = CANDIDATES
        .iter()
        .filter(|(name, _, _)| info.video_encoders.iter().any(|e| e == name))
        .map(|(name, codec, hardware)| {
            let started = Instant::now();
            let result = run_capture(ffmpeg, &test_args(name), Duration::from_secs(20));
            let millis = started.elapsed().as_millis() as u64;
            let (ok, error) = match result {
                Ok(c) if c.success => (true, None),
                Ok(c) if c.timed_out => (false, Some(Msg::EncoderTestTimedOut { secs: 20 })),
                Ok(c) => (false, Some(Msg::line_or(last_line(&c.stderr, 240), Msg::EncoderTestFailed))),
                Err(e) => (false, Some(Msg::launch(e))),
            };
            EncoderTest {
                name: name.to_string(),
                codec: codec.to_string(),
                hardware: *hardware,
                ok,
                millis,
                error,
            }
        })
        .collect();
    let preferred_hevc = preferred(&tests, "hevc");
    let preferred_av1 = preferred(&tests, "av1");
    EncoderReport { tests, preferred_hevc, preferred_av1 }
}

/// First working hardware encoder for the codec, else the first working software one.
pub fn preferred(tests: &[EncoderTest], codec: &str) -> Option<String> {
    let working = || tests.iter().filter(move |t| t.ok && t.codec == codec);
    working()
        .find(|t| t.hardware)
        .or_else(|| working().find(|t| !t.hardware))
        .map(|t| t.name.clone())
}

/// Short name for the UI ("인코더 자동 — VideoToolbox").
pub fn label(encoder: &str) -> String {
    match encoder {
        "hevc_videotoolbox" => "VideoToolbox",
        "hevc_nvenc" | "av1_nvenc" => "NVENC",
        "hevc_qsv" | "av1_qsv" => "Quick Sync",
        "hevc_amf" | "av1_amf" => "AMF",
        "hevc_vaapi" | "av1_vaapi" => "VAAPI",
        "hevc_mf" | "av1_mf" => "Media Foundation",
        "libsvtav1" => "SVT-AV1",
        other => other,
    }
    .to_string()
}

/// The encoder a precise export uses for one codec, and the software encoder it falls back to
/// when a hardware one fails mid-job.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderChoice {
    pub name: String,
    pub label: String,
    pub hardware: bool,
    pub fallback: Option<String>,
    pub fallback_label: Option<String>,
}

/// One choice per output codec; None when no encoder for it works with this FFmpeg.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExportEncoders {
    pub h264: Option<EncoderChoice>,
    pub hevc: Option<EncoderChoice>,
    pub av1: Option<EncoderChoice>,
}

impl ExportEncoders {
    pub fn for_codec(&self, codec: VideoCodec) -> Option<&EncoderChoice> {
        match codec {
            VideoCodec::H264 => self.h264.as_ref(),
            VideoCodec::Hevc => self.hevc.as_ref(),
            VideoCodec::Av1 => self.av1.as_ref(),
        }
    }
}

/// From the three-frame tests: the first working hardware encoder the export has settings for
/// (`command::exports_with`), with the first working software one as its fallback; else that
/// software encoder alone.
pub fn export_choices(report: &EncoderReport) -> ExportEncoders {
    let choose = |codec: &str| {
        let working = || report.tests.iter().filter(move |t| t.ok && t.codec == codec && exports_with(&t.name));
        let software = working().find(|t| !t.hardware);
        let pick = working().find(|t| t.hardware).or(software)?;
        let fallback = software.filter(|s| s.name != pick.name);
        Some(EncoderChoice {
            name: pick.name.clone(),
            label: label(&pick.name),
            hardware: pick.hardware,
            fallback: fallback.map(|f| f.name.clone()),
            fallback_label: fallback.map(|f| label(&f.name)),
        })
    };
    ExportEncoders { h264: choose("h264"), hevc: choose("hevc"), av1: choose("av1") }
}

/// The 일괄 변환 screen's 인코더 list: "자동" (what a precise export picks) and every working
/// encoder for H.265 and AV1 that the export has settings for, in preference order.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EncoderOptions {
    pub auto: ExportEncoders,
    pub hevc: Vec<EncoderChoice>,
    pub av1: Vec<EncoderChoice>,
}

fn codec_name(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "h264",
        VideoCodec::Hevc => "hevc",
        VideoCodec::Av1 => "av1",
    }
}

/// One working encoder as a choice: a hardware one falls back to the first working software
/// encoder of its codec (as "자동" does); a software one runs alone.
fn chosen(report: &EncoderReport, codec: VideoCodec, name: &str) -> Option<EncoderChoice> {
    let codec = codec_name(codec);
    let working = || report.tests.iter().filter(move |t| t.ok && t.codec == codec && exports_with(&t.name));
    let pick = working().find(|t| t.name == name)?;
    let fallback = if pick.hardware { working().find(|t| !t.hardware) } else { None };
    Some(EncoderChoice {
        name: pick.name.clone(),
        label: label(&pick.name),
        hardware: pick.hardware,
        fallback: fallback.map(|f| f.name.clone()),
        fallback_label: fallback.map(|f| label(&f.name)),
    })
}

pub fn encoder_options(report: &EncoderReport) -> EncoderOptions {
    let all = |codec: VideoCodec| -> Vec<EncoderChoice> {
        let name = codec_name(codec);
        let mut list: Vec<EncoderChoice> = report
            .tests
            .iter()
            .filter(|t| t.ok && t.codec == name && exports_with(&t.name))
            .filter_map(|t| chosen(report, codec, &t.name))
            .collect();
        // Hardware first, as "자동" prefers it (the test order lists software last already).
        list.sort_by_key(|c| !c.hardware);
        list
    };
    EncoderOptions { auto: export_choices(report), hevc: all(VideoCodec::Hevc), av1: all(VideoCodec::Av1) }
}

/// The encoder a conversion to `codec` runs: "자동" (None) or the one the user picked, if it still
/// works on this machine.
pub fn pick(report: &EncoderReport, codec: VideoCodec, wanted: Option<&str>) -> Result<EncoderChoice, Msg> {
    match wanted {
        None => export_choices(report).for_codec(codec).cloned().ok_or(Msg::NoEncoder { codec }),
        Some(name) => {
            chosen(report, codec, name).ok_or_else(|| Msg::ChosenEncoderUnavailable { encoder: label(name) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_prefers_hardware_with_software_fallback() {
        let report = EncoderReport {
            tests: vec![
                t("hevc_videotoolbox", "hevc", true, true),
                t("av1_nvenc", "av1", true, true),
                t("libx265", "hevc", false, true),
                t("libsvtav1", "av1", false, true),
                t("libx264", "h264", false, true),
            ],
            preferred_hevc: None,
            preferred_av1: None,
        };
        let c = export_choices(&report);
        let hevc = c.hevc.unwrap();
        assert_eq!((hevc.name.as_str(), hevc.label.as_str(), hevc.hardware), ("hevc_videotoolbox", "VideoToolbox", true));
        assert_eq!((hevc.fallback.as_deref(), hevc.fallback_label.as_deref()), (Some("libx265"), Some("libx265")));
        // AV1 hardware has no export settings yet: SVT-AV1, nothing to fall back to.
        let av1 = c.av1.unwrap();
        assert_eq!((av1.name.as_str(), av1.label.as_str(), av1.fallback), ("libsvtav1", "SVT-AV1", None));
        assert_eq!(c.h264.unwrap().name, "libx264");
    }

    #[test]
    fn export_skips_failed_and_missing_encoders() {
        let report = EncoderReport {
            tests: vec![t("hevc_nvenc", "hevc", true, false), t("hevc_qsv", "hevc", true, true), t("libx264", "h264", false, false)],
            preferred_hevc: None,
            preferred_av1: None,
        };
        let c = export_choices(&report);
        let hevc = c.hevc.unwrap();
        assert_eq!((hevc.name.as_str(), hevc.fallback), ("hevc_qsv", None), "no libx265: hardware alone");
        assert_eq!((c.h264, c.av1), (None, None));
    }

    #[test]
    fn convert_offers_auto_and_every_working_encoder() {
        let report = EncoderReport {
            tests: vec![
                t("hevc_videotoolbox", "hevc", true, true),
                t("hevc_nvenc", "hevc", true, false),
                t("hevc_vaapi", "hevc", true, true),
                t("av1_nvenc", "av1", true, true),
                t("libx265", "hevc", false, true),
                t("libsvtav1", "av1", false, true),
                t("libx264", "h264", false, true),
            ],
            preferred_hevc: None,
            preferred_av1: None,
        };
        let o = encoder_options(&report);
        let names = |l: &[EncoderChoice]| l.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        // Failed NVENC and VAAPI / AV1 hardware (no export settings yet) are not offered.
        assert_eq!(names(&o.hevc), ["hevc_videotoolbox", "libx265"]);
        assert_eq!(names(&o.av1), ["libsvtav1"]);
        assert_eq!(o.auto, export_choices(&report));
        assert_eq!((o.hevc[0].fallback.as_deref(), o.hevc[1].fallback.as_deref()), (Some("libx265"), None), "software runs alone");

        assert_eq!(pick(&report, VideoCodec::Hevc, None).unwrap().name, "hevc_videotoolbox");
        let x265 = pick(&report, VideoCodec::Hevc, Some("libx265")).unwrap();
        assert_eq!((x265.name.as_str(), x265.hardware, x265.fallback), ("libx265", false, None));
        assert_eq!(
            pick(&report, VideoCodec::Hevc, Some("hevc_nvenc")).unwrap_err(),
            Msg::ChosenEncoderUnavailable { encoder: "NVENC".into() },
            "failed the test"
        );
        assert!(pick(&report, VideoCodec::Av1, Some("libx265")).is_err(), "another codec's encoder");
        let none = EncoderReport { tests: vec![], preferred_hevc: None, preferred_av1: None };
        assert_eq!(pick(&none, VideoCodec::Av1, None).unwrap_err(), Msg::NoEncoder { codec: VideoCodec::Av1 });
    }

    fn t(name: &str, codec: &str, hardware: bool, ok: bool) -> EncoderTest {
        EncoderTest { name: name.into(), codec: codec.into(), hardware, ok, millis: 1, error: None }
    }

    #[test]
    fn prefers_working_hardware_then_software() {
        let tests = vec![
            t("hevc_nvenc", "hevc", true, false),
            t("hevc_qsv", "hevc", true, true),
            t("libx265", "hevc", false, true),
            t("libsvtav1", "av1", false, true),
        ];
        assert_eq!(preferred(&tests, "hevc").as_deref(), Some("hevc_qsv"));
        assert_eq!(preferred(&tests, "av1").as_deref(), Some("libsvtav1"));
        assert_eq!(preferred(&tests, "h264"), None);
    }

    #[test]
    fn vaapi_test_uploads_frames() {
        let a = test_args("hevc_vaapi").join(" ");
        assert!(a.contains("-vaapi_device /dev/dri/renderD128"));
        assert!(a.contains("format=nv12,hwupload"));
        let b = test_args("libx265").join(" ");
        assert!(b.contains("-pix_fmt yuv420p") && b.ends_with("-c:v libx265 -f null -"));
    }
}
