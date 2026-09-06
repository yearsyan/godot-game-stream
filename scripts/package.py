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
    parser.add_argument("--client", action="store_true", help="Include the host client and its dependency materials")
    parser.add_argument("--supplemental", action="store_true", help="Use platform-specific names for additional release builds")
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
    platforms = {(json.loads(p.read_text())["platform"], json.loads(p.read_text())["arch"]) for p in versions}
    if len(platforms) != 1:
        parser.error("Package one platform per archive")
    system, arch = platforms.pop()
    target = f"{system}-{arch}"
    suffix = f"-{target}" if args.supplemental else ""
    for config in versions:
        item = json.loads(config.read_text())
        if not args.supplemental and target != "macos-arm64":
            parser.error("Use --supplemental for additional desktop platforms")
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
        package = tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]
        inventory.append(f'{package["name"]} {package["version"]}: {package.get("license", "See source license file")}')
        for pattern in ("LICENSE*", "COPYING*", "NOTICE*"):
            for license_file in manifest.parent.glob(pattern):
                if license_file.is_file():
                    notice_path = notices / manifest.parent.name / license_file.name
                    notice_path.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(license_file, notice_path)
    rust_name = f"rust{suffix}-dependencies.zip"
    ffmpeg_name = f"ffmpeg{suffix}-source.zip"
    (notices / "INDEX.txt").write_text("\n".join(inventory) + f"\n\nMatching source: {rust_name} in this release.\n")
    archive(dist / rust_name, vendor.parent,
            (p for p in vendor.rglob("*") if p.is_file()))
    archive(dist / ffmpeg_name, source,
            (p for p in source.rglob("*") if p.is_file() and not any(part in SKIP for part in p.relative_to(source).parts)))
    for name in ("README.md", "LICENSE", "COPYING.GPLv2"):
        if not (addon / name).is_file():
            parser.error(f"Missing addon document: {name}")
    for doc in ("protocol.md", "building.md", "client-rebuilding.md", "desktop-releases.md"):
        shutil.copy2(ROOT / "docs" / doc, addon / doc)
    shutil.copy2(ROOT / "docs/releases/0.1.0.md", addon / "release-notes.md")
    addon_name = f"godot-game-stream-0.1.0-{target}.zip"
    source_name = f"godot-game-stream-0.1.0{suffix}-source.zip"
    archive(dist / addon_name, ROOT,
            (p for p in addon.rglob("*") if p.is_file() and p.suffix not in (".import", ".pyc") and p.name != "platform.json"))
    tracked = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT).decode().split("\0")
    archive(dist / source_name, ROOT,
            (ROOT / name for name in tracked if name and (ROOT / name).is_file()))
    outputs = [dist / name for name in (addon_name, source_name, ffmpeg_name, rust_name)]
    if args.supplemental:
        sdk_sources = ROOT / "build/desktop-sdk/sources"
        if not (sdk_sources / "build-info.json").is_file():
            parser.error("Supplemental builds require the desktop SDK source and build records")
        native_archive = dist / f"native{suffix}-dependencies.zip"
        archive(native_archive, sdk_sources, (p for p in sdk_sources.rglob("*") if p.is_file()))
        outputs.append(native_archive)
    if args.client:
        from package_client import package_client
        package_client(archive, target)
        outputs += [dist / f"mirctl-0.1.0-{target}.zip", dist / f"mirctl-0.1.0{suffix}-dependencies.zip"]
    metadata = {
        "version": "0.1.0", "source_commit": revision,
        "target": {"os": "macOS", "minimum_os": "26.0", "arch": "arm64",
                   "godot": "4.6.2", "renderer": "Forward+ / Metal", "codec": "H.264 / TCP"},
        "tools": {name: subprocess.check_output(command, text=True).strip() for name, command in {
            "rustc": ["rustc", "--version"], "pug": ["pug", "--version"],
            "conan": ["conan", "--version"]}.items()},
        "signing": "ad-hoc; not Developer ID signed or notarized",
        "exported_game_validation": "pending",
        "asset_sha256": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in outputs},
    }
    if args.supplemental:
        metadata["target"] = {"os": system, "arch": arch, "godot": "4.6.2", "renderer": "Forward+",
                              "minimum_os": "Windows 10" if system == "windows" else "Ubuntu 22.04 / glibc 2.35",
                              "encoders": "NVENC, AMF, QSV; hardware and drivers required"}
        metadata["signing"] = "unsigned"
        metadata["validation"] = "Native CI build, unit tests, dependency loading; upstream Windows/Linux application testing reported by maintainer."
    else:
        metadata["tools"]["xcode"] = subprocess.check_output(["xcodebuild", "-version"], text=True).strip()
    record = dist / f"release-build{suffix}.json"
    record.write_text(json.dumps(metadata, indent=2) + "\n")
    outputs.append(record)
    outputs.sort()
    (dist / f"SHA256SUMS{suffix}").write_text("".join(f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}\n" for p in outputs))
    print("Publish all ZIPs together with the corresponding build records and third-party notices.")
    for path in outputs:
        print(path)


if __name__ == "__main__":
    main()
