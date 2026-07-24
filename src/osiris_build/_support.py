"""Validation and wheel-local rewriting of compiler-linked support provenance."""

import json
import re
from pathlib import PurePosixPath
from typing import Any, Dict, Mapping, Tuple

from ._common import _error, _json_object, _sha256
from ._interface import _LANGUAGE_VERSION, _LINKABLE_HELPER_FORMAT, _STANDARD_LIBRARY_ABI


_HASH = re.compile(r"sha256:[0-9a-f]{64}")
_IDENTITY_FIELDS = {"source", "sourceHash", "generated", "buildHash"}


def _content_hash(value: Any, path: str, field: str) -> str:
    if not isinstance(value, str) or _HASH.fullmatch(value) is None:
        raise _error("linked support manifest `%s` has an invalid %s" % (path, field))
    return value


def _source_map_identity(data: bytes, path: str) -> Dict[str, str]:
    value = _json_object(data, "source map `%s`" % path)
    identity = {
        "source": value.get("source"),
        "sourceHash": value.get("source_hash"),
        "generated": value.get("generated"),
        "buildHash": value.get("build_hash"),
    }
    if not isinstance(identity["source"], str) or not identity["source"]:
        raise _error("source map `%s` has an invalid source identity" % path)
    if not isinstance(identity["generated"], str) or not identity["generated"]:
        raise _error("source map `%s` has an invalid generated identity" % path)
    _content_hash(identity["sourceHash"], path, "source hash")
    _content_hash(identity["buildHash"], path, "build hash")
    return identity  # type: ignore[return-value]


def _validate_header(value: Dict[str, Any], path: str, target_python: Tuple[int, int]) -> None:
    required = {
        "schema", "languageVersion", "pythonTarget", "standardLibraryAbi",
        "standardLibrarySemanticHash", "helperFormat", "reachableBindingIds",
        "helperHashes", "fileHashes", "sourceMaps",
    }
    if set(value) != required or value["schema"] != "osiris-linked-support/v1":
        raise _error("linked support manifest `%s` has an unsupported schema" % path)
    if value["languageVersion"] != _LANGUAGE_VERSION:
        raise _error("linked support manifest `%s` has an incompatible language version" % path)
    if value["pythonTarget"] != "%d.%d" % target_python:
        raise _error("linked support manifest `%s` targets an incompatible Python version" % path)
    if value["standardLibraryAbi"] != _STANDARD_LIBRARY_ABI:
        raise _error("linked support manifest `%s` has an incompatible standard-library ABI" % path)
    if value["helperFormat"] != _LINKABLE_HELPER_FORMAT:
        raise _error("linked support manifest `%s` has an incompatible helper format" % path)
    _content_hash(value["standardLibrarySemanticHash"], path, "standardLibrarySemanticHash")


def _validate_hashes(value: Dict[str, Any], path: str, produced: Mapping[str, bytes]) -> None:
    bindings = value["reachableBindingIds"]
    if (
        not isinstance(bindings, list)
        or bindings != sorted(set(bindings))
        or not all(isinstance(item, str) and item for item in bindings)
    ):
        raise _error("linked support manifest `%s` binding IDs must be sorted and unique" % path)
    for field in ("helperHashes", "fileHashes"):
        hashes = value[field]
        if not isinstance(hashes, dict) or not hashes:
            raise _error("linked support manifest `%s` %s must be a non-empty object" % (path, field))
        for name, digest in hashes.items():
            if not isinstance(name, str) or not name:
                raise _error("linked support manifest `%s` contains an invalid hash key" % path)
            _content_hash(digest, path, field)
    runtime_root = str(PurePosixPath(path).parent) + "/"
    runtime_files = {
        name: contents
        for name, contents in produced.items()
        if name.startswith(runtime_root) and name.endswith(".py")
    }
    if set(value["fileHashes"]) != set(runtime_files):
        raise _error("linked support manifest `%s` does not cover its Python support files" % path)
    for name, contents in runtime_files.items():
        if value["fileHashes"][name] != _sha256(contents):
            raise _error("linked support manifest `%s` has a stale hash for `%s`" % (path, name))


def _rewrite_identities(
    value: Dict[str, Any],
    path: str,
    produced: Mapping[str, bytes],
    packaged: Mapping[str, bytes],
) -> None:
    identities = value["sourceMaps"]
    if not isinstance(identities, list) or not identities:
        raise _error("linked support manifest `%s` sourceMaps must be a non-empty array" % path)
    rewritten = []
    seen = set()
    for identity in identities:
        if not isinstance(identity, dict) or set(identity) != _IDENTITY_FIELDS:
            raise _error("linked support manifest `%s` contains an invalid source-map identity" % path)
        generated = identity.get("generated")
        if not isinstance(generated, str) or not generated:
            raise _error("linked support manifest `%s` contains an invalid generated path" % path)
        map_path = generated + ".map"
        raw = produced.get(map_path)
        final = packaged.get(map_path)
        if raw is None or final is None:
            raise _error("linked support manifest `%s` references missing source map `%s`" % (path, map_path))
        raw_identity = _source_map_identity(raw, map_path)
        if identity != raw_identity:
            raise _error("linked support manifest `%s` contains a stale source-map identity" % path)
        final_identity = _source_map_identity(final, map_path)
        stable_key = tuple(final_identity[field] for field in sorted(_IDENTITY_FIELDS))
        if stable_key in seen:
            raise _error("linked support manifest `%s` repeats a source-map identity" % path)
        seen.add(stable_key)
        rewritten.append(final_identity)
    value["sourceMaps"] = sorted(
        rewritten,
        key=lambda item: (item["source"], item["generated"], item["buildHash"]),
    )


def _rewrite_support_manifest(
    path: str,
    data: bytes,
    target_python: Tuple[int, int],
    produced: Mapping[str, bytes],
    packaged: Mapping[str, bytes],
) -> bytes:
    value = _json_object(data, "linked support manifest `%s`" % path)
    _validate_header(value, path, target_python)
    _validate_hashes(value, path, produced)
    _rewrite_identities(value, path, produced, packaged)
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n").encode("utf-8")
