"""Small dependency-free helpers shared by build backend layers."""

import hashlib
import json
import re
from typing import Any, Dict, List, Tuple

from ._model import BackendError


def _error(message: str) -> BackendError:
    return BackendError("osiris-build: " + message)


def _normalise_name(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def _sha256(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def _json_object(data: bytes, label: str) -> Dict[str, Any]:
    def pairs(items: List[Tuple[str, Any]]) -> Dict[str, Any]:
        result: Dict[str, Any] = {}
        for key, value in items:
            if key in result:
                raise _error("%s repeats JSON member `%s`" % (label, key))
            result[key] = value
        return result

    try:
        value = json.loads(data.decode("utf-8"), object_pairs_hook=pairs)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise _error("%s is not valid UTF-8 JSON: %s" % (label, exc)) from exc
    if not isinstance(value, dict):
        raise _error("%s must contain a JSON object" % label)
    return value


def _metadata_value(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise _error("[project].%s must be a non-empty string" % label)
    if "\r" in value or "\n" in value:
        raise _error("[project].%s contains a newline" % label)
    return value.strip()
