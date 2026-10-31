# Reshot roadmap

**Status as of 2026-10-30: phases 0 to 6 are done, 1.2.4 was the last C# release, and 1.2.5
is the same product rewritten in Rust ([docs/rust-rewrite.md](docs/rust-rewrite.md)).**

The project was built in phases, each one ending with a program you could actually use.
This file is the record of that, plus what is left.

## Shipped

| Phase | Scope | Where it lives |
|---|---|---|
| 0 | Skeleton: tray, global hotkey, single instance, settings, logging | `reshot-platform`, `reshot-core` |
| 1 | Capture, overlay, rectangular selection, copy and save | `reshot-capture`, `reshot/src/overlay` |
| 2 | Selection shapes and the tool toolbar with flyouts | `reshot-core/src/selection.rs`, `overlay/toolbar.rs` |
| 3 | Drawing: brush, shapes, text, eyedropper, undo | `reshot-core/src/{tools,history,editor}.rs` |
| 4 | Erasers and effects | `reshot-core/src/document.rs` |
| 6 | Video recording, and audio recording beyond the original plan | `reshot-recording` |
| — | Rewrite in Rust: one executable, every window on gpui | the whole workspace, 1.2.5 |

### Built beyond the original plan

- **Text recognition (OCR)** with a selectable text layer over the frozen frame, and an
  AUTO mode that merges the Russian and English engines line by line
- **Radial quick menu** on holding the hotkey
- **Separate audio tracks** recorded independently, so the tracks are chosen after the
  recording stops rather than mixed live
- **Per-application audio** through Windows process loopback
- **Hardness** on every tool but text, with the soft-edge model from hive
- **Half-Life 2 / Source styling** across the overlay, the tray menu and the settings
- **Settings window rebuilt on Tauri** instead of WPF, then on gpui with the rest in 1.2.5
- **Per-tool settings**: every tool keeps its own size, opacity and colour
- **Installer and portable build**, produced by one script

## Not done

- **Automatic updates.** Planned in ARCHITECTURE §10; the `update.auto` key exists in the
  settings and in the UI, but nothing is wired to it. Turning it on
  changes no behaviour.
- **Code signing.** Without a certificate SmartScreen warns on first launch. This needs
  money rather than code.
- **Tiled undo.** History snapshots the whole affected region instead of 256x256 tiles,
  which costs more memory on very large strokes.

## Ideas parked for later

These are deliberately not commitments, just the shortlist that keeps coming up:

- Scrolling capture for long pages
- Pinning a capture on top of other windows as a floating panel
- A live mirror of a screen region, reusing the recording stream
- Multitrack audio, one track per application, kept separate in the final file
- A rewind buffer so the last N seconds can be saved after the fact

Anything here would need to survive the project's founding constraint first: **zero cost
while idle**. A rewind buffer, for instance, directly contradicts it.
