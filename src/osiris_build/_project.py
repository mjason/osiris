"""Read and validate project configuration without invoking the compiler."""

import os
import re
import sys
from pathlib import Path
from typing import Any, Dict, List, Set, Tuple

try:  # Python 3.11+
    import tomllib  # type: ignore[import-not-found]
except ModuleNotFoundError:  # pragma: no cover - Python 3.9/3.10
    import tomli as tomllib  # type: ignore[no-redef]

from ._common import _error, _normalise_name
from ._model import BackendError, _Project, _Requirement
from ._requirements import (
    _check_requires_python,
    _entry_dependency_names,
    _lock_packages,
    _lock_root_entry,
    _marker_applies,
    _parse_requirement,
    _project_requirements,
    _satisfies,
    _version_key,
)


def _project_root() -> Path:
    """Return the PEP 517 project root without following an arbitrary path."""

    override = os.environ.get("OSIRIS_PROJECT_ROOT")
    root = Path(override) if override else Path.cwd()
    try:
        return root.resolve()
    except OSError as exc:  # pragma: no cover - unusual filesystem failure
        raise _error("could not resolve project root: %s" % exc) from exc


def _read_toml(path: Path) -> Tuple[bytes, Dict[str, Any]]:
    try:
        raw = path.read_bytes()
    except OSError as exc:
        raise _error("could not read %s: %s" % (path, exc)) from exc
    try:
        value = tomllib.loads(raw.decode("utf-8"))
    except (tomllib.TOMLDecodeError, UnicodeDecodeError) as exc:
        raise _error("invalid TOML in %s: %s" % (path, exc)) from exc
    if not isinstance(value, dict):
        raise _error("TOML document %s is not a table" % path)
    return raw, value


def _as_table(value: Any, label: str) -> Dict[str, Any]:
    if value is None:
        return {}
    if not isinstance(value, dict):
        raise _error("%s must be a TOML table" % label)
    return value


def _parse_target(value: Any) -> Tuple[int, int]:
    if value is None:
        value = "3.9"
    if not isinstance(value, str) or not re.fullmatch(r"\d+\.\d+", value.strip()):
        raise _error("[tool.osiris].target-python must use MAJOR.MINOR form")
    major_text, minor_text = value.strip().split(".", 1)
    target = (int(major_text), int(minor_text))
    if target < (3, 9):
        raise _error("target-python %s.%s is below the supported minimum 3.9" % target)
    return target


def _validate_relative_path(value: Any, label: str) -> Path:
    if not isinstance(value, str) or not value:
        raise _error("%s must be a non-empty relative path" % label)
    path = Path(value)
    if path.is_absolute() or any(part in ("", ".", "..") for part in path.parts):
        raise _error("%s `%s` must be a normalized relative path" % (label, value))
    return path


def _load_project(require_lock: bool = True, enforce_runtime_python: bool = True) -> _Project:
    root = _project_root()
    pyproject_path = root / "pyproject.toml"
    pyproject_bytes, document = _read_toml(pyproject_path)
    project = _as_table(document.get("project"), "[project]")
    name = project.get("name")
    if not isinstance(name, str) or not name.strip():
        raise _error("[project].name is required")
    version = project.get("version")
    if not isinstance(version, str) or not version.strip():
        dynamic = project.get("dynamic", [])
        if "version" in dynamic if isinstance(dynamic, list) else True:
            raise _error("[project].version must be static for deterministic builds")
        raise _error("[project].version is required")
    if any(char in version for char in "\r\n"):
        raise _error("[project].version contains a newline")
    try:
        _version_key(version)
    except BackendError as exc:
        raise _error(
            "[project].version `%s` is outside the backend's numeric PEP 440 subset" % version
        ) from exc
    osiris = _as_table(_as_table(document.get("tool"), "[tool]").get("osiris"), "[tool.osiris]")
    if not osiris:
        raise _error("[tool.osiris] is required by the osiris-build backend")
    target = _parse_target(osiris.get("target-python"))
    _check_requires_python(project.get("requires-python"), target, "[project].requires-python")
    if enforce_runtime_python and (sys.version_info.major, sys.version_info.minor) != target:
        raise _error(
            "build interpreter Python %d.%d does not match target-python %d.%d"
            % (sys.version_info.major, sys.version_info.minor, target[0], target[1])
        )
    source_values = osiris.get("source", ["src"])
    if not isinstance(source_values, list) or not source_values:
        raise _error("[tool.osiris].source must be a non-empty array")
    source_roots: List[Path] = []
    for index, value in enumerate(source_values):
        relative = _validate_relative_path(value, "source root %d" % index)
        absolute = (root / relative).resolve()
        try:
            absolute.relative_to(root)
        except ValueError as exc:
            raise _error("source root `%s` escapes the project" % relative) from exc
        if not absolute.is_dir():
            raise _error("source root `%s` does not exist" % relative)
        if (root / relative).is_symlink():
            raise _error("source root `%s` must not be a symlink" % relative)
        source_roots.append(absolute)
    build_groups = osiris.get("build-groups", [])
    if not isinstance(build_groups, list):
        raise _error("[tool.osiris].build-groups must be an array")
    if any(not isinstance(group, str) or not group.strip() for group in build_groups):
        raise _error("[tool.osiris].build-groups entries must be non-empty strings")
    if len(set(build_groups)) != len(build_groups):
        raise _error("[tool.osiris].build-groups must not contain duplicates")
    lock_path = root / "uv.lock"
    if not require_lock:
        lock_bytes, lock_document = b"", {}
        requirements: List[_Requirement] = []
        locked: List[str] = []
    else:
        lock_bytes, lock_document = _read_toml(lock_path)
        if lock_document.get("version") != 1:
            raise _error("uv.lock must use lock format version 1")
        _check_requires_python(lock_document.get("requires-python"), target, "uv.lock requires-python")
        requirements = _project_requirements(project, document, build_groups)
        packages = _lock_packages(lock_document, target)
        root_entry = _lock_root_entry(lock_document, name)
        runtime_names = {item.normalized_name for item in requirements}
        if root_entry is None and runtime_names:
            raise _error("uv.lock has no editable project entry for `%s`" % name)
        if root_entry is not None:
            root_names: Set[str] = set()
            if "dependencies" in root_entry:
                root_names.update(_entry_dependency_names(root_entry["dependencies"], target))
            declared_runtime = project.get("dependencies", [])
            if not isinstance(declared_runtime, list):
                raise _error("[project].dependencies must be an array")
            expected_runtime = {
                item.normalized_name
                for item in (_parse_requirement(value) for value in declared_runtime)
                if not item.marker or _marker_applies(item.marker, target)
            }
            applicable_requirements = [
                item
                for item in requirements
                if not item.marker or _marker_applies(item.marker, target)
            ]
            selected_group_names = {
                item.normalized_name
                for item in applicable_requirements
                if item.normalized_name not in expected_runtime
            }
            unexpected_root = root_names - expected_runtime - selected_group_names
            missing_runtime = expected_runtime - root_names
            if unexpected_root or missing_runtime:
                raise _error(
                    "uv.lock is stale; root dependencies (%s) do not match project dependencies (%s)"
                    % (", ".join(sorted(root_names)) or "none", ", ".join(sorted(expected_runtime)) or "none")
                )
            # Build groups are represented by uv's dev-dependencies or
            # metadata tables.  Verify presence without assuming a particular
            # uv minor's exact table layout.
            lock_group_names: Set[str] = set()
            for key in ("dev-dependencies", "metadata", "optional-dependencies"):
                if key in root_entry:
                    lock_group_names.update(_entry_dependency_names(root_entry[key], target))
            missing_group = {
                item.normalized_name
                for item in applicable_requirements
                if item.normalized_name not in expected_runtime
                and item.normalized_name not in lock_group_names
                and item.normalized_name not in root_names
            }
            if missing_group:
                raise _error(
                    "uv.lock is stale; selected build-group dependencies are missing: %s"
                    % ", ".join(sorted(missing_group))
                )
        locked = []
        for requirement in requirements:
            if requirement.marker and not _marker_applies(requirement.marker, target):
                continue
            package = packages.get(requirement.normalized_name)
            if package is None:
                raise _error(
                    "uv.lock has no applicable locked package for `%s` at Python %d.%d"
                    % (requirement.raw, target[0], target[1])
                )
            if not _satisfies(requirement.specifier, package.version):
                raise _error(
                    "locked `%s==%s` does not satisfy declared `%s`"
                    % (package.name, package.version, requirement.raw)
                )
            exact = requirement.name + requirement.extras + "==" + package.version
            if requirement.marker:
                exact += "; " + requirement.marker
            locked.append(exact)
        locked.sort(key=lambda item: (_normalise_name(item.split("[", 1)[0].split("==", 1)[0]), item))
    return _Project(
        root=root,
        pyproject_path=pyproject_path,
        pyproject_bytes=pyproject_bytes,
        document=document,
        project=project,
        osiris=osiris,
        source_roots=source_roots,
        target_python=target,
        build_groups=list(build_groups),
        requirements=[item.raw for item in requirements],
        locked_requirements=locked,
        lock_bytes=lock_bytes,
        lock_document=lock_document,
    )
