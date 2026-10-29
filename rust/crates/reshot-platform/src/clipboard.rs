//! Clipboard output. An image goes on as a PNG stream, which Telegram, Discord, Paint.NET and
//! friends read with transparency intact, plus a plain DIB fallback (no alpha) for apps that
//! only understand bitmaps. Text goes on as `CF_UNICODETEXT`.

use std::time::Duration;
use windows::Win32::Foundation::{CloseHandle, GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFOHEADER};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetOpenClipboardWindow, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::core::{PWSTR, w};

/// Why a copy failed. `holder` names the process that had the clipboard open, when it could be
/// identified: the user cannot act on "copy failed", but can act on which app holds it.
#[derive(Debug)]
pub struct ClipboardBusy {
    pub holder: Option<String>,
}

/// An image to put on the clipboard: tightly packed BGRA (premultiplied is fine for the DIB,
/// which ignores alpha) plus its PNG encoding.
pub struct ClipboardImage<'a> {
    pub width: u32,
    pub height: u32,
    pub bgra: &'a [u8],
    pub png: &'a [u8],
}

fn alloc_global(bytes: &[u8]) -> Option<HGLOBAL> {
    // SAFETY: allocate, copy, unlock; ownership passes to the clipboard on success.
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).ok()?;
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(h);
        Some(h)
    }
}

fn set(format: u32, bytes: &[u8]) -> bool {
    let Some(h) = alloc_global(bytes) else { return false };
    // SAFETY: the clipboard is open; on success the system owns `h`.
    unsafe {
        if SetClipboardData(format, Some(HANDLE(h.0))).is_err() {
            let _ = GlobalFree(Some(h));
            return false;
        }
    }
    true
}

/// Opens the clipboard, retrying for about a second: clipboard managers take it for a moment
/// after every change.
fn open() -> Result<(), ClipboardBusy> {
    for _ in 0..10 {
        // SAFETY: no owner window; released in `close`.
        if unsafe { OpenClipboard(None) }.is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Asked now, while the holder still has it; by the time a message reaches the screen the
    // culprit has usually let go and cannot be named.
    let holder = describe_holder();
    log::warn!(
        "Export: clipboard busy or unavailable{}",
        holder.as_deref().map(|h| format!(" (held by {h})")).unwrap_or_default()
    );
    Err(ClipboardBusy { holder })
}

fn close() {
    // SAFETY: paired with a successful OpenClipboard.
    unsafe {
        let _ = CloseClipboard();
    }
}

pub fn set_image(img: &ClipboardImage) -> Result<(), ClipboardBusy> {
    open()?;
    // SAFETY: we hold the clipboard.
    unsafe {
        let _ = EmptyClipboard();
        let png_format = RegisterClipboardFormatW(w!("PNG"));
        set(png_format, img.png);
    }
    set(CF_DIB.0 as u32, &dib(img));
    close();
    log::info!("Export: copied selection to clipboard.");
    Ok(())
}

pub fn set_text(text: &str) -> Result<(), ClipboardBusy> {
    open()?;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: plain byte view of the UTF-16 buffer.
    let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
    // SAFETY: we hold the clipboard.
    unsafe {
        let _ = EmptyClipboard();
    }
    set(CF_UNICODETEXT.0 as u32, bytes);
    close();
    Ok(())
}

/// A bottom-up 32-bit BI_RGB DIB, which every consumer understands.
fn dib(img: &ClipboardImage) -> Vec<u8> {
    let header = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: img.width as i32,
        biHeight: img.height as i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: img.width * img.height * 4,
        ..Default::default()
    };
    let mut out = Vec::with_capacity(header.biSize as usize + header.biSizeImage as usize);
    // SAFETY: plain-old-data header.
    out.extend_from_slice(unsafe {
        std::slice::from_raw_parts(&header as *const _ as *const u8, std::mem::size_of::<BITMAPINFOHEADER>())
    });
    let row = img.width as usize * 4;
    for y in (0..img.height as usize).rev() {
        out.extend_from_slice(&img.bgra[y * row..(y + 1) * row]);
    }
    out
}

fn describe_holder() -> Option<String> {
    // SAFETY: plain queries; the process handle is closed before returning.
    unsafe {
        let hwnd = GetOpenClipboardWindow().ok()?;
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(process);
        ok.then(|| {
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            let name = path.rsplit('\\').next().unwrap_or(&path).to_string();
            format!("{name} (pid {pid})")
        })
    }
}
