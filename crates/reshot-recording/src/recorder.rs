use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use reshot_capture::{MonitorStream, ScreenCaptureService};
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};

use crate::{
    AudioCaptureMixer, AudioSources, Mp4Muxer, Mp4VideoEncoder,
    audio::{CHANNELS, SAMPLE_RATE},
    encoder::{remove_file, temp_path},
};

const AUDIO_MAX_CHUNK_FRAMES: usize = 4800;

trait FrameWriter: Send {
    fn write_frame(&mut self, frame: &[u8]) -> Result<()>;
    fn finish(&mut self) -> Result<()>;
    fn abort(&self) -> Arc<dyn Fn() + Send + Sync>;
}

impl FrameWriter for Mp4VideoEncoder {
    fn write_frame(&mut self, frame: &[u8]) -> Result<()> {
        self.write_frame(frame)
    }
    fn finish(&mut self) -> Result<()> {
        self.finish()
    }
    fn abort(&self) -> Arc<dyn Fn() + Send + Sync> {
        let child = self.abort_handle();
        Arc::new(move || {
            if let Some(child) = &child {
                let _ = child.lock().map(|mut p| p.kill());
            }
        })
    }
}

type WriterFactory =
    Box<dyn FnOnce(&Path, u32, u32, u32, u32) -> Result<Box<dyn FrameWriter>> + Send>;

struct TimerPeriod {
    active: bool,
    end: fn(u32) -> u32,
}
impl TimerPeriod {
    fn raise() -> Self {
        Self::raise_with(
            |period| unsafe { timeBeginPeriod(period) },
            |period| unsafe { timeEndPeriod(period) },
        )
    }
    fn raise_with(begin: fn(u32) -> u32, end: fn(u32) -> u32) -> Self {
        let _ = begin(1);
        Self { active: true, end }
    }
    fn restore(&mut self) {
        if self.active {
            self.active = false;
            let _ = (self.end)(1);
        }
    }
}
impl Drop for TimerPeriod {
    fn drop(&mut self) {
        self.restore();
    }
}

struct ShapeMask {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    stride: usize,
}

/// Live WGC region capture into a video-only temporary MP4, with separate PCM tracks.
/// `finish` selects tracks after recording and copies H.264 into the final MP4.
pub struct VideoRecorder {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub has_system_track: bool,
    pub has_mic_track: bool,
    temp_video: PathBuf,
    system_pcm: Option<PathBuf>,
    mic_pcm: Option<PathBuf>,
    stop: Arc<AtomicBool>,
    abort_writer: Arc<dyn Fn() + Send + Sync>,
    video_worker: Option<JoinHandle<Result<()>>>,
    audio_worker: Option<JoinHandle<Result<()>>>,
    reaper_worker: Option<JoinHandle<()>>,
    timer: Option<TimerPeriod>,
    stopped: bool,
    worker_terminated: bool,
    finished: bool,
    cleanup_owned_by_reaper: bool,
    pub cooperative_timeout: Duration,
    pub drain_timeout: Duration,
    pub reaper_timeout: Duration,
}

impl VideoRecorder {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        capture: &ScreenCaptureService,
        rect_left: i32,
        rect_top: i32,
        rect_width: u32,
        rect_height: u32,
        fps: u32,
        bitrate: u32,
        path: impl AsRef<Path>,
        sources: AudioSources,
        shape_mask: Option<Vec<u8>>,
        mask_stride: usize,
    ) -> Result<Self> {
        Self::new_with_writer(
            capture,
            rect_left,
            rect_top,
            rect_width,
            rect_height,
            fps,
            bitrate,
            path,
            sources,
            shape_mask,
            mask_stride,
            Box::new(|path, w, h, fps, bitrate| {
                Ok(
                    Box::new(Mp4VideoEncoder::new(path, w, h, fps, bitrate, None)?)
                        as Box<dyn FrameWriter>,
                )
            }),
            TimerPeriod::raise(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_writer(
        capture: &ScreenCaptureService,
        rect_left: i32,
        rect_top: i32,
        rect_width: u32,
        rect_height: u32,
        fps: u32,
        bitrate: u32,
        path: impl AsRef<Path>,
        sources: AudioSources,
        shape_mask: Option<Vec<u8>>,
        mask_stride: usize,
        writer_factory: WriterFactory,
        timer: TimerPeriod,
    ) -> Result<Self> {
        if fps == 0 || rect_width == 0 || rect_height == 0 {
            bail!("invalid recording dimensions or fps");
        }
        let stream = capture.start_monitor_stream(rect_left, rect_top)?;
        let monitor = &stream.monitor;
        if monitor.width < 2 || monitor.height < 2 {
            bail!("monitor smaller than 2x2");
        }
        let crop_left = (rect_left - monitor.left).clamp(0, monitor.width as i32 - 2) as u32;
        let crop_top = (rect_top - monitor.top).clamp(0, monitor.height as i32 - 2) as u32;
        let width = (rect_width.min(monitor.width - crop_left) & !1).max(2);
        let height = (rect_height.min(monitor.height - crop_top) & !1).max(2);
        let mask = shape_mask.map(|bytes| ShapeMask {
            bytes,
            width: rect_width,
            height: rect_height,
            stride: if mask_stride > 0 {
                mask_stride
            } else {
                rect_width as usize
            },
        });

        let system_audio = if sources.system_full || !sources.include_pids.is_empty() {
            let mixer = AudioCaptureMixer::new(&AudioSources {
                system_full: sources.system_full,
                include_pids: sources.include_pids,
                ..AudioSources::default()
            });
            mixer.has_any_source().then_some(mixer)
        } else {
            None
        };
        let mic_audio = if sources.mic {
            let mixer = AudioCaptureMixer::new(&AudioSources {
                mic: true,
                mic_device: sources.mic_device,
                ..AudioSources::default()
            });
            mixer.has_any_source().then_some(mixer)
        } else {
            None
        };
        let has_system_track = system_audio.is_some();
        let has_mic_track = mic_audio.is_some();
        let temp_video = temp_path("video.mp4");
        let system_pcm = system_audio.as_ref().map(|_| temp_path("sys.pcm"));
        let mic_pcm = mic_audio.as_ref().map(|_| temp_path("mic.pcm"));
        let system_file = system_pcm.as_ref().map(File::create).transpose()?;
        let mic_file = mic_pcm.as_ref().map(File::create).transpose()?;
        let writer = writer_factory(&temp_video, width, height, fps, bitrate)?;
        let abort_writer = writer.abort();
        let stop = Arc::new(AtomicBool::new(false));
        let video_stop = Arc::clone(&stop);
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let video_worker = thread::Builder::new()
            .name("reshot-video".into())
            .spawn(move || {
                video_loop(
                    stream, writer, video_stop, started_tx, crop_left, crop_top, width, height,
                    fps, mask,
                )
            })?;
        let audio_worker = if system_audio.is_some() || mic_audio.is_some() {
            let audio_stop = Arc::clone(&stop);
            Some(
                thread::Builder::new()
                    .name("reshot-audio".into())
                    .spawn(move || {
                        audio_loop(
                            system_audio.zip(system_file),
                            mic_audio.zip(mic_file),
                            audio_stop,
                            started_rx,
                        )
                    })?,
            )
        } else {
            None
        };
        Ok(Self {
            path: path.as_ref().to_path_buf(),
            width,
            height,
            has_system_track,
            has_mic_track,
            temp_video,
            system_pcm,
            mic_pcm,
            stop,
            abort_writer,
            video_worker: Some(video_worker),
            audio_worker,
            reaper_worker: None,
            timer: Some(timer),
            stopped: false,
            worker_terminated: false,
            finished: false,
            cleanup_owned_by_reaper: false,
            cooperative_timeout: Duration::from_secs(1),
            drain_timeout: Duration::from_secs(5),
            reaper_timeout: Duration::from_secs(15),
        })
    }

    pub fn is_worker_terminated(&self) -> bool {
        self.worker_terminated
    }
    pub fn temp_video_path(&self) -> &Path {
        &self.temp_video
    }
    pub fn reaper_is_finished(&self) -> bool {
        self.reaper_worker
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
    }

    /// Stop capture and finalize temporary video and PCM. A hung writer is interrupted by
    /// killing its ffmpeg child, so the GUI never waits indefinitely on a full pipe.
    pub fn stop(&mut self) -> bool {
        if self.stopped {
            return self.worker_terminated;
        }
        self.stopped = true;
        self.stop.store(true, Ordering::Release);
        let mut clean = self.wait_workers(self.cooperative_timeout);
        if !clean {
            (self.abort_writer)();
            clean = self.wait_workers(self.drain_timeout);
        }
        self.worker_terminated = clean;
        if clean {
            self.join_workers();
        } else {
            self.start_reaper();
        }
        if let Some(mut timer) = self.timer.take() {
            timer.restore();
        }
        clean
    }

    fn wait_workers(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let video_done = self
                .video_worker
                .as_ref()
                .is_none_or(JoinHandle::is_finished);
            let audio_done = self
                .audio_worker
                .as_ref()
                .is_none_or(JoinHandle::is_finished);
            if video_done && audio_done {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn join_workers(&mut self) {
        if let Some(worker) = self.video_worker.take() {
            match worker.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => log::warn!("video worker: {error:#}"),
                Err(_) => log::warn!("video worker panicked"),
            }
        }
        if let Some(worker) = self.audio_worker.take() {
            match worker.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => log::warn!("audio worker: {error:#}"),
                Err(_) => log::warn!("audio worker panicked"),
            }
        }
    }

    fn start_reaper(&mut self) {
        let video = self.video_worker.take();
        let audio = self.audio_worker.take();
        let paths = [
            Some(self.temp_video.clone()),
            self.system_pcm.clone(),
            self.mic_pcm.clone(),
        ];
        let timeout = self.reaper_timeout;
        self.cleanup_owned_by_reaper = true;
        self.reaper_worker = Some(thread::spawn(move || {
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                if video.as_ref().is_none_or(JoinHandle::is_finished)
                    && audio.as_ref().is_none_or(JoinHandle::is_finished)
                {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
            if let Some(worker) = video
                && worker.is_finished()
            {
                let _ = worker.join();
            }
            if let Some(worker) = audio
                && worker.is_finished()
            {
                let _ = worker.join();
            }
            for path in paths.into_iter().flatten() {
                remove_file(&path);
            }
        }));
    }

    /// Choose audio tracks after stopping. Failed mux retains the silent video if possible.
    pub fn finish(&mut self, keep_system: bool, keep_mic: bool) -> Result<bool> {
        if self.finished {
            return Ok(self.path.is_file());
        }
        if !self.stop() {
            return Ok(false);
        }
        let tracks: Vec<PathBuf> = [
            keep_system.then(|| self.system_pcm.clone()).flatten(),
            keep_mic.then(|| self.mic_pcm.clone()).flatten(),
        ]
        .into_iter()
        .flatten()
        .collect();
        let mut ok = Mp4Muxer::mux(&self.temp_video, &tracks, &self.path)?;
        if !ok {
            // Keep at least the silent take if AAC/mux failed.
            ok = fs::copy(&self.temp_video, &self.path).is_ok_and(|size| size > 0);
        }
        self.cleanup_temps();
        self.finished = true;
        Ok(ok)
    }

    fn cleanup_temps(&self) {
        remove_file(&self.temp_video);
        if let Some(path) = &self.system_pcm {
            remove_file(path);
        }
        if let Some(path) = &self.mic_pcm {
            remove_file(path);
        }
    }
}

impl Drop for VideoRecorder {
    fn drop(&mut self) {
        self.stop();
        if !self.finished && !self.cleanup_owned_by_reaper {
            self.cleanup_temps();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn video_loop(
    stream: MonitorStream,
    mut writer: Box<dyn FrameWriter>,
    stop: Arc<AtomicBool>,
    started: mpsc::SyncSender<()>,
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    fps: u32,
    mask: Option<ShapeMask>,
) -> Result<()> {
    let mut buffer = vec![0u8; width as usize * height as usize * 4];
    let wait_deadline = Instant::now() + Duration::from_secs(3);
    while !stop.load(Ordering::Acquire)
        && !stream.copy_region(left, top, width, height, &mut buffer)?
    {
        if Instant::now() >= wait_deadline {
            let _ = started.send(());
            bail!("no video frame within three seconds");
        }
        thread::sleep(Duration::from_millis(4));
    }
    let _ = started.send(());
    if stop.load(Ordering::Acquire) {
        return writer.finish();
    }
    let clock = Instant::now();
    let mut frame_index = 0u64;
    while !stop.load(Ordering::Acquire) {
        if stream.copy_region(left, top, width, height, &mut buffer)? {
            if let Some(mask) = &mask {
                apply_mask(
                    &mut buffer,
                    width,
                    height,
                    Some(&mask.bytes),
                    mask.width,
                    mask.height,
                    mask.stride,
                );
            }
            writer.write_frame(&buffer)?;
        }
        frame_index += 1;
        let next = clock + Duration::from_secs_f64(frame_index as f64 / fps as f64);
        let delay = next.saturating_duration_since(Instant::now());
        if delay > Duration::from_millis(1) {
            thread::sleep(delay);
        }
    }
    writer.finish()
}

fn audio_loop(
    system: Option<(AudioCaptureMixer, File)>,
    mic: Option<(AudioCaptureMixer, File)>,
    stop: Arc<AtomicBool>,
    started: mpsc::Receiver<()>,
) -> Result<()> {
    let _ = started.recv();
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    if let Some((mixer, _)) = &system {
        mixer.flush();
    }
    if let Some((mixer, _)) = &mic {
        mixer.flush();
    }
    let mut system = system;
    let mut mic = mic;
    let mut pcm = vec![0u8; AUDIO_MAX_CHUNK_FRAMES * CHANNELS * 2];
    let clock = Instant::now();
    let mut written = 0usize;
    while !stop.load(Ordering::Acquire) {
        let target = (clock.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as usize;
        let needed = target.saturating_sub(written);
        if needed >= 480 {
            let frames = needed.min(AUDIO_MAX_CHUNK_FRAMES);
            if let Some((mixer, file)) = system.as_mut() {
                let bytes = mixer.read_pcm16(&mut pcm, frames);
                file.write_all(&pcm[..bytes]).context("write system PCM")?;
            }
            if let Some((mixer, file)) = mic.as_mut() {
                let bytes = mixer.read_pcm16(&mut pcm, frames);
                file.write_all(&pcm[..bytes])
                    .context("write microphone PCM")?;
            }
            written += frames;
        }
        thread::sleep(Duration::from_millis(8));
    }
    Ok(())
}

/// Black out pixels outside the shape. MP4/H.264 has no alpha. All dimensions are checked
/// against the actual buffer and mask to handle odd and one-pixel selections safely.
pub fn apply_mask(
    bgra: &mut [u8],
    crop_width: u32,
    crop_height: u32,
    mask: Option<&[u8]>,
    mask_width: u32,
    mask_height: u32,
    mask_stride: usize,
) {
    let Some(mask) = mask else {
        return;
    };
    let Some(pixels) = (crop_width as usize).checked_mul(crop_height as usize) else {
        return;
    };
    if crop_width == 0 || crop_height == 0 || bgra.len() < pixels.saturating_mul(4) {
        return;
    }
    let row_width = crop_width as usize;
    // Recording masks normally cover the crop; validate their bounds once before scanning rows.
    if mask_width >= crop_width
        && mask_height >= crop_height
        && mask_stride >= row_width
        && (crop_height as usize - 1)
            .checked_mul(mask_stride)
            .and_then(|base| base.checked_add(row_width))
            .is_some_and(|end| end <= mask.len())
    {
        for (y, row) in bgra
            .chunks_exact_mut(row_width * 4)
            .take(crop_height as usize)
            .enumerate()
        {
            let mask_row = &mask[y * mask_stride..y * mask_stride + row_width];
            for (pixel, &selected) in row.chunks_exact_mut(4).zip(mask_row) {
                if selected < 128 {
                    pixel.copy_from_slice(&[0, 0, 0, 255]);
                }
            }
        }
        return;
    }
    for y in 0..crop_height as usize {
        for x in 0..crop_width as usize {
            let selected = x < mask_width as usize
                && y < mask_height as usize
                && y.checked_mul(mask_stride)
                    .and_then(|base| base.checked_add(x))
                    .and_then(|index| mask.get(index))
                    .is_some_and(|&value| value >= 128);
            if !selected {
                bgra[(y * crop_width as usize + x) * 4..(y * crop_width as usize + x) * 4 + 4]
                    .copy_from_slice(&[0, 0, 0, 255]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Condvar, Mutex, atomic::AtomicUsize};

    #[test]
    #[ignore]
    fn bench_shape_mask() {
        use std::hint::black_box;
        #[inline(never)]
        fn baseline(bgra: &mut [u8], mask: &[u8], width: u32, height: u32, stride: usize) {
            for y in 0..height as usize {
                for x in 0..width as usize {
                    let selected = x < width as usize
                        && y < height as usize
                        && y.checked_mul(stride)
                            .and_then(|base| base.checked_add(x))
                            .and_then(|index| mask.get(index))
                            .is_some_and(|&value| value >= 128);
                    if !selected {
                        bgra[(y * width as usize + x) * 4..(y * width as usize + x) * 4 + 4]
                            .copy_from_slice(&[0, 0, 0, 255]);
                    }
                }
            }
        }
        let (width, height) = (1920u32, 1080u32);
        let mut frame = vec![127u8; width as usize * height as usize * 4];
        let mask = (0..width as usize * height as usize)
            .map(|i| if i % 3 == 0 { 0 } else { 255 })
            .collect::<Vec<_>>();
        let start = Instant::now();
        for _ in 0..100 {
            baseline(
                &mut frame,
                &mask,
                black_box(width),
                black_box(height),
                black_box(width as usize),
            );
            black_box(&frame);
        }
        println!(
            "mask baseline 1080p: {:.2} ms/frame",
            start.elapsed().as_secs_f64() * 10.0
        );
        let start = Instant::now();
        for _ in 0..100 {
            apply_mask(
                &mut frame,
                black_box(width),
                black_box(height),
                Some(&mask),
                black_box(width),
                black_box(height),
                black_box(width as usize),
            );
            black_box(&frame);
        }
        println!(
            "mask 1080p: {:.2} ms/frame",
            start.elapsed().as_secs_f64() * 10.0
        );
    }

    fn pixel(buffer: &[u8], index: usize) -> &[u8] {
        &buffer[index * 4..index * 4 + 4]
    }

    #[test]
    fn mask_one_by_one_blacks_padded_row_and_column() {
        let mut buffer = vec![255; 2 * 2 * 4];
        apply_mask(&mut buffer, 2, 2, Some(&[255]), 1, 1, 1);
        assert_eq!(pixel(&buffer, 0), [255; 4]);
        for index in 1..4 {
            assert_eq!(pixel(&buffer, index), [0, 0, 0, 255]);
        }
    }

    #[test]
    fn mask_one_by_n_and_n_by_one_do_not_overrun() {
        let mut columns = vec![255; 2 * 10 * 4];
        apply_mask(&mut columns, 2, 10, Some(&[255; 10]), 1, 10, 1);
        for y in 0..10 {
            assert_eq!(pixel(&columns, y * 2), [255; 4]);
            assert_eq!(pixel(&columns, y * 2 + 1), [0, 0, 0, 255]);
        }
        let mut rows = vec![255; 10 * 2 * 4];
        apply_mask(&mut rows, 10, 2, Some(&[255; 10]), 10, 1, 10);
        for x in 0..10 {
            assert_eq!(pixel(&rows, x), [255; 4]);
            assert_eq!(pixel(&rows, 10 + x), [0, 0, 0, 255]);
        }
    }

    #[test]
    fn mask_odd_dimensions_and_threshold_are_safe() {
        let mut buffer = vec![255; 4 * 4 * 4];
        apply_mask(&mut buffer, 4, 4, Some(&[255; 9]), 3, 3, 3);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(
                    pixel(&buffer, y * 4 + x),
                    if x < 3 && y < 3 {
                        &[255; 4]
                    } else {
                        &[0, 0, 0, 255]
                    }
                );
            }
        }
        let mut threshold = vec![255; 2 * 2 * 4];
        apply_mask(&mut threshold, 2, 2, Some(&[127, 128, 0, 255]), 2, 2, 2);
        assert_eq!(pixel(&threshold, 0), [0, 0, 0, 255]);
        assert_eq!(pixel(&threshold, 1), [255; 4]);
        assert_eq!(pixel(&threshold, 2), [0, 0, 0, 255]);
        assert_eq!(pixel(&threshold, 3), [255; 4]);
    }

    #[test]
    fn null_mask_and_undersized_buffer_are_noops() {
        let mut buffer = vec![255; 8];
        apply_mask(&mut buffer, 2, 2, Some(&[255; 4]), 2, 2, 2);
        apply_mask(&mut buffer, 2, 2, None, 2, 2, 2);
        assert_eq!(buffer, vec![255; 8]);
    }

    struct StubWriter {
        entered: Arc<AtomicBool>,
        disposed: Arc<AtomicBool>,
        gate: Option<Arc<(Mutex<bool>, Condvar)>>,
        abort_unblocks: bool,
    }

    impl FrameWriter for StubWriter {
        fn write_frame(&mut self, _: &[u8]) -> Result<()> {
            self.entered.store(true, Ordering::Release);
            if let Some(gate) = &self.gate {
                let (lock, signal) = &**gate;
                let mut open = lock.lock().expect("test gate poisoned");
                while !*open {
                    open = signal.wait(open).expect("test gate poisoned");
                }
                if self.abort_unblocks {
                    bail!("simulated closed pipe");
                }
            }
            Ok(())
        }
        fn finish(&mut self) -> Result<()> {
            self.disposed.store(true, Ordering::Release);
            Ok(())
        }
        fn abort(&self) -> Arc<dyn Fn() + Send + Sync> {
            let gate = self.gate.clone();
            let unblock = self.abort_unblocks;
            Arc::new(move || {
                if unblock && let Some(gate) = &gate {
                    *gate.0.lock().expect("test gate poisoned") = true;
                    gate.1.notify_all();
                }
            })
        }
    }

    impl Drop for StubWriter {
        fn drop(&mut self) {
            self.disposed.store(true, Ordering::Release);
        }
    }

    fn recorder_with_stub(
        gate: Option<Arc<(Mutex<bool>, Condvar)>>,
        abort_unblocks: bool,
        entered: Arc<AtomicBool>,
        disposed: Arc<AtomicBool>,
    ) -> Result<VideoRecorder> {
        let service = ScreenCaptureService::new();
        VideoRecorder::new_with_writer(
            &service,
            0,
            0,
            128,
            128,
            30,
            1_000_000,
            temp_path("shutdown-test.mp4"),
            AudioSources::default(),
            None,
            0,
            Box::new(move |path, _, _, _, _| {
                File::create(path)?;
                Ok(Box::new(StubWriter {
                    gate,
                    abort_unblocks,
                    entered,
                    disposed,
                }) as Box<dyn FrameWriter>)
            }),
            TimerPeriod::raise(),
        )
    }

    fn wait_for_flag(flag: &AtomicBool, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while !flag.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        flag.load(Ordering::Acquire)
    }

    #[test]
    fn stop_unblocks_blocked_writer_and_is_idempotent() -> Result<()> {
        let entered = Arc::new(AtomicBool::new(false));
        let disposed = Arc::new(AtomicBool::new(false));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let mut recorder = recorder_with_stub(
            Some(gate),
            true,
            Arc::clone(&entered),
            Arc::clone(&disposed),
        )?;
        recorder.cooperative_timeout = Duration::from_millis(50);
        recorder.drain_timeout = Duration::from_secs(2);
        assert!(wait_for_flag(&entered, Duration::from_secs(5)));
        let clock = Instant::now();
        assert!(recorder.stop());
        assert!(clock.elapsed() < Duration::from_secs(3));
        assert!(disposed.load(Ordering::Acquire));
        assert!(recorder.stop());
        Ok(())
    }

    #[test]
    fn failed_drain_refuses_mux_and_reaper_cleans_temp_after_worker_exits() -> Result<()> {
        let entered = Arc::new(AtomicBool::new(false));
        let disposed = Arc::new(AtomicBool::new(false));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let mut recorder = recorder_with_stub(
            Some(Arc::clone(&gate)),
            false,
            Arc::clone(&entered),
            disposed,
        )?;
        recorder.cooperative_timeout = Duration::from_millis(50);
        recorder.drain_timeout = Duration::from_millis(50);
        recorder.reaper_timeout = Duration::from_secs(5);
        let temp = recorder.temp_video_path().to_path_buf();
        assert!(wait_for_flag(&entered, Duration::from_secs(5)));
        assert!(!recorder.stop());
        assert!(!recorder.is_worker_terminated());
        assert!(!recorder.finish(false, false)?);
        *gate.0.lock().expect("test gate poisoned") = true;
        gate.1.notify_all();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !recorder.reaper_is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(recorder.reaper_is_finished());
        assert!(!temp.exists());
        Ok(())
    }

    #[test]
    fn reaper_timeout_returns_even_if_worker_never_unblocks() -> Result<()> {
        let entered = Arc::new(AtomicBool::new(false));
        let disposed = Arc::new(AtomicBool::new(false));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let mut recorder = recorder_with_stub(
            Some(Arc::clone(&gate)),
            false,
            Arc::clone(&entered),
            disposed,
        )?;
        recorder.cooperative_timeout = Duration::from_millis(20);
        recorder.drain_timeout = Duration::from_millis(20);
        recorder.reaper_timeout = Duration::from_millis(100);
        assert!(wait_for_flag(&entered, Duration::from_secs(5)));
        assert!(!recorder.stop());
        let deadline = Instant::now() + Duration::from_secs(2);
        while !recorder.reaper_is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(recorder.reaper_is_finished());
        *gate.0.lock().expect("test gate poisoned") = true;
        gate.1.notify_all();
        Ok(())
    }

    #[test]
    fn dispose_without_finish_removes_temp_file() -> Result<()> {
        let entered = Arc::new(AtomicBool::new(false));
        let disposed = Arc::new(AtomicBool::new(false));
        let recorder = recorder_with_stub(None, false, entered, disposed)?;
        let temp = recorder.temp_video_path().to_path_buf();
        assert!(temp.exists());
        drop(recorder);
        assert!(!temp.exists());
        Ok(())
    }

    #[test]
    fn clean_stop_latency_stays_under_fifty_milliseconds() -> Result<()> {
        let entered = Arc::new(AtomicBool::new(false));
        let disposed = Arc::new(AtomicBool::new(false));
        let mut recorder = recorder_with_stub(None, false, entered, Arc::clone(&disposed))?;
        thread::sleep(Duration::from_millis(100));
        let clock = Instant::now();
        assert!(recorder.stop());
        assert!(
            clock.elapsed() < Duration::from_millis(50),
            "clean stop took {:?}",
            clock.elapsed()
        );
        assert!(disposed.load(Ordering::Acquire));
        Ok(())
    }

    #[test]
    fn wgc_stream_drop_while_frames_arrive_is_bounded() -> Result<()> {
        let capture = ScreenCaptureService::new();
        for _ in 0..3 {
            let stream = capture.start_monitor_stream(0, 0)?;
            let deadline = Instant::now() + Duration::from_secs(3);
            while stream.latest_frame().is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            assert!(stream.latest_frame().is_some());
            let clock = Instant::now();
            drop(stream);
            assert!(clock.elapsed() < Duration::from_millis(500));
        }
        Ok(())
    }

    #[test]
    fn can_record_short_clip() -> Result<()> {
        assert!(crate::Ffmpeg::is_available(), "ffmpeg must be available");
        let output = temp_path("recording-live.mp4");
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
            AudioSources::default(),
            None,
            0,
        )?;
        thread::sleep(Duration::from_secs(2));
        assert!(recorder.finish(false, false)?);
        assert!(fs::metadata(&output)?.len() > 0);
        remove_file(&output);
        Ok(())
    }

    #[test]
    fn timer_period_restores_once_on_error_and_repeated_stop() {
        static BEGINS: AtomicUsize = AtomicUsize::new(0);
        static ENDS: AtomicUsize = AtomicUsize::new(0);
        fn begin(_: u32) -> u32 {
            BEGINS.fetch_add(1, Ordering::Relaxed);
            0
        }
        fn end(_: u32) -> u32 {
            ENDS.fetch_add(1, Ordering::Relaxed);
            0
        }
        BEGINS.store(0, Ordering::Relaxed);
        ENDS.store(0, Ordering::Relaxed);
        let service = ScreenCaptureService::new();
        let invalid = VideoRecorder::new_with_writer(
            &service,
            0,
            0,
            128,
            128,
            0,
            1_000_000,
            temp_path("invalid-timer.mp4"),
            AudioSources::default(),
            None,
            0,
            Box::new(|_, _, _, _, _| -> Result<Box<dyn FrameWriter>> {
                bail!("writer should not start")
            }),
            TimerPeriod::raise_with(begin, end),
        );
        assert!(invalid.is_err());
        assert_eq!(BEGINS.load(Ordering::Relaxed), 1);
        assert_eq!(ENDS.load(Ordering::Relaxed), 1);
        let entered = Arc::new(AtomicBool::new(false));
        let disposed = Arc::new(AtomicBool::new(false));
        let mut recorder = VideoRecorder::new_with_writer(
            &service,
            0,
            0,
            128,
            128,
            30,
            1_000_000,
            temp_path("valid-timer.mp4"),
            AudioSources::default(),
            None,
            0,
            Box::new(move |path, _, _, _, _| {
                File::create(path)?;
                Ok(Box::new(StubWriter {
                    gate: None,
                    abort_unblocks: false,
                    entered,
                    disposed,
                }) as Box<dyn FrameWriter>)
            }),
            TimerPeriod::raise_with(begin, end),
        )
        .expect("valid recorder should start");
        recorder.stop();
        recorder.stop();
        drop(recorder);
        assert_eq!(BEGINS.load(Ordering::Relaxed), 2);
        assert_eq!(ENDS.load(Ordering::Relaxed), 2);
    }
}
