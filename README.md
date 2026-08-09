<div align="center">

<img src="reshot.png" width="128" alt="Reshot">

# Reshot

**A resident screenshot and screen recording tool for Windows.**

It lives in the tray and burns no cycles until you call it.

[![License: MIT](https://img.shields.io/badge/License-MIT-3C9898.svg)](LICENSE)
[![Windows 10 2004+](https://img.shields.io/badge/Windows-10%202004%2B-3C9898.svg)](#requirements)
[![.NET 8](https://img.shields.io/badge/.NET-8-3C9898.svg)](https://dotnet.microsoft.com/)

</div>

---

## What it is

Press the hotkey and Reshot **freezes every monitor at once** in a single frame. Select an
area of any shape, annotate it, and send it to the clipboard or to a file. Or record the
selected area to video with sound.

The guiding rule is **zero background cost**. No timers, no polling, just a global hotkey
waiting to fire. Idle, that means under 30 MB of memory and roughly 0% CPU.

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

No .NET runtime is required; the build is self-contained.

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

You need the [.NET 8 SDK](https://dotnet.microsoft.com/download), plus
[Node.js 18+](https://nodejs.org/) and [Rust](https://rustup.rs/) for the settings window.

```powershell
dotnet build reshot.sln -c Debug          # build the solution
dotnet test                               # run the core tests
dotnet run --project src/Reshot.App       # run it (minimises to the tray)
```

The settings window is a separate Tauri application:

```powershell
npm install --prefix src/reshot-tauri
npm run tauri build --prefix src/reshot-tauri -- --no-bundle
```

> Build the settings window exactly like that. A plain `cargo build` produces a **dev**
> binary that loads its UI from the local dev server and shows a blank page without it.

One script produces every release artifact (portable archive, installer, checksums). See
[build/README.md](build/README.md):

```powershell
pwsh build/build-release.ps1
```

## Layout

| Project | Purpose |
|---|---|
| `src/Reshot.App` | WPF shell: tray, hotkeys, overlay, OCR, export |
| `src/Reshot.Core` | UI-free core: document model, history, settings, hotkeys |
| `src/Reshot.Capture` | Windows.Graphics.Capture wrapper (frozen frame and live stream) |
| `src/Reshot.Recording` | MP4 and M4A through the bundled ffmpeg and NAudio |
| `src/reshot-tauri` | Settings window: Tauri 2, Rust, TypeScript |
| `tests/Reshot.Core.Tests` | Core unit tests (xUnit) |

Further reading: [ARCHITECTURE.md](ARCHITECTURE.md) for the stack and the reasoning behind
it, [SPEC.md](SPEC.md) for the functional specification, [ROADMAP.md](ROADMAP.md) for the
development plan.

## License

[MIT](LICENSE).
