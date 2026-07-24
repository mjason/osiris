---
document-id: tooling/diagnostics
title: Osiris Diagnostics
language: en
revision: 1
---

# Osiris Diagnostics

Every Osiris diagnostic has a stable ASCII code, severity, primary source
span, and message. Machine projections can also contain related spans, macro
origin chains, binding identities, and structured facts. The diagnostic code,
not translated prose, is the stable identity clients should use.

## Reading Diagnostics

Text diagnostics identify the source path and one-based line and column. LSC
JSON and LSP use structured ranges and include the analyzed document version.
Macro errors retain the call site and expansion origin. Backend source maps
relate generated Python spans to the packaged `.osr` source and its hash.

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

An error in expanded code reports both the authored call and the standard or
extension macro origin. `osr expand --once` is useful when the complete trace
is too large.

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
