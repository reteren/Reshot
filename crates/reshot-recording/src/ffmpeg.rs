use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_READ,
};

use crate::FfmpegArgs;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const STDERR_TAIL: usize = 2048;
// Embedded at compile time; replacing a neighbouring checksum file cannot change this pin.
const FFMPEG_SHA256: &str = include_str!("../../../build/ffmpeg.sha256");

pub struct Ffmpeg;

pub(crate) struct Process {
    child: Arc<Mutex<Child>>,
    pub stdin: Option<ChildStdin>,
    stderr: Option<JoinHandle<String>>,
    // The verified file and its parent directories stay pinned until the child exits.
    _executable: Arc<VerifiedExecutable>,
}

struct VerifiedExecutable {
    path: PathBuf,
    _file: File,
    _parents: Vec<File>,
}

impl Process {
    pub fn abort_handle(&self) -> Arc<Mutex<Child>> {
        Arc::clone(&self.child)
    }

    pub fn wait(mut self) -> Result<(bool, String)> {
        drop(self.stdin.take());
        let status = self
            .child
            .lock()
            .expect("ffmpeg child mutex poisoned")
            .wait()?;
        let stderr = self
            .stderr
            .take()
            .and_then(|task| task.join().ok())
            .unwrap_or_default();
        Ok((status.success(), stderr))
    }

    pub fn finish(mut self, timeout: Duration) -> Result<(bool, String)> {
        drop(self.stdin.take());
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self
                .child
                .lock()
                .expect("ffmpeg child mutex poisoned")
                .try_wait()?
            {
                let stderr = self
                    .stderr
                    .take()
                    .and_then(|task| task.join().ok())
                    .unwrap_or_default();
                return Ok((status.success(), stderr));
            }
            if Instant::now() >= deadline {
                let _ = self
                    .child
                    .lock()
                    .expect("ffmpeg child mutex poisoned")
                    .kill();
                let _ = self
                    .child
                    .lock()
                    .expect("ffmpeg child mutex poisoned")
                    .wait();
                let stderr = self
                    .stderr
                    .take()
                    .and_then(|task| task.join().ok())
                    .unwrap_or_default();
                return Ok((
                    false,
                    format!("timed out after {} ms: {stderr}", timeout.as_millis()),
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Ffmpeg {
    pub fn executable_path() -> Option<&'static Path> {
        static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
        PATH.get_or_init(resolve_path).as_deref()
    }

    pub fn is_available() -> bool {
        Self::executable_path().is_some()
    }

    pub fn missing_message() -> String {
        "The bundled ffmpeg.exe is missing. Reinstall Reshot to restore screen and audio recording.".into()
    }

    pub(crate) fn start(arguments: &str, pipe_stdin: bool) -> Result<Process> {
        let executable = verified_executable()?;
        let mut command = Command::new(&executable.path);
        command
            .raw_arg(arguments)
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if pipe_stdin {
            command.stdin(Stdio::piped());
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("starting {}", executable.path.display()))?;
        let stdin = child.stdin.take();
        let mut stderr = child
            .stderr
            .take()
            .context("ffmpeg stderr pipe unavailable")?;
        // Drain while the process runs. Waiting before reading can deadlock when stderr fills.
        let reader = thread::spawn(move || {
            let mut tail = Vec::with_capacity(STDERR_TAIL);
            let mut chunk = [0u8; 512];
            loop {
                match stderr.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        tail.extend_from_slice(&chunk[..count]);
                        if tail.len() > STDERR_TAIL {
                            tail.drain(..tail.len() - STDERR_TAIL);
                        }
                    }
                }
            }
            String::from_utf8_lossy(&tail).trim().to_string()
        });
        Ok(Process {
            child: Arc::new(Mutex::new(child)),
            stdin,
            stderr: Some(reader),
            _executable: executable,
        })
    }

    pub fn run(arguments: &str) -> Result<(bool, String)> {
        Self::start(arguments, false)?.wait()
    }

    /// Whether `encoder` opens and encodes a few frames at this size.
    fn probe(encoder: &str, width: u32, height: u32) -> bool {
        let args =
            FfmpegArgs::h264_probe(width, height, encoder, FfmpegArgs::default_tuning(encoder));
        match Self::start(&args, false).and_then(|process| process.finish(Duration::from_secs(3))) {
            Ok((true, _)) => true,
            Ok((false, reason)) => {
                log::warn!("ffmpeg {encoder} probe for {width}x{height}: {reason}");
                false
            }
            Err(error) => {
                log::warn!("ffmpeg {encoder} probe for {width}x{height}: {error:#}");
                false
            }
        }
    }

    /// Probe at the actual even dimensions; ffmpeg's encoder list does not prove GPU or
    /// minimum-size support. Cache per size so a tiny crop cannot poison a larger one.
    pub fn select_h264_encoder(width: u32, height: u32) -> Result<String> {
        if width == 0 || height == 0 {
            bail!("invalid encoder dimensions");
        }
        static CACHE: OnceLock<Mutex<HashMap<(u32, u32), String>>> = OnceLock::new();
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let mut guard = cache.lock().expect("encoder cache mutex poisoned");
        if let Some(value) = guard.get(&(width, height)) {
            return Ok(value.clone());
        }
        let mut chosen = "libx264".to_string();
        if Self::is_available() {
            // Every probe is a whole ffmpeg launch (70-120 ms) and a new recording size
            // probes again, so they run side by side: recording starts after the slowest one
            // rather than after all four in turn. The usual preference still decides.
            let passed: Vec<bool> = std::thread::scope(|scope| {
                let probes: Vec<_> = HARDWARE_ENCODERS
                    .iter()
                    .map(|&encoder| scope.spawn(move || Self::probe(encoder, width, height)))
                    .collect();
                probes
                    .into_iter()
                    .map(|p| p.join().unwrap_or(false))
                    .collect()
            });
            if let Some(i) = passed.iter().position(|&ok| ok) {
                chosen = HARDWARE_ENCODERS[i].to_string();
            }
        }
        guard.insert((width, height), chosen.clone());
        Ok(chosen)
    }
}

/// Hardware encoders, most preferred first.
const HARDWARE_ENCODERS: [&str; 4] = ["h264_nvenc", "h264_amf", "h264_qsv", "h264_mf"];

fn resolve_path() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let base = executable.parent()?;
    let bundled = base.join("ffmpeg.exe");
    if bundled.is_file() {
        return Some(bundled);
    }

    // Development builds and unit tests may use only the repository's pinned copy.
    // Release application code never contains this fallback.
    #[cfg(any(test, debug_assertions))]
    if let Ok(path) = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../build/ffmpeg/ffmpeg.exe")
        .canonicalize()
        && path.is_file()
    {
        return Some(path);
    }

    None
}

fn verified_executable() -> Result<Arc<VerifiedExecutable>> {
    // Concurrent hardware probes share one verification while the same immutable file is
    // held open. Once the last child releases it, the next launch hashes it again.
    static VERIFIED: OnceLock<Mutex<Weak<VerifiedExecutable>>> = OnceLock::new();
    let mut cached = VERIFIED
        .get_or_init(|| Mutex::new(Weak::new()))
        .lock()
        .map_err(|_| anyhow::anyhow!("FFmpeg verification lock poisoned"))?;
    if let Some(executable) = cached.upgrade() {
        return Ok(executable);
    }
    let path = Ffmpeg::executable_path().context(Ffmpeg::missing_message())?;
    let executable = Arc::new(verify_executable(path, FFMPEG_SHA256.trim())?);
    *cached = Arc::downgrade(&executable);
    Ok(executable)
}

fn verify_executable(path: &Path, expected: &str) -> Result<VerifiedExecutable> {
    if !path.is_absolute() {
        bail!("FFmpeg path must be absolute");
    }
    // Pin every parent before opening the EXE: a file lock alone would still allow a
    // writable parent directory to be renamed and replaced between hashing and spawn.
    let mut parents = Vec::new();
    let ancestors: Vec<_> = path
        .parent()
        .context("FFmpeg has no parent directory")?
        .ancestors()
        .filter(|p| p.has_root())
        .collect();
    for parent in ancestors.into_iter().rev() {
        let directory = OpenOptions::new()
            .access_mode(0)
            .share_mode(FILE_SHARE_READ.0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
            .open(parent)
            .with_context(|| format!("locking FFmpeg directory {}", parent.display()))?;
        let metadata = directory.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            bail!(
                "FFmpeg directory must be a regular directory, not a link: {}",
                parent.display()
            );
        }
        parents.push(directory);
    }
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .with_context(|| {
            format!(
                "opening bundled FFmpeg {} (writes and replacement must be blocked)",
                path.display()
            )
        })?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        bail!("Bundled FFmpeg must be a regular file, not a link");
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .context("reading bundled FFmpeg for verification")?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        log::error!("FFmpeg SHA-256 mismatch for {}: expected {expected}, got {actual}", path.display());
        bail!(
            "Bundled FFmpeg SHA-256 mismatch. Recording was blocked; reinstall Reshot."
        );
    }
    Ok(VerifiedExecutable {
        path: path.to_path_buf(),
        _file: file,
        _parents: parents,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn rejects_modified_content_and_non_files_without_execution() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ffmpeg.exe");
        fs::write(&path, b"modified").unwrap();
        let error = verify_executable(&path, ABC_SHA256).err().unwrap();
        assert!(error.to_string().contains("SHA-256 mismatch"));
        assert!(verify_executable(directory.path(), ABC_SHA256).is_err());
        assert!(verify_executable(Path::new("ffmpeg.exe"), ABC_SHA256).is_err());
    }

    #[test]
    fn pins_verified_file_and_parent_until_guard_is_released() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("bundle");
        fs::create_dir(&parent).unwrap();
        let path = parent.join("ffmpeg.exe");
        fs::write(&path, b"abc").unwrap();
        let guard = verify_executable(&path, ABC_SHA256).unwrap();
        assert!(OpenOptions::new().write(true).open(&path).is_err());
        assert!(fs::remove_file(&path).is_err());
        assert!(fs::rename(&path, parent.join("replacement.exe")).is_err());
        let renamed = directory.path().join("renamed");
        assert!(fs::rename(&parent, &renamed).is_err());
        drop(guard);
        fs::rename(&parent, &renamed).unwrap();
        fs::write(renamed.join("ffmpeg.exe"), b"modified").unwrap();
        assert!(verify_executable(&renamed.join("ffmpeg.exe"), ABC_SHA256).is_err());
    }
}
