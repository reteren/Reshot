//! Windows screen capture. Public frames contain owned, top-down BGRA8 pixels only.

mod dd;
mod native;
mod wgc;

use std::sync::atomic::{AtomicBool, Ordering};

pub use wgc::{MonitorFrame, MonitorStream, request_borderless_access};

/// A monitor rectangle in virtual-desktop (physical-pixel) coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedMonitor {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
    pub name: String,
}

impl CapturedMonitor {
    pub fn right(&self) -> i32 {
        self.left + self.width as i32
    }
    pub fn bottom(&self) -> i32 {
        self.top + self.height as i32
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right() && y >= self.top && y < self.bottom()
    }
}

/// One frozen, tightly packed BGRA8 virtual-desktop image.
#[derive(Clone, Debug)]
pub struct CapturedFrame {
    pub pixels_bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub virtual_left: i32,
    pub virtual_top: i32,
    pub monitors: Vec<CapturedMonitor>,
}

impl CapturedFrame {
    fn empty(monitors: &[native::Monitor]) -> anyhow::Result<Self> {
        let (left, top, width, height) = native::virtual_desktop()?;
        let stride = width as usize * 4;
        let len = stride
            .checked_mul(height as usize)
            .ok_or_else(|| anyhow::anyhow!("virtual desktop is too large"))?;
        Ok(Self {
            pixels_bgra: vec![0; len],
            width,
            height,
            stride,
            virtual_left: left,
            virtual_top: top,
            monitors: monitors.iter().map(|m| m.info.clone()).collect(),
        })
    }
}

/// DD-first still captures, with a remembered structural refusal. Stream capture always uses WGC.
#[derive(Default)]
pub struct ScreenCaptureService {
    duplication_unsupported: AtomicBool,
}

impl ScreenCaptureService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot_all_monitors(&self) -> anyhow::Result<CapturedFrame> {
        let monitors = native::enumerate_monitors()?;
        if !self.duplication_unsupported.load(Ordering::Relaxed) {
            match dd::snapshot(&monitors) {
                Ok(frame) => return Ok(frame),
                Err(dd::DdFailure::Structural(reason)) => {
                    self.duplication_unsupported.store(true, Ordering::Relaxed);
                    log::warn!(
                        "Desktop Duplication unavailable on this layout ({reason}); using WGC"
                    );
                }
                Err(dd::DdFailure::Transient(reason)) => {
                    log::warn!("Desktop Duplication failed ({reason}); using WGC for this capture");
                }
            }
        }
        wgc::snapshot(&monitors)
    }

    /// Start a stream on the monitor containing the point, or the primary monitor.
    pub fn start_monitor_stream(
        &self,
        screen_x: i32,
        screen_y: i32,
    ) -> anyhow::Result<MonitorStream> {
        let monitors = native::enumerate_monitors()?;
        let target = monitors
            .iter()
            .find(|m| m.info.contains(screen_x, screen_y))
            .or_else(|| monitors.iter().find(|m| m.info.is_primary))
            .or_else(|| monitors.first())
            .ok_or_else(|| anyhow::anyhow!("no monitor found for recording"))?;
        wgc::start_stream(target.clone())
    }
}
