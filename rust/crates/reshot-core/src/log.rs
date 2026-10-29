//! Minimal file logger behind the `log` facade. Reshot spends most of its life asleep, so
//! there is no logging framework and no worker thread: one append-only file, written through
//! on every line for crash durability, truncated in place once it passes 1 MB.

use parking_lot::Mutex;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MAX_LOG_BYTES: u64 = 1_000_000;

struct FileLogger {
    file: Mutex<Option<File>>,
}

static LOGGER: FileLogger = FileLogger { file: Mutex::new(None) };

impl log::Log for FileLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let tag = match record.level() {
            log::Level::Error => "[Error]",
            log::Level::Warn => "[Warn ]",
            _ => "[Info ]",
        };
        let line = format!(
            "{} {tag} {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            record.args()
        );
        if cfg!(debug_assertions) {
            eprint!("{line}");
        }
        let mut guard = self.file.lock();
        if let Some(f) = guard.as_mut() {
            // Roll over before writing, so the triggering line stays at the top of the file.
            if f.metadata().map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
                let _ = f.set_len(0);
                let _ = f.seek(SeekFrom::Start(0));
            }
            // Logging must never crash the app.
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Some(f) = self.file.lock().as_mut() {
            let _ = f.flush();
        }
    }
}

fn open(path: &Path) -> Option<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    let mut f = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path).ok()?;
    if f.metadata().ok()?.len() > MAX_LOG_BYTES {
        f.set_len(0).ok()?;
    }
    f.seek(SeekFrom::End(0)).ok()?;
    Some(f)
}

/// Points logging at a file, or `None` to disable file logging. Safe to call repeatedly.
pub fn set_log_file(path: Option<PathBuf>) {
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    *LOGGER.file.lock() = path.as_deref().and_then(open);
}

/// Called once at startup: `%AppData%\reshot\logs\reshot.log`.
pub fn init() {
    let _ = crate::paths::ensure_app_data_dir();
    set_log_file(Some(crate::paths::LOGS_DIR.join("reshot.log")));
    log::info!("=== Reshot session started (pid {}) ===", std::process::id());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_lines_and_rolls_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.log");
        set_log_file(Some(path.clone()));
        log::info!("hello");
        log::warn!("careful");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[Info ] hello"));
        assert!(text.contains("[Warn ] careful"));

        std::fs::write(&path, vec![b'x'; (MAX_LOG_BYTES + 10) as usize]).unwrap();
        set_log_file(Some(path.clone()));
        log::info!("after");
        let len = std::fs::metadata(&path).unwrap().len();
        assert!(len < 200, "log was not truncated: {len}");
        set_log_file(None);
    }
}
