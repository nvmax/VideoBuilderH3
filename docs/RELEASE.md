# Building and releasing VRGDG H3 Studio

## Build the installer (Windows)

Needs Rust (stable, MSVC), Node 20 or newer and PowerShell.

`build_installer.bat` in `rustapp` (double-click it, or `build_installer.ps1` from a terminal) does all of the steps below and
prints where the installer is. It downloads ffmpeg only when it is not there yet, and installs the npm packages only when they
are missing. By hand, run these in `rustapp`:

1. `powershell -File tools\fetch_ffmpeg.ps1` downloads the ffmpeg build the installer carries and puts it in
   `app\src-tauri\binaries` with the license files in `app\src-tauri\licenses`. Both folders are ignored by git.
   Use `-Flavor lgpl` only to test: the LGPL build has no libx264, so rendering would be refused (the Setup screen says so).
2. `cd app`, `npm install`, `npm run release:local`.
3. The installer is `target\release\bundle\nsis\VRGDG H3 Studio_<version>_x64-setup.exe`.

`npm run release:local` uses `src-tauri\tauri.release.conf.json`, which adds the ffmpeg sidecar, the license files and the
NSIS settings (per-user install, no administrator rights), and `tauri.local.conf.json`, which leaves out the signed update
files because they need the private update key. `npm run release` is the build for a published release and needs that key
(see Updates). A normal `cargo build`, `cargo test` and `npm run tauri dev` do not need the ffmpeg files.

The installer is about 100 MB because the static ffmpeg and ffprobe are 157 MB each before compression.

## Build the packages (Linux)

`bash build_installer.sh` builds a `.deb` and an AppImage with `npm run release:linux:local`. `docs/BUILDING_LINUX.md` has the requirements, what differs from Windows and how to fix a blank window. The packages are in `target/release/bundle/deb` and `target/release/bundle/appimage`. ffmpeg is not bundled on Linux: the `.deb` depends on the distribution's ffmpeg and the AppImage uses the one on PATH. `tauri.linux.conf.json` is merged over `tauri.release.conf.json` for that build (package types, no ffmpeg sidecar, the `.deb` dependencies).

## What the app does on first start

- It has no config file yet, so it opens on the Setup screen.
- The Setup screen checks ComfyUI (address, version), the H3 node packs (named by the pack that provides them),
  ComfyUI's output folder, the project folder, ffmpeg and ffprobe (bundled one first, then PATH), the libx264 encoder,
  and the LLM (a warning only, rendering does not need it).
- Saving writes `%APPDATA%\VRGDG-H3\config.json` and restarts the app.

The app must run on the same computer as ComfyUI, because scene files are exchanged by path.

## Release checklist

Done: the update channel (Updates below), the license (`LICENSE` is the pack's license, copied, and a copy ships in the
installer's `licenses` folder), the ffmpeg source contact (the repository's issues page), the history (kept as it is, with the
maintainer's address), and a scan of the files and the history for keys and personal paths. Still open: code signing and a
clean machine test.

1. **Code signing.** An unsigned installer shows a SmartScreen warning. Buy or obtain a code signing certificate, then add
   to `tauri.release.conf.json` under `bundle.windows`: `"certificateThumbprint"`, `"digestAlgorithm": "sha256"` and
   `"timestampUrl"`, or use `"signCommand"` for a cloud signing service. Sign the sidecar programs too (ffmpeg.exe and
   ffprobe.exe), or leave them as they come.
2. **Update channel.** Built, see Updates below. It needs the two repository secrets named there.
3. **Secrets.** No API key is stored in the repo. Check with `git grep -i "sk-"` before each push of new files.
4. **A clean machine test.** Install on a Windows account that has no Rust, Node or ffmpeg, start ComfyUI with the node packs,
   and render one scene.

## Updates

The installed app looks for a newer version a few seconds after it starts (Menu, Check for Updates does it on demand, and the
dialog has a switch for the start check). It reads `latest.json` from the latest release of
`https://github.com/nvmax/VideoBuilderH3`, and when the version there is newer it offers **Install and restart**. The offer
shows the release notes. The app downloads the installer, checks its signature against the public key in
`app\src-tauri\tauri.conf.json` (`plugins.updater.pubkey`), runs it, and starts again. Nothing is installed without that
click, while a job runs or waits, or when the signature does not match. The check sends no information about the user.

### The signing key

`tauri signer generate` made one key pair. The public key is in the config. The private key and its password are in
`%USERPROFILE%\.tauri\h3-studio-updater.key` and `.key.password` on the maker's computer, are never in the repository, and
are the two repository secrets `TAURI_SIGNING_PRIVATE_KEY` (the content of the `.key` file) and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (Settings, Secrets and variables, Actions). Keep a copy of both files somewhere safe.
**If the key is lost, installed apps cannot be updated** (they only accept updates signed with it) and everyone has to install
again by hand. A leaked key lets anyone publish an update, so replace the key and the public key in the config at once.
This key is not the Windows code signing certificate: it does not stop the SmartScreen warning.

### Making a release

1. `powershell -File tools\set_version.ps1 -Version 0.1.1` sets the version in the three files that hold it.
2. Optional: write the notes in `docs\release_notes\0.1.1.md`. Without that file the notes are the first 40 commit subjects
   since the last tag, or only the version for the first release.
3. Commit, then `git tag -a v0.1.1 -m "H3 Studio 0.1.1"` and `git push origin main v0.1.1`.
4. `.github\workflows\release.yml` builds the installer on a Windows runner with `npm run release`, signs it, and
   `tools\publish_release.ps1` publishes the release with the installer, its signature and `latest.json`. A draft is
   published last, so an app never sees a release without `latest.json`. The script refuses a tag that does not match the
   three version files.

`tools\publish_release.ps1 -Tag v0.1.1 -DryRun` makes the same files in a temp folder without touching GitHub.

### The Linux packages

The same tag push also starts the `linux` job (Ubuntu 22.04, `npm run release:linux`, signed with the same key). Once the Windows job has published the release, the `linux_publish` job runs `tools/publish_linux_assets.py`. It uploads `H3-Studio_<version>_amd64.AppImage` and `H3-Studio_<version>_amd64.deb` with their signatures and adds `linux-x86_64-appimage`, `linux-x86_64-deb` and `linux-x86_64` to the release's `latest.json`. A Linux build that fails leaves the Windows release as it is, and running the job again replaces the Linux files. An AppImage updates itself like the Windows app. A `.deb` update is installed with `dpkg` after the system asks for the password.

`python3 tools/publish_linux_assets.py --tag v0.1.1 --bundle target/release/bundle --latest latest.json --dry-run` makes the files without touching GitHub (`--latest` is a `latest.json` of that version to add the entries to).

Each update downloads the whole installer (about 100 MB, most of it ffmpeg). A debug build can look at another address:
set `H3_UPDATE_ENDPOINT` to the address of a `latest.json`. A release build ignores it.
