# Osiris demo project

This is a complete `uv` and Osiris project rather than a collection of source
fragments. It demonstrates:

- ordinary imports between `demo.main`, `demo.core`, and `demo.text`;
- the implicitly referred `osiris.core` collection functions;
- a typed `~python` module exposed through `extern python`;
- project configuration, compilation, execution, and LSA workspace context.

From this directory:

```console
uv sync
uv run osr check
uv run osr build
uv run osr run src/demo/main.osr
```

The program prints the incremented values, their total, and normalized text.
LSA uses this complete workspace when it validates generated examples:

```console
uv run osr lsa "Explain demo.core/increment-all and show an imported example"
uv run osr lsa --at src/demo/main.osr:7:37 "Explain this API and show a smaller example"
```

For a broad feature request, LSA first searches the project's persistent
semantic graph and then asks LSP for exact symbol facts. The same facts are
available without a provider:

```console
uv run osr lsc workspace-search "increment every integer" --format json
uv run osr lsc symbol-context demo.core::function::increment-all --format json
```

The disposable graph cache is `.osiris/cache/language-graph.sqlite3`. It
contains Osiris interfaces and relationships, not the body of
`~python<text-tools>`.

`[tool.uv.sources]` builds `osiris-lang` from the repository root as an ordinary
wheel, so the demo exercises the same binary and packaged-standard-library
layout as an installed release. A published copy can remove that table and
resolve the declared compatible release from its package index.
