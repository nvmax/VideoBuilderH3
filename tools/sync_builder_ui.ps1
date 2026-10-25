<#
.SYNOPSIS
  Copy the Video Builder's front end from the pack into the app, unchanged.

.DESCRIPTION
  The app shows the same Builder as the ComfyUI pack, so it uses the pack's own front-end files instead of a copy that
  drifts. This script copies them (never edits them) to app\web\extensions\comfyui-vrgamedevgirl, the path ComfyUI
  serves them from, so their relative imports work as they do there. Run it after the pack's front end changes.

  Copied: web\music_video_builder, web\storyboard_builder and the top level web\VRGDG_*.js files the Builder imports or
  needs. Not copied: node widgets for ComfyUI graphs (LLM_Multi, Qwen, LoRA switch and the like) and LTX tools. Two that the Builder imports for buttons this app hides (Face Fix, Wizard) are copied, because a missing import stops the whole Builder.

  A short list of exact replacements in tools\builder_ui_patches.json is applied to the copies afterwards (the engine a new
  project starts with, for one). Run from the rustapp folder:  powershell -File tools\sync_builder_ui.ps1 [-Pack <pack folder>]
#>
param([string]$Pack = (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)))

$ErrorActionPreference = 'Stop'
$web = Join-Path $Pack 'web'
$target = Join-Path (Split-Path -Parent $PSScriptRoot) 'app\web\extensions\comfyui-vrgamedevgirl'
if (-not (Test-Path (Join-Path $web 'music_video_builder'))) { throw "No Builder front end in $web" }

$top = @(
    'VRGDG_BrowserImageBridge.js', 'VRGDG_OverlayTrack.js', 'VRGDG_UIThemes.js', 'VRGDG_ResourceMonitor.js',
    'VRGDG_MusicVideoBuilderLUTs.js', 'VRGDG_PostProcessComparePreview.js', 'VRGDG_RenderETA.js',
    'VRGDG_StoryboardBuilderUI.js', 'VRGDG_RefModImagePicker.js', 'VRGDG_ShowText.js',
    'VRGDG_FaceFixUI.js', 'VRGDG_Wizard.js'
)

# Empty the folders instead of deleting them. An editor that watches one, or a file explorer window that has one open,
# stops Windows from deleting the folder itself.
New-Item -ItemType Directory -Force $target | Out-Null
Get-ChildItem $target -Force | ForEach-Object {
    if ($_.PSIsContainer) { Get-ChildItem $_.FullName -Force | Remove-Item -Recurse -Force } else { Remove-Item $_.FullName -Force }
}
foreach ($folder in 'music_video_builder', 'storyboard_builder') {
    $destination = Join-Path $target $folder
    New-Item -ItemType Directory -Force $destination | Out-Null
    Copy-Item (Join-Path (Join-Path $web $folder) '*') $destination -Recurse
}
foreach ($file in $top) {
    $source = Join-Path $web $file
    if (Test-Path $source) { Copy-Item $source $target } else { Write-Warning "Missing in the pack: $file" }
}

# A few small, explicit changes to the copies (tools\builder_ui_patches.json). Each must match exactly as often as it says, so a
# change in the pack's code that moves a line makes the sync fail instead of leaving a patch half applied.
$patchFile = Join-Path $PSScriptRoot 'builder_ui_patches.json'
$applied = @()
if (Test-Path $patchFile) {
    foreach ($patch in (Get-Content $patchFile -Raw | ConvertFrom-Json)) {
        $path = Join-Path $target $patch.file
        $text = [IO.File]::ReadAllText($path)
        # The files use Windows line ends. The patch text is written with plain ones.
        $newline = if ($text.Contains("`r`n")) { "`r`n" } else { "`n" }
        $find = $patch.find.Replace("`r`n", "`n").Replace("`n", $newline)
        $replace = $patch.replace.Replace("`r`n", "`n").Replace("`n", $newline)
        $count = ([regex]::Matches($text, [regex]::Escape($find))).Count
        if ($count -ne $patch.expect) { throw "Patch for $($patch.file) matched $count times, expected $($patch.expect): $($patch.find)" }
        [IO.File]::WriteAllText($path, $text.Replace($find, $replace), (New-Object Text.UTF8Encoding $false))
        $applied += "  $($patch.file): $($patch.why)"
    }
}

$commit = (git -C $Pack rev-parse --short HEAD 2>$null)
@"
Copied unchanged from the pack's web folder by tools\sync_builder_ui.ps1.
Pack commit: $commit
Do not edit these files. Change app\web\host\* (the ComfyUI stand-in), tools\builder_ui_patches.json or rerun the script.
Changes made to the copies by tools\builder_ui_patches.json:
$($applied -join "`n")
"@ | Set-Content -Encoding utf8 (Join-Path $target 'COPIED_FROM_PACK.txt')

$count = (Get-ChildItem $target -Recurse -File).Count
Write-Host "Copied $count files from commit $commit to $target"

