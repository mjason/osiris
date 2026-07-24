"""Internal value objects shared by the build backend layers."""

from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

BACKEND_VERSION = "0.3.0"


class BackendError(RuntimeError):
    """A configuration, lock, compiler, or packaging error."""


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
    config_path: Path
    config_bytes: bytes
    document: Dict[str, Any]
    project: Dict[str, Any]
    osiris: Dict[str, Any]
    source_roots: List[Path]
    output_dir: Path
    exclude_patterns: List[str]
    target_python: Tuple[int, int]
    requirements: List[str]
    locked_requirements: List[str]
    lock_bytes: bytes
    lock_document: Dict[str, Any]


@dataclass
class _BuildFiles:
    files: Dict[str, bytes]
    interfaces: List["_ExtensionArtifact"]
    support_manifests: List[str]
    records_path: Optional[str]


@dataclass(frozen=True)
class _InterfaceProjection:
    path: str
    module: str
    semantic_interface_hash: str
    language_version: str
    standard_library_abi: int
    linkable_helper_format: int
    python_target: str
    records: Tuple[Dict[str, Any], ...]


@dataclass(frozen=True)
class _ExtensionArtifact:
    identifier: str
    interface: str
    interface_hash: str
    source: str
    source_hash: str
    source_map: str
    source_map_hash: str
