//! Recording orchestration lives apart from the app shell so the video and audio recorders,
//! the visible HUD, and the post-recording track choice share one lifetime.
// The app shell wires these entry points in the parallel integration pass.
#![allow(dead_code)]

mod corners;
mod hud;
mod track_prompt;

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use gpui::{App, AsyncApp, Global, WindowHandle};
use reshot_capture::ScreenCaptureService;
use reshot_core::settings::VideoCornersSettings;
use reshot_core::{filename, paths};
use reshot_platform::win::{self, PxRect};
use reshot_recording::{AudioRecorder, AudioSources, Ffmpeg, VideoRecorder};

use crate::app::ReshotApp;
use hud::HudWindows;
use track_prompt::{Decision, TrackPrompt};

#[derive(Default)]
struct RecordingState {
    active: Option<Active>,
    pending: HashMap<u64, PendingVideo>,
    serial: u64,
}
impl Global for RecordingState {}

enum Active {
    StartingVideo {
        serial: u64,
        stop_requested: bool,
        receiver: Receiver<anyhow::Result<VideoRecorder>>,
        rect: PxRect,
        corners: VideoCornersSettings,
    },
    StartingAudio {
        serial: u64,
        stop_requested: bool,
        receiver: Receiver<anyhow::Result<AudioRecorder>>,
    },
    Video {
        recorder: Box<VideoRecorder>,
        hud: Option<HudWindows>,
    },
    Audio {
        recorder: Box<AudioRecorder>,
        hud: Option<HudWindows>,
    },
}

struct PendingVideo {
    recorder: Box<VideoRecorder>,
    prompt: Option<WindowHandle<TrackPrompt>>,
}

fn state(cx: &mut App) -> &mut RecordingState {
    if !cx.has_global::<RecordingState>() {
        cx.set_global(RecordingState::default());
    }
    cx.global_mut::<RecordingState>()
}

pub fn is_active(cx: &App) -> bool {
    cx.try_global::<RecordingState>()
        .is_some_and(|s| s.active.is_some())
}

pub fn start_video(cx: &mut App, rect: PxRect, mask: Option<Vec<u8>>) {
    start_video_with(cx, rect, mask, None);
}

/// Overlay source picker variant; captures independent PCM tracks for the selected processes.
pub fn start_video_with(
    cx: &mut App,
    rect: PxRect,
    mask: Option<Vec<u8>>,
    sources: Option<AudioSources>,
) {
    if is_active(cx) {
        return;
    }
    if rect.width() <= 0 || rect.height() <= 0 {
        log::warn!("Recording: empty selection");
        return;
    }
    if !Ffmpeg::is_available() {
        log::error!("Recording unavailable: {}", Ffmpeg::missing_message());
        return;
    }

    let settings = cx.global::<ReshotApp>().settings.current().clone();
    let directory = if settings.paths.videos.trim().is_empty() {
        (*paths::DEFAULT_VIDEOS_DIR).clone()
    } else {
        PathBuf::from(&settings.paths.videos)
    };
    if let Err(error) = fs::create_dir_all(&directory) {
        log::error!("Recording folder: {error}");
        return;
    }
    let path = directory.join(filename::build(
        &settings.filename.template,
        "mp4",
        filename::now(),
    ));
    let width = rect.width() as u32;
    let height = rect.height() as u32;
    let fps = if settings.video.fps <= 0 {
        60
    } else {
        settings.video.fps as u32
    };
    let bitrate =
        ((width as u64 * height as u64 * fps as u64 * 8) / 100).clamp(2_000_000, 40_000_000) as u32;
    let sources = sources.unwrap_or_else(|| AudioSources {
        system_full: settings.video.audio.system,
        mic: settings.video.audio.mic,
        mic_device: Some(settings.audio.mic_device.clone()),
        include_pids: Vec::new(),
    });
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let capture = ScreenCaptureService::new();
        let result = VideoRecorder::new(
            &capture,
            rect.left,
            rect.top,
            width,
            height,
            fps,
            bitrate,
            &path,
            sources,
            mask,
            width as usize,
        );
        let _ = sender.send(result);
    });
    let serial = {
        let s = state(cx);
        s.serial += 1;
        s.active = Some(Active::StartingVideo {
            serial: s.serial,
            stop_requested: false,
            receiver,
            rect,
            corners: settings.video.corners,
        });
        s.serial
    };
    poll_start(cx, serial);
}

pub fn start_audio(cx: &mut App, sources: AudioSources) {
    if is_active(cx) {
        return;
    }
    if !Ffmpeg::is_available() {
        log::error!("Audio recording unavailable: {}", Ffmpeg::missing_message());
        return;
    }
    let settings = cx.global::<ReshotApp>().settings.current().clone();
    let directory = if settings.paths.records.trim().is_empty() {
        (*paths::DEFAULT_RECORDS_DIR).clone()
    } else {
        PathBuf::from(&settings.paths.records)
    };
    if let Err(error) = fs::create_dir_all(&directory) {
        log::error!("Audio recording folder: {error}");
        return;
    }
    let path = directory.join(filename::build(
        &settings.filename.template,
        "m4a",
        filename::now(),
    ));
    {
        let app = cx.global_mut::<ReshotApp>();
        app.settings.current_mut().audio.system = sources.system_full;
        app.settings.current_mut().audio.mic = sources.mic;
        app.settings.save();
    }
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(AudioRecorder::new(&path, &sources));
    });
    let serial = {
        let s = state(cx);
        s.serial += 1;
        s.active = Some(Active::StartingAudio {
            serial: s.serial,
            stop_requested: false,
            receiver,
        });
        s.serial
    };
    poll_start(cx, serial);
}

fn poll_start(cx: &mut App, serial: u64) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(20))
                .await;
            if cx.update(|cx| complete_start(cx, serial)).unwrap_or(true) {
                break;
            }
        }
    })
    .detach();
}

fn complete_start(cx: &mut App, serial: u64) -> bool {
    let active = state(cx).active.take();
    match active {
        Some(Active::StartingVideo {
            serial: id,
            stop_requested,
            receiver,
            rect,
            corners,
        }) if id == serial => match receiver.try_recv() {
            Ok(Ok(recorder)) if stop_requested => {
                let (system, mic) = (recorder.has_system_track, recorder.has_mic_track);
                finish_video(Box::new(recorder), system, mic);
            }
            Ok(Ok(recorder)) => {
                let region = corners.enabled.then_some(rect);
                let hud = HudWindows::open(
                    format!("{}×{}", recorder.width, recorder.height),
                    region,
                    &corners.color,
                    corners.opacity,
                    cx,
                );
                log::info!("Recording started: {}", recorder.path.display());
                state(cx).active = Some(Active::Video {
                    recorder: Box::new(recorder),
                    hud,
                });
            }
            Ok(Err(error)) => log::error!("Recording failed to start: {error:#}"),
            Err(TryRecvError::Disconnected) => {
                log::error!("Recording startup worker terminated unexpectedly")
            }
            Err(TryRecvError::Empty) => {
                state(cx).active = Some(Active::StartingVideo {
                    serial: id,
                    stop_requested,
                    receiver,
                    rect,
                    corners,
                });
                return false;
            }
        },
        Some(Active::StartingAudio {
            serial: id,
            stop_requested,
            receiver,
        }) if id == serial => match receiver.try_recv() {
            Ok(Ok(mut recorder)) if stop_requested => {
                if let Err(error) = recorder.stop() {
                    log::error!("Audio recording save failed: {error:#}");
                }
            }
            Ok(Ok(recorder)) => {
                let hud = HudWindows::open("Audio".into(), None, "#FF0000", 1.0, cx);
                log::info!("Audio recording started: {}", recorder.path.display());
                state(cx).active = Some(Active::Audio {
                    recorder: Box::new(recorder),
                    hud,
                });
            }
            Ok(Err(error)) => log::error!("Audio recording failed to start: {error:#}"),
            Err(TryRecvError::Disconnected) => {
                log::error!("Audio startup worker terminated unexpectedly")
            }
            Err(TryRecvError::Empty) => {
                state(cx).active = Some(Active::StartingAudio {
                    serial: id,
                    stop_requested,
                    receiver,
                });
                return false;
            }
        },
        other => {
            state(cx).active = other;
        }
    }
    true
}

pub fn stop(cx: &mut App) {
    let active = state(cx).active.take();
    match active {
        Some(Active::StartingVideo {
            serial,
            receiver,
            rect,
            corners,
            ..
        }) => {
            state(cx).active = Some(Active::StartingVideo {
                serial,
                stop_requested: true,
                receiver,
                rect,
                corners,
            })
        }
        Some(Active::StartingAudio {
            serial, receiver, ..
        }) => {
            state(cx).active = Some(Active::StartingAudio {
                serial,
                stop_requested: true,
                receiver,
            })
        }
        Some(Active::Video { mut recorder, hud }) => {
            if let Some(hud) = hud {
                hud.close(cx);
            }
            let _ = recorder.stop();
            let settings = cx.global::<ReshotApp>().settings.current().clone();
            let has_system = recorder.has_system_track;
            let has_mic = recorder.has_mic_track;
            if settings.video.audio.ask_on_save && (has_system || has_mic) {
                let id = {
                    let s = state(cx);
                    s.serial += 1;
                    s.serial
                };
                state(cx).pending.insert(
                    id,
                    PendingVideo {
                        recorder,
                        prompt: None,
                    },
                );
                let prompt = TrackPrompt::open(
                    id,
                    has_system,
                    has_mic,
                    settings.video.audio.system,
                    settings.video.audio.mic,
                    cx,
                );
                if let Some(prompt) = prompt {
                    if let Some(pending) = state(cx).pending.get_mut(&id) {
                        pending.prompt = Some(prompt);
                    }
                } else {
                    decide(
                        id,
                        Decision {
                            keep_system: has_system,
                            keep_mic: has_mic,
                            never_ask: false,
                        },
                        cx,
                    );
                }
            } else {
                finish_video(recorder, has_system, has_mic);
            }
        }
        Some(Active::Audio { mut recorder, hud }) => {
            if let Some(hud) = hud {
                hud.close(cx);
            }
            match recorder.stop() {
                Ok(()) => log::info!("Audio recording saved: {}", recorder.path.display()),
                Err(error) => log::error!("Audio recording save failed: {error:#}"),
            }
        }
        None => {}
    }
}

fn finish_video(mut recorder: Box<VideoRecorder>, keep_system: bool, keep_mic: bool) {
    match recorder.finish(keep_system, keep_mic) {
        Ok(true) => log::info!("Recording saved: {}", recorder.path.display()),
        Ok(false) => log::error!("Recording could not be saved: {}", recorder.path.display()),
        Err(error) => log::error!("Recording save failed: {error:#}"),
    }
}

fn decide(id: u64, decision: Decision, cx: &mut App) {
    let Some(pending) = state(cx).pending.remove(&id) else {
        return;
    };
    if decision.never_ask {
        let app = cx.global_mut::<ReshotApp>();
        app.settings.current_mut().video.audio.ask_on_save = false;
        app.settings.save();
    }
    finish_video(pending.recorder, decision.keep_system, decision.keep_mic);
}

/// Called by the app's exit path so a modeless track prompt cannot strand its take.
pub fn shutdown(cx: &mut App) {
    // A startup worker owns the recorder until its result arrives. Wait for it here: otherwise
    // quitting before WGC's first frame could discard a take that has already begun.
    match state(cx).active.take() {
        Some(Active::StartingVideo { receiver, .. }) => match receiver.recv() {
            Ok(Ok(recorder)) => {
                let (system, mic) = (recorder.has_system_track, recorder.has_mic_track);
                finish_video(Box::new(recorder), system, mic);
            }
            Ok(Err(error)) => log::error!("Recording startup on exit: {error:#}"),
            Err(error) => log::error!("Recording startup on exit: {error}"),
        },
        Some(Active::StartingAudio { receiver, .. }) => match receiver.recv() {
            Ok(Ok(mut recorder)) => {
                if let Err(error) = recorder.stop() {
                    log::error!("Audio save on exit: {error:#}");
                }
            }
            Ok(Err(error)) => log::error!("Audio startup on exit: {error:#}"),
            Err(error) => log::error!("Audio startup on exit: {error}"),
        },
        Some(Active::Video { mut recorder, hud }) => {
            if let Some(hud) = hud {
                hud.close(cx);
            }
            let (system, mic) = (recorder.has_system_track, recorder.has_mic_track);
            let _ = recorder.stop();
            finish_video(recorder, system, mic);
        }
        Some(Active::Audio { mut recorder, hud }) => {
            if let Some(hud) = hud {
                hud.close(cx);
            }
            if let Err(error) = recorder.stop() {
                log::error!("Audio save on exit: {error:#}");
            }
        }
        None => {}
    }
    let pending = std::mem::take(&mut state(cx).pending);
    for (_, take) in pending {
        if let Some(prompt) = take.prompt {
            let _ = prompt.update(cx, |_, window, _| window.remove_window());
        }
        let keep_system = take.recorder.has_system_track;
        let keep_mic = take.recorder.has_mic_track;
        finish_video(take.recorder, keep_system, keep_mic);
    }
}

pub fn quick_record_primary_monitor(cx: &mut App) {
    start_video(cx, win::primary_monitor_bounds(), None);
}

pub fn quick_audio_all_sources(cx: &mut App) {
    let mic_device = cx
        .global::<ReshotApp>()
        .settings
        .current()
        .audio
        .mic_device
        .clone();
    start_audio(
        cx,
        AudioSources {
            mic: true,
            mic_device: Some(mic_device),
            system_full: true,
            include_pids: Vec::new(),
        },
    );
}

pub fn toggle_audio_from_settings(cx: &mut App) {
    match cx
        .try_global::<RecordingState>()
        .and_then(|s| s.active.as_ref())
    {
        Some(Active::Audio { .. }) | Some(Active::StartingAudio { .. }) => stop(cx),
        Some(_) => {}
        None => quick_audio_all_sources(cx),
    }
}
