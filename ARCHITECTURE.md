# Reshot architecture

## 1. Stack: Rust

**Decision: Rust, as of 1.2.5.** Reshot was written in C# / .NET 8 with WPF up to 1.2.4,
with the settings window as a separate Tauri application. It was rewritten in Rust, feature
for feature and pixel for pixel (the record of how is [docs/rust-rewrite.md](docs/rust-rewrite.md)),
for three reasons:

- **One UI technology instead of two.** The overlay was WPF and the settings window a
  WebView2 page in another process; the Half-Life 2 / Source look had to be built twice and
  kept in step twice. Now every window is gpui.
- **Idle cost.** The resident process is the tray thread and the hotkeys and nothing else:
  about 21 MB of working set and 0% CPU, with no runtime and no UI framework loaded until
  the first press (§7).
- **One executable.** No .NET runtime published beside it, no second binary for the settings.

The same Skia renders the image as before (SkiaSharp then, `skia-safe` now), so documents,
erasers and exports produce the same pixels.

### Components

| Layer | Technology | Why |
|---|---|---|
| Language | Rust 2024, MSVC toolchain, x64 | One native executable |
| Every window | **gpui** (Zed's UI framework): overlay, toolbar, radial menu, tray menu, recording HUD, audio-track prompt, settings | GPU-drawn, fully stylable; the VGUI bevels are plain elements |
| Editor canvas | **Skia** through `skia-safe` (prebuilt) | Brushes, alpha, blur, pixelation, arbitrary paths and text, all in one library |
| Screen capture | **DXGI Desktop Duplication** for snapshots, **Windows.Graphics.Capture** for recording and as the snapshot fallback, through the `windows` crate | See §6: duplication hands back a desktop that never had a cursor in it, WGC has to switch the cursor off to match and that blinks |
| Global hotkey and tray | `RegisterHotKey` and `Shell_NotifyIcon` on a message-only window, on their own thread | Zero background cost: the message arrives in a message loop |
| Settings | `serde_json` writing `%AppData%\reshot\settings.json` | The key names are the ones 1.2.4 wrote, so an upgrade keeps them |
| Video | A bundled ffmpeg: BGRA frames piped in as rawvideo, H.264 encode (`h264_nvenc` / `h264_amf` / `h264_qsv` when available, `libx264` otherwise), AAC encode and final mux; WASAPI through the `wasapi` crate for the audio capture | Bundled ffmpeg costs about 145 MB per artifact and brings a GPL licence obligation (§9) |
| Text recognition | `Windows.Media.Ocr` | Offline, no models to ship, already part of the OS |
| Settings dialog font | Roboto 3.015, embedded | The dialog is laid out to the pixel of the 1.2.4 webview, which used it |

## 2. Workspace layout

```
reshot/
├── crates/
│   ├── reshot/              the application: composition root and every window (gpui)
│   │   └── src/
│   │       ├── overlay/     session window, toolbar, tool panels, OCR layer
│   │       ├── recording/   recording HUD, corner brackets, audio-track prompt
│   │       ├── settings_window/
│   │       ├── radial.rs    hold-to-open quick menu
│   │       └── tray_menu.rs
│   ├── reshot-core/         no windows: document, tools, history, selections, editor,
│   │                        export, settings, hotkeys, session state machine, fonts, log
│   ├── reshot-capture/      Desktop Duplication + Windows.Graphics.Capture
│   ├── reshot-recording/    ffmpeg process and arguments, WASAPI, PCM tracks, mux
│   ├── reshot-ocr/          Windows.Media.Ocr
│   └── reshot-platform/     tray and hotkeys thread, single instance, autostart, clipboard
├── build/                   release script and the installer
└── docs/
```

Rules:

- **`reshot-core` knows nothing about windows or Win32.** Every tool, the document model and
  the editing session are testable without a display.
- **D3D, DXGI and WGC live only in `reshot-capture`**; the rest sees `ScreenCaptureService`.
- **Only `reshot-recording` builds an ffmpeg command line.**

## 3. Document model

An editing session is a `CaptureDocument`:

```
CaptureDocument
├── base frame            the frozen capture of every monitor (immutable, held by the app)
├── EffectsLayer          blur and pixelation over the untouched base
├── PaintLayer            brush strokes plus rasterised shapes and text
└── AbsoluteMask          coverage of the Absolute Eraser, punched out on export
```

Why it is split this way:

- **The Filter Eraser must return the original pixels**, so effects cannot be burnt into
  the base. They need their own layer over an untouched original.
- **The normal Eraser only removes user drawing**, so PaintLayer is separate from the base.
- **The Absolute Eraser erases everything down to transparency**, which is an alpha mask
  applied to the final composite.

Shapes and text are **baked into PaintLayer on commit**, so the eraser treats them exactly
like brush strokes, pixel by pixel. An earlier design kept a movable vector layer; it was
dropped because "erase part of an arrow" is worth more than "move an arrow afterwards".

Export composition: base, then effects, then paint, cropped to the union of the selection
paths, with everything outside those paths transparent.

### Fonts

`reshot-core/src/fonts.rs` is the single place that enumerates the installed families and
turns a family name into a Skia `Typeface`. Everything that draws or measures
text goes through it — the text object, the caret the overlay draws while typing, the
picker in the tool panel — and nothing else asks Skia for a typeface by name, so a name
is resolved and cached once, and an unknown one falls back to Segoe UI in exactly one
place. The catalogue is built lazily and off the UI thread, because enumerating every
installed family is slow enough to be felt on the first click.

It lives in core rather than in the UI crate for the reason at the end of §2: core owns
rendering and knows nothing about windows, and the same typeface has to serve both the
Skia canvas and the family name stored in the settings model.

The trap worth knowing about: the picker's list is rendered by **gpui** (DirectWrite), which quietly
substitutes another font for glyphs the chosen family does not have, while the screenshot
is rendered by **Skia**, which does not substitute anything. A Latin-only family therefore
previews Cyrillic perfectly and then draws it as empty boxes on the image. That asymmetry
is why `FontEntry` carries `supports_cyrillic` and why the picker dims those rows — the
preview cannot be trusted to show the failure by itself.

## 4. Soft strokes: hardness and coverage

Brush, blur, pixelation and the three erasers share one stroke model, taken from hive
(`app/src/drawing/brush.ts`) and implemented in `reshot-core/src/soft.rs`:

```
core     = clamp(r * hardness, 0, r - 1)       never closer than 1 px to the rim
alpha(d) = 1                                    d <= core
alpha(d) = 1 - smoothstep((d - core) / (r - core))   core < d <= r
alpha(d) = 0                                    d > r
```

`d` is the distance to the stroke's **segment**, not to a dab centre, so a soft stroke is
one continuous shape with no ripples from discrete dabs. Two more pieces of hive keep it
free of creases:

- **Corners are curves.** A soft polyline shows each corner as a diagonal crease on its
  inner side, so the mouse points are joined by a centripetal Catmull-Rom curve cut into
  pieces that turn by at most 3°. The curve through a point needs the next one, so the last
  piece up to the pointer is provisional: the pixels under it are saved, and the next move
  puts them back before drawing the piece for real.
- **Crossings composite.** Every pixel remembers the path length at which the stroke last
  touched it. Touched again within `max(4, 1.5 r)` of path (neighbouring pieces, a resting
  pointer) it keeps the maximum, so joints never build up; touched by a later pass
  (crossing, going back over) the new pass is composited over the earlier ones,
  `a + b (1 − a)`, with a smooth blend between the two rules over the next two windows.
  Pure max left dark diagonal creases at crossings, where both passes give the same alpha.

The stroke is rasterised into a greyscale **coverage bitmap** whose luminance is the
strength, and a move rewrites only what its pieces touch. Strength scales the whole coverage
once, so the solid part never builds up past it. How the finished coverage is used depends
on the tool:

- **Brush**: tinted with the colour and laid on the paint layer once, at the brush opacity.
- **Blur, pixelation**: the effect source is drawn through it onto the effects layer.
- **Erasers**: the eraser opacity is the coverage's strength, and it is applied once with
  `BlendMode::DstOut` (or accumulated into the punch-through mask for the Absolute Eraser).

Shapes and lines are vectors until committed, so they soften differently: their outline is
blurred by `width × (1 − hardness) × 0.35`, as hive does. Text has no hardness.

## 5. Undo and redo

Command pattern, 32 steps deep.

Raster operations (brush, eraser, effect) store a snapshot of the affected region before
and after, and undo restores it. The original design called for 256x256 tiles so a stroke
would only cost a few tiles; the shipped implementation snapshots the whole bounding
region instead, which is simpler and has been adequate in practice but costs more memory
on very large strokes.

## 6. Capture and multi-monitor

- On the hotkey, **every monitor is captured at once** and the frames are composed into a
  single bitmap in virtual-desktop coordinates. That is a hard requirement for "Ctrl+A then
  Ctrl+A selects all monitors": the selection can only grow if the pixels are already there.
- **The snapshot goes through DXGI Desktop Duplication, not WGC.** SPEC §3 wants a frame
  with no cursor in it. WGC can only deliver that by asking the compositor to leave the
  cursor out, which forces it off its hardware plane for the life of the session — one
  visible blink on every screenshot, and no way around it from our side. A duplicated
  desktop simply never contains the cursor (verified on hardware: parking the cursor on a
  provably static patch changes not one channel value of it).
  - The first frame after `DuplicateOutput` is a handshake with an **empty surface**;
    copying it yields a black screenshot. Only a frame reporting `AccumulatedFrames > 0`
    holds desktop content, so the acquire is a short retry loop, not a single call.
  - Duplication is bound to the adapter driving the display, refuses rotated and non-BGRA
    outputs, and loses to an exclusive-fullscreen game. Every one of those throws, and
    `ScreenCaptureService` falls back to WGC for that capture. A structural refusal
    (a second adapter, a rotated display) is remembered so it is not re-tested per capture.
- **Recording stays on WGC**: it needs a live stream, and the cursor question does not
  arise the same way.
- The overlay is one borderless window spanning the whole virtual desktop.
- The freeze runs **on a worker thread**, not the UI thread: device creation plus the wait
  for the first frame is the bulk of the hotkey-to-overlay time, and holding the UI thread
  for it froze the app instead of merely delaying it.
- It starts only once the press is known to be a **tap**. Starting it on key-down instead,
  in parallel with hold detection, is tempting — it saves the user's own key-hold time —
  but opening a capture session takes the cursor off its hardware plane, so every press
  blinks, and a hold pays that plus the GPU cost of capturing a running game for a frame
  that is then discarded. The key-hold time is the cheaper thing to spend.
- A monitor that does not deliver a first frame within 1.2 s is a fullscreen game refusing
  capture, and the wait exists only to reach the `CreateForWindow` fallback below.
- **Elevation is a hard wall.** Reshot runs `asInvoker` (ARCHITECTURE §10: the installer is
  per user and needs no administrator rights). Against a window running elevated, UIPI
  blocks `SetForegroundWindow`, `AttachThreadInput` and injected input alike, so the
  overlay cannot take the foreground no matter how it asks. The only real answers are to
  run that application unelevated, or to run Reshot elevated as well. The mechanism built
  for this case, `uiAccess="true"`, needs a signed binary installed under Program Files,
  and there is no code signing (§10).
- Protected content: Windows.Graphics.Capture returns whatever the system allows. Windows
  with a DRM flag may come back black, which is a platform limitation.
- The yellow Windows capture border is disabled through `IsBorderRequired = false`, which
  needs the Windows 11 SDK projection and a borderless access request.

## 7. Zero background cost

- The background process is a message-only window plus `RegisterHotKey` plus the tray, on
  their own thread. No timers, no polling, no watchers.
- **gpui is not started at startup.** Its Direct3D device and DirectWrite factory cost about
  60 MB of working set, so the UI starts on first use (hotkey, tray, a second launch) and
  the first press is that much slower, as the first WPF window was in the C# build.
  Measured idle before that: 20.8 MB working set, 3.9 MB private, 0 CPU.
- Every capture resource (D3D device, frame pool) is released when the session closes.

## 8. Input routing

One input router per session, highest priority first:

1. Open panels and toolbar flyouts. A right-click on the toolbar never reaches the
   eyedropper.
2. Context modifiers for the tool (`Shift`, `Ctrl`).
3. The active tool.

Session states are a small state machine:

```
Idle → Capturing → Selecting → Editing → Exporting → Idle
Editing → Esc → Selecting → Esc → Idle
Editing → Recording → Idle
```

## 9. Video

- Live capture through the same Windows.Graphics.Capture frame pool, cropped to the
  selection. The frames go to ffmpeg as BGRA rawvideo on its stdin; ffmpeg does the H.264
  encode — hardware `h264_nvenc` / `h264_amf` / `h264_qsv` when available, `libx264`
  otherwise — the AAC encode and the final mux.
- The frames are piped rather than letting ffmpeg capture the screen itself (ddagrab)
  because everything that makes a Reshot recording what it is lives on Reshot's side and
  cannot survive a capture owned by ffmpeg: the selection can be any shape, so the mask is
  applied before frames leave the process; per-application audio is Windows process
  loopback; and the two tracks must stay separate until the user picks.
- Audio: independent WASAPI streams (loopback for system sound, capture for the
  microphone, process loopback for individual applications). Each source is written to its
  **own raw PCM file** during the recording.
- On stop, the chosen tracks are piped to ffmpeg too and encoded to AAC as the final MP4 is
  muxed. The video is already encoded, so picking tracks costs no video quality and little
  time. This is what makes the post-recording track picker honest: nothing is mixed until
  the user picks.
- The price of not writing our own encoder: about 145 MB of ffmpeg in every artifact, and a
  GPL binary inside an otherwise MIT project. The binary's licence and the source offer are
  shipped in `THIRD-PARTY-NOTICES.md`.
- The recording indicator is a separate small topmost window, and the corner brackets are a
  click-through window with a transparent background.
- Stop is the same global hotkey, which the router interprets as Stop while recording.

## 10. Distribution

- A portable ZIP and an Inno Setup installer, both produced by `build/build-release.ps1`.
  The installer is per user and needs no administrator rights.
- The application is one `reshot.exe`; nothing else is installed beside it except ffmpeg.
- A GPL `ffmpeg.exe` is bundled with the installer and the portable ZIP. It is what
  records, it adds roughly 145 MB, and it carries a licence obligation Reshot does not
  share: the binary is GPL even though Reshot stays MIT, so its licence and the source
  offer are shipped in `THIRD-PARTY-NOTICES.md`.
- There is no code signing (open source, no budget), so SmartScreen will complain at first.
  The README explains it.
- **Automatic updates are not implemented.** Velopack was planned and the `update.auto`
  setting exists in the UI, but nothing is wired to it yet.
- License: MIT.
