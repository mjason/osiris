# Osiris Standard Library

This directory is both the standard library source package and the reference
layout for publishing reusable Osiris packages through PyPI and `uv`.

The public modules under `src/osiris/` are normative. Their declarations own
the public signatures and Rich Metadata, including English default
documentation and `zh-CN` translations. `osiris.kernel` is a compiler-private
target boundary; applications cannot import it. Public functions and values are
ordinary Osiris `defn`/`def` implementations. Private typed Kernel leaf
declarations carry no API documentation and exist only to terminate those
source-authored implementations at the compiler boundary.

The package uses the same `pyproject.toml`, `osiris.jsonc`, `osiris_build`,
`.osr`, `.osri`, source-map, and wheel marker contracts as third-party Osiris
packages. Generated applications link reachable standard implementations and
Kernel leaves into their own `__osiris_runtime__` package and do not depend on
this package at runtime.
