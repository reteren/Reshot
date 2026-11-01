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
    let tooltip = if cfg!(debug_assertions) { "Reshot (dev)" } else { "Reshot" };
    let shell = match Shell::start(1, tooltip, events_tx) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Startup: shell thread failed: {e:#}");
            return;
        }
    };
    let mut settings = SettingsService::default();
    let s = settings.load().clone();
    let main_vk = app::register_startup_hotkeys(&shell, &s);

    // Off the main thread: schtasks queries and the WinRT access request are slow.
    let (enabled, elevated) = (s.autostart, s.autostart_elevated);
    std::thread::spawn(move || {
        reshot_platform::autostart::apply(enabled, elevated, false);
        reshot_capture::request_borderless_access();
    });
    log::info!("Startup: ready (idle in tray).");

    // gpui (Direct3D, DirectWrite, its executors) is started by the first thing that needs a
    // window, not at boot: until then the process is the tray, the hotkeys and nothing else.
    // The first press pays for the start-up, as the first WPF window did in the C# build.
    let capture_now = std::env::args().any(|a| a.eq_ignore_ascii_case("--capture"));
    // `--settings` opens the settings window at once; `--settings=<tab>` picks the tab.
    let settings_now = std::env::args().find_map(|a| {
        let lower = a.to_ascii_lowercase();
        lower.strip_prefix("--settings").and_then(|rest| match rest {
            "" => Some(String::new()),
            _ => rest.strip_prefix('=').map(str::to_string),
        })
    });
    let preview = cfg!(debug_assertions)
        && std::env::args().any(|a| ["--preview-track-prompt", "--preview-radial", "--preview-recording-hud"].iter().any(|f| a.eq_ignore_ascii_case(f)));
    if !capture_now && settings_now.is_none() && !preview {
        match events_rx.recv_blocking() {
            Ok(first) => {
                log::info!("Startup: first use ({first:?}); starting the UI.");
                // Handed back so the UI handles it like any other event.
                let _ = shell.event_sender().try_send(first);
            }
            Err(_) => return,
        }
    }

    let resident = app::Resident { settings, shell, single, main_vk };
    gpui::Application::new().with_assets(assets::Assets).run(move |cx| {
        app::init(cx, resident, events_rx, capture_now, settings_now);
    });
    log::info!("Shutdown complete.");
}
