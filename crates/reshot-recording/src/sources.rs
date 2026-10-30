use std::collections::HashMap;
use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongW, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PWSTR};

#[derive(Clone, Debug, Default)]
pub struct AudioSources {
    pub mic: bool,
    /// MMDevice ID, or "default".
    pub mic_device: Option<String>,
    pub system_full: bool,
    pub include_pids: Vec<u32>,
}

impl AudioSources {
    pub fn has_any_audio(&self) -> bool {
        self.mic || self.system_full || !self.include_pids.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowAudioSource {
    pub process_id: u32,
    pub process_name: String,
    pub title: String,
}

/// Visible, unowned top-level windows, one per process, for the audio source picker.
pub fn list_window_audio_sources() -> Vec<WindowAudioSource> {
    unsafe extern "system" fn callback(hwnd: HWND, data: LPARAM) -> BOOL {
        let results = unsafe { &mut *(data.0 as *mut HashMap<u32, WindowAudioSource>) };
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() || unsafe { GetWindowTextLengthW(hwnd) } == 0
        {
            return true.into();
        }
        if unsafe { GetWindow(hwnd, GW_OWNER) }.is_ok_and(|owner| !owner.0.is_null()) {
            return true.into();
        }
        if unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOOLWINDOW.0 != 0 {
            return true.into();
        }
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        if pid == 0 || pid == std::process::id() || results.contains_key(&pid) {
            return true.into();
        }
        let mut title = [0u16; 256];
        let title_len = unsafe { GetWindowTextW(hwnd, &mut title) };
        if title_len <= 0 {
            return true.into();
        }
        if let Some(process_name) = process_name(pid) {
            results.insert(
                pid,
                WindowAudioSource {
                    process_id: pid,
                    process_name,
                    title: String::from_utf16_lossy(&title[..title_len as usize]),
                },
            );
        }
        true.into()
    }
    let mut results: HashMap<u32, WindowAudioSource> = HashMap::new();
    let pointer = &mut results as *mut HashMap<u32, WindowAudioSource>;
    let _ = unsafe { EnumWindows(Some(callback), LPARAM(pointer as isize)) };
    let mut values: Vec<_> = results.into_values().collect();
    values.sort_by_key(|source| source.process_name.to_lowercase());
    values
}

fn process_name(pid: u32) -> Option<String> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut path = [0u16; 1024];
    let mut size = path.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &mut size,
        )
    };
    let _ = unsafe { CloseHandle(handle) };
    result.ok()?;
    Path::new(&String::from_utf16_lossy(&path[..size as usize]))
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
}
