# Osiris

Osiris is the project for `osr`, a small, data-oriented Lisp-to-Python
compiler. The language is designed around readable Python output, explicit
Python decorators, hygienic macros, static types, Unicode names, and
tooling-readable metadata.

The current compiler implements a lossless tokenizer, a recoverable `nom`
reader, surface AST lowering, hygienic macro expansion, name and alias
resolution, typed HIR, `defstruct`, static schemas and records, and structured
Python code generation. It also emits deterministic `.osri` interfaces and
source maps, compiles source modules as one dependency graph, and validates
locked static extension interfaces without importing Python packages.

The versioned prelude supplies Clojure-inspired control flow without making
the reader or Rust core grow for every form. The implemented surface includes
threading (`->`, `->>`, `cond->`, `some->`, `as->`, `doto`), binding and
branching (`when`, `if-let`, `if-some`, `case`, `condp`), comprehensions
(`for`, `doseq`, `dotimes`), constant-stack `loop`/`recur`, `letfn`, `defn-` and
`trampoline`, lazy sequences and reductions, structured exceptions/resources
(`assert`, `throw`, `time`),
typed `^:dynamic` Var bindings, and the initial future/promise/locking
primitives plus eager, ordered parallel forms (`pmap`, `pcalls`, `pvalues`) and
the typed sequence predicates (`empty?`, `seq?`, `coll?`, `sequential?`). Dynamic
values use Python `contextvars`,
including context capture when a future is submitted. See the
[control-flow coverage matrix](docs/language-design.md#452-clojure-控制流覆盖矩阵与分期)
for exact semantics, tests, and intentionally deferred facilities such as
`with-bindings`, `with-local-vars`, `with-redefs`, STM, Agents, and the complete
Clojure sequence/transducer protocols. Osiris borrows these designs but does
not claim Clojure compatibility.

## Requirements

- Rust 1.85 or newer
- Python 3.9 or newer for the optional Python package
- `uv` for Python development

## Project Quick Start

Osiris discovers the nearest `pyproject.toml` containing a `[tool.osiris]`
table. The smallest project configuration for the checked-in example is:

```toml
[tool.osiris]
source = ["examples"]
target-python = "3.9"
strict = true
extensions = []
build-groups = []
display-locale = "zh-CN"
```

With that configuration and [`examples/hello.osr`](examples/hello.osr):

```console
cargo run --bin osr -- check examples/hello.osr
cargo run --bin osr -- compile examples/hello.osr
```

The multi-file [`examples/tutorial/app.osr`](examples/tutorial/app.osr)
demonstrates importing another Osiris module with `:as` and `:refer`, importing
a macro with `import-for-syntax`, and keeping Python `py/import` separate:

```clojure
(import tutorial.transforms :as transforms :refer [sum-values])
(import-for-syntax tutorial.macros :refer [unless])
(py/import math :as math)
```

Run `cargo run --bin osr -- check examples/tutorial/app.osr` to analyze the
whole local dependency graph. See [`examples/README.md`](examples/README.md)
for the module-to-path mapping and generated outputs.

`check` parses and validates the project and leaves the working tree
unchanged. `compile` prints the output directory (`target/osr/` by default)
and publishes one artifact set atomically:

- `target/osr/hello.py` is the readable generated Python module.
- `target/osr/hello.osri` is the public, versioned Osiris compilation
  interface used by downstream modules and tools.
- `target/osr/hello.py.map` maps generated Python spans back to source and
  macro-expansion spans.
- A distribution-level `*.records.json` sidecar is emitted only when the
  compiled modules own public static records (or when `--emit records` is
  requested).

Python dependencies are still ordinary Python project dependencies. Add them
from PyPI (or another index/path supported by `uv`) in the standard
`[project].dependencies` or a dependency group, then let `uv` resolve and lock
them. `[tool.osiris].extensions` is only a list of explicitly enabled static
Osiris extension IDs: it points discovery at wheel `osiris.toml` markers and
their `.osri` interfaces; it is not a package registry, installer, or second
lock file, and the compiler never imports extension Python code during
discovery.

## Native CLI

```console
cargo run --bin osr -- --version
cargo run --bin osr -- check source.osr
cargo run --bin osr -- compile source.osr
cargo run --bin osr -- expand source.osr
cargo run --bin osr -- inspect --semantic source.osr --format json
cargo run --bin osr -- lsp
cargo test --all-targets --all-features
```

`check` runs the frontend and semantic gates. `compile` emits readable Python,
an `.osri` compilation interface, and a `.py.map` source map into
`target/osr/`. `expand` shows macro output. `inspect` exposes either the
lossless syntax tree or the versioned semantic model used by the LSP and Agent
APIs. Compilation errors return status 1; command-line misuse returns status
2.

The reader is implemented as composable `nom` grammar productions over a
lossless token stream. All whitespace, commas, comments, original Unicode
spelling, and raw string spelling remain available to future formatting and
LSP stages. Symbols and keywords also carry an NFC canonical spelling for
collision-safe name resolution.

## Python package

The Python package embeds the same Rust core through PyO3 and installs an
`osr` console command. `osiris_build` provides the PEP 517 backend used by
Osiris source distributions; Python dependencies continue to be declared in
`pyproject.toml` and locked by `uv`.

The PyPI distribution is named `osiris-lang` because the `osiris` project name
is already occupied. The installed Python package remains `osiris`:

```console
uv tool install osiris-lang
osr --version
```

For repository development:

```console
uv sync
uv run osr --version
```

The package version is defined once in `Cargo.toml`; maturin supplies it to the
Python package during the build. The Python console script delegates to the
same Rust CLI dispatcher as the native executable, so parsing and diagnostics
do not diverge.

## VS Code

The extension lives in [`editors/vscode`](editors/vscode) and delegates all
semantic behavior to `osr lsp`. Until Marketplace publishing is enabled, open
the repository's [GitHub Releases](https://github.com/mjason/osiris/releases),
select the latest `vscode-vX.Y.Z` release, download its `.vsix`, and run
**Extensions: Install from VSIX...** in VS Code.

Maintainers publish Python releases with a `vX.Y.Z` tag and VS Code releases
with a separate `vscode-vX.Y.Z` tag. Both tags must match the corresponding
package version committed in the repository. Trusted Publisher fields and the
full tag procedure are documented in [`docs/releasing.md`](docs/releasing.md).

The current language design is in
[`docs/language-design.md`](docs/language-design.md). Compiler ownership and
the kernel/macro/extension boundary are documented in
[`docs/architecture.md`](docs/architecture.md).
