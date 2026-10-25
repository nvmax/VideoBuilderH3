<#
.SYNOPSIS
  Publish the built installer as a GitHub release that installed apps can update from.

.DESCRIPTION
  Run by .github/workflows/release.yml after `npm run release` has built and signed the installer. It checks that the tag
  matches the version in the three version files, names the installer without spaces (GitHub renames a file with spaces, which
  would break the address in the update file), writes latest.json (the file the app reads: version, notes, address and
  signature of the installer), creates the release as a draft, uploads the files, and only then publishes it, so an app never
  sees a release without latest.json.

  The release notes are docs\release_notes\<version>.md when that file exists, else the commit subjects since the last tag.

  -DryRun makes the files in -OutDir and does not touch GitHub. Needs `gh` and GH_TOKEN (or a gh login) otherwise.
#>
param(
    [Parameter(Mandatory)][string]$Tag,
    [string]$Repo = 'nvmax/VideoBuilderH3',
    [string]$OutDir,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$version = $Tag.TrimStart('v')

function Write-Step([string]$message) {
    Write-Host "[VRGDG-RS Release] $message"
}

function Write-Utf8([string]$path, [string]$text) {
    [System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding $false))
}

# The tag, the installer and the app all have to say the same version.
$found = [ordered]@{
    'Cargo.toml'                      = [regex]::Match([System.IO.File]::ReadAllText((Join-Path $root 'Cargo.toml')), '(?m)^version = "([^"]*)"').Groups[1].Value
    'app/src-tauri/tauri.conf.json'   = (Get-Content (Join-Path $root 'app\src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json).version
    'app/package.json'                = (Get-Content (Join-Path $root 'app\package.json') -Raw | ConvertFrom-Json).version
}
foreach ($file in $found.Keys) {
    if ($found[$file] -ne $version) { throw "The tag says $version but $file says $($found[$file]). Run tools\set_version.ps1 -Version $version, commit, and tag again." }
}

$bundle = Join-Path $root 'target\release\bundle\nsis'
$installer = Get-ChildItem $bundle -Filter "*_${version}_x64-setup.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $installer) { throw "No installer for version $version in $bundle." }
$signature = "$($installer.FullName).sig"
if (-not (Test-Path $signature)) { throw "The installer has no signature file ($signature). The build needs TAURI_SIGNING_PRIVATE_KEY." }

if (-not $OutDir) { $OutDir = Join-Path ([System.IO.Path]::GetTempPath()) "h3-release-$version" }
if (Test-Path $OutDir) { Remove-Item -Recurse -Force $OutDir }
New-Item -ItemType Directory -Force $OutDir | Out-Null

$assetName = "H3-Studio_${version}_x64-setup.exe"
Copy-Item $installer.FullName (Join-Path $OutDir $assetName)
Copy-Item $signature (Join-Path $OutDir "$assetName.sig")

# Release notes.
$notesFile = Join-Path $root "docs\release_notes\$version.md"
if (Test-Path $notesFile) {
    $notes = ([System.IO.File]::ReadAllText($notesFile)).Trim()
}
else {
    # git writes to stderr when there is no earlier tag (or, in a dry run, no tag yet), which Windows PowerShell treats as an error.
    $ErrorActionPreference = 'Continue'
    $end = if (git -C $root rev-parse -q --verify "refs/tags/$Tag" 2>$null) { $Tag } else { 'HEAD' }
    $previous = git -C $root describe --tags --abbrev=0 "$end^" 2>$null
    if ($LASTEXITCODE -eq 0 -and $previous) {
        # The first 40 subjects: a long list is not a note an update dialog can show.
        $notes = (git -C $root log "$previous..$end" --no-merges --pretty='- %s' 2>$null | Select-Object -First 40) -join "`n"
    }
    $ErrorActionPreference = 'Stop'
}
if (-not $notes) { $notes = "H3 Studio $version" }

$manifest = [ordered]@{
    version  = $version
    notes    = $notes
    pub_date = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    platforms = [ordered]@{
        'windows-x86_64' = [ordered]@{
            signature = (Get-Content $signature -Raw).Trim()
            url       = "https://github.com/$Repo/releases/download/$Tag/$assetName"
        }
    }
}
$latest = Join-Path $OutDir 'latest.json'
Write-Utf8 $latest (($manifest | ConvertTo-Json -Depth 6) + "`n")
$notesOut = Join-Path $OutDir 'notes.md'
Write-Utf8 $notesOut ($notes + "`n")
Write-Step "Made the release files for $Tag in $OutDir"

if ($DryRun) { return }

gh release create $Tag --repo $Repo --draft --title "H3 Studio $version" --notes-file $notesOut (Join-Path $OutDir $assetName) (Join-Path $OutDir "$assetName.sig")
if ($LASTEXITCODE -ne 0) { throw 'gh could not create the release.' }
gh release upload $Tag $latest --repo $Repo
if ($LASTEXITCODE -ne 0) { throw 'gh could not upload latest.json.' }
gh release edit $Tag --repo $Repo --draft=false --latest
if ($LASTEXITCODE -ne 0) { throw 'gh could not publish the release.' }
Write-Step "Published https://github.com/$Repo/releases/tag/$Tag"
