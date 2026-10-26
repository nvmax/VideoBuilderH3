# VRGDG H3 Studio on Linux

The app runs on Linux with the same Builder screens, API and MCP server as on Windows. A build makes a `.deb` package and an
AppImage. Releases on GitHub carry both (`H3-Studio_<version>_amd64.deb` and `H3-Studio_<version>_amd64.AppImage`).

ComfyUI must run on the same computer, as on Windows, because scene files are exchanged by path.

## Run it

- **AppImage:** `chmod +x H3-Studio_<version>_amd64.AppImage`, then run it. Without FUSE (`libfuse2` on Ubuntu 22.04 and newer) start it
  with `--appimage-extract-and-run`.
- **.deb:** `sudo apt install ./H3-Studio_<version>_amd64.deb`. It installs ffmpeg with it and recommends `zenity`.

The app needs these programs on the computer, because the Linux packages do not carry them:

| Program | For | Install (Debian, Ubuntu) |
|---|---|---|
| `ffmpeg` and `ffprobe`, with libx264 | cutting, joining and encoding video | `sudo apt install ffmpeg` |
| `zenity` or `kdialog` | the Browse buttons (file and folder dialogs). Without one, type the path | `sudo apt install zenity` |
| GStreamer plugins | playing H.264 video in the Builder | `sudo apt install gstreamer1.0-libav gstreamer1.0-plugins-good gstreamer1.0-plugins-bad` |

The first start opens the Setup screen. It checks ComfyUI, the node packs, your folders and ffmpeg, and says what to fix. The
config is `~/.config/VRGDG-H3/config.json` and the projects folder defaults to `~/Documents/VRGDG-H3 Projects`.

## Build it yourself

Needs Rust stable, Node 20 or newer, and the development packages of the window (WebKitGTK 4.1 and GTK 3):

```bash
# Debian, Ubuntu
sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev patchelf file curl
# Fedora
sudo dnf install gcc pkgconf-pkg-config webkit2gtk4.1-devel gtk3-devel librsvg2-devel patchelf file curl
# Arch
sudo pacman -S base-devel webkit2gtk-4.1 gtk3 librsvg patchelf file curl

git clone https://github.com/nvmax/VideoBuilderH3.git
cd VideoBuilderH3
bash build_installer.sh
```

`build_installer.sh` checks the tools, installs the npm packages when they are missing, builds, and prints the paths of the packages in
`target/release/bundle/deb` and `target/release/bundle/appimage`. The first build takes several minutes.

For development use `cargo test --workspace`, `cargo run -p h3-cli -- serve` and `cd app; npm install; npm run tauri dev`.
Some tests compare against output of the pack made on Windows and are written with Windows paths, so they only run on Windows.

## What differs from Windows

- **File dialogs** come from `zenity` or `kdialog` (`crates/h3-builder/src/linux_dialog.rs`). Without either, the Browse buttons say so
  and the path can be typed.
- **ffmpeg** is the system's, not a bundled copy.
- **CapCut beat import** only looks in the Windows CapCut folder, so on Linux it reports that no project was found.
- **Updates:** an AppImage replaces itself after you click Install and restart. A `.deb` is installed with `dpkg` through `pkexec`
  (the system asks for the password).
- **Config and data:** `~/.config/VRGDG-H3` instead of `%APPDATA%\VRGDG-H3`.

## Problems

- **The window stays blank or white** (often NVIDIA with Wayland): start the app with `WEBKIT_DISABLE_DMABUF_RENDERER=1`, for example
  `WEBKIT_DISABLE_DMABUF_RENDERER=1 ./H3-Studio_<version>_amd64.AppImage`.
- **A video does not play in the Builder:** install the GStreamer plugins from the table above.
- **The AppImage build fails while stripping** a library: the script already sets `NO_STRIP=true`. Set it too when you run
  `npm run release:linux:local` yourself.
- **The Setup screen says ffmpeg is missing:** install it, or set the two paths in Menu > App Settings.
