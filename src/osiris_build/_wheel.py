"""Compiler invocation plus deterministic wheel and sdist assembly."""

import base64
import fnmatch
import gzip
import hashlib
import io
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path, PurePosixPath
from typing import Any, Dict, List, Mapping, Optional, Sequence, Set, Tuple, Union

from ._common import _error, _metadata_value, _normalise_name
from ._interface import (
    _INTERFACE_COMPILER_ABI,
    _INTERFACE_LANGUAGE_ABI,
    _interface_projection,
    _validate_sidecar,
    _validate_sidecar_against_interfaces,
)
from ._model import BACKEND_VERSION, _BuildFiles, _InterfaceProjection, _Project
from ._project import _validate_relative_path
from ._requirements import _version_key


_MARKER_SCHEMA = 1
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
            if path.is_file() and _file_inside(project.root, path) and not _is_excluded(project, path)
        )
    if any(path.is_symlink() for source_root in project.source_roots for path in source_root.rglob("*.osr")):
        raise _error("Osiris source files must not be symlinks")
    source_files.sort(key=lambda item: item.relative_to(project.root).as_posix())
    if not source_files:
        raise _error("no .osr source files found under osiris.jsonc source roots")
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
        "Root-Is-Purelib: true\nTag: py3-none-any\n" % BACKEND_VERSION
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
    candidates: Set[Path] = {project.pyproject_path, project.config_path}
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
                if _is_excluded(project, path):
                    continue
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
        "osiris.jsonc sha256:%s" % hashlib.sha256(project.config_bytes).hexdigest(),
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
