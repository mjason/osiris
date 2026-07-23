"""Internal value objects shared by the build backend layers."""

from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

BACKEND_VERSION = "0.1.0"


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
