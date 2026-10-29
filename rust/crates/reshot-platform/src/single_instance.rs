//! One running Reshot per user session (SPEC §16). A named mutex detects a prior instance and
//! a named auto-reset event wakes it: a second launch signals the event and exits. The first
//! instance waits on the event on a parked thread, so there is no polling and no CPU cost.

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, ReleaseMutex, SetEvent,
    WaitForSingleObject,
};
use windows::core::w;

use crate::shell::ShellEvent;

pub struct SingleInstance {
    mutex: HANDLE,
}

// SAFETY: kernel handles may be used from any thread.
unsafe impl Send for SingleInstance {}

impl SingleInstance {
    /// `Some` when this is the primary instance. `None` means the running instance has been
    /// signalled and the caller should exit right away.
    pub fn acquire(events: async_channel::Sender<ShellEvent>) -> Option<Self> {
        // SAFETY: named kernel objects in the per-user Local\ namespace.
        unsafe {
            let mutex = CreateMutexW(None, true, w!("Local\\reshot.singleinstance.mutex")).ok()?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                if let Ok(ev) = OpenEventW(EVENT_MODIFY_STATE, false, w!("Local\\reshot.singleinstance.signal")) {
                    let _ = SetEvent(ev);
                    let _ = CloseHandle(ev);
                    log::info!("SingleInstance: signalled the running instance.");
                }
                let _ = CloseHandle(mutex);
                return None;
            }
            let event = CreateEventW(None, false, false, w!("Local\\reshot.singleinstance.signal")).ok()?;
            let raw = event.0 as isize;
            std::thread::Builder::new()
                .name("single-instance".into())
                .spawn(move || {
                    let event = HANDLE(raw as _);
                    while WaitForSingleObject(event, INFINITE) == WAIT_OBJECT_0 {
                        if events.send_blocking(ShellEvent::SecondInstance).is_err() {
                            break;
                        }
                    }
                })
                .ok()?;
            Some(Self { mutex })
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: we own the mutex handle.
        unsafe {
            let _ = ReleaseMutex(self.mutex);
            let _ = CloseHandle(self.mutex);
        }
    }
}
