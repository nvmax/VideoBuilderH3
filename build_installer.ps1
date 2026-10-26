<#
.SYNOPSIS
  Build the VRGDG H3 Studio installer.

.DESCRIPTION
  Runs the steps of docs/RELEASE.md in order: checks Rust, Node and npm, downloads the bundled ffmpeg when it is not
  there yet, installs the npm packages when they are missing, then runs `npm run release`.
  The installer is written to target\release\bundle\nsis and its path is printed at the end.

  When the node pack is the folder this one sits in (the usual place), the build first copies the pack's Video Builder front
  end into the app (tools\sync_builder_ui.ps1) and refreshes the agent tool catalog and instructions from it
  (tools\export_mcp_catalog.py), so the installer carries the pack's current files, committed or not. The Rust code is always
  built from the files on disk. Pass -NoSync to build exactly what is in this folder now.

  The installer carries the version that will be released next: one patch above the latest release tag (v0.1.4 builds 0.1.5),
  or the version already in the version files when that is higher. The script sets it in the three version files
  (tools\set_version.ps1), so a test build is the build that gets tagged and pushed later. Nothing is committed or pushed.
  Pass -Version 0.2.0 for another number, or -KeepVersion to build the number in the files.

  Run it from anywhere:  powershell -ExecutionPolicy Bypass -File build_installer.ps1 [-NoSync] [-Version x.y.z] [-KeepVersion]
  Or double-click build_installer.bat (a terminal run can pass the options too).
#>
param([switch]$NoSync, [string]$Version = '', [switch]$KeepVersion)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$app = Join-Path $root 'app'
$triple = 'x86_64-pc-windows-msvc'

function Write-Step([string]$message) {
    Write-Host "[VRGDG-RS Build] $message"
}

function Assert-Tool([string]$name, [string]$hint) {
    if (-not (Get-Command $name -ErrorAction SilentlyContinue)) {
        throw "$name was not found. $hint"
    }
}

try {
    # A shell that was opened before Rust or Node was installed does not have them on its path yet.
    $env:Path = @(
        [Environment]::GetEnvironmentVariable('Path', 'Machine'),
        [Environment]::GetEnvironmentVariable('Path', 'User'),
        (Join-Path $env:USERPROFILE '.cargo\bin'),
        $env:Path
    ) -join ';'

    Assert-Tool cargo 'Install Rust (stable, MSVC) from https://rustup.rs.'
    Assert-Tool node 'Install Node 20 or newer from https://nodejs.org.'
    Assert-Tool npm 'Install Node 20 or newer from https://nodejs.org.'
    $nodeMajor = [int]((node --version).TrimStart('v').Split('.')[0])
    if ($nodeMajor -lt 20) { throw "Node $nodeMajor is too old, the build needs Node 20 or newer." }

    # A copy of the app that runs from the target folder keeps the build from replacing its files.
    $target = Join-Path $root 'target'
    $running = Get-Process h3-studio, h3 -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($target, [StringComparison]::OrdinalIgnoreCase) }
    if ($running) {
        $names = ($running | ForEach-Object { $_.ProcessName } | Sort-Object -Unique) -join ', '
        throw "Close $names first. It runs from the target folder and would block the build."
    }

    # The app shows the pack's Video Builder and ships the pack's agent instructions. Copy both from the pack as it is now, so a
    # build made before the pack is pushed (or before this repository is committed) still has the latest.
    $pack = Split-Path -Parent $root
    if ($NoSync) {
        Write-Step 'Skipping the copy from the pack (-NoSync).'
    }
    elseif (-not (Test-Path (Join-Path $pack 'web\music_video_builder'))) {
        Write-Step "No node pack at $pack, building with the front end already in app\web."
    }
    else {
        Write-Step "Copying the Video Builder front end from $pack."
        & (Join-Path $root 'tools\sync_builder_ui.ps1') -Pack $pack
        # The pack's own Python is the one that is always there. Any Python 3 does the job (standard library only).
        $python = Join-Path (Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $pack))) 'python_embeded\python.exe'
        if (-not (Test-Path $python)) { $python = (Get-Command python -ErrorAction SilentlyContinue).Source }
        if ($python) {
            Write-Step 'Refreshing the agent tool catalog and instructions from the pack.'
            & $python (Join-Path $root 'tools\export_mcp_catalog.py') --pack $pack
            if ($LASTEXITCODE -ne 0) { throw 'tools\export_mcp_catalog.py failed. The messages above say why.' }
        }
        else {
            Write-Warning 'No Python found, so the agent tool catalog was not refreshed from the pack.'
        }
    }

    # The version of this build: the one that will be released next, so what is tested here is what gets tagged and pushed.
    $cargoText = Get-Content (Join-Path $root 'Cargo.toml') -Raw
    $current = [regex]::Match($cargoText, '(?m)^version = "(\d+\.\d+\.\d+)"').Groups[1].Value
    if (-not $current) { throw 'No version found in Cargo.toml.' }
    if ($Version) {
        if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "The version must look like 1.2.3, not '$Version'." }
        $wanted = $Version
    }
    elseif ($KeepVersion) {
        $wanted = $current
    }
    else {
        $wanted = $current
        # A repository with no tags (or no git) keeps the version in the files.
        try { $tag = (git -C $root describe --tags --abbrev=0 2>$null) } catch { $tag = '' }
        if ($tag -match '^v?(\d+\.\d+\.\d+)$') {
            $released = [version]$Matches[1]
            if ([version]$current -le $released) { $wanted = '{0}.{1}.{2}' -f $released.Major, $released.Minor, ($released.Build + 1) }
        }
    }
    if ($wanted -ne $current) {
        Write-Step "Setting the version to $wanted (it was $current)."
        & (Join-Path $root 'tools\set_version.ps1') -Version $wanted
    }
    Write-Step "Building version $wanted."

    $sidecars = 'ffmpeg', 'ffprobe' | ForEach-Object { Join-Path $app "src-tauri\binaries\$_-$triple.exe" }
    if ($sidecars | Where-Object { -not (Test-Path $_) }) {
        Write-Step 'The bundled ffmpeg is missing, downloading it.'
        & (Join-Path $root 'tools\fetch_ffmpeg.ps1')
    }

    Push-Location $app
    try {
        if (-not (Test-Path 'node_modules\.bin\tauri.cmd')) {
            Write-Step 'Installing the npm packages.'
            npm install
            if ($LASTEXITCODE -ne 0) { throw 'npm install failed.' }
        }
        Write-Step 'Building the installer. The first build takes several minutes.'
        $started = Get-Date
        # A build on this computer is not published, so it does not make the signed update files (they need the private update key).
        npm run release:local
        if ($LASTEXITCODE -ne 0) { throw 'The build failed. The messages above say why.' }
    }
    finally {
        Pop-Location
    }

    $installer = Get-ChildItem (Join-Path $target 'release\bundle\nsis') -Filter '*-setup.exe' -ErrorAction SilentlyContinue |
        Where-Object { $_.LastWriteTime -ge $started } |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
    if (-not $installer) { throw 'The build finished but there is no new installer in target\release\bundle\nsis.' }

    Write-Host ''
    Write-Step ('Installer for version {0}: {1} ({2:N1} MiB)' -f $wanted, $installer.FullName, ($installer.Length / 1MB))
    Write-Step 'Nothing was committed or pushed. Install it over your current copy to test.'
}
catch {
    Write-Host "[VRGDG-RS Build] $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
