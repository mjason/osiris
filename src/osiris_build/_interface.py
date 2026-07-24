"""Validation and projection of compiler-owned .osri artifacts."""

from typing import Any, Dict, List

from ._common import _error
from ._interface_records import _record_from_interface
from ._model import _InterfaceProjection
from ._sexpr import (
    _SAtom,
    _SList,
    _interface_hash,
    _read_osri,
    _sexpr_integer,
    _sexpr_map,
    _sexpr_string,
    _sexpr_vector,
)
from ._sidecar import _validate_sidecar, _validate_sidecar_against_interfaces


_INTERFACE_FORMAT = "osiris-interface"
_INTERFACE_FORMAT_VERSION = 3
_INTERFACE_COMPILER_ABI = "osiris-compiler-v0"
_LANGUAGE_VERSION = "0.1"
_INTERFACE_LANGUAGE_ABI = "osiris-language-v1"
_STANDARD_LIBRARY_ABI = 1
_LINKABLE_HELPER_FORMAT = 1


def _is_supported_python_target(value: str) -> bool:
    parts = value.split(".")
    return (
        len(parts) == 2
        and all(part.isascii() and part.isdigit() for part in parts)
        and tuple(map(int, parts)) >= (3, 11)
    )


def _interface_projection(path: str, data: bytes) -> _InterfaceProjection:
    forms = _read_osri(data, path)
    wrapped: Dict[str, Any] = {}
    for form in forms:
        if (
            not isinstance(form, _SList)
            or len(form.items) != 2
            or not isinstance(form.items[0], _SAtom)
        ):
            raise _error("interface `%s` contains an invalid top-level form" % path)
        name = str(form.items[0])
        if name in wrapped:
            raise _error("interface `%s` repeats top-level form `%s`" % (path, name))
        wrapped[name] = form.items[1]
    expected_forms = {
        "osiris-interface/header",
        "osiris-interface/body",
        "osiris-interface/graph",
        "osiris-interface/hashes",
    }
    if set(wrapped) != expected_forms:
        raise _error("interface `%s` does not use the current four-form schema" % path)

    header = _sexpr_map(
        wrapped["osiris-interface/header"],
        "`%s` header" % path,
        {
            "format",
            "format-version",
            "compiler-abi",
            "language-version",
            "language-abi",
            "standard-library-abi",
            "linkable-helper-format",
            "python-target",
        },
    )
    if (
        _sexpr_string(header["format"], "`%s` format" % path) != _INTERFACE_FORMAT
        or _sexpr_integer(header["format-version"], "`%s` format-version" % path)
        != _INTERFACE_FORMAT_VERSION
        or _sexpr_string(header["compiler-abi"], "`%s` compiler-abi" % path)
        != _INTERFACE_COMPILER_ABI
        or _sexpr_string(header["language-version"], "`%s` language-version" % path)
        != _LANGUAGE_VERSION
        or _sexpr_string(header["language-abi"], "`%s` language-abi" % path)
        != _INTERFACE_LANGUAGE_ABI
        or _sexpr_integer(
            header["standard-library-abi"], "`%s` standard-library-abi" % path
        )
        != _STANDARD_LIBRARY_ABI
        or _sexpr_integer(
            header["linkable-helper-format"], "`%s` linkable-helper-format" % path
        )
        != _LINKABLE_HELPER_FORMAT
    ):
        raise _error("interface `%s` has an incompatible format or ABI" % path)
    python_target = _sexpr_string(header["python-target"], "`%s` python-target" % path)
    if not _is_supported_python_target(python_target):
        raise _error("interface `%s` has an unsupported Python target" % path)

    body = _sexpr_map(
        wrapped["osiris-interface/body"],
        "`%s` body" % path,
        {
            "module",
            "metadata",
            "bindings",
            "aliases",
            "functions",
            "structs",
            "operator-instances",
            "macros",
            "phase-helpers",
            "static-schemas",
            "owned-records",
        },
    )
    module = _sexpr_string(body["module"], "`%s` module" % path)
    expected_module = path[: -len(".osri")].replace("/", ".")
    if module != expected_module:
        raise _error(
            "interface `%s` declares module `%s`, expected `%s`"
            % (path, module, expected_module)
        )

    hashes = _sexpr_map(
        wrapped["osiris-interface/hashes"],
        "`%s` hashes" % path,
        {"interface-body", "semantic-body", "tooling-body", "content-integrity"},
    )
    semantic_body_hash = _interface_hash(
        hashes["semantic-body"], "`%s` semantic body hash" % path
    )
    tooling_body_hash = _interface_hash(
        hashes["tooling-body"], "`%s` tooling body hash" % path
    )
    _interface_hash(hashes["interface-body"], "`%s` interface body hash" % path)
    _interface_hash(hashes["content-integrity"], "`%s` content integrity" % path)

    graph = _sexpr_map(
        wrapped["osiris-interface/graph"],
        "`%s` graph" % path,
        {
            "group-id",
            "members",
            "internal-edges",
            "external-dependencies",
            "semantic-interface-hash",
            "tooling-metadata-hash",
        },
    )
    semantic_interface_hash = _interface_hash(
        graph["semantic-interface-hash"], "`%s` semantic interface hash" % path
    )
    _interface_hash(graph["tooling-metadata-hash"], "`%s` tooling metadata hash" % path)
    members = []
    for item in _sexpr_vector(graph["members"], "`%s` graph members" % path):
        member = _sexpr_map(
            item,
            "`%s` graph member" % path,
            {"module", "semantic-body", "tooling-body"},
        )
        members.append(
            (
                _sexpr_string(member["module"], "`%s` graph member module" % path),
                _interface_hash(
                    member["semantic-body"], "`%s` graph member semantic body" % path
                ),
                _interface_hash(
                    member["tooling-body"], "`%s` graph member tooling body" % path
                ),
            )
        )
    own_members = [member for member in members if member[0] == module]
    if own_members != [(module, semantic_body_hash, tooling_body_hash)]:
        raise _error("interface `%s` graph does not identify its body exactly once" % path)

    records = tuple(
        _record_from_interface(item, "`%s` owned record" % path)
        for item in _sexpr_vector(body["owned-records"], "`%s` owned-records" % path)
    )
    for record in records:
        if record["module"] != module:
            raise _error("interface `%s` contains a record owned by another module" % path)
    return _InterfaceProjection(
        path,
        module,
        semantic_interface_hash,
        _LANGUAGE_VERSION,
        _STANDARD_LIBRARY_ABI,
        _LINKABLE_HELPER_FORMAT,
        python_target,
        records,
    )
