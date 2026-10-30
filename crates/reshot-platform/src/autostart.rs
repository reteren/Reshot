//! "Run at logon", in one of two lanes.
//!
//! **Plain** is the `HKCU\...\Run` value: per user, no elevation, nothing to approve.
//! **Elevated** is a scheduled task with `RunLevel=HighestAvailable`, the only way Windows
//! starts a program with administrator rights at logon *without* a UAC prompt every time:
//! consent is given once, when the task is created. Reshot needs it because it cannot bring
//! its overlay over an application that is itself elevated (ARCHITECTURE §6).
//!
//! The lanes are mutually exclusive: running both would start Reshot twice.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, REG_OPTION_NON_VOLATILE,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{HSTRING, PCWSTR, w};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE_NAME: PCWSTR = w!("reshot");
/// Deliberately plain and top-level, so it is easy to find and delete by hand.
const TASK_NAME: &str = "Reshot";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn executable_path() -> PathBuf {
    std::env::current_exe().unwrap_or_default()
}

/// Aligns Windows with the desired state and returns whether the elevated lane is active.
/// A caller that gets `false` back with `elevated` requested should persist that: the
/// checkbox would otherwise claim something Windows never agreed to.
///
/// `allow_prompt`: whether this call may raise UAC. True when the user just changed the
/// setting; false at startup, where a prompt on every boot would be intolerable, so a
/// mismatch is reported rather than fixed.
pub fn apply(enabled: bool, elevated: bool, allow_prompt: bool) -> bool {
    // A development build shares settings.json with the installed Reshot; applying it here
    // would point the user's autostart at target\debug.
    if cfg!(debug_assertions) {
        log::info!("Autostart: skipped in a debug build.");
        return elevated && is_elevated_task_current();
    }
    let elevated_active = enabled && elevated && ensure_task(allow_prompt);
    // Only removed when it exists, so a lane that was never created cannot cost a second
    // prompt after the first was declined.
    if !elevated_active && elevated_task_exists() {
        remove_task(allow_prompt);
    }
    apply_run_key(enabled && !elevated_active);
    elevated_active
}

pub fn is_elevated_task_current() -> bool {
    read_task_command().is_some_and(|cmd| cmd.eq_ignore_ascii_case(&executable_path().to_string_lossy()))
}

pub fn elevated_task_exists() -> bool {
    run_schtasks(&["/Query", "/TN", TASK_NAME]).is_some()
}

// ---- plain lane: HKCU\...\Run ---------------------------------------------------------

fn open_run_key(access: windows::Win32::System::Registry::REG_SAM_FLAGS, create: bool) -> Option<HKEY> {
    let mut key = HKEY::default();
    // SAFETY: registry calls with valid out pointers.
    let ok = unsafe {
        if create {
            RegCreateKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, None, REG_OPTION_NON_VOLATILE, access, None, &mut key, None)
                .is_ok()
        } else {
            RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, access, &mut key).is_ok()
        }
    };
    ok.then_some(key)
}

pub fn is_run_key_enabled() -> bool {
    let Some(key) = open_run_key(KEY_READ, false) else { return false };
    // SAFETY: querying existence only.
    let found = unsafe { RegQueryValueExW(key, VALUE_NAME, None, None, None, None).is_ok() };
    // SAFETY: key was opened above.
    unsafe {
        let _ = RegCloseKey(key);
    }
    found
}

fn apply_run_key(enabled: bool) {
    if enabled {
        let Some(key) = open_run_key(KEY_WRITE, true) else {
            log::error!("Autostart: failed to open the Run key for writing");
            return;
        };
        let value: Vec<u16> =
            format!("\"{}\"", executable_path().display()).encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: REG_SZ data is the UTF-16 string including its terminator.
        let bytes = unsafe { std::slice::from_raw_parts(value.as_ptr() as *const u8, value.len() * 2) };
        // SAFETY: key is open for writing.
        match unsafe { RegSetValueExW(key, VALUE_NAME, None, REG_SZ, Some(bytes)) }.ok() {
            Ok(()) => log::info!("Autostart: Run key enabled."),
            Err(e) => log::error!("Autostart: failed to enable the Run key: {e}"),
        }
        // SAFETY: key was opened above.
        unsafe {
            let _ = RegCloseKey(key);
        }
    } else if is_run_key_enabled() {
        let Some(key) = open_run_key(KEY_WRITE, false) else { return };
        // SAFETY: key is open for writing.
        if unsafe { RegDeleteValueW(key, VALUE_NAME) }.is_ok() {
            log::info!("Autostart: Run key disabled.");
        }
        // SAFETY: key was opened above.
        unsafe {
            let _ = RegCloseKey(key);
        }
    }
}

// ---- elevated lane: scheduled task ----------------------------------------------------

fn ensure_task(allow_prompt: bool) -> bool {
    if is_elevated_task_current() {
        return true;
    }
    if !allow_prompt {
        // Startup: say what is wrong and leave it alone.
        if elevated_task_exists() {
            log::warn!(
                "Autostart: the '{TASK_NAME}' task points at another build; re-toggle \"Start as administrator\" in Settings to update it."
            );
        } else {
            log::warn!(
                "Autostart: elevated start is enabled but the '{TASK_NAME}' task is missing; re-toggle \"Start as administrator\" in Settings to recreate it."
            );
        }
        return false;
    }
    let xml_file = std::env::temp_dir().join(format!("reshot-autostart-{}.xml", std::process::id()));
    // Registering from XML rather than switches: /RU with a logon trigger otherwise drags in
    // credential handling, and the defaults that matter (no battery stop, no time limit) are
    // not reachable from the command line. schtasks expects UTF-16 with a BOM.
    let mut data = vec![0xFF, 0xFE];
    for u in build_task_xml().encode_utf16() {
        data.extend_from_slice(&u.to_le_bytes());
    }
    if let Err(e) = std::fs::write(&xml_file, data) {
        log::error!("Autostart: failed to write the task XML: {e}");
        return false;
    }
    let ok = run_schtasks_elevated(&format!("/Create /TN \"{TASK_NAME}\" /XML \"{}\" /F", xml_file.display()));
    let _ = std::fs::remove_file(&xml_file);
    if ok {
        log::info!("Autostart: registered the elevated '{TASK_NAME}' task → {}", executable_path().display());
    }
    ok
}

fn remove_task(allow_prompt: bool) {
    if !allow_prompt {
        log::warn!(
            "Autostart: the elevated '{TASK_NAME}' task is still registered; it will be removed the next time the setting is changed."
        );
        return;
    }
    if run_schtasks_elevated(&format!("/Delete /TN \"{TASK_NAME}\" /F")) {
        log::info!("Autostart: removed the elevated '{TASK_NAME}' task.");
    }
}

fn read_task_command() -> Option<String> {
    let xml = run_schtasks(&["/Query", "/TN", TASK_NAME, "/XML", "ONE"])?;
    let open = xml.find("<Command>")? + "<Command>".len();
    let close = xml[open..].find("</Command>")? + open;
    Some(xml[open..close].trim().trim_matches('"').to_string())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn build_task_xml() -> String {
    let user = xml_escape(&format!(
        "{}\\{}",
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default()
    ));
    let exe = xml_escape(&executable_path().to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>{user}</Author>
    <Description>Starts Reshot at logon with administrator rights, so its overlay can appear over elevated applications.</Description>
    <URI>\{TASK_NAME}</URI>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>false</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
    </Exec>
  </Actions>
</Task>"#
    )
}

/// Runs schtasks unelevated and returns its stdout on success. Queries need no rights.
fn run_schtasks(args: &[&str]) -> Option<String> {
    let out = Command::new("schtasks.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| log::error!("Autostart: schtasks {args:?} failed: {e}"))
        .ok()?;
    // /XML comes back in the console code page; a parse that fails only costs the staleness
    // check, never a wrong answer.
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn is_process_elevated() -> bool {
    // SAFETY: token query with a correctly sized buffer.
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

/// Runs schtasks with administrator rights, asking only if we do not have them yet. Once the
/// elevated lane is in use Reshot itself starts elevated, so later changes cost no prompt.
fn run_schtasks_elevated(args: &str) -> bool {
    if is_process_elevated() {
        log::info!("Autostart: already elevated; running schtasks without a prompt.");
        let status = Command::new("schtasks.exe")
            .raw_arg(args)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        return status.is_ok_and(|s| s.success());
    }
    let file = HSTRING::from("schtasks.exe");
    let params = HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: info points at live strings for the duration of the call.
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            if e.code() == ERROR_CANCELLED.to_hresult() {
                log::warn!("Autostart: the administrator prompt was declined.");
            } else {
                log::error!("Autostart: elevated schtasks {args} failed: {e}");
            }
            return false;
        }
        if info.hProcess.is_invalid() {
            return false;
        }
        WaitForSingleObject(info.hProcess, 60_000);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if code != 0 {
            log::error!("Autostart: schtasks exited with {code} for '{args}'.");
        }
        code == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_xml_names_this_executable_and_runs_elevated() {
        let xml = build_task_xml();
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains(&xml_escape(&executable_path().to_string_lossy())));
        assert!(xml.contains("<URI>\\Reshot</URI>"));
    }
}
