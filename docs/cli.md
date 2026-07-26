---
document-id: tooling/cli
title: Osiris Command-Line Interface
language: en
revision: 2
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

## Language Server Agent

`osr lsa "<request>"` explains Osiris APIs and returns complete examples that
have been formatted and checked by the compiler. It is deliberately not a
coding agent: it does not edit source or run shell commands. The example is
compiled as a temporary entry in the current Osiris workspace and executed
with the current project Python. Normal Osiris imports, extension interfaces,
`py/import`, and `~python` use the same staging semantics as `osr run`; the
provider still returns only Osiris source, never generated Python.
JSON is the default output so another agent can consume the result directly;
use `--format text` for terminal reading.
Successful execution sets `evaluated: true` and replaces any model-authored
result with the captured runtime value. Execution uses a credential-cleared
temporary runtime with finite time and output limits. Compilation or execution
failure may cause one diagnostic-driven repair request; LSA never enters an
unbounded generation loop.

Every response includes a `sessionId`. Continue a conversation with
`osr lsa --session <id> "<follow-up>"`. Editable JSONC history is stored under
`.osiris/cache/agent/<session-id>/session.jsonc`. `--file <path>` explicitly
adds one project source file as context; project source is not uploaded
implicitly.

LSA uses the OpenAI-compatible protocol selected by the `agent` object in
`osiris.jsonc`: `responses` calls `/responses`, while `chatCompletions` calls
`/chat/completions` and is the compatibility-first default. `OSR_API_KEY` is
required. `OSR_MODEL`, `OSR_BASE_URL`, and `OSR_WIRE_API` override project
values, and a project-root `.env` is supported. The locale precedence is
`--locale`, project `displayLocale`, then request language detection.

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
