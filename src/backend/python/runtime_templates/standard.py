"""Domain-neutral standard functions linked into generated distributions."""

from __future__ import annotations

import builtins as _builtins
import functools as _functools
import math as _math
from collections.abc import Callable, Iterable, Mapping, Sequence, Set as AbstractSet
from typing import Any

from ._logical import LogicalMap as _LogicalMap
from ._logical import entry_index as _entry_index
from ._logical import logical_map as _logical_map
from ._sequence_consumers import every_q as every_p
from ._sequence_consumers import not_any_q as not_any_p
from ._sequence_consumers import not_every_q as not_every_p
from ._sequence_core import _LazySeq, _iter_or_empty, lazy_seq
from ._sequence_core import coll_q as coll_p
from ._sequence_core import empty_q as empty_p
from ._sequence_core import seq_q as seq_p
from ._sequence_core import sequential_q as sequential_p
from .control import deref, future_call
from .control import future_cancel as future_cancel_p
from .control import future_cancelled as future_cancelled_p
from .control import future_done as future_done_p


def _mapping_entries(value: Mapping[Any, Any]) -> Iterable[tuple[Any, Any]]:
    return value.items()


def identity(value: Any) -> Any:
    return value


def apply(function: Callable[..., Any], prefix: Iterable[Any], arguments: Iterable[Any]) -> Any:
    """Invoke one Kernel leaf with fixed and variadic positional arguments."""
    return function(*tuple(prefix), *tuple(arguments))


def constantly(value: Any) -> Callable[..., Any]:
    return lambda *args, **kwargs: value


def comp(*functions: Callable[..., Any]) -> Callable[..., Any]:
    if not functions:
        return identity

    def composed(*args: Any, **kwargs: Any) -> Any:
        result = functions[-1](*args, **kwargs)
        for function in reversed(functions[:-1]):
            result = function(result)
        return result

    return composed


def partial(function: Callable[..., Any], *args: Any, **kwargs: Any) -> Callable[..., Any]:
    return _functools.partial(function, *args, **kwargs)


def juxt(*functions: Callable[..., Any]) -> Callable[..., tuple[Any, ...]]:
    return lambda *args, **kwargs: tuple(function(*args, **kwargs) for function in functions)


def complement(function: Callable[..., Any]) -> Callable[..., bool]:
    return lambda *args, **kwargs: not _truthy(function(*args, **kwargs))


def _truthy(value: object) -> bool:
    return value is not None and value is not False


def get(collection: object, key: object, not_found: object = None) -> object:
    if collection is None:
        return not_found
    if isinstance(collection, Mapping):
        return collection.get(key, not_found)
    if isinstance(key, int) and not isinstance(key, bool):
        try:
            return collection[key]  # type: ignore[index]
        except (IndexError, KeyError):
            return not_found
    raise TypeError("get expects a mapping or an integer-indexed collection")


def assoc(collection: object, *entries: object) -> object:
    if len(entries) % 2:
        raise TypeError("assoc expects key/value pairs")
    if collection is None or isinstance(collection, Mapping):
        result = [] if collection is None else list(_mapping_entries(collection))
        for index in _builtins.range(0, len(entries), 2):
            key = entries[index]
            value = entries[index + 1]
            existing = _entry_index(result, key)
            if existing is None:
                result.append((key, value))
            else:
                original_key, _ = result[existing]
                result[existing] = (original_key, value)
        return _LogicalMap(result)
    if isinstance(collection, (list, tuple)):
        result = list(collection)
        for index in _builtins.range(0, len(entries), 2):
            key = entries[index]
            if not isinstance(key, int) or isinstance(key, bool):
                raise TypeError("assoc sequence keys must be integers")
            result[key] = entries[index + 1]
        return tuple(result) if isinstance(collection, tuple) else result
    raise TypeError("assoc expects a mapping or indexed collection")


def dissoc(collection: object, *keys: object) -> Mapping[object, object]:
    if collection is None:
        return _LogicalMap()
    if not isinstance(collection, Mapping):
        raise TypeError("dissoc expects a mapping")
    result = list(_mapping_entries(collection))
    for key in keys:
        index = _entry_index(result, key)
        if index is not None:
            result.pop(index)
    return _LogicalMap(result)


def update(collection: object, key: object, function: Callable[..., Any], *args: Any) -> object:
    return assoc(collection, key, function(get(collection, key), *args))


def get_in(collection: object, keys: Iterable[object], not_found: object = None) -> object:
    current = collection
    sentinel = object()
    for key in keys:
        current = get(current, key, sentinel)
        if current is sentinel:
            return not_found
    return current


def assoc_in(collection: object, keys: Iterable[object], value: object) -> object:
    path = tuple(keys)
    if not path:
        return value
    child = get(collection, path[0], None)
    return assoc(collection, path[0], assoc_in(child, path[1:], value))


def update_in(
    collection: object,
    keys: Iterable[object],
    function: Callable[..., Any],
    *args: Any,
) -> object:
    path = tuple(keys)
    return assoc_in(collection, path, function(get_in(collection, path), *args))


def select_keys(collection: Mapping[Any, Any], keys: Iterable[Any]) -> Mapping[Any, Any]:
    return _logical_map((key, collection[key]) for key in keys if key in collection)


def merge(*collections: object) -> Mapping[object, object]:
    result: Mapping[object, object] = _LogicalMap()
    for collection in collections:
        if collection is not None:
            if not isinstance(collection, Mapping):
                raise TypeError("merge expects mappings or none")
            for key, value in _mapping_entries(collection):
                result = assoc(result, key, value)  # type: ignore[assignment]
    return result


def merge_with(function: Callable[..., Any], *collections: object) -> Mapping[object, object]:
    result: Mapping[object, object] = _LogicalMap()
    for collection in collections:
        if collection is None:
            continue
        if not isinstance(collection, Mapping):
            raise TypeError("merge-with expects mappings or none")
        for key, value in _mapping_entries(collection):
            result = assoc(result, key, function(result[key], value) if key in result else value)  # type: ignore[assignment]
    return result


def group_by(function: Callable[[Any], Any], collection: object) -> Mapping[Any, tuple[Any, ...]]:
    groups: list[tuple[Any, list[Any]]] = []
    for value in _iter_or_empty(collection):
        key = function(value)
        index = _entry_index(groups, key)
        if index is None:
            groups.append((key, [value]))
        else:
            groups[index][1].append(value)
    return _LogicalMap((key, tuple(values)) for key, values in groups)


def frequencies(collection: object) -> Mapping[Any, int]:
    result: list[tuple[Any, int]] = []
    for value in _iter_or_empty(collection):
        index = _entry_index(result, value)
        if index is None:
            result.append((value, 1))
        else:
            key, count = result[index]
            result[index] = (key, count + 1)
    return _LogicalMap(result)


def index_by(function: Callable[[Any], Any], collection: object) -> Mapping[Any, Any]:
    return _logical_map(
        ((function(value), value) for value in _iter_or_empty(collection)),
        reject_collisions=True,
        operation="index-by",
    )


def rename_keys(collection: Mapping[Any, Any], renames: Mapping[Any, Any]) -> Mapping[Any, Any]:
    return _logical_map(
        ((renames.get(key, key), value) for key, value in _mapping_entries(collection)),
        reject_collisions=True,
        operation="rename-keys",
    )


def update_keys(function: Callable[[Any], Any], collection: Mapping[Any, Any]) -> Mapping[Any, Any]:
    return _logical_map(
        ((function(key), value) for key, value in _mapping_entries(collection)),
        reject_collisions=True,
        operation="update-keys",
    )


def update_vals(function: Callable[[Any], Any], collection: Mapping[Any, Any]) -> Mapping[Any, Any]:
    return _LogicalMap((key, function(value)) for key, value in _mapping_entries(collection))


def zipmap(keys: Iterable[Any], values: Iterable[Any]) -> Mapping[Any, Any]:
    return _logical_map(zip(keys, values))


def invert(collection: Mapping[Any, Any]) -> Mapping[Any, Any]:
    return _logical_map(
        ((value, key) for key, value in _mapping_entries(collection)),
        reject_collisions=True,
        operation="invert",
    )


def range(*arguments: int) -> _LazySeq:
    if not 1 <= len(arguments) <= 3:
        raise TypeError("range expects end, start/end, or start/end/step")
    if any(not isinstance(value, int) or isinstance(value, bool) for value in arguments):
        raise TypeError("range arguments must be integers")
    if len(arguments) == 3 and arguments[2] == 0:
        raise ValueError("range step cannot be zero")
    return lazy_seq(lambda: _builtins.range(*arguments))


def flatten(value: object) -> _LazySeq:
    def walk(current: object) -> Iterable[object]:
        if isinstance(current, (list, tuple, _LazySeq)):
            for item in current:
                yield from walk(item)
        else:
            yield current

    return lazy_seq(lambda: walk(value))


def nil_p(value: object) -> bool:
    return value is None


def some_p(value: object) -> bool:
    return value is not None


def true_p(value: object) -> bool:
    return value is True


def false_p(value: object) -> bool:
    return value is False


def number_p(value: object) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def string_p(value: object) -> bool:
    return isinstance(value, str)


def list_p(value: object) -> bool:
    return isinstance(value, list)


def vector_p(value: object) -> bool:
    return isinstance(value, tuple)


def map_p(value: object) -> bool:
    return isinstance(value, Mapping)


def set_p(value: object) -> bool:
    return isinstance(value, AbstractSet)


def sequence_p(value: object) -> bool:
    return isinstance(value, (list, tuple, _LazySeq))


def trim(text: str) -> str:
    return text.strip()


def trim_left(text: str) -> str:
    return text.lstrip()


def trim_right(text: str) -> str:
    return text.rstrip()


def split(text: str, separator: str, limit: int | None = None) -> tuple[str, ...]:
    if not isinstance(separator, str) or not separator:
        raise ValueError("split separator must be a non-empty string")
    if limit is None:
        return tuple(text.split(separator))
    if not isinstance(limit, int) or isinstance(limit, bool) or limit <= 0:
        raise ValueError("split limit must be a positive integer")
    return tuple(text.split(separator, limit - 1))


def split_lines(text: str, keep_ends: bool = False) -> tuple[str, ...]:
    return tuple(text.splitlines(keep_ends))


def join(separator: str, strings: Iterable[str]) -> str:
    values = tuple(strings)
    if not all(isinstance(value, str) for value in values):
        raise TypeError("join expects strings")
    return separator.join(values)


def replace(text: str, old: str, new: str) -> str:
    return text.replace(old, new)


def starts_with_p(text: str, fragment: str) -> bool:
    return text.startswith(fragment)


def ends_with_p(text: str, fragment: str) -> bool:
    return text.endswith(fragment)


def includes_p(text: str, fragment: str) -> bool:
    return fragment in text


def lower(text: str) -> str:
    return text.lower()


def upper(text: str) -> str:
    return text.upper()


def capitalize(text: str) -> str:
    return text.capitalize()


def blank_p(text: str) -> bool:
    return not text or text.isspace()


pi = _math.pi
e = _math.e
tau = _math.tau
inf = _math.inf
nan = _math.nan
abs = _builtins.abs
floor = _math.floor
ceil = _math.ceil
trunc = _math.trunc
sqrt = _math.sqrt
exp = _math.exp
log10 = _math.log10
sin = _math.sin
cos = _math.cos
tan = _math.tan
asin = _math.asin
acos = _math.acos
atan = _math.atan
round = _builtins.round
log = _math.log
pow = _builtins.pow
atan2 = _math.atan2
finite_p = _math.isfinite
infinite_p = _math.isinf
nan_p = _math.isnan


def pmap(function: Callable[..., Any], *collections: Iterable[Any]) -> tuple[Any, ...]:
    futures = tuple(future_call(lambda values=values: function(*values)) for values in zip(*collections))
    return tuple(deref(value) for value in futures)


def get_attr(value: object, name: str) -> Any:
    return getattr(value, name)


def get_attr_or(value: object, name: str, fallback: object) -> Any:
    return getattr(value, name, fallback)


def has_attr_p(value: object, name: str) -> bool:
    return hasattr(value, name)


def set_attr_bang(value: object, name: str, new_value: object) -> None:
    setattr(value, name, new_value)


def del_attr_bang(value: object, name: str) -> None:
    delattr(value, name)


def get_item(value: object, key: object) -> Any:
    return value[key]  # type: ignore[index]


def set_item_bang(value: object, key: object, new_value: object) -> None:
    value[key] = new_value  # type: ignore[index]


def del_item_bang(value: object, key: object) -> None:
    del value[key]  # type: ignore[index]


def call(function: Callable[..., Any], args: Iterable[Any], kwargs: Mapping[str, Any] | None = None) -> Any:
    return function(*tuple(args), **({} if kwargs is None else dict(kwargs)))


def iter(value: object) -> Iterable[Any]:
    return _builtins.iter(value)  # type: ignore[arg-type]


def type_name(value: object) -> str:
    value_type = type(value)
    return f"{value_type.__module__}.{value_type.__qualname__}"


__all__ = [name for name in globals() if not name.startswith("_") and name not in {
    "Any", "Callable", "Iterable", "Mapping", "Sequence", "annotations"
}]
