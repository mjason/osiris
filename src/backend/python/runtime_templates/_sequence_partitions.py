"""Lazy partitioning, interleaving, and terminal-window operations."""

from __future__ import annotations

import builtins as _builtins
import collections as _collections
import itertools as _itertools
from collections.abc import Callable
from typing import Iterator

from ._sequence_core import _LazySeq, _MISSING, _iter_or_empty, lazy_seq
from ._sequence_eager import Reduced
from ._sequence_transforms import _clojure_equal

def _positive_partition_integer(value: object, name: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise TypeError(f"{name} must be an integer")
    if value <= 0:
        raise ValueError(f"{name} must be positive")
    return value


def _partition_windows(
    size: int,
    step: int,
    collection: object,
    incomplete: str,
    padding: object = _MISSING,
) -> _LazySeq:
    """Build streaming windows with at most ``size`` retained source values."""

    def produce() -> Iterator[object]:
        iterator = _iter_or_empty(collection)
        window: list[object] = []
        exhausted = False
        while True:
            while len(window) < size and not exhausted:
                value = _builtins.next(iterator, _MISSING)
                if value is _MISSING:
                    exhausted = True
                    break
                window.append(value)

            if not window:
                return
            if len(window) == size:
                yield tuple(window)
            elif incomplete == "drop":
                return
            elif incomplete == "pad":
                padded = list(window)
                padded.extend(
                    _itertools.islice(
                        _iter_or_empty(padding),
                        size - len(padded),
                    )
                )
                yield tuple(padded)
                return
            else:
                yield tuple(window)

            if step < len(window):
                del window[:step]
                continue

            skip = step - len(window)
            window.clear()
            for _ in range(skip):
                if _builtins.next(iterator, _MISSING) is _MISSING:
                    exhausted = True
                    break

    return lazy_seq(produce)


def partition(size: int, *arguments: object) -> _LazySeq:
    """Lazily produce complete windows, optionally padding the final one."""

    size = _positive_partition_integer(size, "partition size")
    if len(arguments) == 1:
        step = size
        padding = _MISSING
        collection = arguments[0]
    elif len(arguments) == 2:
        step = _positive_partition_integer(arguments[0], "partition step")
        padding = _MISSING
        collection = arguments[1]
    elif len(arguments) == 3:
        step = _positive_partition_integer(arguments[0], "partition step")
        padding = arguments[1]
        collection = arguments[2]
    else:
        raise TypeError("partition expects size, optional step/padding, and collection")
    mode = "drop" if padding is _MISSING else "pad"
    return _partition_windows(size, step, collection, mode, padding)


def partition_all(size: int, *arguments: object) -> _LazySeq:
    """Lazily produce every window, retaining incomplete trailing windows."""

    size = _positive_partition_integer(size, "partition-all size")
    if len(arguments) == 1:
        step = size
        collection = arguments[0]
    elif len(arguments) == 2:
        step = _positive_partition_integer(arguments[0], "partition-all step")
        collection = arguments[1]
    else:
        raise TypeError("partition-all expects size, optional step, and collection")
    return _partition_windows(size, step, collection, "all")


def partition_by(function: Callable[[object], object], collection: object) -> _LazySeq:
    """Lazily group consecutive values whose callback results are equal."""

    if not callable(function):
        raise TypeError("partition-by expects a callable")

    def keyed_values() -> Iterator[object]:
        for value in _iter_or_empty(collection):
            yield value, function(value)

    keyed = lazy_seq(keyed_values)

    def group_values(start: int, key: object) -> Iterator[object]:
        for value, candidate_key in keyed._iter_from(start):
            if not _clojure_equal(candidate_key, key):
                return
            yield value

    def produce() -> Iterator[object]:
        start = 0
        while True:
            first_pair = _builtins.next(keyed._iter_from(start), _MISSING)
            if first_pair is _MISSING:
                return
            _, key = first_pair
            yield lazy_seq(
                lambda group_start=start, group_key=key: group_values(
                    group_start, group_key
                )
            )
            for _, candidate_key in keyed._iter_from(start):
                if not _clojure_equal(candidate_key, key):
                    break
                start += 1
            else:
                return

    return lazy_seq(produce)


def interleave(*collections: object) -> _LazySeq:
    """Lazily alternate values until the shortest collection is exhausted."""

    if len(collections) < 2:
        raise TypeError("interleave expects at least two collections")

    def produce() -> Iterator[object]:
        iterators = tuple(_iter_or_empty(collection) for collection in collections)
        while True:
            values: list[object] = []
            for iterator in iterators:
                value = _builtins.next(iterator, _MISSING)
                if value is _MISSING:
                    return
                values.append(value)
            yield from values

    return lazy_seq(produce)


def interpose(separator: object, collection: object) -> _LazySeq:
    """Lazily place ``separator`` between adjacent collection values."""

    def produce() -> Iterator[object]:
        iterator = _iter_or_empty(collection)
        first_value = _builtins.next(iterator, _MISSING)
        if first_value is _MISSING:
            return
        yield first_value
        for value in iterator:
            yield separator
            yield value

    return lazy_seq(produce)


def take_last(amount: int, collection: object) -> _LazySeq:
    """Return the final values; an infinite source therefore never yields."""

    if not isinstance(amount, int) or isinstance(amount, bool):
        raise TypeError("take-last count must be an integer")
    if amount < 0:
        raise ValueError("take-last count must be non-negative")

    def produce() -> Iterator[object]:
        if amount <= 0:
            return
        buffer = _collections.deque(maxlen=amount)
        buffer.extend(_iter_or_empty(collection))
        yield from buffer

    return lazy_seq(produce)


def drop_last(*arguments: object) -> _LazySeq:
    """Lazily omit a trailing window, defaulting to one value."""

    if len(arguments) == 1:
        amount = 1
        collection = arguments[0]
    elif len(arguments) == 2:
        amount = arguments[0]
        collection = arguments[1]
    else:
        raise TypeError("drop-last expects a collection or count and collection")
    if not isinstance(amount, int) or isinstance(amount, bool):
        raise TypeError("drop-last count must be an integer")
    if amount < 0:
        raise ValueError("drop-last count must be non-negative")

    def produce() -> Iterator[object]:
        if amount <= 0:
            yield from _iter_or_empty(collection)
            return
        buffer: _collections.deque[object] = _collections.deque()
        for value in _iter_or_empty(collection):
            buffer.append(value)
            if len(buffer) > amount:
                yield buffer.popleft()

    return lazy_seq(produce)


def reductions(function: Callable[[object, object], object], *arguments: object) -> _LazySeq:
    """Yield each intermediate accumulator, including the initial value."""

    if len(arguments) not in (1, 2):
        raise TypeError("reductions expects function, collection, or initial value and collection")

    def produce() -> Iterator[object]:
        iterator = _iter_or_empty(arguments[0] if len(arguments) == 1 else arguments[1])
        if len(arguments) == 1:
            try:
                accumulator = _builtins.next(iterator)
            except StopIteration:
                yield function()
                return
        else:
            accumulator = arguments[0]
        yield accumulator
        for value in iterator:
            accumulator = function(accumulator, value)
            if isinstance(accumulator, Reduced):
                accumulator = accumulator.value
                yield accumulator
                return
            yield accumulator

    return lazy_seq(produce)
