"""Control-flow, concurrency, and resource helpers for emitted code."""

from __future__ import annotations

import concurrent.futures as _futures
import contextvars as _contextvars
import threading as _threading
import time as _time
from collections.abc import Callable, Iterable
from typing import Generic, Optional, TypeVar

_Result = TypeVar("_Result")

__all__ = [
    "Delay",
    "Future",
    "Promise",
    "assert_value",
    "binding_values",
    "close",
    "delay",
    "deliver",
    "deref",
    "dynamic_get",
    "force",
    "future_call",
    "future_cancel",
    "future_cancelled",
    "future_done",
    "is_nil",
    "lock",
    "locking",
    "present",
    "promise",
    "realized",
    "time_value",
    "truthy",
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
