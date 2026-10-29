//! Reshot: a resident Windows screenshot, OCR and screen-recording utility.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod assets;
mod export;
mod overlay;
mod radial;
mod recording;
mod settings_window;
mod theme;
mod tray_menu;
mod winutil;

use reshot_core::settings::SettingsService;
use reshot_platform::shell::Shell;
use reshot_platform::single_instance::SingleInstance;

fn main() {
    // Logging first: a launch that dies on the single-instance check must still leave a trace.
    reshot_core::log::init();
    std::panic::set_hook(Box::new(|info| log::error!("Panic: {info}")));

    let (events_tx, events_rx) = async_channel::unbounded();
    let Some(single) = SingleInstance::acquire(events_tx.clone()) else {
        log::info!("Startup: another instance already holds the lock; signalled it and exiting.");
        return;
    };
    log::info!("Startup: acquired single-instance lock.");

    // Tray icon and hotkey before anything else: they are what the user is waiting for.
    let shell = match Shell::start(1, "Reshot", events_tx) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Startup: shell thread failed: {e:#}");
            return;
        }
    };
    let capture_now = std::env::args().any(|a| a.eq_ignore_ascii_case("--capture"));
    reshot_core::fonts::warmup();

    gpui::Application::new().with_assets(assets::Assets).run(move |cx| {
        app::init(cx, SettingsService::default(), shell, single, events_rx, capture_now);
    });
    log::info!("Shutdown complete.");
}
