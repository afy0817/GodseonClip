//! What Rust tells the user, as a code with arguments. Rust builds no user-facing sentences: the UI's
//! dictionaries (`src/lib/i18n/` — `msg` in `ko.ts` · `en.ts`) turn a [`Msg`] into a sentence in the
//! app's language, so switching the language also changes errors and notices already shown. Text with
//! no translation (an FFmpeg · ffprobe stderr line, an OS error) goes in as an argument (`detail`,
//! `reason`) or as [`Msg::Raw`]. The native quit dialog is the one exception ([`crate::settings::Lang`]).
//!
//! On the wire: `{"code": "fileNotFound", "path": "…"}` — the variant name in camelCase plus its
//! fields. `src/lib/ipc.ts` has the same union (`Msg`); a test below checks every variant is there.

use crate::command::VideoCodec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Msg {
    /// Text with no translation: an FFmpeg · ffprobe stderr line.
    Raw { text: String },

    // Files
    FileNotFound { path: Option<String> },
    /// The editor was given a folder (`name`: its name).
    CannotOpenFolder { name: String },
    IsFolder,
    FileUnreadable { detail: String },
    PermissionDenied,
    NotMedia,
    ImageFile,
    NoTracks,
    DurationUnknown,
    EmptyRange,
    NoVideoTrack,
    NoAudioTrack,
    /// ffprobe failed; `reason` is its last stderr line when there was one.
    ProbeFailed { reason: Option<String> },
    ProbeOutputUnreadable { detail: String },
    KeyframesFailed,
    NotProbed,
    PieceUnreadable,
    PiecePacketsMissing,

    // FFmpeg
    FfmpegNotReady,
    FfmpegTooOld { version: String, min: String },
    FfprobeMissing { path: String },
    /// `program` (a path) could not be started.
    RunFailed { program: String, detail: String },
    /// `-version` failed (`program`: "ffmpeg" · "ffprobe").
    VersionFailed { program: String, detail: Option<String> },
    EncoderListFailed { detail: String },
    EnteredPathUnusable { why: Box<Msg> },
    SavedPathUnusable { why: Box<Msg> },
    FfmpegLaunchFailed { detail: String },
    FfmpegRelaunchFailed { detail: String },
    FfmpegFailed,

    // Encoders
    EncoderTestTimedOut { secs: u64 },
    EncoderTestFailed,
    /// No settings for this encoder name yet (quality values, arguments).
    EncoderUnsupported { encoder: String },
    /// The encoder picked on the convert screen did not pass the test (`encoder`: its label).
    ChosenEncoderUnavailable { encoder: String },
    NoEncoder { codec: VideoCodec },
    /// A hardware encode failed and the item started again with `next` (labels); `reason`: the
    /// FFmpeg error line.
    RetriedWith { failed: String, next: String, reason: Option<String> },

    // Editor export
    PreciseNeedsVideo,
    LosslessKeepsCodec,
    CutNeedsVideo,
    CutNeedsKeyframes,
    NoRangesToRemove,
    RemovesEverything,
    NothingKept,
    NothingRemovableLossless,
    CutExtensionUnsupported { extension: String },
    /// Joining at this keyframe (seconds) would break the picture: it is not an IDR frame.
    OpenGopJoin { at: f64 },
    TempFolderFailed { detail: String },
    /// 화면 효과 need re-encoding (무손실 was asked with effects).
    EffectsNeedPrecise,
    /// The crop box is not inside the `width` × `height` (upright) picture.
    CropOutsidePicture { width: u32, height: u32 },
    CropTooSmall { min: u32 },
    /// 출력 크기 outside `min` … `max` pixels.
    OutputSizeInvalid { min: u32, max: u32 },
    PictureSizeUnknown,
    /// A 모자이크 · 블러 box is not inside the `width` × `height` (upright) picture.
    MaskOutsidePicture { width: u32, height: u32 },
    MaskTooSmall { min: u32 },
    /// A mask's end is not after its start.
    MaskTimeInvalid,
    TooManyMasks { max: usize },
    /// A PiP box is not inside the `width` × `height` (upright) picture.
    OverlayOutsidePicture { width: u32, height: u32 },
    OverlayTooSmall { min: u32 },
    /// A PiP's end is not after its start, or its start in the put-on video is negative.
    OverlayTimeInvalid,
    TooManyOverlays { max: usize },
    /// The file put over the picture (`name`) cannot be read.
    OverlayUnreadable { name: String, why: Box<Msg> },
    ConcatListFailed { detail: String },
    RenameFailed { detail: String },

    // Convert
    ConvertNeedsVideo,
    ConvertCodecUnsupported,
    /// `codec` ("H.265" · "AV1") cannot go in `container` ("MP4" · "WEBM" …).
    CodecNotInContainer { codec: String, container: String },
    WebmNeedsOpus,
    OutputFolderNotFound { path: String },
    SuffixTooLong { max: usize },
    /// `ch` None: a control character.
    SuffixBadChar { ch: Option<String> },
    SuffixPart,
    FolderUnreadable { path: String, detail: String },
    NoVideoFiles { name: String },
    TooManyFiles { limit: usize },

    // Estimate
    EstimateNeedsDuration,
    EstimateFolderFailed { detail: String },
    SampleFailed { why: Option<Box<Msg>> },
    SampleEmpty,

    // Preview
    ProxyNeedsVideo,
    ProxyNeedsX264,
    CacheFolderFailed { detail: String },
    ThumbnailsTimedOut,
    ThumbnailsFailed,
    ThumbnailsNoKeyframes,
    WaveformTimedOut,
    WaveformFailed,

    // Queue
    QueueClosed,
    QueueItemNotFound,
    ItemFinished,
    CancelRunningFirst,
    OnlyWaitingMoves,
    OnlyStoppedRestart,

    // Settings
    SettingsUnreadable { path: String, detail: String },
    SettingsCorrupt { path: String, detail: String },
    SettingsNotSaved { path: String, detail: String },
    NoConfigFolder,
    ThisRunOnly,
    ConcurrencyNotSaved,
}

impl Msg {
    pub fn raw(text: impl Into<String>) -> Msg {
        Msg::Raw { text: text.into() }
    }

    /// An FFmpeg stderr line when there is one, else `fallback`.
    pub fn line_or(line: Option<String>, fallback: Msg) -> Msg {
        line.map(Msg::raw).unwrap_or(fallback)
    }

    pub fn launch(e: impl std::fmt::Display) -> Msg {
        Msg::FfmpegLaunchFailed { detail: e.to_string() }
    }

    pub fn file_not_found(path: impl std::fmt::Display) -> Msg {
        Msg::FileNotFound { path: Some(path.to_string()) }
    }
}

/// For logs and test failures (not for the user): the JSON form.
impl std::fmt::Display for Msg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&serde_json::to_string(self).unwrap_or_else(|_| format!("{self:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_msg_is_its_code_and_fields_in_camel_case() {
        let j = |m: &Msg| serde_json::to_value(m).unwrap();
        assert_eq!(j(&Msg::IsFolder), serde_json::json!({ "code": "isFolder" }));
        assert_eq!(
            j(&Msg::file_not_found("/Users/사용자/내 영상/테스트 클립 01.mp4")),
            serde_json::json!({ "code": "fileNotFound", "path": "/Users/사용자/내 영상/테스트 클립 01.mp4" })
        );
        assert_eq!(j(&Msg::FileNotFound { path: None }), serde_json::json!({ "code": "fileNotFound", "path": null }));
        assert_eq!(
            j(&Msg::EnteredPathUnusable {
                why: Box::new(Msg::FfmpegTooOld { version: "5.1.2".into(), min: "6.0".into() })
            }),
            serde_json::json!({ "code": "enteredPathUnusable", "why": { "code": "ffmpegTooOld", "version": "5.1.2", "min": "6.0" } })
        );
        assert_eq!(
            j(&Msg::NoEncoder { codec: VideoCodec::Av1 }),
            serde_json::json!({ "code": "noEncoder", "codec": "av1" })
        );
        assert_eq!(
            j(&Msg::SuffixBadChar { ch: None }),
            serde_json::json!({ "code": "suffixBadChar", "ch": null })
        );
        let back: Msg = serde_json::from_value(j(&Msg::SampleFailed { why: Some(Box::new(Msg::raw("x"))) })).unwrap();
        assert_eq!(back, Msg::SampleFailed { why: Some(Box::new(Msg::raw("x"))) });
    }

    /// Variant names of `Msg`, read from this file (the enum body above).
    fn variants() -> Vec<String> {
        // A Windows checkout may have CRLF line ends (git autocrlf): read it as LF.
        let src = include_str!("msg.rs").replace("\r\n", "\n");
        let body = src.split("pub enum Msg {").nth(1).unwrap().split("\n}\n").next().unwrap();
        body.lines()
            .map(str::trim)
            .filter(|l| l.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
            .map(|l| l.split(|c: char| !c.is_ascii_alphanumeric()).next().unwrap().to_string())
            .collect()
    }

    fn camel(name: &str) -> String {
        let mut c = name.chars();
        c.next().map(|f| f.to_ascii_lowercase().to_string() + c.as_str()).unwrap_or_default()
    }

    /// Every code Rust can send is in the TS union and in both dictionaries (`msg` section). The
    /// TS compiler checks the dictionaries cover the union; this checks the union covers Rust.
    #[test]
    fn every_code_is_in_ipc_ts_and_both_dictionaries() {
        let names = variants();
        assert!(names.len() > 80, "{names:?}");
        let ipc = include_str!("../../src/lib/ipc.ts");
        let ko = include_str!("../../src/lib/i18n/ko.ts");
        let en = include_str!("../../src/lib/i18n/en.ts");
        for n in &names {
            let code = camel(n);
            assert!(ipc.contains(&format!("code: \"{code}\"")), "ipc.ts Msg has no {code}");
            assert!(ko.contains(&format!("  {code}: ")), "ko.ts msg has no {code}");
            assert!(en.contains(&format!("  {code}: ")), "en.ts msg has no {code}");
        }
    }

    /// Rust sends codes, not sentences: no Korean string outside the tests (and comments), but the
    /// native quit dialog's words (`queue::quit_dialog`).
    #[test]
    fn no_korean_sentences_in_rust_code() {
        let hangul = |c: char| ('\u{AC00}'..='\u{D7A3}').contains(&c) || ('\u{3131}'..='\u{318E}').contains(&c);
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let code = text.split("#[cfg(test)]").next().unwrap();
            let mut in_quit_dialog = false;
            for (n, line) in code.lines().enumerate() {
                if line.starts_with("pub fn quit_dialog") {
                    in_quit_dialog = true;
                } else if in_quit_dialog && line == "}" {
                    in_quit_dialog = false;
                }
                let without_comment = line.split("//").next().unwrap();
                if !in_quit_dialog && without_comment.contains('"') && without_comment.chars().any(hangul) {
                    found.push(format!("{}:{}: {}", path.file_name().unwrap().to_string_lossy(), n + 1, line.trim()));
                }
            }
        }
        assert!(found.is_empty(), "{found:#?}");
    }
}
