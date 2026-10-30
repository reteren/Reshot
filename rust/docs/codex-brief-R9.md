# Brief for Codex: phase R9 (settings window in gpui, replaces Tauri)

Start after R8 is done. Same rules: **do not commit**, Claude reviews and commits; say
"R9 done" with a summary.

## Reference (read all of it)

- `src/reshot-tauri/index.html` — every tab, row, label, hint text (copy texts verbatim)
- `src/reshot-tauri/src/styles.css` — the VGUI look: colours, bevels, tab strip, sliders,
  checkboxes, selects, inputs, swatches, palette, kbd chips, the 0.60 panel tint, 8 px corners
- `src/reshot-tauri/src/main.ts` — behaviour: hotkey capture field (click, then press a
  combination; PrtScn supported), Clear, colour palette popup, folder browse buttons, selects
  (format png/jpg/webp, fps 60/30/25, microphone device list), live Info tab texts that follow
  the hotkeys and the radial mode, the inverted "Choose by releasing the key" checkbox
  (`radial.clickToChoose` negated), autostart prompt on close, settings path line
- `src/reshot-tauri/src-tauri/src/lib.rs` + `audio.rs` — window setup and the mic device list

## Your files

- `rust/crates/reshot/src/settings_window.rs` (replace the interim one; it may become a
  `settings_window/` module) — keep `pub fn open(cx: &mut App)`.
- Microphone device enumeration: use `reshot-recording` if it already exposes it, otherwise
  add it there (your crate).

## How it fits

- One settings window at a time; a second `open` activates the existing one.
- Read: `cx.global::<crate::app::ReshotApp>().settings.current()`.
- Write: add and call `pub fn apply_settings(cx: &mut App, new: AppSettings)` in `app.rs`
  — **tell me the exact signature and I add it** (it saves, re-registers hotkeys, applies
  autostart with `allow_prompt: true`, and pushes the change to live components). Do not edit
  `app.rs` yourself.
- Styling via `crate::theme` (extend it if needed: it is shared, so add, don't change existing
  items). Same bevel technique as the toolbar: nested elements, light top/left, dark
  bottom/right. Normal window with our own titlebar (draggable header + close icon as in the
  HTML); rounded 8 px corners; translucent panel like the HTML (`--bg: rgba(118,118,118,0.6)`).
- Text inputs: gpui has no input widget. `overlay/panel.rs` has a minimal focused-field
  pattern (hex input, font search) — reuse the idea or extract a small shared widget into
  `crate::widgets` (new file, yours).
- Folder pickers: `rfd` is already a dependency (see `export.rs::ask_save_path`).
- Win32 calls that re-enter gpui must go through `winutil::after_update`.
- No UI tests by synthesising input on the live desktop.

## Done means

Builds, `cargo clippy -p reshot -- -D warnings` clean, every setting from the HTML present and
persisted to the same JSON keys, looks like the Tauri window.
