# SPDX-License-Identifier: LGPL-2.1-or-later
import importlib.util
import json
import contextlib
import io
from pathlib import Path
import sys
import tempfile
import unittest
import zipfile
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("addon_build", ROOT / "scripts/build.py")
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)
package_spec = importlib.util.spec_from_file_location("addon_package", ROOT / "scripts/package.py")
package = importlib.util.module_from_spec(package_spec)
package_spec.loader.exec_module(package)


class DistributionTests(unittest.TestCase):
    def test_supplemental_archives_keep_platform_names_and_source_revision(self):
        for system in ("windows", "linux"):
            with self.subTest(system=system), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                addon = root / "addons/game_stream"
                platform = addon / "bin" / system / "x86_64"
                platform.mkdir(parents=True)
                (platform / "game_stream.bin").write_bytes(b"extension fixture")
                (platform / "avcodec.bin").write_bytes(b"codec fixture")
                (platform / "platform.json").write_text(json.dumps({
                    "platform": system, "arch": "x86_64", "library": "game_stream.bin",
                    "dependencies": ["avcodec.bin"]}))
                for name in ("game_stream.gdextension", "README.md", "LICENSE", "COPYING.GPLv2"):
                    (addon / name).write_text("fixture\n")
                vendor = root / "build/rust-vendor/godot"
                vendor.mkdir(parents=True)
                (vendor / "Cargo.toml").write_text('[package]\nname="godot"\nversion="0.5.5"\nlicense="MIT"\n')
                (vendor / "LICENSE").write_text("license fixture\n")
                source = root / "ffmpeg"
                source.mkdir()
                (source / "COPYING.LGPLv2.1").write_text("license fixture\n")
                sdk_sources = root / "build/desktop-sdk/sources"
                sdk_sources.mkdir(parents=True)
                (sdk_sources / "build-info.json").write_text("{}\n")
                (root / "docs/releases").mkdir(parents=True)
                for name in ("protocol.md", "building.md", "client-rebuilding.md", "desktop-releases.md", "releases/0.1.0.md"):
                    (root / "docs" / name).write_text("documentation fixture\n")
                def output(command, **kwargs):
                    if command[:2] == ["git", "status"]:
                        return ""
                    if command[:2] == ["git", "rev-parse"]:
                        return "fixture-revision\n"
                    if command[:2] == ["git", "ls-files"]:
                        return b"addons/game_stream/README.md\0"
                    return "fixture-tool-version\n"
                with patch.object(package, "ROOT", root), patch.object(package.subprocess, "check_output", side_effect=output), \
                     patch.object(sys, "argv", ["package.py", "--ffmpeg-source", str(source), "--supplemental"]), \
                     contextlib.redirect_stdout(io.StringIO()):
                    package.main()
                target = f"{system}-x86_64"
                with zipfile.ZipFile(root / f"dist/godot-game-stream-0.1.0-{target}.zip") as bundle:
                    self.assertIn(f"addons/game_stream/bin/{system}/x86_64/game_stream.bin", bundle.namelist())
                    self.assertIn("addons/game_stream/THIRD_PARTY/rust/godot/LICENSE", bundle.namelist())
                record = json.loads((root / f"dist/release-build-{target}.json").read_text())
                self.assertEqual(record["source_commit"], "fixture-revision")
                self.assertIn(f"godot-game-stream-0.1.0-{target}-source.zip", record["asset_sha256"])

    def test_manifest_preserves_multiple_platforms_and_export_dependencies(self):
        with tempfile.TemporaryDirectory() as temp:
            addon = Path(temp)
            for system, arch, name, dep in (
                ("macos", "arm64", "libgame_stream.dylib", "libavcodec.63.dylib"),
                ("windows", "x86_64", "game_stream.dll", "avcodec-63.dll"),
            ):
                folder = addon / "bin" / system / arch
                folder.mkdir(parents=True)
                (folder / "platform.json").write_text(json.dumps({
                    "platform": system, "arch": arch, "library": name, "dependencies": [dep]
                }))
            with patch.object(build, "ADDON", addon):
                build.write_manifest()
            manifest = (addon / "game_stream.gdextension").read_text()
            self.assertIn('macos.arm64 = "./bin/macos/arm64/libgame_stream.dylib"', manifest)
            self.assertIn('windows.x86_64 = "./bin/windows/x86_64/game_stream.dll"', manifest)
            self.assertIn('"./bin/macos/arm64/libavcodec.63.dylib": "Contents/Frameworks"', manifest)
            self.assertIn('"./bin/windows/x86_64/avcodec-63.dll": ""', manifest)

    def test_incomplete_sdk_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            sdk = Path(temp)
            (sdk / "lib").mkdir()
            (sdk / "lib/libavcodec.63.dylib").touch()
            with self.assertRaisesRegex(ValueError, "Missing avutil"):
                build.sdk_libraries(sdk, "macos")

    def test_shared_protocol_fixtures_have_fixed_wire_header_and_size(self):
        for name, length in (("key-a.bin", 10), ("mouse-left.bin", 13), ("mouse-motion.bin", 17)):
            data = (ROOT / "protocol/fixtures" / name).read_bytes()
            self.assertEqual(data[:4], b"1ISG")
            self.assertEqual(int.from_bytes(data[4:6], "little"), length)
            self.assertEqual(len(data), 6 + length)


if __name__ == "__main__":
    unittest.main()
