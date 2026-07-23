"""Sequence realization and predicate consumers."""

from __future__ import annotations

import builtins as _builtins
from collections.abc import Callable

from ._sequence_core import _MISSING, _iter_or_empty
from .control import truthy

def run_bang(function: Callable[[object], object], collection: object) -> None:
    """Invoke ``function`` for each item and discard all results."""

    for value in _iter_or_empty(collection):
        function(value)
    return None


def _realize_prefix(arguments: tuple[object, ...]) -> object:
    if len(arguments) == 1:
        limit = None
        collection = arguments[0]
    elif len(arguments) == 2:
        limit = arguments[0]
        collection = arguments[1]
        if not isinstance(limit, int) or isinstance(limit, bool):
            raise TypeError("realization count must be an integer")
    else:
        raise TypeError("expected collection or count and collection")
    iterator = _iter_or_empty(collection)
    if limit is None:
        for _ in iterator:
            pass
    else:
        for _ in range(max(0, limit)):
            if _builtins.next(iterator, _MISSING) is _MISSING:
                break
    return collection


def doall(*arguments: object) -> object:
    """Realize all, or a bounded prefix of, an iterable and return it."""

    return _realize_prefix(arguments)


def dorun(*arguments: object) -> None:
    """Realize all, or a bounded prefix of, an iterable and return nil."""

    _realize_prefix(arguments)
    return None


def some(predicate: Callable[[object], object], collection: object) -> object:
    """Return the first Clojure-truthy predicate result, or nil."""

    for value in _iter_or_empty(collection):
        result = predicate(value)
        if truthy(result):
            return result
    return None


def every_q(predicate: Callable[[object], object], collection: object) -> bool:
    """Clojure-truthy all predicate check."""

    return all(truthy(predicate(value)) for value in _iter_or_empty(collection))


def not_every_q(predicate: Callable[[object], object], collection: object) -> bool:
    return not every_q(predicate, collection)


def not_any_q(predicate: Callable[[object], object], collection: object) -> bool:
    # ``some`` returns the predicate's value, and Python's ``not`` would
    # incorrectly treat Clojure-truthy values such as ``0``/``""`` as false.
    return not truthy(some(predicate, collection))
