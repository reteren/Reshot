# Reshot in Rust

The Rust rewrite of Reshot: the same product, feature for feature. The plan, its state and
the lessons learned on the way are in [ROADMAP-RUST.md](ROADMAP-RUST.md).

## Layout

| Crate | What it is |
|---|---|
| `crates/reshot-core` | No windows: settings, hotkey parsing, session state machine, logger, the document model on Skia, undo, selections, the editing session, export |
| `crates/reshot-capture` | Desktop Duplication snapshots, Windows.Graphics.Capture fallback and stream |
| `crates/reshot-recording` | ffmpeg process, WASAPI and process loopback, PCM tracks, MP4/M4A mux |
| `crates/reshot-ocr` | Windows.Media.Ocr with the AUTO (RU+EN) line merge |
| `crates/reshot-platform` | Tray and global hotkeys on their own thread, single instance, autostart, clipboard, Win32 window helpers |
| `crates/reshot` | The app on gpui: tray menu, overlay, radial menu, recording HUD, settings window |

## Build and run

```
cargo test --workspace
cargo run -p reshot -- --capture
cargo run -p reshot -- --settings=info   # settings window, optionally on a tab
```

A debug build (and the `release-dev` profile) runs next to an installed Reshot without
disturbing it: it uses its own single-instance names, logs to
`%AppData%\reshot\logs\reshot-dev.log`, never touches autostart, and its tray icon says
"Reshot (dev)". It shares `settings.json` with the installed app. While the installed app
owns the hotkey, start captures with `--capture` or a double click on the dev tray icon.

```
cargo build -p reshot --profile release-dev   # optimised, still side-by-side safe
pwsh ../build/build-release-rust.ps1           # release artifacts into ../dist
```

Recording needs `ffmpeg.exe` next to `reshot.exe`, in an `ffmpeg` folder beside it, under
`build/ffmpeg`, or on `PATH`; `build/fetch-ffmpeg.ps1` fetches the pinned build.

## Toolchain notes

- Skia comes prebuilt through `skia-safe`; no LLVM or ninja is needed as long as no extra
  Skia features are enabled (the prebuilt set has no WebP encoder, which is why WebP goes
  through libwebp).
- Do not `cargo update` blindly: the lockfile pins `bindgen 0.73.2` to `prettyplease 0.3.0`,
  without which skia-bindings fails to build. See ROADMAP-RUST.md.
