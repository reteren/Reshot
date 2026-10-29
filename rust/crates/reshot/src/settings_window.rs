//! The settings dialog in gpui, replacing the Tauri window. Ported in phase R9; until then it
//! opens settings.json in the default editor.

use gpui::App;

pub fn open(cx: &mut App) {
    let path = cx.global::<crate::app::ReshotApp>().settings.path().to_path_buf();
    log::info!("Settings: opening {} (gpui dialog lands in R9).", path.display());
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", &path.to_string_lossy()]).spawn();
}
