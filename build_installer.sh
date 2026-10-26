#!/usr/bin/env bash
# Build VRGDG H3 Studio on Linux: a .deb package and an AppImage.
#
# Checks Rust, Node, npm and the system libraries the window needs, installs the npm packages when they are missing, then runs
# `npm run release:linux:local`. The packages are written to target/release/bundle/deb and target/release/bundle/appimage and their
# paths are printed at the end. ffmpeg is not bundled on Linux: the .deb depends on the distribution's ffmpeg, and the AppImage uses
# the ffmpeg on PATH (the Setup screen checks it and says what is missing).
#
# Run it from anywhere:  bash build_installer.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
app="$root/app"

step() { echo "[VRGDG-RS Build] $*"; }
fail() { echo "[VRGDG-RS Build] $*" >&2; exit 1; }

# A shell that was opened before Rust was installed does not have it on its path yet.
if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi

command -v cargo >/dev/null || fail "cargo was not found. Install Rust (stable) from https://rustup.rs."
command -v node >/dev/null || fail "node was not found. Install Node 20 or newer from https://nodejs.org."
command -v npm >/dev/null || fail "npm was not found. Install Node 20 or newer from https://nodejs.org."
node_major="$(node --version | sed 's/^v//; s/\..*//')"
[ "$node_major" -ge 20 ] || fail "Node $node_major is too old, the build needs Node 20 or newer."
command -v pkg-config >/dev/null || fail "pkg-config was not found. Install it with the libraries listed in docs/BUILDING_LINUX.md."

# The window is WebKitGTK. These are the development packages the build links against.
missing=()
for lib in webkit2gtk-4.1 gtk+-3.0 librsvg-2.0; do
  pkg-config --exists "$lib" || missing+=("$lib")
done
for tool in patchelf file curl; do
  command -v "$tool" >/dev/null || missing+=("$tool")
done
if [ "${#missing[@]}" -gt 0 ]; then
  echo "[VRGDG-RS Build] Missing: ${missing[*]}" >&2
  echo "Install the build packages first:" >&2
  echo "  Debian, Ubuntu:  sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev patchelf file curl" >&2
  echo "  Fedora:          sudo dnf install gcc pkgconf-pkg-config webkit2gtk4.1-devel gtk3-devel librsvg2-devel patchelf file curl" >&2
  echo "  Arch:            sudo pacman -S base-devel webkit2gtk-4.1 gtk3 librsvg patchelf file curl" >&2
  exit 1
fi

command -v ffmpeg >/dev/null || step "Note: ffmpeg is not installed here. The app needs it to run (sudo apt install ffmpeg), the build does not."

# linuxdeploy (the AppImage tool) strips the libraries it copies with its own, older strip, which fails on programs built by a new
# toolchain. The app is already stripped by the release profile, so skipping it only leaves the copied libraries as they are.
export NO_STRIP=true

cd "$app"
if [ ! -x node_modules/.bin/tauri ]; then
  step "Installing the npm packages."
  npm install
fi
step "Building the packages. The first build takes several minutes."
started="$(date +%s)"
npm run release:linux:local

echo
found=0
for package in "$root"/target/release/bundle/deb/*.deb "$root"/target/release/bundle/appimage/*.AppImage; do
  [ -f "$package" ] || continue
  [ "$(stat -c %Y "$package")" -ge "$started" ] || continue
  size="$(du -h "$package" | cut -f1)"
  step "Built $package ($size)"
  found=1
done
[ "$found" -eq 1 ] || fail "The build finished but there is no new package in target/release/bundle."
step "Install the .deb with: sudo apt install ./<file>.deb, or make the AppImage executable and run it."
