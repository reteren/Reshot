//! Writing results to disk. The pixels come from `reshot_core::export`; this only decides
//! where they go.

use anyhow::Result;
use std::path::PathBuf;

use reshot_core::export::{self, ImageFormat};
use reshot_core::settings::AppSettings;
use reshot_core::skia::Bitmap;
use reshot_core::{filename, paths};

pub fn screenshot_dir(settings: &AppSettings) -> PathBuf {
    let dir = settings.paths.screenshots.trim();
    if dir.is_empty() { paths::DEFAULT_SCREENSHOTS_DIR.clone() } else { PathBuf::from(dir) }
}

/// Saves to the configured screenshots folder with the templated name.
pub fn save(bitmap: &Bitmap, settings: &AppSettings) -> Result<PathBuf> {
    let format = ImageFormat::parse(&settings.format.image);
    let dir = screenshot_dir(settings);
    std::fs::create_dir_all(&dir)?;
    let name = filename::build(&settings.filename.template, format.extension(), chrono_now());
    let path = dir.join(name);
    write(bitmap, &path, settings.format.quality)?;
    log::info!("Export: saved {}", path.display());
    Ok(path)
}

/// Encodes by the file's extension.
pub fn write(bitmap: &Bitmap, path: &std::path::Path, quality: i32) -> Result<()> {
    let format = ImageFormat::parse(&path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default());
    let data = export::encode(bitmap, format, quality).ok_or_else(|| anyhow::anyhow!("encoding failed"))?;
    std::fs::write(path, data)?;
    Ok(())
}

fn chrono_now() -> reshot_core::filename::NaiveDateTime {
    reshot_core::filename::now()
}
