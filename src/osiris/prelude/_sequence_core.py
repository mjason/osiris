"""Memoized lazy-sequence representation and core sequence operations."""

from __future__ import annotations

import builtins as _builtins
import threading as _threading
from collections.abc import Callable, Iterable
from typing import Iterator, Optional

_MISSING = object()

class _LazySeq:
    """Memoized lazy sequence that realizes elements one at a time.

    The source iterator and already produced values are retained so a lazy
    sequence can be traversed repeatedly without re-running its thunk or
    losing values from a one-shot generator.
    """

    __slots__ = ("_thunk", "_source", "_cache", "_done", "_error", "_lock")

    def __init__(self, thunk: Callable[[], Iterable[object]]) -> None:
        self._thunk = thunk
        self._source: Optional[Iterator[object]] = None
        self._cache: list[object] = []
        self._done = False
        self._error: Optional[BaseException] = None
        self._lock = _threading.RLock()

    def _ensure_source_locked(self) -> Iterator[object]:
        """Initialize the source while ``_lock`` is held."""

        if self._error is not None:
            raise self._error
        if self._source is None:
            try:
                value = self._thunk()
                self._source = iter(()) if value is None else iter(value)
            except BaseException as error:
                # A lazy source is a memoized computation.  Cache failures so
                # a second consumer cannot rerun an effectful or partial thunk.
                self._error = error
                self._done = True
                raise
        return self._source

    def _iter_from(self, index: int) -> Iterator[object]:
        """Iterate from a cached position without replaying earlier values."""

        while True:
            with self._lock:
                if self._error is not None:
                    raise self._error
                if index < len(self._cache):
                    value = self._cache[index]
                    index += 1
                elif self._done:
                    return
                else:
                    try:
                        value = _builtins.next(self._ensure_source_locked())
                    except StopIteration:
                        self._done = True
                        return
                    except BaseException as error:
                        self._error = error
                        self._done = True
                        raise
                    self._cache.append(value)
                    index += 1
            yield value

    def __iter__(self) -> Iterator[object]:
        return self._iter_from(0)

    def __bool__(self) -> bool:
        """Match sequence truthiness without realizing more than one item."""

        try:
            _builtins.next(iter(self))
        except StopIteration:
            return False
        return True


def lazy_seq(thunk: Callable[[], Iterable[object]]) -> _LazySeq:
    """Construct a memoized, iterable lazy sequence from ``thunk``."""

    return _LazySeq(thunk)


def _iter_or_empty(collection: object) -> Iterator[object]:
    """Treat Clojure nil as an empty sequence at sequence boundaries."""

    if collection is None:
        return iter(())
    return iter(collection)  # type: ignore[arg-type]


def cons(value: object, collection: object) -> _LazySeq:
    """Return a lazy sequence with ``value`` before ``collection``."""

    def produce() -> Iterator[object]:
        yield value
        yield from _iter_or_empty(collection)

    return lazy_seq(produce)


def concat(*collections: object) -> _LazySeq:
    """Concatenate sequences lazily, skipping nil collections."""

    def produce() -> Iterator[object]:
        for collection in collections:
            yield from _iter_or_empty(collection)

    return lazy_seq(produce)


def count(collection: object) -> int:
    """Count a collection without applying Python truthiness to it."""

    if collection is None:
        return 0
    try:
        return len(collection)  # type: ignore[arg-type]
    except TypeError:
        return sum(1 for _ in collection)  # type: ignore[operator]


_EMPTY_PROBE = object()


def empty_q(collection: object) -> bool:
    """Return whether ``collection`` has no values.

    Sized containers are checked without iteration.  For a one-shot iterable
    the probe necessarily advances its iterator by at most one item; Osiris
    lazy sequences memoize that item, so repeated checks remain non-destructive
    at the public sequence boundary.  ``None`` follows Clojure's empty-seq
    convention, while scalar non-collections raise ``TypeError``.
    """

    if collection is None:
        return True
    try:
        return len(collection) == 0  # type: ignore[arg-type]
    except TypeError:
        try:
            iterator = iter(collection)  # type: ignore[arg-type]
        except TypeError as error:
            raise TypeError("empty? expects a collection") from error
        return _builtins.next(iterator, _EMPTY_PROBE) is _EMPTY_PROBE


def seq_q(value: object) -> bool:
    """Return whether ``value`` is an Osiris seq/list value.

    Raw Python iterators are deliberately not treated as seqs; callers can
    opt into the replayable seq protocol with :func:`sequence`.
    """

    return isinstance(value, (list, _LazySeq))


def coll_q(value: object) -> bool:
    """Return whether ``value`` is an Osiris collection value."""

    return isinstance(value, (list, tuple, dict, set, _LazySeq))


def sequential_q(value: object) -> bool:
    """Return whether ``value`` is an ordered sequential collection."""

    return isinstance(value, (list, tuple, _LazySeq))


def first(collection: object) -> object:
    """Return the first item or nil for an empty sequence."""

    return _builtins.next(_iter_or_empty(collection), None)


def rest(collection: object) -> list[object]:
    """Return all but the first item as a materialized list."""

    iterator = _iter_or_empty(collection)
    _builtins.next(iterator, None)
    return list(iterator)


def next(collection: object) -> Optional[_LazySeq]:
    """Return the lazy tail, or nil when no tail exists."""

    iterator = _iter_or_empty(collection)
    try:
        _builtins.next(iterator)
    except StopIteration:
        return None
    try:
        first_tail = _builtins.next(iterator)
    except StopIteration:
        return None

    def produce() -> Iterator[object]:
        yield first_tail
        yield from iterator

    return lazy_seq(produce)


def nth(collection: object, index: int, not_found: object = _MISSING) -> object:
    """Return an indexed item, or use the explicitly supplied fallback.

    Clojure distinguishes ``(nth coll index)`` from
    ``(nth coll index not-found)`` even when ``not-found`` is nil.  Keep that
    distinction at the Python boundary instead of silently turning an invalid
    two-argument lookup into ``None``.
    """

    if not isinstance(index, int) or isinstance(index, bool):
        raise TypeError("nth index must be an integer")
    if index < 0:
        if not_found is _MISSING:
            raise IndexError("nth index is out of range")
        return not_found
    for position, value in enumerate(_iter_or_empty(collection)):
        if position == index:
            return value
    if not_found is _MISSING:
        raise IndexError("nth index is out of range")
    return not_found


def seq(collection: object) -> Optional[object]:
    """Return a non-empty sequence or nil."""

    iterator = _iter_or_empty(collection)
    try:
        first_value = _builtins.next(iterator)
    except StopIteration:
        return None
    return cons(first_value, iterator)


def empty(collection: object) -> object:
    """Return an empty value with the broad shape of ``collection``."""

    if isinstance(collection, tuple):
        return ()
    if isinstance(collection, set):
        return set()
    if isinstance(collection, dict):
        return {}
    if isinstance(collection, str):
        return ""
    if isinstance(collection, bytes):
        return b""
    return []
