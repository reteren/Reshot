<#
.SYNOPSIS
    Builds every release artifact for Reshot: the app, the settings window, a portable
    ZIP and (when Inno Setup is present) the installer.

.DESCRIPTION
    reshot ships as two executables that must sit side by side:

      reshot.exe        the WPF app (tray, overlay, capture, recording)
      reshot-tauri.exe  the settings window, launched as a child process

    App.ResolveSettingsExe looks for reshot-tauri.exe next to reshot.exe first, which is
    exactly the layout this script produces.

    The app is published self-contained, so users do not need the .NET 8 runtime.

.PARAMETER Version
    Version stamped into the artifact names. Defaults to the <Version> in
    Directory.Build.props, so bumping it there is enough.

.PARAMETER SkipSettings
    Skip the Tauri build (slow, needs the Rust toolchain) and reuse the existing binary.

.PARAMETER SkipFfmpeg
    Skip fetching the pinned FFmpeg binary and reuse build/ffmpeg/ffmpeg.exe.

.PARAMETER CompressedSingleFile
    Enable single-file internal bundle compression. Default is uncompressed single-file:
    uncompressed allows Windows to memory-map PE sections directly, saving ~75 MB of idle
    RAM commit and ~279 ms of cold start, while the installer and portable ZIP compress
    the payload anyway (yielding identical download sizes).

.EXAMPLE
    pwsh build/build-release.ps1
    pwsh build/build-release.ps1 -Version 1.0.1 -SkipSettings
#>
[CmdletBinding()]
param(
    [string]$Version,
    [switch]$SkipSettings,
    [switch]$SkipFfmpeg,
    [switch]$CompressedSingleFile
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$dist = Join-Path $repo 'dist'
$stage = Join-Path $dist 'reshot'

function Step($text) { Write-Host "`n=== $text ===" -ForegroundColor Cyan }
function Fail($text) { Write-Host "ERROR: $text" -ForegroundColor Red; exit 1 }

# ---- Version -----------------------------------------------------------------

if (-not $Version) {
    $props = Get-Content (Join-Path $repo 'Directory.Build.props') -Raw
    if ($props -match '<Version>([^<]+)</Version>') { $Version = $Matches[1] }
    else { Fail 'Could not read <Version> from Directory.Build.props.' }
}
Write-Host "reshot $Version" -ForegroundColor Green

# A running instance locks its own exe, which makes the build fail halfway through.
foreach ($name in 'reshot', 'reshot-tauri') {
    Get-Process $name -ErrorAction SilentlyContinue | ForEach-Object {
        Write-Host "Stopping running $name (pid $($_.Id))"
        Stop-Process -Id $_.Id -Force
    }
}

# ---- Clean -------------------------------------------------------------------

Step 'Preparing dist/'
if (Test-Path $dist) {
    # An Explorer window open on dist\ — or a virus scanner still reading the setup it
    # just saw appear — holds the directory itself. A recursive delete then empties the
    # folder and fails on the last step, taking the whole build with it. Emptying is what
    # actually matters, so a surviving empty directory is not an error.
    Get-ChildItem $dist -Force | Remove-Item -Recurse -Force
    Remove-Item $dist -Force -ErrorAction SilentlyContinue
}
New-Item -ItemType Directory -Path $stage -Force | Out-Null

# ---- FFmpeg -----------------------------------------------------------------

if (-not $SkipFfmpeg) {
    Step 'Fetching the pinned FFmpeg binary'
    & (Join-Path $PSScriptRoot 'fetch-ffmpeg.ps1')
}

$ffmpegExe = Join-Path $PSScriptRoot 'ffmpeg/ffmpeg.exe'
if (-not (Test-Path $ffmpegExe)) {
    Fail "FFmpeg binary missing: $ffmpegExe (run without -SkipFfmpeg)."
}

# ---- Tests -------------------------------------------------------------------

Step 'Running tests'
# The whole solution, so the ffmpeg command-line tests count too, not just the core ones.
dotnet test (Join-Path $repo 'reshot.sln') -c Release --nologo
if ($LASTEXITCODE -ne 0) { Fail 'Tests failed; refusing to package.' }

# ---- App ---------------------------------------------------------------------

# Uncompressed single-file is DELIBERATE and the default: EnableCompressionInSingleFile
# forces the .NET single-file host to decompress assemblies into anonymous RAM commit
# at launch, wasting ~75 MB of idle private memory and adding ~279 ms of cold-start
# latency. When uncompressed, Windows memory-maps PE sections directly from disk.
# The portable ZIP and Inno Setup installer compress the binary anyway (yielding
# virtually identical ~74 MB download sizes), so bundle-internal compression is purely
# a runtime penalty. Pass -CompressedSingleFile if disk space is strictly prioritized.
#
# The value is materialised into its own variable first: in argument mode PowerShell
# expands `$CompressedSingleFile.IsPresent` as an expandable string, yielding the literal
# `False.IsPresent`, which MSBuild reads as a non-boolean and silently ignores.
$compressSingleFile = if ($CompressedSingleFile) { 'true' } else { 'false' }
Step "Publishing reshot.exe (self-contained x64, single-file, compression=$compressSingleFile)"
dotnet publish (Join-Path $repo 'src/Reshot.App/Reshot.App.csproj') `
    -c Release -r win-x64 --self-contained true `
    -p:PublishSingleFile=true `
    -p:IncludeNativeLibrariesForSelfExtract=true `
    -p:EnableCompressionInSingleFile=$compressSingleFile `
    -p:DebugType=none `
    -o $stage --nologo
if ($LASTEXITCODE -ne 0) { Fail 'dotnet publish failed.' }

# Publishing drops the pdb/xml noise next to the exe; the release doesn't need it.
Get-ChildItem $stage -Include *.pdb, *.xml -Recurse | Remove-Item -Force -ErrorAction SilentlyContinue

# ---- Settings window ---------------------------------------------------------

$settingsExe = Join-Path $repo 'src/reshot-tauri/src-tauri/target/release/reshot-tauri.exe'

if (-not $SkipSettings) {
    Step 'Building the settings window (Tauri release)'
    Push-Location (Join-Path $repo 'src/reshot-tauri')
    try {
        if (-not (Test-Path 'node_modules')) {
            Write-Host 'Installing npm dependencies...'
            npm ci
            if ($LASTEXITCODE -ne 0) { Fail 'npm ci failed.' }
        }
        # --no-bundle: we package it ourselves. A plain `cargo build` would produce a DEV
        # binary that loads its UI from the vite dev server and shows a blank page.
        npm run tauri build -- --no-bundle
        if ($LASTEXITCODE -ne 0) { Fail 'Tauri build failed.' }
    }
    finally { Pop-Location }
}

if (-not (Test-Path $settingsExe)) {
    Fail "Settings binary missing: $settingsExe (run without -SkipSettings)."
}
Copy-Item $settingsExe (Join-Path $stage 'reshot-tauri.exe') -Force
Copy-Item $ffmpegExe (Join-Path $stage 'ffmpeg.exe') -Force

# ---- Extra files -------------------------------------------------------------

Copy-Item (Join-Path $repo 'LICENSE') $stage -Force
Copy-Item (Join-Path $repo 'README.md') $stage -Force
Copy-Item (Join-Path $repo 'THIRD-PARTY-NOTICES.md') $stage -Force

# ---- Portable ZIP ------------------------------------------------------------

Step 'Packing the portable ZIP'
$zip = Join-Path $dist "reshot-$Version-win-x64-portable.zip"
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal
Write-Host "  $zip"

# ---- Notice text for the wizard ----------------------------------------------

# The installer's info page renders plain text, so handing it the Markdown source would
# show the reader raw '#', '**' and link syntax. One notice, two renderings: the shipped
# file stays Markdown, the wizard gets this. Paragraphs are unwrapped so the wizard's own
# word wrap decides the line length instead of the Markdown source's 90-column habit.
function ConvertTo-NoticeText([string]$markdown) {
    $lines = New-Object System.Collections.Generic.List[string]
    foreach ($block in [regex]::Split($markdown, '(?:\r?\n){2,}')) {
        $text = $block.Trim()
        if (-not $text) { continue }
        $text = (($text -split '\r?\n') | ForEach-Object { $_.Trim() }) -join ' '
        $text = $text -replace '^#{1,6}\s*', ''                                  # headings
        $text = $text -replace '\[([^\]]+)\]\((https?://[^)]+)\)', '$1: $2'      # external links keep the URL
        $text = $text -replace '\[([^\]]+)\]\([^)]*\)', '$1'                     # in-repo links do not
        $text = $text -replace '\*\*([^*]+)\*\*', '$1'
        $text = $text -replace '`', ''
        $lines.Add($text)
        $lines.Add('')
    }
    return ($lines -join "`r`n")
}

$noticeTxt = Join-Path $dist 'THIRD-PARTY-NOTICES.txt'
ConvertTo-NoticeText (Get-Content (Join-Path $repo 'THIRD-PARTY-NOTICES.md') -Raw) |
    Set-Content $noticeTxt -Encoding utf8

# ---- Installer ---------------------------------------------------------------

Step 'Building the installer'
$iscc = Get-Command 'ISCC.exe' -ErrorAction SilentlyContinue
if (-not $iscc) {
    # Inno Setup installs per-machine or per-user depending on how it was installed;
    # winget's package lands in LocalAppData, which the Program Files guesses miss.
    foreach ($guess in @(
            "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
            "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
            "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe")) {
        if (Test-Path $guess) { $iscc = Get-Item $guess; break }
    }
}

if ($iscc) {
    & $iscc.FullName (Join-Path $PSScriptRoot 'installer.iss') `
        /DAppVersion=$Version /DPayloadDir=$stage /DOutputDir=$dist /DNoticeFile=$noticeTxt
    if ($LASTEXITCODE -ne 0) { Fail 'Inno Setup failed.' }
    Write-Host "  $(Join-Path $dist "reshot-$Version-setup.exe")"
}
else {
    Write-Host '  Inno Setup 6 not found; skipping the installer.' -ForegroundColor Yellow
    Write-Host '  Install it from https://jrsoftware.org/isdl.php and re-run.' -ForegroundColor Yellow
}

# ---- Checksums ---------------------------------------------------------------

Step 'Checksums'
$sums = Join-Path $dist 'SHA256SUMS.txt'
# -Include needs a wildcard path (or -Recurse) to match anything at all, hence dist\*.
Get-ChildItem (Join-Path $dist '*') -File -Include '*.zip', '*.exe' | ForEach-Object {
    $h = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower()
    "$h  $($_.Name)"
} | Set-Content $sums -Encoding utf8

if (Test-Path $sums) { Get-Content $sums | Write-Host }
else { Write-Host '  No artifacts to hash.' -ForegroundColor Yellow }

Step "Done: $dist"
