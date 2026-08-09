# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this
project adheres to [Semantic Versioning](https://semver.org/).

## [1.1.0] - 2026-08-09

### Added

- The selection shows its exported size in pixels above its top-left corner, and keeps
  showing it while the selection is being dragged or resized. Against the top edge of the
  screen, where there is no room outside, it sits just inside the frame instead.
- The radial quick menu now stays open after the hotkey is released and commits on a left
  click. The wheel gains an outer edge the gesture deliberately lacked — beyond the rim
  nothing is pointed at, so a click elsewhere on the screen cancels rather than running the
  nearest slice.
- "Choose by releasing the key" (Settings → General, `radial.clickToChoose` off) brings the
  old behaviour back: the wheel lives only while the hotkey is held and commits whatever the
  cursor points at on release, with no outer edge, so a flick past the rim still counts.

## [1.0.2] - 2026-08-02

### Changed

- Recording no longer uses Media Foundation. BGRA frames are piped into a bundled ffmpeg
  as rawvideo, and ffmpeg does the H.264 encode — hardware `h264_nvenc` / `h264_amf` /
  `h264_qsv` when available, `libx264` otherwise — the AAC encode and the final mux.
- ffmpeg is now bundled in the installer and the portable ZIP (a GPL build), which adds
  about 145 MB to every release artifact. Reshot stays MIT; the binary's licence and the
  source offer are in `THIRD-PARTY-NOTICES.md`.
- The audio-track prompt shown after a recording is dressed from the same Half-Life 2
  vocabulary as the settings window instead of the default WPF one, so its checkboxes and
  its Save button stop looking pasted on. The shared styles now live in one place rather
  than being restated per window.
- The hardware encoder is chosen by a real trial encode at the dimensions of the recording
  about to start, not by what the binary was compiled with. A GPL ffmpeg lists every
  backend regardless of the hardware present, so asking it would have picked NVENC on an
  AMD machine and produced no file at all. It also settles the case NVENC cannot serve: a
  selection below its minimum frame size now falls back to `libx264` instead of failing.

### Fixed

- The recording HUD — the corner brackets and the REC indicator — is excluded from the
  captured frame through `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`, so it no
  longer appears in the recorded video.
- The capture overlay no longer appears in the first frames of a recording. The monitor
  stream opens before the overlay closes, so the frozen frame, the selection outline and
  the toolbar were being composited into the start of every recording; the overlay is now
  excluded from capture the same way.

## [1.0.1] - 2026-07-31

### Changed

- Screenshots are taken with **DXGI Desktop Duplication** instead of Windows.Graphics
  Capture, which removes the cursor blink at the root. The frame has to have no cursor in
  it (SPEC §3); WGC can only manage that by asking the compositor to leave the cursor out,
  and that request pushes the cursor off its hardware plane for the life of the session,
  which is what blinked. A duplicated desktop never contains the cursor in the first place.
  WGC still does all recording, and still takes over any snapshot duplication refuses: a
  display driven by a second adapter, a rotated or HDR output, an exclusive-fullscreen game.

- The radial menu is a gesture rather than a dialog: point at a slice and release the
  hotkey to run it, release over the central hub to cancel. Selection follows the raw
  cursor direction, so a flick past the rim still selects, and clicking is no longer
  needed (it still works).
- The radial menu is now a window the size of the wheel, placed inside the work area. It
  used to span the whole virtual desktop, which made the shell treat it as a fullscreen
  app and hide the taskbar.
- The wait for a monitor's first capture frame is 1.2 s instead of 3 s, so a fullscreen
  game that refuses monitor capture reaches the window fallback quickly instead of
  stalling first.

### Fixed

- The capture runs off the UI thread, so a slow one no longer freezes the whole app while
  the overlay is on its way.
- Holding the hotkey no longer starts a screen capture. Opening a capture session takes the
  cursor off its hardware plane — one visible blink per press — and a hold, which never
  needs a frame, was paying it along with the GPU cost of capturing a running game only to
  throw the result away.
- The synthetic Alt keypress used to defeat the foreground lock is gone. It was injected
  globally, so the whole desktop saw it and games received it as input; sharing the input
  queue is what makes the foreground call legal anyway.
- Every hotkey press is logged before the guards that can swallow it, along with the
  foreground window and the state of each guard. A press that went nowhere used to leave
  no trace at all, which made "nothing happens" impossible to tell apart from "the hotkey
  never arrived". A guard flag left set by a window that vanished without notice is also
  cleared on the next press instead of killing the hotkey for the rest of the session.

### Added

- **Start with Windows as administrator**, a second checkbox under autostart. Reshot cannot
  put its overlay over an application that runs elevated unless it is elevated itself, and
  the plain `Run` key cannot do that without a UAC prompt at every logon. The setting
  registers a scheduled task named `Reshot` (logon trigger, highest privileges) instead, so
  the permission is granted once, when the box is ticked. Declining that prompt falls back
  to the ordinary autostart and unticks the box rather than leaving it claiming otherwise.
  Windows is only asked when Reshot does not already hold the rights, so once the setting
  is in use — and Reshot is therefore starting elevated — changing it again costs no prompt.

### Documented

- Reshot cannot bring its overlay over an application running as administrator: Windows
  blocks a normal process from taking the foreground from an elevated one, and the
  mechanism meant for this (`uiAccess`) needs a signed binary. README and ARCHITECTURE §6.
- The tray menu no longer has black wedges in its rounded corners: the DWM blur-behind it
  asked for was painting the corner pixels black on a layered window.
- The cursor no longer blinks and changes shape in the moment before the overlay or the
  radial menu appears. Opening either ran the input unlock nine times over, and every run
  left the cursor's reference-counted display counter one higher than it found it, on top
  of attaching and detaching the input queue each time. Nothing that touches global input
  state runs at all now unless a fullscreen foreign window was in front when we opened —
  on the desktop there is nothing to fight, so the window is simply shown.
- Against a game, foreground is stolen a bounded number of times (~0.6 s) and then let go,
  while topmost keeps being re-asserted for the rest of the session: a game that re-asserts
  its own topmost every frame used to bury the overlay a moment after it appeared.
  Re-asserting topmost costs no input state, so it cannot flicker. The input-queue attach —
  the one moment a game's mouse capture can be dropped — now releases it, and the window
  that kept the foreground is named in the log.
- A slow capture no longer swallows the hotkey. It used to be ignored until the frame came
  back, which in a game meant a second or more of presses doing nothing at all; a newer
  press now supersedes the frame in flight, and pressing again with the overlay already
  open pulls it back in front instead of being dropped.

## [1.0.0] - 2026-07-28

First public release.

### Capture

- Instant snapshot of every monitor in a single frame (Windows.Graphics.Capture)
- Shaped selections: rectangle, ellipse, triangle, lasso, polygon
- Multiple selections with `Ctrl` + drag, exported as their union
- `Ctrl+A` selects the primary monitor, pressing it again selects all of them
- Transparency outside a non-rectangular shape, clipboard included
- Export to PNG, JPG and WebP; copy, save and save as

### Annotation

- Brush, shapes (square, circle, triangle), lines and arrows, text
- Blur and pixelation, applied by brushing or as a rectangle with `Ctrl`
- Erasers: normal, absolute (down to transparency), and one for filters
- Soft erasing with a radial falloff: the centre always erases fully and the rim follows a
  setting, the way a Photoshop brush behaves
- Eyedropper, either by holding the right mouse button or from a panel button that stays
  armed until you click
- Full HSV picker, hex entry, 16 swatches
- Undo and redo, 32 steps deep
- Per-tool size, opacity and colour

### Text from an image

- Recognition through the built-in Windows engine: offline, no models to ship
- Recognised words are selected with the mouse straight over the frame, then copied
- AUTO mode merges the Russian and English engines line by line, which removes the Latin
  characters being swapped for Cyrillic look-alikes on mixed text
- Manual AUTO / RU / EN switching by right-clicking the tool

### Recording

- MP4 / H.264 with hardware encoding (NVENC, AMF, QSV), no ffmpeg
- Records a selection of any shape, not just a rectangle
- System audio and the microphone are captured separately, so the tracks are chosen after
  the recording stops; the choice can be made permanent with a "never ask again" checkbox
- Per-application sound through Windows Process Loopback
- Standalone audio recorder writing M4A
- Recording indicator with a timer and corner brackets

### Shell

- Radial menu on holding the hotkey: quick screen recording, quick audio recorder, settings
- Settings window built with Tauri 2, including a full keyboard shortcut reference
- Half-Life 2 / Source VGUI styling
- Tray icon, autostart, single instance, global hotkeys
- Tools switched with the digits `1` to `0`, in toolbar order

### Known limitations

- Monitors with different DPI scaling are not supported
- Per-application audio capture requires Windows 11
- Russian text recognition needs a Windows OCR language pack
- Automatic updates are not implemented: `update.auto` currently does nothing
- MP4 has no alpha channel, so the area outside a non-rectangular recording is black
  rather than transparent

[1.0.1]: https://github.com/reteren/reshot/releases/tag/v1.0.1
[1.0.0]: https://github.com/reteren/reshot/releases/tag/v1.0.0
