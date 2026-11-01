# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this
project adheres to [Semantic Versioning](https://semver.org/).

## [1.3.4] - Unreleased (test build)

### Changed

- Faster: where Desktop Duplication keeps failing (a still pointer whose state it never
  reports), captures stop paying 40-150 ms for it before WGC takes the frame; it is retried
  every tenth capture. The overlay opens without one more copy of the frame, an export
  composites only the selected region instead of the whole desktop, overlay tiles convert
  through a lookup table, OCR fills the engine's buffer directly, and recording reuses its
  audio mix buffer and masks whole rows at a time. A recording at a new size probes the
  four GPU encoders side by side instead of one after another (about 300 ms sooner).
- Recording uses Media Foundation only through a GPU encoder. Without one it used to get
  Microsoft's software encoder: on 1080p60 its ffmpeg took 693 MB; libx264, used now in its
  place, takes 232 MB for about 0.2 of a core more.
- Recording converts frames to YUV (NV12) on the graphics card and pipes 1.5 bytes a pixel
  to ffmpeg instead of 4, so ffmpeg no longer converts colours. On the same 1080p60 content
  ffmpeg needs 20-30% less CPU and Reshot about half; if the card cannot convert, recording
  falls back to the previous path.
- Recordings are BT.709 and say so in the file. They used to carry BT.601 colours untagged,
  which players read as BT.709, so colours were slightly off.
- Settings: the elevated autostart checkbox reads "As administrator".

## [1.3.3] - Unreleased (test build)

### Fixed

- The outline and shadow around the recorded area now show with the corner brackets turned
  off too; they used to come only with the brackets.

## [1.3.2] - Unreleased (test build)

### Fixed

- The thin outline and soft shadow around the recorded area are back; 1.3.1 had removed them
  along with the radial menu's frame.

### Added

- A **Bitrate, kbps** slider in Settings → Video (2000–20000, 2000 by default): the lowest
  bitrate a recording gets. Larger areas still get more on their own; small ones, which
  always got 2000, can now be recorded sharper.

## [1.3.1] - Unreleased (test build)

### Changed

- The tool settings strip's tooltip names its key: "Tool settings (Shift)".
- Debug builds take `--preview-track-prompt` and `--preview-radial` to open the Audio Tracks
  window or the radial menu on their own.

### Fixed

- The Audio Tracks window after a recording uses the settings dialog's font, caption,
  close glyph, checkboxes and button, and its Save button is no longer cut off.
- The radial menu, the Audio Tracks window and the tray menu no longer show a faint square
  outline and shadow around them: Windows drew a normal window frame around the whole
  (transparent) window.
- The radial menu's icons are the ones from before the Rust rewrite again: a screen with a
  record dot, a microphone and a solid gear, taken from the C# build's geometry.

## [1.3.0] - Unreleased (test build)

### Changed

- Hardness is 100% by default, so every tool starts with a crisp edge.
- With several selections (`Ctrl`), one lying inside another (at most a tenth of it
  outside) cuts a hole in it, and one reaching further out extends the area. The dimming,
  the export, recording and drawing all follow the same region; before, a selection inside
  another only looked cut out while the export still took the whole outer one.

### Fixed

- Soft strokes no longer show diagonal seams where they cross themselves or turn sharply:
  the stroke follows a smooth curve through the mouse points, and where it crosses itself
  its soft edges add up like paint instead of keeping the larger of the two, as hive does.
- A click with the Text tool puts the middle of the line on the cursor; the text used to
  start just below it.

## [1.2.9] - Unreleased (test build)

### Added

- A **Hardness** slider for every tool but text, working as in hive: the brush is solid out
  to that share of its radius and fades smoothly to its edge. Brush, blur, pixelation and
  the three erasers get soft edges; shapes and lines a softened outline. 85% by default.

### Changed

- The erasers' **Opacity** is plain opacity now, how much a stroke removes; the soft edge it
  used to stand for is the new Hardness slider.

### Fixed

- Opacity works for shapes, lines and text; the slider was there but changed nothing.
- A brush stroke repaints only its newest piece on each move, like the erasers and effects,
  instead of everything under the stroke so far.

## [1.2.8] - Unreleased (test build)

### Changed

- In text recognition the outlines follow whole lines rather than single words, and a
  selection is one box per line from its first to its last selected word.

### Fixed

- Text recognition finds small text. A selection wider than 1200 px went to the OCR engine
  at its own size, where text 9 to 10 px tall (UI text at 100 % scaling) was mostly not
  detected at all: 3 to 38 % of the words. Every selection is now enlarged as far as the
  engine allows, three times up to 3333 px wide and twice up to 5000 px, which finds 95 to
  100 % of them; a full 4480x1440 desktop gets 74 to 97 %. A large selection takes about
  1 to 1.5 s to recognise instead of 0.5; the enlargement itself now takes tens of
  milliseconds rather than more than a second.

## [1.2.7] - Unreleased

### Fixed

- The erasers, blur and pixelation keep up with the mouse. Each move used to redo the whole
  stroke so far (up to 0.4 s a move on a long stroke over two monitors, which is what made
  the cursor jump); now it does only the new piece of it, in a few milliseconds.
- Blur and pixelation lie on the image without a thin dark outline. Every soft edge,
  brush strokes included, was drawn slightly too dark.

## [1.2.6] - Unreleased (test build)

### Fixed

- The toolbar icons sit inside their buttons again; they had dropped below the toolbar.
- The screen no longer shifts a few pixels right and down while a capture is open. The
  frozen frame, the radial menu and the recording HUD were all drawn 8 px right and 1 px
  down of where they belonged.

## [1.2.5] - Unreleased (test build)

### Changed

- Reshot is rewritten in Rust. It is the same program, feature for feature, now one
  executable: the settings window is part of it instead of a separate Tauri/WebView2
  process, and no .NET runtime ships beside it.
- Idle in the tray it holds about 21 MB and no UI framework at all; the interface starts on
  the first press.
- The settings window is drawn by the same UI as the overlay and is laid out to match the
  previous one line for line, in the same Roboto, with the acrylic backdrop and rounded frame.

### Removed

- The C# solution and the Tauri settings application. Settings carry over: `settings.json`
  keeps its keys.

## [1.1.4] - 2026-09-01

### Added

- The font dropdown has a search box pinned at the top of it, filtering the list as you type. With
  a few hundred families installed, scrolling was the only way to reach one.

### Fixed

- Typing an annotation after choosing a font goes into the annotation again. The closed font picker
  used to keep the keyboard to itself and read every letter as a jump-to-that-font, so the list
  flickered and the family changed on each keystroke; the keys that got past it landed on the tool
  shortcuts, which is what folded the settings panel away in the middle of a word. The picker can no
  longer take the keyboard at all — the search box does the searching now.
- The tool settings panel — colour, size, opacity, font — stays open while text is being typed, so
  the annotation can still be restyled without starting it over.

## [1.1.3] - 2026-09-01

### Added

- The Text tool draws in any font installed on the machine, picked from the same panel as
  the colour palette. Every family in the list is drawn in its own typeface, so the list
  is the preview: no choosing a name, typing a line and finding out afterwards.
- Families that cannot draw Cyrillic are dimmed in that list. Picking one is still
  allowed, but Russian text in it comes out as empty boxes — the screenshot is drawn by
  Skia, which renders what the font actually has and nothing else — so the list says so
  before you type rather than after.

### Changed

- The area a piece of text occupies is measured from the font's own metrics instead of
  being estimated at roughly 0.6 em per character. That area is what gets baked into the
  image and what undo restores, so text in a wide font is no longer at risk of being
  clipped by a box that assumed a narrower one.

## [1.1.2] - 2026-08-26

### Added

- Every slider takes the mouse wheel: size, opacity and hardness in the overlay, and all
  of them in the settings window. A notch is worth one unit of whatever the label beside
  the slider shows, a pixel or a percent, and in the overlay a high-resolution wheel moves
  the value proportionally rather than jumping a whole unit per event. Neither WPF's
  slider nor an HTML range input answers the wheel on its own, so a small correction used
  to mean putting the cursor on the thumb first.

### Fixed

- The build stamps its own version again. Everything after 1.1.0, the 1.1.1 release
  included, still reported `1.1.0` in the executable's file properties and in the version
  the settings window is built with, which made those artifacts indistinguishable from the
  release before them.

## [1.1.1] - 2026-08-23

### Added

- The installer offers to free the Print Screen key from the Windows snipping overlay,
  ticked by default. Windows 11 binds that key ahead of any hotkey an application
  registers, so on a default install Reshot's own default hotkey did nothing and the app
  looked broken on first run. The task clears
  `HKCU\Control Panel\Keyboard\PrintScreenKeyForSnippingEnabled`, which is the same switch
  as Settings → Accessibility → Keyboard, and uninstalling puts the Windows default back.
  It is a task rather than a silent write, because changing a Windows setting without
  saying so is not something an installer should do.

### Changed

- The capture overlay is no longer hidden from other applications. Screen sharing Reshot
  used to show the other side a live desktop while you selected and drew on a frozen one,
  because the overlay was excluded from every capture API for the whole session. The
  exclusion now lasts only from the instant a recording starts until the overlay closes,
  which is the one moment it was ever needed — so Reshot's own recordings still never
  contain the overlay, and everyone watching a call sees the selection and the annotations.
- The old behaviour is available as **Settings → Overlay → Hide the overlay from other
  applications** (`overlay.hideFromCapture`, off by default).

The recording HUD — the corner brackets and the REC indicator — stays excluded either way.
It is on screen for the whole recording rather than for a moment, so showing it to other
applications would also burn it into your own MP4.

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
