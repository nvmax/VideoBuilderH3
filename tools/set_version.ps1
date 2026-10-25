<#
.SYNOPSIS
  Set the app version in the three files that hold it.

.DESCRIPTION
  The version is in Cargo.toml (workspace), app/src-tauri/tauri.conf.json and app/package.json. The installer, the update
  check and the release tag all use it, and tools/publish_release.ps1 refuses a tag that does not match the three.
  Run from anywhere:  powershell -File tools\set_version.ps1 -Version 0.1.1
#>
param(
    [Parameter(Mandatory)][string]$Version
)

$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "The version must look like 1.2.3, not '$Version'." }
$root = Split-Path -Parent $PSScriptRoot

# Keeps each file's line endings and encoding: only the one version line changes.
function Set-VersionLine([string]$relative, [string]$pattern, [string]$replacement, [int]$count = 1) {
    $path = Join-Path $root $relative
    $text = [System.IO.File]::ReadAllText($path)
    $regex = [regex]::new($pattern, 'Multiline')
    if (-not $regex.IsMatch($text)) { throw "No version line found in $relative." }
    $updated = $regex.Replace($text, $replacement, $count)
    [System.IO.File]::WriteAllText($path, $updated, (New-Object System.Text.UTF8Encoding $false))
    Write-Host "[VRGDG-RS Release] $relative -> $Version"
}

Set-VersionLine 'Cargo.toml' '^version = "[^"]*"' "version = `"$Version`""
Set-VersionLine 'app\src-tauri\tauri.conf.json' '^(\s*)"version": "[^"]*"' "`$1`"version`": `"$Version`""
Set-VersionLine 'app\package.json' '^(\s*)"version": "[^"]*"' "`$1`"version`": `"$Version`""
# The npm lock file names the app's version twice, before any dependency: at the top and in its own package entry.
Set-VersionLine 'app\package-lock.json' '^(\s*)"version": "[^"]*"' "`$1`"version`": `"$Version`"" 2

# The lock file names the version of every crate of the workspace.
$env:Path = @([Environment]::GetEnvironmentVariable('Path', 'Machine'), [Environment]::GetEnvironmentVariable('Path', 'User'), (Join-Path $env:USERPROFILE '.cargo\bin'), $env:Path) -join ';'
if (Get-Command cargo -ErrorAction SilentlyContinue) {
    Push-Location $root
    try { cargo update --workspace --quiet } finally { Pop-Location }
}

Write-Host ''
Write-Host "[VRGDG-RS Release] Next: commit, then  git tag -a v$Version -m `"H3 Studio $Version`"  and push the tag to publish."
