#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Collect the macOS client and its exact Conan dependency materials."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
CLIENT = ROOT / "clients/mirctl"


def package_client(archive):
    graph_path = CLIENT / "build/conan-graph.json"
    graph = json.loads(graph_path.read_text())["graph"]
    nodes = list(graph["nodes"].values())
    settings = nodes[0]["settings"]
    if settings.get("os") != "Macos" or settings.get("arch") != "armv8":
        raise ValueError("The v0.1.0 client package targets macOS arm64 only")
    executable = CLIENT / "dist/mirctl"
    version = subprocess.check_output([str(executable), "--version"], text=True)
    if "FFmpeg license: LGPL" not in version:
        raise ValueError("The client must report an LGPL FFmpeg build")

    work = ROOT / "build/client-release"
    if work.exists():
        shutil.rmtree(work)
    binary = work / "mirctl-0.1.0-macos-arm64"
    dependencies = work / "mirctl-dependencies"
    binary.mkdir(parents=True)
    dependencies.mkdir()
    shutil.copy2(executable, binary / "mirctl")
    for name in ("LICENSE", "COPYING.GPLv2", "THIRD_PARTY.md"):
        shutil.copy2(CLIENT / name, binary / name)
    shutil.copy2(ROOT / "docs/client-installation.md", binary / "README.md")
    shutil.copy2(ROOT / "docs/releases/0.1.0.md", binary / "release-notes.md")
    shutil.copy2(ROOT / "docs/client-rebuilding.md", dependencies / "README.md")
    shutil.copy2(CLIENT / "build/conan.lock", dependencies / "conan.lock")

    records = []
    seen = set()
    for node in nodes:
        ref = node.get("ref")
        if not ref or ref == "conanfile" or ref in seen:
            continue
        seen.add(ref)
        name = node["name"]
        recipe = Path(node["recipe_folder"])
        shutil.copytree(recipe, dependencies / "recipes" / name,
                        ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
        record = {key: node.get(key) for key in (
            "ref", "context", "package_id", "prev", "license", "settings", "options", "conandata")}
        records.append(record)
        if node["context"] != "host" or name == "opengl":
            continue
        if name not in {"ffmpeg", "sdl", "dav1d"}:
            raise ValueError(f"Unreviewed client runtime dependency: {ref}")
        source = node["conandata"]["sources"][node["version"]]
        url = source["url"][0] if isinstance(source["url"], list) else source["url"]
        download = ROOT / "build/downloads" / Path(urlparse(url).path).name
        download.parent.mkdir(parents=True, exist_ok=True)
        if not download.exists():
            subprocess.run(["curl", "--fail", "--location", "--retry", "2", url,
                            "--output", str(download)], check=True)
        if hashlib.sha256(download.read_bytes()).hexdigest() != source["sha256"]:
            raise ValueError(f"Source checksum mismatch: {download.name}")
        sources = dependencies / "sources"
        sources.mkdir(exist_ok=True)
        shutil.copy2(download, sources / download.name)
        package = Path(node["package_folder"])
        licenses = package / "licenses"
        if not licenses.is_dir() or not any(licenses.iterdir()):
            raise ValueError(f"Missing dependency license files: {ref}")
        shutil.copytree(licenses, binary / "THIRD_PARTY" / name)
        shutil.copytree(licenses, dependencies / "licenses" / name)
        sdk = dependencies / "static-libraries" / name
        shutil.copytree(package / "include", sdk / "include")
        (sdk / "lib").mkdir()
        for library in (package / "lib").glob("*.a"):
            shutil.copy2(library, sdk / "lib" / library.name)

    profile = "[settings]\n" + "".join(f"{key}={value}\n" for key, value in settings.items())
    (dependencies / "macos-arm64.profile").write_text(profile)
    (dependencies / "dependency-build.json").write_text(json.dumps(records, indent=2) + "\n")
    # Consumers can relink the original application objects with replacement libraries.
    objects = CLIENT / "build/app/Release/CMakeFiles"
    for target in ("mirctl.dir", "mirctl_core.dir"):
        for obj in (objects / target).rglob("*.o"):
            dest = dependencies / "objects" / obj.relative_to(objects)
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(obj, dest)
    link_file = objects / "mirctl.dir/link.txt"
    link = link_file.read_text()
    for node in nodes:
        if node.get("package_folder"):
            link = link.replace(node["package_folder"], f'$DEPENDENCIES/static-libraries/{node["name"]}')
    link = link.replace(str(CLIENT), "$CLIENT_SOURCE")
    (dependencies / "original-link-command.txt").write_text(link)
    shutil.copy2(CLIENT / "build/app/Release/libmirctl_core.a", dependencies / "objects/libmirctl_core.a")
    (binary / "build-info.txt").write_text(version)
    archive(ROOT / "dist/mirctl-0.1.0-macos-arm64.zip", binary,
            (p for p in binary.rglob("*") if p.is_file()))
    archive(ROOT / "dist/mirctl-0.1.0-dependencies.zip", work,
            (p for p in dependencies.rglob("*") if p.is_file()))
