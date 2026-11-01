//! Video and audio recording, ffmpeg encoding, and Windows WASAPI sources.

mod args;
pub use args::video_bitrate;
mod audio;
mod encoder;
mod ffmpeg;
mod recorder;
mod sources;

pub use args::{FfmpegArgs, H264Tuning};
pub use audio::{AudioCaptureMixer, AudioRecorder, list_microphones};
pub use encoder::{AudioConfig, M4aAudioEncoder, Mp4Muxer, Mp4VideoEncoder};
pub use ffmpeg::Ffmpeg;
pub use recorder::{VideoRecorder, apply_mask};
pub use sources::{AudioSources, WindowAudioSource, list_window_audio_sources};
