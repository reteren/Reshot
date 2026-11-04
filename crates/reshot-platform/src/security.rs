//! Elevated execution requires a machine installation whose binaries cannot be changed by
//! an unelevated process. A Program Files prefix alone does not establish that boundary.

use std::fs::{File, OpenOptions};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSidToSidW, GetSecurityInfo, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, EqualSid, GetAce,
    GetTokenInformation, INHERIT_ONLY_ACE, IsValidAcl, IsValidSid, IsWellKnownSid,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_ELEVATION, TOKEN_QUERY,
    TokenElevation, WinBuiltinAdministratorsSid, WinLocalSystemSid,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_READ,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{FOLDERID_ProgramFiles, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
use windows::core::w;

/// Kept alive by the application to prevent replacement of checked path components.
pub struct TrustedInstallation {
    _files: Vec<File>,
}

struct LocalAllocation(*mut core::ffi::c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: each allocation here is returned by a Win32 LocalAlloc-based API.
        unsafe {
            LocalFree(Some(HLOCAL(self.0)));
        }
    }
}

pub fn is_process_elevated() -> Result<bool> {
    // SAFETY: a correctly sized token query; the handle is closed on both outcomes.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = 0;
        let result = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );
        let _ = CloseHandle(token);
        result?;
        Ok(elevation.TokenIsElevated != 0)
    }
}

/// A normal launch is unrestricted; an elevated launch fails closed before loading settings.
pub fn guard_elevated_startup() -> Result<Option<TrustedInstallation>> {
    if is_process_elevated()? {
        Ok(Some(trusted_installation()?))
    } else {
        Ok(None)
    }
}

/// Also checked before registering an elevated logon task, while still unelevated.
pub fn trusted_installation() -> Result<TrustedInstallation> {
    let executable = std::env::current_exe().context("locating Reshot")?;
    let directory = executable
        .parent()
        .context("Reshot has no installation directory")?;
    let program_files = program_files_directory()?;
    let relative = directory.strip_prefix(&program_files).context(
        "Install Reshot for all users in Program Files before enabling administrator mode",
    )?;
    if relative.as_os_str().is_empty() {
        bail!("Reshot must have its own installation directory under Program Files");
    }

    // Program Files is normally owned by TrustedInstaller; installed application files are
    // normally owned by Administrators or SYSTEM. An ordinary owner could rewrite the DACL.
    let mut installer = PSID::default();
    unsafe {
        ConvertStringSidToSidW(
            w!("S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"),
            &mut installer,
        )?;
    }
    let _installer_allocation = LocalAllocation(installer.0);
    let mut files = Vec::new();
    let mut path = program_files.clone();
    files.push(check_protected_object(&path, installer)?);
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            bail!("Installation path contains an unexpected path component");
        }
        path.push(component.as_os_str());
        files.push(check_protected_object(&path, installer)?);
    }
    files.push(check_protected_object(&executable, installer)?);
    let ffmpeg = directory.join("ffmpeg.exe");
    match std::fs::symlink_metadata(&ffmpeg) {
        Ok(_) => files.push(check_protected_object(&ffmpeg, installer)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("checking bundled FFmpeg permissions"),
    }
    Ok(TrustedInstallation { _files: files })
}

fn program_files_directory() -> Result<PathBuf> {
    // The trusted location comes from Windows, not a caller-controlled environment variable.
    unsafe {
        let pointer = SHGetKnownFolderPath(&FOLDERID_ProgramFiles, KF_FLAG_DEFAULT, None)?;
        let path = pointer.to_string();
        CoTaskMemFree(Some(pointer.0.cast()));
        Ok(PathBuf::from(path?))
    }
}

fn is_trusted_sid(sid: PSID, installer: PSID) -> bool {
    // SAFETY: SIDs belong to the live security descriptor or the allocated service SID.
    unsafe {
        !sid.0.is_null()
            && IsValidSid(sid).as_bool()
            && (IsWellKnownSid(sid, WinBuiltinAdministratorsSid).as_bool()
                || IsWellKnownSid(sid, WinLocalSystemSid).as_bool()
                || EqualSid(sid, installer).is_ok())
    }
}

fn check_protected_object(path: &Path, installer: PSID) -> Result<File> {
    // READ_CONTROL is enough to read the DACL, including for directories. Keeping this
    // handle open denies subsequent writers and deletion; links are inspected, not followed.
    let file = OpenOptions::new()
        .access_mode(0x0002_0000)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(path)
        .with_context(|| format!("opening protected installation path {}", path.display()))?;
    let metadata = file.metadata()?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        bail!(
            "Elevated installation must not contain links: {}",
            path.display()
        );
    }
    let mut owner = PSID::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: valid file handle and out pointers; descriptor owns the returned SID and ACL.
    unsafe {
        GetSecurityInfo(
            HANDLE(file.as_raw_handle()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut dacl),
            None,
            Some(&mut descriptor),
        )
        .ok()?;
    }
    let _descriptor_allocation = LocalAllocation(descriptor.0);
    validate_permissions(owner, dacl, installer, metadata.is_dir(), path)?;
    Ok(file)
}

fn validate_permissions(
    owner: PSID,
    dacl: *mut ACL,
    installer: PSID,
    directory: bool,
    path: &Path,
) -> Result<()> {
    if !is_trusted_sid(owner, installer) {
        bail!(
            "Elevated installation has an untrusted owner: {}",
            path.display()
        );
    }
    if dacl.is_null() || !unsafe { IsValidAcl(dacl) }.as_bool() {
        bail!(
            "Elevated installation has no valid restrictive DACL: {}",
            path.display()
        );
    }

    // A conservative allow-list: denies cannot make an untrusted write grant acceptable.
    // Unknown ACE forms are refused instead of guessing their effective permissions.
    for index in 0..unsafe { (*dacl).AceCount } as u32 {
        let mut pointer = std::ptr::null_mut();
        unsafe {
            GetAce(dacl, index, &mut pointer)?;
        }
        let header = unsafe { &*pointer.cast::<ACE_HEADER>() };
        if u32::from(header.AceFlags) & INHERIT_ONLY_ACE.0 != 0 {
            continue;
        }
        match header.AceType {
            0 => {
                let ace = unsafe { &*pointer.cast::<ACCESS_ALLOWED_ACE>() };
                let sid = PSID((&ace.SidStart as *const u32).cast_mut().cast());
                if grants_modification(ace.Mask, directory) && !is_trusted_sid(sid, installer) {
                    bail!(
                        "Elevated installation is writable without administrator rights: {}",
                        path.display()
                    );
                }
            }
            1 => {} // An access-denied ACE grants no permissions.
            _ => bail!(
                "Unsupported installation ACL; administrator mode was blocked: {}",
                path.display()
            ),
        }
    }
    Ok(())
}

fn grants_modification(mask: u32, directory: bool) -> bool {
    const MODIFY: u32 = 0x0000_0002 // FILE_WRITE_DATA / FILE_ADD_FILE
        | 0x0000_0004 // FILE_APPEND_DATA / FILE_ADD_SUBDIRECTORY
        | 0x0000_0010 // FILE_WRITE_EA
        | 0x0000_0100 // FILE_WRITE_ATTRIBUTES
        | 0x0001_0000 // DELETE
        | 0x0004_0000 // WRITE_DAC
        | 0x0008_0000 // WRITE_OWNER
        | 0x1000_0000 // GENERIC_ALL
        | 0x4000_0000; // GENERIC_WRITE
    mask & (MODIFY | if directory { 0x40 } else { 0 }) != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows::Win32::Security::{GetSecurityDescriptorDacl, GetSecurityDescriptorOwner};
    use windows::core::{BOOL, HSTRING};

    fn check_sddl(sddl: &str) -> Result<()> {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &HSTRING::from(sddl),
                1,
                &mut descriptor,
                None,
            )?;
        }
        let _allocation = LocalAllocation(descriptor.0);
        let mut owner = PSID::default();
        let mut dacl = std::ptr::null_mut();
        let mut present = BOOL::default();
        let mut defaulted = BOOL::default();
        unsafe {
            GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted)?;
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)?;
        }
        let mut trusted = PSID::default();
        unsafe {
            ConvertStringSidToSidW(w!("S-1-5-32-544"), &mut trusted)?;
        }
        let _trusted_allocation = LocalAllocation(trusted.0);
        // These fixtures use only built-in principals; no service SID is needed.
        validate_permissions(owner, dacl, trusted, true, Path::new("fixture"))
    }

    #[test]
    fn protected_machine_acl_accepts_user_read_execute() {
        assert!(check_sddl("O:BAG:BAD:(A;;FA;;;BA)(A;;FA;;;SY)(A;;0x1200a9;;;BU)").is_ok());
    }

    #[test]
    fn rejects_user_control_of_contents_owner_or_permissions() {
        for rights in ["FW", "FA", "WD", "WO", "SD", "0x40"] {
            let sddl = format!("O:BAG:BAD:(A;;FA;;;BA)(A;;{rights};;;BU)");
            assert!(check_sddl(&sddl).is_err(), "accepted {sddl}");
        }
        assert!(check_sddl("O:BAG:BAD:NO_ACCESS_CONTROL").is_err());
        assert!(check_sddl("O:BUG:BAD:(A;;FA;;;BA)(A;;FR;;;BU)").is_err());
    }
}
