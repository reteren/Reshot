//! Composition root: owns settings, the shell thread, the capture service and the windows,
//! and routes shell events (hotkeys, tray) to them. Holds no editing or capture logic.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, AsyncApp, Global, WindowHandle};
use reshot_capture::ScreenCaptureService;
use reshot_core::hotkey::HotkeyDefinition;
use reshot_core::session::{SessionState, SessionStateMachine};
use reshot_core::settings::{AppSettings, SettingsService};
use reshot_platform::shell::{BalloonIcon, HotkeySlot, Shell, ShellEvent};
use reshot_platform::single_instance::SingleInstance;
use reshot_platform::{autostart, win};

use crate::overlay::Overlay;
use crate::tray_menu::TrayMenu;
use crate::winutil;

/// Holding the hotkey longer than this opens the radial menu instead of capturing.
const HOLD_THRESHOLD: Duration = Duration::from_millis(250);
/// Hold-detection poll interval. It is pure added latency after the finger leaves the key, so
/// it is as short as is worth making; it only runs while the key is physically down.
const HOLD_POLL: Duration = Duration::from_millis(8);

pub struct ReshotApp {
    pub settings: SettingsService,
    shell: Shell,
    _single: SingleInstance,
    capture: Arc<ScreenCaptureService>,
    session: SessionStateMachine,
    paused: bool,
    main_vk: u32,
    hold_detecting: bool,
    /// Bumped by every press; a hold poll from an older press gives up.
    press_generation: u64,
    /// Bumped by every capture; a frame that comes back under an old number was superseded
    /// and is dropped, so a slow capture never opens a session on a screen the user left.
    capture_generation: u64,
    overlay: Option<WindowHandle<Overlay>>,
    tray_menu: Option<WindowHandle<TrayMenu>>,
}

impl Global for ReshotApp {}

/// What `main` prepared before gpui started: the shell is up, the hotkeys are registered and
/// the settings are loaded, so the tray works while no UI framework is loaded at all.
pub struct Resident {
    pub settings: SettingsService,
    pub shell: Shell,
    pub single: SingleInstance,
    pub main_vk: u32,
}

/// Registers the hotkeys on the shell and returns the main key's virtual-key code (0 when
/// it could not be parsed). Runs before gpui exists.
pub fn register_startup_hotkeys(shell: &Shell, s: &AppSettings) -> u32 {
    let vk = register_main(shell, &s.hotkey);
    register_audio(shell, &s.audio_hotkey);
    vk
}

fn register_main(shell: &Shell, text: &str) -> u32 {
    match HotkeyDefinition::parse(text) {
        Ok(def) => {
            if let Err(e) = shell.register_hotkey(HotkeySlot::Main, &def) {
                log::error!("{e}");
                shell.show_balloon(
                    "Reshot: hotkey unavailable",
                    &format!("Could not register '{text}'. It may be in use by another app. Pick another in Settings."),
                    BalloonIcon::Warning,
                );
            }
            def.virtual_key
        }
        Err(e) => {
            log::error!("Hotkey: cannot parse '{text}': {e}");
            0
        }
    }
}

fn register_audio(shell: &Shell, text: &str) {
    shell.unregister_hotkey(HotkeySlot::Audio);
    if text.trim().is_empty() {
        return;
    }
    match HotkeyDefinition::parse(text) {
        Ok(def) => {
            if let Err(e) = shell.register_hotkey(HotkeySlot::Audio, &def) {
                log::error!("{e}");
            }
        }
        Err(e) => log::error!("Audio hotkey: cannot parse '{text}': {e}"),
    }
}

pub fn init(
    cx: &mut App,
    resident: Resident,
    events: async_channel::Receiver<ShellEvent>,
    capture_now: bool,
    settings_now: Option<String>,
) {
    let Resident { settings, shell, single, main_vk } = resident;

    // gpui on Windows quits when its last window closes. A tray utility has no window most of
    // the time, so one invisible window anchors the process for its whole life.
    open_anchor(cx);
    crate::assets::register_fonts(cx);

    let app = ReshotApp {
        settings,
        shell,
        _single: single,
        capture: Arc::new(ScreenCaptureService::new()),
        session: SessionStateMachine::new(),
        paused: false,
        main_vk,
        hold_detecting: false,
        press_generation: 0,
        capture_generation: 0,
        overlay: None,
        tray_menu: None,
    };
    cx.set_global(app);

    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(event) = events.recv().await {
            if cx.update(|cx| on_shell_event(event, cx)).is_err() {
                break;
            }
        }
    })
    .detach();

    if capture_now {
        cx.spawn(async move |cx: &mut AsyncApp| {
            let _ = cx.update(|cx| capture(cx, "cli --capture"));
        })
        .detach();
    }
    // Debug builds: `--preview-track-prompt` opens the audio-track prompt with no recording
    // behind it, to look at; closing it decides nothing. `--preview-radial` opens the radial
    // menu at the cursor in its click mode, as if its key had been tapped.
    #[cfg(debug_assertions)]
    {
        let has = |flag: &str| std::env::args().any(|a| a.eq_ignore_ascii_case(flag));
        if has("--preview-track-prompt") {
            cx.spawn(async move |cx: &mut AsyncApp| {
                let _ = cx.update(crate::recording::preview_track_prompt);
            })
            .detach();
        }
        // `--bench-video` records a 1920x1080 area of the primary monitor and
        // `--bench-audio` records audio from the settings' sources, with no UI, to measure
        // what a recording costs. Stopped by killing the process.
        if has("--bench-video") {
            cx.spawn(async move |cx: &mut AsyncApp| {
                let _ = cx.update(|cx| {
                    let p = win::primary_monitor_bounds();
                    let rect = win::PxRect { left: p.left, top: p.top, right: p.left + 1920, bottom: p.top + 1080 };
                    crate::recording::start_video(cx, rect, None);
                });
            })
            .detach();
        }
        // `--bench-cycles` opens and closes the overlay eight times, 3 s each, to see
        // whether memory comes back after a session.
        if has("--bench-cycles") {
            cx.spawn(async move |cx: &mut AsyncApp| {
                for _ in 0..8 {
                    let _ = cx.update(|cx| capture(cx, "bench"));
                    cx.background_executor().timer(Duration::from_secs(3)).await;
                    let _ = cx.update(|cx| {
                        if let Some(h) = cx.global::<ReshotApp>().overlay {
                            let _ = h.update(cx, |o, window, cx| o.bench_close(window, cx));
                        }
                    });
                    cx.background_executor().timer(Duration::from_secs(3)).await;
                }
                log::info!("Bench: cycles done.");
            })
            .detach();
        }
        // `--bench-mem` walks the steps of a screenshot session 5 s apart, logging each, so
        // an outside sampler can tell which step leaves memory behind.
        if has("--bench-mem") {
            cx.spawn(async move |cx: &mut AsyncApp| {
                let wait = || Duration::from_secs(5);
                cx.background_executor().timer(wait()).await;
                log::info!("Bench: step 1, capture only.");
                let service = cx.update(|cx| cx.global::<ReshotApp>().capture.clone()).ok();
                if let Some(service) = service {
                    let frame = cx.background_executor().spawn(async move { service.snapshot_all_monitors() }).await;
                    drop(frame);
                }
                cx.background_executor().timer(wait()).await;
                log::info!("Bench: step 2, font catalogue.");
                reshot_core::fonts::warmup();
                cx.background_executor().timer(wait()).await;
                for n in 3..6 {
                    log::info!("Bench: step {n}, overlay open.");
                    let _ = cx.update(|cx| capture(cx, "bench"));
                    cx.background_executor().timer(wait()).await;
                    log::info!("Bench: step {n}, overlay closed.");
                    let _ = cx.update(|cx| {
                        if let Some(h) = cx.global::<ReshotApp>().overlay {
                            let _ = h.update(cx, |o, window, cx| o.bench_close(window, cx));
                        }
                    });
                    cx.background_executor().timer(wait()).await;
                }
                log::info!("Bench: done.");
            })
            .detach();
        }
        if has("--bench-audio") {
            cx.spawn(async move |cx: &mut AsyncApp| {
                let _ = cx.update(crate::recording::toggle_audio_from_settings);
            })
            .detach();
        }
        if has("--preview-recording-hud") {
            cx.spawn(async move |cx: &mut AsyncApp| {
                let _ = cx.update(crate::recording::preview_recording_hud);
            })
            .detach();
        }
        if has("--preview-radial") {
            cx.spawn(async move |cx: &mut AsyncApp| {
                let _ = cx.update(|cx| crate::radial::open(cx, 0));
            })
            .detach();
        }
    }
    if let Some(tab) = settings_now {
        cx.spawn(async move |cx: &mut AsyncApp| {
            let _ = cx.update(|cx| crate::settings_window::open_tab(cx, &tab));
        })
        .detach();
    }
    log::info!("UI started.");
}

fn open_anchor(cx: &mut App) {
    let mut opts = winutil::popup_options(winutil::logical(win::PxRect { left: -32000, top: -32000, right: -31999, bottom: -31999 }, 1.0), false);
    opts.show = false;
    if let Err(e) = cx.open_window(opts, |_, cx| cx.new(|_| Anchor)) {
        log::error!("Startup: could not open the anchor window: {e}");
    }
}

struct Anchor;

impl gpui::Render for Anchor {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        gpui::Empty
    }
}

impl ReshotApp {
    fn register_main_hotkey(&mut self, text: &str) {
        self.main_vk = register_main(&self.shell, text);
    }

    fn register_audio_hotkey(&mut self, text: &str) {
        register_audio(&self.shell, text);
    }
}

fn on_shell_event(event: ShellEvent, cx: &mut App) {
    match event {
        ShellEvent::Hotkey(HotkeySlot::Main) => on_hotkey(cx),
        ShellEvent::Hotkey(HotkeySlot::Audio) => crate::recording::toggle_audio_from_settings(cx),
        ShellEvent::TrayMenu => show_tray_menu(cx),
        ShellEvent::TrayCapture => capture(cx, "tray"),
        ShellEvent::SecondInstance => {
            log::info!("SingleInstance: another launch was redirected here.");
            balloon(cx, "Reshot is already running", "It lives in the tray.", BalloonIcon::Info);
        }
    }
}

/// Main hotkey pressed. A quick tap is a screenshot; holding it past [`HOLD_THRESHOLD`] opens
/// the radial menu. Telling them apart needs key-up, which RegisterHotKey does not give, so
/// the key is polled while it is held, and only then.
fn on_hotkey(cx: &mut App) {
    // The settings window is recording a new binding: the press is its input, not a capture.
    if crate::settings_window::is_capturing_hotkey(cx) {
        return;
    }
    let recording = crate::recording::is_active(cx);
    let app = cx.global::<ReshotApp>();
    // Logged before every guard below: each is a silent return, and this line tells "the
    // hotkey never arrived" apart from "it arrived and was swallowed".
    log::info!(
        "Hotkey pressed [overlay={}, holdDetecting={}, recording={recording}].",
        app.overlay.is_some(),
        app.hold_detecting,
    );
    // Recording: the hotkey is the Stop control, no menu.
    if recording {
        crate::recording::stop(cx);
        return;
    }
    if let Some(overlay) = app.overlay {
        // The overlay is already up: pull it back in front. A game that took the foreground
        // back is exactly when the user presses again.
        let _ = overlay.update(cx, |_, window, _| {
            if let Some(h) = winutil::hwnd(window) {
                win::force_foreground(h, true);
            }
            window.activate_window();
        });
        return;
    }
    if app.hold_detecting {
        return;
    }
    let vk = app.main_vk;
    if vk == 0 {
        capture(cx, "hotkey");
        return;
    }
    let generation = {
        let app = cx.global_mut::<ReshotApp>();
        app.hold_detecting = true;
        app.press_generation += 1;
        app.press_generation
    };
    // Nothing is captured until the press is known to be a tap: opening a capture session
    // takes the cursor off its hardware plane, one visible blink, and a hold never needs
    // a frame at all.
    let started = Instant::now();
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(HOLD_POLL).await;
            let down = win::is_key_down(vk);
            let held = started.elapsed() >= HOLD_THRESHOLD;
            if down && !held {
                continue;
            }
            let _ = cx.update(|cx| {
                let app = cx.global_mut::<ReshotApp>();
                if app.press_generation != generation {
                    return; // superseded by a newer press; that one owns the outcome
                }
                app.hold_detecting = false;
                if down {
                    log::info!("Hotkey held: radial menu.");
                    crate::radial::open(cx, vk);
                } else {
                    capture(cx, "hotkey tap");
                }
            });
            break;
        }
    })
    .detach();
}

/// Freezes every monitor on a worker thread, then opens the overlay. Device creation plus the
/// wait for the first frame is the bulk of hotkey-to-overlay time; on the UI thread it froze
/// the app instead of merely delaying it.
pub fn capture(cx: &mut App, source: &'static str) {
    // While recording, any capture request is the Stop control (SPEC §10).
    if crate::recording::is_active(cx) {
        log::info!("Stop recording via {source}.");
        crate::recording::stop(cx);
        return;
    }
    let app = cx.global_mut::<ReshotApp>();
    if let Some(overlay) = app.overlay {
        // Already up: bring it back in front instead of starting a second session.
        if let Ok(Some(h)) = overlay.update(cx, |_, window, _| winutil::hwnd(window)) {
            let raw = h.0 as isize;
            winutil::after_update(cx, move || {
                win::force_foreground(win::hwnd(raw), false);
            });
        }
        return;
    }
    if !app.session.try_transition(SessionState::Capturing) {
        return;
    }
    app.capture_generation += 1;
    let generation = app.capture_generation;
    let service = app.capture.clone();
    log::info!("Capture requested ({source}).");
    let started = Instant::now();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let result = cx.background_executor().spawn(async move { service.snapshot_all_monitors() }).await;
        let _ = cx.update(|cx| {
            let app = cx.global_mut::<ReshotApp>();
            if app.capture_generation != generation {
                log::info!("Capture: dropped a superseded frame.");
                return;
            }
            match result {
                Ok(frame) => {
                    log::info!("Capture: {}x{} in {} ms.", frame.width, frame.height, started.elapsed().as_millis());
                    let settings = app.settings.current().clone();
                    let handle = Overlay::open(frame, settings, cx);
                    let app = cx.global_mut::<ReshotApp>();
                    app.overlay = handle;
                    if app.overlay.is_some() {
                        app.session.try_transition(SessionState::Selecting);
                    } else {
                        app.session.reset();
                    }
                }
                Err(e) => {
                    log::error!("Capture failed: {e:#}");
                    app.session.reset();
                    app.shell.show_balloon("Reshot: capture failed", &format!("{e}"), BalloonIcon::Error);
                }
            }
        });
    })
    .detach();
}

pub fn overlay_closed(cx: &mut App) {
    let app = cx.global_mut::<ReshotApp>();
    app.overlay = None;
    app.session.reset();
}

fn show_tray_menu(cx: &mut App) {
    if let Some(menu) = cx.global::<ReshotApp>().tray_menu
        && menu.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return;
    }
    let paused = cx.global::<ReshotApp>().paused;
    let menu = TrayMenu::open(paused, cx);
    cx.global_mut::<ReshotApp>().tray_menu = menu;
}

pub fn set_paused(cx: &mut App, paused: bool) {
    let app = cx.global_mut::<ReshotApp>();
    app.paused = paused;
    if paused {
        app.shell.unregister_hotkey(HotkeySlot::Main);
        app.shell.show_balloon("Reshot", "Hotkey paused.", BalloonIcon::Info);
        log::info!("Hotkey paused by user.");
    } else {
        let hotkey = app.settings.current().hotkey.clone();
        app.register_main_hotkey(&hotkey);
        app.shell.show_balloon("Reshot", "Hotkey resumed.", BalloonIcon::Info);
        log::info!("Hotkey resumed by user.");
    }
}

/// Persists settings from the settings window and re-applies the ones that act live.
pub fn apply_settings(cx: &mut App, updated: AppSettings) {
    let app = cx.global_mut::<ReshotApp>();
    let old = app.settings.current().clone();
    app.settings.update(updated.clone());
    if !old.audio_hotkey.eq_ignore_ascii_case(&updated.audio_hotkey) {
        app.register_audio_hotkey(&updated.audio_hotkey);
    }
    // A paused hotkey stays paused; it picks up the new binding on resume.
    if !app.paused && !old.hotkey.eq_ignore_ascii_case(&updated.hotkey) {
        app.shell.unregister_hotkey(HotkeySlot::Main);
        app.register_main_hotkey(&updated.hotkey);
    }
    log::info!("Settings updated from the settings window.");

    if old.autostart == updated.autostart && old.autostart_elevated == updated.autostart_elevated {
        return;
    }
    // The elevated lane may raise UAC and wait on schtasks, so it runs off the UI thread.
    let (enabled, wanted) = (updated.autostart, updated.autostart_elevated);
    cx.spawn(async move |cx: &mut AsyncApp| {
        let elevated = cx.background_spawn(async move { autostart::apply(enabled, wanted, true) }).await;
        if elevated == wanted {
            return;
        }
        // The checkbox must not claim something Windows never agreed to.
        let _ = cx.update(|cx| {
            let app = cx.global_mut::<ReshotApp>();
            app.settings.current_mut().autostart_elevated = elevated;
            app.settings.save();
            if !elevated {
                app.shell.show_balloon(
                    "Reshot: starting normally",
                    "Administrator rights were not granted, so Reshot will start with Windows without them. Overlays over elevated applications will not work.",
                    BalloonIcon::Warning,
                );
            }
        });
    })
    .detach();
}

pub fn open_settings(cx: &mut App) {
    crate::settings_window::open(cx);
}

pub fn quit(cx: &mut App) {
    log::info!("Quit requested.");
    // A recording in flight is finalised rather than thrown away.
    crate::recording::shutdown(cx);
    cx.quit();
}

/// A tray balloon, for anything that has to tell the user something without a window.
pub fn balloon(cx: &App, title: &str, text: &str, icon: BalloonIcon) {
    cx.global::<ReshotApp>().shell.show_balloon(title, text, icon);
}
