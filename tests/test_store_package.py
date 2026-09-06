# SPDX-License-Identifier: LGPL-2.1-or-later
import hashlib
import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
spec = importlib.util.spec_from_file_location("store_package", ROOT / "scripts/package_store.py")
store = importlib.util.module_from_spec(spec)
spec.loader.exec_module(store)


class StorePackageTests(unittest.TestCase):
    def fixture(self, folder, missing_dependency=False):
        path = folder / "fixture.zip"
        prefix = "addons/game_stream/"
        manifest = '''[configuration]
entry_symbol = "gdext_rust_init"
compatibility_minimum = "4.6.2"
reloadable = false
[libraries]
windows.x86_64 = "./bin/windows/x86_64/game_stream.dll"
[dependencies]
windows.x86_64 = {"./bin/windows/x86_64/avcodec.dll": ""}
'''
        with zipfile.ZipFile(path, "w") as bundle:
            bundle.writestr(prefix + "game_stream.gdextension", manifest)
            bundle.writestr(prefix + "bin/windows/x86_64/game_stream.dll", b"native fixture")
            if not missing_dependency:
                bundle.writestr(prefix + "bin/windows/x86_64/avcodec.dll", b"dependency fixture")
            bundle.writestr(prefix + "THIRD_PARTY/rust/LICENSE", b"notice fixture")
        return {"platforms": [{"platform": "windows-x86_64", "assets": [{
            "name": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}]}]}

    def test_corrupted_input_is_rejected_before_unpacking(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            inputs = self.fixture(folder)
            with (folder / "fixture.zip").open("ab") as stream:
                stream.write(b"unexpected bytes")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                store.merge_packages(inputs, folder, folder / "output")
            self.assertFalse((folder / "output").exists())

    def test_missing_export_dependency_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            inputs = self.fixture(folder, missing_dependency=True)
            with self.assertRaisesRegex(ValueError, "missing file"):
                store.merge_packages(inputs, folder, folder / "output")

    def test_native_bytes_and_platform_notices_are_preserved(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            inputs = self.fixture(folder)
            store.merge_packages(inputs, folder, folder / "output")
            addon = folder / "output/addons/game_stream"
            self.assertEqual((addon / "bin/windows/x86_64/game_stream.dll").read_bytes(), b"native fixture")
            self.assertEqual((addon / "THIRD_PARTY/rust-windows-x86_64/LICENSE").read_bytes(), b"notice fixture")


if __name__ == "__main__":
    unittest.main()
