<p align="center">
  <img src="reshot.png" width="128" alt="Reshot">
</p>

# Reshot

screenshot and screen recording tool for windows that sits in the tray and does nothing until you press the hotkey

press PrtScn and every monitor freezes in one frame. then you select any area, draw on it and send it to the clipboard or a file. hold the key instead and you get a radial menu with quick screen recording, an audio recorder and settings

the whole interface is styled like half-life 2 / source vgui

what it can do:

selection: rectangle, ellipse, triangle, lasso, polygon, several at once with ctrl, a selection inside another one cuts a hole in it

drawing: brush, shapes, lines and arrows, text in any installed font, blur and pixelation, three erasers, eyedropper and a full colour picker, 32 steps of undo. drag text to move it; double-click it with the text tool to edit it

text: recognised words can be selected right over the frozen screen and copied. it works offline through the windows engine, and auto mode reads russian and english together

recording: mp4 of any shape, system sound and mic go to separate tracks so you choose what to keep after you stop, sound from a single app, and a separate audio only recorder

everything outside a non rectangular selection stays transparent, in the clipboard too

## a few notes from the build side

- in the tray it takes about 21 mb and 0% cpu. the ui isn't even loaded until the first press
- screenshots go through desktop duplication, so the cursor doesn't blink when the screen freezes. windows.graphics.capture takes over where duplication fails, like exclusive fullscreen games
- up to 1.2.4 it was c# and wpf, and the settings were a separate webview window. now it's all rust on gpui, one exe, no .net and no webview. old settings carry over
- recording converts frames on the gpu and encodes with nvenc, amf or qsv if the card has them, libx264 if not
- the overlay and the rec indicator are hidden from capture, so they never end up in your own video
- the installer is around 48 mb because ffmpeg is inside. ffmpeg is gpl, reshot is mit, details in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)
- it can't come up over a program running as admin, windows doesn't let a normal process do that. for that there's a "start with windows as administrator" checkbox in settings, available after installing for all users in protected Program Files
- monitors with different dpi scaling aren't supported yet
- sound from a single app needs windows 11, russian text recognition needs the russian ocr pack in windows language settings

## install

grab `reshot-x.y.z-setup.exe` or the portable zip from [releases](https://github.com/reteren/reshot/releases). the installer offers a per-user installation without admin rights, or an all-users installation in Program Files for administrator mode. portable copies run without administrator rights. the build isn't signed so smartscreen will complain on first run, click more info and run anyway

recording accepts only the bundled ffmpeg next to reshot.exe and checks its built-in SHA-256 before running it. if that file is missing or changed, recording is blocked; reinstall or restore the matching bundled file.

## license

MIT, see [LICENSE](LICENSE)

## building

needs [rust](https://rustup.rs) 1.90+ with the msvc toolchain, on windows 10 (2004+) or 11

```powershell
cargo run -p reshot                 # run it, it goes to the tray
pwsh build/fetch-ffmpeg.ps1         # ffmpeg, needed for recording
pwsh build/build-release.ps1        # installer and portable zip in dist/
```

more in [CONTRIBUTING.md](CONTRIBUTING.md) and [ARCHITECTURE.md](ARCHITECTURE.md)
