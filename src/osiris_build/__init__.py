"""Deterministic PEP 517 backend for Osiris projects.

Dependency resolution remains owned by uv. This package only validates locked
inputs, invokes ``osr``, and assembles reproducible Python artifacts.
"""

from . import _requirements
from ._api import (
    build_sdist,
    build_wheel,
    get_requires_for_build_sdist,
    get_requires_for_build_wheel,
    prepare_metadata_for_build_wheel,
)
from ._model import BACKEND_VERSION, BackendError

__version__ = BACKEND_VERSION
__all__ = [
    "BackendError",
    "build_sdist",
    "build_wheel",
    "get_requires_for_build_sdist",
    "get_requires_for_build_wheel",
    "prepare_metadata_for_build_wheel",
]

# Compatibility for existing backend tests and callers of private diagnostics.
platform = _requirements.platform
sys = _requirements.sys
_marker_applies = _requirements._marker_applies
_satisfies = _requirements._satisfies
