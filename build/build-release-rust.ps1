<#
.SYNOPSIS
    Builds every release artifact of the Rust Reshot: reshot.exe, a portable ZIP and (when
    Inno Setup is present) the installer.
.DESCRIPTION
    Reshot ships as one executable plus the pinned GPL ffmpeg.exe beside it, where
    reshot-recording looks for it first. The settings window is part of reshot.exe now,
    so there is no second binary.
.PARAMETER Version
    Version stamped into the artifact names. Defaults to [workspace.package] version in
    rust/Cargo.toml, so bumping it there is enough.
.PARAMETER SkipFfmpeg
    Skip fetching the pinned FFmpeg binary and reuse build/ffmpeg/ffmpeg.exe.
.EXAMPLE
    pwsh build/build-release-rust.ps1
#>
[CmdletBinding()]
param(
    [string]$Version,
    [switch]$SkipFfmpeg
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$rust = Join-Path $repo 'rust'
$dist = Join-Path $repo 'dist'
$stage = Join-Path $dist 'reshot'

function Step($text) { Write-Host "`n=== $text ===" -ForegroundColor Cyan }
function Fail($text) { Write-Host "ERROR: $text" -ForegroundColor Red; exit 1 }

if (-not $Version) {
    $toml = Get-Content (Join-Path $rust 'Cargo.toml') -Raw
    if ($toml -match '(?ms)\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') { $Version = $Matches[1] }
    else { Fail 'Could not read [workspace.package] version from rust/Cargo.toml.' }
}
Write-Host "reshot $Version" -ForegroundColor Green

# A running copy holds reshot.exe open and the copy below would fail.
foreach ($name in 'reshot', 'reshot-tauri') {
    Get-Process $name -ErrorAction SilentlyContinue | ForEach-Object {
        Write-Host "Stopping running $name (pid $($_.Id))"
        Stop-Process -Id $_.Id -Force
    }
}

Step 'Preparing dist/'
if (Test-Path $dist) {
    Get-ChildItem $dist -Force | Remove-Item -Recurse -Force
    Remove-Item $dist -Force -ErrorAction SilentlyContinue
}
New-Item -ItemType Directory -Path $stage -Force | Out-Null

if (-not $SkipFfmpeg) {
    Step 'Fetching the pinned FFmpeg binary'
    & (Join-Path $PSScriptRoot 'fetch-ffmpeg.ps1')
}
$ffmpegExe = Join-Path $PSScriptRoot 'ffmpeg/ffmpeg.exe'
if (-not (Test-Path $ffmpegExe)) { Fail "FFmpeg binary missing: $ffmpegExe (run without -SkipFfmpeg)." }

Step 'Running tests'
cargo test --manifest-path (Join-Path $rust 'Cargo.toml') --workspace --release
if ($LASTEXITCODE -ne 0) { Fail 'Tests failed; refusing to package.' }

Step 'Building reshot.exe (release)'
cargo build --manifest-path (Join-Path $rust 'Cargo.toml') -p reshot --release
if ($LASTEXITCODE -ne 0) { Fail 'cargo build failed.' }

Copy-Item (Join-Path $rust 'target/release/reshot.exe') (Join-Path $stage 'reshot.exe') -Force
Copy-Item $ffmpegExe (Join-Path $stage 'ffmpeg.exe') -Force
Copy-Item (Join-Path $repo 'LICENSE') $stage -Force
Copy-Item (Join-Path $repo 'README.md') $stage -Force
Copy-Item (Join-Path $repo 'THIRD-PARTY-NOTICES.md') $stage -Force

Step 'Packing the portable ZIP'
$zip = Join-Path $dist "reshot-$Version-win-x64-portable.zip"
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal
Write-Host "  $zip"

# The installer shows the notices as plain text, so the Markdown is flattened for it.
$noticeTxt = Join-Path $dist 'THIRD-PARTY-NOTICES.txt'
$md = Get-Content (Join-Path $repo 'THIRD-PARTY-NOTICES.md') -Raw
$text = foreach ($block in [regex]::Split($md, '(?:\r?\n){2,}')) {
    $t = $block.Trim()
    if (-not $t) { continue }
    $t = (($t -split '\r?\n') | ForEach-Object { $_.Trim() }) -join ' '
    $t = $t -replace '^#{1,6}\s*', ''
    $t = $t -replace '\[([^\]]+)\]\((https?://[^)]+)\)', '$1: $2'
    $t = $t -replace '\[([^\]]+)\]\([^)]*\)', '$1'
    $t = $t -replace '\*\*([^*]+)\*\*', '$1'
    $t -replace '`', ''
    ''
}
$text -join "`r`n" | Set-Content $noticeTxt -Encoding utf8

Step 'Building the installer'
$iscc = Get-Command 'ISCC.exe' -ErrorAction SilentlyContinue
if (-not $iscc) {
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
}

Step 'Checksums'
$sums = Join-Path $dist 'SHA256SUMS.txt'
Get-ChildItem (Join-Path $dist '*') -File -Include '*.zip', '*.exe' | ForEach-Object {
    $h = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower()
    "$h  $($_.Name)"
} | Set-Content $sums -Encoding ascii
Write-Host "  $sums"
