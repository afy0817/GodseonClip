//! App settings, saved as JSON in the app config folder:
//! macOS `~/Library/Application Support/com.godseon.clip/settings.json`,
//! Windows `%APPDATA%\com.godseon.clip\settings.json`,
//! Linux `~/.config/com.godseon.clip/settings.json`.
//!
//! A plain file from Rust instead of `tauri-plugin-store`: no new plugin or webview permission, the
//! UI never writes settings directly (it asks for an action and Rust decides what is saved), and
//! reading · writing is testable with temporary folders. Writes go to a temporary file in the same
//! folder that is then renamed over the old one, so a crash leaves the old or the new file, never half.

use crate::ffmpeg::{self, FfmpegInfo, LocateResult};
use crate::msg::Msg;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "settings.json";

/// Missing keys take their defaults, so later versions can add settings freely.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// ffmpeg file the user chose in settings, saved only once it was confirmed usable (6.0+).
    /// None: automatic search (PATH and install folders) at start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ffmpeg_path: Option<String>,
    /// "동시 작업" on the queue screen (1–4). None: 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_concurrency: Option<usize>,
    /// The language the user picked: "ko" · "en". None (or a value this version does not know,
    /// kept as it is): the system's language ([`system_lang`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Keys this version does not know (written by a newer one), written back unchanged.
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

impl Settings {
    fn normalized(mut self) -> Self {
        self.ffmpeg_path = self.ffmpeg_path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
        self.queue_concurrency = self.queue_concurrency.map(crate::queue::clamp_concurrency);
        self
    }
}

#[derive(Debug)]
pub struct Loaded {
    pub settings: Settings,
    /// Why the file could not be used (the defaults are used instead). A missing file is not a problem.
    pub warning: Option<Msg>,
}

pub fn load(file: &Path) -> Loaded {
    let defaults = |warning: Msg| Loaded { settings: Settings::default(), warning: Some(warning) };
    match std::fs::read(file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded { settings: Settings::default(), warning: None },
        Err(e) => defaults(Msg::SettingsUnreadable { path: file.display().to_string(), detail: e.to_string() }),
        Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
            Ok(s) => Loaded { settings: s.normalized(), warning: None },
            Err(e) => defaults(Msg::SettingsCorrupt { path: file.display().to_string(), detail: e.to_string() }),
        },
    }
}

/// Temporary file next to `file`; the process id keeps two running copies of the app apart.
fn temp_path(file: &Path) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    file.with_file_name(name)
}

/// Writes `settings` to a temporary file in the same folder, flushes it to disk, then renames it
/// over `file` (an atomic replace on macOS, Linux and Windows). Creates the folder if needed.
pub fn save(file: &Path, settings: &Settings) -> Result<(), Msg> {
    let fail = |e: &dyn std::fmt::Display| Msg::SettingsNotSaved { path: file.display().to_string(), detail: e.to_string() };
    let mut json = serde_json::to_vec_pretty(settings).map_err(|e| fail(&e))?;
    json.push(b'\n');
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail(&e))?;
    }
    let tmp = temp_path(file);
    let written = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, file)
    })();
    written.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        fail(&e)
    })
}

/// Saves the queue's "동시 작업" (the rest of the file stays as it is; an unreadable file is
/// replaced, as any save does).
pub fn save_queue_concurrency(file: &Path, n: usize) -> Result<(), Msg> {
    let mut settings = load(file).settings;
    settings.queue_concurrency = Some(crate::queue::clamp_concurrency(n));
    save(file, &settings)
}

/// The app's language. The UI holds the dictionaries; Rust needs it only for the native quit dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Lang {
    Ko,
    En,
}

impl Lang {
    /// "ko" · "ko-KR" · "ko_KR.UTF-8" → Korean, "en…" → English, anything else None.
    pub fn from_tag(tag: &str) -> Option<Lang> {
        let primary = tag.trim().split(['-', '_', '.', '@']).next().unwrap_or("").to_ascii_lowercase();
        match primary.as_str() {
            "ko" => Some(Lang::Ko),
            "en" => Some(Lang::En),
            _ => None,
        }
    }
}

/// The first of Korean and English in the system's preferred languages (most preferred first);
/// English when neither is there.
pub fn system_lang<S: AsRef<str>>(locales: impl IntoIterator<Item = S>) -> Lang {
    locales.into_iter().find_map(|l| Lang::from_tag(l.as_ref())).unwrap_or(Lang::En)
}

/// The language the user picked in settings (None: follow the system).
pub fn chosen_lang(settings: &Settings) -> Option<Lang> {
    settings.language.as_deref().and_then(Lang::from_tag)
}

/// What the settings screen shows: the pick, the language in use, and what "system" means here.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageInfo {
    /// None: the system's language.
    pub setting: Option<Lang>,
    pub effective: Lang,
    pub system: Lang,
    /// The pick could not be saved (it holds for this run).
    pub notice: Option<Msg>,
}

pub fn language_info(setting: Option<Lang>, system: Lang, notice: Option<Msg>) -> LanguageInfo {
    LanguageInfo { setting, effective: setting.unwrap_or(system), system, notice }
}

/// Saves the language pick (None: remove it — the system's language). The rest stays as it is.
pub fn save_language(file: &Path, setting: Option<Lang>) -> Result<(), Msg> {
    let mut settings = load(file).settings;
    settings.language = setting.map(|l| match l {
        Lang::Ko => "ko".to_string(),
        Lang::En => "en".to_string(),
    });
    save(file, &settings)
}

/// What the settings screen asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum FfmpegRequest {
    /// App start and "다시 찾기": the saved path first, then the automatic search.
    UseSaved,
    /// "이 경로로 확인" / "찾아보기": this path first, saved when it works.
    Choose(String),
    /// "자동 탐색으로 되돌리기": forget the saved path and search automatically.
    Forget,
}

/// Runs one FFmpeg request against the settings file: load, decide ([`ffmpeg::locate_with`]),
/// save what changed. `file` is None when there is no config folder (nothing is read or saved).
/// `check` and `auto` are [`ffmpeg::check_path`] and the automatic search in the app.
pub fn apply_ffmpeg_request(
    file: Option<&Path>,
    request: &FfmpegRequest,
    check: impl Fn(&str) -> Result<FfmpegInfo, Msg>,
    auto: impl FnOnce() -> LocateResult,
) -> LocateResult {
    let mut notices = Vec::new();
    let mut settings = match file {
        Some(f) => {
            let loaded = load(f);
            notices.extend(loaded.warning);
            loaded.settings
        }
        None => {
            notices.push(Msg::NoConfigFolder);
            Settings::default()
        }
    };
    // What is on disk after this call.
    let mut saved = settings.ffmpeg_path.clone();
    let store = |settings: &Settings, notices: &mut Vec<Msg>| match file.map(|f| save(f, settings)) {
        Some(Ok(())) => true,
        Some(Err(e)) => {
            notices.push(e);
            false
        }
        None => false,
    };

    if *request == FfmpegRequest::Forget && settings.ffmpeg_path.is_some() {
        settings.ffmpeg_path = None;
        if store(&settings, &mut notices) {
            saved = None;
        }
    }
    let entered = match request {
        FfmpegRequest::Choose(p) => Some(p.as_str()),
        _ => None,
    };
    let (mut result, accepted) = ffmpeg::locate_with(entered, settings.ffmpeg_path.as_deref(), check, auto);
    if let Some(path) = accepted.filter(|p| settings.ffmpeg_path.as_ref() != Some(p)) {
        settings.ffmpeg_path = Some(path.clone());
        if store(&settings, &mut notices) {
            saved = Some(path);
        } else {
            notices.push(Msg::ThisRunOnly);
        }
    }
    notices.append(&mut result.notices);
    result.notices = notices;
    result.saved_path = saved;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::tests::{auto_found, info_at};

    /// A fresh folder with Korean and spaces in its name, like a user's home on any OS.
    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("godseon 설정 {tag} {}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const MINE: &str = "/Users/사용자/내 도구/ffmpeg 7/ffmpeg";

    fn only_mine(p: &str) -> Result<FfmpegInfo, Msg> {
        if p == MINE { Ok(info_at(p)) } else { Err(Msg::file_not_found(p)) }
    }

    fn missing(p: &str) -> Result<FfmpegInfo, Msg> {
        Err(Msg::file_not_found(p))
    }

    fn on_disk(file: &Path) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap()
    }

    #[test]
    fn missing_file_is_defaults_without_a_warning() {
        let dir = temp_dir("없음");
        let l = load(&dir.join(FILE_NAME));
        assert_eq!(l.settings, Settings::default());
        assert!(l.warning.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_then_load_round_trips_in_camel_case() {
        let dir = temp_dir("왕복");
        // The config folder does not exist yet on a first run.
        let file = dir.join("com.godseon.clip").join(FILE_NAME);
        let s = Settings { ffmpeg_path: Some(MINE.into()), ..Default::default() };
        save(&file, &s).unwrap();
        assert_eq!(on_disk(&file), serde_json::json!({ "ffmpegPath": MINE }));
        assert_eq!(load(&file).settings, s);
        // Only settings.json is left: the temporary file was renamed.
        let names: Vec<_> = std::fs::read_dir(file.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from(FILE_NAME)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn queue_concurrency_is_saved_next_to_the_ffmpeg_path() {
        let dir = temp_dir("동시 작업");
        let file = dir.join(FILE_NAME);
        save(&file, &Settings { ffmpeg_path: Some(MINE.into()), ..Default::default() }).unwrap();
        save_queue_concurrency(&file, 3).unwrap();
        assert_eq!(on_disk(&file), serde_json::json!({ "ffmpegPath": MINE, "queueConcurrency": 3 }));
        save_queue_concurrency(&file, 0).unwrap();
        assert_eq!(load(&file).settings.queue_concurrency, Some(1));
        std::fs::write(&file, r#"{ "queueConcurrency": 12 }"#).unwrap();
        assert_eq!(load(&file).settings.queue_concurrency, Some(4), "out of range reads as the nearest allowed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_replaces_an_existing_file() {
        let dir = temp_dir("덮어쓰기");
        let file = dir.join(FILE_NAME);
        save(&file, &Settings { ffmpeg_path: Some("/옛 경로/ffmpeg".into()), ..Default::default() }).unwrap();
        save(&file, &Settings::default()).unwrap();
        assert_eq!(on_disk(&file), serde_json::json!({}));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_keys_survive_a_save_and_missing_keys_default() {
        let dir = temp_dir("모르는 키");
        let file = dir.join(FILE_NAME);
        std::fs::write(&file, r#"{ "outputFolder": "/Users/사용자/내 영상", "ffmpegPath": "  " }"#).unwrap();
        let l = load(&file);
        assert!(l.warning.is_none());
        assert_eq!(l.settings.ffmpeg_path, None, "a blank path means automatic search");
        let mut s = l.settings;
        s.ffmpeg_path = Some(MINE.into());
        save(&file, &s).unwrap();
        assert_eq!(on_disk(&file), serde_json::json!({ "ffmpegPath": MINE, "outputFolder": "/Users/사용자/내 영상" }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_gives_defaults_and_a_warning() {
        let dir = temp_dir("손상");
        let file = dir.join(FILE_NAME);
        for text in ["{ \"ffmpegPath\": ", "", "[1, 2]", r#"{ "ffmpegPath": 3 }"#] {
            std::fs::write(&file, text).unwrap();
            let l = load(&file);
            assert_eq!(l.settings, Settings::default(), "{text:?}");
            assert!(matches!(l.warning, Some(Msg::SettingsCorrupt { .. })), "{text:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_save_leaves_the_old_file_and_no_temporary_file() {
        let dir = temp_dir("실패");
        // The "file" is a folder: the rename fails.
        let file = dir.join(FILE_NAME);
        std::fs::create_dir_all(file.join("안쪽")).unwrap();
        let err = save(&file, &Settings { ffmpeg_path: Some(MINE.into()), ..Default::default() }).unwrap_err();
        assert!(matches!(&err, Msg::SettingsNotSaved { path, .. } if *path == file.display().to_string()), "{err}");
        assert!(file.join("안쪽").is_dir());
        assert!(!temp_path(&file).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chosen_path_is_saved_and_used_at_the_next_start() {
        let dir = temp_dir("저장");
        let file = dir.join(FILE_NAME);
        let r = apply_ffmpeg_request(Some(&file), &FfmpegRequest::Choose(MINE.into()), only_mine, || panic!("no auto search"));
        assert_eq!(r.info.unwrap().ffmpeg_path, MINE);
        assert_eq!(r.saved_path.as_deref(), Some(MINE));
        assert!(r.notices.is_empty(), "{:?}", r.notices);
        assert_eq!(on_disk(&file), serde_json::json!({ "ffmpegPath": MINE }));

        // Restart: the saved path is checked first.
        let r = apply_ffmpeg_request(Some(&file), &FfmpegRequest::UseSaved, only_mine, || panic!("no auto search"));
        assert_eq!(r.info.unwrap().ffmpeg_path, MINE);
        assert_eq!(r.saved_path.as_deref(), Some(MINE));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn vanished_saved_path_falls_back_with_a_notice_and_stays_saved() {
        let dir = temp_dir("사라짐");
        let file = dir.join(FILE_NAME);
        save(&file, &Settings { ffmpeg_path: Some(MINE.into()), ..Default::default() }).unwrap();
        let r = apply_ffmpeg_request(Some(&file), &FfmpegRequest::UseSaved, missing, auto_found);
        assert_eq!(r.info.unwrap().ffmpeg_path, "/opt/homebrew/bin/ffmpeg");
        assert_eq!(r.notices, vec![Msg::SavedPathUnusable { why: Box::new(Msg::file_not_found(MINE)) }]);
        // Kept (a disk that is not mounted yet comes back), until the user chooses or forgets.
        assert_eq!(r.saved_path.as_deref(), Some(MINE));
        assert_eq!(on_disk(&file), serde_json::json!({ "ffmpegPath": MINE }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unusable_choice_changes_nothing_on_disk() {
        let dir = temp_dir("거절");
        let file = dir.join(FILE_NAME);
        let r = apply_ffmpeg_request(Some(&file), &FfmpegRequest::Choose("/없는 폴더/ffmpeg".into()), missing, auto_found);
        assert!(r.found);
        assert_eq!(r.saved_path, None);
        assert_eq!(r.notices.len(), 1);
        assert!(!file.exists(), "nothing to save");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn forget_clears_the_saved_path_and_searches_automatically() {
        let dir = temp_dir("되돌리기");
        let file = dir.join(FILE_NAME);
        std::fs::write(&file, format!(r#"{{ "ffmpegPath": "{MINE}", "outputFolder": "/내 영상" }}"#)).unwrap();
        let r = apply_ffmpeg_request(Some(&file), &FfmpegRequest::Forget, |_: &str| panic!("the saved path is not checked"), auto_found);
        assert_eq!(r.info.unwrap().ffmpeg_path, "/opt/homebrew/bin/ffmpeg");
        assert_eq!(r.saved_path, None);
        assert!(r.notices.is_empty(), "{:?}", r.notices);
        assert_eq!(on_disk(&file), serde_json::json!({ "outputFolder": "/내 영상" }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_reported_then_replaced_by_a_good_choice() {
        let dir = temp_dir("손상 후 저장");
        let file = dir.join(FILE_NAME);
        std::fs::write(&file, "{ 깨진").unwrap();
        let r = apply_ffmpeg_request(Some(&file), &FfmpegRequest::UseSaved, missing, auto_found);
        assert_eq!(r.notices.len(), 1);
        assert!(matches!(r.notices[0], Msg::SettingsCorrupt { .. }), "{:?}", r.notices);
        let r = apply_ffmpeg_request(Some(&file), &FfmpegRequest::Choose(MINE.into()), only_mine, auto_found);
        assert_eq!(r.saved_path.as_deref(), Some(MINE));
        assert_eq!(on_disk(&file), serde_json::json!({ "ffmpegPath": MINE }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn choice_that_cannot_be_saved_is_used_for_this_run_only() {
        let dir = temp_dir("저장 실패");
        let file = dir.join(FILE_NAME);
        std::fs::create_dir_all(file.join("안쪽")).unwrap();
        let r = apply_ffmpeg_request(Some(&file), &FfmpegRequest::Choose(MINE.into()), only_mine, auto_found);
        assert_eq!(r.info.unwrap().ffmpeg_path, MINE);
        assert_eq!(r.saved_path, None);
        // settings.json is a folder here, so reading it failed too.
        assert_eq!(r.notices.len(), 3, "{:?}", r.notices);
        assert!(matches!(r.notices[0], Msg::SettingsUnreadable { .. }), "{:?}", r.notices);
        assert!(matches!(r.notices[1], Msg::SettingsNotSaved { .. }), "{:?}", r.notices);
        assert_eq!(r.notices[2], Msg::ThisRunOnly);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_config_folder_still_locates() {
        let r = apply_ffmpeg_request(None, &FfmpegRequest::Choose(MINE.into()), only_mine, auto_found);
        assert_eq!(r.info.unwrap().ffmpeg_path, MINE);
        assert_eq!(r.saved_path, None);
        assert_eq!(r.notices[0], Msg::NoConfigFolder);
    }

    #[test]
    fn language_follows_the_pick_else_the_system() {
        assert_eq!(system_lang(["ko-KR", "en-US"]), Lang::Ko);
        assert_eq!(system_lang(["en-GB"]), Lang::En);
        // The first of the two the app has, most preferred first; neither: English.
        assert_eq!(system_lang(["ja-JP", "ko-KR", "en"]), Lang::Ko);
        assert_eq!(system_lang(["ja-JP", "zh-Hans-CN"]), Lang::En);
        assert_eq!(system_lang(Vec::<String>::new()), Lang::En);
        assert_eq!(system_lang(["ko_KR.UTF-8"]), Lang::Ko, "a POSIX locale (Linux LANG)");
        assert_eq!(Lang::from_tag("korean"), None);

        let info = language_info(None, Lang::Ko, None);
        assert_eq!((info.setting, info.effective, info.system), (None, Lang::Ko, Lang::Ko));
        assert_eq!(language_info(Some(Lang::En), Lang::Ko, None).effective, Lang::En);
        assert_eq!(
            serde_json::to_value(language_info(Some(Lang::En), Lang::Ko, Some(Msg::NoConfigFolder))).unwrap(),
            serde_json::json!({ "setting": "en", "effective": "en", "system": "ko", "notice": { "code": "noConfigFolder" } })
        );
    }

    #[test]
    fn language_is_saved_without_touching_the_rest() {
        let dir = temp_dir("언어");
        let file = dir.join(FILE_NAME);
        std::fs::write(&file, format!(r#"{{ "ffmpegPath": "{MINE}", "queueConcurrency": 2, "새 항목": true }}"#)).unwrap();
        save_language(&file, Some(Lang::En)).unwrap();
        assert_eq!(chosen_lang(&load(&file).settings), Some(Lang::En));
        let json = on_disk(&file);
        assert_eq!((json["language"].as_str(), json["queueConcurrency"].as_u64(), json["새 항목"].as_bool()), (Some("en"), Some(2), Some(true)));
        assert_eq!(json["ffmpegPath"].as_str(), Some(MINE));
        // "시스템" removes the key.
        save_language(&file, None).unwrap();
        assert!(on_disk(&file).get("language").is_none());
        assert_eq!(chosen_lang(&load(&file).settings), None);
        // A value a newer version wrote is no error: the system's language, and it is kept.
        std::fs::write(&file, r#"{ "language": "ja" }"#).unwrap();
        let l = load(&file);
        assert!(l.warning.is_none());
        assert_eq!(chosen_lang(&l.settings), None);
        save(&file, &l.settings).unwrap();
        assert_eq!(on_disk(&file)["language"].as_str(), Some("ja"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
