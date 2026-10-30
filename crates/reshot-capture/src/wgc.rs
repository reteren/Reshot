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
    let surface = frame.Surface()?;
    let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
    let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
    d3d.read_texture(&texture)
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
