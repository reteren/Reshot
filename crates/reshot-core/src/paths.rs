//! Every path Reshot writes to under the user profile. Everything lives under
//! `%AppData%\reshot\` (SPEC §15).

use std::path::PathBuf;
use std::sync::LazyLock;

fn known(dir: Option<PathBuf>) -> PathBuf {
    dir.unwrap_or_else(std::env::temp_dir).join("reshot")
}

/// `%AppData%\reshot`, root of all persisted state.
pub static APP_DATA_DIR: LazyLock<PathBuf> = LazyLock::new(|| known(dirs::config_dir()));
/// `%AppData%\reshot\settings.json`.
pub static SETTINGS_FILE: LazyLock<PathBuf> = LazyLock::new(|| APP_DATA_DIR.join("settings.json"));
/// `%AppData%\reshot\logs`.
pub static LOGS_DIR: LazyLock<PathBuf> = LazyLock::new(|| APP_DATA_DIR.join("logs"));
/// Default screenshot output folder: `Pictures\reshot`.
pub static DEFAULT_SCREENSHOTS_DIR: LazyLock<PathBuf> =
    LazyLock::new(|| known(dirs::picture_dir()));
/// Default video output folder: `Videos\reshot`.
pub static DEFAULT_VIDEOS_DIR: LazyLock<PathBuf> = LazyLock::new(|| known(dirs::video_dir()));
/// Default audio-recording output folder: `Music\reshot`.
pub static DEFAULT_RECORDS_DIR: LazyLock<PathBuf> = LazyLock::new(|| known(dirs::audio_dir()));

/// Creates the AppData and logs directories if they do not exist yet.
pub fn ensure_app_data_dir() -> std::io::Result<()> {
    std::fs::create_dir_all(&*APP_DATA_DIR)?;
    std::fs::create_dir_all(&*LOGS_DIR)
}
