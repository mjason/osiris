"""Eager collection, reduction, loop, and trampoline operations."""

from __future__ import annotations

import builtins as _builtins
from collections.abc import Callable, Iterable
from typing import Generic, Iterator, TypeVar, Union

from ._sequence_core import _LazySeq, _iter_or_empty, empty_q, lazy_seq
from .control import truthy

_Result = TypeVar("_Result")

def nonempty(collection: object) -> bool:
    """Test a collection without consuming memoized lazy values."""

    return not empty_q(collection)


class _ForStop:
    """Private nearest-collection stop token used by ``for :while``."""

    __slots__ = ()


_FOR_STOP = _ForStop()


def for_stop() -> _ForStop:
    """Return the private stop token consumed by collection control helpers."""

    return _FOR_STOP


def doseq(
    function: Callable[[object], object], collection: Iterable[object]
) -> None:
    """Invoke ``function`` in order without materializing callback results."""

    for value in _iter_or_empty(collection):
        if function(value) is _FOR_STOP:
            break


def _map_values(
    function: Callable[..., _Result], collections: tuple[Iterable[object], ...]
) -> Iterator[_Result]:
    return _builtins.map(
        function, *(_iter_or_empty(collection) for collection in collections)
    )


def mapv(
    function: Callable[..., _Result], *collections: Iterable[object]
) -> tuple[_Result, ...]:
    """Apply ``function`` lazily through ``map`` and materialize a vector."""

    return tuple(_map_values(function, collections))


def map(
    function: Callable[..., _Result], *collections: Iterable[object]
) -> _LazySeq:
    """Apply ``function`` lazily and memoize realized values.

    ``mapv`` is the explicit eager Vector form.  Keeping laziness in the
    linked helper lets generated Python preserve one callback invocation for
    every value already observed by a consumer.
    """

    return lazy_seq(lambda: _map_values(function, collections))


def mapcatv(
    function: Callable[..., Iterable[object]], *collections: Iterable[object]
) -> tuple[object, ...]:
    """Map one or more collections and concatenate each returned sequence.

    Like Clojure's ``mapcat``, traversal stops at the shortest input and the
    callback receives one value from each collection.
    """

    result: list[object] = []
    iterators = tuple(_iter_or_empty(collection) for collection in collections)
    for values in zip(*iterators):
        mapped = function(*values)
        if mapped is _FOR_STOP:
            break
        result.extend(_iter_or_empty(mapped))
    return tuple(result)


def mapcat(
    function: Callable[..., Iterable[object]], *collections: Iterable[object]
) -> _LazySeq:
    """Lazily map and concatenate, stopping at the private ``for`` token."""

    def produce() -> Iterator[object]:
        iterators = tuple(_iter_or_empty(collection) for collection in collections)
        for values in zip(*iterators):
            mapped = function(*values)
            if mapped is _FOR_STOP:
                return
            yield from _iter_or_empty(mapped)

    return lazy_seq(produce)


def _filter_values(
    predicate: Callable[[object], object], collection: Iterable[object]
) -> Iterator[object]:
    return (
        value
        for value in _iter_or_empty(collection)
        if truthy(predicate(value))
    )


def filterv(
    predicate: Callable[[object], bool], collection: Iterable[object]
) -> tuple[object, ...]:
    """Return the values satisfying ``predicate`` as a vector."""

    return tuple(_filter_values(predicate, collection))


def filter(
    predicate: Callable[[object], bool], collection: Iterable[object]
) -> _LazySeq:
    """Lazily retain values satisfying ``predicate`` using Clojure truthiness."""

    return lazy_seq(lambda: _filter_values(predicate, collection))


def removev(
    predicate: Callable[[object], object], collection: object
) -> tuple[object, ...]:
    """Vector form of ``remove`` using Clojure truthiness."""

    return tuple(value for value in _iter_or_empty(collection) if not truthy(predicate(value)))


def remove(
    predicate: Callable[[object], object], collection: object
) -> _LazySeq:
    """Lazily retain values for which ``predicate`` is false/nil."""

    def produce() -> Iterator[object]:
        for value in _iter_or_empty(collection):
            if not truthy(predicate(value)):
                yield value

    return lazy_seq(produce)


class Reduced(Generic[_Result]):
    """Value marker returned by a reducing callback to stop traversal."""

    __slots__ = ("value",)

    def __init__(self, value: _Result) -> None:
        self.value = value


def reduced(value: _Result) -> Reduced[_Result]:
    """Wrap ``value`` as an early result for ``reduce`` or ``fold``."""

    return Reduced(value)


def reduced_p(value: object) -> bool:
    """Return whether ``value`` carries the reduction stop marker."""

    return isinstance(value, Reduced)


def unreduced(value: Union[_Result, Reduced[_Result]]) -> _Result:
    """Remove one reduction marker, leaving ordinary values unchanged."""

    if isinstance(value, Reduced):
        return value.value
    return value


def _reduce_with_initial(
    function: Callable[[object, object], object],
    initial: object,
    collection: Iterable[object],
) -> object:
    accumulator = initial
    for item in collection:
        accumulator = function(accumulator, item)
        if isinstance(accumulator, Reduced):
            return accumulator.value
    return accumulator


def reduce(function: Callable[[object, object], object], *arguments: object) -> object:
    """Reduce ``collection`` with an optional initial accumulator."""

    if len(arguments) == 1:
        iterator = _iter_or_empty(arguments[0])
        try:
            initial = _builtins.next(iterator)
        except StopIteration:
            return function()
        return _reduce_with_initial(function, initial, iterator)
    if len(arguments) == 2:
        return _reduce_with_initial(
            function,
            arguments[0],
            _iter_or_empty(arguments[1]),
        )
    raise TypeError("reduce expects a function, collection, and optional initial value")


def fold(
    function: Callable[[object, object], object],
    initial: object,
    collection: Iterable[object],
) -> object:
    """Named initial-value form of ``reduce``."""

    return _reduce_with_initial(function, initial, collection)


class _RecurSignal:
    """Private value used by ``loop`` to carry the next state tuple."""

    __slots__ = ("values",)

    def __init__(self, values: tuple[object, ...]) -> None:
        self.values = values


def recur(*values: object) -> _RecurSignal:
    """Return a control token consumed by :func:`loop`."""

    return _RecurSignal(tuple(values))


def loop(function: Callable[..., _Result], *initial: object) -> _Result:
    """Run a Clojure-style state loop without growing the Python stack."""

    state = tuple(initial)
    while True:
        result = function(*state)
        if not isinstance(result, _RecurSignal):
            return result
        if len(result.values) != len(state):
            raise TypeError(
                "recur must supply the same number of values as loop bindings"
            )
        state = result.values


def trampoline(function: Callable[..., object], *arguments: object) -> object:
    """Invoke a function, then bounce through zero-argument callables."""

    result = function(*arguments)
    while callable(result):
        result = result()
    return result
