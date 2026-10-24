<#
.SYNOPSIS
  Download the ffmpeg build the installer bundles and put it where Tauri expects its sidecar programs.

.DESCRIPTION
  Writes app/src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe and ffprobe-x86_64-pc-windows-msvc.exe, and the
  license files under app/src-tauri/licenses. Run it once before `npm run tauri build`.

  The app's trimming, stitching and slideshow use the libx264 encoder, which only the GPL build has. The LGPL build
  has no software H.264 encoder, so the app would refuse to start rendering with it (the setup screen says so).
  The GPL build runs as a separate program. The app does not link to it. The installer must ship the license text
  and the source offer in THIRD_PARTY_NOTICES.md.

  Run from the rustapp folder:  powershell -File tools\fetch_ffmpeg.ps1
#>
param(
    [ValidateSet('gpl', 'lgpl')][string]$Flavor = 'gpl',
    [string]$Release = 'n8.1',
    [string]$Triple = 'x86_64-pc-windows-msvc'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$tauri = Join-Path $root 'app\src-tauri'
$binaries = Join-Path $tauri 'binaries'
$licenses = Join-Path $tauri 'licenses'
$cache = Join-Path $binaries '.cache'
New-Item -ItemType Directory -Force $binaries, $licenses, $cache | Out-Null

$name = "ffmpeg-$Release-latest-win64-$Flavor-$($Release.TrimStart('n')).zip"
$url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/$name"
$zip = Join-Path $cache $name
if (-not (Test-Path $zip)) {
    Write-Host "Downloading $url"
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
}

$extract = Join-Path $cache ($name -replace '\.zip$', '')
if (Test-Path $extract) { Remove-Item -Recurse -Force $extract }
Expand-Archive -Path $zip -DestinationPath $extract
$top = Get-ChildItem $extract -Directory | Select-Object -First 1

foreach ($tool in 'ffmpeg', 'ffprobe') {
    $source = Join-Path $top.FullName "bin\$tool.exe"
    if (-not (Test-Path $source)) { throw "$tool.exe is not in $($top.FullName)\bin" }
    Copy-Item $source (Join-Path $binaries "$tool-$Triple.exe") -Force
}
Copy-Item (Join-Path $top.FullName 'LICENSE.txt') (Join-Path $licenses "ffmpeg-$Flavor-LICENSE.txt") -Force

$version = & (Join-Path $binaries "ffmpeg-$Triple.exe") -hide_banner -version | Select-Object -First 1
@"
$version
Build: BtbN FFmpeg-Builds, $name
Download: $url
Source: https://github.com/BtbN/FFmpeg-Builds and https://ffmpeg.org/download.html
"@ | Set-Content -Encoding utf8 (Join-Path $licenses 'ffmpeg-VERSION.txt')

Write-Host "Ready: $version"
