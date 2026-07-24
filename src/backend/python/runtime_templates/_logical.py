"""Logical Osiris equality and immutable associative containers."""

from __future__ import annotations

from collections.abc import Iterable, Iterator, Mapping, Set as AbstractSet
from typing import Any


def logical_equal(left: object, right: object) -> bool:
    """Compare values without Python's bool/int aliasing."""

    if left is right:
        return True
    if left is None or right is None:
        return False
    if isinstance(left, bool) or isinstance(right, bool):
        return type(left) is type(right) and left == right
    try:
        result = left == right
    except Exception:
        return False
    try:
        return bool(result)
    except (TypeError, ValueError):
        all_equal = getattr(result, "all", None)
        if callable(all_equal):
            try:
                return bool(all_equal())
            except (TypeError, ValueError):
                return False
        return False


def entry_index(entries: Iterable[tuple[Any, Any]], key: object) -> int | None:
    for index, (existing, _) in enumerate(entries):
        if logical_equal(existing, key):
            return index
    return None


class LogicalMap(Mapping[Any, Any]):
    """Immutable ordered mapping with Osiris key equality."""

    __slots__ = ("_entries",)

    def __init__(self, entries: Iterable[tuple[Any, Any]] = ()) -> None:
        self._entries = tuple(entries)

    def __getitem__(self, key: object) -> Any:
        index = entry_index(self._entries, key)
        if index is None:
            raise KeyError(key)
        return self._entries[index][1]

    def __iter__(self) -> Iterator[Any]:
        return (key for key, _ in self._entries)

    def __len__(self) -> int:
        return len(self._entries)

    def items(self) -> Any:
        return self._entries

    def __repr__(self) -> str:
        body = ", ".join(f"{key!r}: {value!r}" for key, value in self._entries)
        return "{" + body + "}"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Mapping) or len(self) != len(other):
            return False
        other_entries = tuple(other.items())
        return all(
            (index := entry_index(other_entries, key)) is not None
            and logical_equal(value, other_entries[index][1])
            for key, value in self._entries
        )


class LogicalSet(AbstractSet[Any]):
    """Immutable ordered set with Osiris value equality."""

    __slots__ = ("_items",)

    def __init__(self, items: Iterable[Any] = ()) -> None:
        unique: list[Any] = []
        for item in items:
            if not any(logical_equal(item, existing) for existing in unique):
                unique.append(item)
        self._items = tuple(unique)

    def __contains__(self, value: object) -> bool:
        return any(logical_equal(value, item) for item in self._items)

    def __iter__(self) -> Iterator[Any]:
        return iter(self._items)

    def __len__(self) -> int:
        return len(self._items)

    def __repr__(self) -> str:
        return "#{" + ", ".join(repr(item) for item in self._items) + "}"

    def __eq__(self, other: object) -> bool:
        return (
            isinstance(other, AbstractSet)
            and len(self) == len(other)
            and all(item in other for item in self._items)
        )


def logical_map(
    entries: Iterable[tuple[Any, Any]],
    *,
    reject_collisions: bool = False,
    operation: str = "map",
) -> LogicalMap:
    result: list[tuple[Any, Any]] = []
    for key, value in entries:
        index = entry_index(result, key)
        if index is None:
            result.append((key, value))
        elif reject_collisions:
            raise ValueError(f"{operation} produced duplicate key {key!r}")
        else:
            original_key, _ = result[index]
            result[index] = (original_key, value)
    return LogicalMap(result)


def logical_set(items: Iterable[Any]) -> LogicalSet:
    return LogicalSet(items)


__all__ = ["LogicalMap", "LogicalSet", "logical_equal", "logical_map", "logical_set"]
