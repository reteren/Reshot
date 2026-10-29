//! Strongly-typed mirror of `settings.json` (SPEC §15), stored as pretty camelCase JSON in
//! `%AppData%\reshot\settings.json`. Every field carries its documented default, so a fresh
//! value already equals the shipped defaults; [`SettingsService`] only fills in the
//! machine-specific output folders. The key names are the ones the C# build wrote, so an
//! existing settings file keeps working after the switch.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::paths;

macro_rules! section {
    ($(#[$m:meta])* $name:ident { $($(#[$fm:meta])* $field:ident : $ty:ty = $def:expr),* $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase", default)]
        pub struct $name { $($(#[$fm])* pub $field: $ty),* }
        impl Default for $name {
            fn default() -> Self { Self { $($field: $def),* } }
        }
    };
}

section!(AppSettings {
    /// Main global hotkey. Default is Print Screen.
    hotkey: String = "PrtScn".into(),
    /// Optional global hotkey for instant audio recording. Empty = off.
    audio_hotkey: String = String::new(),
    dim: DimSettings = DimSettings::default(),
    overlay: OverlaySettings = OverlaySettings::default(),
    paths: PathSettings = PathSettings::default(),
    /// Start Reshot with Windows.
    autostart: bool = true,
    /// Start it elevated through a scheduled task, which is what lets the overlay come up over
    /// an elevated application. Ignored while `autostart` is off.
    autostart_elevated: bool = false,
    format: FormatSettings = FormatSettings::default(),
    filename: FilenameSettings = FilenameSettings::default(),
    video: VideoSettings = VideoSettings::default(),
    audio: AudioSettings = AudioSettings::default(),
    update: UpdateSettings = UpdateSettings::default(),
    radial: RadialSettings = RadialSettings::default(),
});

section!(
    /// Behaviour of the hold-to-open radial quick menu.
    RadialSettings {
    /// Keep the wheel open after the hotkey is released and pick with a left click. On by
    /// default: a menu that vanishes when you let go teaches nobody that it exists.
    click_to_choose: bool = true,
});

section!(
    /// Standalone audio recorder; remembers the last-used sources.
    AudioSettings {
    system: bool = true,
    mic: bool = false,
    /// Microphone device id, or "default". Shared with the video recorder.
    mic_device: String = "default".into(),
});

section!(OverlaySettings {
    /// Hide the capture overlay from every screen-capture API (`WDA_EXCLUDEFROMCAPTURE`).
    /// Off by default: someone screen sharing usually wants the other side to see what is
    /// being pointed at. Reshot's own recordings are unaffected either way.
    hide_from_capture: bool = false,
});

section!(DimSettings {
    opacity: f64 = 0.5,
    color: String = "#000000".into(),
});

section!(
    /// Output folders. Empty means "use the per-user default folder".
    PathSettings {
    screenshots: String = String::new(),
    videos: String = String::new(),
    records: String = String::new(),
});

section!(FormatSettings {
    /// png / jpg / webp.
    image: String = "png".into(),
    /// 1..100 for jpg and webp.
    quality: i32 = 90,
});

section!(FilenameSettings {
    template: String = crate::filename::DEFAULT_TEMPLATE.into(),
});

section!(VideoSettings {
    /// 60 / 30 / 25.
    fps: i32 = 60,
    audio: VideoAudioSettings = VideoAudioSettings::default(),
    corners: VideoCornersSettings = VideoCornersSettings::default(),
});

section!(VideoAudioSettings {
    mic: bool = true,
    system: bool = true,
    /// Show the track picker when saving a recording.
    ask_on_save: bool = true,
    mic_device: String = "default".into(),
});

section!(VideoCornersSettings {
    enabled: bool = true,
    color: String = "#3C9898".into(),
    opacity: f64 = 0.7,
});

section!(UpdateSettings {
    /// Automatic updates. Not implemented yet.
    auto: bool = true,
});

/// Loads and saves [`AppSettings`]. On a missing or corrupt file it falls back to defaults,
/// so the app always starts.
pub struct SettingsService {
    file: PathBuf,
    current: AppSettings,
}

impl Default for SettingsService {
    fn default() -> Self {
        Self::new(paths::SETTINGS_FILE.clone())
    }
}

impl SettingsService {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        Self {
            file: file.into(),
            current: AppSettings::default(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.file
    }

    pub fn current(&self) -> &AppSettings {
        &self.current
    }

    pub fn current_mut(&mut self) -> &mut AppSettings {
        &mut self.current
    }

    /// Loads settings from disk, or writes the defaults file on first run.
    pub fn load(&mut self) -> &AppSettings {
        let settings = match std::fs::read_to_string(&self.file) {
            Ok(json) => parse(&json).unwrap_or_else(|e| {
                log::error!(
                    "Settings: failed to read {}, using defaults: {e}",
                    self.file.display()
                );
                AppSettings::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.current = resolve_defaults(AppSettings::default());
                self.save();
                log::info!("Settings: created defaults at {}", self.file.display());
                return &self.current;
            }
            Err(e) => {
                log::error!(
                    "Settings: failed to read {}, using defaults: {e}",
                    self.file.display()
                );
                AppSettings::default()
            }
        };
        self.current = resolve_defaults(settings);
        &self.current
    }

    /// Replaces the current settings (from the settings window) and saves.
    pub fn update(&mut self, settings: AppSettings) {
        self.current = resolve_defaults(settings);
        self.save();
    }

    /// Persists the current settings, creating the folder if needed.
    pub fn save(&self) {
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = self.file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let json =
                serde_json::to_string_pretty(&self.current).map_err(std::io::Error::other)?;
            std::fs::write(&self.file, json)
        })();
        if let Err(e) = result {
            log::error!("Settings: failed to write {}: {e}", self.file.display());
        }
    }
}

/// The C# reader skipped comments and allowed trailing commas; keep accepting files that
/// were hand-edited that way.
fn parse(json: &str) -> serde_json::Result<AppSettings> {
    serde_json::from_str(json).or_else(|e| serde_json::from_str(&relax_json(json)).map_err(|_| e))
}

/// Strips `//` and `/* */` comments and commas that directly precede `}` or `]`, leaving
/// string contents untouched.
fn relax_json(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let mut in_str = false;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match (c, chars.get(i + 1)) {
            ('"', _) => {
                in_str = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            ('/', Some('*')) => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            (',', _) => {
                let mut j = i + 1;
                // Look past whitespace and comments for the next meaningful character.
                loop {
                    while j < chars.len() && chars[j].is_whitespace() {
                        j += 1;
                    }
                    if j + 1 < chars.len() && chars[j] == '/' && chars[j + 1] == '/' {
                        while j < chars.len() && chars[j] != '\n' {
                            j += 1;
                        }
                    } else if j + 1 < chars.len() && chars[j] == '/' && chars[j + 1] == '*' {
                        j += 2;
                        while j + 1 < chars.len() && !(chars[j] == '*' && chars[j + 1] == '/') {
                            j += 1;
                        }
                        j += 2;
                    } else {
                        break;
                    }
                }
                if !matches!(chars.get(j), Some('}') | Some(']')) {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

fn resolve_defaults(mut s: AppSettings) -> AppSettings {
    let fill = |v: &mut String, d: &PathBuf| {
        if v.trim().is_empty() {
            *v = d.to_string_lossy().into_owned();
        }
    };
    fill(&mut s.paths.screenshots, &paths::DEFAULT_SCREENSHOTS_DIR);
    fill(&mut s.paths.videos, &paths::DEFAULT_VIDEOS_DIR);
    fill(&mut s.paths.records, &paths::DEFAULT_RECORDS_DIR);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        (dir, file)
    }

    #[test]
    fn load_missing_file_creates_defaults() {
        let (_d, f) = temp();
        let mut svc = SettingsService::new(&f);
        let s = svc.load().clone();
        assert!(f.exists());
        assert_eq!(s.hotkey, "PrtScn");
        assert_eq!(s.dim.opacity, 0.5);
        assert_eq!(s.dim.color, "#000000");
        assert!(s.autostart);
        assert_eq!(s.video.fps, 60);
        assert!(s.video.audio.mic);
        assert!(s.radial.click_to_choose);
        assert!(!s.overlay.hide_from_capture);
    }

    #[test]
    fn load_resolves_empty_paths_to_defaults() {
        let (_d, f) = temp();
        let mut svc = SettingsService::new(&f);
        let s = svc.load();
        assert_eq!(
            PathBuf::from(&s.paths.screenshots),
            *paths::DEFAULT_SCREENSHOTS_DIR
        );
        assert_eq!(PathBuf::from(&s.paths.videos), *paths::DEFAULT_VIDEOS_DIR);
    }

    #[test]
    fn save_then_load_roundtrips_custom_values() {
        let (_d, f) = temp();
        let mut svc = SettingsService::new(&f);
        svc.load();
        let c = svc.current_mut();
        c.hotkey = "Ctrl+Shift+4".into();
        c.dim.opacity = 0.75;
        c.autostart = false;
        c.radial.click_to_choose = false;
        c.overlay.hide_from_capture = true;
        svc.save();

        let mut again = SettingsService::new(&f);
        let s = again.load();
        assert_eq!(s.hotkey, "Ctrl+Shift+4");
        assert_eq!(s.dim.opacity, 0.75);
        assert!(!s.autostart);
        assert!(!s.radial.click_to_choose);
        assert!(s.overlay.hide_from_capture);
    }

    #[test]
    fn load_corrupt_file_falls_back_to_defaults() {
        let (_d, f) = temp();
        std::fs::write(&f, "{ this is not valid json ").unwrap();
        let mut svc = SettingsService::new(&f);
        assert_eq!(svc.load().hotkey, "PrtScn");
    }

    #[test]
    fn accepts_comments_and_trailing_commas() {
        let (_d, f) = temp();
        std::fs::write(
            &f,
            "{\n // note\n \"hotkey\": \"F9\", /* x */\n \"dim\": {\"opacity\": 0.3,},\n \"filename\": {\"template\": \"a//b,}\"}\n}",
        )
        .unwrap();
        let mut svc = SettingsService::new(&f);
        let s = svc.load();
        assert_eq!(s.hotkey, "F9");
        assert_eq!(s.dim.opacity, 0.3);
        assert_eq!(s.filename.template, "a//b,}");
    }

    #[test]
    fn serialized_json_uses_camel_case_keys() {
        let (_d, f) = temp();
        let mut svc = SettingsService::new(&f);
        svc.load();
        svc.save();
        let json = std::fs::read_to_string(&f).unwrap();
        for key in [
            "\"hotkey\"",
            "\"askOnSave\"",
            "\"radial\"",
            "\"clickToChoose\"",
            "\"overlay\"",
            "\"hideFromCapture\"",
        ] {
            assert!(json.contains(key), "missing {key}");
        }
        assert!(!json.contains("\"Hotkey\""));
    }
}
