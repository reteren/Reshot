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

/// The Save As dialog, owned by the overlay so it surfaces above the topmost window. `None`
/// when cancelled.
pub async fn ask_save_path(settings: &AppSettings, parent: Option<isize>) -> Option<PathBuf> {
    let format = ImageFormat::parse(&settings.format.image);
    let name = filename::build(&settings.filename.template, format.extension(), chrono_now());
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title("Save screenshot")
        .set_directory(screenshot_dir(settings))
        .set_file_name(name);
    // The settings' format is offered first, so it is the default filter.
    let filters = [(ImageFormat::Png, "PNG image"), (ImageFormat::Jpg, "JPEG image"), (ImageFormat::Webp, "WebP image")];
    let first = filters.iter().position(|(f, _)| *f == format).unwrap_or(0);
    for i in (first..filters.len()).chain(0..first) {
        let (f, label) = filters[i];
        dialog = dialog.add_filter(label, &[f.extension()]);
    }
    if let Some(raw) = parent {
        dialog = dialog.set_parent(&Parent(raw));
    }
    let file = dialog.save_file().await?;
    let mut path = file.path().to_path_buf();
    if path.extension().is_none() {
        path.set_extension(format.extension());
    }
    Some(path)
}

struct Parent(isize);

impl raw_window_handle::HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let hwnd = std::num::NonZeroIsize::new(self.0).ok_or(raw_window_handle::HandleError::Unavailable)?;
        let raw = raw_window_handle::RawWindowHandle::Win32(raw_window_handle::Win32WindowHandle::new(hwnd));
        // SAFETY: the overlay window outlives the modal dialog it owns.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
    }
}

impl raw_window_handle::HasDisplayHandle for Parent {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Ok(raw_window_handle::DisplayHandle::windows())
    }
}
