# Reshot architecture

## 1. Stack: C# / .NET 8

**Decision: C# rather than Rust.** The project is written together with an AI assistant by
an author without deep programming experience, and C# wins on every criterion that matters
for that situation:

- Compiler and runtime errors are readable and easy to feed back into the assistant. In
  Rust, fighting the borrow checker as a beginner stretches every phase severalfold.
- There is a huge amount of open source to learn from. **ShareX** (C#) is effectively a
  reference implementation of half of Reshot.
- First-class access to the Windows API: Windows.Graphics.Capture, DXGI Desktop
  Duplication, WASAPI, the registry, the tray. All of it is either in the BCL or one NuGet
  away.
- Performance is more than sufficient. The frame is frozen, so editing is operations on a
  single bitmap rather than a GPU render every frame.

The tradeoff is accepted honestly: roughly 40 to 60 MB of RAM during an active editing
session and 25 to 30 MB in the background. That is normal for a utility, and the "under
30 MB idle" goal is reachable because WPF is not loaded until the first call (see §7).

### Components

| Layer | Technology | Why |
|---|---|---|
| Runtime | .NET 8 (self-contained, win-x64) | One installer, no runtime prerequisite |
| Overlay and UI | WPF (a borderless topmost window per stage) | Mature, flexible, easy to style dark |
| Editor canvas | **SkiaSharp** (`SKElement`) | Brushes, alpha, blur, pixelation, arbitrary paths and text, all in one library |
| Screen capture | **DXGI Desktop Duplication** for snapshots, **Windows.Graphics.Capture** for recording and as the snapshot fallback | See §6: duplication hands back a desktop that never had a cursor in it, WGC has to switch the cursor off to match and that blinks |
| Global hotkey | `RegisterHotKey` (user32, P/Invoke) | Zero background cost: the message arrives in the message loop |
| Tray | `NotifyIcon` (WinForms interop) | Standard |
| Settings | `System.Text.Json` writing `%AppData%\reshot\settings.json` | Shared with the settings window |
| Settings window | **Tauri 2** (Rust + TypeScript), a separate process | Styling the Source-like dialog in HTML and CSS beats fighting WPF for it |
| Video | A bundled ffmpeg: BGRA frames piped in as rawvideo, H.264 encode (`h264_nvenc` / `h264_amf` / `h264_qsv` when available, `libx264` otherwise), AAC encode and final mux; WASAPI through NAudio for the audio capture | Bundled ffmpeg costs about 145 MB per artifact and brings a GPL licence obligation (§9) |
| Text recognition | `Windows.Media.Ocr` | Offline, no models to ship, already part of the OS |

### Key NuGet packages

```
SkiaSharp
SkiaSharp.Views.WPF
Vortice.Direct3D11        (capture)
NAudio                    (audio)
```

## 2. Solution layout

```
reshot/
├── src/
│   ├── Reshot.App/            WPF: entry point, tray, hotkeys, overlay, OCR, export
│   │   ├── Overlay/           session window, toolbar, tool panels
│   │   ├── Ocr/               recognition and the selectable text layer
│   │   ├── Radial/            hold-to-open quick menu
│   │   ├── Recording/         recording HUD, audio track prompt
│   │   └── Tray/
│   ├── Reshot.Core/           no UI dependencies: document model, tools, history
│   │   ├── Document/
│   │   ├── Tools/
│   │   ├── History/
│   │   └── Export/
│   ├── Reshot.Capture/        Windows.Graphics.Capture wrapper
│   ├── Reshot.Recording/      ffmpeg shell-out plus WASAPI
│   └── reshot-tauri/          settings window (Tauri 2)
├── build/                     release scripts and the installer
└── tests/Reshot.Core.Tests/
```

Rule: **Reshot.Core knows nothing about WPF.** Every tool and the document model are
testable without windows.

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

`Reshot.Core/Tools/FontCatalog.cs` is the single place that enumerates the installed
families and turns a family name into an `SKTypeface`. Everything that draws or measures
text goes through it — the text object, the caret the overlay draws while typing, the
picker in the tool panel — and nothing else calls `SKTypeface.FromFamilyName`, so a name
is resolved and cached once, and an unknown one falls back to Segoe UI in exactly one
place. The catalogue is built lazily and off the UI thread, because enumerating every
installed family is slow enough to be felt on the first click.

It lives in Core rather than in the WPF layer for the reason at the end of §2: Core owns
rendering and knows nothing about windows, and the same typeface has to serve both the
Skia canvas and the family name stored in the settings model.

The trap worth knowing about: the picker's list is rendered by **WPF**, which quietly
substitutes another font for glyphs the chosen family does not have, while the screenshot
is rendered by **Skia**, which does not substitute anything. A Latin-only family therefore
previews Cyrillic perfectly and then draws it as empty boxes on the image. That asymmetry
is why `FontEntry` carries `SupportsCyrillic` and why the picker dims those rows — the
preview cannot be trusted to show the failure by itself.

## 4. Erasers: the strength model

An eraser stroke is a stamp with a radial alpha gradient:

```
alpha(r) = 1.0                    at the centre, always 100%
alpha(r) = falloff(r, hardness)   towards the rim
```

- `hardness = 1.0` gives a step: the whole disc erases fully, a hard round eraser.
- `hardness = 0.0` fades from the centre to the rim, a soft patch.
- The centre erases fully at any setting.

Implementation: the stroke is rasterised into a greyscale **coverage bitmap** whose
luminance is the erase strength. Overlapping stamps along the stroke are combined with
`SKBlendMode.Lighten`, taking the maximum rather than accumulating, and the finished
coverage is applied to the layer once with `SKBlendMode.DstOut`. Stamping semi-transparent
discs directly would compound along the stroke and erase the middle of a soft line fully.

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
  for the first frame is the bulk of the hotkey-to-overlay time, and holding the dispatcher
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

- The background process is a message-only window plus `RegisterHotKey` plus the tray. No
  timers, no polling, no watchers.
- The WPF overlay is **not created at startup**. It is constructed on the first hotkey, so
  the first call is about 100 ms slower.
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
  because everything that makes a Reshot recording what it is lives on the C# side and
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
- The application is published self-contained, so no .NET runtime is required.
- A GPL `ffmpeg.exe` is bundled with the installer and the portable ZIP. It is what
  records, it adds roughly 145 MB, and it carries a licence obligation Reshot does not
  share: the binary is GPL even though Reshot stays MIT, so its licence and the source
  offer are shipped in `THIRD-PARTY-NOTICES.md`.
- There is no code signing (open source, no budget), so SmartScreen will complain at first.
  The README explains it.
- **Automatic updates are not implemented.** Velopack was planned and the `update.auto`
  setting exists in the UI, but nothing is wired to it yet.
- License: MIT.
