"""Static record projection from normalized interface forms."""

import re
from typing import Any, Dict

from ._common import _error
from ._sexpr import (
    _interface_hash,
    _optional_interface_string,
    _sexpr_integer,
    _sexpr_keyword,
    _sexpr_map,
    _sexpr_string,
    _sexpr_vector,
)


def _static_datum_from_interface(value: Any, label: str) -> Any:
    values = _sexpr_vector(value, label)
    if not values:
        raise _error("interface %s has an empty static datum" % label)
    tag = _sexpr_keyword(values[0], label + " tag")
    if tag == "none" and len(values) == 1:
        return None
    if tag == "bool" and len(values) == 2 and isinstance(values[1], bool):
        return values[1]
    if tag == "int" and len(values) == 2:
        integer = _sexpr_string(values[1], label + " integer")
        if not re.fullmatch(r"0|-?[1-9]\d*", integer):
            raise _error("interface %s contains a non-canonical integer" % label)
        return {"$osiris": "int", "value": integer}
    if tag == "float" and len(values) == 2:
        bits = _sexpr_string(values[1], label + " float")
        if not re.fullmatch(r"[0-9a-f]{16}", bits):
            raise _error("interface %s contains invalid binary64 bits" % label)
        exponent = (int(bits, 16) >> 52) & 0x7FF
        if exponent == 0x7FF:
            raise _error("interface %s contains a non-finite float" % label)
        return {"$osiris": "float", "value": bits}
    if tag == "string" and len(values) == 2:
        return _sexpr_string(values[1], label + " string")
    if tag == "keyword" and len(values) == 2:
        return {
            "$osiris": "keyword",
            "spelling": _sexpr_string(values[1], label + " keyword"),
        }
    if tag == "symbol" and len(values) == 3:
        result = {
            "$osiris": "symbol",
            "spelling": _sexpr_string(values[1], label + " symbol"),
        }
        binding = _optional_interface_string(values[2], label + " binding")
        if binding is not None:
            result["binding-id"] = binding
        return result
    if tag in ("list", "vector", "set") and len(values) == 2:
        items = [
            _static_datum_from_interface(item, label + " item")
            for item in _sexpr_vector(values[1], label + " items")
        ]
        return {"$osiris": tag, "items": items}
    if tag == "map" and len(values) == 2:
        entries = []
        for item in _sexpr_vector(values[1], label + " entries"):
            pair = _sexpr_vector(item, label + " entry")
            if len(pair) != 2:
                raise _error("interface %s map entry must be a pair" % label)
            entries.append(
                [
                    _static_datum_from_interface(pair[0], label + " key"),
                    _static_datum_from_interface(pair[1], label + " value"),
                ]
            )
        return {"$osiris": "map", "entries": entries}
    raise _error("interface %s contains unsupported static datum `%s`" % (label, tag))


def _schema_identity_from_interface(value: Any, label: str) -> Dict[str, Any]:
    fields = _sexpr_map(
        value,
        label,
        {"binding-id", "schema-id", "version", "body-hash"},
    )
    return {
        "binding-id": _sexpr_string(fields["binding-id"], label + " binding-id"),
        "schema-id": _sexpr_string(fields["schema-id"], label + " schema-id"),
        "version": _sexpr_integer(fields["version"], label + " version"),
        "body-hash": _interface_hash(fields["body-hash"], label + " body-hash"),
    }


def _index_claim_from_interface(value: Any, label: str) -> Dict[str, Any]:
    fields = _sexpr_map(
        value,
        label,
        {
            "index-id",
            "projection-field",
            "projection-role",
            "key",
            "normalized-key",
            "raw-spelling",
        },
    )
    result = {
        "index-id": _sexpr_string(fields["index-id"], label + " index-id"),
        "projection-field": _sexpr_string(
            fields["projection-field"], label + " projection-field"
        ),
        "projection-role": _sexpr_string(
            fields["projection-role"], label + " projection-role"
        ),
        "key": _static_datum_from_interface(fields["key"], label + " key"),
        "normalized-key": _sexpr_string(
            fields["normalized-key"], label + " normalized-key"
        ),
    }
    raw = _optional_interface_string(fields["raw-spelling"], label + " raw-spelling")
    if raw is not None:
        result["raw-spelling"] = raw
    return result


def _record_origin_from_interface(value: Any, label: str) -> Dict[str, Any]:
    fields = _sexpr_map(value, label, {"module", "span", "macro-origin"})
    span = _sexpr_vector(fields["span"], label + " span")
    if len(span) != 2:
        raise _error("interface %s span must contain two offsets" % label)
    start = _sexpr_integer(span[0], label + " span start")
    end = _sexpr_integer(span[1], label + " span end")
    if start > end:
        raise _error("interface %s span is reversed" % label)
    result = {
        "module": _sexpr_string(fields["module"], label + " module"),
        "span": [start, end],
    }
    macro_origin = _optional_interface_string(fields["macro-origin"], label + " macro-origin")
    if macro_origin is not None:
        result["macro-origin"] = macro_origin
    return result


def _record_from_interface(value: Any, label: str) -> Dict[str, Any]:
    fields = _sexpr_map(
        value,
        label,
        {
            "schema",
            "owner-binding-id",
            "owner-name",
            "module",
            "visibility",
            "stable-record-id",
            "record-body-hash",
            "fields",
            "index-claims",
            "origin",
        },
    )
    if _sexpr_keyword(fields["visibility"], label + " visibility") != "public":
        raise _error("interface %s contains a non-public owned record" % label)
    record_fields = []
    for item in _sexpr_vector(fields["fields"], label + " fields"):
        pair = _sexpr_vector(item, label + " field")
        if len(pair) != 2:
            raise _error("interface %s field must be a pair" % label)
        record_fields.append(
            [
                _sexpr_string(pair[0], label + " field name"),
                _static_datum_from_interface(pair[1], label + " field value"),
            ]
        )
    record = {
        "schema": _schema_identity_from_interface(fields["schema"], label + " schema"),
        "owner-binding-id": _sexpr_string(
            fields["owner-binding-id"], label + " owner-binding-id"
        ),
        "owner-name": _sexpr_string(fields["owner-name"], label + " owner-name"),
        "module": _sexpr_string(fields["module"], label + " module"),
        "public": True,
        "stable-record-id": _interface_hash(
            fields["stable-record-id"], label + " stable-record-id"
        ),
        "record-body-hash": _interface_hash(
            fields["record-body-hash"], label + " record-body-hash"
        ),
        "fields": record_fields,
        "index-claims": [
            _index_claim_from_interface(item, label + " index claim")
            for item in _sexpr_vector(fields["index-claims"], label + " index-claims")
        ],
        "origin": _record_origin_from_interface(fields["origin"], label + " origin"),
    }
    if record["module"] != record["origin"]["module"]:
        raise _error("interface %s record module differs from its origin" % label)
    return record
