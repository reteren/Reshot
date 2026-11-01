//! Windows screen capture. Public frames contain owned, top-down BGRA8 pixels only.

mod dd;
mod native;
mod wgc;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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
    /// Desktop Duplication attempts that failed in a row, for a transient reason.
    duplication_misses: AtomicU32,
    /// Captures that went straight to WGC since duplication was last tried.
    duplication_skipped: AtomicU32,
}

/// After this many transient duplication failures in a row, most captures skip it: on some
/// set-ups (a still pointer, whose state duplication never reports) it fails every time,
/// and each attempt cost 40-150 ms before WGC took the frame anyway.
const MISSES_BEFORE_SKIPPING: u32 = 3;
/// While skipping, one capture in this many still tries duplication, so a set-up where it
/// works again is noticed.
const RETRY_EVERY: u32 = 10;

impl ScreenCaptureService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot_all_monitors(&self) -> anyhow::Result<CapturedFrame> {
        let monitors = native::enumerate_monitors()?;
        if !self.duplication_unsupported.load(Ordering::Relaxed) && self.try_duplication() {
            match dd::snapshot(&monitors) {
                Ok(frame) => {
                    self.duplication_misses.store(0, Ordering::Relaxed);
                    return Ok(frame);
                }
                Err(dd::DdFailure::Structural(reason)) => {
                    self.duplication_unsupported.store(true, Ordering::Relaxed);
                    log::warn!(
                        "Desktop Duplication unavailable on this layout ({reason}); using WGC"
                    );
                }
                Err(dd::DdFailure::Transient(reason)) => {
                    let misses = self.duplication_misses.fetch_add(1, Ordering::Relaxed) + 1;
                    if misses == MISSES_BEFORE_SKIPPING {
                        log::warn!(
                            "Desktop Duplication failed ({reason}) {misses} times in a row; using WGC, retrying it every {RETRY_EVERY} captures"
                        );
                    } else if misses < MISSES_BEFORE_SKIPPING {
                        log::warn!("Desktop Duplication failed ({reason}); using WGC for this capture");
                    }
                }
            }
        }
        wgc::snapshot(&monitors)
    }

    /// Whether this capture tries Desktop Duplication: always, until it has failed
    /// [`MISSES_BEFORE_SKIPPING`] times in a row; then once every [`RETRY_EVERY`] captures.
    fn try_duplication(&self) -> bool {
        if self.duplication_misses.load(Ordering::Relaxed) < MISSES_BEFORE_SKIPPING {
            return true;
        }
        let skipped = self.duplication_skipped.fetch_add(1, Ordering::Relaxed) + 1;
        if skipped >= RETRY_EVERY {
            self.duplication_skipped.store(0, Ordering::Relaxed);
            return true;
        }
        false
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplication_is_skipped_after_repeated_misses_but_still_retried() {
        let service = ScreenCaptureService::new();
        assert!(service.try_duplication());
        service.duplication_misses.store(MISSES_BEFORE_SKIPPING, Ordering::Relaxed);
        let tried: Vec<bool> = (0..2 * RETRY_EVERY).map(|_| service.try_duplication()).collect();
        assert_eq!(tried.iter().filter(|&&t| t).count(), 2);
        assert!(tried[RETRY_EVERY as usize - 1]);
        // A success resets the count: every capture tries it again.
        service.duplication_misses.store(0, Ordering::Relaxed);
        assert!(service.try_duplication() && service.try_duplication());
    }
}
