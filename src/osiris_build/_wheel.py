"""Compiler invocation plus deterministic wheel and sdist assembly."""

import fnmatch
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path, PurePosixPath
from typing import Any, Dict, List, Mapping, Optional, Sequence, Set, Tuple, Union

from ._common import _error, _json_object, _normalise_name, _sha256
from ._interface import (
    _INTERFACE_COMPILER_ABI,
    _INTERFACE_LANGUAGE_ABI,
    _LANGUAGE_VERSION,
    _LINKABLE_HELPER_FORMAT,
    _STANDARD_LIBRARY_ABI,
    _interface_projection,
    _validate_sidecar,
    _validate_sidecar_against_interfaces,
)
from ._support import _rewrite_support_manifest
from ._model import (
    _BuildFiles,
    _ExtensionArtifact,
    _InterfaceProjection,
    _Project,
)


_MARKER_SCHEMA = 2
_MARKER_COMPILER_ABI = 1
_MARKER_LANGUAGE_ABI = 2
def _setting(config_settings: Optional[Mapping[str, Any]], *keys: str) -> Optional[Any]:
    if not config_settings:
        return None
    for key in keys:
        if key not in config_settings:
            continue
        value = config_settings[key]
        if isinstance(value, (list, tuple)):
            return value[-1] if value else None
        return value
    return None


def _compiler_command(config_settings: Optional[Mapping[str, Any]], project: _Project) -> List[str]:
    setting = _setting(config_settings, "osr-command", "compiler", "--osr-command")
    if setting is None:
        setting = os.environ.get("OSR_COMMAND") or os.environ.get("OSR")
    if setting is None:
        executable = shutil.which("osr")
        if executable:
            return [executable]
        raise _error("could not find `osr`; set OSR_COMMAND or config_settings['osr-command']")
    if isinstance(setting, (list, tuple)):
        command = [str(item) for item in setting]
    elif isinstance(setting, str):
        command = shlex.split(setting)
    else:
        raise _error("osr-command must be a command string or array")
    if not command:
        raise _error("osr-command cannot be empty")
    return command


def _safe_archive_path(value: Union[Path, str]) -> str:
    path = PurePosixPath(str(value).replace("\\", "/"))
    if path.is_absolute() or not path.parts or any(part in ("", ".", "..") for part in path.parts):
        raise _error("invalid generated archive path `%s`" % value)
    return str(path)


def _file_inside(root: Path, path: Path) -> bool:
    try:
        path.resolve().relative_to(root.resolve())
        return True
    except ValueError:
        return False


def _add_file(files: Dict[str, bytes], archive_path: str, data: bytes) -> None:
    archive_path = _safe_archive_path(archive_path)
    previous = files.get(archive_path)
    if previous is not None and previous != data:
        raise _error("two build inputs produce different `%s`" % archive_path)
    files[archive_path] = data


def _is_excluded(project: _Project, path: Path) -> bool:
    try:
        path.relative_to(project.output_dir)
    except ValueError:
        pass
    else:
        return True
    try:
        relative = path.relative_to(project.root).as_posix()
    except ValueError:
        return False
    return any(_matches_exclude(relative, pattern) for pattern in project.exclude_patterns)


def _matches_exclude(path: str, pattern: str) -> bool:
    has_glob = any(character in pattern for character in "*?[")
    if not has_glob:
        return path == pattern or path.startswith(pattern + "/")
    if pattern.endswith("/**") and path == pattern[:-3].rstrip("/"):
        return True
    segments = pattern.split("/")
    expression = "^"
    for index, segment in enumerate(segments):
        if segment == "**":
            expression += ".*" if index == len(segments) - 1 else "(?:[^/]+/)*"
            continue
        translated = fnmatch.translate(segment)
        if translated.endswith((r"\z", r"\Z")):
            translated = translated[:-2]
        expression += translated
        if index != len(segments) - 1:
            expression += "/"
    return re.fullmatch(expression, path) is not None


def _collect_static_files(project: _Project) -> Dict[str, bytes]:
    files: Dict[str, bytes] = {}
    for source_root in project.source_roots:
        for path in sorted(source_root.rglob("*"), key=lambda item: item.as_posix()):
            if _is_excluded(project, path):
                continue
            if path.is_symlink():
                raise _error("source file `%s` must not be a symlink" % path)
            if not path.is_file():
                continue
            if path.suffix in (".osr", ".pyc") or "__pycache__" in path.parts:
                continue
            if not _file_inside(project.root, path):
                raise _error("source file `%s` escapes the project" % path)
            relative = path.relative_to(source_root)
            if "__osiris_runtime__" in relative.parts:
                raise _error("`__osiris_runtime__` is reserved for compiler-linked support")
            _add_file(files, _safe_archive_path(relative), path.read_bytes())
    return files


def _packaged_sources(project: _Project, source_files: Sequence[Path]) -> Dict[str, bytes]:
    packaged: Dict[str, bytes] = {}
    for source in source_files:
        owners = [root for root in project.source_roots if _file_inside(root, source)]
        if len(owners) != 1:
            raise _error("source file `%s` must belong to exactly one source root" % source)
        relative = source.relative_to(owners[0])
        if "__osiris_runtime__" in relative.parts:
            raise _error("`__osiris_runtime__` is reserved for compiler-linked support")
        archive_path = _safe_archive_path(relative)
        _add_file(packaged, archive_path, source.read_bytes())
    return packaged


def _rewrite_source_map(
    project: _Project,
    path: str,
    data: bytes,
    source_path: str,
    source_data: bytes,
    generated_path: str,
) -> bytes:
    value = _json_object(data, "source map `%s`" % path)
    required = {
        "version", "language_version", "python_target", "source", "source_hash", "generated",
        "trust_policy_hash", "build_hash", "mappings",
    }
    if set(value) != required:
        raise _error("source map `%s` does not use the current schema" % path)
    if value["version"] != 3:
        raise _error("source map `%s` has unsupported version" % path)
    if value["language_version"] != _LANGUAGE_VERSION:
        raise _error("source map `%s` has an incompatible language version" % path)
    if value["python_target"] != "%d.%d" % project.target_python:
        raise _error("source map `%s` targets an incompatible Python version" % path)
    expected_hash = _sha256(source_data)
    if value["source_hash"] != expected_hash:
        raise _error("source map `%s` does not match authored source hash" % path)
    if value["generated"] != generated_path:
        raise _error("source map `%s` names an unexpected generated module" % path)
    if not isinstance(value["mappings"], list):
        raise _error("source map `%s` mappings must be an array" % path)
    value["source"] = source_path
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n").encode("utf-8")


def _collect_compiler_output(
    project: _Project,
    config_settings: Optional[Mapping[str, Any]],
    static_files: Dict[str, bytes],
) -> Tuple[List[_ExtensionArtifact], List[str], Optional[Tuple[str, bytes]]]:
    source_files: List[Path] = []
    for source_root in project.source_roots:
        source_files.extend(
            path
            for path in source_root.rglob("*.osr")
            if path.is_file() and _file_inside(project.root, path) and not _is_excluded(project, path)
        )
    if any(path.is_symlink() for source_root in project.source_roots for path in source_root.rglob("*.osr")):
        raise _error("Osiris source files must not be symlinks")
    source_files.sort(key=lambda item: item.relative_to(project.root).as_posix())
    if not source_files:
        raise _error("no .osr source files found under osiris.jsonc source roots")
    command = _compiler_command(config_settings, project)
    interfaces: List[_ExtensionArtifact] = []
    interface_projections: List[_InterfaceProjection] = []
    record_artifacts: List[Tuple[str, bytes]] = []
    support_manifests: List[str] = []
    packaged_sources = _packaged_sources(project, source_files)
    for archive_path, contents in packaged_sources.items():
        _add_file(static_files, archive_path, contents)
    with tempfile.TemporaryDirectory(prefix="osiris-build-compile-") as temporary:
        temporary_path = Path(temporary)
        output = temporary_path / "modules"
        output.mkdir()
        invocation = command + ["compile"]
        invocation.extend(str(source) for source in source_files)
        invocation.extend(["--out-dir", str(output), "--emit", "py,osri,map,records"])
        site_roots = sorted(
            {
                str(Path(entry).resolve())
                for entry in sys.path
                if entry and Path(entry).is_dir()
            }
        )
        for site_root in site_roots:
            invocation.extend(["--site-root", site_root])
        try:
            completed = subprocess.run(
                invocation,
                cwd=str(project.root),
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
                text=True,
            )
        except OSError as exc:
            raise _error("could not execute osr compiler: %s" % exc) from exc
        if completed.returncode != 0:
            details = (completed.stderr or completed.stdout).strip()
            if len(details) > 4000:
                details = details[-4000:]
            source_list = ", ".join(str(source.relative_to(project.root)) for source in source_files)
            raise _error(
                "osr compile failed for `%s` (exit %d)%s"
                % (source_list, completed.returncode, ": " + details if details else "")
            )
        produced = [
            path
            for path in sorted(output.rglob("*"), key=lambda item: item.as_posix())
            if path.is_file() and not path.is_symlink()
        ]
        if not any(path.suffix == ".py" for path in produced):
            raise _error("osr produced no .py artifacts")
        if not any(path.suffix == ".osri" for path in produced):
            raise _error("osr produced no .osri artifacts")
        produced_files = {
            _safe_archive_path(path.relative_to(output)): path.read_bytes() for path in produced
        }
        source_maps: Dict[str, bytes] = {}
        for path in produced:
            relative = path.relative_to(output)
            name = relative.as_posix()
            if name.endswith(".records.json"):
                record_artifacts.append((name, path.read_bytes()))
                continue
            if name.endswith("/__osiris_runtime__/manifest.json"):
                support_manifests.append(name)
                continue
            if not (name.endswith(".py") or name.endswith(".osri") or name.endswith(".py.map")):
                raise _error("osr produced unsupported artifact `%s`" % name)
            archive_path = _safe_archive_path(relative)
            artifact = path.read_bytes()
            if name.endswith(".py.map"):
                if "__osiris_runtime__" in PurePosixPath(archive_path).parts:
                    _add_file(static_files, archive_path, artifact)
                else:
                    source_maps[archive_path] = artifact
                continue
            _add_file(static_files, archive_path, artifact)
            if name.endswith(".osri"):
                interface_projections.append(_interface_projection(archive_path, artifact))
        for projection in interface_projections:
            base = projection.module.replace(".", "/")
            source_path = base + ".osr"
            generated_path = base + ".py"
            source_map_path = base + ".py.map"
            if source_path not in packaged_sources:
                raise _error("interface `%s` has no corresponding authored source" % projection.path)
            if generated_path not in static_files:
                raise _error("interface `%s` has no corresponding generated Python" % projection.path)
            raw_map = source_maps.pop(source_map_path, None)
            if raw_map is None:
                raise _error("interface `%s` has no corresponding source map" % projection.path)
            rewritten_map = _rewrite_source_map(
                project,
                source_map_path,
                raw_map,
                source_path,
                packaged_sources[source_path],
                generated_path,
            )
            _add_file(static_files, source_map_path, rewritten_map)
            interfaces.append(_ExtensionArtifact(
                projection.module,
                projection.path,
                projection.semantic_interface_hash,
                source_path,
                _sha256(packaged_sources[source_path]),
                source_map_path,
                _sha256(rewritten_map),
            ))
        if source_maps:
            raise _error("osr produced source maps without matching interfaces: %s" % ", ".join(sorted(source_maps)))
        for manifest_path in support_manifests:
            manifest_data = produced_files[manifest_path]
            manifest_data = _rewrite_support_manifest(
                manifest_path,
                manifest_data,
                project.target_python,
                produced_files,
                static_files,
            )
            _add_file(static_files, manifest_path, manifest_data)
    if len(record_artifacts) > 1:
        raise _error("osr emitted more than one distribution records manifest")
    modules = [interface.module for interface in interface_projections]
    if len(modules) != len(set(modules)):
        raise _error("osr emitted duplicate interface modules")
    if not record_artifacts:
        raise _error("osr emitted no distribution records manifest")
    sidecar = _validate_sidecar(record_artifacts[0][1])
    _validate_sidecar_against_interfaces(sidecar, interface_projections, project)
    if not sidecar["records"]:
        return sorted(interfaces, key=lambda item: item.identifier), sorted(set(support_manifests)), None
    return (
        sorted(interfaces, key=lambda item: item.identifier),
        sorted(set(support_manifests)),
        (record_artifacts[0][0], record_artifacts[0][1]),
    )


def _package_roots(files: Mapping[str, bytes]) -> Set[str]:
    roots: Set[str] = set()
    for name in files:
        if not name.endswith(".py"):
            continue
        parts = PurePosixPath(name).parts
        directories = [PurePosixPath(*parts[:index]) for index in range(1, len(parts))]
        package_dirs = [directory for directory in directories if str(directory / "__init__.py") in files]
        if package_dirs:
            roots.add(str(min(package_dirs, key=lambda item: len(item.parts))))
        elif directories:
            roots.add(str(directories[0]))
        else:
            roots.add("")
    return roots


def _marker_text(
    project: _Project,
    built: _BuildFiles,
    records_data: Optional[bytes],
) -> str:
    distribution, _ = _distribution_name(project)
    version, _ = _distribution_version(project)
    lines = [
        "schema = %d" % _MARKER_SCHEMA,
        "compiler_abi = %d" % _MARKER_COMPILER_ABI,
        "language_abi = %d" % _MARKER_LANGUAGE_ABI,
        "language_version = " + json.dumps(_LANGUAGE_VERSION),
        "standard_library_abi = %d" % _STANDARD_LIBRARY_ABI,
        "linkable_helper_format = %d" % _LINKABLE_HELPER_FORMAT,
        "distribution = " + json.dumps(distribution, ensure_ascii=False),
        "version = " + json.dumps(version, ensure_ascii=False),
        "python_target = " + json.dumps("%d.%d" % project.target_python),
        "dependencies = " + json.dumps(sorted(project.locked_requirements), ensure_ascii=False),
    ]
    if built.records_path is not None and records_data is not None:
        digest = _sha256(records_data)
        lines.extend([
            "records = " + json.dumps(built.records_path, ensure_ascii=False),
            "records_hash = " + json.dumps(digest),
        ])
    for manifest in sorted(built.support_manifests):
        lines.extend([
            "",
            "[[linked_support]]",
            "manifest = " + json.dumps(manifest, ensure_ascii=False),
            "manifest_hash = " + json.dumps(_sha256(built.files[manifest])),
        ])
    for artifact in sorted(built.interfaces, key=lambda item: item.identifier):
        lines.extend([
            "",
            "[[extension]]",
            "id = " + json.dumps(artifact.identifier, ensure_ascii=False),
            "interface = " + json.dumps(artifact.interface, ensure_ascii=False),
            "interface_hash = " + json.dumps(artifact.interface_hash),
            "source = " + json.dumps(artifact.source, ensure_ascii=False),
            "source_hash = " + json.dumps(artifact.source_hash),
            "source_map = " + json.dumps(artifact.source_map, ensure_ascii=False),
            "source_map_hash = " + json.dumps(artifact.source_map_hash),
        ])
    return "\n".join(lines) + "\n"


def _build_files(project: _Project, config_settings: Optional[Mapping[str, Any]]) -> _BuildFiles:
    files = _collect_static_files(project)
    interfaces, support_manifests, record_artifact = _collect_compiler_output(
        project, config_settings, files
    )
    for package_root in sorted(_package_roots(files)):
        marker_path = (PurePosixPath(package_root) / "py.typed") if package_root else PurePosixPath("py.typed")
        if str(marker_path) not in files:
            _add_file(files, str(marker_path), b"")
    records_path: Optional[str] = None
    if record_artifact is not None:
        generated_name = _normalise_name(str(project.project["name"])) + ".records.json"
        records_path = generated_name
        # The compiler's output path is intentionally not part of the package
        # contract; the distribution-level name is canonical.
        _add_file(files, records_path, record_artifact[1])
    return _BuildFiles(
        files=files,
        interfaces=interfaces,
        support_manifests=support_manifests,
        records_path=records_path,
    )


from ._wheel_archive import (  # noqa: E402,F401
    _dist_info_name,
    _distribution_name,
    _distribution_version,
    _metadata_bytes,
    _sdist_bytes,
    _sdist_inputs,
    _wheel_bytes,
    _wheel_metadata_bytes,
    _write_atomic,
)
