use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use wasapi::{
    AudioClient, DeviceCollection, Direction, SampleType, StreamMode, WaveFormat,
    get_default_device, initialize_mta,
};

use crate::{AudioSources, M4aAudioEncoder};

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
const MAX_BUFFER_SAMPLES: usize = SAMPLE_RATE as usize * CHANNELS * 10;

#[derive(Clone)]
enum SourceKind {
    System,
    Microphone(Option<String>),
    Process(u32),
}

struct Source {
    samples: Arc<Mutex<VecDeque<f32>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for Source {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// 48 kHz stereo float mixer backed by independent WASAPI streams. Silence is returned when
/// a source is idle, so an encoder's fixed-rate pull always advances the PCM clock.
pub struct AudioCaptureMixer {
    sources: Vec<Source>,
}

impl AudioCaptureMixer {
    pub fn new(config: &AudioSources) -> Self {
        let mut kinds = Vec::new();
        if config.system_full {
            kinds.push(SourceKind::System);
        } else {
            kinds.extend(config.include_pids.iter().copied().map(SourceKind::Process));
        }
        if config.mic {
            kinds.push(SourceKind::Microphone(config.mic_device.clone()));
        }
        let mut sources = Vec::new();
        for kind in kinds {
            match start_source(kind) {
                Ok(source) => sources.push(source),
                Err(error) => log::warn!("audio source unavailable: {error:#}"),
            }
        }
        Self { sources }
    }

    pub fn has_any_source(&self) -> bool {
        !self.sources.is_empty()
    }

    pub fn flush(&self) {
        for source in &self.sources {
            if let Ok(mut queue) = source.samples.lock() {
                queue.clear();
            }
        }
    }

    /// Write up to `frames` interleaved PCM16 frames, limited by `dest` capacity.
    pub fn read_pcm16(&self, dest: &mut [u8], frames: usize) -> usize {
        let samples = frames.saturating_mul(CHANNELS).min(dest.len() / 2);
        if samples == 0 {
            return 0;
        }
        let mut mix = vec![0.0f32; samples];
        for source in &self.sources {
            if let Ok(mut queue) = source.samples.lock() {
                for sample in &mut mix {
                    *sample += queue.pop_front().unwrap_or(0.0);
                }
            }
        }
        for (sample, out) in mix.into_iter().zip(dest.chunks_exact_mut(2)) {
            out.copy_from_slice(
                &((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes(),
            );
        }
        samples * 2
    }
}

fn start_source(kind: SourceKind) -> Result<Source> {
    let samples = Arc::new(Mutex::new(VecDeque::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let thread_samples = Arc::clone(&samples);
    let thread_stop = Arc::clone(&stop);
    let worker = thread::Builder::new()
        .name("reshot-wasapi".into())
        .spawn(move || {
            let _ = initialize_mta();
            if let Err(error) = capture_source(kind, thread_samples, thread_stop, ready_tx) {
                log::warn!("WASAPI capture: {error:#}");
            }
        })?;
    let source = Source {
        samples,
        stop,
        worker: Some(worker),
    };
    match ready_rx.recv_timeout(Duration::from_millis(3500)) {
        Ok(Ok(())) => Ok(source),
        Ok(Err(error)) => anyhow::bail!("{error}"),
        Err(error) => anyhow::bail!("WASAPI startup timed out: {error}"),
    }
}

fn capture_source(
    kind: SourceKind,
    samples: Arc<Mutex<VecDeque<f32>>>,
    stop: Arc<AtomicBool>,
    ready: mpsc::SyncSender<std::result::Result<(), String>>,
) -> Result<()> {
    let setup = (|| -> Result<_> {
        let mut client = match kind {
            SourceKind::System => get_default_device(&Direction::Render)?.get_iaudioclient()?,
            SourceKind::Microphone(id) => microphone_device(id.as_deref())?.get_iaudioclient()?,
            SourceKind::Process(pid) => AudioClient::new_application_loopback_client(pid, true)?,
        };
        let format = WaveFormat::new(
            32,
            32,
            &SampleType::Float,
            SAMPLE_RATE as usize,
            CHANNELS,
            None,
        );
        client.initialize_client(
            &format,
            &Direction::Capture,
            &StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: 0,
            },
        )?;
        let event = client.set_get_eventhandle()?;
        let capture = client.get_audiocaptureclient()?;
        client.start_stream()?;
        Ok((client, event, capture))
    })();
    let (client, event, capture) = match setup {
        Ok(value) => value,
        Err(error) => {
            let _ = ready.send(Err(format!("{error:#}")));
            return Err(error);
        }
    };
    let _ = ready.send(Ok(()));
    let mut bytes = VecDeque::new();
    while !stop.load(Ordering::Acquire) {
        let _ = event.wait_for_event(50);
        while capture.get_next_packet_size()?.unwrap_or(0) > 0 {
            capture.read_from_device_to_deque(&mut bytes)?;
        }
        if bytes.is_empty() {
            continue;
        }
        if let Ok(mut output) = samples.lock() {
            while bytes.len() >= 4 {
                let raw = [
                    bytes.pop_front().unwrap_or(0),
                    bytes.pop_front().unwrap_or(0),
                    bytes.pop_front().unwrap_or(0),
                    bytes.pop_front().unwrap_or(0),
                ];
                output.push_back(f32::from_le_bytes(raw));
            }
            let overflow = output.len().saturating_sub(MAX_BUFFER_SAMPLES);
            if overflow > 0 {
                output.drain(..overflow);
            }
        }
    }
    client.stop_stream()?;
    Ok(())
}

fn microphone_device(id: Option<&str>) -> Result<wasapi::Device> {
    if let Some(id) = id.filter(|id| !id.is_empty() && *id != "default") {
        for device in &DeviceCollection::new(&Direction::Capture)? {
            let device = device?;
            if device.get_id()? == id {
                return Ok(device);
            }
        }
    }
    Ok(get_default_device(&Direction::Capture)?)
}

pub fn list_microphones() -> Vec<(String, String)> {
    let _ = initialize_mta();
    let mut result = Vec::new();
    if let Ok(devices) = DeviceCollection::new(&Direction::Capture) {
        for device in (&devices).into_iter().flatten() {
            if let (Ok(id), Ok(name)) = (device.get_id(), device.get_friendlyname()) {
                result.push((id, name));
            }
        }
    }
    result
}

/// Standalone M4A capture of selected WASAPI sources.
pub struct AudioRecorder {
    pub path: PathBuf,
    pub has_source: bool,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<()>>>,
}

impl AudioRecorder {
    pub fn new(path: impl AsRef<Path>, sources: &AudioSources) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mixer = AudioCaptureMixer::new(sources);
        let has_source = mixer.has_any_source();
        let encoder = M4aAudioEncoder::new(&path, SAMPLE_RATE, CHANNELS as u16, 192_000)?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("reshot-audiorec".into())
            .spawn(move || audio_loop(mixer, encoder, thread_stop))?;
        Ok(Self {
            path,
            has_source,
            stop,
            worker: Some(worker),
        })
    }

    pub fn stop(&mut self) -> Result<()> {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("audio worker panicked"))??;
        }
        Ok(())
    }
}

impl Drop for AudioRecorder {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            log::warn!("audio recorder shutdown: {error:#}");
        }
    }
}

fn audio_loop(
    mixer: AudioCaptureMixer,
    mut encoder: M4aAudioEncoder,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let mut pcm = vec![0u8; 4800 * CHANNELS * 2];
    mixer.flush();
    let clock = Instant::now();
    let mut written = 0usize;
    while !stop.load(Ordering::Acquire) {
        let target = (clock.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as usize;
        let needed = target.saturating_sub(written);
        if needed >= 480 {
            let count = needed.min(4800);
            let bytes = mixer.read_pcm16(&mut pcm, count);
            encoder
                .write_audio(&pcm[..bytes])
                .context("writing standalone audio")?;
            written += count;
        }
        thread::sleep(Duration::from_millis(8));
    }
    encoder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_read_clamps_to_undersized_destination() {
        let mixer = AudioCaptureMixer::new(&AudioSources::default());
        let mut output = [255u8; 10];
        assert_eq!(mixer.read_pcm16(&mut output, 100), 10);
        assert_eq!(output, [0; 10]);
    }

    #[test]
    fn pcm_read_with_zero_or_odd_destination_does_not_overflow() {
        let mixer = AudioCaptureMixer::new(&AudioSources::default());
        assert_eq!(mixer.read_pcm16(&mut [], 100), 0);
        assert_eq!(mixer.read_pcm16(&mut [0; 1], 100), 0);
        assert_eq!(mixer.read_pcm16(&mut [0; 100], 0), 0);
    }

    #[test]
    fn process_loopback_opens_for_current_process() {
        let mixer = AudioCaptureMixer::new(&AudioSources {
            include_pids: vec![std::process::id()],
            ..AudioSources::default()
        });
        assert!(mixer.has_any_source());
    }

    #[test]
    fn standalone_audio_encodes_m4a() -> Result<()> {
        let output = crate::encoder::temp_path("audio-test.m4a");
        let mut recorder = AudioRecorder::new(
            &output,
            &AudioSources {
                system_full: true,
                ..AudioSources::default()
            },
        )?;
        assert!(recorder.has_source);
        thread::sleep(Duration::from_secs(1));
        recorder.stop()?;
        assert!(std::fs::metadata(&output)?.len() > 0);
        crate::encoder::remove_file(&output);
        Ok(())
    }
}
