"""Bounded S-expression reader for compiler-owned interfaces."""

import json
import re
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Set, Tuple

from ._common import _error


_HASH_RE = re.compile(r"^sha256:[0-9a-f]{64}$")


class _SAtom(str):
    pass


class _SKeyword(str):
    pass


@dataclass(frozen=True)
class _SList:
    items: Tuple[Any, ...]


@dataclass(frozen=True)
class _SVector:
    items: Tuple[Any, ...]


@dataclass(frozen=True)
class _SMap:
    items: Tuple[Any, ...]


@dataclass(frozen=True)
class _SSet:
    items: Tuple[Any, ...]


@dataclass(frozen=True)
class _SPrefix:
    prefix: str
    items: Tuple[Any, ...]


def _has_valid_unicode_scalars(value: str) -> bool:
    return not any(0xD800 <= ord(char) <= 0xDFFF for char in value)


class _SExpressionReader:
    """A bounded reader for compiler-owned, normalized `.osri` data."""

    def __init__(self, data: bytes, label: str):
        if len(data) > 32 * 1024 * 1024:
            raise _error("interface `%s` exceeds the 32 MiB build limit" % label)
        try:
            self.text = data.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise _error("interface `%s` is not UTF-8: %s" % (label, exc)) from exc
        if self.text.startswith("\ufeff"):
            raise _error("interface `%s` must not contain a UTF-8 BOM" % label)
        self.label = label
        self.index = 0
        self.nodes = 0

    def read_document(self) -> Tuple[Any, ...]:
        forms: List[Any] = []
        self._skip_trivia()
        while self.index < len(self.text):
            forms.append(self._read_form(0))
            self._skip_trivia()
        return tuple(forms)

    def _skip_trivia(self) -> None:
        while self.index < len(self.text):
            char = self.text[self.index]
            if char.isspace():
                self.index += 1
                continue
            if char == ";":
                newline = self.text.find("\n", self.index)
                self.index = len(self.text) if newline < 0 else newline + 1
                continue
            break

    def _read_form(self, depth: int) -> Any:
        if depth > 256:
            raise _error("interface `%s` exceeds the reader nesting limit" % self.label)
        self.nodes += 1
        if self.nodes > 1_000_000:
            raise _error("interface `%s` exceeds the reader form limit" % self.label)
        self._skip_trivia()
        if self.index >= len(self.text):
            raise _error("interface `%s` ends inside a form" % self.label)
        if self.text.startswith("#{", self.index):
            self.index += 2
            return _SSet(self._read_collection("}", depth))
        char = self.text[self.index]
        if char == "(":
            self.index += 1
            return _SList(self._read_collection(")", depth))
        if char == "[":
            self.index += 1
            return _SVector(self._read_collection("]", depth))
        if char == "{":
            self.index += 1
            return _SMap(self._read_collection("}", depth))
        if char in ")]}":
            raise _error("interface `%s` contains unmatched `%s`" % (self.label, char))
        if char == '"':
            return self._read_string()
        if self.text.startswith("~@", self.index):
            self.index += 2
            return _SPrefix("~@", (self._read_form(depth + 1),))
        if char in "'`~":
            self.index += 1
            return _SPrefix(char, (self._read_form(depth + 1),))
        if char == "^":
            self.index += 1
            return _SPrefix(
                "^",
                (self._read_form(depth + 1), self._read_form(depth + 1)),
            )
        return self._read_atom()

    def _read_collection(self, closer: str, depth: int) -> Tuple[Any, ...]:
        items: List[Any] = []
        while True:
            self._skip_trivia()
            if self.index >= len(self.text):
                raise _error(
                    "interface `%s` is missing closing `%s`" % (self.label, closer)
                )
            if self.text[self.index] == closer:
                self.index += 1
                return tuple(items)
            items.append(self._read_form(depth + 1))

    def _read_string(self) -> str:
        try:
            value, end = json.JSONDecoder().raw_decode(self.text, self.index)
        except json.JSONDecodeError as exc:
            raise _error("interface `%s` has an invalid string: %s" % (self.label, exc)) from exc
        if not isinstance(value, str) or not _has_valid_unicode_scalars(value):
            raise _error("interface `%s` contains an invalid Unicode scalar" % self.label)
        self.index = end
        return value

    def _read_atom(self) -> Any:
        start = self.index
        delimiters = set("()[]{}\"'`~^;")
        while self.index < len(self.text):
            char = self.text[self.index]
            if char.isspace() or char in delimiters:
                break
            self.index += 1
        if self.index == start:
            raise _error(
                "interface `%s` contains unsupported reader syntax near byte %d"
                % (self.label, self.index)
            )
        atom = self.text[start:self.index]
        if atom == "true":
            return True
        if atom == "false":
            return False
        if atom == "none":
            return None
        if re.fullmatch(r"-?(?:0|[1-9]\d*)", atom):
            return int(atom)
        if atom.startswith(":") and len(atom) > 1:
            return _SKeyword(atom[1:])
        return _SAtom(atom)


def _read_osri(data: bytes, label: str) -> Tuple[Any, ...]:
    return _SExpressionReader(data, label).read_document()


def _sexpr_map(value: Any, label: str, expected: Optional[Set[str]] = None) -> Dict[str, Any]:
    if not isinstance(value, _SMap) or len(value.items) % 2:
        raise _error("interface %s must be a keyword map" % label)
    result: Dict[str, Any] = {}
    iterator = iter(value.items)
    for key, item in zip(iterator, iterator):
        if not isinstance(key, _SKeyword):
            raise _error("interface %s contains a non-keyword map key" % label)
        name = str(key)
        if name in result:
            raise _error("interface %s contains duplicate key `%s`" % (label, name))
        result[name] = item
    if expected is not None and set(result) != expected:
        raise _error("interface %s has an unsupported shape" % label)
    return result


def _sexpr_vector(value: Any, label: str) -> Tuple[Any, ...]:
    if not isinstance(value, _SVector):
        raise _error("interface %s must be a vector" % label)
    return value.items


def _sexpr_string(value: Any, label: str) -> str:
    if not isinstance(value, str) or isinstance(value, (_SAtom, _SKeyword)):
        raise _error("interface %s must be a string" % label)
    return value


def _sexpr_integer(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise _error("interface %s must be a non-negative integer" % label)
    return value


def _sexpr_keyword(value: Any, label: str) -> str:
    if not isinstance(value, _SKeyword):
        raise _error("interface %s must be a keyword" % label)
    return str(value)


def _interface_hash(value: Any, label: str) -> str:
    digest = _sexpr_string(value, label)
    if not _HASH_RE.fullmatch(digest):
        raise _error("interface %s must be a lowercase SHA-256 digest" % label)
    return digest


def _optional_interface_string(value: Any, label: str) -> Optional[str]:
    if value is None:
        return None
    return _sexpr_string(value, label)
