use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::{Ffmpeg, FfmpegArgs, ffmpeg::Process};

const FINALIZE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub bitrate: u32,
}

/// Streams tightly packed BGRA frames to ffmpeg. Odd dimensions are cropped to even dimensions.
pub struct Mp4VideoEncoder {
    output: PathBuf,
    video_path: PathBuf,
    audio_path: Option<PathBuf>,
    audio_file: Option<File>,
    audio: Option<AudioConfig>,
    process: Option<Process>,
    frame_bytes: usize,
    frames: u64,
    audio_frames: u64,
    finished: bool,
}

impl Mp4VideoEncoder {
    pub fn new(
        path: impl AsRef<Path>,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
        audio: Option<AudioConfig>,
    ) -> Result<Self> {
        let width = width & !1;
        let height = height & !1;
        if width < 2 || height < 2 || fps == 0 {
            bail!("video needs an even 2x2 block and positive fps");
        }
        let frame_bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .context("video dimensions too large")?;
        let output = path.as_ref().to_path_buf();
        let video_path = if audio.is_some() {
            temp_path("video.mp4")
        } else {
            output.clone()
        };
        let audio_path = audio.map(|_| temp_path("audio.pcm"));
        let audio_file = match audio_path.as_ref() {
            Some(path) => Some(File::create(path)?),
            None => None,
        };
        let encoder = Ffmpeg::select_h264_encoder(width, height)?;
        let args = FfmpegArgs::video(
            width,
            height,
            fps,
            bitrate,
            &encoder,
            &video_path.to_string_lossy(),
            FfmpegArgs::default_tuning(&encoder),
        );
        let process = Ffmpeg::start(&args, true)?;
        Ok(Self {
            output,
            video_path,
            audio_path,
            audio_file,
            audio,
            process: Some(process),
            frame_bytes,
            frames: 0,
            audio_frames: 0,
            finished: false,
        })
    }

    pub fn abort_handle(&self) -> Option<Arc<Mutex<Child>>> {
        self.process.as_ref().map(Process::abort_handle)
    }

    pub fn write_frame(&mut self, bgra: &[u8]) -> Result<()> {
        if self.finished {
            bail!("video encoder already finished");
        }
        if bgra.len() != self.frame_bytes {
            bail!(
                "expected {} BGRA bytes, got {}",
                self.frame_bytes,
                bgra.len()
            );
        }
        self.process
            .as_mut()
            .and_then(|p| p.stdin.as_mut())
            .context("video input pipe closed")?
            .write_all(bgra)?;
        self.frames += 1;
        Ok(())
    }

    pub fn write_audio(&mut self, pcm: &[u8]) -> Result<()> {
        if self.finished {
            bail!("video encoder already finished");
        }
        if let (Some(file), Some(config)) = (self.audio_file.as_mut(), self.audio) {
            file.write_all(pcm)?;
            self.audio_frames += pcm.len() as u64 / (config.channels as u64 * 2);
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        drop(self.audio_file.take());
        let (ok, stderr) = self
            .process
            .take()
            .context("video process missing")?
            .finish(FINALIZE_TIMEOUT)?;
        if !ok || !has_content(&self.video_path) {
            remove_file(&self.video_path);
            if let Some(path) = &self.audio_path {
                remove_file(path);
            }
            bail!("ffmpeg video failed after {} frames: {stderr}", self.frames);
        }
        if let Some(config) = self.audio {
            let paths = if self.audio_frames > 0 {
                vec![
                    self.audio_path
                        .as_ref()
                        .expect("audio path")
                        .to_string_lossy()
                        .to_string(),
                ]
            } else {
                Vec::new()
            };
            let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
            let args = FfmpegArgs::mux(
                &self.video_path.to_string_lossy(),
                &refs,
                &self.output.to_string_lossy(),
                config.sample_rate,
                config.channels,
                config.bitrate,
            );
            let muxed = Ffmpeg::run(&args);
            remove_file(&self.video_path);
            if let Some(path) = &self.audio_path {
                remove_file(path);
            }
            let (ok, stderr) = muxed?;
            if !ok || !has_content(&self.output) {
                remove_file(&self.output);
                bail!("ffmpeg audio mux failed: {stderr}");
            }
        }
        Ok(())
    }
}

impl Drop for Mp4VideoEncoder {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            log::warn!("video encoder finalization: {error:#}");
        }
    }
}

/// Encodes standalone 16-bit interleaved PCM to AAC/M4A.
pub struct M4aAudioEncoder {
    path: PathBuf,
    process: Option<Process>,
    bytes_per_frame: usize,
    frames: u64,
    finished: bool,
}

impl M4aAudioEncoder {
    pub fn new(
        path: impl AsRef<Path>,
        sample_rate: u32,
        channels: u16,
        bitrate: u32,
    ) -> Result<Self> {
        if sample_rate == 0 || channels == 0 {
            bail!("invalid audio format");
        }
        let path = path.as_ref().to_path_buf();
        let args = FfmpegArgs::audio(sample_rate, channels, bitrate, &path.to_string_lossy());
        let process = Ffmpeg::start(&args, true)?;
        Ok(Self {
            path,
            process: Some(process),
            bytes_per_frame: channels as usize * 2,
            frames: 0,
            finished: false,
        })
    }

    pub fn write_audio(&mut self, pcm: &[u8]) -> Result<()> {
        if self.finished {
            bail!("audio encoder already finished");
        }
        self.process
            .as_mut()
            .and_then(|p| p.stdin.as_mut())
            .context("audio input pipe closed")?
            .write_all(pcm)?;
        self.frames += (pcm.len() / self.bytes_per_frame) as u64;
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        let (ok, stderr) = self
            .process
            .take()
            .context("audio process missing")?
            .finish(FINALIZE_TIMEOUT)?;
        if !ok || !has_content(&self.path) {
            remove_file(&self.path);
            bail!("ffmpeg audio failed after {} frames: {stderr}", self.frames);
        }
        Ok(())
    }
}

impl Drop for M4aAudioEncoder {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            log::warn!("audio encoder finalization: {error:#}");
        }
    }
}

pub struct Mp4Muxer;

impl Mp4Muxer {
    /// Copy video without re-encoding; encode selected 48 kHz stereo PCM tracks to AAC.
    pub fn mux(
        video_only_mp4: impl AsRef<Path>,
        pcm_paths: &[PathBuf],
        output_path: impl AsRef<Path>,
    ) -> Result<bool> {
        let video = video_only_mp4.as_ref();
        let output = output_path.as_ref();
        let tracks: Vec<_> = pcm_paths.iter().filter(|path| path.is_file()).collect();
        let track_names: Vec<_> = tracks
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();
        let refs: Vec<_> = track_names.iter().map(String::as_str).collect();
        let args = FfmpegArgs::mux(
            &video.to_string_lossy(),
            &refs,
            &output.to_string_lossy(),
            48_000,
            2,
            160_000,
        );
        let (ok, stderr) = Ffmpeg::run(&args)?;
        if !ok || !has_content(output) {
            log::warn!("ffmpeg mux failed: {stderr}");
            if video != output {
                remove_file(output);
            }
            return Ok(false);
        }
        Ok(true)
    }
}

fn has_content(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.len() > 0)
}
pub(crate) fn remove_file(path: &Path) {
    if let Err(error) = fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!("could not remove {}: {error}", path.display());
    }
}

pub(crate) fn temp_path(suffix: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "reshot_{stamp:x}_{:x}_{suffix}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
