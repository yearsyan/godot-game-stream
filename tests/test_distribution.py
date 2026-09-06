# SPDX-License-Identifier: LGPL-2.1-or-later
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("addon_build", ROOT / "scripts/build.py")
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


class DistributionTests(unittest.TestCase):
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
