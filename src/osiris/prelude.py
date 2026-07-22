"""Small runtime helpers used by code emitted from Osiris prelude macros."""

from __future__ import annotations

import builtins as _builtins
import collections as _collections
import concurrent.futures as _futures
import contextvars as _contextvars
import itertools as _itertools
import threading as _threading
import time as _time
from collections.abc import Callable, Iterable
from typing import Generic, Iterator, Optional, TypeVar, Union

_Result = TypeVar("_Result")
_MISSING = object()

__all__ = [
    "Reduced",
    "filter",
    "filterv",
    "remove",
    "removev",
    "distinct",
    "dedupe",
    "partition",
    "partition_all",
    "partition_by",
    "interleave",
    "interpose",
    "take_last",
    "drop_last",
    "for_stop",
    "fold",
    "is_nil",
    "map",
    "mapcat",
    "mapcatv",
    "mapv",
    "nonempty",
    "present",
    "assert_value",
    "time_value",
    "Delay",
    "Future",
    "Promise",
    "delay",
    "future_call",
    "future_done",
    "future_cancelled",
    "future_cancel",
    "promise",
    "deliver",
    "force",
    "deref",
    "realized",
    "lock",
    "locking",
    "close",
    "reduce",
    "reduced",
    "reduced_p",
    "unreduced",
    "doseq",
    "truthy",
    "dynamic_get",
    "binding_values",
    "loop",
    "recur",
    "trampoline",
    "lazy_seq",
    "cons",
    "concat",
    "count",
    "empty_q",
    "seq_q",
    "coll_q",
    "sequential_q",
    "first",
    "rest",
    "next",
    "nth",
    "seq",
    "empty",
    "take",
    "drop",
    "take_while",
    "drop_while",
    "keep",
    "keep_indexed",
    "map_indexed",
    "iterate",
    "repeat",
    "repeatedly",
    "cycle",
    "sequence",
    "reductions",
    "run_bang",
    "doall",
    "dorun",
    "some",
    "every_q",
    "not_every_q",
    "not_any_q",
]


def truthy(value: object) -> bool:
    """Return Clojure truthiness: only ``None`` and ``False`` are false."""

    return value is not None and value is not False


def is_nil(value: object) -> bool:
    """Return whether ``value`` is the Osiris/Python nil value."""

    return value is None


def present(value: _Result) -> _Result:
    """Expose a value already guarded by an internal control-flow macro."""

    return value


_DYNAMIC_VALUES: _contextvars.ContextVar[Optional[dict[str, object]]] = (
    _contextvars.ContextVar("osiris_dynamic_values", default=None)
)


def dynamic_get(binding_id: str, root: _Result) -> _Result:
    """Read a dynamic Var override, falling back to its module root value."""

    values = _DYNAMIC_VALUES.get()
    if values is None or binding_id not in values:
        return root
    return values[binding_id]  # type: ignore[return-value]


def binding_values(
    binding_ids: Iterable[str],
    values: Iterable[object],
    thunk: Callable[[], _Result],
) -> _Result:
    """Install context-local dynamic values while invoking ``thunk``.

    Callers evaluate all values before entering this helper.  The immutable
    context snapshot makes nested scopes restore correctly and keeps sibling
    asyncio tasks and threads isolated through Python's ``contextvars`` ABI.
    """

    ids = tuple(binding_ids)
    replacements = tuple(values)
    if len(ids) != len(replacements):
        raise ValueError("binding requires the same number of ids and values")
    if not callable(thunk):
        raise TypeError("binding expects a zero-argument callable body")
    if any(not isinstance(binding_id, str) for binding_id in ids):
        raise TypeError("binding ids must be strings")
    if len(set(ids)) != len(ids):
        raise ValueError("binding ids must be unique")
    if not ids:
        return thunk()

    current = _DYNAMIC_VALUES.get()
    nested = {} if current is None else dict(current)
    nested.update(zip(ids, replacements))
    token = _DYNAMIC_VALUES.set(nested)
    try:
        return thunk()
    finally:
        _DYNAMIC_VALUES.reset(token)


def assert_value(condition: object, message: object = None) -> None:
    """Raise ``AssertionError`` when a Clojure-truthy condition is false.

    The compiler calls this helper only from the failure branch of the
    ``assert`` macro, so a message expression keeps Clojure's lazy failure
    behavior.  This is deliberately a runtime exception rather than Python's
    ``assert`` statement, which disappears under ``python -O``.
    """

    if truthy(condition):
        return None
    if message is None:
        raise AssertionError("assertion failed")
    raise AssertionError(message)


def time_value(thunk: Callable[[], _Result]) -> _Result:
    """Evaluate ``thunk``, report elapsed milliseconds, and return its value.

    This is the runtime boundary for the Clojure-style ``time`` macro.  The
    body is evaluated exactly once; exceptions propagate unchanged and do not
    produce a misleading successful timing line.
    """

    if not callable(thunk):
        raise TypeError("time expects a zero-argument callable body")
    started = _time.perf_counter()
    result = thunk()
    elapsed_ms = (_time.perf_counter() - started) * 1000.0
    print(f"Elapsed time: {elapsed_ms:.3f} msecs")
    return result


class Delay(Generic[_Result]):
    """Thread-safe memoized computation used by Clojure-style ``delay``.

    The thunk is invoked at most once.  Both a successful value and an
    exception are cached, so concurrent callers observe one realization and
    subsequent callers receive the same result or exception without rerunning
    user code.
    """

    __slots__ = ("_thunk", "_lock", "_realized", "_value", "_error")

    def __init__(self, thunk: Callable[[], _Result]) -> None:
        if not callable(thunk):
            raise TypeError("delay expects a zero-argument callable")
        self._thunk = thunk
        self._lock = _threading.RLock()
        self._realized = False
        self._value: Optional[_Result] = None
        self._error: Optional[BaseException] = None

    @property
    def realized(self) -> bool:
        return self._realized

    def force(self) -> _Result:
        if self._realized:
            if self._error is not None:
                raise self._error
            return self._value  # type: ignore[return-value]
        with self._lock:
            if not self._realized:
                try:
                    self._value = self._thunk()
                except BaseException as error:
                    self._error = error
                finally:
                    self._realized = True
            if self._error is not None:
                raise self._error
            return self._value  # type: ignore[return-value]


def delay(thunk: Callable[[], _Result]) -> Delay[_Result]:
    """Construct a deferred, memoized computation without invoking ``thunk``."""

    return Delay(thunk)


_TIMEOUT_UNSET = object()
_FUTURE_EXECUTOR = _futures.ThreadPoolExecutor(
    thread_name_prefix="osiris-future"
)


def _timeout_seconds(timeout_ms: object) -> Optional[float]:
    """Convert Clojure's millisecond timeout to Python seconds."""

    if timeout_ms is None:
        return None
    if not isinstance(timeout_ms, (int, float)) or isinstance(timeout_ms, bool):
        raise TypeError("deref timeout must be a non-negative number of milliseconds")
    if timeout_ms < 0:
        raise ValueError("deref timeout must be non-negative")
    return float(timeout_ms) / 1000.0


class Future(Generic[_Result]):
    """A small, readable wrapper around a background Python future."""

    __slots__ = ("_future",)

    def __init__(self, future: object) -> None:
        if not isinstance(future, _futures.Future):
            raise TypeError("Future expects a concurrent.futures.Future")
        self._future = future

    @property
    def done(self) -> bool:
        return self._future.done()

    @property
    def cancelled(self) -> bool:
        return self._future.cancelled()

    def cancel(self) -> bool:
        return self._future.cancel()

    def result(
        self,
        timeout_ms: object = _TIMEOUT_UNSET,
        default: object = _TIMEOUT_UNSET,
    ) -> _Result:
        timeout = None if timeout_ms is _TIMEOUT_UNSET else _timeout_seconds(timeout_ms)
        try:
            value = self._future.result(timeout=timeout)
        except _futures.TimeoutError:
            # ``concurrent.futures.TimeoutError`` aliases the builtin
            # ``TimeoutError``.  A completed task may legitimately raise that
            # exception itself, in which case it must propagate rather than be
            # mistaken for a wait timeout.
            if self._future.done():
                raise
            if default is not _TIMEOUT_UNSET:
                return default  # type: ignore[return-value]
            raise TimeoutError("future deref timed out") from None
        return value  # type: ignore[return-value]


def future_call(function: Callable[[], _Result]) -> Future[_Result]:
    """Schedule a zero-argument callable on the Osiris future executor."""

    if not callable(function):
        raise TypeError("future-call expects a callable")
    context = _contextvars.copy_context()
    return Future(_FUTURE_EXECUTOR.submit(context.run, function))


def future_done(value: object) -> bool:
    """Return whether a future has completed (successfully or exceptionally)."""

    return isinstance(value, Future) and value.done


def future_cancelled(value: object) -> bool:
    """Return whether a future was cancelled before it started."""

    return isinstance(value, Future) and value.cancelled


def future_cancel(value: object) -> bool:
    """Attempt to cancel a future and return Python's cancellation result."""

    if not isinstance(value, Future):
        raise TypeError("future-cancel expects a Future")
    return value.cancel()


class Promise(Generic[_Result]):
    """A one-shot, thread-safe value that can be delivered by another task."""

    __slots__ = ("_event", "_lock", "_realized", "_value")

    def __init__(self) -> None:
        self._event = _threading.Event()
        self._lock = _threading.Lock()
        self._realized = False
        self._value: Optional[_Result] = None

    @property
    def realized(self) -> bool:
        return self._realized

    def deliver(self, value: _Result) -> "Promise[_Result]":
        """Deliver once; later deliveries leave the original value untouched."""

        with self._lock:
            if not self._realized:
                self._value = value
                self._realized = True
                self._event.set()
        return self

    def deref(
        self,
        timeout_ms: object = _TIMEOUT_UNSET,
        default: object = _TIMEOUT_UNSET,
    ) -> _Result:
        timeout = None if timeout_ms is _TIMEOUT_UNSET else _timeout_seconds(timeout_ms)
        if not self._event.wait(timeout):
            if default is not _TIMEOUT_UNSET:
                return default  # type: ignore[return-value]
            raise TimeoutError("promise deref timed out")
        return self._value  # type: ignore[return-value]


def promise() -> Promise[object]:
    """Create an undelivered promise."""

    return Promise()


def deliver(value: object, result: _Result) -> Promise[_Result]:
    """Deliver ``result`` to a promise and return that promise."""

    if not isinstance(value, Promise):
        raise TypeError("deliver expects a Promise as its first argument")
    return value.deliver(result)


def force(value: _Result) -> _Result:
    """Realize a ``Delay``; ordinary values pass through unchanged."""

    if isinstance(value, Delay):
        return value.force()  # type: ignore[return-value]
    return value


def deref(value: _Result, *options: object) -> _Result:
    """Deref a delay, future, promise, or ordinary value.

    ``options`` is either empty or ``(timeout-ms, default)``.  Timeout units
    follow Clojure and are milliseconds; ordinary values remain idempotent.
    """

    if len(options) not in (0, 2):
        raise TypeError("deref expects one or three arguments")
    timeout = options[0] if options else _TIMEOUT_UNSET
    default = options[1] if options else _TIMEOUT_UNSET
    if isinstance(value, Delay):
        return value.force()
    if isinstance(value, Future):
        return value.result(timeout, default)
    if isinstance(value, Promise):
        return value.deref(timeout, default)
    return value


def realized(value: object) -> bool:
    """Return whether a delay-like value has already been realized."""

    return (
        (isinstance(value, Delay) and value.realized)
        or (isinstance(value, Promise) and value.realized)
        or (isinstance(value, Future) and value.done)
    )


def lock() -> object:
    """Create a reentrant lock suitable for :func:`locking`."""

    return _threading.RLock()


def locking(value: object, thunk: Callable[[], _Result]) -> _Result:
    """Acquire ``value`` around ``thunk`` and always release it."""

    if not callable(thunk):
        raise TypeError("locking expects a zero-argument callable body")
    acquire = getattr(value, "acquire", None)
    release = getattr(value, "release", None)
    if callable(acquire) and callable(release):
        acquired = acquire()
        if acquired is False:
            raise RuntimeError("locking could not acquire the lock")
        try:
            return thunk()
        finally:
            release()
    enter = getattr(value, "__enter__", None)
    exit_ = getattr(value, "__exit__", None)
    if callable(enter) and callable(exit_):
        enter()
        try:
            result = thunk()
        except BaseException as error:
            if exit_(type(error), error, error.__traceback__):
                return None  # type: ignore[return-value]
            raise
        else:
            exit_(None, None, None)
            return result
    raise TypeError("locking expects an object with acquire/release methods")


def close(value: object) -> None:
    """Close one Clojure ``with-open`` resource, ignoring ``None``.

    Python objects may expose either a conventional ``close`` method or no
    close operation at all.  The latter is treated as an already-closed
    extension value so resource adapters can be used without wrapper classes.
    """

    if value is None:
        return None
    closer = getattr(value, "close", None)
    if closer is None:
        return None
    if not callable(closer):
        raise TypeError("with-open resource has a non-callable close attribute")
    closer()
    return None


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


def mapv(
    function: Callable[..., _Result], *collections: Iterable[object]
) -> tuple[_Result, ...]:
    """Apply ``function`` lazily through ``map`` and materialize a vector."""

    return tuple(
        _builtins.map(function, *(_iter_or_empty(collection) for collection in collections))
    )


def map(
    function: Callable[..., _Result], *collections: Iterable[object]
) -> list[_Result]:
    """Apply ``function`` in order and materialize a Lisp list.

    ``mapv`` is the explicit tuple/vector variant.  Keeping this helper
    materialized gives the typed core a deterministic ``List`` result while
    ``lazy_seq`` remains available when a caller needs deferred production.
    """

    return list(
        _builtins.map(function, *(_iter_or_empty(collection) for collection in collections))
    )


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
) -> list[object]:
    """List variant of :func:`mapcatv`."""

    return list(mapcatv(function, *collections))


def filterv(
    predicate: Callable[[object], bool], collection: Iterable[object]
) -> tuple[object, ...]:
    """Return the values satisfying ``predicate`` as a vector."""

    return tuple(
        value
        for value in _iter_or_empty(collection)
        if truthy(predicate(value))
    )


def filter(
    predicate: Callable[[object], bool], collection: Iterable[object]
) -> list[object]:
    """Return the values satisfying ``predicate`` as a Lisp list."""

    return [
        value
        for value in _iter_or_empty(collection)
        if truthy(predicate(value))
    ]


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
            raise TypeError("reduce() of empty iterable with no initial value") from None
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
                yield list(window)
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
                yield padded
                return
            else:
                yield list(window)

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
