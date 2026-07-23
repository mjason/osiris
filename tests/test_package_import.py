import os
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class PackageImportTests(unittest.TestCase):
    def test_python_runtime_is_independent_from_the_native_cli(self):
        script = r"""
import osiris
from osiris import prelude
assert isinstance(osiris.version(), str)
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
