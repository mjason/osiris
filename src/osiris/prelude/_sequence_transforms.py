"""Lazy sequence transforms and generators."""

from __future__ import annotations

import builtins as _builtins
import itertools as _itertools
from collections.abc import Callable
from typing import Iterator, Optional, TypeVar

from ._sequence_core import _LazySeq, _MISSING, _iter_or_empty, lazy_seq
from .control import truthy

_Result = TypeVar("_Result")

def take(amount: int, collection: object) -> _LazySeq:
    """Lazily take at most ``amount`` values from a collection."""

    def produce() -> Iterator[object]:
        if amount > 0:
            yield from _itertools.islice(_iter_or_empty(collection), amount)

    return lazy_seq(produce)


def drop(amount: int, collection: object) -> _LazySeq:
    """Lazily skip at most ``amount`` values from a collection."""

    def produce() -> Iterator[object]:
        iterator = _iter_or_empty(collection)
        if amount > 0:
            for _ in range(amount):
                if _builtins.next(iterator, _MISSING) is _MISSING:
                    return
        yield from iterator

    return lazy_seq(produce)


def take_while(predicate: Callable[[object], object], collection: object) -> _LazySeq:
    """Lazily take values while a Clojure-truthy predicate holds."""

    def produce() -> Iterator[object]:
        for value in _iter_or_empty(collection):
            if not truthy(predicate(value)):
                return
            yield value

    return lazy_seq(produce)


def drop_while(predicate: Callable[[object], object], collection: object) -> _LazySeq:
    """Lazily drop the prefix satisfying ``predicate``."""

    def produce() -> Iterator[object]:
        iterator = _iter_or_empty(collection)
        for value in iterator:
            if not truthy(predicate(value)):
                yield value
                break
        yield from iterator

    return lazy_seq(produce)


def keep(function: Callable[[object], object], collection: object) -> _LazySeq:
    """Apply ``function`` and retain non-nil results lazily."""

    def produce() -> Iterator[object]:
        for value in _iter_or_empty(collection):
            result = function(value)
            if result is not None:
                yield result

    return lazy_seq(produce)


def keep_indexed(function: Callable[[int, object], object], collection: object) -> _LazySeq:
    """Indexed variant of :func:`keep`."""

    def produce() -> Iterator[object]:
        for index, value in enumerate(_iter_or_empty(collection)):
            result = function(index, value)
            if result is not None:
                yield result

    return lazy_seq(produce)


def map_indexed(function: Callable[[int, object], _Result], collection: object) -> _LazySeq:
    """Map a function over ``(index, value)`` pairs lazily."""

    def produce() -> Iterator[_Result]:
        for index, value in enumerate(_iter_or_empty(collection)):
            yield function(index, value)

    return lazy_seq(produce)


def iterate(function: Callable[[_Result], _Result], initial: _Result) -> _LazySeq:
    """Produce an infinite lazy sequence ``initial, f(initial), ...``."""

    def produce() -> Iterator[_Result]:
        value = initial
        while True:
            yield value
            value = function(value)

    return lazy_seq(produce)


def _repeat_count(value: object, name: str) -> int:
    """Validate the finite count accepted by ``repeat``-style helpers.

    Python's ``int`` constructor is intentionally not used here: accepting
    floats, strings, or booleans would silently change a malformed Osiris
    program into a different sequence length.
    """

    if not isinstance(value, int) or isinstance(value, bool):
        raise TypeError(f"{name} count must be an integer")
    return value


def repeat(*arguments: object) -> _LazySeq:
    """Produce a repeated value, finite or infinite depending on arity."""

    if len(arguments) == 1:
        amount: Optional[int] = None
        value = arguments[0]
    elif len(arguments) == 2:
        amount = _repeat_count(arguments[0], "repeat")
        value = arguments[1]
    else:
        raise TypeError("repeat expects a value or count and value")

    def produce() -> Iterator[object]:
        if amount is None:
            while True:
                yield value
        else:
            yield from _itertools.repeat(value, max(0, amount))

    return lazy_seq(produce)


def repeatedly(*arguments: object) -> _LazySeq:
    """Call a zero-argument function repeatedly, optionally a finite count."""

    if len(arguments) == 1:
        amount: Optional[int] = None
        function = arguments[0]
    elif len(arguments) == 2:
        amount = _repeat_count(arguments[0], "repeatedly")
        function = arguments[1]
    else:
        raise TypeError("repeatedly expects a function or count and function")
    if not callable(function):
        raise TypeError("repeatedly expects a callable")

    def produce() -> Iterator[object]:
        if amount is None:
            while True:
                yield function()
        else:
            for _ in range(max(0, amount)):
                yield function()

    return lazy_seq(produce)


def cycle(collection: object) -> _LazySeq:
    """Repeat a collection indefinitely while streaming its first traversal."""

    def produce() -> Iterator[object]:
        values: list[object] = []
        for value in _iter_or_empty(collection):
            values.append(value)
            yield value
        while values:
            yield from values

    return lazy_seq(produce)


def sequence(collection: object) -> _LazySeq:
    """Expose any iterable as a memoized lazy sequence."""

    return lazy_seq(lambda: _iter_or_empty(collection))


def _clojure_equal(left: object, right: object) -> bool:
    """Compare values for ``distinct`` without Python's bool/int aliasing.

    Clojure treats ``false`` and ``0`` as different values even though Python
    considers them equal.  Most Osiris values use ordinary Python equality;
    the small type guard below preserves that distinction while the fallback
    keeps unhashable values (vectors/maps) usable in the lazy sequence.
    """

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
        # Array-like equality (for example NumPy) returns an elementwise
        # object.  Treat two arrays as equal only when every element agrees.
        all_equal = getattr(result, "all", None)
        if callable(all_equal):
            try:
                return bool(all_equal())
            except (TypeError, ValueError):
                return False
        return False


def distinct(collection: object) -> _LazySeq:
    """Lazily yield the first occurrence of each value in ``collection``.

    The result is a memoized ``LazySeq`` just like ``take`` and ``filter``;
    creating it does not consume the source, and replaying it does not invoke
    the source a second time.  A linear seen-value table intentionally accepts
    unhashable vectors/maps and keeps Clojure's value equality visible.
    """

    def produce() -> Iterator[object]:
        seen: list[object] = []
        for value in _iter_or_empty(collection):
            if any(_clojure_equal(value, previous) for previous in seen):
                continue
            seen.append(value)
            yield value

    return lazy_seq(produce)


def dedupe(collection: object) -> _LazySeq:
    """Lazily remove consecutive values that are equal in Clojure terms."""

    def produce() -> Iterator[object]:
        previous = _MISSING
        for value in _iter_or_empty(collection):
            if previous is not _MISSING and _clojure_equal(value, previous):
                continue
            previous = value
            yield value

    return lazy_seq(produce)
