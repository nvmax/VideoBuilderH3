#!/usr/bin/env python3
"""Add the Linux packages to a release that is already published.

Run by .github/workflows/release.yml after the Windows job has published the release (tools/publish_release.ps1). It names the
AppImage and the .deb without spaces (GitHub renames a file with spaces, which would break the address in latest.json), uploads
them with their signatures, adds the Linux entries to the release's latest.json (the file installed apps read to find an update)
and uploads that file again. The Windows entry stays as it is. The packages go up first, so an app never reads an entry whose
file is not there yet.

  python3 tools/publish_linux_assets.py --tag v0.1.6 --bundle target/release/bundle

--dry-run makes the files in --out and does not touch GitHub. It starts from the file given with --latest (a latest.json of an
earlier release or a test file) instead of downloading the release's. Otherwise it needs `gh` and GH_TOKEN (or a gh login).
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def step(message: str) -> None:
    print(f"[VRGDG-RS Release] {message}")


def workspace_version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'(?m)^version = "([^"]*)"', text)
    if not match:
        sys.exit("No version found in Cargo.toml.")
    return match.group(1)


def find_package(bundle: Path, folder: str, suffix: str, version: str) -> Path | None:
    """The package of this version in `bundle/folder`, with its signature next to it."""
    found = sorted((bundle / folder).glob(f"*_{version}_amd64{suffix}"))
    return found[0] if found else None


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--tag", required=True, help="the release tag, for example v0.1.6")
    parser.add_argument("--repo", default="nvmax/VideoBuilderH3")
    parser.add_argument("--bundle", required=True, type=Path, help="the folder that holds the deb and appimage folders")
    parser.add_argument("--out", type=Path, help="where the renamed files and latest.json are made (default: a temp folder)")
    parser.add_argument("--latest", type=Path, help="with --dry-run: the latest.json to add the Linux entries to")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    version = args.tag.lstrip("v")
    if workspace_version() != version:
        sys.exit(f"The tag says {version} but Cargo.toml says {workspace_version()}. Tag again after tools\\set_version.ps1.")

    packages = {
        "appimage": ("appimage", ".AppImage", f"H3-Studio_{version}_amd64.AppImage"),
        "deb": ("deb", ".deb", f"H3-Studio_{version}_amd64.deb"),
    }
    out = args.out or Path(tempfile.gettempdir()) / f"h3-release-linux-{version}"
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)

    entries: dict[str, dict[str, str]] = {}
    uploads: list[Path] = []
    for kind, (folder, suffix, name) in packages.items():
        package = find_package(args.bundle, folder, suffix, version)
        if package is None:
            step(f"No {kind} package for {version} in {args.bundle / folder}, skipping it.")
            continue
        signature = package.with_name(package.name + ".sig")
        shutil.copy2(package, out / name)
        uploads.append(out / name)
        if signature.is_file():
            shutil.copy2(signature, out / f"{name}.sig")
            uploads.append(out / f"{name}.sig")
            entries[kind] = {
                "signature": signature.read_text(encoding="utf-8").strip(),
                "url": f"https://github.com/{args.repo}/releases/download/{args.tag}/{name}",
            }
        else:
            step(f"The {kind} package has no signature file, so it is uploaded but not offered as an update.")
    if "appimage" not in entries and "deb" not in entries:
        sys.exit("There is no signed Linux package to offer. The build needs TAURI_SIGNING_PRIVATE_KEY.")

    if args.dry_run:
        if not args.latest:
            sys.exit("--dry-run needs --latest, the latest.json to add the entries to.")
        manifest = json.loads(args.latest.read_text(encoding="utf-8"))
    else:
        download = Path(tempfile.mkdtemp(prefix="h3-latest-"))
        subprocess.run(["gh", "release", "download", args.tag, "--repo", args.repo, "--pattern", "latest.json", "--dir", str(download)], check=True)
        manifest = json.loads((download / "latest.json").read_text(encoding="utf-8"))
    if manifest.get("version") != version:
        sys.exit(f"latest.json is for version {manifest.get('version')}, not {version}.")

    platforms = manifest.setdefault("platforms", {})
    # The updater looks for `linux-x86_64-<package type>` first and then for `linux-x86_64`. An AppImage updates itself and a .deb is
    # installed with the system's package tool (it asks for the password). The plain entry covers a copy that cannot tell which it is.
    if "appimage" in entries:
        platforms["linux-x86_64-appimage"] = entries["appimage"]
        platforms["linux-x86_64"] = entries["appimage"]
    if "deb" in entries:
        platforms["linux-x86_64-deb"] = entries["deb"]
    latest = out / "latest.json"
    latest.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    step(f"Made the Linux release files for {args.tag} in {out}")
    if args.dry_run:
        return

    subprocess.run(["gh", "release", "upload", args.tag, *map(str, uploads), "--repo", args.repo, "--clobber"], check=True)
    subprocess.run(["gh", "release", "upload", args.tag, str(latest), "--repo", args.repo, "--clobber"], check=True)
    step(f"Added the Linux packages to https://github.com/{args.repo}/releases/tag/{args.tag}")


if __name__ == "__main__":
    main()
