<#
.SYNOPSIS
    Fetches the pinned GPL FFmpeg build required by Reshot releases.

.DESCRIPTION
    Downloads the versioned BtbN win64 GPL package, verifies its SHA-256, and extracts
    only ffmpeg.exe to build/ffmpeg/ffmpeg.exe. The build has libx264 enabled.

    The package is cached in %TEMP% by default. CI may set RESHOT_FFMPEG_CACHE_DIR to
    preserve that verified package between workflow runs.
#>
[CmdletBinding()]
param(
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

# BtbN source/build page: https://github.com/BtbN/FFmpeg-Builds
$FfmpegVersion = 'N-125875-g5d4d3bdc61-20260731 (BtbN autobuild-2026-07-31-14-10)'
$FfmpegUrl = 'https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-07-31-14-10/ffmpeg-N-125875-g5d4d3bdc61-win64-gpl.zip'
$FfmpegArchiveSha256 = '68a5e966533002785c3e4b9a98327e21d5277802668bf889d94086cb6426cbb4'
$FfmpegExeSha256 = (Get-Content -LiteralPath (Join-Path $PSScriptRoot 'ffmpeg.sha256') -Raw).Trim()
if ($FfmpegExeSha256 -cnotmatch '^[0-9a-f]{64}$') {
    throw 'build/ffmpeg.sha256 must contain exactly one lowercase SHA-256 digest.'
}

$outputDirectory = Join-Path $PSScriptRoot 'ffmpeg'
$outputPath = Join-Path $outputDirectory 'ffmpeg.exe'
$cacheDirectory = if ($env:RESHOT_FFMPEG_CACHE_DIR) {
    $env:RESHOT_FFMPEG_CACHE_DIR
}
else {
    Join-Path ([System.IO.Path]::GetTempPath()) 'reshot-ffmpeg-cache'
}
$archivePath = Join-Path $cacheDirectory 'ffmpeg-N-125875-g5d4d3bdc61-win64-gpl.zip'

function Get-Sha256([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

if ((Test-Path -LiteralPath $outputPath) -and -not $Force) {
    $existingHash = Get-Sha256 $outputPath
    if ($existingHash -eq $FfmpegExeSha256) {
        Write-Host "FFmpeg $FfmpegVersion is already verified: $outputPath" -ForegroundColor Green
        return
    }

    Write-Warning "Existing ffmpeg.exe has SHA-256 $existingHash; expected $FfmpegExeSha256. Fetching a fresh copy."
}

New-Item -ItemType Directory -Path $outputDirectory, $cacheDirectory -Force | Out-Null

$downloadRequired = $Force -or -not (Test-Path -LiteralPath $archivePath)
if (-not $downloadRequired) {
    $cachedHash = Get-Sha256 $archivePath
    if ($cachedHash -ne $FfmpegArchiveSha256) {
        Write-Warning "Cached FFmpeg package has SHA-256 $cachedHash; expected $FfmpegArchiveSha256. Downloading it again."
        $downloadRequired = $true
    }
}

if ($downloadRequired) {
    Write-Host "Downloading FFmpeg $FfmpegVersion..." -ForegroundColor Cyan
    Write-Host "  $FfmpegUrl"
    Invoke-WebRequest -Uri $FfmpegUrl -OutFile $archivePath
}
else {
    Write-Host "Using verified cached FFmpeg package: $archivePath"
}

$archiveHash = Get-Sha256 $archivePath
if ($archiveHash -ne $FfmpegArchiveSha256) {
    throw "FFmpeg package SHA-256 mismatch. Expected $FfmpegArchiveSha256, got $archiveHash. Delete '$archivePath' and retry, or run with -Force."
}

$extractDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ("reshot-ffmpeg-extract-" + [guid]::NewGuid().ToString('N'))
try {
    Expand-Archive -LiteralPath $archivePath -DestinationPath $extractDirectory
    $sourceExe = Get-ChildItem -LiteralPath $extractDirectory -Filter 'ffmpeg.exe' -File -Recurse | Select-Object -First 1
    if (-not $sourceExe) {
        throw "The verified FFmpeg package did not contain ffmpeg.exe: $archivePath"
    }

    Copy-Item -LiteralPath $sourceExe.FullName -Destination $outputPath -Force
}
finally {
    if (Test-Path -LiteralPath $extractDirectory) {
        Remove-Item -LiteralPath $extractDirectory -Recurse -Force
    }
}

$outputHash = Get-Sha256 $outputPath
if ($outputHash -ne $FfmpegExeSha256) {
    throw "Extracted ffmpeg.exe SHA-256 mismatch. Expected $FfmpegExeSha256, got $outputHash. The package may be corrupt; retry with -Force."
}

$size = (Get-Item -LiteralPath $outputPath).Length
Write-Host "Verified ffmpeg.exe: $outputPath ($size bytes)" -ForegroundColor Green
