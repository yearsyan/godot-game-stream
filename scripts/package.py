#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Create an addon-only ZIP and a matching first-party source ZIP."""
import argparse
import hashlib
from pathlib import Path
import shutil
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SKIP = {".git", ".godot", "target", "build", "dist", "__pycache__", ".cargo"}


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
    args = parser.parse_args()
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
        import json
        item = json.loads(config.read_text())
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
    for doc in ("protocol.md", "building.md"):
        shutil.copy2(ROOT / "docs" / doc, addon / doc)
    archive(dist / "godot-game-stream-0.1.0.zip", ROOT,
            (p for p in addon.rglob("*") if p.is_file() and p.suffix not in (".import", ".pyc") and p.name != "platform.json"))
    archive(dist / "godot-game-stream-0.1.0-source.zip", ROOT,
            (p for p in ROOT.rglob("*") if p.is_file() and not any(part in SKIP for part in p.relative_to(ROOT).parts)
             and "bin" not in p.relative_to(ROOT).parts and p.suffix != ".import"))
    outputs = sorted(dist.glob("*.zip"))
    (dist / "SHA256SUMS").write_text("".join(f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}\n" for p in outputs))
    print("Publish all ZIPs together with the corresponding build records and third-party notices.")
    for path in outputs:
        print(path)


if __name__ == "__main__":
    main()
