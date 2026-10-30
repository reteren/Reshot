use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, MONITORINFOF_PRIMARY, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::CapturedMonitor;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

#[derive(Clone)]
pub(crate) struct Monitor {
    pub handle: HMONITOR,
    pub info: CapturedMonitor,
}

pub(crate) fn enumerate_monitors() -> Result<Vec<Monitor>> {
    unsafe extern "system" fn callback(
        hmon: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> windows::core::BOOL {
        let monitors = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
        let mut details = MONITORINFOEXW::default();
        details.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(hmon, &mut details.monitorInfo) }.as_bool() {
            let rect = details.monitorInfo.rcMonitor;
            let name_end = details
                .szDevice
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(details.szDevice.len());
            monitors.push(Monitor {
                handle: hmon,
                info: CapturedMonitor {
                    left: rect.left,
                    top: rect.top,
                    width: (rect.right - rect.left) as u32,
                    height: (rect.bottom - rect.top) as u32,
                    is_primary: details.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                    name: String::from_utf16_lossy(&details.szDevice[..name_end]),
                },
            });
        }
        true.into()
    }
    let mut monitors = Vec::new();
    let data = LPARAM((&mut monitors as *mut Vec<Monitor>) as isize);
    if !unsafe { EnumDisplayMonitors(None, None, Some(callback), data) }.as_bool() {
        bail!("EnumDisplayMonitors failed");
    }
    if monitors.is_empty() {
        bail!("no monitors found");
    }
    Ok(monitors)
}

pub(crate) fn virtual_desktop() -> Result<(i32, i32, u32, u32)> {
    let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if width <= 0 || height <= 0 {
        bail!("invalid virtual desktop dimensions {width}x{height}");
    }
    Ok((
        left,
        top,
        width.try_into().context("desktop width")?,
        height.try_into().context("desktop height")?,
    ))
}

/// Copy pitched mapped GPU rows into a tightly described CPU image.
pub(crate) unsafe fn copy_rows(
    source: *const u8,
    source_stride: usize,
    destination: &mut [u8],
    destination_offset: usize,
    destination_stride: usize,
    row_bytes: usize,
    rows: usize,
) -> Result<()> {
    if source.is_null() {
        bail!("mapped GPU surface is null");
    }
    for row in 0..rows {
        let start = destination_offset
            .checked_add(
                row.checked_mul(destination_stride)
                    .context("destination row overflow")?,
            )
            .context("destination offset overflow")?;
        let end = start
            .checked_add(row_bytes)
            .context("destination row length overflow")?;
        let target = destination
            .get_mut(start..end)
            .context("GPU frame exceeds destination")?;
        unsafe {
            std::ptr::copy_nonoverlapping(
                source.add(row * source_stride),
                target.as_mut_ptr(),
                row_bytes,
            );
        }
    }
    Ok(())
}

pub(crate) struct D3d {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
}

impl D3d {
    pub fn create() -> Result<Self> {
        let levels = [
            D3D_FEATURE_LEVEL_11_1,
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0,
        ];
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .context("D3D11CreateDevice")?;
        Ok(Self {
            device: device.context("D3D11 device is null")?,
            context: context.context("D3D11 context is null")?,
        })
    }

    pub fn read_texture(&self, texture: &ID3D11Texture2D) -> Result<(u32, u32, Vec<u8>)> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            texture.GetDesc(&mut desc);
        }
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            bail!("capture surface is not BGRA8: {:?}", desc.Format);
        }
        let len = (desc.Width as usize)
            .checked_mul(desc.Height as usize)
            .and_then(|n| n.checked_mul(4))
            .context("capture texture too large")?;
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            BindFlags: 0,
            MiscFlags: 0,
            MipLevels: 1,
            ArraySize: 1,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..desc
        };
        let mut staging = None;
        unsafe {
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
        }
        .context("CreateTexture2D staging")?;
        let staging = staging.context("staging texture is null")?;
        unsafe {
            self.context.CopyResource(&staging, texture);
        }
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .context("Map staging texture")?;
        let mut pixels = vec![0; len];
        let copied = unsafe {
            copy_rows(
                mapped.pData.cast(),
                mapped.RowPitch as usize,
                &mut pixels,
                0,
                desc.Width as usize * 4,
                desc.Width as usize * 4,
                desc.Height as usize,
            )
        };
        unsafe {
            self.context.Unmap(&staging, 0);
        }
        copied?;
        Ok((desc.Width, desc.Height, pixels))
    }
}

pub(crate) fn blit(
    source: &[u8],
    source_width: u32,
    source_height: u32,
    frame: &mut crate::CapturedFrame,
    monitor: &Monitor,
) -> Result<()> {
    let width = source_width.min(monitor.info.width) as usize;
    let height = source_height.min(monitor.info.height) as usize;
    let x = (monitor.info.left - frame.virtual_left) as usize;
    let y = (monitor.info.top - frame.virtual_top) as usize;
    if x + width > frame.width as usize || y + height > frame.height as usize {
        bail!("monitor exceeds virtual desktop");
    }
    for row in 0..height {
        let src = row * source_width as usize * 4;
        let dst = (y + row) * frame.stride + x * 4;
        frame.pixels_bgra[dst..dst + width * 4].copy_from_slice(&source[src..src + width * 4]);
    }
    Ok(())
}
