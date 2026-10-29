//! The resident part of Reshot: one hidden window on its own thread that owns the global
//! hotkeys and the tray icon. Windows delivers `WM_HOTKEY` and tray callbacks to its message
//! loop, so idle costs nothing: no timers, no polling (ARCHITECTURE §7). Events leave through
//! an async channel the UI awaits.
//!
//! The window is a hidden top-level window rather than a message-only one because
//! message-only windows never receive the `TaskbarCreated` broadcast, and without it the tray
//! icon would vanish for good after Explorer restarts.

use anyhow::{Context, Result, anyhow};
use std::sync::mpsc;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, RegisterHotKey, UnregisterHotKey};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIIF_WARNING, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyWindow, DispatchMessageW, GetMessageW, HICON,
    IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR, LoadIconW, LoadImageW, MSG, PostMessageW, PostQuitMessage,
    RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, TranslateMessage, WINDOW_EX_STYLE, WM_APP,
    WM_CONTEXTMENU, WM_DESTROY, WM_HOTKEY, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::{PCWSTR, w};

use reshot_core::hotkey::HotkeyDefinition;

/// Which registered hotkey fired.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HotkeySlot {
    /// The main capture hotkey.
    Main,
    /// The optional instant audio-recording hotkey.
    Audio,
}

impl HotkeySlot {
    fn id(self) -> i32 {
        match self {
            HotkeySlot::Main => 0xB001,
            HotkeySlot::Audio => 0xB002,
        }
    }

    fn from_id(id: i32) -> Option<Self> {
        [HotkeySlot::Main, HotkeySlot::Audio].into_iter().find(|s| s.id() == id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellEvent {
    Hotkey(HotkeySlot),
    /// Left or right click on the tray icon: open the tray menu.
    TrayMenu,
    /// Double click on the tray icon: start a capture.
    TrayCapture,
    /// Another `reshot.exe` was launched and exited in our favour.
    SecondInstance,
}

#[derive(Clone, Copy, Debug)]
pub enum BalloonIcon {
    Info,
    Warning,
    Error,
}

enum Command {
    Register(HotkeySlot, HotkeyDefinition, mpsc::Sender<Result<()>>),
    Unregister(HotkeySlot),
    Balloon(String, String, BalloonIcon),
    Quit,
}

const WM_TRAY: u32 = WM_APP + 101;
const WM_COMMAND_READY: u32 = WM_APP + 102;
const TRAY_UID: u32 = 1;

/// Handle to the shell thread. Dropping it removes the tray icon and the hotkeys.
pub struct Shell {
    hwnd: isize,
    commands: mpsc::Sender<Command>,
    thread: Option<std::thread::JoinHandle<()>>,
    events: async_channel::Sender<ShellEvent>,
}

struct State {
    hwnd: HWND,
    icon: HICON,
    owns_icon: bool,
    tooltip: String,
    taskbar_created: u32,
    commands: mpsc::Receiver<Command>,
    events: async_channel::Sender<ShellEvent>,
}

thread_local! {
    static STATE: std::cell::RefCell<Option<State>> = const { std::cell::RefCell::new(None) };
}

impl Shell {
    /// Starts the shell thread with the tray icon from resource `icon_resource_id` of the
    /// executable (falls back to the stock application icon). Events go to `events_tx`.
    pub fn start(icon_resource_id: u16, tooltip: &str, events_tx: async_channel::Sender<ShellEvent>) -> Result<Shell> {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<isize>>();
        let tooltip = tooltip.to_string();
        let events = events_tx.clone();
        let thread = std::thread::Builder::new()
            .name("reshot-shell".into())
            .spawn(move || run(icon_resource_id, tooltip, cmd_rx, events, ready_tx))?;
        let hwnd = ready_rx.recv().context("shell thread died during start-up")??;
        Ok(Shell { hwnd, commands: cmd_tx, thread: Some(thread), events: events_tx })
    }

    /// A sender other parts of the app (single instance) can post events through.
    pub fn event_sender(&self) -> async_channel::Sender<ShellEvent> {
        self.events.clone()
    }

    fn send(&self, c: Command) {
        if self.commands.send(c).is_ok() {
            // SAFETY: posting to a window owned by our own live shell thread.
            unsafe {
                let _ = PostMessageW(Some(HWND(self.hwnd as _)), WM_COMMAND_READY, WPARAM(0), LPARAM(0));
            }
        }
    }

    /// Registers a hotkey, replacing whatever the slot had. Fails when the combination is
    /// owned by another application.
    pub fn register_hotkey(&self, slot: HotkeySlot, def: &HotkeyDefinition) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        self.send(Command::Register(slot, def.clone(), tx));
        rx.recv().map_err(|_| anyhow!("shell thread is gone"))?
    }

    pub fn unregister_hotkey(&self, slot: HotkeySlot) {
        self.send(Command::Unregister(slot));
    }

    pub fn show_balloon(&self, title: &str, text: &str, icon: BalloonIcon) {
        self.send(Command::Balloon(title.into(), text.into(), icon));
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        self.send(Command::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(
    icon_id: u16,
    tooltip: String,
    commands: mpsc::Receiver<Command>,
    events: async_channel::Sender<ShellEvent>,
    ready: mpsc::Sender<Result<isize>>,
) {
    // SAFETY: plain Win32 window creation and a message loop on this thread.
    unsafe {
        let hinstance = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(e) => {
                let _ = ready.send(Err(e.into()));
                return;
            }
        };
        let class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: w!("reshot.shell"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("reshot.shell"),
            w!("reshot"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance.into()),
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                let _ = ready.send(Err(e.into()));
                return;
            }
        };

        let (icon, owns_icon) = match LoadImageW(
            Some(hinstance.into()),
            PCWSTR(icon_id as usize as *const u16),
            IMAGE_ICON,
            32,
            32,
            LR_DEFAULTCOLOR,
        ) {
            Ok(h) => (HICON(h.0), true),
            Err(_) => (LoadIconW(None, IDI_APPLICATION).unwrap_or_default(), false),
        };
        let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
        STATE.with(|s| {
            *s.borrow_mut() = Some(State { hwnd, icon, owns_icon, tooltip, taskbar_created, commands, events });
        });
        add_tray_icon();
        let _ = ready.send(Ok(hwnd.0 as isize));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        STATE.with(|s| {
            if let Some(st) = s.borrow_mut().take() {
                let nid = base_nid(&st);
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
                for slot in [HotkeySlot::Main, HotkeySlot::Audio] {
                    let _ = UnregisterHotKey(Some(st.hwnd), slot.id());
                }
                if st.owns_icon {
                    let _ = DestroyIcon(st.icon);
                }
            }
        });
    }
}

fn base_nid(st: &State) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: st.hwnd,
        uID: TRAY_UID,
        ..Default::default()
    }
}

fn copy_wide(dst: &mut [u16], s: &str) {
    let n = dst.len() - 1;
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(n).chain(std::iter::once(0))) {
        *d = c;
    }
}

fn add_tray_icon() {
    STATE.with(|s| {
        let s = s.borrow();
        let Some(st) = s.as_ref() else { return };
        let mut nid = base_nid(st);
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = st.icon;
        copy_wide(&mut nid.szTip, &st.tooltip);
        // SAFETY: nid is fully initialised and points at our own window.
        unsafe {
            // After an Explorer restart the old icon may linger in the shell's bookkeeping:
            // ADD then fails and MODIFY revives it.
            if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() && !Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool() {
                log::warn!("Tray: Shell_NotifyIcon(NIM_ADD) failed.");
            }
        }
    });
}

fn handle_commands() {
    loop {
        let cmd = STATE.with(|s| s.borrow().as_ref().and_then(|st| st.commands.try_recv().ok()));
        let Some(cmd) = cmd else { return };
        let hwnd = STATE.with(|s| s.borrow().as_ref().map(|st| st.hwnd)).unwrap_or_default();
        // SAFETY: hotkeys are registered on the thread that owns `hwnd`, as Win32 requires.
        unsafe {
            match cmd {
                Command::Register(slot, def, reply) => {
                    let _ = UnregisterHotKey(Some(hwnd), slot.id());
                    let r = RegisterHotKey(Some(hwnd), slot.id(), HOT_KEY_MODIFIERS(def.modifiers.0), def.virtual_key)
                        .map_err(|e| anyhow!("RegisterHotKey failed for '{def}' ({e}); likely already in use"));
                    match &r {
                        Ok(()) => log::info!("Hotkey: registered '{def}' ({slot:?})."),
                        Err(e) => log::error!("Hotkey: {e}"),
                    }
                    let _ = reply.send(r);
                }
                Command::Unregister(slot) => {
                    if UnregisterHotKey(Some(hwnd), slot.id()).is_ok() {
                        log::info!("Hotkey: unregistered {slot:?}.");
                    }
                }
                Command::Balloon(title, text, icon) => STATE.with(|s| {
                    if let Some(st) = s.borrow().as_ref() {
                        let mut nid = base_nid(st);
                        nid.uFlags = NIF_INFO;
                        copy_wide(&mut nid.szInfoTitle, &title);
                        copy_wide(&mut nid.szInfo, &text);
                        nid.dwInfoFlags = match icon {
                            BalloonIcon::Info => NIIF_INFO,
                            BalloonIcon::Warning => NIIF_WARNING,
                            BalloonIcon::Error => NIIF_ERROR,
                        };
                        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
                    }
                }),
                Command::Quit => {
                    let _ = DestroyWindow(hwnd);
                }
            }
        }
    }
}

fn emit(e: ShellEvent) {
    STATE.with(|s| {
        if let Some(st) = s.borrow().as_ref() {
            let _ = st.events.try_send(e);
        }
    });
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let taskbar_created = STATE.with(|s| s.borrow().as_ref().map(|st| st.taskbar_created)).unwrap_or(0);
    match msg {
        WM_HOTKEY => {
            if let Some(slot) = HotkeySlot::from_id(wparam.0 as i32) {
                emit(ShellEvent::Hotkey(slot));
            }
            LRESULT(0)
        }
        WM_TRAY => {
            match lparam.0 as u32 {
                WM_LBUTTONUP | WM_RBUTTONUP | WM_CONTEXTMENU => {
                    // SAFETY: our own window; lets the menu window take the foreground.
                    unsafe {
                        let _ = SetForegroundWindow(hwnd);
                    }
                    emit(ShellEvent::TrayMenu);
                }
                WM_LBUTTONDBLCLK => emit(ShellEvent::TrayCapture),
                _ => {}
            }
            LRESULT(0)
        }
        WM_COMMAND_READY => {
            handle_commands();
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        m if m != 0 && m == taskbar_created => {
            log::info!("Tray: taskbar recreated, restoring tray icon.");
            add_tray_icon();
            LRESULT(0)
        }
        // SAFETY: default handling for everything else.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
