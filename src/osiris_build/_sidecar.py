"""Canonical records-sidecar decoding and interface reconstruction."""

import hashlib
import json
from typing import Any, Dict, List, Mapping, Sequence, Set, Tuple

from ._common import _error, _metadata_value, _normalise_name
from ._model import _InterfaceProjection, _Project
from ._sexpr import _HASH_RE, _has_valid_unicode_scalars


def _canonical_json(value: Any) -> bytes:
    """Encode the sidecar's JSON subset in the same compact JCS shape.

    Sidecar object keys are fixed ASCII names and all user data is represented
    by tagged strings/arrays, so Python's compact UTF-8 encoder has the same
    ordering and escaping as the Rust encoder for this schema.
    """

    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")


def _json_without_duplicate_keys(data: bytes) -> Any:
    def hook(pairs: List[Tuple[str, Any]]) -> Dict[str, Any]:
        result: Dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise _error("records sidecar contains duplicate JSON member `%s`" % key)
            result[key] = value
        return result

    try:
        value = json.loads(data.decode("utf-8"), object_pairs_hook=hook)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise _error("records sidecar is not valid UTF-8 JSON: %s" % exc) from exc

    def validate_scalars(item: Any) -> None:
        if isinstance(item, str) and not _has_valid_unicode_scalars(item):
            raise _error("records sidecar contains an invalid Unicode scalar")
        if isinstance(item, dict):
            for key, child in item.items():
                validate_scalars(key)
                validate_scalars(child)
        elif isinstance(item, list):
            for child in item:
                validate_scalars(child)

    validate_scalars(value)
    return value


def _sidecar_occurrence_key(value: Any) -> Tuple[str, str, str, str, str, str]:
    if not isinstance(value, dict):
        raise _error("records sidecar occurrence must be an object")
    expected = (
        "distribution",
        "version",
        "interface-member-id",
        "semantic-interface-hash",
        "stable-record-id",
        "record-body-hash",
    )
    if (
        set(value) != set(expected)
        or any(not isinstance(value[key], str) or not value[key] for key in expected)
        or not _HASH_RE.fullmatch(value["semantic-interface-hash"])
        or not _HASH_RE.fullmatch(value["stable-record-id"])
        or not _HASH_RE.fullmatch(value["record-body-hash"])
    ):
        raise _error("records sidecar occurrence has an invalid shape")
    return tuple(value[key] for key in expected)  # type: ignore[return-value]


def _validate_sidecar(data: bytes) -> Dict[str, Any]:
    value = _json_without_duplicate_keys(data)
    if not isinstance(value, dict):
        raise _error("records sidecar root must be an object")
    expected = {
        "format-version",
        "interface-semantic-hashes",
        "record-identities",
        "record-set-hash",
        "records",
    }
    if set(value) != expected or value.get("format-version") != 1:
        raise _error("records sidecar has an unsupported schema")
    hashes = value["interface-semantic-hashes"]
    identities = value["record-identities"]
    records = value["records"]
    record_set_hash = value["record-set-hash"]
    if (
        not isinstance(hashes, list)
        or not all(isinstance(item, str) and _HASH_RE.fullmatch(item) for item in hashes)
        or hashes != sorted(set(hashes))
        or not isinstance(identities, list)
        or not isinstance(records, list)
        or len(identities) != len(records)
        or not isinstance(record_set_hash, str)
        or not _HASH_RE.fullmatch(record_set_hash)
    ):
        raise _error("records sidecar has invalid canonical fields")
    occurrence_keys: List[Tuple[str, str, str, str, str, str]] = []
    for identity, record in zip(identities, records):
        identity_key = _sidecar_occurrence_key(identity)
        if not isinstance(record, dict) or set(record) != {"occurrence", "record"}:
            raise _error("records sidecar record has an invalid shape")
        record_key = _sidecar_occurrence_key(record["occurrence"])
        if identity_key != record_key:
            raise _error("records sidecar identity differs between header and record")
        payload = record["record"]
        if not isinstance(payload, dict):
            raise _error("records sidecar record payload must be an object")
        if payload.get("public") is not True:
            raise _error("records sidecar contains a private or invalid record")
        occurrence_keys.append(identity_key)
    expected_set_hash = "sha256:" + hashlib.sha256(_canonical_json(records)).hexdigest()
    if record_set_hash != expected_set_hash:
        raise _error("records sidecar record-set-hash does not match its records")
    # Rust emits canonical bytes without a trailing newline.  Re-encoding with
    # fixed ASCII keys catches hand-edited/non-canonical sidecars before merge.
    if _canonical_json(value) != data:
        raise _error("records sidecar is not canonical JSON")
    if occurrence_keys != sorted(occurrence_keys):
        raise _error("records sidecar records are not in canonical order")
    if len(set(occurrence_keys)) != len(occurrence_keys):
        raise _error("records sidecar contains duplicate occurrences")
    return value


def _validate_sidecar_against_interfaces(
    sidecar: Mapping[str, Any],
    interfaces: Sequence[_InterfaceProjection],
    project: _Project,
) -> None:
    modules: Set[str] = set()
    expected_records: List[Dict[str, Any]] = []
    distribution = _normalise_name(_metadata_value(project.project.get("name"), "name"))
    version = _metadata_value(project.project.get("version"), "version")
    for interface in interfaces:
        if interface.module in modules:
            raise _error("osr emitted duplicate interface module `%s`" % interface.module)
        modules.add(interface.module)
        for record in interface.records:
            occurrence = {
                "distribution": distribution,
                "version": version,
                "interface-member-id": interface.module,
                "semantic-interface-hash": interface.semantic_interface_hash,
                "stable-record-id": record["stable-record-id"],
                "record-body-hash": record["record-body-hash"],
            }
            expected_records.append({"occurrence": occurrence, "record": record})
    expected_records.sort(key=lambda item: _sidecar_occurrence_key(item["occurrence"]))
    expected_hashes = sorted({interface.semantic_interface_hash for interface in interfaces})
    if sidecar["interface-semantic-hashes"] != expected_hashes:
        raise _error("records sidecar interface hashes differ from emitted `.osri` files")
    if sidecar["records"] != expected_records:
        raise _error("records sidecar cannot be reconstructed from emitted `.osri` files")
    expected_identities = [item["occurrence"] for item in expected_records]
    if sidecar["record-identities"] != expected_identities:
        raise _error("records sidecar identities differ from emitted `.osri` files")
