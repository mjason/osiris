import os
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class PackageImportTests(unittest.TestCase):
    def test_prelude_import_does_not_require_native_extension(self):
        script = r"""
import importlib.abc
import sys

class BlockNativeCore(importlib.abc.MetaPathFinder):
    def find_spec(self, fullname, path=None, target=None):
        if fullname == "osiris._core":
            raise ModuleNotFoundError("native core intentionally unavailable")
        return None

sys.meta_path.insert(0, BlockNativeCore())
from osiris import prelude
assert prelude.mapv(lambda value: value + 1, (1, 2)) == (2, 3)
"""
        environment = os.environ.copy()
        environment["PYTHONPATH"] = str(ROOT / "src")
        result = subprocess.run(
            [sys.executable, "-c", script],
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
