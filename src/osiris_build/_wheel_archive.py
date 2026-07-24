"""Deterministic wheel and sdist metadata and archive assembly."""

import base64
import gzip
import hashlib
import io
import os
import tarfile
import zipfile
from pathlib import Path
from typing import Dict, List, Mapping, Optional, Set, Tuple

from ._common import _error, _metadata_value, _normalise_name
from ._model import BACKEND_VERSION, _BuildFiles, _Project
from ._project import _validate_relative_path
from ._requirements import _version_key
from ._wheel import (
    _add_file,
    _file_inside,
    _is_excluded,
    _marker_text,
    _safe_archive_path,
)


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
        content_type = "text/markdown" if suffix in (".md", ".markdown") else "text/plain"
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
            raise _error("metadata_directory does not contain expected `%s`" % dist_info)
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
    _, normalized_name = _distribution_name(project)
    _, normalized_version = _distribution_version(project)
    dist_info = "%s-%s.dist-info" % (_dist_info_name(normalized_name), normalized_version)
    wheel_files: Dict[str, bytes] = dict(built.files)
    records_data = wheel_files.get(built.records_path) if built.records_path else None
    marker = _marker_text(project, built, records_data)
    wheel_files[dist_info + "/osiris.toml"] = marker.encode("utf-8")
    metadata = _metadata_bytes(project)
    wheel_metadata = _wheel_metadata_bytes()
    wheel_files[dist_info + "/METADATA"] = metadata
    wheel_files[dist_info + "/WHEEL"] = wheel_metadata
    if metadata_directory is not None:
        prepared = _prepared_metadata_files(metadata_directory, dist_info, metadata, wheel_metadata)
        for relative, contents in prepared.items():
            _add_file(wheel_files, dist_info + "/" + relative, contents)
    records_rows: List[str] = []
    for archive_path in sorted(wheel_files):
        digest = base64.urlsafe_b64encode(
            hashlib.sha256(wheel_files[archive_path]).digest()
        ).decode("ascii").rstrip("=")
        records_rows.append(
            "%s,sha256=%s,%d" % (archive_path, digest, len(wheel_files[archive_path]))
        )
    records_rows.append(dist_info + "/RECORD,,")
    wheel_files[dist_info + "/RECORD"] = ("\n".join(records_rows) + "\n").encode("utf-8")
    output = io.BytesIO()
    with zipfile.ZipFile(
        output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9
    ) as archive:
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
