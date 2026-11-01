use std::sync::{
    Arc, Mutex, Once,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureAccess, GraphicsCaptureAccessKind,
    GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};
use windows::core::{IInspectable, Interface, factory};

use crate::{
    CapturedFrame, CapturedMonitor,
    native::{D3d, Monitor, blit},
    nv12::{Crop, Nv12Converter},
};

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_millis(1200);

/// A monitor-sized, top-down BGRA8 stream frame.
#[derive(Debug)]
pub struct MonitorFrame {
    pub pixels_bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
}

/// WGC stream. Frames arrive through a bounded channel; the latest frame is also available
/// for fixed-rate encoders that must repeat a display frame when the desktop is idle.
pub struct MonitorStream {
    pub monitor: CapturedMonitor,
    receiver: Receiver<Arc<MonitorFrame>>,
    latest: Arc<Mutex<Option<Arc<MonitorFrame>>>>,
    stop: Arc<AtomicBool>,
    wake: SyncSender<()>,
    worker: Option<JoinHandle<()>>,
}

impl MonitorStream {
    pub fn try_next_frame(&self) -> Option<Arc<MonitorFrame>> {
        self.receiver.try_recv().ok()
    }

    pub fn latest_frame(&self) -> Option<Arc<MonitorFrame>> {
        self.latest.lock().ok()?.clone()
    }

    /// Copy a monitor-relative crop into a tightly packed destination. Returns false until
    /// the first frame arrives. Bounds are checked to protect the encoder from stale geometry.
    pub fn copy_region(
        &self,
        left: u32,
        top: u32,
        width: u32,
        height: u32,
        destination: &mut [u8],
    ) -> Result<bool> {
        let Some(frame) = self.latest_frame() else {
            return Ok(false);
        };
        if left
            .checked_add(width)
            .is_none_or(|right| right > frame.width)
            || top
                .checked_add(height)
                .is_none_or(|bottom| bottom > frame.height)
        {
            bail!("crop outside monitor");
        }
        let row_bytes = width as usize * 4;
        if destination.len() != row_bytes * height as usize {
            bail!("crop destination size mismatch");
        }
        for row in 0..height as usize {
            let src = (top as usize + row) * frame.stride + left as usize * 4;
            destination[row * row_bytes..(row + 1) * row_bytes]
                .copy_from_slice(&frame.pixels_bgra[src..src + row_bytes]);
        }
        Ok(true)
    }
}

impl Drop for MonitorStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.wake.try_send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Nv12Frame {
    pixels: Vec<u8>,
}

/// Cropped NV12 frames from the video processor, with BGRA readback as a safe runtime fallback.
pub struct Nv12MonitorStream {
    pub monitor: CapturedMonitor,
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
    latest: Arc<Mutex<Option<Arc<Nv12Frame>>>>,
    stop: Arc<AtomicBool>,
    wake: SyncSender<()>,
    worker: Option<JoinHandle<()>>,
}

impl Nv12MonitorStream {
    pub fn copy_frame(&self, destination: &mut [u8]) -> Result<bool> {
        let Some(frame) = self.latest.lock().ok().and_then(|guard| guard.clone()) else {
            return Ok(false);
        };
        if destination.len() != frame.pixels.len() {
            bail!("NV12 destination size mismatch");
        }
        destination.copy_from_slice(&frame.pixels);
        Ok(true)
    }
}

impl Drop for Nv12MonitorStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.wake.try_send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(crate) fn start_nv12_stream(monitor: Monitor, crop: Crop) -> Result<Nv12MonitorStream> {
    if !GraphicsCaptureSession::IsSupported()? {
        bail!("Windows.Graphics.Capture is unsupported");
    }
    request_borderless_access();
    let info = monitor.info.clone();
    let handle = monitor.handle.0 as isize;
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let latest = Arc::new(Mutex::new(None));
    let stop = Arc::new(AtomicBool::new(false));
    let worker_latest = Arc::clone(&latest);
    let worker_stop = Arc::clone(&stop);
    let worker = thread::Builder::new()
        .name("reshot-wgc-nv12".into())
        .spawn(move || {
            // SAFETY: This worker owns its WinRT apartment until capture ends.
            let initialized = unsafe {
                windows::Win32::System::WinRT::RoInitialize(
                    windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
                )
            }
            .is_ok();
            let monitor = Monitor {
                handle: HMONITOR(handle as *mut _),
                info,
            };
            let _ = nv12_stream_worker(monitor, crop, worker_latest, worker_stop, ready_tx);
            if initialized {
                // SAFETY: RoInitialize succeeded on this same worker thread.
                unsafe { windows::Win32::System::WinRT::RoUninitialize() };
            }
        })?;
    match ready_rx.recv_timeout(FIRST_FRAME_TIMEOUT) {
        Ok(Ok(wake)) => Ok(Nv12MonitorStream {
            monitor: monitor.info,
            left: crop.left,
            top: crop.top,
            width: crop.width,
            height: crop.height,
            latest,
            stop,
            wake,
            worker: Some(worker),
        }),
        Ok(Err(message)) => {
            stop.store(true, Ordering::Release);
            let _ = worker.join();
            bail!("{message}")
        }
        Err(error) => {
            stop.store(true, Ordering::Release);
            let _ = worker.join();
            bail!("WGC NV12 stream startup: {error}")
        }
    }
}

fn nv12_stream_worker(
    monitor: Monitor,
    crop: Crop,
    latest: Arc<Mutex<Option<Arc<Nv12Frame>>>>,
    stop: Arc<AtomicBool>,
    ready: SyncSender<std::result::Result<SyncSender<()>, String>>,
) -> Result<()> {
    let setup = (|| -> Result<_> {
        let d3d = D3d::create()?;
        let converter = Nv12Converter::new(&d3d, monitor.info.width, monitor.info.height, crop)?;
        let item = create_item_monitor(monitor.handle)?;
        let (session, wake) = Session::new(&d3d, item, true)?;
        Ok((d3d, converter, session, wake))
    })();
    let (d3d, converter, session, wake) = match setup {
        Ok(value) => value,
        Err(error) => {
            let _ = ready.send(Err(format!("{error:#}")));
            return Err(error);
        }
    };
    ready.send(Ok(session.wake.clone())).ok();
    let mut converter = Some(converter);
    while !stop.load(Ordering::Acquire) {
        if wake.recv_timeout(Duration::from_millis(50)).is_err() {
            continue;
        }
        if stop.load(Ordering::Acquire) {
            break;
        }
        while let Ok(frame) = session.pool.TryGetNextFrame() {
            let result = read_texture(&frame).and_then(|texture| {
                if let Some(gpu) = &converter {
                    match gpu.convert(&d3d, &texture) {
                        Ok(pixels) => return Ok(pixels),
                        Err(error) => {
                            log::warn!("GPU NV12 conversion failed; using BGRA readback and CPU conversion: {error:#}");
                            converter = None;
                        }
                    }
                }
                let (width, height, bgra) = d3d.read_texture(&texture)?;
                bgra_to_nv12(&bgra, width, height, crop)
            });
            let _ = frame.Close();
            match result {
                Ok(pixels) => {
                    if let Ok(mut guard) = latest.lock() {
                        *guard = Some(Arc::new(Nv12Frame { pixels }));
                    }
                }
                Err(error) => log::warn!("WGC NV12 frame: {error:#}"),
            }
        }
    }
    Ok(())
}

fn bgra_to_nv12(bgra: &[u8], source_width: u32, source_height: u32, crop: Crop) -> Result<Vec<u8>> {
    if crop
        .left
        .checked_add(crop.width)
        .is_none_or(|x| x > source_width)
        || crop
            .top
            .checked_add(crop.height)
            .is_none_or(|y| y > source_height)
    {
        bail!("NV12 fallback crop exceeds WGC frame");
    }
    let (width, height) = (crop.width as usize, crop.height as usize);
    let mut out = vec![0; width * height * 3 / 2];
    let yuv = |pixel: &[u8]| {
        let (b, g, r) = (pixel[0] as f32, pixel[1] as f32, pixel[2] as f32);
        let y = 16.0 + 0.182586 * r + 0.614231 * g + 0.062007 * b;
        let u = 128.0 - 0.100644 * r - 0.338572 * g + 0.439216 * b;
        let v = 128.0 + 0.439216 * r - 0.398942 * g - 0.040274 * b;
        (y.round().clamp(16.0, 235.0) as u8, u, v)
    };
    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            let mut u_sum = 0.0;
            let mut v_sum = 0.0;
            for dy in 0..2 {
                for dx in 0..2 {
                    let source_index = ((crop.top as usize + y + dy) * source_width as usize
                        + crop.left as usize
                        + x
                        + dx)
                        * 4;
                    let (luma, u, v) = yuv(&bgra[source_index..source_index + 4]);
                    out[(y + dy) * width + x + dx] = luma;
                    u_sum += u;
                    v_sum += v;
                }
            }
            let uv_index = width * height + (y / 2) * width + x;
            out[uv_index] = (u_sum / 4.0).round().clamp(16.0, 240.0) as u8;
            out[uv_index + 1] = (v_sum / 4.0).round().clamp(16.0, 240.0) as u8;
        }
    }
    Ok(out)
}

pub(crate) fn start_stream(monitor: Monitor) -> Result<MonitorStream> {
    if !GraphicsCaptureSession::IsSupported()? {
        bail!("Windows.Graphics.Capture is unsupported");
    }
    request_borderless_access();
    let info = monitor.info.clone();
    let handle = monitor.handle.0 as isize;
    let (frames_tx, frames_rx) = mpsc::sync_channel(2);
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let latest = Arc::new(Mutex::new(None));
    let stop = Arc::new(AtomicBool::new(false));
    let worker_latest = Arc::clone(&latest);
    let worker_stop = Arc::clone(&stop);
    let worker = thread::spawn(move || {
        let initialized = unsafe {
            windows::Win32::System::WinRT::RoInitialize(
                windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
            )
        }
        .is_ok();
        let monitor = Monitor {
            handle: HMONITOR(handle as *mut _),
            info,
        };
        if let Err(error) = stream_worker(monitor, worker_latest, worker_stop, frames_tx, ready_tx)
        {
            log::warn!("WGC stream stopped: {error:#}");
        }
        if initialized {
            unsafe {
                windows::Win32::System::WinRT::RoUninitialize();
            }
        }
    });
    match ready_rx.recv_timeout(FIRST_FRAME_TIMEOUT) {
        Ok(Ok(wake)) => Ok(MonitorStream {
            monitor: monitor.info,
            receiver: frames_rx,
            latest,
            stop,
            wake,
            worker: Some(worker),
        }),
        Ok(Err(message)) => {
            stop.store(true, Ordering::Release);
            let _ = worker.join();
            bail!("{message}")
        }
        Err(error) => {
            stop.store(true, Ordering::Release);
            let _ = worker.join();
            bail!("WGC stream startup: {error}")
        }
    }
}

fn stream_worker(
    monitor: Monitor,
    latest: Arc<Mutex<Option<Arc<MonitorFrame>>>>,
    stop: Arc<AtomicBool>,
    frames: SyncSender<Arc<MonitorFrame>>,
    ready: SyncSender<std::result::Result<SyncSender<()>, String>>,
) -> Result<()> {
    let setup = (|| -> Result<_> {
        let d3d = D3d::create()?;
        let item = create_item_monitor(monitor.handle)?;
        let (session, wake) = Session::new(&d3d, item, true)?;
        Ok((d3d, session, wake))
    })();
    let (d3d, session, wake) = match setup {
        Ok(value) => value,
        Err(error) => {
            let _ = ready.send(Err(format!("{error:#}")));
            return Err(error);
        }
    };
    ready.send(Ok(session.wake.clone())).ok();
    while !stop.load(Ordering::Acquire) {
        if wake.recv_timeout(Duration::from_millis(50)).is_err() {
            continue;
        }
        if stop.load(Ordering::Acquire) {
            break;
        }
        while let Ok(frame) = session.pool.TryGetNextFrame() {
            let result = read_frame(&d3d, &frame);
            let _ = frame.Close();
            match result {
                Ok((width, height, pixels_bgra)) => {
                    let frame = Arc::new(MonitorFrame {
                        stride: width as usize * 4,
                        width,
                        height,
                        pixels_bgra,
                    });
                    if let Ok(mut guard) = latest.lock() {
                        *guard = Some(Arc::clone(&frame));
                    }
                    let _ = frames.try_send(frame);
                }
                Err(error) => log::warn!("WGC stream frame: {error:#}"),
            }
        }
    }
    Ok(())
}

pub(crate) fn snapshot(monitors: &[Monitor]) -> Result<CapturedFrame> {
    if !GraphicsCaptureSession::IsSupported()? {
        bail!("Windows.Graphics.Capture is unsupported");
    }
    request_borderless_access();
    let mut output = CapturedFrame::empty(monitors)?;
    let foreground = unsafe { GetForegroundWindow() };
    let d3d = D3d::create()?;
    for monitor in monitors {
        let captured =
            create_item_monitor(monitor.handle).and_then(|item| capture_one(&d3d, item, false));
        let (width, height, pixels) = match captured {
            Ok(frame) => frame,
            Err(error) => {
                log::warn!(
                    "monitor WGC failed at ({},{}): {error:#}; trying foreground window",
                    monitor.info.left,
                    monitor.info.top
                );
                capture_foreground(&d3d, foreground, monitor)
                    .with_context(|| format!("monitor capture failed: {error:#}"))?
            }
        };
        blit(&pixels, width, height, &mut output, monitor)?;
    }
    Ok(output)
}

fn capture_foreground(d3d: &D3d, hwnd: HWND, monitor: &Monitor) -> Result<(u32, u32, Vec<u8>)> {
    if hwnd.0.is_null() {
        bail!("no foreground window");
    }
    let mut rect = windows::Win32::Foundation::RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }?;
    let overlap_w = rect.right.min(monitor.info.right()) - rect.left.max(monitor.info.left);
    let overlap_h = rect.bottom.min(monitor.info.bottom()) - rect.top.max(monitor.info.top);
    if overlap_w < monitor.info.width as i32 / 2 || overlap_h < monitor.info.height as i32 / 2 {
        bail!("foreground window does not cover this monitor");
    }
    capture_one(d3d, create_item_window(hwnd)?, false)
}

fn capture_one(d3d: &D3d, item: GraphicsCaptureItem, cursor: bool) -> Result<(u32, u32, Vec<u8>)> {
    let (session, wake) = Session::new(d3d, item, cursor)?;
    let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || wake.recv_timeout(remaining).is_err() {
            bail!("first WGC frame timed out after 1.2 s");
        }
        if let Ok(frame) = session.pool.TryGetNextFrame() {
            let result = read_frame(d3d, &frame);
            let _ = frame.Close();
            return result;
        }
    }
}

fn read_frame(
    d3d: &D3d,
    frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
) -> Result<(u32, u32, Vec<u8>)> {
    d3d.read_texture(&read_texture(frame)?)
}

fn read_texture(
    frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
) -> Result<ID3D11Texture2D> {
    let surface = frame.Surface()?;
    let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
    // SAFETY: The WGC frame owns its Direct3D surface until it is closed.
    unsafe { access.GetInterface().map_err(Into::into) }
}

fn create_item_monitor(handle: HMONITOR) -> Result<GraphicsCaptureItem> {
    let interop: IGraphicsCaptureItemInterop =
        factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    unsafe { interop.CreateForMonitor(handle) }.context("CreateForMonitor")
}

fn create_item_window(handle: HWND) -> Result<GraphicsCaptureItem> {
    let interop: IGraphicsCaptureItemInterop =
        factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    unsafe { interop.CreateForWindow(handle) }.context("CreateForWindow")
}

struct Session {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    token: i64,
    wake: SyncSender<()>,
}

impl Session {
    fn new(d3d: &D3d, item: GraphicsCaptureItem, cursor: bool) -> Result<(Self, Receiver<()>)> {
        let dxgi: IDXGIDevice = d3d.device.cast()?;
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }?;
        let winrt: IDirect3DDevice = inspectable.cast()?;
        let size = item.Size()?;
        if size.Width <= 0 || size.Height <= 0 {
            bail!("WGC item has invalid dimensions");
        }
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )?;
        let session = pool.CreateCaptureSession(&item)?;
        if !cursor {
            session.SetIsCursorCaptureEnabled(false)?;
        }
        // Older builds have no border switch. On Windows 11 this suppresses the yellow border
        // after the borderless access grant has been requested.
        let _ = session.SetIsBorderRequired(false);
        let (sender, receiver) = mpsc::sync_channel(2);
        let callback_sender = sender.clone();
        let handler =
            TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |_, _| {
                let _ = callback_sender.try_send(());
                Ok(())
            });
        let token = pool.FrameArrived(&handler)?;
        if let Err(error) = session.StartCapture() {
            let _ = pool.RemoveFrameArrived(token);
            let _ = session.Close();
            let _ = pool.Close();
            return Err(error.into());
        }
        Ok((
            Self {
                pool,
                session,
                token,
                wake: sender,
            },
            receiver,
        ))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.pool.RemoveFrameArrived(self.token);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// Ask Windows once for borderless capture access. The grant persists for the process.
pub fn request_borderless_access() {
    static REQUEST: Once = Once::new();
    REQUEST.call_once(|| {
        thread::spawn(|| {
            let initialized = unsafe {
                windows::Win32::System::WinRT::RoInitialize(
                    windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
                )
            }
            .is_ok();
            if let Ok(request) =
                GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless)
            {
                let _ = request.get();
            }
            if initialized {
                unsafe {
                    windows::Win32::System::WinRT::RoUninitialize();
                }
            }
        });
    });
}
