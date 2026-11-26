//! Finds the user's own FFmpeg install and checks what it can do.
//!
//! Godseon does not bundle FFmpeg. Search order ([`locate_with`]):
//! 1. a path the user entered just now in settings (file or folder)
//! 2. the path saved in the settings file (`settings.rs`) — only paths that were confirmed usable get saved
//! 3. every folder on PATH
//! 4. well-known install folders (Homebrew, WinGet links, Scoop, Chocolatey, /usr/bin, snap)
//!
//! A user path that is missing or unusable (no ffprobe, does not run, older than 6.0) is skipped
//! with a notice and the search goes on. Step 4 matters on macOS: an app opened from Finder does
//! not inherit the shell's PATH, so /opt/homebrew/bin is invisible unless we look there ourselves.

use crate::msg::Msg;
use crate::util::{last_line, run_capture};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// libavcodec major version shipped with FFmpeg 6.0. Checking the library version instead of
/// the release string also works for git builds that report "N-12345-g…" as their version.
pub const MIN_LIBAVCODEC_MAJOR: u32 = 60;
pub const MIN_VERSION_LABEL: &str = "6.0";

const HEVC_HARDWARE: &[&str] = &[
    "hevc_videotoolbox",
    "hevc_nvenc",
    "hevc_qsv",
    "hevc_amf",
    "hevc_vaapi",
    "hevc_mf",
];
const AV1_HARDWARE: &[&str] = &["av1_nvenc", "av1_qsv", "av1_amf", "av1_vaapi", "av1_mf"];

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Features {
    pub libx264: bool,
    pub libx265: bool,
    pub libsvtav1: bool,
    pub hevc_hardware: Vec<String>,
    pub av1_hardware: Vec<String>,
    /// Opus audio encoder: 일괄 변환 to WebM needs it.
    pub libopus: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FfmpegInfo {
    pub ffmpeg_path: String,
    pub ffprobe_path: String,
    /// Raw version token, e.g. "7.1.1", "n7.1-12-gabc", "N-118000-g…", "6.1.1-3ubuntu5".
    pub version: String,
    /// Release number when one can be read from the token ("7.1.1").
    pub release: Option<String>,
    pub libavcodec_major: Option<u32>,
    pub meets_minimum: bool,
    pub source: String,
    pub video_encoders: Vec<String>,
    pub features: Features,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocateResult {
    pub found: bool,
    pub info: Option<FfmpegInfo>,
    pub searched: Vec<String>,
    /// Why a found FFmpeg cannot be used (too old, no ffprobe, does not run).
    pub problem: Option<Msg>,
    pub min_version: String,
    /// The ffmpeg path in the settings file after this call; None: automatic search at start.
    pub saved_path: Option<String>,
    /// For the user: why their own path was skipped, or that it could not be saved.
    pub notices: Vec<Msg>,
}

pub fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

/// Folders searched after a user-saved path, in order, without duplicates.
pub fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if cfg!(target_os = "macos") {
        dirs.push("/opt/homebrew/bin".into());
        dirs.push("/usr/local/bin".into());
        dirs.push("/opt/local/bin".into());
    } else if cfg!(windows) {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Microsoft").join("WinGet").join("Links"));
        }
        if let Some(home) = std::env::var_os("USERPROFILE") {
            dirs.push(PathBuf::from(home).join("scoop").join("shims"));
        }
        if let Some(data) = std::env::var_os("ProgramData") {
            dirs.push(PathBuf::from(data).join("chocolatey").join("bin"));
        }
    } else {
        dirs.push("/usr/bin".into());
        dirs.push("/usr/local/bin".into());
        dirs.push("/snap/bin".into());
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| !d.as_os_str().is_empty() && seen.insert(d.clone()));
    dirs
}

/// Turns a user-supplied path (the ffmpeg file or its folder) into the ffmpeg path.
fn resolve_custom(custom: &str) -> PathBuf {
    let p = PathBuf::from(custom.trim());
    if p.is_dir() { p.join(exe_name("ffmpeg")) } else { p }
}

fn not_found(searched: Vec<String>, problem: Option<Msg>) -> LocateResult {
    LocateResult {
        found: false,
        info: None,
        searched,
        problem,
        min_version: MIN_VERSION_LABEL.to_string(),
        saved_path: None,
        notices: Vec::new(),
    }
}

fn found(info: FfmpegInfo, searched: Vec<String>) -> LocateResult {
    let problem = (!info.meets_minimum).then(|| too_old(&info));
    LocateResult {
        found: true,
        info: Some(info),
        searched,
        problem,
        min_version: MIN_VERSION_LABEL.to_string(),
        saved_path: None,
        notices: Vec::new(),
    }
}

fn too_old(info: &FfmpegInfo) -> Msg {
    Msg::FfmpegTooOld {
        version: info.release.as_deref().unwrap_or(&info.version).to_string(),
        min: MIN_VERSION_LABEL.to_string(),
    }
}

/// ffprobe next to ffmpeg, else the first one in the search folders.
fn find_ffprobe(ffmpeg: &Path, dirs: &[PathBuf]) -> Option<PathBuf> {
    let sibling = ffmpeg.with_file_name(exe_name("ffprobe"));
    if sibling.is_file() {
        return Some(sibling);
    }
    dirs.iter().map(|d| d.join(exe_name("ffprobe"))).find(|p| p.is_file())
}

/// The first ffmpeg file among `custom` and the search folders, used as it is even when it is too
/// old. The app goes through [`locate_with`]; the integration tests use this with `GODSEON_FFMPEG`.
pub fn locate(custom: Option<&str>) -> LocateResult {
    let mut searched = Vec::new();
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(c) = custom.filter(|c| !c.trim().is_empty()) {
        let p = resolve_custom(c);
        searched.push(p.display().to_string());
        candidates.push(p);
    }
    let dirs = candidate_dirs();
    for d in &dirs {
        searched.push(d.display().to_string());
        candidates.push(d.join(exe_name("ffmpeg")));
    }

    let Some(ffmpeg) = candidates.into_iter().find(|p| p.is_file()) else {
        return not_found(searched, None);
    };
    let Some(ffprobe) = find_ffprobe(&ffmpeg, &dirs) else {
        return not_found(searched, Some(Msg::FfprobeMissing { path: ffmpeg.display().to_string() }));
    };
    match inspect(&ffmpeg, &ffprobe) {
        Ok(info) => found(info, searched),
        Err(e) => not_found(searched, Some(e)),
    }
}

/// Checks one path the user chose (the ffmpeg file or its folder). Ok only when it can be used:
/// it exists, has an ffprobe, runs, and is 6.0 or newer.
pub fn check_path(custom: &str) -> Result<FfmpegInfo, Msg> {
    let ffmpeg = resolve_custom(custom);
    if !ffmpeg.is_file() {
        return Err(Msg::file_not_found(ffmpeg.display()));
    }
    let ffprobe = find_ffprobe(&ffmpeg, &candidate_dirs())
        .ok_or_else(|| Msg::FfprobeMissing { path: ffmpeg.display().to_string() })?;
    let info = inspect(&ffmpeg, &ffprobe)?;
    if !info.meets_minimum {
        return Err(too_old(&info));
    }
    Ok(info)
}

/// Locates FFmpeg with the user's own paths first: `entered` (typed or picked just now), then
/// `saved` (from the settings file). A path that cannot be used is skipped with a notice and the
/// search goes on, ending in `auto` (the PATH and install-folder search).
///
/// `check` tests one path ([`check_path`] in the app). Returns the result and, when `entered`
/// worked, the ffmpeg path to save.
pub fn locate_with(
    entered: Option<&str>,
    saved: Option<&str>,
    check: impl Fn(&str) -> Result<FfmpegInfo, Msg>,
    auto: impl FnOnce() -> LocateResult,
) -> (LocateResult, Option<String>) {
    let entered = entered.map(str::trim).filter(|p| !p.is_empty());
    let saved = saved.map(str::trim).filter(|p| !p.is_empty() && Some(*p) != entered);
    let mut searched = Vec::new();
    let mut notices = Vec::new();

    if let Some(p) = entered {
        searched.push(resolve_custom(p).display().to_string());
        match check(p) {
            Ok(info) => {
                let save = info.ffmpeg_path.clone();
                return (found(info, searched), Some(save));
            }
            Err(why) => notices.push(Msg::EnteredPathUnusable { why: Box::new(why) }),
        }
    }
    if let Some(p) = saved {
        searched.push(resolve_custom(p).display().to_string());
        match check(p) {
            Ok(info) => {
                let mut r = found(info, searched);
                r.notices = notices;
                return (r, None);
            }
            Err(why) => notices.push(Msg::SavedPathUnusable { why: Box::new(why) }),
        }
    }
    let mut r = auto();
    searched.append(&mut r.searched);
    r.searched = searched;
    r.notices = notices;
    (r, None)
}

/// Runs `-version` and `-encoders` on a found binary.
pub fn inspect(ffmpeg: &Path, ffprobe: &Path) -> Result<FfmpegInfo, Msg> {
    let timeout = Duration::from_secs(15);
    let run_failed = |p: &Path, e: std::io::Error| Msg::RunFailed { program: p.display().to_string(), detail: e.to_string() };
    let ver = run_capture(ffmpeg, &["-version"], timeout).map_err(|e| run_failed(ffmpeg, e))?;
    if !ver.success {
        return Err(Msg::VersionFailed { program: "ffmpeg".into(), detail: last_line(&ver.stderr, 200) });
    }
    let probe_ver = run_capture(ffprobe, &["-version"], timeout).map_err(|e| run_failed(ffprobe, e))?;
    if !probe_ver.success {
        return Err(Msg::VersionFailed { program: "ffprobe".into(), detail: last_line(&probe_ver.stderr, 200) });
    }

    let (version, libavcodec_major) = parse_version(&ver.stdout);
    let release = release_number(&version);
    let meets_minimum = match (libavcodec_major, &release) {
        (Some(major), _) => major >= MIN_LIBAVCODEC_MAJOR,
        (None, Some(r)) => r.split('.').next().and_then(|m| m.parse::<u32>().ok()).unwrap_or(0) >= 6,
        (None, None) => false,
    };

    let enc = run_capture(ffmpeg, &["-hide_banner", "-encoders"], timeout)
        .map_err(|e| Msg::EncoderListFailed { detail: e.to_string() })?;
    let video_encoders = parse_video_encoders(&enc.stdout);
    let mut features = features_from(&video_encoders);
    features.libopus = parse_encoders(&enc.stdout, 'A').iter().any(|e| e == "libopus");

    let canonical = std::fs::canonicalize(ffmpeg).unwrap_or_else(|_| ffmpeg.to_path_buf());
    Ok(FfmpegInfo {
        ffmpeg_path: ffmpeg.display().to_string(),
        ffprobe_path: ffprobe.display().to_string(),
        version,
        release,
        libavcodec_major,
        meets_minimum,
        source: guess_source(&canonical),
        video_encoders,
        features,
    })
}

/// Reads the version token and the libavcodec major version from `ffmpeg -version`.
pub fn parse_version(text: &str) -> (String, Option<u32>) {
    let mut version = String::from("unknown");
    let mut lavc = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("ffmpeg version ") {
            if let Some(tok) = rest.split_whitespace().next() {
                version = tok.to_string();
            }
        } else if let Some(rest) = line.strip_prefix("libavcodec") {
            let digits: String = rest.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
            lavc = digits.parse().ok();
        }
    }
    (version, lavc)
}

/// "7.1.1" from "7.1.1", "n7.1-12-gabc", "6.1.1-3ubuntu5", "7.0.2-tessus"; None for git "N-…" builds.
pub fn release_number(token: &str) -> Option<String> {
    let t = token.strip_prefix('n').unwrap_or(token);
    if !t.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let s: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let s = s.trim_end_matches('.').to_string();
    (!s.is_empty()).then_some(s)
}

/// Video encoder names from `ffmpeg -encoders` (lines after the "------" separator whose flags start with V).
pub fn parse_video_encoders(text: &str) -> Vec<String> {
    parse_encoders(text, 'V')
}

/// Encoder names of one kind (`V` video, `A` audio, `S` subtitle) from `ffmpeg -encoders`.
pub fn parse_encoders(text: &str, kind: char) -> Vec<String> {
    let mut in_list = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !in_list {
            if trimmed.starts_with("------") {
                in_list = true;
            }
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        if let (Some(flags), Some(name)) = (parts.next(), parts.next()) {
            if flags.starts_with(kind) {
                out.push(name.to_string());
            }
        }
    }
    out
}

pub fn features_from(encoders: &[String]) -> Features {
    let has = |n: &str| encoders.iter().any(|e| e == n);
    let pick = |list: &[&str]| list.iter().filter(|n| has(n)).map(|n| n.to_string()).collect();
    Features {
        libx264: has("libx264"),
        libx265: has("libx265"),
        libsvtav1: has("libsvtav1"),
        hevc_hardware: pick(HEVC_HARDWARE),
        av1_hardware: pick(AV1_HARDWARE),
        libopus: false,
    }
}

/// Best guess at how the user installed FFmpeg, shown in the UI: a package manager's name, or
/// "system" (the OS's package, `/usr/bin`) · "manual" (anything else) for the UI to word.
pub fn guess_source(path: &Path) -> String {
    let p = path.display().to_string().replace('\\', "/").to_lowercase();
    let label = if p.contains("/opt/homebrew/") || p.contains("/cellar/") || p.contains("/usr/local/opt/") {
        "Homebrew"
    } else if p.contains("/opt/local/") {
        "MacPorts"
    } else if p.contains("/winget/") {
        "winget"
    } else if p.contains("/scoop/") {
        "Scoop"
    } else if p.contains("/chocolatey/") {
        "Chocolatey"
    } else if p.contains("/snap/") {
        "snap"
    } else if p.starts_with("/usr/bin/") {
        "system"
    } else {
        "manual"
    };
    label.to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const VERSION_BREW: &str = "ffmpeg version 7.1.1 Copyright (c) 2000-2025 the FFmpeg developers
built with Apple clang version 16.0.0
configuration: --prefix=/opt/homebrew/Cellar/ffmpeg/7.1.1_1 --enable-libx265 --enable-libsvtav1
libavutil      59. 39.100 / 59. 39.100
libavcodec     61. 19.101 / 61. 19.101
libavformat    61.  7.100 / 61.  7.100";

    const VERSION_UBUNTU_2204: &str = "ffmpeg version 4.4.2-0ubuntu0.22.04.1 Copyright (c) 2000-2021 the FFmpeg developers
libavutil      56. 70.100 / 56. 70.100
libavcodec     58.134.100 / 58.134.100";

    const VERSION_GIT: &str = "ffmpeg version N-118000-g1234abcd-20250101 Copyright (c) 2000-2025 the FFmpeg developers
libavcodec     62.  3.100 / 62.  3.100";

    #[test]
    fn parses_release_and_lavc() {
        assert_eq!(parse_version(VERSION_BREW), ("7.1.1".into(), Some(61)));
        assert_eq!(parse_version(VERSION_UBUNTU_2204).1, Some(58));
        assert_eq!(parse_version(VERSION_GIT), ("N-118000-g1234abcd-20250101".into(), Some(62)));
        assert_eq!(parse_version("garbage"), ("unknown".into(), None));
    }

    #[test]
    fn release_number_handles_vendor_suffixes() {
        assert_eq!(release_number("7.1.1").as_deref(), Some("7.1.1"));
        assert_eq!(release_number("n7.1-12-gabc").as_deref(), Some("7.1"));
        assert_eq!(release_number("6.1.1-3ubuntu5").as_deref(), Some("6.1.1"));
        assert_eq!(release_number("4.4.2-0ubuntu0.22.04.1").as_deref(), Some("4.4.2"));
        assert_eq!(release_number("N-118000-g1234abcd"), None);
    }

    #[test]
    fn minimum_is_libavcodec_60() {
        assert!(parse_version(VERSION_BREW).1.unwrap() >= MIN_LIBAVCODEC_MAJOR);
        assert!(parse_version(VERSION_UBUNTU_2204).1.unwrap() < MIN_LIBAVCODEC_MAJOR);
        assert!(parse_version(VERSION_GIT).1.unwrap() >= MIN_LIBAVCODEC_MAJOR);
    }

    const ENCODERS: &str = "Encoders:
 V..... = Video
 A..... = Audio
 ------
 V....D libx264              libx264 H.264 / AVC / MPEG-4 AVC (codec h264)
 V....D h264_videotoolbox    VideoToolbox H.264 Encoder (codec h264)
 V....D libx265              libx265 H.265 / HEVC (codec hevc)
 V....D hevc_videotoolbox    VideoToolbox H.265 Encoder (codec hevc)
 V..... libsvtav1            SVT-AV1(Scalable Video Technology for AV1) encoder (codec av1)
 A....D aac                  AAC (Advanced Audio Coding)
 A....D libopus              libopus Opus (codec opus)
 S..... mov_text             3GPP Timed Text subtitle";

    #[test]
    fn parses_only_video_encoders() {
        let v = parse_video_encoders(ENCODERS);
        assert_eq!(
            v,
            vec!["libx264", "h264_videotoolbox", "libx265", "hevc_videotoolbox", "libsvtav1"]
        );
        // The legend above the separator must not be read as encoders.
        assert!(!v.iter().any(|e| e == "="));
        assert_eq!(parse_encoders(ENCODERS, 'A'), vec!["aac", "libopus"]);
    }

    #[test]
    fn features_pick_known_names() {
        let f = features_from(&parse_video_encoders(ENCODERS));
        assert!(f.libx264 && f.libx265 && f.libsvtav1);
        assert_eq!(f.hevc_hardware, vec!["hevc_videotoolbox"]);
        assert!(f.av1_hardware.is_empty());
    }

    #[test]
    fn guesses_install_source() {
        assert_eq!(guess_source(Path::new("/opt/homebrew/Cellar/ffmpeg/7.1/bin/ffmpeg")), "Homebrew");
        assert_eq!(guess_source(Path::new("/usr/local/Cellar/ffmpeg/7.1/bin/ffmpeg")), "Homebrew");
        assert_eq!(
            guess_source(Path::new(r"C:\Users\a\AppData\Local\Microsoft\WinGet\Links\ffmpeg.exe")),
            "winget"
        );
        assert_eq!(guess_source(Path::new(r"C:\Users\a\scoop\shims\ffmpeg.exe")), "Scoop");
        assert_eq!(guess_source(Path::new("/usr/bin/ffmpeg")), "system");
        assert_eq!(guess_source(Path::new("/home/a/ffmpeg-n7.1/bin/ffmpeg")), "manual");
    }

    #[test]
    fn missing_custom_path_is_reported_in_searched_list() {
        let r = locate(Some("/definitely/not/here/ffmpeg"));
        assert_eq!(r.searched.first().map(String::as_str), Some("/definitely/not/here/ffmpeg"));
        assert_eq!(r.min_version, "6.0");
    }

    /// Paths with Korean and spaces, as users have them.
    const ENTERED: &str = "/Users/사용자/내 도구/ffmpeg 7/ffmpeg";
    const SAVED: &str = "/Volumes/외장 디스크/FFmpeg 빌드/ffmpeg";
    const AUTO: &str = "/opt/homebrew/bin/ffmpeg";

    pub(crate) fn info_at(path: &str) -> FfmpegInfo {
        FfmpegInfo {
            ffmpeg_path: path.to_string(),
            ffprobe_path: Path::new(path).with_file_name("ffprobe").display().to_string(),
            version: "9.0.2".into(),
            release: Some("9.0.2".into()),
            libavcodec_major: Some(63),
            meets_minimum: true,
            source: "직접 설치".into(),
            video_encoders: Vec::new(),
            features: Features::default(),
        }
    }

    pub(crate) fn auto_found() -> LocateResult {
        found(info_at(AUTO), vec!["/opt/homebrew/bin".into()])
    }

    /// `check` that accepts only the listed paths; the others fail like a missing file.
    fn only(ok: &'static [&'static str]) -> impl Fn(&str) -> Result<FfmpegInfo, Msg> {
        move |p| ok.contains(&p).then(|| info_at(p)).ok_or_else(|| Msg::file_not_found(p))
    }

    fn used(r: &LocateResult) -> &str {
        &r.info.as_ref().unwrap().ffmpeg_path
    }

    #[test]
    fn entered_path_that_works_is_used_and_returned_for_saving() {
        let (r, save) = locate_with(Some(ENTERED), Some(SAVED), only(&[ENTERED, SAVED]), || panic!("no auto search"));
        assert_eq!(used(&r), ENTERED);
        assert_eq!(save.as_deref(), Some(ENTERED));
        assert!(r.notices.is_empty());
        assert_eq!(r.searched, vec![ENTERED]);
    }

    #[test]
    fn saved_path_is_used_at_start_without_saving_again() {
        let (r, save) = locate_with(None, Some(SAVED), only(&[SAVED]), || panic!("no auto search"));
        assert_eq!(used(&r), SAVED);
        assert_eq!(save, None);
        assert!(r.notices.is_empty());
    }

    #[test]
    fn missing_saved_path_falls_back_to_auto_with_a_notice() {
        let (r, save) = locate_with(None, Some(SAVED), only(&[]), auto_found);
        assert_eq!(used(&r), AUTO);
        assert_eq!(save, None);
        assert_eq!(r.notices.len(), 1);
        assert_eq!(r.notices[0], Msg::SavedPathUnusable { why: Box::new(Msg::file_not_found(SAVED)) });
        // The skipped path is listed first, then what the automatic search looked at.
        assert_eq!(r.searched, vec![SAVED, "/opt/homebrew/bin"]);
    }

    #[test]
    fn too_old_saved_path_falls_back_to_auto() {
        let too_old = Msg::FfmpegTooOld { version: "5.1.2".into(), min: "6.0".into() };
        let old = |_: &str| -> Result<FfmpegInfo, Msg> { Err(too_old.clone()) };
        let (r, _) = locate_with(None, Some(SAVED), old, auto_found);
        assert_eq!(used(&r), AUTO);
        assert!(r.problem.is_none());
        assert_eq!(r.notices, [Msg::SavedPathUnusable { why: Box::new(too_old) }]);
    }

    #[test]
    fn unusable_entered_path_is_not_saved_and_the_saved_one_stays_in_use() {
        let (r, save) = locate_with(Some(ENTERED), Some(SAVED), only(&[SAVED]), || panic!("no auto search"));
        assert_eq!(used(&r), SAVED);
        assert_eq!(save, None);
        assert_eq!(r.notices.len(), 1);
        assert_eq!(r.notices[0], Msg::EnteredPathUnusable { why: Box::new(Msg::file_not_found(ENTERED)) });
        assert_eq!(r.searched, vec![ENTERED, SAVED]);
    }

    #[test]
    fn entering_the_saved_path_again_checks_it_once() {
        let calls = std::cell::Cell::new(0);
        let check = |p: &str| {
            calls.set(calls.get() + 1);
            only(&[])(p)
        };
        let (r, _) = locate_with(Some(&format!("  {SAVED} ")), Some(SAVED), check, auto_found);
        assert_eq!(calls.get(), 1);
        assert_eq!(r.notices.len(), 1);
        assert_eq!(used(&r), AUTO);
    }

    #[test]
    fn no_user_paths_is_the_plain_automatic_search() {
        let (r, save) = locate_with(Some("   "), None, |_: &str| -> Result<FfmpegInfo, Msg> { panic!("nothing to check") }, auto_found);
        assert_eq!(used(&r), AUTO);
        assert_eq!(save, None);
        assert!(r.notices.is_empty());
        assert_eq!(r.searched, vec!["/opt/homebrew/bin"]);
    }

    #[test]
    fn nothing_found_anywhere_keeps_every_notice() {
        let (r, _) = locate_with(Some(ENTERED), Some(SAVED), only(&[]), || not_found(vec!["/opt/homebrew/bin".into()], None));
        assert!(!r.found);
        assert_eq!(r.notices.len(), 2);
    }

    #[test]
    fn check_path_explains_a_missing_file() {
        let dir = std::env::temp_dir().join(format!("godseon 경로 확인 {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A folder without ffmpeg in it: the message names the file that was expected.
        let err = check_path(dir.to_str().unwrap()).unwrap_err();
        assert_eq!(err, Msg::file_not_found(dir.join(exe_name("ffmpeg")).display()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fake ffmpeg / ffprobe scripts reporting libavcodec 59 (FFmpeg 5.1): found, but too old.
    #[cfg(unix)]
    #[test]
    fn check_path_rejects_an_ffmpeg_older_than_6() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("godseon 옛 FFmpeg {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["ffmpeg", "ffprobe"] {
            let script = dir.join(name);
            std::fs::write(
                &script,
                format!("#!/bin/sh\necho '{name} version 5.1.2 Copyright (c) 2000-2022 the FFmpeg developers'\necho 'libavcodec     59. 37.100 / 59. 37.100'\n"),
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let err = check_path(dir.join("ffmpeg").to_str().unwrap()).unwrap_err();
        assert_eq!(err, Msg::FfmpegTooOld { version: "5.1.2".into(), min: "6.0".into() });
        let _ = std::fs::remove_dir_all(&dir);
    }
}
