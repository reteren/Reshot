//! Win32 helpers for windows, monitors, the cursor and the foreground. Everything that touches
//! global input state is opt-in and meant to be called once per session; see the notes on
//! each function for why.

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, MonitorFromWindow,
};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, BringWindowToTop, ClipCursor, GetClientRect, GetCursorPos, GetForegroundWindow, GetSystemMetrics,
    GetWindowRect, GetWindowThreadProcessId, HWND_TOPMOST, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_SHOWWINDOW, SetForegroundWindow, SetWindowDisplayAffinity, SetWindowPos, ShowCursor,
    WINDOW_DISPLAY_AFFINITY,
};

/// A rectangle in physical virtual-desktop pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PxRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PxRect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

impl From<RECT> for PxRect {
    fn from(r: RECT) -> Self {
        Self { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
    }
}

/// Wraps a raw `HWND` value as it comes out of `raw-window-handle`.
pub fn hwnd(raw: isize) -> HWND {
    HWND(raw as _)
}

pub fn is_key_down(vk: u32) -> bool {
    // SAFETY: plain query.
    unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    // SAFETY: valid out pointer.
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

pub fn virtual_screen() -> PxRect {
    // SAFETY: plain queries.
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        PxRect {
            left: x,
            top: y,
            right: x + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: y + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

fn monitor_info_at(x: i32, y: i32) -> Option<MONITORINFO> {
    // SAFETY: plain queries with a correctly sized struct.
    unsafe {
        let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        if mon.is_invalid() {
            return None;
        }
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        GetMonitorInfoW(mon, &mut mi).as_bool().then_some(mi)
    }
}

/// Bounds of the primary monitor, in physical pixels.
pub fn primary_monitor_bounds() -> PxRect {
    monitor_info_at(0, 0).map(|mi| mi.rcMonitor.into()).unwrap_or_else(|| {
        // SAFETY: plain queries.
        unsafe { PxRect { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) } }
    })
}

/// Work area (without the taskbar) of the monitor containing the point.
pub fn working_area(x: i32, y: i32) -> PxRect {
    monitor_info_at(x, y).map(|mi| mi.rcWork.into()).unwrap_or_else(primary_monitor_bounds)
}

/// Bounds of the monitor containing the point.
pub fn monitor_bounds(x: i32, y: i32) -> PxRect {
    monitor_info_at(x, y).map(|mi| mi.rcMonitor.into()).unwrap_or_else(primary_monitor_bounds)
}

/// Places a window topmost so that its client area covers exactly `r` (physical pixels), and
/// shows it.
///
/// gpui's borderless windows keep `WS_CAPTION`, and Windows still insets their client area by
/// the frame: 8 px left, right and bottom and 1 px at the top. Placing the window rect on `r`
/// therefore drew everything 8 px right and 1 px down, which over a frozen frame reads as
/// the whole desktop shifting while the overlay is up. The window is placed once, the insets
/// measured, and the window grown by them.
pub fn place_topmost(hwnd: HWND, r: PxRect) {
    no_frame(hwnd);
    // SAFETY: hwnd belongs to this process; the out-parameters are locals.
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), r.left, r.top, r.width(), r.height(), SWP_SHOWWINDOW);
        let mut client = RECT::default();
        let mut origin = POINT::default();
        if GetClientRect(hwnd, &mut client).is_err() || !ClientToScreen(hwnd, &mut origin).as_bool() {
            return;
        }
        let left = origin.x - r.left;
        let top = origin.y - r.top;
        let right = r.right - (origin.x + client.right);
        let bottom = r.bottom - (origin.y + client.bottom);
        if (left, top, right, bottom) == (0, 0, 0, 0) {
            return;
        }
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            r.left - left,
            r.top - top,
            r.width() + left + right,
            r.height() + top + bottom,
            SWP_SHOWWINDOW,
        );
    }
}

/// Stops DWM drawing a frame around a window: no 1 px border and no drop shadow.
///
/// Because gpui's borderless windows keep `WS_CAPTION`, Windows 11 gives them the frame of a
/// normal window, drawn around the whole window rectangle. Over a transparent window (the
/// radial menu is a circle in a square) that showed as a faint square outline with a shadow
/// around nothing.
pub fn no_frame(hwnd: HWND) {
    use windows::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_NCRENDERING_POLICY, DwmSetWindowAttribute,
    };
    /// `DWMWA_COLOR_NONE`: no border at all (Windows 11).
    const COLOR_NONE: u32 = 0xFFFF_FFFE;
    let policy = DWMNCRP_DISABLED;
    // SAFETY: hwnd belongs to this process; each value matches its attribute's type.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_NCRENDERING_POLICY,
            &policy as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&policy) as u32,
        );
        // Windows 10 has no such attribute; the policy above already removed its frame.
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &COLOR_NONE as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&COLOR_NONE) as u32,
        );
    }
}

/// Hides (or shows again) a window from every screen-capture API (`WDA_EXCLUDEFROMCAPTURE`):
/// it stays on the physical display, but recorders and screen shares see what is behind it.
pub fn exclude_from_capture(hwnd: HWND, exclude: bool) {
    const WDA_NONE: u32 = 0x0;
    const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;
    let affinity = if exclude { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    // SAFETY: hwnd belongs to this process.
    if let Err(e) = unsafe { SetWindowDisplayAffinity(hwnd, WINDOW_DISPLAY_AFFINITY(affinity)) } {
        log::warn!("SetWindowDisplayAffinity({affinity:#x}) failed: {e}");
    }
}

/// Asks DWM to round the window's own frame. The acrylic backdrop covers the whole window
/// rectangle, so a panel drawn with rounded corners alone would leave square blurred wedges
/// poking out past them; rounding the frame clips the backdrop to the same shape.
pub fn round_corners(hwnd: HWND) {
    use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
    let preference = DWMWCP_ROUND;
    // SAFETY: hwnd belongs to this process; the attribute is a DWM_WINDOW_CORNER_PREFERENCE.
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&preference) as u32,
        )
    };
    // Windows 10 has no rounded frames; the panel's own corners are all it gets there.
    if let Err(e) = result {
        log::debug!("DWM corner preference unavailable: {e}");
    }
}

/// Whether the foreground window belongs to another process and covers its whole screen: in
/// practice a fullscreen or borderless game. Everything that touches global input state is
/// gated on this, because against an ordinary desktop window there is nothing to fight for.
pub fn foreground_is_fullscreen_foreign_window() -> bool {
    // SAFETY: plain queries.
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() {
            return false;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        if pid == GetCurrentProcessId() {
            return false;
        }
        let mut rect = RECT::default();
        if GetWindowRect(fg, &mut rect).is_err() {
            return false;
        }
        // Screen bounds, not the work area: a maximised window stops at the taskbar, a game
        // does not. That difference is the whole test.
        let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if mon.is_invalid() || !GetMonitorInfoW(mon, &mut mi).as_bool() {
            return false;
        }
        let m = mi.rcMonitor;
        rect.left <= m.left && rect.top <= m.top && rect.right >= m.right && rect.bottom >= m.bottom
    }
}

/// Puts a window back on top without activating it. Free of input-state side effects, so it
/// is the one thing safe to repeat against a game that re-asserts its own topmost.
pub fn assert_topmost(hwnd: HWND) {
    // SAFETY: hwnd belongs to this process.
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

/// Un-confines the pointer. Free of side effects when nothing is confined, so it is safe to
/// repeat while a game keeps re-clipping.
pub fn release_cursor_clip() {
    // SAFETY: None clears the clip.
    unsafe {
        let _ = ClipCursor(None);
    }
}

/// Releases the cursor confinement and capture games apply and makes the cursor visible.
/// Call once before showing the overlay or radial menu: it touches global input state, and
/// repeating it shows up as a blinking cursor.
pub fn unlock_input_from_game() {
    release_cursor_clip();
    // SAFETY: plain calls.
    unsafe {
        let _ = ReleaseCapture();
        // ShowCursor is reference-counted and returns the new count, so merely asking costs
        // an increment that has to be handed back. Looping unconditionally left the counter
        // one higher on every call, which is what made the cursor blink before the overlay.
        let mut count = ShowCursor(true);
        if count > 0 {
            ShowCursor(false);
            return;
        }
        // Games hide it many times over; climb back to zero, never above.
        let mut i = 0;
        while i < 32 && count < 0 {
            count = ShowCursor(true);
            i += 1;
        }
    }
}

/// Pulls `hwnd` to the top and, if it can, to the foreground; returns whether it holds the
/// foreground afterwards.
///
/// Two tiers, because they cost wildly different things. The polite tier (topmost, bring to
/// top, SetForegroundWindow) touches nothing outside this process and always runs. The
/// invasive tier attaches our input queue to the foreground window's, which merges cursor
/// state with a foreign process and serialises our thread against its input thread: cheap
/// against Notepad, not cheap against a game mid-frame. So it is opt-in, once per session.
pub fn force_foreground(hwnd: HWND, invasive: bool) -> bool {
    assert_topmost(hwnd);
    // SAFETY: Win32 calls on our own window and a foreign thread id we only attach to briefly.
    unsafe {
        let fg = GetForegroundWindow();
        if fg == hwnd {
            return true;
        }
        const ASFW_ANY: u32 = u32::MAX;
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        if GetForegroundWindow() == hwnd || !invasive {
            return GetForegroundWindow() == hwnd;
        }
        let this_thread = GetCurrentThreadId();
        let fg_thread = GetWindowThreadProcessId(fg, None);
        let attached =
            fg_thread != 0 && fg_thread != this_thread && AttachThreadInput(fg_thread, this_thread, true).as_bool();
        // No synthetic Alt: a global keypress is seen by the whole desktop and by games as
        // input. Sharing the input queue is what makes the calls below legal anyway.
        if attached {
            // Mouse capture belongs to a thread; while the queues are joined the game's
            // capture is ours to drop, and while it holds it every WM_SETCURSOR goes to it.
            let _ = ReleaseCapture();
        }
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        if attached {
            let _ = AttachThreadInput(fg_thread, this_thread, false);
        }
        GetForegroundWindow() == hwnd
    }
}

/// Shows or hides a window without activating it.
pub fn show(hwnd: HWND, visible: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNA, ShowWindow};
    // SAFETY: hwnd belongs to this process.
    unsafe {
        let _ = ShowWindow(hwnd, if visible { SW_SHOWNA } else { SW_HIDE });
    }
}

/// The overlay's crosshair: a white core inside a black outline, drawn here rather than the
/// stock one, which is a single colour and disappears against a background of that colour.
fn outlined_crosshair() -> Option<windows::Win32::UI::WindowsAndMessaging::HCURSOR> {
    use std::sync::OnceLock;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject,
    };
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HCURSOR, ICONINFO};
    const SIZE: i32 = 32;
    const HOT: i32 = 16;
    // Built once: a cursor costs a GDI handle. Stored as an integer to be shareable.
    static CURSOR: OnceLock<isize> = OnceLock::new();
    let raw = *CURSOR.get_or_init(|| {
        // SAFETY: plain GDI object creation; the bitmaps are freed once the icon owns copies.
        unsafe {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: SIZE,
                    biHeight: -SIZE, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let Ok(color) = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) else { return 0 };
            let px = std::slice::from_raw_parts_mut(bits as *mut u32, (SIZE * SIZE) as usize);
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let (dx, dy) = (x - HOT, y - HOT);
                    let arm = (-1..=1).contains(&dx) || (-1..=1).contains(&dy);
                    let core = dx == 0 || dy == 0;
                    px[(y * SIZE + x) as usize] = if !arm {
                        0
                    } else if core {
                        0xFFFF_FFFF
                    } else {
                        0xFF00_0000
                    };
                }
            }
            // For a 32-bit cursor the alpha decides; the AND mask is all zeros.
            let mask = CreateBitmap(SIZE, SIZE, 1, 1, None);
            let icon = ICONINFO {
                fIcon: false.into(),
                xHotspot: HOT as u32,
                yHotspot: HOT as u32,
                hbmMask: mask,
                hbmColor: color,
            };
            let cursor = CreateIconIndirect(&icon).map(|h| h.0 as isize).unwrap_or(0);
            let _ = DeleteObject(color.into());
            let _ = DeleteObject(mask.into());
            cursor
        }
    });
    (raw != 0).then_some(HCURSOR(raw as _))
}

/// Swaps the stock crosshair for the outlined one on `hwnd`. gpui sets its cursor from
/// WM_SETCURSOR using the shared system crosshair; this subclass lets it do so and then
/// replaces that one cursor, leaving every other shape (I-beam, resize, hand) alone.
pub fn use_outlined_crosshair(hwnd: HWND) {
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{GetCursor, IDC_CROSS, LoadCursorW, SetCursor, WM_SETCURSOR};

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _: usize, _: usize) -> LRESULT {
        // SAFETY: forwarding to the original procedure, then querying cursors.
        unsafe {
            let r = DefSubclassProc(hwnd, msg, wp, lp);
            if msg == WM_SETCURSOR
                && let (Ok(cross), Some(ours)) = (LoadCursorW(None, IDC_CROSS), outlined_crosshair())
                && GetCursor() == cross
            {
                SetCursor(Some(ours));
            }
            r
        }
    }
    if outlined_crosshair().is_none() {
        return; // not worth failing a capture over: the stock crosshair stays
    }
    // SAFETY: subclassing our own window on its own thread.
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(proc), 0x5245_5348, 0);
    }
}
