---
document-id: tooling/cli
title: Osiris Command-Line Interface
language: en
revision: 1
---

# Osiris Command-Line Interface

`osr` is the native command-line interface for compiling, querying, formatting,
and running Osiris. It does not use a Python process for compilation, watch, or
language-server operation. Concise command help is available with
`osr <command> --help`; this document explains the complete command families.

## Projects

`osr init <project>` creates a uv-compatible project, `osr init --existing`
adds Osiris to an existing uv project, and `osr init --extension <project>`
creates a PyPI extension project using the `osiris_build` backend. Dependency
resolution, locking, installation, and publication remain ordinary uv/PyPI
operations.

Project commands discover `osiris.jsonc` from the selected path. `osr check`
analyzes without writing output. `osr build` compiles the complete configured
source scope into `outDir`. `osr watch` performs the same build after source
changes and exits promptly on an interrupt. `osr run <file> -- <args>` compiles
the source entry and propagates the target program's status.

`osr compile <file>...` is the lower-level explicit compiler entry. Its default
artifacts are readable Python, `.osri` interfaces, and `.py.map` source maps.
Use `--out-dir` to select an output directory and `--emit` to select artifact
kinds. Project and explicit builds link reachable support into the owning
Python package's private `__osiris_runtime__` package.

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
```

Text is the default. `--format json` returns one versioned object. With no
`--locale`, LSC selects authored `:default` documentation and the canonical
name; it does not inherit project `displayLocale`. An explicit locale must be a
BCP 47 tag and is matched with RFC 4647 lookup.

`osr lsp` runs the editor protocol over standard Content-Length framed stdin
and stdout. It uses the same compiler queries and formatter as LSC and `fmt`.

## Embedded Documentation

`osr syntax` prints the complete release-pinned English syntax manual.
`osr syntax --format json` returns its identity, revision, content hash, and
Markdown in one object.

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
