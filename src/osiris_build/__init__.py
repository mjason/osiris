"""A small, deterministic PEP 517 backend for Osiris projects.

The backend deliberately does not resolve or install dependencies.  It only
projects requirements already present in ``uv.lock`` and invokes the ``osr``
compiler supplied by the build environment.  This keeps dependency ownership
with uv/PEP 517 while making an Osiris build reproducible and fail closed when
the lock or compiler contract is incomplete.
"""

from __future__ import annotations

import base64
import gzip
import hashlib
import io
import json
import os
import platform
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any, Dict, List, Mapping, Optional, Sequence, Set, Tuple, Union

try:  # Python 3.11+
    import tomllib  # type: ignore[import-not-found]
except ModuleNotFoundError:  # pragma: no cover - exercised on Python 3.9/3.10
    import tomli as tomllib  # type: ignore[no-redef]


__version__ = "0.1.0"
__all__ = [
    "BackendError",
    "build_sdist",
    "build_wheel",
    "get_requires_for_build_sdist",
    "get_requires_for_build_wheel",
    "prepare_metadata_for_build_wheel",
]


class BackendError(RuntimeError):
    """A configuration, lock, compiler, or packaging error."""


_NAME_RE = re.compile(r"^\s*([A-Za-z0-9][A-Za-z0-9._-]*)(?:\[([^\]]+)\])?\s*(.*)$")
_COMPARATOR_RE = re.compile(r"^(===|==|!=|~=|>=|<=|>|<)\s*([^,]+)$")
_MARKER_ATOM_RE = re.compile(
    r"^\s*(python_version|python_full_version|os_name|sys_platform|platform_machine|"
    r"platform_python_implementation|platform_system|implementation_name)\s*"
    r"(==|!=|>=|<=|>|<|in|not\s+in)\s*(['\"])(.*?)\3\s*$",
    re.IGNORECASE,
)
_NUMERIC_RELEASE_RE = re.compile(r"^\d+(?:\.\d+)*$")
_HASH_RE = re.compile(r"^sha256:[0-9a-f]{64}$")

_INTERFACE_FORMAT = "osiris-interface"
_INTERFACE_FORMAT_VERSION = 2
_INTERFACE_COMPILER_ABI = "osiris-compiler-v0"
_INTERFACE_LANGUAGE_ABI = "osiris-language-v1"
_MARKER_SCHEMA = 1
_MARKER_COMPILER_ABI = 1
_MARKER_LANGUAGE_ABI = 2


@dataclass(frozen=True)
class _Requirement:
    raw: str
    name: str
    normalized_name: str
    extras: str
    specifier: str
    marker: str


@dataclass(frozen=True)
class _LockedPackage:
    name: str
    normalized_name: str
    version: str
    markers: Tuple[str, ...]


@dataclass
class _Project:
    root: Path
    pyproject_path: Path
    pyproject_bytes: bytes
    document: Dict[str, Any]
    project: Dict[str, Any]
    osiris: Dict[str, Any]
    source_roots: List[Path]
    target_python: Tuple[int, int]
    build_groups: List[str]
    requirements: List[str]
    locked_requirements: List[str]
    lock_bytes: bytes
    lock_document: Dict[str, Any]


@dataclass
class _BuildFiles:
    files: Dict[str, bytes]
    interfaces: List[str]
    records_path: Optional[str]


@dataclass(frozen=True)
class _InterfaceProjection:
    path: str
    module: str
    semantic_interface_hash: str
    records: Tuple[Dict[str, Any], ...]


def _error(message: str) -> BackendError:
    return BackendError("osiris-build: " + message)


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


def _normalise_name(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


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


def _parse_requirement(raw: Any) -> _Requirement:
    if not isinstance(raw, str) or not raw.strip():
        raise _error("dependency requirement must be a non-empty string")
    text = raw.strip()
    left, marker = (text.split(";", 1) + [""])[:2] if ";" in text else (text, "")
    left = left.strip()
    marker = marker.strip()
    match = _NAME_RE.match(left)
    if not match:
        raise _error("unsupported dependency requirement `%s`" % text)
    name, extras, specifier = match.groups()
    if " @ " in left or specifier.startswith(("@", "://", "+")):
        raise _error("direct URL/path dependency `%s` cannot be projected from uv.lock" % text)
    if specifier and not specifier.startswith(("<", ">", "=", "!", "~")):
        raise _error("unsupported dependency requirement `%s`" % text)
    if marker:
        _validate_marker(marker)
    return _Requirement(
        raw=text,
        name=name,
        normalized_name=_normalise_name(name),
        extras=("[" + extras + "]") if extras else "",
        specifier=specifier.strip(),
        marker=marker,
    )


def _split_top_level(text: str, operator: str) -> List[str]:
    """Split a marker around a boolean operator outside quotes/parentheses."""

    result: List[str] = []
    start = 0
    depth = 0
    quote: Optional[str] = None
    index = 0
    needle = " " + operator + " "
    while index < len(text):
        char = text[index]
        if quote:
            if char == quote and (index == 0 or text[index - 1] != "\\"):
                quote = None
            index += 1
            continue
        if char in "'\"":
            quote = char
            index += 1
            continue
        if char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
            if depth < 0:
                raise _error("invalid dependency marker `%s`" % text)
        elif depth == 0 and text.startswith(needle, index):
            result.append(text[start:index].strip())
            index += len(needle)
            start = index
            continue
        index += 1
    if quote or depth:
        raise _error("invalid dependency marker `%s`" % text)
    result.append(text[start:].strip())
    return result


def _strip_parentheses(text: str) -> str:
    value = text.strip()
    while value.startswith("(") and value.endswith(")"):
        depth = 0
        quote: Optional[str] = None
        encloses = True
        for index, char in enumerate(value):
            if quote:
                if char == quote and (index == 0 or value[index - 1] != "\\"):
                    quote = None
                continue
            if char in "'\"":
                quote = char
            elif char == "(":
                depth += 1
            elif char == ")":
                depth -= 1
                if depth == 0 and index != len(value) - 1:
                    encloses = False
                    break
        if encloses and depth == 0:
            value = value[1:-1].strip()
        else:
            break
    return value


def _validate_marker(marker: str) -> None:
    # A neutral Python target is sufficient for syntax validation; platform
    # values still come from the build interpreter because cross-platform
    # builds are not an advertised backend capability.
    _marker_applies(marker, (3, 9), validate_only=True)


def _marker_applies(marker: str, target: Tuple[int, int], validate_only: bool = False) -> bool:
    text = _strip_parentheses(marker.strip())
    if not text:
        return True
    # PEP 508 gives ``or`` lower precedence than ``and``.
    alternatives = _split_top_level(text, "or")
    if len(alternatives) > 1:
        results = [_marker_applies(item, target, validate_only) for item in alternatives]
        return True if validate_only else any(results)
    conjunction = _split_top_level(text, "and")
    if len(conjunction) > 1:
        results = [_marker_applies(item, target, validate_only) for item in conjunction]
        return True if validate_only else all(results)
    match = _MARKER_ATOM_RE.match(text)
    if not match:
        # ``extra`` markers are meaningful only while building an optional
        # extra.  This backend does not project optional dependencies.
        if "extra" in text.lower():
            raise _error("dependency marker using `extra` is not build-time resolvable: `%s`" % marker)
        raise _error("unsupported dependency marker `%s`" % marker)
    variable, operator, _, expected = match.groups()
    variable = variable.lower()
    if variable == "python_version":
        actual = "%d.%d" % target
    elif variable == "python_full_version":
        actual = "%d.%d.0" % target
    elif variable == "os_name":
        actual = os.name
    elif variable == "sys_platform":
        actual = sys.platform
    elif variable == "platform_machine":
        actual = platform.machine()
    elif variable == "platform_python_implementation":
        actual = platform.python_implementation()
    elif variable == "platform_system":
        actual = platform.system()
    elif variable == "implementation_name":
        actual = getattr(sys.implementation, "name", "")
    else:  # pragma: no cover - regex currently enumerates all variables
        raise _error("unsupported dependency marker `%s`" % marker)
    operator = " ".join(operator.lower().split())
    if operator in ("in", "not in"):
        if validate_only:
            return True
        result = actual in expected
        return not result if operator == "not in" else result
    if variable.startswith("python_"):
        left_version = _version_key(actual)
        right_version = _version_key(expected)
        if validate_only:
            return True
        if operator == "==":
            return _compare_releases(left_version, right_version) == 0
        if operator == "!=":
            return _compare_releases(left_version, right_version) != 0
        if operator == ">=":
            return _compare_releases(left_version, right_version) >= 0
        if operator == "<=":
            return _compare_releases(left_version, right_version) <= 0
        if operator == ">":
            return _compare_releases(left_version, right_version) > 0
        if operator == "<":
            return _compare_releases(left_version, right_version) < 0
    if validate_only:
        return True
    if operator == "==":
        return actual == expected
    if operator == "!=":
        return actual != expected
    if operator == ">=":
        return actual >= expected
    if operator == "<=":
        return actual <= expected
    if operator == ">":
        return actual > expected
    if operator == "<":
        return actual < expected
    raise _error("unsupported dependency marker `%s`" % marker)


def _version_key(value: str) -> Tuple[int, ...]:
    text = value.strip()
    if not _NUMERIC_RELEASE_RE.fullmatch(text):
        raise _error("unsupported non-numeric PEP 440 release `%s`" % value)
    return tuple(int(part) for part in text.split("."))


def _compare_releases(left: Tuple[int, ...], right: Tuple[int, ...]) -> int:
    width = max(len(left), len(right))
    padded_left = left + (0,) * (width - len(left))
    padded_right = right + (0,) * (width - len(right))
    return (padded_left > padded_right) - (padded_left < padded_right)


def _release_has_prefix(release: Tuple[int, ...], prefix: Tuple[int, ...]) -> bool:
    padded = release + (0,) * max(0, len(prefix) - len(release))
    return padded[: len(prefix)] == prefix


def _satisfies(specifier: str, version: str) -> bool:
    """Check a conservative numeric-release subset of PEP 440.

    Pre/dev/post/local releases are rejected instead of being truncated.  The
    backend is a lock projector, not a resolver, so an unsupported version must
    fail closed rather than select a potentially different lock fork.
    """

    if not specifier:
        return True
    actual: Optional[Tuple[int, ...]] = None
    for clause in specifier.split(","):
        clause = clause.strip()
        if not clause:
            raise _error("unsupported dependency version specifier `%s`" % specifier)
        match = _COMPARATOR_RE.match(clause)
        if not match:
            raise _error("unsupported dependency version specifier `%s`" % specifier)
        operator, expected_text = match.groups()
        expected_text = expected_text.strip()
        if operator == "===":
            if version != expected_text:
                return False
            continue
        if actual is None:
            try:
                actual = _version_key(version)
            except BackendError as exc:
                raise _error(
                    "cannot validate locked version `%s` against `%s`: %s"
                    % (version, specifier, exc)
                ) from exc
        if expected_text.endswith(".*"):
            prefix_text = expected_text[:-2]
            if operator not in ("==", "!="):
                raise _error("unsupported wildcard dependency specifier `%s`" % specifier)
            prefix = _version_key(prefix_text)
            matches = _release_has_prefix(actual, prefix)
            if operator == "==" and not matches:
                return False
            if operator == "!=" and matches:
                return False
            continue
        expected = _version_key(expected_text)
        comparison = _compare_releases(actual, expected)
        if operator == "==" and comparison != 0:
            return False
        if operator == "!=" and comparison == 0:
            return False
        if operator == ">=" and comparison < 0:
            return False
        if operator == "<=" and comparison > 0:
            return False
        if operator == ">" and comparison <= 0:
            return False
        if operator == "<" and comparison >= 0:
            return False
        if operator == "~=":
            if len(expected) < 2:
                raise _error("compatible release specifier requires at least two segments: `%s`" % clause)
            if comparison < 0 or not _release_has_prefix(actual, expected[:-1]):
                return False
    return True


def _group_requirements(document: Mapping[str, Any], groups: Sequence[str]) -> List[_Requirement]:
    table = document.get("dependency-groups", {})
    if table is None:
        table = {}
    if not isinstance(table, dict):
        raise _error("[dependency-groups] must be a TOML table")
    result: List[_Requirement] = []
    visiting: Set[str] = set()

    def visit(name: str) -> None:
        if name in visiting:
            raise _error("dependency group cycle at `%s`" % name)
        if name not in table:
            raise _error("selected dependency group `%s` is missing" % name)
        value = table[name]
        if not isinstance(value, list):
            raise _error("dependency group `%s` must be an array" % name)
        visiting.add(name)
        try:
            for item in value:
                if isinstance(item, str):
                    result.append(_parse_requirement(item))
                elif isinstance(item, dict) and set(item) == {"include-group"}:
                    included = item.get("include-group")
                    if not isinstance(included, str):
                        raise _error("include-group in `%s` must be a string" % name)
                    visit(included)
                else:
                    raise _error("unsupported dependency group entry in `%s`" % name)
        finally:
            visiting.remove(name)

    for group in groups:
        if not isinstance(group, str) or not group:
            raise _error("[tool.osiris].build-groups entries must be strings")
        visit(group)
    return result


def _lock_package_entries(document: Mapping[str, Any]) -> List[Mapping[str, Any]]:
    packages = document.get("package", [])
    if not isinstance(packages, list):
        raise _error("uv.lock package entries must be an array")
    result: List[Mapping[str, Any]] = []
    for package in packages:
        if not isinstance(package, dict):
            raise _error("uv.lock contains a non-table package entry")
        result.append(package)
    return result


def _package_markers(package: Mapping[str, Any]) -> Tuple[str, ...]:
    value = package.get("resolution-markers", package.get("resolution_markers", []))
    if value is None:
        return ()
    if isinstance(value, str):
        return (value,)
    if isinstance(value, list) and all(isinstance(item, str) for item in value):
        return tuple(value)
    raise _error("uv.lock resolution-markers must contain strings")


def _lock_packages(document: Mapping[str, Any], target: Tuple[int, int]) -> Dict[str, _LockedPackage]:
    candidates: Dict[str, List[_LockedPackage]] = {}
    for package in _lock_package_entries(document):
        name = package.get("name")
        version = package.get("version")
        if not isinstance(name, str) or not name:
            raise _error("uv.lock package is missing a name")
        if not isinstance(version, str):
            # Editable project roots do not have a version and are never a
            # dependency candidate, but all third-party entries must.
            source = package.get("source", {})
            if not isinstance(source, dict) or source.get("editable") != ".":
                raise _error("uv.lock package `%s` is missing a locked version" % name)
            continue
        try:
            _version_key(version)
        except BackendError as exc:
            raise _error("uv.lock package `%s` has an unsupported version `%s`" % (name, version)) from exc
        markers = _package_markers(package)
        if markers and not any(_marker_applies(marker, target) for marker in markers):
            continue
        normalized = _normalise_name(name)
        candidates.setdefault(normalized, []).append(
            _LockedPackage(name, normalized, version, markers)
        )
    result: Dict[str, _LockedPackage] = {}
    for normalized, entries in candidates.items():
        versions = {entry.version for entry in entries}
        if len(versions) != 1:
            raise _error(
                "uv.lock has multiple applicable versions for `%s` at Python %d.%d"
                % (normalized, target[0], target[1])
            )
        result[normalized] = entries[0]
    return result


def _entry_dependency_names(value: Any, target: Tuple[int, int]) -> Set[str]:
    names: Set[str] = set()
    if isinstance(value, str):
        requirement = _parse_requirement(value)
        if not requirement.marker or _marker_applies(requirement.marker, target):
            names.add(requirement.normalized_name)
    elif isinstance(value, dict):
        marker = value.get("marker")
        if marker is not None:
            if not isinstance(marker, str):
                raise _error("uv.lock dependency marker must be a string")
            _validate_marker(marker)
            if not _marker_applies(marker, target):
                return names
        name = value.get("name")
        if isinstance(name, str):
            names.add(_normalise_name(name))
        for key, child in value.items():
            if key in ("name", "marker", "version", "source"):
                continue
            if isinstance(child, (list, dict)):
                names.update(_entry_dependency_names(child, target))
    elif isinstance(value, list):
        for child in value:
            names.update(_entry_dependency_names(child, target))
    return names


def _lock_root_entry(document: Mapping[str, Any], project_name: str) -> Optional[Mapping[str, Any]]:
    entries = _lock_package_entries(document)
    editable: List[Mapping[str, Any]] = []
    for package in entries:
        source = package.get("source")
        if isinstance(source, dict) and source.get("editable") == ".":
            editable.append(package)
    if len(editable) > 1:
        raise _error("uv.lock contains more than one editable project root")
    if editable:
        return editable[0]
    matching = [
        package
        for package in entries
        if isinstance(package.get("name"), str)
        and _normalise_name(package["name"]) == _normalise_name(project_name)
    ]
    if len(matching) > 1:
        raise _error("uv.lock contains multiple project entries for `%s`" % project_name)
    return matching[0] if matching else None


def _check_requires_python(expression: Any, target: Tuple[int, int], label: str) -> None:
    if expression is None:
        return
    if not isinstance(expression, str) or not expression.strip():
        raise _error("%s must be a Python version specifier" % label)
    # Reuse the same conservative comparator parser used for package ranges.
    if not _satisfies(expression, "%d.%d" % target):
        raise _error(
            "%s `%s` excludes target Python %d.%d"
            % (label, expression, target[0], target[1])
        )


def _project_requirements(
    project: Mapping[str, Any],
    document: Mapping[str, Any],
    groups: Sequence[str],
) -> List[_Requirement]:
    dependencies = project.get("dependencies", [])
    if dependencies is None:
        dependencies = []
    if not isinstance(dependencies, list):
        raise _error("[project].dependencies must be an array")
    result = [_parse_requirement(item) for item in dependencies]
    result.extend(_group_requirements(document, groups))
    return result


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


class _SAtom(str):
    pass


class _SKeyword(str):
    pass


@dataclass(frozen=True)
class _SList:
    items: Tuple[Any, ...]


@dataclass(frozen=True)
class _SVector:
    items: Tuple[Any, ...]


@dataclass(frozen=True)
class _SMap:
    items: Tuple[Any, ...]


@dataclass(frozen=True)
class _SSet:
    items: Tuple[Any, ...]


@dataclass(frozen=True)
class _SPrefix:
    prefix: str
    items: Tuple[Any, ...]


def _has_valid_unicode_scalars(value: str) -> bool:
    return not any(0xD800 <= ord(char) <= 0xDFFF for char in value)


class _SExpressionReader:
    """A bounded reader for compiler-owned, normalized `.osri` data."""

    def __init__(self, data: bytes, label: str):
        if len(data) > 32 * 1024 * 1024:
            raise _error("interface `%s` exceeds the 32 MiB build limit" % label)
        try:
            self.text = data.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise _error("interface `%s` is not UTF-8: %s" % (label, exc)) from exc
        if self.text.startswith("\ufeff"):
            raise _error("interface `%s` must not contain a UTF-8 BOM" % label)
        self.label = label
        self.index = 0
        self.nodes = 0

    def read_document(self) -> Tuple[Any, ...]:
        forms: List[Any] = []
        self._skip_trivia()
        while self.index < len(self.text):
            forms.append(self._read_form(0))
            self._skip_trivia()
        return tuple(forms)

    def _skip_trivia(self) -> None:
        while self.index < len(self.text):
            char = self.text[self.index]
            if char.isspace():
                self.index += 1
                continue
            if char == ";":
                newline = self.text.find("\n", self.index)
                self.index = len(self.text) if newline < 0 else newline + 1
                continue
            break

    def _read_form(self, depth: int) -> Any:
        if depth > 256:
            raise _error("interface `%s` exceeds the reader nesting limit" % self.label)
        self.nodes += 1
        if self.nodes > 1_000_000:
            raise _error("interface `%s` exceeds the reader form limit" % self.label)
        self._skip_trivia()
        if self.index >= len(self.text):
            raise _error("interface `%s` ends inside a form" % self.label)
        if self.text.startswith("#{", self.index):
            self.index += 2
            return _SSet(self._read_collection("}", depth))
        char = self.text[self.index]
        if char == "(":
            self.index += 1
            return _SList(self._read_collection(")", depth))
        if char == "[":
            self.index += 1
            return _SVector(self._read_collection("]", depth))
        if char == "{":
            self.index += 1
            return _SMap(self._read_collection("}", depth))
        if char in ")]}":
            raise _error("interface `%s` contains unmatched `%s`" % (self.label, char))
        if char == '"':
            return self._read_string()
        if self.text.startswith("~@", self.index):
            self.index += 2
            return _SPrefix("~@", (self._read_form(depth + 1),))
        if char in "'`~":
            self.index += 1
            return _SPrefix(char, (self._read_form(depth + 1),))
        if char == "^":
            self.index += 1
            return _SPrefix(
                "^",
                (self._read_form(depth + 1), self._read_form(depth + 1)),
            )
        return self._read_atom()

    def _read_collection(self, closer: str, depth: int) -> Tuple[Any, ...]:
        items: List[Any] = []
        while True:
            self._skip_trivia()
            if self.index >= len(self.text):
                raise _error(
                    "interface `%s` is missing closing `%s`" % (self.label, closer)
                )
            if self.text[self.index] == closer:
                self.index += 1
                return tuple(items)
            items.append(self._read_form(depth + 1))

    def _read_string(self) -> str:
        try:
            value, end = json.JSONDecoder().raw_decode(self.text, self.index)
        except json.JSONDecodeError as exc:
            raise _error("interface `%s` has an invalid string: %s" % (self.label, exc)) from exc
        if not isinstance(value, str) or not _has_valid_unicode_scalars(value):
            raise _error("interface `%s` contains an invalid Unicode scalar" % self.label)
        self.index = end
        return value

    def _read_atom(self) -> Any:
        start = self.index
        delimiters = set("()[]{}\"'`~^;")
        while self.index < len(self.text):
            char = self.text[self.index]
            if char.isspace() or char in delimiters:
                break
            self.index += 1
        if self.index == start:
            raise _error(
                "interface `%s` contains unsupported reader syntax near byte %d"
                % (self.label, self.index)
            )
        atom = self.text[start:self.index]
        if atom == "true":
            return True
        if atom == "false":
            return False
        if atom == "none":
            return None
        if re.fullmatch(r"-?(?:0|[1-9]\d*)", atom):
            return int(atom)
        if atom.startswith(":") and len(atom) > 1:
            return _SKeyword(atom[1:])
        return _SAtom(atom)


def _read_osri(data: bytes, label: str) -> Tuple[Any, ...]:
    return _SExpressionReader(data, label).read_document()


def _sexpr_map(value: Any, label: str, expected: Optional[Set[str]] = None) -> Dict[str, Any]:
    if not isinstance(value, _SMap) or len(value.items) % 2:
        raise _error("interface %s must be a keyword map" % label)
    result: Dict[str, Any] = {}
    iterator = iter(value.items)
    for key, item in zip(iterator, iterator):
        if not isinstance(key, _SKeyword):
            raise _error("interface %s contains a non-keyword map key" % label)
        name = str(key)
        if name in result:
            raise _error("interface %s contains duplicate key `%s`" % (label, name))
        result[name] = item
    if expected is not None and set(result) != expected:
        raise _error("interface %s has an unsupported shape" % label)
    return result


def _sexpr_vector(value: Any, label: str) -> Tuple[Any, ...]:
    if not isinstance(value, _SVector):
        raise _error("interface %s must be a vector" % label)
    return value.items


def _sexpr_string(value: Any, label: str) -> str:
    if not isinstance(value, str) or isinstance(value, (_SAtom, _SKeyword)):
        raise _error("interface %s must be a string" % label)
    return value


def _sexpr_integer(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise _error("interface %s must be a non-negative integer" % label)
    return value


def _sexpr_keyword(value: Any, label: str) -> str:
    if not isinstance(value, _SKeyword):
        raise _error("interface %s must be a keyword" % label)
    return str(value)


def _interface_hash(value: Any, label: str) -> str:
    digest = _sexpr_string(value, label)
    if not _HASH_RE.fullmatch(digest):
        raise _error("interface %s must be a lowercase SHA-256 digest" % label)
    return digest


def _optional_interface_string(value: Any, label: str) -> Optional[str]:
    if value is None:
        return None
    return _sexpr_string(value, label)


def _static_datum_from_interface(value: Any, label: str) -> Any:
    values = _sexpr_vector(value, label)
    if not values:
        raise _error("interface %s has an empty static datum" % label)
    tag = _sexpr_keyword(values[0], label + " tag")
    if tag == "none" and len(values) == 1:
        return None
    if tag == "bool" and len(values) == 2 and isinstance(values[1], bool):
        return values[1]
    if tag == "int" and len(values) == 2:
        integer = _sexpr_string(values[1], label + " integer")
        if not re.fullmatch(r"0|-?[1-9]\d*", integer):
            raise _error("interface %s contains a non-canonical integer" % label)
        return {"$osiris": "int", "value": integer}
    if tag == "float" and len(values) == 2:
        bits = _sexpr_string(values[1], label + " float")
        if not re.fullmatch(r"[0-9a-f]{16}", bits):
            raise _error("interface %s contains invalid binary64 bits" % label)
        exponent = (int(bits, 16) >> 52) & 0x7FF
        if exponent == 0x7FF:
            raise _error("interface %s contains a non-finite float" % label)
        return {"$osiris": "float", "value": bits}
    if tag == "string" and len(values) == 2:
        return _sexpr_string(values[1], label + " string")
    if tag == "keyword" and len(values) == 2:
        return {
            "$osiris": "keyword",
            "spelling": _sexpr_string(values[1], label + " keyword"),
        }
    if tag == "symbol" and len(values) == 3:
        result = {
            "$osiris": "symbol",
            "spelling": _sexpr_string(values[1], label + " symbol"),
        }
        binding = _optional_interface_string(values[2], label + " binding")
        if binding is not None:
            result["binding-id"] = binding
        return result
    if tag in ("list", "vector", "set") and len(values) == 2:
        items = [
            _static_datum_from_interface(item, label + " item")
            for item in _sexpr_vector(values[1], label + " items")
        ]
        return {"$osiris": tag, "items": items}
    if tag == "map" and len(values) == 2:
        entries = []
        for item in _sexpr_vector(values[1], label + " entries"):
            pair = _sexpr_vector(item, label + " entry")
            if len(pair) != 2:
                raise _error("interface %s map entry must be a pair" % label)
            entries.append(
                [
                    _static_datum_from_interface(pair[0], label + " key"),
                    _static_datum_from_interface(pair[1], label + " value"),
                ]
            )
        return {"$osiris": "map", "entries": entries}
    raise _error("interface %s contains unsupported static datum `%s`" % (label, tag))


def _schema_identity_from_interface(value: Any, label: str) -> Dict[str, Any]:
    fields = _sexpr_map(
        value,
        label,
        {"binding-id", "schema-id", "version", "body-hash"},
    )
    return {
        "binding-id": _sexpr_string(fields["binding-id"], label + " binding-id"),
        "schema-id": _sexpr_string(fields["schema-id"], label + " schema-id"),
        "version": _sexpr_integer(fields["version"], label + " version"),
        "body-hash": _interface_hash(fields["body-hash"], label + " body-hash"),
    }


def _index_claim_from_interface(value: Any, label: str) -> Dict[str, Any]:
    fields = _sexpr_map(
        value,
        label,
        {
            "index-id",
            "projection-field",
            "projection-role",
            "key",
            "normalized-key",
            "raw-spelling",
        },
    )
    result = {
        "index-id": _sexpr_string(fields["index-id"], label + " index-id"),
        "projection-field": _sexpr_string(
            fields["projection-field"], label + " projection-field"
        ),
        "projection-role": _sexpr_string(
            fields["projection-role"], label + " projection-role"
        ),
        "key": _static_datum_from_interface(fields["key"], label + " key"),
        "normalized-key": _sexpr_string(
            fields["normalized-key"], label + " normalized-key"
        ),
    }
    raw = _optional_interface_string(fields["raw-spelling"], label + " raw-spelling")
    if raw is not None:
        result["raw-spelling"] = raw
    return result


def _record_origin_from_interface(value: Any, label: str) -> Dict[str, Any]:
    fields = _sexpr_map(value, label, {"module", "span", "macro-origin"})
    span = _sexpr_vector(fields["span"], label + " span")
    if len(span) != 2:
        raise _error("interface %s span must contain two offsets" % label)
    start = _sexpr_integer(span[0], label + " span start")
    end = _sexpr_integer(span[1], label + " span end")
    if start > end:
        raise _error("interface %s span is reversed" % label)
    result = {
        "module": _sexpr_string(fields["module"], label + " module"),
        "span": [start, end],
    }
    macro_origin = _optional_interface_string(fields["macro-origin"], label + " macro-origin")
    if macro_origin is not None:
        result["macro-origin"] = macro_origin
    return result


def _record_from_interface(value: Any, label: str) -> Dict[str, Any]:
    fields = _sexpr_map(
        value,
        label,
        {
            "schema",
            "owner-binding-id",
            "owner-name",
            "module",
            "visibility",
            "stable-record-id",
            "record-body-hash",
            "fields",
            "index-claims",
            "origin",
        },
    )
    if _sexpr_keyword(fields["visibility"], label + " visibility") != "public":
        raise _error("interface %s contains a non-public owned record" % label)
    record_fields = []
    for item in _sexpr_vector(fields["fields"], label + " fields"):
        pair = _sexpr_vector(item, label + " field")
        if len(pair) != 2:
            raise _error("interface %s field must be a pair" % label)
        record_fields.append(
            [
                _sexpr_string(pair[0], label + " field name"),
                _static_datum_from_interface(pair[1], label + " field value"),
            ]
        )
    record = {
        "schema": _schema_identity_from_interface(fields["schema"], label + " schema"),
        "owner-binding-id": _sexpr_string(
            fields["owner-binding-id"], label + " owner-binding-id"
        ),
        "owner-name": _sexpr_string(fields["owner-name"], label + " owner-name"),
        "module": _sexpr_string(fields["module"], label + " module"),
        "public": True,
        "stable-record-id": _interface_hash(
            fields["stable-record-id"], label + " stable-record-id"
        ),
        "record-body-hash": _interface_hash(
            fields["record-body-hash"], label + " record-body-hash"
        ),
        "fields": record_fields,
        "index-claims": [
            _index_claim_from_interface(item, label + " index claim")
            for item in _sexpr_vector(fields["index-claims"], label + " index-claims")
        ],
        "origin": _record_origin_from_interface(fields["origin"], label + " origin"),
    }
    if record["module"] != record["origin"]["module"]:
        raise _error("interface %s record module differs from its origin" % label)
    return record


def _interface_projection(path: str, data: bytes) -> _InterfaceProjection:
    forms = _read_osri(data, path)
    wrapped: Dict[str, Any] = {}
    for form in forms:
        if (
            not isinstance(form, _SList)
            or len(form.items) != 2
            or not isinstance(form.items[0], _SAtom)
        ):
            raise _error("interface `%s` contains an invalid top-level form" % path)
        name = str(form.items[0])
        if name in wrapped:
            raise _error("interface `%s` repeats top-level form `%s`" % (path, name))
        wrapped[name] = form.items[1]
    expected_forms = {
        "osiris-interface/header",
        "osiris-interface/body",
        "osiris-interface/graph",
        "osiris-interface/hashes",
    }
    if set(wrapped) != expected_forms:
        raise _error("interface `%s` does not use the current four-form schema" % path)

    header = _sexpr_map(
        wrapped["osiris-interface/header"],
        "`%s` header" % path,
        {"format", "format-version", "compiler-abi", "language-abi"},
    )
    if (
        _sexpr_string(header["format"], "`%s` format" % path) != _INTERFACE_FORMAT
        or _sexpr_integer(header["format-version"], "`%s` format-version" % path)
        != _INTERFACE_FORMAT_VERSION
        or _sexpr_string(header["compiler-abi"], "`%s` compiler-abi" % path)
        != _INTERFACE_COMPILER_ABI
        or _sexpr_string(header["language-abi"], "`%s` language-abi" % path)
        != _INTERFACE_LANGUAGE_ABI
    ):
        raise _error("interface `%s` has an incompatible format or ABI" % path)

    body = _sexpr_map(
        wrapped["osiris-interface/body"],
        "`%s` body" % path,
        {
            "module",
            "metadata",
            "bindings",
            "aliases",
            "functions",
            "structs",
            "operator-instances",
            "macros",
            "phase-helpers",
            "static-schemas",
            "owned-records",
        },
    )
    module = _sexpr_string(body["module"], "`%s` module" % path)
    expected_module = path[: -len(".osri")].replace("/", ".")
    if module != expected_module:
        raise _error(
            "interface `%s` declares module `%s`, expected `%s`"
            % (path, module, expected_module)
        )

    hashes = _sexpr_map(
        wrapped["osiris-interface/hashes"],
        "`%s` hashes" % path,
        {"interface-body", "semantic-body", "tooling-body", "content-integrity"},
    )
    semantic_body_hash = _interface_hash(
        hashes["semantic-body"], "`%s` semantic body hash" % path
    )
    tooling_body_hash = _interface_hash(
        hashes["tooling-body"], "`%s` tooling body hash" % path
    )
    _interface_hash(hashes["interface-body"], "`%s` interface body hash" % path)
    _interface_hash(hashes["content-integrity"], "`%s` content integrity" % path)

    graph = _sexpr_map(
        wrapped["osiris-interface/graph"],
        "`%s` graph" % path,
        {
            "group-id",
            "members",
            "internal-edges",
            "external-dependencies",
            "semantic-interface-hash",
            "tooling-metadata-hash",
        },
    )
    semantic_interface_hash = _interface_hash(
        graph["semantic-interface-hash"], "`%s` semantic interface hash" % path
    )
    _interface_hash(graph["tooling-metadata-hash"], "`%s` tooling metadata hash" % path)
    members = []
    for item in _sexpr_vector(graph["members"], "`%s` graph members" % path):
        member = _sexpr_map(
            item,
            "`%s` graph member" % path,
            {"module", "semantic-body", "tooling-body"},
        )
        members.append(
            (
                _sexpr_string(member["module"], "`%s` graph member module" % path),
                _interface_hash(
                    member["semantic-body"], "`%s` graph member semantic body" % path
                ),
                _interface_hash(
                    member["tooling-body"], "`%s` graph member tooling body" % path
                ),
            )
        )
    own_members = [member for member in members if member[0] == module]
    if own_members != [(module, semantic_body_hash, tooling_body_hash)]:
        raise _error("interface `%s` graph does not identify its body exactly once" % path)

    records = tuple(
        _record_from_interface(item, "`%s` owned record" % path)
        for item in _sexpr_vector(body["owned-records"], "`%s` owned-records" % path)
    )
    for record in records:
        if record["module"] != module:
            raise _error("interface `%s` contains a record owned by another module" % path)
    return _InterfaceProjection(path, module, semantic_interface_hash, records)


def _canonical_json(value: Any) -> bytes:
    """Encode the sidecar's JSON subset in the same compact JCS shape.

    Sidecar object keys are fixed ASCII names and all user data is represented
    by tagged strings/arrays, so Python's compact UTF-8 encoder has the same
    ordering and escaping as the Rust encoder for this schema.
    """

    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")


def _json_without_duplicate_keys(data: bytes) -> Any:
    def hook(pairs: List[Tuple[str, Any]]) -> Dict[str, Any]:
        result: Dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise _error("records sidecar contains duplicate JSON member `%s`" % key)
            result[key] = value
        return result

    try:
        value = json.loads(data.decode("utf-8"), object_pairs_hook=hook)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise _error("records sidecar is not valid UTF-8 JSON: %s" % exc) from exc

    def validate_scalars(item: Any) -> None:
        if isinstance(item, str) and not _has_valid_unicode_scalars(item):
            raise _error("records sidecar contains an invalid Unicode scalar")
        if isinstance(item, dict):
            for key, child in item.items():
                validate_scalars(key)
                validate_scalars(child)
        elif isinstance(item, list):
            for child in item:
                validate_scalars(child)

    validate_scalars(value)
    return value


def _sidecar_occurrence_key(value: Any) -> Tuple[str, str, str, str, str, str]:
    if not isinstance(value, dict):
        raise _error("records sidecar occurrence must be an object")
    expected = (
        "distribution",
        "version",
        "interface-member-id",
        "semantic-interface-hash",
        "stable-record-id",
        "record-body-hash",
    )
    if (
        set(value) != set(expected)
        or any(not isinstance(value[key], str) or not value[key] for key in expected)
        or not _HASH_RE.fullmatch(value["semantic-interface-hash"])
        or not _HASH_RE.fullmatch(value["stable-record-id"])
        or not _HASH_RE.fullmatch(value["record-body-hash"])
    ):
        raise _error("records sidecar occurrence has an invalid shape")
    return tuple(value[key] for key in expected)  # type: ignore[return-value]


def _validate_sidecar(data: bytes) -> Dict[str, Any]:
    value = _json_without_duplicate_keys(data)
    if not isinstance(value, dict):
        raise _error("records sidecar root must be an object")
    expected = {
        "format-version",
        "interface-semantic-hashes",
        "record-identities",
        "record-set-hash",
        "records",
    }
    if set(value) != expected or value.get("format-version") != 1:
        raise _error("records sidecar has an unsupported schema")
    hashes = value["interface-semantic-hashes"]
    identities = value["record-identities"]
    records = value["records"]
    record_set_hash = value["record-set-hash"]
    if (
        not isinstance(hashes, list)
        or not all(isinstance(item, str) and _HASH_RE.fullmatch(item) for item in hashes)
        or hashes != sorted(set(hashes))
        or not isinstance(identities, list)
        or not isinstance(records, list)
        or len(identities) != len(records)
        or not isinstance(record_set_hash, str)
        or not _HASH_RE.fullmatch(record_set_hash)
    ):
        raise _error("records sidecar has invalid canonical fields")
    occurrence_keys: List[Tuple[str, str, str, str, str, str]] = []
    for identity, record in zip(identities, records):
        identity_key = _sidecar_occurrence_key(identity)
        if not isinstance(record, dict) or set(record) != {"occurrence", "record"}:
            raise _error("records sidecar record has an invalid shape")
        record_key = _sidecar_occurrence_key(record["occurrence"])
        if identity_key != record_key:
            raise _error("records sidecar identity differs between header and record")
        payload = record["record"]
        if not isinstance(payload, dict):
            raise _error("records sidecar record payload must be an object")
        if payload.get("public") is not True:
            raise _error("records sidecar contains a private or invalid record")
        occurrence_keys.append(identity_key)
    expected_set_hash = "sha256:" + hashlib.sha256(_canonical_json(records)).hexdigest()
    if record_set_hash != expected_set_hash:
        raise _error("records sidecar record-set-hash does not match its records")
    # Rust emits canonical bytes without a trailing newline.  Re-encoding with
    # fixed ASCII keys catches hand-edited/non-canonical sidecars before merge.
    if _canonical_json(value) != data:
        raise _error("records sidecar is not canonical JSON")
    if occurrence_keys != sorted(occurrence_keys):
        raise _error("records sidecar records are not in canonical order")
    if len(set(occurrence_keys)) != len(occurrence_keys):
        raise _error("records sidecar contains duplicate occurrences")
    return value


def _validate_sidecar_against_interfaces(
    sidecar: Mapping[str, Any],
    interfaces: Sequence[_InterfaceProjection],
    project: _Project,
) -> None:
    modules: Set[str] = set()
    expected_records: List[Dict[str, Any]] = []
    distribution = _normalise_name(_metadata_value(project.project.get("name"), "name"))
    version = _metadata_value(project.project.get("version"), "version")
    for interface in interfaces:
        if interface.module in modules:
            raise _error("osr emitted duplicate interface module `%s`" % interface.module)
        modules.add(interface.module)
        for record in interface.records:
            occurrence = {
                "distribution": distribution,
                "version": version,
                "interface-member-id": interface.module,
                "semantic-interface-hash": interface.semantic_interface_hash,
                "stable-record-id": record["stable-record-id"],
                "record-body-hash": record["record-body-hash"],
            }
            expected_records.append({"occurrence": occurrence, "record": record})
    expected_records.sort(key=lambda item: _sidecar_occurrence_key(item["occurrence"]))
    expected_hashes = sorted({interface.semantic_interface_hash for interface in interfaces})
    if sidecar["interface-semantic-hashes"] != expected_hashes:
        raise _error("records sidecar interface hashes differ from emitted `.osri` files")
    if sidecar["records"] != expected_records:
        raise _error("records sidecar cannot be reconstructed from emitted `.osri` files")
    expected_identities = [item["occurrence"] for item in expected_records]
    if sidecar["record-identities"] != expected_identities:
        raise _error("records sidecar identities differ from emitted `.osri` files")


def _collect_static_files(project: _Project) -> Dict[str, bytes]:
    files: Dict[str, bytes] = {}
    for source_root in project.source_roots:
        for path in sorted(source_root.rglob("*"), key=lambda item: item.as_posix()):
            if path.is_symlink():
                raise _error("source file `%s` must not be a symlink" % path)
            if not path.is_file():
                continue
            if path.suffix in (".osr", ".pyc") or "__pycache__" in path.parts:
                continue
            if not _file_inside(project.root, path):
                raise _error("source file `%s` escapes the project" % path)
            relative = path.relative_to(source_root)
            _add_file(files, _safe_archive_path(relative), path.read_bytes())
    return files


def _collect_compiler_output(
    project: _Project,
    config_settings: Optional[Mapping[str, Any]],
    static_files: Dict[str, bytes],
) -> Tuple[List[str], Optional[Tuple[str, bytes]]]:
    source_files: List[Path] = []
    for source_root in project.source_roots:
        source_files.extend(
            path
            for path in source_root.rglob("*.osr")
            if path.is_file() and _file_inside(project.root, path)
        )
    if any(path.is_symlink() for source_root in project.source_roots for path in source_root.rglob("*.osr")):
        raise _error("Osiris source files must not be symlinks")
    source_files.sort(key=lambda item: item.relative_to(project.root).as_posix())
    if not source_files:
        raise _error("no .osr source files found under [tool.osiris].source")
    command = _compiler_command(config_settings, project)
    interfaces: List[str] = []
    interface_projections: List[_InterfaceProjection] = []
    record_artifacts: List[Tuple[str, bytes]] = []
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
        for path in produced:
            relative = path.relative_to(output)
            name = relative.as_posix()
            if name.endswith(".records.json"):
                record_artifacts.append((name, path.read_bytes()))
                continue
            if not (name.endswith(".py") or name.endswith(".osri") or name.endswith(".py.map")):
                raise _error("osr produced unsupported artifact `%s`" % name)
            archive_path = _safe_archive_path(relative)
            artifact = path.read_bytes()
            _add_file(static_files, archive_path, artifact)
            if name.endswith(".osri"):
                interfaces.append(archive_path)
                interface_projections.append(_interface_projection(archive_path, artifact))
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
        return sorted(set(interfaces)), None
    return sorted(set(interfaces)), (record_artifacts[0][0], record_artifacts[0][1])


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


def _marker_text(interfaces: Sequence[str], records_path: Optional[str], records_data: Optional[bytes]) -> str:
    lines = [
        "schema = %d" % _MARKER_SCHEMA,
        "compiler_abi = %d" % _MARKER_COMPILER_ABI,
        "language_abi = %d" % _MARKER_LANGUAGE_ABI,
    ]
    if records_path is not None and records_data is not None:
        digest = "sha256:" + hashlib.sha256(records_data).hexdigest()
        lines.extend([
            "records = " + json.dumps(records_path, ensure_ascii=False),
            "records_hash = " + json.dumps(digest),
        ])
    for interface in sorted(interfaces):
        identifier = interface[: -len(".osri")].replace("/", ".")
        lines.extend([
            "",
            "[[extension]]",
            "id = " + json.dumps(identifier, ensure_ascii=False),
            "interface = " + json.dumps(interface, ensure_ascii=False),
        ])
    return "\n".join(lines) + "\n"


def _build_files(project: _Project, config_settings: Optional[Mapping[str, Any]]) -> _BuildFiles:
    files = _collect_static_files(project)
    interfaces, record_artifact = _collect_compiler_output(project, config_settings, files)
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
    return _BuildFiles(files=files, interfaces=interfaces, records_path=records_path)


def _metadata_value(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise _error("[project].%s must be a non-empty string" % label)
    if "\r" in value or "\n" in value:
        raise _error("[project].%s contains a newline" % label)
    return value.strip()


def _distribution_name(project: _Project) -> Tuple[str, str]:
    name = _metadata_value(project.project.get("name"), "name")
    normalized = _normalise_name(name)
    if not normalized:
        raise _error("[project].name normalizes to an empty value")
    return name, normalized


def _distribution_version(project: _Project) -> Tuple[str, str]:
    version = _metadata_value(project.project.get("version"), "version")
    if any(char in version for char in "/\\"):
        raise _error("[project].version contains a path separator")
    release = _version_key(version)
    return version, ".".join(str(part) for part in release)


def _dist_info_name(normalized_name: str) -> str:
    """PEP 427 uses underscores inside the ``.dist-info`` directory name."""

    return normalized_name.replace("-", "_")


def _metadata_bytes(project: _Project) -> bytes:
    name, _ = _distribution_name(project)
    version, _ = _distribution_version(project)
    lines = ["Metadata-Version: 2.3", "Name: " + name, "Version: " + version]
    for field, key in (("Summary", "description"), ("Requires-Python", "requires-python")):
        value = project.project.get(key)
        if value:
            lines.append(field + ": " + _metadata_value(value, key))
    authors = project.project.get("authors", [])
    if isinstance(authors, list):
        for author in authors:
            if isinstance(author, dict):
                if author.get("name"):
                    lines.append("Author: " + _metadata_value(author["name"], "authors.name"))
                if author.get("email"):
                    lines.append("Author-email: " + _metadata_value(author["email"], "authors.email"))
    license_value = project.project.get("license")
    if isinstance(license_value, dict) and license_value.get("text"):
        lines.append("License: " + _metadata_value(license_value["text"], "license.text"))
    elif isinstance(license_value, str):
        lines.append("License: " + _metadata_value(license_value, "license"))
    for requirement in project.locked_requirements:
        lines.append("Requires-Dist: " + _metadata_value(requirement, "dependency"))
    readme = project.project.get("readme")
    body = ""
    if isinstance(readme, str):
        readme_path = (project.root / _validate_relative_path(readme, "project.readme")).resolve()
        if not _file_inside(project.root, readme_path) or not readme_path.is_file():
            raise _error("[project].readme file `%s` does not exist inside project" % readme)
        body = readme_path.read_text(encoding="utf-8")
        suffix = readme_path.suffix.lower()
        content_type = (
            "text/markdown" if suffix in (".md", ".markdown") else "text/plain"
        )
        lines.append("Description-Content-Type: " + content_type)
    if body:
        lines.extend(["", body.rstrip("\n")])
    return ("\n".join(lines) + "\n").encode("utf-8")


def _wheel_metadata_bytes() -> bytes:
    return (
        "Wheel-Version: 1.0\nGenerator: osiris-build %s\n"
        "Root-Is-Purelib: true\nTag: py3-none-any\n" % __version__
    ).encode("utf-8")


def _prepared_metadata_files(
    metadata_directory: Path,
    dist_info: str,
    expected_metadata: bytes,
    expected_wheel: bytes,
) -> Dict[str, bytes]:
    if metadata_directory.is_symlink() or not metadata_directory.is_dir():
        raise _error("metadata_directory must be an existing, non-symlink directory")
    source = metadata_directory
    if source.name != dist_info:
        nested = source / dist_info
        if nested.is_symlink() or not nested.is_dir():
            raise _error(
                "metadata_directory does not contain expected `%s`" % dist_info
            )
        source = nested
    metadata_path = source / "METADATA"
    wheel_path = source / "WHEEL"
    for required in (metadata_path, wheel_path):
        if required.is_symlink() or not required.is_file():
            raise _error("prepared metadata is missing regular file `%s`" % required.name)
    if metadata_path.read_bytes() != expected_metadata:
        raise _error("prepared METADATA differs from the current project and lock")
    if wheel_path.read_bytes() != expected_wheel:
        raise _error("prepared WHEEL metadata has an incompatible backend ABI or tag")

    result: Dict[str, bytes] = {}
    for path in sorted(source.rglob("*"), key=lambda item: item.as_posix()):
        if path.is_symlink():
            raise _error("prepared metadata must not contain symlink `%s`" % path)
        if path.is_dir():
            continue
        if not path.is_file():
            raise _error("prepared metadata contains non-regular path `%s`" % path)
        relative = _safe_archive_path(path.relative_to(source))
        if relative == "RECORD":
            raise _error("prepared metadata must not provide a wheel RECORD")
        result[relative] = path.read_bytes()
    return result


def _wheel_bytes(
    project: _Project,
    built: _BuildFiles,
    metadata_directory: Optional[Path],
) -> Tuple[str, bytes]:
    name, normalized_name = _distribution_name(project)
    version, normalized_version = _distribution_version(project)
    dist_info = "%s-%s.dist-info" % (_dist_info_name(normalized_name), normalized_version)
    wheel_files: Dict[str, bytes] = dict(built.files)
    records_data = wheel_files.get(built.records_path) if built.records_path else None
    marker = _marker_text(built.interfaces, built.records_path, records_data)
    wheel_files[dist_info + "/osiris.toml"] = marker.encode("utf-8")
    metadata = _metadata_bytes(project)
    wheel_metadata = _wheel_metadata_bytes()
    wheel_files[dist_info + "/METADATA"] = metadata
    wheel_files[dist_info + "/WHEEL"] = wheel_metadata
    if metadata_directory is not None:
        prepared = _prepared_metadata_files(
            metadata_directory, dist_info, metadata, wheel_metadata
        )
        for relative, contents in prepared.items():
            _add_file(wheel_files, dist_info + "/" + relative, contents)
    records_rows: List[str] = []
    for archive_path in sorted(wheel_files):
        digest = base64.urlsafe_b64encode(
            hashlib.sha256(wheel_files[archive_path]).digest()
        ).decode("ascii").rstrip("=")
        records_rows.append("%s,sha256=%s,%d" % (archive_path, digest, len(wheel_files[archive_path])))
    records_rows.append(dist_info + "/RECORD,,")
    wheel_files[dist_info + "/RECORD"] = ("\n".join(records_rows) + "\n").encode("utf-8")
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for archive_path in sorted(wheel_files):
            info = zipfile.ZipInfo(archive_path, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.create_system = 3
            info.external_attr = (0o644 & 0xFFFF) << 16
            archive.writestr(info, wheel_files[archive_path])
    filename = "%s-%s-py3-none-any.whl" % (
        _dist_info_name(normalized_name),
        normalized_version,
    )
    return filename, output.getvalue()


def _write_atomic(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp-%d" % os.getpid())
    try:
        temporary.write_bytes(data)
        os.replace(str(temporary), str(path))
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def _sdist_inputs(project: _Project) -> Dict[str, bytes]:
    files: Dict[str, bytes] = {}
    # Build from an explicit, source-focused set rather than accidentally
    # embedding .git, target, virtualenvs, or generated wheels.
    candidates: Set[Path] = {project.pyproject_path}
    lock_path = project.root / "uv.lock"
    if lock_path.is_file():
        candidates.add(lock_path)
    readme = project.project.get("readme")
    if isinstance(readme, str):
        candidates.add((project.root / _validate_relative_path(readme, "project.readme")).resolve())
    for source_root in project.source_roots:
        for path in source_root.rglob("*"):
            if path.is_symlink():
                raise _error("source file `%s` must not be a symlink" % path)
            if path.is_file():
                candidates.add(path)
    for extra in ("LICENSE", "LICENSE.txt", "COPYING", "README", "README.md"):
        candidate = project.root / extra
        if candidate.is_file():
            candidates.add(candidate)
    for path in sorted(candidates, key=lambda item: item.as_posix()):
        if not _file_inside(project.root, path) or "__pycache__" in path.parts or path.suffix == ".pyc":
            continue
        relative = path.relative_to(project.root)
        _add_file(files, _safe_archive_path(relative), path.read_bytes())
    constraints = ("\n".join(project.locked_requirements) + "\n").encode("utf-8")
    _add_file(files, "osiris-build-constraints.txt", constraints)
    hashes = [
        "pyproject.toml sha256:%s" % hashlib.sha256(project.pyproject_bytes).hexdigest(),
        "uv.lock sha256:%s" % hashlib.sha256(project.lock_bytes).hexdigest(),
    ]
    _add_file(files, "osiris-build-inputs.sha256", ("\n".join(hashes) + "\n").encode("ascii"))
    return files


def _sdist_bytes(project: _Project, files: Mapping[str, bytes]) -> Tuple[str, bytes]:
    _, normalized_name = _distribution_name(project)
    _, normalized_version = _distribution_version(project)
    root_name = "%s-%s" % (normalized_name, normalized_version)
    output = io.BytesIO()
    with gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
            for relative in sorted(files):
                data = files[relative]
                info = tarfile.TarInfo(root_name + "/" + relative)
                info.size = len(data)
                info.mtime = 0
                info.uid = 0
                info.gid = 0
                info.uname = ""
                info.gname = ""
                info.mode = 0o644
                archive.addfile(info, io.BytesIO(data))
    return root_name + ".tar.gz", output.getvalue()


def get_requires_for_build_wheel(config_settings: Optional[Mapping[str, Any]] = None) -> List[str]:
    """Return only exact requirements represented by the current uv.lock."""

    project = _load_project(require_lock=True, enforce_runtime_python=True)
    return list(project.locked_requirements)


def get_requires_for_build_sdist(config_settings: Optional[Mapping[str, Any]] = None) -> List[str]:
    project = _load_project(require_lock=True, enforce_runtime_python=True)
    return list(project.locked_requirements)


def prepare_metadata_for_build_wheel(
    metadata_directory: str,
    config_settings: Optional[Mapping[str, Any]] = None,
) -> str:
    project = _load_project(require_lock=True, enforce_runtime_python=True)
    _, normalized_name = _distribution_name(project)
    _, normalized_version = _distribution_version(project)
    dist_info = "%s-%s.dist-info" % (_dist_info_name(normalized_name), normalized_version)
    metadata_root = Path(metadata_directory).absolute()
    if metadata_root.exists() and (metadata_root.is_symlink() or not metadata_root.is_dir()):
        raise _error("metadata_directory must be a non-symlink directory")
    metadata_root.mkdir(parents=True, exist_ok=True)
    destination = metadata_root / dist_info
    if destination.exists() and (destination.is_symlink() or not destination.is_dir()):
        raise _error("prepared metadata destination `%s` is not a regular directory" % dist_info)
    destination.mkdir(parents=True, exist_ok=True)
    (destination / "METADATA").write_bytes(_metadata_bytes(project))
    (destination / "WHEEL").write_bytes(_wheel_metadata_bytes())
    return dist_info


def build_wheel(
    wheel_directory: str,
    config_settings: Optional[Mapping[str, Any]] = None,
    metadata_directory: Optional[str] = None,
) -> str:
    project = _load_project(require_lock=True, enforce_runtime_python=True)
    built = _build_files(project, config_settings)
    metadata_path = Path(metadata_directory).absolute() if metadata_directory else None
    filename, data = _wheel_bytes(project, built, metadata_path)
    destination = Path(wheel_directory).resolve() / filename
    _write_atomic(destination, data)
    return filename


def build_sdist(
    sdist_directory: str,
    config_settings: Optional[Mapping[str, Any]] = None,
) -> str:
    project = _load_project(require_lock=True, enforce_runtime_python=True)
    filename, data = _sdist_bytes(project, _sdist_inputs(project))
    destination = Path(sdist_directory).resolve() / filename
    _write_atomic(destination, data)
    return filename
