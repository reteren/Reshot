<div align="center">

<img src="reshot.png" width="128" alt="Reshot">

# Reshot

**A resident screenshot and screen recording tool for Windows.**

It lives in the tray and burns no cycles until you call it.

[![License: MIT](https://img.shields.io/badge/License-MIT-3C9898.svg)](LICENSE)
[![Windows 10 2004+](https://img.shields.io/badge/Windows-10%202004%2B-3C9898.svg)](#requirements)
[![Rust](https://img.shields.io/badge/Rust-1.90%2B-3C9898.svg)](https://www.rust-lang.org/)

</div>

---

## What it is

Press the hotkey and Reshot **freezes every monitor at once** in a single frame. Select an
area of any shape, annotate it, and send it to the clipboard or to a file. Or record the
selected area to video with sound.

The guiding rule is **zero background cost**. No timers, no polling, just a global hotkey
waiting to fire. Idle, that means about 21 MB of memory and 0% CPU: even the UI framework
is only started by the first press.

## Features

**Capture**

- Instant snapshot of every monitor at once, through DXGI Desktop Duplication, with
  Windows.Graphics.Capture as the fallback
- Shaped selections: rectangle, ellipse, triangle, lasso, polygon
- Several selections at once (`Ctrl` + drag)
- The pixel size of the selection is shown at its top-left corner while you size it
- Anything outside a non-rectangular shape becomes transparent, clipboard included

**Annotation**

- Brush, shapes, lines and arrows, text
- Blur and pixelation with adjustable strength
- Three erasers: normal, absolute (down to transparency), and one for filters
- Photoshop-style soft erasing: the centre always erases fully, the rim follows a setting
- Eyedropper, full HSV picker, 16 swatches
- 32 steps of undo
- Every tool keeps its own size, opacity and colour

**Text from an image (OCR)**

- Recognition through the built-in Windows engine: offline, no models, no network
- Recognised words become selectable straight over the frame, so you drag and copy
- AUTO mode runs the Russian and English engines and merges them line by line, because a
  single Windows OCR engine mangles the other alphabet

**Recording**

- MP4 / H.264, encoded by the bundled ffmpeg with hardware acceleration (NVENC, AMF, QSV)
  when available and libx264 otherwise
- Records a selection of any shape, not just a rectangle
- System audio and the microphone are captured **separately**, so the tracks are chosen
  after the recording stops
- Per-application sound through Windows Process Loopback
- A standalone recorder for audio only

**Shell**

- Radial menu on holding the hotkey: it stays up after you let go, and a click runs a
  slice. Optionally a pure gesture instead: point and release the key, no clicking
- Half-Life 2 / Source VGUI styling
- Settings, autostart, single instance

## Install

Prebuilt binaries live on the [Releases](https://github.com/reteren/reshot/releases) page.

| File | What it is |
|---|---|
| `reshot-<version>-setup.exe` | Installer. Installs per user, no administrator rights |
| `reshot-<version>-win-x64-portable.zip` | Portable build: unpack and run `reshot.exe` |

One executable, nothing else to install: no runtime, no WebView2.

The releases bundle a GPL build of ffmpeg (that is what records), which is what adds about
145 MB to every artifact. Reshot itself stays MIT; the binary's licence and the source
offer are in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

> The application is not code signed, so SmartScreen may warn on first launch. That is
> expected: "More info" then "Run anyway". You can verify the download against
> `SHA256SUMS.txt` from the release.

## Requirements

- Windows 10 version 2004 (build 19041) or newer, x64
- Windows 11 for per-application audio capture
- A Windows OCR language pack for Russian text recognition
  (Settings, Language, Russian, Optional features, Optical character recognition)
- Monitors with different DPI scaling are not supported
- **An application running as administrator cannot be captured over** unless Reshot is
  elevated too. Windows forbids a normal program from taking the foreground away from an
  elevated one, so the overlay cannot come up on top of it. Either run that application
  without administrator rights, or turn on **Start with Windows → …as administrator** in
  the settings, which registers a scheduled task so Windows grants the rights once instead
  of prompting at every logon

## Usage

The default hotkey is `PrtScn` (Print Screen) and can be rebound in the settings.

| Action | What happens |
|---|---|
| Tap | The screen freezes and a capture session starts |
| Hold | Radial menu: point at a slice and release the key to run it |
| While recording | Stops the recording and saves the file |

Tools are switched with the digits `1` to `0`, in toolbar order. The full list of shortcuts
lives in the **Info** tab of the settings window, and it is kept in step with the code.

## Building from source

You need [Rust](https://rustup.rs/) 1.90 or newer (MSVC toolchain). Skia arrives prebuilt
with `skia-safe`, so there is no LLVM or ninja to install.

```powershell
cargo test --workspace                    # run the tests
cargo run -p reshot                       # run it (it lives in the tray)
cargo run -p reshot -- --capture          # ... and start a capture right away
cargo run -p reshot -- --settings=info    # ... or open the settings on a tab
```

Recording needs `ffmpeg.exe`; `pwsh build/fetch-ffmpeg.ps1` downloads the pinned build.
A debug build runs beside an installed Reshot without disturbing it; see
[CONTRIBUTING.md](CONTRIBUTING.md).

One script produces every release artifact (portable archive, installer, checksums). See
[build/README.md](build/README.md):

```powershell
pwsh build/build-release.ps1
```

## Layout

| Crate | Purpose |
|---|---|
| `crates/reshot` | The application on [gpui](https://crates.io/crates/gpui): tray menu, overlay, radial menu, recording HUD, settings |
| `crates/reshot-core` | No windows: document model on Skia, undo, selections, settings, hotkeys, export |
| `crates/reshot-capture` | Desktop Duplication snapshots, Windows.Graphics.Capture fallback and stream |
| `crates/reshot-recording` | ffmpeg process, WASAPI and process loopback, separate PCM tracks, MP4/M4A |
| `crates/reshot-ocr` | Windows.Media.Ocr and the AUTO (Russian + English) line merge |
| `crates/reshot-platform` | Tray and hotkeys on their own thread, single instance, autostart, clipboard |

Further reading: [ARCHITECTURE.md](ARCHITECTURE.md) for the stack and the reasoning behind
it, [SPEC.md](SPEC.md) for the functional specification, [ROADMAP.md](ROADMAP.md) for the
development plan, [docs/rust-rewrite.md](docs/rust-rewrite.md) for how the move from C# to
Rust went.

## License

[MIT](LICENSE).
