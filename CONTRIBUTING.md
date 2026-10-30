# Contributing

## What you need

- Windows 10 2004+ x64
- [Rust](https://rustup.rs/) 1.90 or newer with the MSVC toolchain

Skia comes prebuilt through `skia-safe`, so no LLVM or ninja is needed as long as no extra
Skia features are enabled.

```powershell
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo run -p reshot -- --capture          # start a capture right away
cargo run -p reshot -- --settings=info    # open the settings window on a tab
```

Recording needs `ffmpeg.exe`. `pwsh build/fetch-ffmpeg.ps1` downloads the pinned build into
`build/ffmpeg`, where a debug build looks for it; a release build looks next to
`reshot.exe`, in an `ffmpeg` folder beside it, in `%AppData%\reshot\ffmpeg`, then on `PATH`.

## Running next to an installed Reshot

A debug build (and the `release-dev` profile, optimised with debug assertions on) uses its
own single-instance names, logs to `%AppData%\reshot\logs\reshot-dev.log`, never touches
autostart, and says "Reshot (dev)" in the tray. It shares `settings.json` with the installed
app. While the installed app owns the hotkey, start captures with `--capture` or a double
click on the dev tray icon.

```powershell
cargo build -p reshot --profile release-dev
```

## How the workspace is laid out

| Crate | Purpose |
|---|---|
| `crates/reshot` | The application on gpui: composition root and every window |
| `crates/reshot-core` | **No windows, no Win32**: document model, history, selections, editor, settings, hotkeys |
| `crates/reshot-capture` | The only place Desktop Duplication and Windows.Graphics.Capture live |
| `crates/reshot-recording` | ffmpeg and WASAPI; the only place an ffmpeg command line is built |
| `crates/reshot-ocr` | Windows.Media.Ocr |
| `crates/reshot-platform` | Tray, hotkeys, single instance, autostart, clipboard, Win32 window helpers |

Boundaries worth keeping:

1. **`reshot-core` knows nothing about windows.** That is what lets the document model, the
   history and the settings be unit tested without a display.
2. **D3D, DXGI and WGC live only in `reshot-capture`.** The rest of the code sees
   `ScreenCaptureService`.
3. **Everything that talks to ffmpeg lives in `reshot-recording`.**
4. **`settings.json` keeps the key names earlier versions wrote**, so an upgrade keeps the
   user's settings.

## Things that bite

- **Do not `cargo update` blindly.** The lockfile pins `bindgen 0.73.2` to `prettyplease
  0.3.0`; without that, skia-bindings fails to build. CI builds with `--locked`.
- **gpui quits when its last window closes**, so the app keeps one hidden anchor window.
- **Win32 calls that re-enter the window procedure** (`SetWindowPos`, `SetForegroundWindow`,
  `ShowWindow`) must not run inside a gpui update: `winutil::after_update` defers them.
- **A refresh asked for while a frame is drawn can be lost**; ask for it with
  `window.defer(cx, |window, _| window.refresh())`.
- More of these, with the reasoning, are in [docs/rust-rewrite.md](docs/rust-rewrite.md).

## Style

- Comments explain **why**, they do not restate the code
- Documentation, comments and the user interface are all in English
- New logic in `reshot-core` comes with tests

## Verifying changes

A running `reshot.exe` holds a lock on its own file, so close it before rebuilding:

```powershell
taskkill /IM reshot.exe /F
```

The overlay is a fullscreen window and launching it for every check is disruptive. Whatever
can be verified without it should be: the core logic is covered by unit tests, and the
settings window opens on its own with `--settings=<tab>`.

## Building a release

See [build/README.md](build/README.md).
