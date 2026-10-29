# Brief for Codex: phase R8 (recording UI, radial menu, recording orchestration)

Thanks for R2-R4: reviewed, committed and pushed (`193d4aa`). Same rules as before:
**do not commit**, Claude reviews and commits. Say "R8 done" with a summary at the end.

Read first: `rust/ROADMAP-RUST.md`, SPEC.md §10 and §13, and these C# sources:

- `src/Reshot.App/Radial/RadialMenuWindow.cs` → `rust/crates/reshot/src/radial.rs`
- `src/Reshot.App/Recording/RecordingHudWindow.cs` → `rust/crates/reshot/src/recording/hud.rs`
- the corner brackets (find them in `App.xaml.cs` / `RecordingHudWindow.cs`) → `recording/corners.rs`
- `src/Reshot.App/Recording/AudioTrackPrompt.cs` → `recording/track_prompt.rs`
- `App.xaml.cs`: `QuickRecordPrimaryMonitor`, `QuickAudioAllSources`, `StartRecording*`,
  `StopRecording`, `FinishRecording`, `RegisterAudioHotkey`/`OnAudioHotkey`,
  `StartAudioRecording*`, `StopAudioRecording`, `ShowOverGame`, `KeepWindowInFront`
  → `rust/crates/reshot/src/recording/mod.rs`

## Your files (only these)

- `rust/crates/reshot/src/radial.rs` (replace the interim file; keep `pub fn open(cx: &mut App, vk: u32)`)
- `rust/crates/reshot/src/recording/**` (new module)
- you may add the single line `mod recording;` to `rust/crates/reshot/src/main.rs`
- you may add deps to `rust/crates/reshot/Cargo.toml` (tell me which)

Claude is editing `app.rs`, `overlay/**`, `theme.rs`, `tray_menu.rs`, `winutil.rs` in parallel.
Do not edit them; if you need something there, write it in your final message and I wire it.

## Public API I will call from app.rs / overlay

```rust
pub fn is_active(cx: &App) -> bool;              // video or audio recording running
pub fn stop(cx: &mut App);                        // hotkey while recording = stop
/// Overlay's Record tool: selection rect in physical virtual-desktop pixels plus an optional
/// shape mask (row-major, 1 byte per pixel, 0 = outside) for non-rectangular selections.
pub fn start_video(cx: &mut App, rect: PxRect, mask: Option<Vec<u8>>);
pub fn start_audio(cx: &mut App, sources: reshot_recording::AudioSources);
pub fn quick_record_primary_monitor(cx: &mut App); // radial slice
pub fn quick_audio_all_sources(cx: &mut App);      // radial slice
pub fn toggle_audio_from_settings(cx: &mut App);   // audio hotkey
```

## How to fit the existing app (read `app.rs`, `winutil.rs`, `theme.rs`, `tray_menu.rs`)

- Settings: `cx.global::<crate::app::ReshotApp>().settings.current()`.
- Store your state in your own `gpui::Global` (e.g. `RecordingState`), not in `ReshotApp`.
- Windows: `winutil::popup_options(...)`; HWND via `winutil::hwnd(window)`.
  **Any Win32 call that can synchronously re-enter gpui (SetWindowPos, SetForegroundWindow,
  ShowWindow...) must go through `winutil::after_update(cx, ...)`**, never inside
  `update`/`defer`, or you get "RefCell already borrowed". See `tray_menu.rs`.
- Win32 helpers: `reshot_platform::win` (`place_topmost`, `force_foreground`,
  `exclude_from_capture`, `foreground_is_fullscreen_foreign_window`, `unlock_input_from_game`,
  `assert_topmost`, `is_key_down`, `cursor_pos`, `working_area`, `monitor_bounds`...).
- Styling: `crate::theme` (VGUI palette, `panel()`, `button()`, `check_row()`, `separator()`),
  must look the same as the C# windows. Icons: add SVGs under `crates/reshot/assets/icons/`
  and list them in `src/assets.rs` (`embedded![...]`) — that one-line list edit is allowed.
- REC HUD and corner brackets are excluded from capture (`win::exclude_from_capture`) so they
  never appear in the recording. Corner window is click-through (WS_EX_TRANSPARENT|LAYERED).
- Errors/notices: `ReshotApp` has a `shell` with `show_balloon` — if you need it, ask me to
  expose a `pub fn balloon(cx, title, text, icon)` in app.rs; I'll add it right away.
- gpui quits when its last window closes: the app keeps a hidden anchor window, so you may
  freely open/close your windows.
- Don't run UI tests on the live desktop by synthesising input: the user is working at this
  machine. `cargo build`, `cargo clippy -p reshot -- -D warnings`, unit tests only.

## Done means

Builds, clippy clean on the `reshot` crate, no `todo!()`, behaviour matches the C# code.
