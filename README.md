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

Create a new uv project with an Osiris source root and starter module:

```console
osr init my-project
cd my-project
uv run osr run src/main.osr
```

To add Osiris to an existing uv project, run this from its root (or pass the
directory explicitly):

```console
osr init --existing
osr init --existing path/to/project
```

`init` preserves the existing `pyproject.toml` layout, comments, project
metadata, and dependencies. It creates `osiris.jsonc` and `src/main.osr` only
when those files do not exist, and asks `uv` to add
`osiris-lang` to the development dependency group. Re-running the command is
safe. A new project path must not already exist; use `--existing` when joining
an established uv project.

Osiris discovers the nearest `osiris.jsonc`; the adjacent `pyproject.toml`
continues to own Python package metadata and dependencies. JSONC comments and
trailing commas are accepted. A typical configuration is deliberately small:

```jsonc
{
  "$schema": "https://raw.githubusercontent.com/mjason/osiris/main/schemas/osiris.schema.json",
  "source": ["examples"],
  "outDir": "dist",
  "targetPython": "3.11",
  "strict": true,
  "displayLocale": "zh-CN"
}
```

`source` defines the complete project source scope. `exclude` contains
project-root-relative glob rules shared by compilation and language tooling;
a value without glob syntax, such as `src/generated`, also excludes its
descendants.
Patterns such as `src/**/generated/**` and `src/**/*_test.osr` can select files
inside a source root when a project needs those rules. `outDir` is the default
compile destination; artifact selection remains an explicit
`osr compile --emit` option. One invocation targets one Python version.
Changing `targetPython` invalidates target-sensitive analysis, interfaces,
extension resolution, and build artifacts.

`displayLocale` is a closed enum used by hover, completion, and signature
help when Rich Metadata provides localized labels or documentation:

- `"zh-CN"` displays Simplified Chinese labels and documentation when present.
- `"en"` displays English labels and documentation when present.

It changes tooling presentation, not binding identity or generated Python.
An explicit locale sent by an LSP client takes precedence over the project
value. `osr init` writes `"displayLocale": "zh-CN"` by default; change that
single value to `"en"` for an English tooling view.

With that configuration and [`examples/hello.osr`](examples/hello.osr):

```console
cargo run --bin osr -- check examples/hello.osr
cargo run --bin osr -- build
cargo run --bin osr -- watch
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
unchanged. `build` compiles the complete project described by `osiris.jsonc`,
prints the output directory (`dist/` by default), and publishes one artifact
set atomically. `watch` reruns that same build when a non-excluded `.osr`
source changes. `compile` remains the lower-level command for explicit source
and `--emit` control.

- `dist/hello.py` is the readable generated Python module.
- `dist/hello.osri` is the public, versioned Osiris compilation
  interface used by downstream modules and tools.
- `dist/hello.py.map` maps generated Python spans back to source and
  macro-expansion spans.
- A distribution-level `*.records.json` sidecar is emitted only when the
  compiled modules own public static records (or when `--emit records` is
  requested).

Python dependencies and Osiris extensions are ordinary Python project
dependencies. Add them from PyPI (or another index/path supported by `uv`) in
`[project].dependencies`, then let `uv` resolve and lock them. The compiler
automatically reads `osiris.toml` and `.osri` resources only from distributions
reachable in the runtime lock graph; it never imports extension Python code or
scans unrelated installed packages during discovery.

## Publishing an Extension

An Osiris extension is an ordinary Python distribution whose wheel contains
compiled `.osri` interfaces and an automatically generated
`dist-info/osiris.toml` marker. Create one with:

```console
osr init --extension acme-osiris
cd acme-osiris
uv lock
uv build --python 3.11
uv publish dist/*
```

The generated `pyproject.toml` pins the installed compiler distribution and
selects its bundled PEP 517 backend:

```toml
[build-system]
requires = ["osiris-lang==<osr-version>"]
build-backend = "osiris_build"
```

`osr init --extension acme-osiris` creates
`src/acme_osiris/core.osr` with module `acme_osiris.core`. Each public module
is compiled into readable Python plus an `.osri` interface; the backend adds
one `[[extension]]` marker entry for each interface, using the module name
(`acme_osiris.core`) as its ID. Do not write `osiris.toml` by hand.

To convert an existing uv package, run `osr init --existing --extension` from
its root. The command preserves existing metadata and refuses to replace a
different build backend. If that package needs Hatchling, maturin, or another
backend for additional native build work, backend composition is not yet
supported and should be handled as a separate distribution.

Consumers install the published extension exactly like any other dependency:

```console
uv add acme-osiris
uv lock
uv run osr check src/main.osr
```

The compiler follows the consumer's locked runtime dependency graph and reads
the extension's static marker and interfaces without importing its Python
package during discovery. Public interface dependencies of an extension must
therefore be declared in `[project].dependencies`, so they are preserved as
standard `Requires-Dist` metadata.

## Native CLI

```console
cargo run --bin osr -- --version
cargo run --bin osr -- check source.osr
cargo run --bin osr -- build
cargo run --bin osr -- watch
cargo run --bin osr -- compile source.osr
cargo run --bin osr -- expand source.osr
cargo run --bin osr -- inspect --semantic source.osr --format json
cargo run --bin osr -- lsp
cargo test --all-targets --all-features
```

`check` runs the frontend and semantic gates. `build` emits readable Python,
an `.osri` compilation interface, and a `.py.map` source map into `dist/` by
default. `expand` shows macro output. `inspect` exposes either the
lossless syntax tree or the versioned semantic model used by the LSP and Agent
APIs. Compilation errors return status 1; command-line misuse returns status
2.

The reader is implemented as composable `nom` grammar productions over a
lossless token stream. All whitespace, commas, comments, original Unicode
spelling, and raw string spelling remain available to future formatting and
LSP stages. Symbols and keywords also carry an NFC canonical spelling for
collision-safe name resolution.

## Python package

The PyPI wheel carries `osr` as a native Rust executable. Python and its
packaging tools install the wheel, but they do not launch or host the CLI.
The same wheel also contains the `osiris` runtime used by generated Python and
the `osiris_build` PEP 517 backend used by Osiris source distributions. Python
dependencies continue to be declared in `pyproject.toml` and locked by `uv`.

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

The package version is defined once in `Cargo.toml`; maturin uses it for the
platform wheel and places the native executable in the wheel's scripts area.
Consequently `osr`, `osr watch`, and `osr lsp` run without a Python interpreter
process.

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
