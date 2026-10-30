# Third-party notices

Reshot's own source code is licensed under the [MIT License](LICENSE).

## FFmpeg

Reshot ships the unmodified `ffmpeg.exe` binary from BtbN's **FFmpeg
`N-125875-g5d4d3bdc61-20260731` win64-gpl** package (release
`autobuild-2026-07-31-14-10`). This build enables `--enable-gpl`, `--enable-version3`,
and `libx264`; it is licensed under the GNU General Public License, version 3 or later.

The corresponding FFmpeg source for this build is available from the
[FFmpeg source tree at commit `5d4d3bdc61412641883a45e060e810f80ea7f4b5`](https://github.com/FFmpeg/FFmpeg/commit/5d4d3bdc61412641883a45e060e810f80ea7f4b5).
The build scripts and package provenance are available from
[BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds) and its
[pinned release page](https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-07-31-14-10).
Accordingly, the corresponding source for the FFmpeg binary Reshot ships is available there.

## Roboto

The Rust build embeds Roboto 3.015 (Thin, Light, Regular and Bold), Copyright 2011 The
Roboto Project Authors ([googlefonts/roboto-classic](https://github.com/googlefonts/roboto-classic)),
licensed under the [SIL Open Font License, Version 1.1](https://openfontlicense.org). The
files are the @fontsource/roboto 5.3.0 subsets merged into one file per weight and renamed
"Reshot Roboto"; the glyphs are unchanged.
