"""Conservative requirement, marker, version, and uv.lock handling."""

import os
import platform
import re
import sys
from typing import Any, Dict, List, Mapping, Optional, Sequence, Set, Tuple

from ._common import _error, _normalise_name
from ._model import BackendError, _LockedPackage, _Requirement


_NAME_RE = re.compile(r"^\s*([A-Za-z0-9][A-Za-z0-9._-]*)(?:\[([^\]]+)\])?\s*(.*)$")
_COMPARATOR_RE = re.compile(r"^(===|==|!=|~=|>=|<=|>|<)\s*([^,]+)$")
_MARKER_ATOM_RE = re.compile(
    r"^\s*(python_version|python_full_version|os_name|sys_platform|platform_machine|"
    r"platform_python_implementation|platform_system|implementation_name)\s*"
    r"(==|!=|>=|<=|>|<|in|not\s+in)\s*(['\"])(.*?)\3\s*$",
    re.IGNORECASE,
)
_NUMERIC_RELEASE_RE = re.compile(r"^\d+(?:\.\d+)*$")
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
