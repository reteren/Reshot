use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use reshot_capture::ScreenCaptureService;
use reshot_recording::{AudioSources, Ffmpeg, VideoRecorder};

fn main() -> Result<()> {
    if !Ffmpeg::is_available() {
        bail!("{}", Ffmpeg::missing_message());
    }
    let mut args = std::env::args_os().skip(1);
    let output = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("recording.mp4"));
    let system_audio = args.next().is_some_and(|arg| arg == "system");
    let capture = ScreenCaptureService::new();
    let mut recorder = VideoRecorder::new(
        &capture,
        0,
        0,
        1280,
        720,
        30,
        4_000_000,
        &output,
        AudioSources {
            system_full: system_audio,
            ..AudioSources::default()
        },
        None,
        0,
    )?;
    std::thread::sleep(Duration::from_secs(3));
    if !recorder.finish(system_audio, false)? {
        bail!("recording did not produce an MP4");
    }
    println!(
        "{}x{} -> {}",
        recorder.width,
        recorder.height,
        output.display()
    );
    Ok(())
}
