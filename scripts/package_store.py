#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Assemble a reproducible desktop addon from checksum-pinned release archives."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import tempfile
import zipfile

from package import archive

ROOT = Path(__file__).resolve().parents[1]
PREFIX = "addons/game_stream/"


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def merge_packages(inputs, cache, destination):
    """Preserve native bytes and notices; replace only shared text and metadata."""
    configurations, libraries, dependencies = [], [], []
    for platform in inputs["platforms"]:
        target = platform["platform"]
        asset = platform["assets"][0]
        path = cache / asset["name"]
        if sha256(path) != asset["sha256"]:
            raise ValueError(f"Release checksum mismatch: {path.name}")
        with zipfile.ZipFile(path) as bundle:
            text = bundle.read(PREFIX + "game_stream.gdextension").decode().replace("\r\n", "\n")
            config, rest = text.split("[libraries]\n", 1)
            library, dependency = rest.split("[dependencies]\n", 1)
            configurations.append(config.strip())
            libraries.append(library.strip())
            dependencies.append(dependency.strip())
            for name in bundle.namelist():
                if not name.startswith(PREFIX) or ".." in PurePosixPath(name).parts:
                    raise ValueError(f"Unexpected archive path: {name}")
                if name.endswith("/"):
                    continue
                relative = name[len(PREFIX):]
                if relative.startswith("bin/"):
                    expected = "bin/" + target.replace("-", "/", 1) + "/"
                    if not relative.startswith(expected):
                        raise ValueError(f"Unexpected native platform: {name}")
                elif relative.startswith("THIRD_PARTY/"):
                    relative = relative.replace("THIRD_PARTY/rust/", f"THIRD_PARTY/rust-{target}/", 1)
                else:
                    # The wrapper must remain compatible with every released binary.
                    if relative.endswith(".gd"):
                        source = (ROOT / PREFIX / relative).read_text(encoding="utf-8")
                        if bundle.read(name).decode().replace("\r\n", "\n") != source:
                            raise ValueError(f"Released wrapper differs from working source: {name}")
                    continue
                output = destination / PREFIX / relative
                data = bundle.read(name)
                if output.exists() and output.read_bytes() != data:
                    raise ValueError(f"Conflicting archive entry: {relative}")
                output.parent.mkdir(parents=True, exist_ok=True)
                output.write_bytes(data)
                mode = (bundle.getinfo(name).external_attr >> 16) & 0o777
                output.chmod(mode or 0o644)
    if len(set(configurations)) != 1:
        raise ValueError("Platform extension configurations differ")
    manifest = configurations[0] + "\n\n[libraries]\n" + "\n".join(libraries)
    manifest += "\n\n[dependencies]\n" + "\n\n".join(dependencies) + "\n"
    for name in re.findall(r'"\./([^"\n]+)"', manifest):
        if not (destination / PREFIX / name).is_file():
            raise ValueError(f"Manifest references a missing file: {name}")
    (destination / PREFIX / "game_stream.gdextension").write_text(manifest, encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, default=ROOT / "build/store-inputs")
    parser.add_argument("--output", type=Path, default=ROOT / "dist/store")
    args = parser.parse_args()
    inputs = json.loads((ROOT / "store/package-inputs.json").read_text())
    args.cache.mkdir(parents=True, exist_ok=True)
    args.output.mkdir(parents=True, exist_ok=True)
    for platform in inputs["platforms"]:
        asset = platform["assets"][0]
        path = args.cache / asset["name"]
        if not path.exists():
            subprocess.run(["curl", "--fail", "--location", "--retry", "3", asset["url"],
                            "--output", str(path)], check=True)
    with tempfile.TemporaryDirectory(prefix="game-stream-store-") as directory:
        stage = Path(directory)
        merge_packages(inputs, args.cache, stage)
        addon = stage / PREFIX
        tracked = subprocess.check_output(["git", "ls-files", "-z", "addons/game_stream"], cwd=ROOT).decode().split("\0")
        for name in tracked:
            if not name or name.endswith(".import"):
                continue
            path = ROOT / name
            output = stage / name
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(path.read_bytes())
        # Keep the small runnable example inside the installed addon directory.
        example = addon / "example"
        example.mkdir()
        for name in ("demo.gd", "demo.tscn"):
            text = (ROOT / "examples" / name).read_text(encoding="utf-8")
            text = text.replace("res://examples/", "res://addons/game_stream/example/")
            (example / name).write_text(text, encoding="utf-8")
        (addon / "distribution.json").write_text(json.dumps(inputs, indent=2) + "\n", encoding="utf-8")
        sources = ["# Corresponding sources", "", "This installation archive combines the unchanged native binaries from the following release builds.",
                   "Shared documentation, the manifest and the bundled example are assembled from the standalone repository.",
                   "Each platform's exact source archives, build records and dependency sources are freely available at the links below.", ""]
        for platform in inputs["platforms"]:
            sources += [f'## {platform["platform"]}', "", f'Source commit: `{platform["source_commit"]}`', ""]
            sources += [f'- [{a["name"]}]({a["url"]})' for a in platform["assets"]]
            sources.append("")
        sources += ["Shared packaging and example source: [repository](https://github.com/yearsyan/godot-game-stream).",
                    "SHA-256 values for all inputs are recorded in `distribution.json`.", ""]
        (addon / "SOURCES.md").write_text("\n".join(sources), encoding="utf-8")
        output = args.output / f'godot-game-stream-{inputs["version"]}-desktop.zip'
        archive(output, stage, (p for p in stage.rglob("*") if p.is_file()))
    checksum = sha256(output)
    (args.output / (output.name + ".sha256")).write_text(f"{checksum}  {output.name}\n")
    print(json.dumps({"path": str(output), "sha256": checksum, "bytes": output.stat().st_size}, indent=2))


if __name__ == "__main__":
    main()
