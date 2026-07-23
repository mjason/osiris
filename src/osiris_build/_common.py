"""Small dependency-free helpers shared by build backend layers."""

import re
from typing import Any

from ._model import BackendError


def _error(message: str) -> BackendError:
    return BackendError("osiris-build: " + message)


def _normalise_name(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def _metadata_value(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise _error("[project].%s must be a non-empty string" % label)
    if "\r" in value or "\n" in value:
        raise _error("[project].%s contains a newline" % label)
    return value.strip()
