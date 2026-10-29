# Brief for Codex: phases R2, R3, R4 (capture, recording, OCR)

Read first: `rust/ROADMAP-RUST.md`, `ARCHITECTURE.md` §6 and §9, `SPEC.md` §9-10.

## Your scope — only these directories

- `rust/crates/reshot-capture`  ← port of `src/Reshot.Capture/*`
- `rust/crates/reshot-recording` ← port of `src/Reshot.Recording/*` + `tests/Reshot.Recording.Tests/*`
- `rust/crates/reshot-ocr`      ← port of `src/Reshot.App/Ocr/OcrService.cs` (recognition only;
  the selectable text layer `OcrTextLayer.cs` is UI and stays with Claude)

Do not edit any other path. Claude works in `reshot-core`, `reshot-platform`, `reshot` at the
same time in the same checkout. If you need something from the root `rust/Cargo.toml`, use
`{ workspace = true }` deps already there (`windows`, `windows-core`, `anyhow`, `thiserror`,
`parking_lot`, `crossbeam-channel`, `log`); add crate features in your own crate's
Cargo.toml. If you need a new workspace dep, tell Claude instead of editing the root file.

**Do not commit or push.** Claude reviews and commits. Say "R2 done" / "R3 done" / "R4 done"
in your final message with a short summary of the public API.

## Contracts

- Pin `windows = 0.61` (that is what gpui uses — no second copy).
- Frames are BGRA8, top-down, tightly described by `width`, `height`, `stride`, plus the
  virtual-desktop origin and the list of monitors (bounds, primary flag, name). Mirror
  `CapturedFrame.cs` as a plain struct with `Vec<u8>` pixels — no D3D types in the public API.
- `reshot-capture` public API mirrors `IScreenCaptureService`: capture all monitors into one
  frame (DD first, WGC fallback, remembered structural refusals, 1.2 s first-frame timeout,
  `AccumulatedFrames > 0` handshake loop), and a monitor stream for recording that yields
  frames via a callback/channel, no yellow border, cursor policy as in C#.
- `reshot-recording` public API mirrors `VideoRecorder`, `AudioRecorder`, `AudioSources`,
  `Mp4Muxer`, `M4aAudioEncoder`; ffmpeg path discovery as in `Ffmpeg.cs`. Port every test in
  `tests/Reshot.Recording.Tests` to Rust unit/integration tests (args builders, safety,
  timers, shutdown races). WASAPI via the `windows` crate (or the `wasapi` crate if it uses
  windows 0.61 — check `cargo tree -d`).
- `reshot-ocr`: `recognize(bgra, w, h, stride, OcrLanguage::{Auto,Ru,En}) -> Vec<OcrLine{words: Vec<OcrWord{text, rect}>}>`
  with the AUTO line-by-line merge exactly as in C#.
- Behaviour must match the C# code; comments in the C# explain hard-won quirks — keep them
  as Rust comments where they explain *why*.

## Done means

`cargo build --workspace`, `cargo clippy -p <crate> -- -D warnings` and `cargo test -p <crate>`
pass; no `todo!()`/`unimplemented!()`; a small `examples/` binary per crate that proves it on
real hardware (capture → PNG; record 3 s → MP4; OCR a PNG).
