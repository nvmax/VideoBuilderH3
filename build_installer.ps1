<#
.SYNOPSIS
  Build the VRGDG H3 Studio installer.

.DESCRIPTION
  Runs the steps of docs/RELEASE.md in order: checks Rust, Node and npm, downloads the bundled ffmpeg when it is not
  there yet, installs the npm packages when they are missing, then runs `npm run release`.
  The installer is written to target\release\bundle\nsis and its path is printed at the end.

  Run it from anywhere:  powershell -ExecutionPolicy Bypass -File build_installer.ps1
  Or double-click build_installer.bat.
#>

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
    Write-Step ('Installer: {0} ({1:N1} MiB)' -f $installer.FullName, ($installer.Length / 1MB))
}
catch {
    Write-Host "[VRGDG-RS Build] $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
