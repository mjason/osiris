---
document-id: tooling/diagnostics
title: Osiris Diagnostics
language: en
revision: 3
---

# Osiris Diagnostics

Every Osiris diagnostic has a stable ASCII code, severity, primary source
span, message, and an ordered list of related locations. Each related location
carries a machine-readable `kind`, a `span`, the `module` that span belongs to
when it is not the reported one, and a `binding_id` when it names a macro. The
diagnostic code and these fields, not translated prose, are the stable identity
clients should use.

## Reading Diagnostics

Text diagnostics identify the source path and one-based line and column. LSC
JSON and LSP use structured ranges and include the analyzed document version.
Backend source maps relate generated Python spans to the packaged `.osr` source
and its hash.

The reported column counts characters, which is what an editor expects, while
the caret is placed in terminal columns so that it lands under the span in
mixed-script source. A CJK or full-width character occupies two columns, a
combining mark none, and a tab advances to the next eight-column stop. A quoted
line longer than 100 columns is windowed around the span with `...`, because a
line the terminal folds puts the caret under the wrong row.

Ambiguous-width characters — `→`, `※`, `±`, the box-drawing set — are one column
beside Latin text and two beside CJK text, and a terminal resolves this by its
font rather than by locale. They are measured as one column by default. Set
`OSIRIS_EAST_ASIAN_WIDTH=wide` when reading Osiris output in a terminal
configured for CJK. This affects presentation only: the canonical source format
always measures them as one column so `osr fmt --check` gives the same answer
everywhere.

A diagnostic reported against macro-generated syntax carries the whole
expansion chain, outermost macro first, with a `macro-call-site` and a
`macro-definition` entry per macro:

```
chain.osr:2:27: error[OSR-N0012]: unknown name `no-such-fn`
   |
 2 | (defmacro inner [value] `(no-such-fn ~value))
   |                           ^^^^^^^^^^
  = note: expanded from macro `outer` called here (chain.osr:4:26)
  = note: macro `outer` is defined here (chain.osr:3:1)
  = note: expanded from macro `inner` called here (chain.osr:4:26)
  = note: macro `inner` is defined here (chain.osr:2:1)
```

A span is a byte range with no file identity, so syntax coming from an imported
macro's template is reported at the call site and the defining module is named
in the related entry instead. `osr lsc diagnostics --format json` and LSP
`publishDiagnostics` expose the same entries; LSP additionally publishes the
same-module ones as `relatedInformation` jump targets.

Use `osr check` for project diagnostics or
`osr lsc diagnostics <path> --format json` for a stable local-tooling object.
Fix reader errors first, then expansion/name/type errors, and run `osr fmt`
before checking again.

## Code Families

| Prefix | Area | Typical cause |
| --- | --- | --- |
| `OSR-R` | Reader | Malformed fixed syntax, collection, string, or metadata prefix. |
| `OSR-A` | AST | Invalid declaration, parameter, binding, import, or metadata shape. |
| `OSR-M` | Macro phase | Expansion limit, invalid macro output, phase access, or syntax error. |
| `OSR-N` | Names | Duplicate, ambiguous, non-normalized, reserved, or colliding name. |
| `OSR-H` | HIR | Invalid resolved declaration, import, call, control flow, or export. |
| `OSR-T` | Types | Arity, inference, annotation, nominal, operator, or boundary mismatch. |
| `OSR-S` | Static data | Invalid schema, record, ownership, index, or record identity. |
| `OSR-I` | Interface | Invalid `.osri`, ABI/hash mismatch, dependency graph, or artifact data. |
| `OSR-G` | Package graph | Module identity, source mapping, cycle, or dependency mismatch. |
| `OSR-C` | Compiler/build | Target, workspace, artifact, or configuration failure. |
| `OSR-B` | Backend | Structured Python generation or target validation failure. |
| `OSR-F` | Formatter | Source cannot be formatted without changing reader meaning. |
| `OSR-L` | Language service | Invalid local query, document version, position, or edit request. |
| `OSR-D` | Documentation | Embedded snapshot or GraphQL query initialization failure. |

Numbers are stable within a family but are not severity or ordering. Clients
must read the explicit severity field.

## Reader and Metadata Failures

The reader never treats malformed fixed syntax as a different atom. A reader
diagnostic can recover at a deterministic form boundary so later diagnostics
remain visible. Rich Metadata is immutable data: invalid `:doc` locale maps,
localized names, or type metadata are reported at the attached source node.

The localized metadata contract uses these interface diagnostics:

| Code | Meaning |
| --- | --- |
| `OSR-I0085` | `:doc` is empty, malformed, lacks `:default`, or contains an invalid or duplicate normalized locale. |
| `OSR-I0086` | `:osiris/names` has an invalid locale entry, unknown key, non-symbol name, or duplicate normalized name. |
| `OSR-I0087` | An exported declaration or macro has no authored `:doc`. |

Unicode identity uses NFC. A warning can preserve authored spelling while
showing the canonical identity and any generated-Python collision.

## Macro Failures

Macro diagnostics distinguish invalid declaration/import phase graphs from
runtime code. Phase 1 has deterministic limits for steps, depth, expansion
count, nodes, and metadata resources. It cannot access Python, files, network,
environment variables, clocks, randomness, subprocesses, or threads.

An error in expanded code reports both the authored call and the macro origin
through the related locations described above, for local, standard, and package
macros alike. `osr expand --once` is useful when the complete trace is too
large.

## Type and Boundary Failures

An omitted type requests inference. Published interfaces cannot contain
unresolved `Unknown`; explicit `Any` is required at a dynamic boundary.
`defstruct` fields require stable explicit types. Python behavior should be
declared with a typed `extern` or left explicitly dynamic through
`osiris.python` operations.

Type diagnostics preserve the canonical binding ID. Aliases and localized
names do not create a second type identity.

## Package and Artifact Failures

Package discovery reads static wheel metadata and `.osri` files without
importing package code. It fails closed on lock/provider mismatch, incompatible
compiler/language/standard/helper ABI, escaping or duplicate paths, missing
authored source, stale source maps, or any hash mismatch.

Generated support is private to one distribution under
`__osiris_runtime__`. A source module occupying that reserved path is rejected
before output is written.

## Reporting a Diagnostic Bug

Include the `osr --version` output, diagnostic code, target Python, minimal
formatted `.osr` source, and the JSON result from `osr lsc diagnostics`. Do not
remove binding IDs, source spans, interface hashes, or macro origins needed to
reproduce identity and phase behavior. Remove unrelated private source and
credentials.
