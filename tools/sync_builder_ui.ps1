<#
.SYNOPSIS
  Copy the Video Builder's front end from the pack into the app, unchanged.

.DESCRIPTION
  The app shows the same Builder as the ComfyUI pack, so it uses the pack's own front-end files instead of a copy that
  drifts. This script copies them (never edits them) to app\web\extensions\comfyui-vrgamedevgirl, the path ComfyUI
  serves them from, so their relative imports work as they do there. Run it after the pack's front end changes.

  Copied: web\music_video_builder, web\storyboard_builder and the top level web\VRGDG_*.js files the Builder imports or
  needs. Not copied: node widgets for ComfyUI graphs (LLM_Multi, Qwen, LoRA switch and the like) and LTX tools.

  Run from the rustapp folder:  powershell -File tools\sync_builder_ui.ps1 [-Pack <pack folder>]
#>
param([string]$Pack = (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)))

$ErrorActionPreference = 'Stop'
$web = Join-Path $Pack 'web'
$target = Join-Path (Split-Path -Parent $PSScriptRoot) 'app\web\extensions\comfyui-vrgamedevgirl'
if (-not (Test-Path (Join-Path $web 'music_video_builder'))) { throw "No Builder front end in $web" }

$top = @(
    'VRGDG_BrowserImageBridge.js', 'VRGDG_OverlayTrack.js', 'VRGDG_UIThemes.js', 'VRGDG_ResourceMonitor.js',
    'VRGDG_MusicVideoBuilderLUTs.js', 'VRGDG_PostProcessComparePreview.js', 'VRGDG_RenderETA.js',
    'VRGDG_StoryboardBuilderUI.js', 'VRGDG_RefModImagePicker.js', 'VRGDG_ShowText.js'
)

if (Test-Path $target) { Remove-Item -Recurse -Force $target }
New-Item -ItemType Directory -Force $target | Out-Null
foreach ($folder in 'music_video_builder', 'storyboard_builder') {
    Copy-Item (Join-Path $web $folder) (Join-Path $target $folder) -Recurse
}
foreach ($file in $top) {
    $source = Join-Path $web $file
    if (Test-Path $source) { Copy-Item $source $target } else { Write-Warning "Missing in the pack: $file" }
}

$commit = (git -C $Pack rev-parse --short HEAD 2>$null)
@"
Copied unchanged from the pack's web folder by tools\sync_builder_ui.ps1.
Pack commit: $commit
Do not edit these files. Change app\web\host\* (the ComfyUI stand-in) or rerun the script.
"@ | Set-Content -Encoding utf8 (Join-Path $target 'COPIED_FROM_PACK.txt')

$count = (Get-ChildItem $target -Recurse -File).Count
Write-Host "Copied $count files from commit $commit to $target"

