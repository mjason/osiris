"""PEP 517 hook implementations over the internal build layers."""

from pathlib import Path
from typing import Any, List, Mapping, Optional

from ._common import _error
from ._project import _load_project
from ._wheel import (
    _build_files,
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


def _locked_requirements() -> List[str]:
    project = _load_project(require_lock=True, enforce_runtime_python=True)
    return list(project.locked_requirements)


def get_requires_for_build_wheel(config_settings: Optional[Mapping[str, Any]] = None) -> List[str]:
    """Return only exact requirements represented by the current uv.lock."""

    return _locked_requirements()


def get_requires_for_build_sdist(config_settings: Optional[Mapping[str, Any]] = None) -> List[str]:
    return _locked_requirements()


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
