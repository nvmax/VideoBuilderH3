# Building and releasing VRGDG H3 Studio

## Build the installer (Windows)

Needs Rust (stable, MSVC), Node 20 or newer and PowerShell. Run these in `rustapp`.

1. `powershell -File tools\fetch_ffmpeg.ps1` downloads the ffmpeg build the installer carries and puts it in
   `app\src-tauri\binaries` with the license files in `app\src-tauri\licenses`. Both folders are ignored by git.
   Use `-Flavor lgpl` only to test: the LGPL build has no libx264, so rendering would be refused (the Setup screen says so).
2. `cd app`, `npm install`, `npm run release`.
3. The installer is `target\release\bundle\nsis\VRGDG H3 Studio_<version>_x64-setup.exe`.

`npm run release` uses `src-tauri\tauri.release.conf.json`, which adds the ffmpeg sidecar, the license files and the
NSIS settings (per-user install, no administrator rights). A normal `cargo build`, `cargo test` and `npm run tauri dev`
do not need the ffmpeg files.

The installer is about 100 MB because the static ffmpeg and ffprobe are 157 MB each before compression.

## What the app does on first start

- It has no config file yet, so it opens on the Setup screen.
- The Setup screen checks ComfyUI (address, version), the H3 node packs (named by the pack that provides them),
  ComfyUI's output folder, the project folder, ffmpeg and ffprobe (bundled one first, then PATH), the libx264 encoder,
  and the LLM (a warning only, rendering does not need it).
- Saving writes `%APPDATA%\VRGDG-H3\config.json` and restarts the app.

The app must run on the same computer as ComfyUI, because scene files are exchanged by path.

## Before the first public release

1. **Code signing.** An unsigned installer shows a SmartScreen warning. Buy or obtain a code signing certificate, then add
   to `tauri.release.conf.json` under `bundle.windows`: `"certificateThumbprint"`, `"digestAlgorithm": "sha256"` and
   `"timestampUrl"`, or use `"signCommand"` for a cloud signing service. Sign the sidecar programs too (ffmpeg.exe and
   ffprobe.exe), or leave them as they come.
2. **Update channel.** Not built. The plan is the Tauri updater plugin with a signed `latest.json` on a release page:
   `npm run tauri signer generate` makes the key pair (keep the private key out of the repo), put the public key and the
   endpoint in the config, and sign each build with `TAURI_SIGNING_PRIVATE_KEY`. Until then users download new installers.
3. **License contact.** `licenses\THIRD_PARTY_NOTICES.md` promises a source copy of ffmpeg on request. Put a real contact
   address or project page in it.
4. **Git history.** The local history has the author's email address in it. Squash it into a fresh history before the
   repository is published.
5. **Secrets.** No API key is stored in the repo. Check with `git grep -i "sk-"` before publishing.
6. **A clean machine test.** Install on a Windows account that has no Rust, Node or ffmpeg, start ComfyUI with the node packs,
   and render one scene.
