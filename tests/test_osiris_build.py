import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))

import osiris_build  # noqa: E402
from osiris_build._interface import _is_supported_python_target  # noqa: E402
from osiris_build._wheel import _compiler_command  # noqa: E402


class OsirisBuildTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="osiris-build-test-")
        self.root = Path(self.temp.name)
        self.previous_cwd = Path.cwd()
        os.chdir(self.root)
        self._write_project()

    def tearDown(self):
        os.chdir(self.previous_cwd)
        self.temp.cleanup()

    def test_compiler_command_accepts_environment_override(self):
        with mock.patch.dict(os.environ, {"OSR_COMMAND": "custom-osr --flag"}, clear=True):
            self.assertEqual(
                _compiler_command(None, mock.sentinel.project),
                ["custom-osr", "--flag"],
            )

    def _write_project(self, target=None):
        if target is None:
            target = "%d.%d" % (sys.version_info.major, sys.version_info.minor)
        (self.root / "src" / "demo").mkdir(parents=True, exist_ok=True)
        (self.root / "src" / "demo" / "__init__.py").write_text("# package\n", encoding="utf-8")
        (self.root / "src" / "demo" / "hello.osr").write_text("(module demo.hello)\n", encoding="utf-8")
        (self.root / "pyproject.toml").write_text(
            """[project]
name = "demo-osiris"
version = "1.2.3"
description = "fixture"
requires-python = ">=3.11"
dependencies = ["NumPy>=2"]
""",
            encoding="utf-8",
        )
        (self.root / "osiris.jsonc").write_text(
            """{{
  // Build configuration remains readable by people and editors.
  "source": ["src"],
  "targetPython": "{target}",
}}
""".format(target=target),
            encoding="utf-8",
        )
        (self.root / "uv.lock").write_text(
            """version = 1
revision = 3
requires-python = ">=3.11"

[[package]]
name = "demo-osiris"
source = { editable = "." }
dependencies = [
  { name = "numpy", version = "2.1.0" },
]

[[package]]
name = "numpy"
version = "2.1.0"
source = { registry = "https://pypi.org/simple" }

""",
            encoding="utf-8",
        )
        compiler = self.root / "fake-osr.py"
        compiler.write_text(
            """#!/usr/bin/env python3
import pathlib
import sys
import hashlib
import json
args = sys.argv[1:]
out = pathlib.Path(args[args.index('--out-dir') + 1])
package = out / 'demo'
package.mkdir()
compile_index = args.index('compile')
sources = [pathlib.Path(value) for value in args[compile_index + 1:args.index('--out-dir')]]
records = []
interface_hashes = []
def digest(value):
    return 'sha256:' + hashlib.sha256(value.encode()).hexdigest()
def quote(value):
    return json.dumps(value, ensure_ascii=False)
for source in sources:
    stem = source.stem
    module = 'demo.' + stem
    semantic_body = digest(module + ':semantic-body')
    tooling_body = digest(module + ':tooling-body')
    interface_body = digest(module + ':interface-body')
    semantic_interface = digest(module + ':semantic-interface')
    tooling_interface = digest(module + ':tooling-interface')
    stable_record_id = digest(module + ':record')
    record_body_hash = digest(module + ':record-body')
    schema_body_hash = digest('demo/schema')
    interface_hashes.append(semantic_interface)
    record = {
        'schema': {
            'binding-id': 'demo/schema',
            'schema-id': 'demo/schema',
            'version': 1,
            'body-hash': schema_body_hash,
        },
        'owner-binding-id': module + '/value',
        'owner-name': 'value',
        'module': module,
        'public': True,
        'stable-record-id': stable_record_id,
        'record-body-hash': record_body_hash,
        'fields': [['id', stem]],
        'index-claims': [],
        'origin': {'module': module, 'span': [0, 1]},
    }
    occurrence = {
        'distribution': 'demo-osiris',
        'version': '1.2.3',
        'interface-member-id': module,
        'semantic-interface-hash': semantic_interface,
        'stable-record-id': stable_record_id,
        'record-body-hash': record_body_hash,
    }
    records.append({'occurrence': occurrence, 'record': record})
    record_form = '''{:schema {:binding-id %s :schema-id %s :version 1 :body-hash %s}
      :owner-binding-id %s :owner-name "value" :module %s :visibility :public
      :stable-record-id %s :record-body-hash %s :fields [["id" [:string %s]]]
      :index-claims [] :origin {:module %s :span [0 1] :macro-origin none}}''' % (
        quote('demo/schema'), quote('demo/schema'), quote(schema_body_hash),
        quote(module + '/value'), quote(module), quote(stable_record_id),
        quote(record_body_hash), quote(stem), quote(module),
    )
    interface = '''(osiris-interface/header {:format "osiris-interface" :format-version 3
  :compiler-abi "osiris-compiler-v0" :language-version "0.1" :language-abi "osiris-language-v1"
  :standard-library-abi 1 :linkable-helper-format 1 :python-target %s})
(osiris-interface/body {:module %s :metadata [] :bindings [] :aliases [] :functions []
  :structs [] :operator-instances [] :macros [] :phase-helpers [] :static-schemas []
  :owned-records [%s]})
(osiris-interface/graph {:group-id %s
  :members [{:module %s :semantic-body %s :tooling-body %s}]
  :internal-edges [] :external-dependencies []
  :semantic-interface-hash %s :tooling-metadata-hash %s})
(osiris-interface/hashes {:interface-body %s :semantic-body %s :tooling-body %s
  :content-integrity %s})
''' % (
        quote('%d.%d' % (sys.version_info.major, sys.version_info.minor)),
        quote(module), record_form, quote(module), quote(module), quote(semantic_body),
        quote(tooling_body), quote(semantic_interface), quote(tooling_interface),
        quote(interface_body), quote(semantic_body), quote(tooling_body),
        quote(digest(module + ':integrity')),
    )
    (package / (stem + '.py')).write_text('value = 42\\n', encoding='utf-8')
    (package / (stem + '.osri')).write_text(interface, encoding='utf-8')
    source_data = source.read_bytes()
    source_map = {
        'version': 3,
        'language_version': '0.1',
        'python_target': '%d.%d' % (sys.version_info.major, sys.version_info.minor),
        'source': str(source),
        'source_hash': 'sha256:' + hashlib.sha256(source_data).hexdigest(),
        'generated': 'demo/' + stem + '.py',
        'trust_policy_hash': digest('trust'),
        'build_hash': digest(module + ':build'),
        'mappings': [],
    }
    (package / (stem + '.py.map')).write_text(
        json.dumps(source_map, ensure_ascii=False), encoding='utf-8'
    )
records.sort(key=lambda item: tuple(
    item['occurrence'][key] for key in (
        'distribution', 'version', 'interface-member-id',
        'semantic-interface-hash', 'stable-record-id', 'record-body-hash',
    )
))
encoded_records = json.dumps(records, ensure_ascii=False, sort_keys=True, separators=(',', ':')).encode()
sidecar = {
    'format-version': 1,
    'interface-semantic-hashes': sorted(set(interface_hashes)),
    'record-identities': [item['occurrence'] for item in records],
    'record-set-hash': 'sha256:' + hashlib.sha256(encoded_records).hexdigest(),
    'records': records,
}
(out / 'demo-osiris.records.json').write_text(
    json.dumps(sidecar, ensure_ascii=False, sort_keys=True, separators=(',', ':')),
    encoding='utf-8',
)
""",
            encoding="utf-8",
        )
        compiler.chmod(0o755)
        self.compiler_path = compiler
        self.compiler = [sys.executable, str(compiler)]

    def _settings(self):
        return {"osr-command": self.compiler}

    def _replace(self, path, old, new):
        value = path.read_text(encoding="utf-8")
        self.assertIn(old, value)
        path.write_text(value.replace(old, new, 1), encoding="utf-8")

    def test_compatible_release_uses_declared_precision(self):
        self.assertTrue(osiris_build._satisfies("~=1.4.5", "1.4.5"))
        self.assertTrue(osiris_build._satisfies("~=1.4.5", "1.4.99"))
        self.assertFalse(osiris_build._satisfies("~=1.4.5", "1.5.0"))
        self.assertTrue(osiris_build._satisfies("~=1.4", "1.99"))
        self.assertFalse(osiris_build._satisfies("~=1.4", "2.0"))
        with self.assertRaises(osiris_build.BackendError):
            osiris_build._satisfies("~=1", "1.0")

    def test_non_numeric_locked_version_is_not_silently_truncated(self):
        self._replace(
            self.root / "uv.lock",
            'name = "numpy"\nversion = "2.1.0"',
            'name = "numpy"\nversion = "2.1garbage"',
        )
        with self.assertRaises(osiris_build.BackendError) as context:
            osiris_build.get_requires_for_build_wheel()
        self.assertIn("unsupported version", str(context.exception))

    def test_platform_marker_uses_the_actual_target_platform(self):
        target = (sys.version_info.major, sys.version_info.minor)
        with mock.patch.object(osiris_build.platform, "system", return_value="Darwin"):
            self.assertTrue(
                osiris_build._marker_applies('platform_system == "Darwin"', target)
            )
            self.assertFalse(
                osiris_build._marker_applies('platform_system == "Linux"', target)
            )
        with mock.patch.object(osiris_build.sys, "platform", "linux"):
            self.assertTrue(
                osiris_build._marker_applies('sys_platform in "xxlinuxxx"', target)
            )

    def test_lock_root_markers_are_filtered_for_the_build_platform(self):
        self._replace(
            self.root / "pyproject.toml",
            'dependencies = ["NumPy>=2"]',
            'dependencies = ["NumPy>=2; platform_system == \'Windows\'"]',
        )
        self._replace(
            self.root / "uv.lock",
            '{ name = "numpy", version = "2.1.0" }',
            '{ name = "numpy", version = "2.1.0", marker = "platform_system == \'Windows\'" }',
        )
        with mock.patch.object(osiris_build.platform, "system", return_value="Linux"):
            self.assertEqual(osiris_build.get_requires_for_build_wheel(), [])

    def test_projects_dependencies_are_projected_to_exact_lock_versions(self):
        self.assertEqual(
            osiris_build.get_requires_for_build_wheel(),
            ["NumPy==2.1.0"],
        )
        self.assertEqual(
            osiris_build.get_requires_for_build_sdist(),
            ["NumPy==2.1.0"],
        )

    def test_source_requires_canonical_array_form(self):
        self._replace(self.root / "osiris.jsonc", '"source": ["src"]', '"source": "src"')
        with self.assertRaises(osiris_build.BackendError) as context:
            osiris_build.get_requires_for_build_wheel()
        self.assertIn("source must be a non-empty array", str(context.exception))

    def test_removed_configuration_fields_fail_closed(self):
        for field in ("watch", "emit", "extensions", "buildGroups", "trust"):
            self._write_project()
            self._replace(
                self.root / "osiris.jsonc",
                '"source": ["src"],',
                '"source": ["src"],\n  "%s": {},' % field,
            )
            with self.assertRaises(osiris_build.BackendError) as context:
                osiris_build.get_requires_for_build_wheel()
            self.assertIn("unknown osiris.jsonc field `%s`" % field, str(context.exception))

    def test_non_strict_configuration_ignores_future_fields(self):
        self._replace(
            self.root / "osiris.jsonc",
            '"source": ["src"],',
            '"source": ["src"],\n  "strict": false,\n  "futureOption": {"enabled": true},',
        )
        self.assertEqual(osiris_build.get_requires_for_build_wheel(), ["NumPy==2.1.0"])

    def test_source_roots_reject_duplicates_and_output_owned_paths(self):
        for replacement, expected in (
            (
                '"source": ["src", "src"],',
                "duplicate normalized source root",
            ),
            (
                '"source": ["dist/source"],\n  "outDir": "dist",',
                "must not be inside output directory",
            ),
        ):
            self._write_project()
            self._replace(
                self.root / "osiris.jsonc",
                '"source": ["src"],',
                replacement,
            )
            with self.subTest(replacement=replacement):
                with self.assertRaises(osiris_build.BackendError) as context:
                    osiris_build.get_requires_for_build_wheel()
                self.assertIn(expected, str(context.exception))

    def test_duplicate_future_configuration_fields_are_rejected(self):
        self._replace(
            self.root / "osiris.jsonc",
            '"source": ["src"],',
            '"source": ["src"],\n  "strict": false,\n  "futureOption": 1,\n  "futureOption": 2,',
        )
        with self.assertRaises(osiris_build.BackendError) as context:
            osiris_build.get_requires_for_build_wheel()
        self.assertIn("duplicate JSONC field `futureOption`", str(context.exception))

    def test_display_locale_accepts_bcp47(self):
        self._replace(
            self.root / "osiris.jsonc",
            '"targetPython": "',
            '"displayLocale": "zh",\n  "targetPython": "',
        )
        self.assertEqual(osiris_build.get_requires_for_build_wheel(), ["NumPy==2.1.0"])

    def test_target_python_mismatch_fails_closed(self):
        current = "%d.%d" % (sys.version_info.major, sys.version_info.minor)
        self._write_project(target="3.11" if current != "3.11" else "3.12")
        with self.assertRaises(osiris_build.BackendError) as context:
            osiris_build.prepare_metadata_for_build_wheel(str(self.root / "meta"))
        self.assertIn("does not match target-python", str(context.exception))

    def test_interface_target_floor_keeps_future_major_versions_open(self):
        self.assertFalse(_is_supported_python_target("3.10"))
        self.assertTrue(_is_supported_python_target("3.11"))
        self.assertTrue(_is_supported_python_target("4.0"))
        self.assertFalse(_is_supported_python_target("3.11.1"))

    def test_wheel_contains_compiler_outputs_marker_and_record(self):
        wheel_dir = self.root / "wheel"
        filename = osiris_build.build_wheel(str(wheel_dir), self._settings())
        self.assertEqual(filename, "demo_osiris-1.2.3-py3-none-any.whl")
        wheel_bytes = (wheel_dir / filename).read_bytes()
        with zipfile.ZipFile(io.BytesIO(wheel_bytes)) as archive:
            names = archive.namelist()
            self.assertIn("demo/hello.py", names)
            self.assertIn("demo/hello.osri", names)
            self.assertIn("demo/hello.py.map", names)
            self.assertIn("demo/hello.osr", names)
            self.assertIn("demo/py.typed", names)
            marker = archive.read("demo_osiris-1.2.3.dist-info/osiris.toml").decode("utf-8")
            self.assertIn("records = \"demo-osiris.records.json\"", marker)
            self.assertIn('language_version = "0.1"', marker)
            self.assertIn("[[extension]]", marker)
            self.assertIn('id = "demo.hello"', marker)
            self.assertIn('interface = "demo/hello.osri"', marker)
            self.assertIn('source = "demo/hello.osr"', marker)
            self.assertIn('source_map = "demo/hello.py.map"', marker)
            records = archive.read("demo_osiris-1.2.3.dist-info/RECORD").decode("utf-8")
            self.assertIn("demo/hello.py,sha256=", records)
            self.assertTrue(records.endswith("demo_osiris-1.2.3.dist-info/RECORD,,\n"))
            sidecar = archive.read("demo-osiris.records.json")
            self.assertIn(b'"format-version":1', sidecar)
            self.assertNotIn(b"\n", sidecar)

    def test_incompatible_interface_abi_fails_closed(self):
        self._replace(
            self.compiler_path,
            ':compiler-abi "osiris-compiler-v0"',
            ':compiler-abi "unknown-compiler"',
        )
        with self.assertRaises(osiris_build.BackendError) as context:
            osiris_build.build_wheel(str(self.root / "wheel"), self._settings())
        self.assertIn("incompatible format or ABI", str(context.exception))

    def test_sidecar_must_reconstruct_from_interfaces(self):
        self._replace(
            self.compiler_path,
            "'owner-name': 'value',",
            "'owner-name': 'drifted',",
        )
        with self.assertRaises(osiris_build.BackendError) as context:
            osiris_build.build_wheel(str(self.root / "wheel"), self._settings())
        self.assertIn("cannot be reconstructed", str(context.exception))

    def test_prepared_metadata_must_match_current_build(self):
        metadata_root = self.root / "metadata"
        dist_info = osiris_build.prepare_metadata_for_build_wheel(str(metadata_root))
        metadata = metadata_root / dist_info / "METADATA"
        metadata.write_bytes(metadata.read_bytes() + b"X-Drift: true\n")
        with self.assertRaises(osiris_build.BackendError) as context:
            osiris_build.build_wheel(
                str(self.root / "wheel"),
                self._settings(),
                str(metadata_root),
            )
        self.assertIn("prepared METADATA differs", str(context.exception))

    def test_missing_metadata_directory_is_not_ignored(self):
        with self.assertRaises(osiris_build.BackendError) as context:
            osiris_build.build_wheel(
                str(self.root / "wheel"),
                self._settings(),
                str(self.root / "missing-metadata"),
            )
        self.assertIn("metadata_directory", str(context.exception))

    def test_wheel_is_byte_deterministic(self):
        first_dir = self.root / "wheel-a"
        second_dir = self.root / "wheel-b"
        first = (first_dir / osiris_build.build_wheel(str(first_dir), self._settings())).read_bytes()
        second = (second_dir / osiris_build.build_wheel(str(second_dir), self._settings())).read_bytes()
        self.assertEqual(first, second)
        self.assertEqual(hashlib.sha256(first).digest(), hashlib.sha256(second).digest())

    def test_multiple_sources_use_one_compiler_invocation(self):
        (self.root / "src" / "demo" / "world.osr").write_text("(module demo.world)\n", encoding="utf-8")
        wheel_dir = self.root / "multi-wheel"
        filename = osiris_build.build_wheel(str(wheel_dir), self._settings())
        with zipfile.ZipFile(wheel_dir / filename) as archive:
            self.assertIn("demo/hello.py", archive.namelist())
            self.assertIn("demo/world.py", archive.namelist())

    def test_real_compiler_wheel_runs_without_osiris_tooling_or_authored_source(self):
        compiler = ROOT / "target" / "debug" / "osr"
        if not compiler.is_file():
            self.skipTest("build target/debug/osr before running the real compiler wheel test")
        (self.root / "src" / "demo" / "hello.osr").write_text(
            """(module demo.hello)
(import osiris.core :refer [mapv])
(export [double-all])

^{:doc "Double every integer in a vector."}
(defn ^{:type (Vector Int)} double-all
  [^{:type (Vector Int)} values]
  (mapv (fn [^Int value] (* value 2)) values))
""",
            encoding="utf-8",
        )
        self._replace(
            self.root / "pyproject.toml",
            'dependencies = ["NumPy>=2"]',
            "dependencies = []",
        )
        (self.root / "uv.lock").write_text(
            """version = 1
revision = 3
requires-python = ">=3.11"

[[package]]
name = "demo-osiris"
source = { editable = "." }
""",
            encoding="utf-8",
        )
        wheel_dir = self.root / "real-wheel"
        filename = osiris_build.build_wheel(
            str(wheel_dir), {"osr-command": [str(compiler)]}
        )
        installed = self.root / "isolated-wheel"
        with zipfile.ZipFile(wheel_dir / filename) as archive:
            archive.extractall(installed)
            generated = archive.read("demo/hello.py").decode("utf-8")
            self.assertNotIn("import osiris", generated)
            self.assertNotIn("osiris.prelude", generated)
            names = archive.namelist()
            self.assertTrue(
                any(name.startswith("demo/__osiris_runtime__/") for name in names),
                names,
            )
            manifest_name = next(
                name
                for name in names
                if name.endswith("/__osiris_runtime__/manifest.json")
            )
            manifest = json.loads(archive.read(manifest_name))
            identity = manifest["sourceMaps"][0]
            source_map = json.loads(archive.read("demo/hello.py.map"))
            self.assertEqual(identity["source"], "demo/hello.osr")
            self.assertEqual(identity["sourceHash"], source_map["source_hash"])
            self.assertEqual(identity["generated"], source_map["generated"])
            self.assertEqual(identity["buildHash"], source_map["build_hash"])
            self.assertNotIn(str(self.root), archive.read(manifest_name).decode("utf-8"))
        shutil.move(str(self.root / "src"), str(self.root / "authored-source-removed"))
        script = """
import sys
sys.path[:] = [sys.argv[1]] + [path for path in sys.path if 'osiris' not in path]
from demo.hello import double_all
assert double_all((1, 2, 3)) == (2, 4, 6)
assert not any(name == 'osiris' or name.startswith('osiris.') for name in sys.modules)
print('standalone-ok')
"""
        environment = {"PATH": "/usr/bin:/bin", "PYTHONPATH": ""}
        completed = subprocess.run(
            [sys.executable, "-I", "-c", script, str(installed)],
            cwd=installed,
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=False,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(completed.stdout, "standalone-ok\n")

    def test_exclude_globs_filter_files_inside_source_roots(self):
        generated = self.root / "src" / "demo" / "generated"
        generated.mkdir()
        (generated / "ignored.osr").write_text("(module demo.generated.ignored)\n", encoding="utf-8")
        (self.root / "src" / "demo" / "hello_test.osr").write_text(
            "(module demo.hello_test)\n", encoding="utf-8"
        )
        self._replace(
            self.root / "osiris.jsonc",
            '"source": ["src"],',
            '"source": ["src"],\n  "exclude": ["src/**/generated/**", "src/**/*_test.osr"],',
        )
        wheel_dir = self.root / "excluded-wheel"
        filename = osiris_build.build_wheel(str(wheel_dir), self._settings())
        with zipfile.ZipFile(wheel_dir / filename) as archive:
            self.assertIn("demo/hello.py", archive.namelist())
            self.assertNotIn("demo/ignored.py", archive.namelist())
            self.assertNotIn("demo/hello_test.py", archive.namelist())

    def test_sdist_contains_sources_and_locked_build_inputs(self):
        sdist_dir = self.root / "sdist"
        filename = osiris_build.build_sdist(str(sdist_dir))
        with tarfile.open(sdist_dir / filename, "r:gz") as archive:
            names = archive.getnames()
            self.assertIn("demo-osiris-1.2.3/src/demo/hello.osr", names)
            constraints = archive.extractfile("demo-osiris-1.2.3/osiris-build-constraints.txt")
            self.assertEqual(constraints.read(), b"NumPy==2.1.0\n")
            self.assertIn("demo-osiris-1.2.3/osiris-build-inputs.sha256", names)


if __name__ == "__main__":
    unittest.main()
