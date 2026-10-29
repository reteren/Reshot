use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::FfmpegArgs;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const STDERR_TAIL: usize = 2048;

pub struct Ffmpeg;

pub(crate) struct Process {
    child: Arc<Mutex<Child>>,
    pub stdin: Option<ChildStdin>,
    stderr: Option<JoinHandle<String>>,
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
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("reshot")
            .join("ffmpeg");
        format!(
            "ffmpeg.exe was not found next to Reshot, in its ffmpeg subdirectory, under '{}', or on PATH.",
            appdata.display()
        )
    }

    pub(crate) fn start(arguments: &str, pipe_stdin: bool) -> Result<Process> {
        let executable = Self::executable_path().context(Self::missing_message())?;
        let mut command = Command::new(executable);
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
            .with_context(|| format!("starting {}", executable.display()))?;
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
        })
    }

    pub fn run(arguments: &str) -> Result<(bool, String)> {
        Self::start(arguments, false)?.wait()
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
            for encoder in ["h264_nvenc", "h264_amf", "h264_qsv", "h264_mf"] {
                let args = FfmpegArgs::h264_probe(
                    width,
                    height,
                    encoder,
                    FfmpegArgs::default_tuning(encoder),
                );
                match Self::start(&args, false)
                    .and_then(|process| process.finish(Duration::from_secs(3)))
                {
                    Ok((true, _)) => {
                        chosen = encoder.to_string();
                        break;
                    }
                    Ok((false, reason)) => {
                        log::warn!("ffmpeg {encoder} probe for {width}x{height}: {reason}")
                    }
                    Err(error) => {
                        log::warn!("ffmpeg {encoder} probe for {width}x{height}: {error:#}")
                    }
                }
            }
        }
        guard.insert((width, height), chosen.clone());
        Ok(chosen)
    }
}

fn resolve_path() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let base = executable.parent()?;
    let mut candidates = vec![
        base.join("ffmpeg.exe"),
        base.join("ffmpeg").join("ffmpeg.exe"),
    ];
    if let Some(appdata) = std::env::var_os("APPDATA") {
        candidates.push(
            PathBuf::from(appdata)
                .join("reshot")
                .join("ffmpeg")
                .join("ffmpeg.exe"),
        );
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|dir| dir.join("ffmpeg.exe")));
    }
    candidates
        .into_iter()
        .find(|candidate| fs::metadata(candidate).is_ok_and(|m| m.is_file()))
}
