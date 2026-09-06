#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Create an addon-only ZIP and a matching first-party source ZIP."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SKIP = {".git", ".godot", ".godot_rust_home", "target", "build", "dist", "__pycache__", ".cargo"}


def archive(output, base, files):
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as bundle:
        for path in sorted(files):
            info = zipfile.ZipInfo(path.relative_to(base).as_posix(), (1980, 1, 1, 0, 0, 0))
            info.create_system = 3
            info.external_attr = (path.stat().st_mode & 0xFFFF) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            with path.open("rb") as source, bundle.open(info, "w") as target:
                shutil.copyfileobj(source, target)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ffmpeg-source", type=Path, required=True,
                        help="Exact source used for every included FFmpeg binary (one SDK version per package)")
    parser.add_argument("--rust-vendor", type=Path, default=ROOT / "build/rust-vendor",
                        help="Dependency sources created by cargo vendor --locked")
    parser.add_argument("--client", action="store_true", help="Include the macOS client and its dependency materials")
    args = parser.parse_args()
    if subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True).strip():
        parser.error("Commit all source changes before packaging a release")
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    source = args.ffmpeg_source.resolve()
    vendor = args.rust_vendor.resolve()
    if not (vendor / "godot/Cargo.toml").is_file():
        parser.error("Run cargo vendor --locked --manifest-path native/game_stream/Cargo.toml build/rust-vendor first")
    if not (source / "COPYING.LGPLv2.1").is_file():
        parser.error("--ffmpeg-source must be a complete FFmpeg source tree")
    addon = ROOT / "addons/game_stream"
    if not (addon / "game_stream.gdextension").is_file():
        parser.error("Build at least one platform with scripts/build.py first")
    versions = list((addon / "bin").glob("*/*/platform.json"))
    if not versions:
        parser.error("No native platform libraries installed")
    for config in versions:
        item = json.loads(config.read_text())
        if (item["platform"], item["arch"]) != ("macos", "arm64"):
            parser.error("The v0.1.0 release package supports only macOS arm64")
        for name in [item["library"]] + item["dependencies"]:
            if not (config.parent / name).is_file():
                parser.error(f"Missing packaged dependency: {name}")
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    (dist / ".gdignore").touch()
    notices = addon / "THIRD_PARTY/rust"
    notices.mkdir(parents=True, exist_ok=True)
    inventory = []
    for manifest in sorted(vendor.glob("*/Cargo.toml")):
        package = tomllib.loads(manifest.read_text())["package"]
        inventory.append(f'{package["name"]} {package["version"]}: {package.get("license", "See source license file")}')
        for pattern in ("LICENSE*", "COPYING*", "NOTICE*"):
            for license_file in manifest.parent.glob(pattern):
                if license_file.is_file():
                    target = notices / manifest.parent.name / license_file.name
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(license_file, target)
    (notices / "INDEX.txt").write_text("\n".join(inventory) + "\n\nMatching source: rust-dependencies.zip in this release.\n")
    archive(dist / "rust-dependencies.zip", vendor.parent,
            (p for p in vendor.rglob("*") if p.is_file()))
    archive(dist / "ffmpeg-source.zip", source,
            (p for p in source.rglob("*") if p.is_file() and not any(part in SKIP for part in p.relative_to(source).parts)))
    for name in ("README.md", "LICENSE", "COPYING.GPLv2"):
        if not (addon / name).is_file():
            parser.error(f"Missing addon document: {name}")
    for doc in ("protocol.md", "building.md", "client-rebuilding.md"):
        shutil.copy2(ROOT / "docs" / doc, addon / doc)
    shutil.copy2(ROOT / "docs/releases/0.1.0.md", addon / "release-notes.md")
    archive(dist / "godot-game-stream-0.1.0-macos-arm64.zip", ROOT,
            (p for p in addon.rglob("*") if p.is_file() and p.suffix not in (".import", ".pyc") and p.name != "platform.json"))
    tracked = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT).decode().split("\0")
    archive(dist / "godot-game-stream-0.1.0-source.zip", ROOT,
            (ROOT / name for name in tracked if name and (ROOT / name).is_file()))
    outputs = [dist / name for name in ("godot-game-stream-0.1.0-macos-arm64.zip",
               "godot-game-stream-0.1.0-source.zip", "ffmpeg-source.zip", "rust-dependencies.zip")]
    if args.client:
        from package_client import package_client
        package_client(archive)
        outputs += [dist / "mirctl-0.1.0-macos-arm64.zip", dist / "mirctl-0.1.0-dependencies.zip"]
    metadata = {
        "version": "0.1.0", "source_commit": revision,
        "target": {"os": "macOS", "minimum_os": "26.0", "arch": "arm64",
                   "godot": "4.6.2", "renderer": "Forward+ / Metal", "codec": "H.264 / TCP"},
        "tools": {name: subprocess.check_output(command, text=True).strip() for name, command in {
            "rustc": ["rustc", "--version"], "pug": ["pug", "--version"],
            "conan": ["conan", "--version"], "xcode": ["xcodebuild", "-version"]}.items()},
        "signing": "ad-hoc; not Developer ID signed or notarized",
        "exported_game_validation": "pending",
        "asset_sha256": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in outputs},
    }
    (dist / "release-build.json").write_text(json.dumps(metadata, indent=2) + "\n")
    outputs.append(dist / "release-build.json")
    outputs.sort()
    (dist / "SHA256SUMS").write_text("".join(f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}\n" for p in outputs))
    print("Publish all ZIPs together with the corresponding build records and third-party notices.")
    for path in outputs:
        print(path)


if __name__ == "__main__":
    main()
