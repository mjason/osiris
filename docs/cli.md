---
document-id: tooling/cli
title: Osiris Command-Line Interface
language: en
revision: 5
---

# Osiris Command-Line Interface

`osr` is the native command-line interface for compiling, querying, formatting,
and running Osiris. A bare `osr` resolved through PATH first prefers the
project's own compiler: the activated `VIRTUAL_ENV`, then a `.venv` found on
the ancestor path of the working directory, handing the whole invocation —
help, version, and LSP included — to the project's binary, so what answers is
always what the project locked. Invoking a binary by explicit path runs
exactly that binary; `OSR_NO_DELEGATE=1` opts out entirely. It does not use a Python process for compilation, watch, or
language-server operation. Concise command help is available with
`osr <command> --help`; this document explains the complete command families.

## Projects

`osr init <project>` creates a uv-compatible project, `osr init --existing`
adds Osiris to an existing uv project, and `osr init --package <project>`
creates a publishable Osiris package using the `osiris_build` backend. Dependency
resolution, locking, installation, and publication remain ordinary uv/PyPI
operations.

`init` is additive on an existing project: a present `osiris.jsonc`, starter
source, or foreign `build-system` is left alone (the last is refused, not
replaced). A fresh project gets a `.gitignore` that ignores the environment,
caches, and the output directory as a whole; every optional rule is present
but commented — the whole-directory line can be deleted to commit generated
code, and the per-suffix `*.osri` / `*.py.map` rules can be uncommented, which
is the form an `outDir: "."` layout needs since it has no directory to ignore.
An existing `.gitignore` is treated as chosen policy: the machine-local
`.osiris/` cache entry is added active, the optional rules only as comments.

Project commands discover `osiris.jsonc` from the selected path. `osr check`
analyzes without writing output. `osr build` compiles the complete configured
source scope into `outDir`. With `outDir: "."` the artifacts land beside the
sources at the project root — importable and testable with no path prefix —
and publication switches to an in-place mode that only ever deletes artifacts
a previous build recorded, never an authored file. `osr clean` removes what builds produced and nothing
else: a dedicated `outDir` goes whole, while under `outDir: "."` only the
artifacts the publication manifest recorded are deleted — generated files are
never guessed at among authored ones — and the `.osiris/` cache goes in both
modes. `osr watch` performs the same build after source
changes and exits promptly on an interrupt. `osr run <file> -- <args>` compiles
the source entry and propagates the target program's status.

`osr compile <file>...` is the lower-level explicit compiler entry. Its default
artifacts are readable Python, `.osri` interfaces, and `.py.map` source maps.
Use `--out-dir` to select an output directory and `--emit` to select artifact
kinds. A project build (`osr build`, `osr run`) emits all compiler-linked
runtime support into one shared `<outDir>/__osiris_runtime__` tree; bare
`osr compile` defaults to the per-package layout wheels need, and
`--runtime-layout shared|package` selects explicitly. Project and explicit builds link reachable support into the owning
Python package's private `__osiris_runtime__` package.

## Project Configuration

`osiris.jsonc` contains only compiler and tooling settings. It accepts JSONC
comments and trailing commas. The supported fields are:

- `source`: project-relative source roots; defaults to `["src"]`.
- `outDir`: project-relative build destination; defaults to `"dist"` and is
  always excluded from source discovery.
- `exclude`: project-relative paths or glob patterns removed from the shared
  build, watch, check, format, LSC, and LSP source scope.
- `targetPython`: the single Python language target; defaults to `"3.11"`.
- `strict`: whether unresolved dynamic boundaries and incomplete public
  contracts are errors; defaults to `true`.
- `displayLocale`: a BCP 47 locale used by editor presentation, for example
  `"zh-CN"`, `"en"`, or `"ja"`.

For example, this changes the complete build destination while excluding test
fixtures inside the source tree:

```jsonc
{
  "$schema": "https://raw.githubusercontent.com/mjason/osiris/main/schemas/osiris.schema.json",
  "source": ["src"],
  "outDir": "build/osiris",
  "exclude": ["src/**/fixtures/**"],
  "targetPython": "3.11",
  "strict": true,
  "displayLocale": "zh-CN"
}
```

Package name, version, Python dependencies, indexes, and publication metadata
belong in `pyproject.toml`. There is no `watch`, `extensions`, `buildGroups`, or
`trust` field: watch is a command, dependencies are ordinary Python packages,
and trust is expressed by language-level contracts.

## Publishing a Package

Create and publish a reusable Osiris library through the normal Python package
workflow:

```console
osr init --package acme-text
cd acme-text
uv lock
uv build --python 3.11
uv publish dist/*
```

`osr init --existing --package .` converts an existing compatible uv package.
The scaffold selects `osiris_build`, pins a compatible `osiris-lang` build
requirement, creates a canonical public module, and leaves package metadata in
`pyproject.toml`. The wheel contains authored `.osr` source, compiled Python,
`.osri` interfaces, source maps, static metadata, and only the reachable private
runtime support it needs. Consumers install it like any other dependency:

```console
uv add acme-text
uv run osr check src/main.osr
```

Do not create a separate Osiris registry or list packages in `osiris.jsonc`.
The compiler discovers validated static interfaces from the consumer's locked
Python dependency graph without importing package code.

## Canonical Formatting

`osr fmt [<path>...]` applies the one language-wide source format. With no path
it selects the configured project source scope; `osr fmt --all` is the explicit,
Cargo-style spelling for that complete scope. `--all` cannot be combined with a
path or stdin. `osr fmt --all --check` performs no writes and fails when any
project source is not canonical. `osr fmt -` reads one source from stdin and
writes only canonical source to stdout.

Formatting is deterministic and locale-independent. There are no style
settings. A reader error prevents that file from being partially rewritten.

## Expansion and Local Language Services

`osr expand <file>` prints fully macro-expanded Osiris source. `--once` performs
one expansion step. Expansion never imports or executes generated Python.

`osr lsc` is the finite Language Server Console. It provides the information
available from compiler-owned IDE features without requiring an LSP client:

```text
osr lsc diagnostics [<path>]
osr lsc hover <api-name-or-binding-id>
osr lsc hover --at <path>:<line>:<column>
osr lsc completion --at <path>:<line>:<column>
osr lsc signature <api-name-or-binding-id>
osr lsc signature --at <path>:<line>:<column>
osr lsc definition <api-name-or-binding-id>
osr lsc definition --at <path>:<line>:<column>
osr lsc references --at <path>:<line>:<column>
osr lsc rename --at <path>:<line>:<column> --to <name>
osr lsc expand <path>
osr lsc syntax <path>
osr lsc semantic <path>
osr lsc symbol <name-or-binding-id>
osr lsc workspace-search <concept-or-api-name>
osr lsc symbol-context <api-name-or-binding-id>
osr lsc symbol-context --at <path>:<line>:<column>
osr lsc source-context --at <path>:<line>:<column>
osr lsc name <osiris-name>
osr lsc cache status
osr lsc cache rebuild
```

`osr lsc name` answers what a name becomes in generated Python — the mapping is
fixed by OEP-0001-R005A, so it needs no workspace and works anywhere. It reports
the identifier spelling and, for a dotted name, the module-path spelling too,
which differ: `dm.dsl.pandas` is `dm_u2e_dsl_u2e_pandas` as one identifier and
`dm.dsl.pandas` as an import path.

Text is the default. `--format json` returns one versioned object. With no
`--locale`, LSC selects authored `:default` documentation and the canonical
name; it does not inherit project `displayLocale`. An explicit locale must be a
BCP 47 tag and is matched with RFC 4647 lookup.

The composite queries use a compiler-owned semantic graph stored at
`.osiris/cache/language-graph.sqlite3`. The disposable libSQL cache indexes
Osiris modules, symbols, types, Rich Metadata, examples, imports, calls,
references, exports, and aliases. It includes statically discovered Osiris
extension interfaces used by the project, but not ordinary Python packages or
raw embedded-Python bodies. Results use stable `osiris-workspace:///` source
URIs, so the cache does not retain the machine's project path. Deleting the
database only causes it to be rebuilt.

Graph-only searches validate a content fingerprint before full workspace
analysis. A per-input manifest means the normal check only inspects file
metadata and rereads files whose size or modification stamp changed. A fresh
cache opens directly without starting LSP; source, configuration, lock, target,
strictness, or reachable interface changes are detected and rebuilt
automatically. `osr lsc cache status` performs this check without rebuilding.
`osr lsc cache rebuild` ignores both the matching fingerprint and manifest,
rereads all inputs, and performs a complete atomic rebuild for recovery or
diagnosis; it is not needed after ordinary edits.
Both commands report `inputCount`, `reusedHashes`, and `hashedInputs` in JSON;
the text form prints the same counts. A fresh cache should normally report zero
hashed inputs, while a manual rebuild reports zero reused hashes.

`osr lsp` runs the editor protocol over standard Content-Length framed stdin
and stdout. It uses the same compiler queries and formatter as LSC and `fmt`.

## Embedded Documentation

`osr syntax` prints the complete release-pinned English syntax manual.
`osr syntax --format json` returns its identity, revision, content hash, and
Markdown in one object.

`osr agents` prints the complete release-pinned English agent manual: the
working order, identity and provenance rules, and tool failure modes an AI
agent needs before editing Osiris source. It accepts the same `--format`
projection as `osr syntax`. The normative requirements it summarizes are
OEP-0001-R054 through OEP-0001-R056.

`osr doc <graphql-document>` executes exactly one GraphQL query against the
read-only English documentation snapshot embedded in the executable. Use
`osr doc -` for a query read from stdin. Schema introspection is enabled. The
query engine is local and never connects to a documentation service.

## Help and Machine Metadata

`osr --help` and `osr <command> --help` are concise projections of the native
command registry. `osr --help --format json` returns the complete versioned
command definitions. `osr --help --format completion` returns command names,
aliases, and option spellings for shell-completion generators.

## Streams and Status

Requested text or JSON is written to stdout; diagnostics and operational
failures are written to stderr. Compiler-owned stable statuses are `0` for
success, `1` for validation or operation failure, `2` for CLI misuse, and `130`
for POSIX interruption. `run` propagates the invoked program's status after a
successful compile.
