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
```

`[tool.uv.sources]` builds `osiris-lang` from the repository root as an ordinary
wheel, so the demo exercises the same binary and packaged-standard-library
layout as an installed release. A published copy can remove that table and
resolve the declared compatible release from its package index.
