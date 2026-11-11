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
mod winutil;

use reshot_core::i18n::{self, tr, trf};
use reshot_core::settings::SettingsService;
use reshot_platform::shell::Shell;
use reshot_platform::single_instance::SingleInstance;

fn main() {
    // The display language is known before any setting is read, so even the refusal below
    // speaks the user's language.
    i18n::set_system_languages(reshot_platform::win::preferred_ui_languages());
    i18n::apply("");
    // Refuse unsafe elevated copies before reading user-controlled settings or starting
    // workers. Keep the installation handles pinned for the full application lifetime.
    let _installation = match reshot_platform::security::guard_elevated_startup() {
        Ok(installation) => installation,
        Err(error) => {
            let message = windows::core::HSTRING::from(trf(
                "Reshot cannot start safely as administrator.\n\nInstall it for all users in Program Files, or launch it without administrator rights.\n\n{error}",
                &[("error", &format!("{error:#}"))],
            ));
            let title = windows::core::HSTRING::from(tr("Reshot: administrator mode blocked"));
            unsafe {
                windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                    None,
                    &message,
                    &title,
                    windows::Win32::UI::WindowsAndMessaging::MB_OK
                        | windows::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
                );
            }
            return;
        }
    };
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
    // `--lang=<code>` (debug builds) shows the interface in another language without touching
    // the settings file the installed copy shares.
    let lang_override = std::env::args()
        .filter(|_| cfg!(debug_assertions))
        .find_map(|a| a.strip_prefix("--lang=").map(str::to_string));
    i18n::apply(lang_override.as_deref().unwrap_or(&s.language));
    log::info!("Startup: interface language {}.", i18n::current().code);
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
    // The tray menu runs entirely on its shell thread; only an action needing UI starts gpui.
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
        && std::env::args().any(|a| ["--preview-track-prompt", "--preview-radial", "--preview-recording-hud", "--bench-video", "--bench-audio", "--bench-cycles", "--bench-mem"].iter().any(|f| a.eq_ignore_ascii_case(f)));
    let initial_event = if !capture_now && settings_now.is_none() && !preview {
        match events_rx.recv_blocking() {
            Ok(first) => {
                if first == reshot_platform::shell::ShellEvent::TrayQuit { return; }
                log::info!("Startup: first use ({first:?}); starting the UI.");
                Some(first)
            }
            Err(_) => return,
        }
    } else { None };

    let resident = app::Resident { settings, shell, single, main_vk };
    gpui::Application::new().with_assets(assets::Assets).run(move |cx| {
        app::init(cx, resident, events_rx, initial_event, capture_now, settings_now);
    });
    log::info!("Shutdown complete.");
}
