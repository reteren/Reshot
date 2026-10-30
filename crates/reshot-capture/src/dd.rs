use std::time::{Duration, Instant};

use anyhow::Context;
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIDevice, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource,
};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
use windows::core::Interface;

use crate::{
    CapturedFrame,
    native::{D3d, Monitor, blit},
};

pub(crate) enum DdFailure {
    Structural(String),
    Transient(String),
}

struct Acquired<'a>(&'a IDXGIOutputDuplication);
impl Drop for Acquired<'_> {
    fn drop(&mut self) {
        let _ = unsafe { self.0.ReleaseFrame() };
    }
}

pub(crate) fn snapshot(monitors: &[Monitor]) -> std::result::Result<CapturedFrame, DdFailure> {
    let mut frame = CapturedFrame::empty(monitors).map_err(transient)?;
    let d3d = D3d::create().map_err(transient)?;
    let dxgi: IDXGIDevice = d3d.device.cast().map_err(transient)?;
    let adapter = unsafe { dxgi.GetAdapter() }.map_err(transient)?;
    let mut captured = vec![false; monitors.len()];
    let mut pointer = POINT::default();
    let pointer_known = unsafe { GetCursorPos(&mut pointer) }.is_ok();

    for index in 0.. {
        let output = match unsafe { adapter.EnumOutputs(index) } {
            Ok(output) => output,
            Err(_) => break,
        };
        let desc = unsafe { output.GetDesc() }.map_err(transient)?;
        if !desc.AttachedToDesktop.as_bool() {
            continue;
        }
        let Some((slot, monitor)) = monitors.iter().enumerate().find(|(_, m)| {
            m.info.left == desc.DesktopCoordinates.left && m.info.top == desc.DesktopCoordinates.top
        }) else {
            continue;
        };
        let output1: IDXGIOutput1 = output.cast().map_err(transient)?;
        let duplication = unsafe { output1.DuplicateOutput(&d3d.device) }.map_err(transient)?;
        let (width, height, pixels) =
            capture_output(&d3d, &duplication, monitor, pointer_known, pointer)?;
        blit(&pixels, width, height, &mut frame, monitor).map_err(transient)?;
        captured[slot] = true;
    }
    // An output on another adapter is invisible to this device. Never return a black monitor.
    if let Some(slot) = captured.iter().position(|&done| !done) {
        let m = &monitors[slot].info;
        return Err(DdFailure::Structural(format!(
            "adapter did not cover monitor at ({},{})",
            m.left, m.top
        )));
    }
    Ok(frame)
}

fn capture_output(
    d3d: &D3d,
    duplication: &IDXGIOutputDuplication,
    monitor: &Monitor,
    pointer_known: bool,
    pointer: POINT,
) -> std::result::Result<(u32, u32, Vec<u8>), DdFailure> {
    let deadline = Instant::now() + Duration::from_millis(400);
    let cursor_on_monitor = pointer_known && monitor.info.contains(pointer.x, pointer.y);
    let mut cursor_composited = None;
    loop {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        match unsafe { duplication.AcquireNextFrame(60, &mut info, &mut resource) } {
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT && Instant::now() < deadline => continue,
            Err(e) => return Err(transient(e)),
            Ok(()) => {}
        }
        let _acquired = Acquired(duplication);
        if cursor_on_monitor && info.LastMouseUpdateTime != 0 {
            // Visible=false means the pointer is baked into the desktop pixels.
            cursor_composited = Some(!info.PointerPosition.Visible.as_bool());
        }
        // The first acquire is often an empty handshake surface. Copying it makes a black still.
        if info.AccumulatedFrames == 0 {
            if Instant::now() < deadline {
                continue;
            }
            return Err(DdFailure::Transient(format!(
                "only empty duplication frames at ({},{})",
                monitor.info.left, monitor.info.top
            )));
        }
        if cursor_on_monitor && cursor_composited != Some(false) {
            return Err(DdFailure::Transient(format!(
                "cursor state not proven separate at ({},{})",
                monitor.info.left, monitor.info.top
            )));
        }
        let resource = resource
            .context("duplication surface was null")
            .map_err(transient)?;
        let texture: ID3D11Texture2D = resource.cast().map_err(transient)?;
        let result = d3d.read_texture(&texture).map_err(|e| {
            let message = e.to_string();
            if message.contains("not BGRA8") {
                DdFailure::Structural(message)
            } else {
                DdFailure::Transient(message)
            }
        })?;
        // A rotated output no longer matches its monitor rectangle; WGC normalizes it.
        if result.0 != monitor.info.width || result.1 != monitor.info.height {
            return Err(DdFailure::Structural(format!(
                "surface {}x{} differs from monitor {}x{}",
                result.0, result.1, monitor.info.width, monitor.info.height
            )));
        }
        return Ok(result);
    }
}

fn transient(error: impl std::fmt::Display) -> DdFailure {
    DdFailure::Transient(error.to_string())
}
