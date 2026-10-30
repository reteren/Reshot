# Reshot → Rust: how the rewrite went

Goal: the same product, feature for feature and pixel for pixel, rewritten in Rust. The C#
solution (`src/`, `tests/`, `reshot.sln`) and the Tauri settings window were the reference
until 1.2.4; they were deleted in 1.2.5 and live on in git history (the `main` branch up to
the merge of `rust-rewrite`). Paths below that start with `src/` refer to that history.

UI: **gpui** (Zed's framework) for every window — overlay, HUD, radial menu, tray menu,
recording HUD, audio-track prompt, settings. It must look the same as today's HL2 / Source
VGUI styling (`src/Reshot.App/Theme/SourceVgui.xaml`, `src/reshot-tauri/src/styles.css`).

Pinned to what gpui 0.2.2 already pulls in: `windows 0.61`.

## Crates

| Crate | Ports | Notes |
|---|---|---|
| `reshot-core` | `Reshot.Core` | No UI, no Win32. Document model on Skia (`skia-safe`, as SkiaSharp before it), undo (32), settings JSON, hotkey parsing, session state machine, logger, font catalogue. Tests ported from `tests/Reshot.Core.Tests`. |
| `reshot-capture` | `Reshot.Capture` | DXGI Desktop Duplication snapshot + WGC fallback + WGC monitor stream. Only place with D3D/DXGI/WGC interop. |
| `reshot-recording` | `Reshot.Recording` | ffmpeg process + args builders, WASAPI loopback/mic, process loopback, PCM tracks, mux, M4A. Tests ported from `tests/Reshot.Recording.Tests`. Only place that builds an ffmpeg command line. |
| `reshot-ocr` | `Reshot.App/Ocr/OcrService.cs` | Windows.Media.Ocr, AUTO/RU/EN merge. |
| `reshot-platform` | `Reshot.App/{Input,Platform,Interop,SingleInstance}`, tray | RegisterHotKey on a message-only window, tap/hold detector, tray icon, single instance, autostart (Run key / scheduled task), clipboard PNG, `WDA_EXCLUDEFROMCAPTURE`. |
| `reshot` (bin) | `Reshot.App` + `reshot-tauri` | gpui composition root and every window. |

## Phases

Each phase ends with something runnable.

| # | Scope | Owner | State |
|---|---|---|---|
| R0 | Workspace, roadmap, gpui spike: transparent topmost popup over the virtual desktop, HWND access, exclude-from-capture | Claude | done |
| R1 | `reshot-core`: settings, hotkey parsing, session state machine, log, document model, history, tools rasterisation, font catalogue + ported tests | Claude | done |
| R2 | `reshot-capture`: DD snapshot, WGC fallback, WGC stream | Codex | done |
| R3 | `reshot-recording`: ffmpeg, WASAPI, process loopback, mux, M4A + ported tests | Codex | done |
| R4 | `reshot-ocr` | Codex | done |
| R5 | `reshot-platform`: hotkey, tap/hold, tray, single instance, autostart, clipboard | Claude | done |
| R6 | App shell: tray menu window, capture → overlay with rectangular selection, copy/save | Claude | done |
| R7 | Overlay parity: all selection shapes, toolbar + flyouts, drawing, erasers, effects, text, eyedropper, undo, OCR layer | Claude | built; manual pass pending |
| R8 | Recording UI: REC HUD, corner brackets, audio-track prompt, radial menu | Codex | built; manual pass pending |
| R9 | Settings window in gpui (replaces Tauri), measured against the webview in Edge tab by tab | Codex + Claude | done; interactive pass pending |
| R10 | Release: build script, installer, portable zip, version bump; delete C# + Tauri; workspace at the repo root; CI on cargo | Claude | done (installer 1.2.5 not built yet) |

## Rules for everyone working here

- Behaviour reference = `SPEC.md` and, for what it leaves open, the 1.2.4 C# code in git
  history. When they disagree, the code wins and the difference is noted here.
- Keep the layer boundaries from `ARCHITECTURE.md`: core knows nothing about windows,
  interop stays in its crate.
- `cargo clippy --workspace` and `cargo test --workspace` must pass before handing work back.
- No placeholders, no `todo!()` left in a finished phase.

## Notes learned on the way

- **gpui quits when its last window closes** (Windows). The app keeps one hidden anchor
  window for its whole life.
- **Win32 calls that re-enter the window procedure** (`SetWindowPos`, `SetForegroundWindow`,
  `ShowWindow`) must not run inside a gpui `update`/`defer`: the procedure finds the `App`
  already borrowed ("RefCell already borrowed"). `winutil::after_update` runs them in a
  foreground task instead.
- **gpui embeds its own manifest** (PerMonitorV2, common controls v6); a second MANIFEST
  resource fails the link, so the app only embeds its icon.
- **skia-safe 0.156 / bindgen 0.73.2**: bindgen resolves `syn 3` with `prettyplease 0.2`
  (syn 2) and fails to build. The lockfile pins bindgen 0.73.2 to `prettyplease 0.3.0`; do not
  `cargo update` it away. The prebuilt Skia has no WebP encoder, so WebP goes through libwebp.
- **Debug builds** use their own single-instance names and `reshot-dev.log`, and never touch
  autostart, so they run next to an installed Reshot without disturbing it.

- **Idle memory**: gpui creates its Direct3D device and DirectWrite factory when it starts,
  about 60 MB of working set. So gpui starts on first use (hotkey, tray, second launch);
  until then the process is the tray thread and the hotkeys: measured 20.8 MB working set,
  3.9 MB private, 0 CPU. The first press after boot pays the UI start, as the first WPF
  window did in the C# build.

- **gpui wraps text a word early compared with Chrome**: its wrapper counts the space after a
  line's last word against the width (browsers let it hang) and sums unkerned advances. The
  settings dialog breaks its paragraphs itself with kerned measurements
  (`settings_window/vgui.rs`, `wrapped`); a 0.5 px tolerance covers the kerning pairs
  DirectWrite skips and HarfBuzz applies, calibrated against Edge on the dialog's own lines.
- **gpui leaves kerning off** (it hands DirectWrite an explicit, empty feature set); text that
  must measure like a browser asks for `kern` explicitly.
- **Fonts with the same family name collide**: the dialog's Roboto 3.015 is embedded as
  "Reshot Roboto", because an installed Roboto 2.x answers to "Roboto" with other metrics.
- **`window.refresh()` from inside a prepaint can be lost** when that frame finishes; defer it
  (`window.defer(cx, |w, _| w.refresh())`). Seen as a first-frame layout left on screen.
- **Comparing against the webview**: the Tauri page renders in headless Edge with a mocked
  `__TAURI_INTERNALS__` (`invoke` answering `load_settings` and friends), and the gpui
  window opens on any tab with `--settings=<tab>`; screenshot both and compare row by row.

## Deliberate deviations from the C# build

- Settings dialog: no `text-align: justify` and no `letter-spacing` on the title; gpui has
  neither. Line breaks, boxes and colours match the webview.
- None other. The outlined crosshair, which gpui cannot set, is restored by subclassing the
  overlay window and swapping the stock crosshair on WM_SETCURSOR.
