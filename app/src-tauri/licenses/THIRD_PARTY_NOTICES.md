# Third party notices

## VRGDG H3 Studio

VRGDG H3 Studio is a Rust port and desktop app of the Video Builder of the comfyui-vrgamedevgirl ComfyUI node pack
(https://github.com/vrgamegirl19/comfyui-vrgamedevgirl), copyright (c) 2026 Jean Thompson (VRGameDevGirl). It is released under
the same license, the VRGameDevGirl Community and Commercial License, Version 1.0, in `LICENSE-VRGameDevGirl.txt` next to this
file. Its Video Builder screens are the pack's own, copied and changed as listed in `tools/builder_ui_patches.json` of the
source repository (https://github.com/nvmax/VideoBuilderH3), which also lists every other file copied or ported from the pack
(`COPIED_FROM.md`).

VRGDG H3 Studio includes or runs the following software.

## FFmpeg (separate program)

The Windows installer includes `ffmpeg.exe` and `ffprobe.exe` from the BtbN FFmpeg-Builds project (GPL build, which includes the
libx264 H.264 encoder). The Linux packages do not include ffmpeg: they use the one installed on the computer. They are
separate programs. VRGDG H3 Studio starts them as processes to cut, join and encode
video and does not link to them.

FFmpeg is licensed under the GNU General Public License version 2 or later for this build. The license text is in
`ffmpeg-gpl-LICENSE.txt` next to this file and the exact build is named in `ffmpeg-VERSION.txt`.

Source code: the source of this build is available from https://github.com/BtbN/FFmpeg-Builds (the build scripts and
the pinned FFmpeg and library versions) and from https://ffmpeg.org/download.html. If you want a copy of the corresponding
source on another medium, open an issue at https://github.com/nvmax/VideoBuilderH3/issues. This offer is valid for
three years after you receive the program.

You can replace the bundled programs with your own ffmpeg in the Setup screen (a build that includes libx264 is needed).

## ComfyUI and its node packs

VRGDG H3 Studio talks to a ComfyUI that you run yourself. ComfyUI and the node packs it needs (comfyui-vrgamedevgirl,
comfyui-kjnodes, ComfyUI-VideoHelperSuite and others listed on the Setup screen) are not included and keep their own
licenses.

## Application libraries

The application is built with Tauri (MIT or Apache-2.0), SolidJS (MIT), and Rust crates under MIT, Apache-2.0 or similar
licenses, including tokio, axum, reqwest, serde, rusqlite (with bundled SQLite, public domain), image, symphonia and
rustfft. The Microsoft Edge WebView2 runtime is installed by the installer when it is missing, under Microsoft's terms. On Linux
the window is WebKitGTK (LGPL) from the system, and the AppImage carries the GTK libraries it needs under their own licenses.

## Inter font

`app/web/host/fonts/inter-latin-normal.woff2` is the Inter typeface, copied from ComfyUI's frontend package, which ships
it for the same reason: the Video Builder is drawn with it. Copyright 2016 The Inter Project Authors
(https://github.com/rsms/inter). Licensed under the SIL Open Font License, Version 1.1 (https://openfontlicense.org).
