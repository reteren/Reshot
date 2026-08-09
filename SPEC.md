# Reshot functional specification

Version 1.0 · 2026-07-28

---

## 1. Overview

Reshot is a resident screenshot utility. It lives in the tray, wakes on a global hotkey
(`PrtScn` / Print Screen by default), freezes the screen, lets you select an area of any shape, edit it
(drawing, text, shapes, blur, pixelation, erasers, text recognition) and copy or save the
result. It also records the selected area to video with sound.

## 2. Non-functional requirements

| Metric | Value |
|---|---|
| RAM when idle (tray only) | under 30 MB |
| CPU when idle | about 0%, no timers and no polling, just a hotkey |
| Hotkey to overlay | under 150 ms |
| Editor responsiveness | 60 FPS while selecting and drawing on a 4K capture |
| OS | Windows 10 2004 (build 19041) or newer, x64 only |
| Monitors with different DPI | not supported, no scale correction |

## 3. Session lifecycle

1. The program sleeps in the tray.
2. The user taps the hotkey, which **captures every monitor instantly** into one frozen
   frame of the whole virtual desktop.
3. A fullscreen overlay appears: the frozen frame, dimmed by a fill (black at 50% by
   default). The cursor is **not** part of the frame.
4. The user selects an area, which is shown undimmed at full brightness.
5. A toolbar appears near the selection. The session does **not** end after a selection;
   you can reselect, edit and combine tools freely.
6. Leaving the session: Copy, Save, Save As, or Esc to cancel with no result.

Session rules:

- Every editing tool (drawing, blur, erasers and so on) works **only inside a selection**.
  With nothing selected there is nothing to edit.
- A new selection with the same tool replaces the old one, and the old one is removed along
  with its edits.
- **Multiple selections:** holding `Ctrl` creates several active selections at once.
- `Ctrl+A` selects the primary monitor. Pressing it again in the same session extends the
  selection to **all monitors**.

## 4. Selection

### 4.1 Common mechanics

- Any selection shape (ellipse, lasso, polygon, triangle) is wrapped in a **rectangular
  bounding box** with handles once created, like a free-form object selection in Photoshop.
- Everything inside the box but outside the shape itself is **transparent**. The clipboard
  receives a PNG with alpha.
- The frame has 8 handles (4 corners, 4 edge midpoints) for **resizing**. Grabbing the
  frame itself **moves** the area.
- `Shift` while dragging:
  - held **before** the drag starts, it locks a 1:1 ratio (square or perfect circle);
  - pressed **during** the drag, it locks the current aspect ratio and keeps it.
- `Esc` with an active selection clears it. A second `Esc` closes the session.

### 4.2 Selection shapes (tool 1, right-click the toolbar tab for the flyout)

| ID | Shape | Behaviour |
|---|---|---|
| 1 | Rectangle | The default, active as soon as the hotkey fires |
| 1.1 | Ellipse | Circular or elliptical selection. `Shift` gives a perfect circle |
| 1.2 | Lasso | Freehand curve. Releasing the button closes the outline with a straight line back to the start |
| 1.3 | Polygon | Clicks place points joined by straight lines. Closing: click the first point, or right-click to close by the shortest line |
| 1.4 | Triangle | Triangular selection, dragged like a rectangle with the triangle inscribed |

## 5. Toolbar

- **Default position: outside the selection, bottom right.** If the selection fills the
  screen and there is no room outside, the toolbar moves **inside** the area instead.
- **Right-clicking a tab** opens a vertical flyout of sub-tools, like Photoshop. Left-click
  activates the current sub-tool. Hovering a tab for 0.5 s opens the flyout as well.
- Each category remembers the sub-tool you used last.
- Tools are switched with the digits **`1` to `0`**, following the toolbar left to right.

| Key | Tool | Sub-tools |
|---|---|---|
| `1` | Selection | Rectangle, Ellipse, Lasso, Polygon, Triangle |
| `2` | Brush | |
| `3` | Shapes | Square, Circle, Triangle |
| `4` | Lines | Line, Arrow |
| `5` | Erasers | Eraser, Absolute Eraser, Filter Eraser |
| `6` | Effects | Blur, Pixelate |
| `7` | Text recognition (OCR) | |
| `8` | Text | |
| `9` | Record video | |
| `0` | Record audio | |

Every tool keeps its **own** size, opacity and colour, so switching tools does not carry
the previous tool's settings over.

## 6. Drawing

### 6.1 Brush

- A full colour picker with hex entry.
- Adjustable **opacity** and **thickness**, up to 200 px.
- Brush strokes are **permanent raster**: once painted they cannot be selected and moved,
  only erased or undone.

### 6.2 Eyedropper

- **Holding the right mouse button** on the canvas picks a colour. While held, a small
  preview square with the hex code follows the cursor. Releasing takes the colour.
- A button in the settings panel arms the eyedropper so it **stays active** until the next
  right-click, for when holding a button is awkward.
- Right-clicking the toolbar does not trigger the eyedropper; the toolbar has priority.

### 6.3 Shapes and lines

- Shapes: square, circle, triangle. Lines: straight line and arrow.
- Shapes and text are **baked into the paint layer** when committed, so the eraser removes
  them pixel by pixel like any other ink. They cannot be moved after the fact.

### 6.4 Text

- Settings: font size, colour, opacity.
- Typing inserts characters, `Enter` starts a new line, `Backspace` deletes.
- `Esc`, clicking away, or switching tools commits the text and keeps it.

### 6.5 Tool settings

- `Shift` while drawing opens the tool settings panel. The thin dotted strip above the
  toolbar does the same on click.
- The panel adapts to the tool: erasers show size and the Photoshop-style opacity, effects
  show size and strength with no colour, ink tools show size, opacity and the picker.

## 7. Erasers

All three share a size setting and the Photoshop-style **opacity**, which is a radial
falloff rather than plain transparency:

- The eraser area is a disc of the given size.
- **The centre always erases at 100%**, whatever the setting.
- At 100% the whole disc erases everything it touches, a hard edge.
- At 30% the rim erases at 30% and the strength rises smoothly towards the centre, a soft
  gradient.

| ID | Eraser | What it removes |
|---|---|---|
| 5 | Eraser | Only what the user drew: brush, shapes, text |
| 5.1 | Absolute Eraser | **Everything**, including the captured frame, down to transparency |
| 5.2 | Filter Eraser | Only the filters (blur, pixelation), restoring the original pixels |

## 8. Effects

Both effects share a size and strength setting, and have two application modes:

- **By default** you brush them on.
- **Holding `Ctrl`** you drag a rectangle and the effect fills it.

| ID | Effect | Settings |
|---|---|---|
| 6 | Blur | Blur strength, brush size |
| 6.1 | Pixelate | Pixel size (strength), brush size |

Effects live on their own layer and are removed by the Filter Eraser without touching the
original.

## 9. Text recognition (OCR)

- Select an area and press the OCR tool. The text inside it is recognised by the built-in
  Windows engine: offline, with no models to ship and nothing sent over the network.
- Recognised words become **selectable** over the frozen frame. Dragging across them
  selects a span and copies it on release. `Ctrl+C` copies the selection, `Ctrl+A` selects
  and copies everything, `Esc` leaves the mode.
- Each word is outlined so it is clear what can be selected.
- Windows OCR is **one language per engine**, and a mismatched engine substitutes
  look-alike glyphs (a Russian engine turns the Latin "un" into the Cyrillic "ип"). The
  default **AUTO** mode therefore runs both the Russian and English engines and merges them
  line by line: the Russian pass reliably reports a line's real script, so Latin-dominant
  lines are taken from the English pass and Cyrillic or mixed lines from the Russian one.
- Right-clicking the tool cycles AUTO, RU, EN for the cases where the automatic choice is
  wrong.

## 10. Recording

- Select an area with any selection tool, press Record, and the recording starts. The
  screen is **not** frozen while recording; the live image is captured.
- The whole editing UI disappears during recording.
- A small indicator window sits in a corner: a red light, a timer, and the resolution.
- The bounds of the recorded area are shown with **corner brackets**. The settings can turn
  them off or change their colour and opacity.
- **Recording stops on the main hotkey.**

Technical parameters:

- Container and codec: **MP4 / H.264 with hardware encoding** (NVENC, AMF, QSV).
- FPS: a setting, 60 by default, with 30 and 25 available.
- **Audio sources**: the microphone, the full system mix, or individual applications
  through Windows process loopback (Windows 11). Right-clicking the record tool opens the
  source picker.
- Sources are recorded **separately**, so which tracks end up in the file is decided after
  the recording stops. A small panel appears at the bottom right offering system audio and
  the microphone; both, one or neither can be kept. The chosen tracks are mixed into a
  single track and muxed with the video, which is copied through without re-encoding.
- A "never show this again" checkbox in that panel is the same setting as "Ask which
  tracks to keep when saving" in the settings window.
- The microphone device is chosen in the settings.

A standalone audio recorder (tool `0`) records sound alone to M4A, with the same source
picker.

## 11. Output actions

| ID | Action | Behaviour |
|---|---|---|
| Copy | Copy | The result, cropped to the selection with transparency and all edits, goes to the clipboard as PNG |
| Save | Save | Saves to the folder from the settings. Screenshots to `Pictures\reshot`, video to `Videos\reshot` by default |
| Save As | Save As | A dialog for the folder and name |

The file name follows a template, `Reshot_YYYY-MM-DD_HH-mm-ss.png` by default.

## 12. Undo and redo

- `Ctrl+Z` undoes. `Ctrl+Shift+Z` and `Ctrl+Y` both redo.
- History depth: **32 steps**.
- Covers strokes, shapes, text, effects, erasing, and selection changes.

## 13. Radial menu

Holding the main hotkey instead of tapping it opens a radial menu at the cursor with three
slices and a cancel hub in the middle:

| Slice | Action |
|---|---|
| Record screen | Starts recording the primary monitor immediately with the saved settings |
| Record audio | Starts the audio recorder with every source |
| Settings | Opens the settings window |

By default the wheel **outlives the keypress** (`radial.clickToChoose`): let the hotkey go
and it stays up, waiting for a left click on a slice. The cursor starts in the central hub,
which means cancel, and so does everything past the outer rim — a click out on the desktop
is "go away", not "run the nearest slice".

Turning `radial.clickToChoose` off makes it a **gesture instead of a dialog**: the wheel
lives only while the key is held, and releasing it runs whatever the cursor points at.
There is then nothing to click and no outer edge to stay inside, a flick past the rim still
counts as pointing that way. Releasing without moving does nothing, because the cursor
starts in the hub. This is the faster way round once known, which is exactly why it is not
the default: a menu that disappears when you let go never gets the chance to teach itself.

Cancelling, in either mode: the centre hub, `Esc`, or a right-click.

The wheel is placed at the cursor but kept inside the work area, so it never runs off the
edge of the screen and never covers the taskbar.

## 14. Modifier map

| Input | Context | Action |
|---|---|---|
| Hotkey, tap | global | Start a capture session |
| Hotkey, hold | global | Open the radial menu |
| Left button | radial menu | Run the slice clicked; the hub and everything past the rim cancel |
| Hotkey, release | radial menu, `radial.clickToChoose` off | Run the slice the cursor points at; the hub cancels |
| Hotkey | while recording | Stop the recording |
| `Ctrl+A` | session | Select the primary monitor, again for all monitors |
| `Ctrl` + drag | selection | Add another selection |
| `Ctrl` + drag | effects | Rectangular area instead of brushing |
| `Shift` | selection | Lock the ratio, 1:1 or the current one |
| `Shift` | drawing, erasers, effects | Open the tool settings panel |
| Right button, held | drawing, on the canvas | Eyedropper with a colour preview |
| Right button | polygon | Close the outline by the shortest line |
| Right button | on a toolbar tab | Sub-tool flyout |
| Right button | on record video or record audio | Audio source picker |
| Right button | on the OCR tab | Switch AUTO, RU, EN |
| `1` to `0` | session | Switch tools, following the toolbar |
| `Ctrl+Z`, `Ctrl+Shift+Z`, `Ctrl+Y` | session | Undo and redo |
| `Enter`, `Ctrl+C` | session | Copy and close |
| `Esc` | session | Back out one layer: armed eyedropper, settings panel, unfinished polygon, then the session |

## 15. Settings

Stored as JSON in `%AppData%\reshot\settings.json`.

| Key | Default | Description |
|---|---|---|
| `hotkey` | `PrtScn` | The main hotkey, rebindable |
| `audioHotkey` | empty | Optional second hotkey for the audio recorder |
| `dim.opacity` | `0.5` | Dimming strength outside the selection |
| `dim.color` | `#000000` | Dimming fill colour |
| `paths.screenshots` | `Pictures\reshot` | Screenshot folder |
| `paths.videos` | `Videos\reshot` | Video folder |
| `paths.records` | `Music\reshot` | Audio recording folder |
| `autostart` | `true` | Start with Windows |
| `autostartElevated` | `false` | Start with Windows **as administrator**, through a scheduled task. Only then can the overlay appear over an elevated application |
| `format.image` | `png` | Default format: png, jpg or webp |
| `quality` | `90` | JPG and WebP quality |
| `filename.template` | `Reshot_{date}_{time}` | File name template |
| `video.fps` | `60` | 60, 30 or 25 |
| `video.audio.mic` | `true` | Record the microphone track |
| `video.audio.system` | `true` | Record system sound |
| `video.audio.askOnSave` | `true` | Ask which tracks to keep after recording |
| `audio.micDevice` | `default` | Microphone device |
| `video.corners.enabled` | `true` | Corner brackets around the recorded area |
| `video.corners.color` | `#3C9898` | Bracket colour |
| `video.corners.opacity` | `0.7` | Bracket opacity |
| `update.auto` | `true` | Automatic updates. **Not implemented yet** |
| `radial.clickToChoose` | `true` | Keep the radial menu open after the hotkey is released and pick with a left click. Off = the menu is a gesture that ends with the keypress. The settings window offers this inverted, as "Choose by releasing the key" |

The interface is English only, with a fixed dark theme.

## 16. System integration

- Tray icon with a menu: Capture, Settings, Pause hotkey, Quit.
- Single instance: a second launch signals the first and exits.
- Autostart through the `HKCU\...\Run` registry key.
- **Autostart as administrator** replaces that key with a scheduled task named `Reshot`
  (logon trigger, highest privileges). This is the only way Windows starts a program
  elevated at logon without prompting every time: permission is given once, when the task
  is created. The two are mutually exclusive, or Reshot would be started twice. Declining
  the prompt falls back to the plain lane and the setting is corrected to match.

## 17. Deliberately out of scope

- macOS and Linux
- Different DPI per monitor
- Scrolling screenshots
- Highlighting the window under the cursor
- Pinning a screenshot on top of other windows
- Capturing the cursor
- A history or gallery inside the program
- Cloud upload, imgur
- Localisation, the UI is English only
- GIF and WebM

## 18. Known deviations from this specification

- The vector layer was dropped. Shapes and text are baked into the paint layer on commit,
  so the eraser can remove parts of them. In exchange they cannot be moved afterwards.
- Undo stores whole-region snapshots rather than 256x256 tiles, which costs more memory on
  large strokes.
- Automatic updates are not implemented.
- MP4 has no alpha channel, so the area outside a non-rectangular recording is black rather
  than transparent.
