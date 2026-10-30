## Install

| File | What it is |
|---|---|
| `reshot-<version>-setup.exe` | Installer. Installs per user, no administrator rights |
| `reshot-<version>-win-x64-portable.zip` | Portable build: unpack and run `reshot.exe` |

One executable and nothing to install alongside it: no runtime, no WebView2 window.

The application is not code signed, so SmartScreen may warn on first launch: "More info"
then "Run anyway". Checksums are in `SHA256SUMS.txt`.

## Requirements

- Windows 10 version 2004 (build 19041) or newer, x64
- Per-application audio capture requires Windows 11
- Russian text recognition needs a Windows OCR language pack
  (Settings, Language, Russian, Optional features, Optical character recognition)

## What's new

<!-- Copy this version's section from CHANGELOG.md here. -->

The full list of changes is in [CHANGELOG.md](../blob/main/CHANGELOG.md).
