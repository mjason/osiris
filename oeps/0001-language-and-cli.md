---
oep: 1
title: Language and CLI Documentation Foundation
description: Core language boundaries, source semantics, command definitions, embedded English documentation, and local AI-facing tooling contracts.
author: MJ
status: Draft
type: Standards Track
areas:
  - Language
  - Compiler
  - CLI
  - Documentation
  - AI
created: 2026-07-23
updated: 2026-07-31
revision: 33
requires: [0]
replaces: []
superseded-by: null
resolution: null
translations:
  zh: local/zh/0001-language-and-cli.md
---

# OEP-0001: Language and CLI Documentation Foundation

## Abstract

This proposal defines the stable foundation of the Osiris language and its
`osr` command-line interface. It fixes the source and reader boundary, module
and binding identity, the compiler kernel, macro phases, Rich Metadata, type
annotations, `defstruct`, Python generation principles, diagnostics, and
semantic queries. It also makes canonical source formatting a core tooling
contract rather than an editor preference.

Since OEP-0005, the notation this proposal specifies is the language's
**form level**: the data structure macros receive, the `.osr` transitional
input, and the `osr expand` output. Osiris source is authored on the
OEP-0005 surface (`.ois`), which reads to exactly these forms. Examples in
this document deliberately remain in form notation — it is the semantics
being specified.

It also defines a structured CLI command model and two deliberately separate
documentation systems. Every native `osr` executable embeds a content-addressed,
read-only libSQL snapshot of complete authored English documents. `osr syntax`
prints the release's syntax manual directly, while `osr doc` executes GraphQL
in-process over the same snapshot. A local compiler query engine exposes
source-embedded API documentation through a human-readable CLI and LSP without
uploading project source.

## Motivation

Osiris is intended to stay small while supporting a surface language that can
grow through macros and packages. That requires a precise boundary: reader and
semantic invariants belong to the language; conveniences such as threading and
comprehensions normally belong to reviewed macros; domain behavior belongs to
extensions.

A language optimized for AI-assisted work also needs queryable sources of truth.
Short `--help` output is insufficient for learning the language, and many AI
agents cannot speak editor protocols. Complete manuals and live code facts have
different ownership and privacy boundaries: manuals can be published with the
compiler, while workspace bindings, signatures, aliases, and metadata must
remain local. Osiris therefore uses a release-pinned embedded document snapshot
for the former and one compiler-owned local query model with CLI and LSP access
for the latter.

## Scope

This proposal specifies:

- the source, reader, binding, module, phase, and kernel boundaries;
- Rich Metadata, aliases, type annotations, and nominal structures;
- macro expansion and Python backend invariants;
- canonical formatting, diagnostics, source maps, syntax inspection, and
  semantic inspection;
- the public `osr` command families and what constitutes a command definition;
- embedded libSQL snapshots of complete English documents, FTS5 search, and
  in-process GraphQL access;
- local source-documentation queries through CLI and LSP;
- localization, versioning, exit behavior, and machine-readable output;
- the required AI workflow for writing and checking Osiris source.

This proposal does not specify:

- the full standard macro or function inventory, which belongs to OEP-0003;
- project configuration, dependency resolution, wheel contents, or extension
  discovery, which belong to OEP-0002;
- standard LSP protocol behavior not specialized by this proposal;
- a particular Rust parser type, private libSQL table layout, or Python AST
  library;
- formatter style or a runtime metadata wrapper for arbitrary Python values.

## Terminology

- **Reader form**: fixed syntax recognized before macro expansion.
- **Syntax value**: an immutable datum with source and lexical context used by
  Phase 1.
- **Kernel form**: a compiler-owned form that establishes semantics a macro
  cannot preserve by ordinary expansion.
- **Surface form**: user-visible syntax that may be implemented by a macro.
- **Phase 1**: deterministic compile-time evaluation of macros and helpers.
- **Phase 0**: runtime evaluation represented by typed HIR and generated Python.
- **Canonical binding ID**: locale-independent identity of a declaration,
  parameter, field, type, or macro.
- **Authored metadata**: immutable, non-executable data supplied by source or a
  macro and kept separate from compiler-proven facts.
- **Command definition**: the structured public contract for one CLI command.
- **Reference collection**: accepted normative English documents published for
  a release snapshot.
- **Discussions collection**: non-normative English Draft or Review documents
  optionally published for a preview release snapshot.
- **Documentation snapshot**: an immutable, content-addressed libSQL database
  containing the complete authored English documents and derived FTS5 indexes
  selected for one compiler release and publication channel.
- **Embedded documentation engine**: the in-process, read-only GraphQL query
  engine over the documentation snapshot embedded in the native `osr` binary.
- **Local code documentation engine**: the compiler-owned query model built
  from current workspace `.osr` and dependency `.osri` data and exposed through
  CLI and LSP.

## Specification

### Source and reader boundary

**OEP-0001-R001:** Osiris source files MUST use the `.osr` extension and MUST
be interpreted as UTF-8. A byte-order mark MAY be accepted but MUST NOT affect
source positions or binding spelling.

**OEP-0001-R002:** The reader MUST recognize lists, vectors, maps, sets,
symbols, keywords, strings, numbers, booleans, `none`, comments, quote, syntax
quote, unquote, unquote-splicing, embedded-language sigils, and Rich Metadata
prefixes as fixed reader forms. Commas outside strings MUST be treated as
whitespace.

**OEP-0001-R003:** The reader MUST preserve original spelling, trivia, and byte
spans in a lossless source model while also exposing normalized datum values.
Recoverable reader errors MUST retain subsequent forms when a deterministic
recovery point exists.

**OEP-0001-R004:** User code and extension packages MUST NOT register tokenizer
rules, reader macros, tagged literals, parser productions, or backend lowering
callbacks. New user-facing syntax MUST use ordinary data forms and Phase-1
macros unless a later accepted OEP changes the fixed reader.

**OEP-0001-R005:** Symbol comparison and binding lookup MUST use Unicode NFC
canonical spelling while diagnostics and source maps preserve authored
spelling. Generated Python name collision checks MUST also account for the
target Python identifier normalization rules.

**OEP-0001-R005A:** The mapping from an Osiris identifier to a Python
identifier MUST be total, deterministic, and injective, and it is a stable part
of the compiler's public interface: generated Python is code a person imports
and calls, so a change to this mapping breaks callers and MUST be treated as a
compatibility break rather than an implementation detail. The rules are:

| Osiris | Python |
| --- | --- |
| `-` | `_` |
| `?` | `_p` |
| `!` | `_bang` |
| letter or digit, including non-ASCII | kept as written, NFC normalized |
| any other character | `_u<lowercase hex code point>_` |

A result that would begin with a digit MUST gain a leading `_`; a result equal
to a Python keyword MUST gain a trailing `_`; an empty result MUST become
`_osiris_empty`. A compiler-internal name MUST be prefixed `_osr_` so it cannot
collide with an authored one. Because the mapping is not surjective, two
distinct Osiris names MAY still produce one Python name only if the language
forbids them in the same scope; where it does not, the collision MUST be
diagnosed under OEP-0001-R005.

**OEP-0001-R005B:** A module path MUST map to a Python import path by applying
OEP-0001-R005A to each component and rejoining with `.`. Where a module path is
used as a single identifier, such as an import alias, OEP-0001-R005A MUST be
applied to the whole dotted string, so the separators become `_u2e_`.

**OEP-0001-R005C:** A distribution name MUST be validated before it is
normalized: it MUST be non-empty, MUST begin and end with an ASCII letter or
digit, and MUST otherwise contain only ASCII letters, digits, `-`, `_`, and `.`.
A valid name MUST then be normalized per PEP 503 by replacing each run of
`-`, `_`, or `.` with a single `-` and lower-casing, and escaped per PEP 427 and
PEP 625 by replacing `-` with `_` wherever a filename or directory name carries
it — the wheel filename, the `.dist-info` directory, the sdist filename, and the
sdist root directory alike. Validation is required before normalization because
the two are only equivalent on valid input; skipping it lets a name through that
one component accepts and another rejects.

**OEP-0001-R005D:** Every one of these mappings MUST have exactly one
authoritative implementation per component that derives them, and components
that must agree MUST be held to that agreement by a test rather than by
convention. A second, independently written copy of one of these rules is the
defect this requirement exists to prevent.

**OEP-0001-R006:** Each fixed prefix and collection form MUST have an explicit,
independently testable grammar and recovery contract. A malformed fixed form
MUST NOT silently fall back to an atom with different meaning, and domain
syntax MUST NOT be introduced through atom-token ambiguity.

**OEP-0001-R006A:** The fixed reader MUST support embedded-language sigils with
this grammar:

```text
~<language><label><body></label>
```

`language` MUST be a non-empty lowercase ASCII identifier beginning with a
letter and continuing with ASCII letters, digits, `+`, or `-`. `label` is
required, MUST be non-empty NFC UTF-8 without newline, `<`, or `>`, and MUST NOT
have leading or trailing whitespace. The exact authored label in `</label>`
terminates the body. A body that needs that byte sequence MUST choose a different
label; the reader defines no escape sequence. Sigil bodies are raw: quotes,
reader escapes, unquote, interpolation, metadata prefixes, comments, and
collection delimiters have no Osiris meaning inside them. The lossless source
model MUST retain the language, semantic label, authored closing spelling, raw
body bytes, and the byte/line mapping of every embedded position.

**OEP-0001-R006B:** A generic embedded-language sigil is a top-level named
declaration. Its label MUST be a valid Osiris binding name and declares a `Str`
whose value is the normalized raw body. The binding has ordinary module
visibility: it is private by default and crosses a module boundary only when
published under OEP-0001-R009A, by either explicit form. Interfaces and tooling
MUST retain its language and source map:

```clojure
~markdown<usage>
# Usage

Pass the value to `render`.
</usage>

(render usage)
```

The ordinary module rules therefore permit an explicitly exported data block
to be consumed elsewhere:

```clojure
;; app/resources.osr
(module app.resources)

~json<default-config>
{"theme": "dark"}
</default-config>

(export [default-config])
```

```clojure
;; app/main.osr
(module app.main)

(import app.resources [default-config])

(load-config default-config)
```

Built-in generic languages MUST include `osiris`, `markdown`, `sql`, `json`,
`html`, `css`, `javascript`, `typescript`, `toml`, and `yaml`; an unknown
well-formed language remains a valid raw embedded text declaration and MUST NOT
load compiler code. Packages MUST NOT register reader callbacks, lowering
callbacks, or executable language handlers. This open data identifier space
does not weaken OEP-0001-R004.

**OEP-0001-R006C:** `python` is the built-in embedded Python-module language. Its
label MUST be a valid Osiris binding name and its body MUST contain exactly one
Python module:

```clojure
~python<text-backend>
from __future__ import annotations

def normalize(value: str) -> str:
    return value.strip().casefold()
</text-backend>

(extern python text-backend
  (defn ^Str normalize
    [^Str value]))
```

The declaration creates a package-owned linkable Python provider handle; it
does not evaluate to `Str`, execute during compilation, or create an ordinary
installed-package import. The handle is module-private, is valid only where an
embedded provider is accepted, and MUST NOT be exported, imported, passed to a
function, or emitted as a runtime value. `extern python <handle>` MUST resolve
that exact local declaration, while `extern python "<module>"` MUST continue to
name an ordinary external Python module. The compiler MUST NOT fall back from a
handle to installed Python or scan/import Python to guess a provider. Duplicate
handles and collisions after Python identifier lowering MUST be diagnosed. The
compiler MUST derive and expose a deterministic relocatable logical module
identity from the owning Osiris module and handle. The extern declaration's
validated bindings form the only Osiris-visible API of the embedded module and
are exported or imported through the ordinary module rules.

**OEP-0001-R006CA:** An embedded Python provider MAY name a file instead of
carrying its body inline:

```clojure
(py/embed text-backend "backend/text.py")

(extern python text-backend
  (defn ^Str normalize
    [^Str value]))
```

The two forms MUST be indistinguishable everywhere downstream. The referenced
content becomes the provider body, so relocation into the owning distribution's
private runtime package, the linkable closure, hashing, and `extern python
<handle>` resolution MUST behave exactly as they do for an inline body, and
generated Python MUST continue to require no Osiris runtime package. The form is
an authored boundary declaration: a macro MUST NOT generate one, because the
handle identity is an authored decision under OEP-0001-R006C.

`py/embed` exists because the inline form makes a copy the natural way to work:
Python is drafted in a `.py`, moved into the block, and the original is left
behind to drift. A referenced file is one source of truth, is testable without a
build, and lets ordinary Python tooling read it directly.

**OEP-0001-R006CB:** The reference MUST be a relative path with `/` separators,
MUST resolve inside a configured source root, MUST NOT traverse upward, and MUST
NOT be a symlink. A referenced file MUST NOT also be a compiled Osiris source, and
MUST NOT be referenced by more than one provider: one file has one owner. A
missing file, an escaping path, a symlink, and a duplicate reference MUST each be
a distinct diagnostic rather than a fallback.

**OEP-0001-R006CC:** Compilation MUST remain a function of source text and
options: the compiler core MUST NOT read the filesystem to resolve a reference.
The caller MUST resolve every reference and pass the content in, exactly as it
already passes resolved external interfaces. The resolved content and its hash
MUST be recorded so an artifact identifies what it embedded, and the interface
MUST change when that content changes — the body is the provider's
implementation, so this is a semantic change, not a documentation one.

**OEP-0001-R006CE:** The standard library MUST NOT use `py/embed`. Resolution is
the caller's responsibility under OEP-0001-R006CC, and while the compiler builds
its own standard library there is no caller to resolve anything — the reference
would resolve to nothing and the provider would be empty. A standard module
carries its provider body inline.

**OEP-0001-R006CD:** Tooling MUST treat the referenced file as the authored
source it is. `osr fmt` MUST format it in place with the same pinned profile it
applies to an inline body, and a source map MUST identify the referenced file
rather than the `.osr` that names it, so a runtime traceback points at the line a
person can edit.

**OEP-0001-R006D:** A body beginning immediately after the opening tag's newline
and ending on the closing tag's own line MUST remove that structural first/last
newline and the closing tag's indentation from
each non-empty body line. Relative indentation after that removal is literal.
An under-indented non-empty line MUST be diagnosed. Inline bodies preserve all
bytes between tags. The canonical formatter MUST NOT rename the label or change
generic body content and MUST preserve an exact source mapping through
indentation normalization.

**OEP-0001-R006E:** A Rich Metadata field whose schema permits an embedded
content reference MAY contain an unquoted Symbol naming a generic embedded
binding in the same source module. Resolution MUST be static, order-independent,
and restricted by the field's required language; it MUST NOT evaluate the
binding or arbitrary metadata expressions. The resolved normalized content,
language, source span, label, and content hash MUST be retained in `.osri` and
tooling records. A reference used only by metadata does not make the `Str`
binding runtime-reachable; ordinary code use and explicit export retain their
normal runtime meaning.

### Modules, names, and aliases

**OEP-0001-R007:** Every compilable source module MUST have one canonical
module identity. File-to-module mapping is a project contract specified by
OEP-0002; source `(module ...)` declarations, when present, MUST agree with it.

**OEP-0001-R008:** Osiris module imports, Phase-1 imports, and Python runtime
imports MUST remain distinct operations. Loading an Osiris interface MUST NOT
import or execute the corresponding Python module.

**OEP-0001-R009:** Public cross-module access MUST be determined by explicit
exports and resolved interfaces. Source order, filesystem enumeration, display
locale, and Python import side effects MUST NOT change name resolution.

**OEP-0001-R009A:** A declaration MUST be published by either of two explicit
forms: naming it in a module-level `(export [...])` manifest, or marking the
declaration itself with `^:export`. The public surface MUST be the union of the
two, and a name published both ways MUST be published once. Any value other
than `true` under the `:export` key MUST remain ordinary authored metadata and
MUST NOT publish. A declaration published by neither form MUST remain module
private; no other spelling, namespace component, or authored key MUST imply
visibility.

A marker MUST be accepted wherever the manifest can publish the same
declaration, including declarations an `extern` block nests. Where a marker
cannot publish — a form with no public name, or an embedded Python provider
handle, which is not a binding — the compiler MUST report it rather than ignore
it, as the manifest already reports a name it cannot publish.

**OEP-0001-R009B:** Because a marker rides on the declaration it publishes
rather than on an authored boundary form, a declaration macro MAY generate one.
The public surface a macro contributes therefore appears only in the interface
built from the expanded surface, and the compiler MUST converge on that surface
before publishing an artifact. This does not relax the authored boundary:
`module`, `import` and `export` remain fixed before expansion, and a macro that
generates one MUST still be rejected.

**OEP-0001-R009C:** Every code path that decides whether a declaration is
published MUST reach that decision through one shared entry point. A component
MUST NOT match the manifest form directly to answer the question, because a path
that does so silently ignores the marker: the declaration compiles, nothing is
published, and no diagnostic is produced.

This is not a style rule. Five separate paths decide publicity — the provisional
interface, HIR lowering, static-record owners, the Phase-1 interface, and
standard-API extraction — and each of the four added after the marker had to be
found by noticing its absence downstream rather than by a failing test. A
component that must agree with the others MUST be held to that agreement by a
test, as OEP-0001-R005D already requires of the name mappings.

**OEP-0001-R010:** Every declaration, parameter, field, type, and macro MUST
have one locale-independent canonical binding ID. Chinese or other localized
preferred names and aliases MUST resolve to that identity rather than creating
a second declaration.

**OEP-0001-R011:** An alias declaration MUST identify its target unambiguously,
MUST participate in duplicate and collision diagnostics, and MUST preserve the
canonical target in semantic output and generated keyword arguments. Localized
names MUST be presentation data, never the basis of type equality or linkage.

### Kernel and phase boundary

**OEP-0001-R012:** The compiler-owned kernel MUST be closed and versioned. A
form belongs in the kernel only when it establishes module or phase separation,
lexical evaluation, binding or nominal identity, static interface data, Python
ABI, exception control flow, or another semantic boundary that macros cannot
preserve.

**OEP-0001-R013:** The kernel MUST provide semantics for module/import/export
and alias declarations; runtime and Phase-1 bindings; `fn`, `let`, `if`, `do`,
structured exception flow, and raising; `defstruct`; external Python bindings
and decorators; embedded-language text and linkable embedded Python module
declarations; macro definitions; and typed static schema or record declarations.
A declaration macro MAY provide the authored spelling of a compiler-owned
declaration, but MUST NOT synthesize an embedded foreign-source body.

**OEP-0001-R014:** Threading, ordinary conditional conveniences,
comprehensions, recursion conveniences, resource helpers, and data-sequence
composition SHOULD be standard or extension macros/functions. Adding one of
these forms MUST NOT add reader syntax or backend branches unless a separate
accepted OEP establishes a missing kernel semantic.

**OEP-0001-R015:** Phase 1 MUST receive immutable Syntax values, MUST be
deterministic for identical compiler, source, interface, and configuration
inputs, and MUST NOT import Python modules, perform network access, mutate the
project, or observe undeclared ambient state.

**OEP-0001-R016:** Macros MUST return Osiris syntax that resolves, expands,
type-checks, and lowers through the normal compiler pipeline. A macro MUST NOT
return Python source text, a Python AST, untyped HIR, or a backend callback.

**OEP-0001-R017:** Macro-created identifiers MUST be hygienic by default.
Intentional use of call-site names MUST require an explicit syntax operation,
and expansion output MUST retain both definition and call-site origin chains.

**OEP-0001-R017A:** Hygiene applies to every binding position a macro template
authors directly. Where a template's binding positions cannot be identified
before expansion, the compiler MUST leave the authored names unchanged rather
than rename them on an assumption, and the syntax manual MUST list those
shapes so a macro author knows when an explicit fresh name is required.

**OEP-0001-R018:** A Phase-1 package interface MUST declare macro code and
compile-time dependencies statically. Macro expansion MUST depend only on
locked, validated interfaces selected by OEP-0002 and MUST fail closed when
required macro code is absent or incompatible.

### Rich Metadata and types

**OEP-0001-R019:** Rich Metadata MUST use the Clojure-compatible prefix forms
`^{:key value}`, `^:flag`, `^TypeTag`, `^"tag"`, and `^[Tag ...]` on supported
syntax nodes. The reader MUST normalize these forms to immutable metadata maps
without executing their contents.

**OEP-0001-R019A:** Metadata MUST hold only inert, serializable data — no
reader forms, embedded blocks, error nodes, non-finite floats, odd maps, or
metadata attached to metadata. Inside a macro template, `^{...}` MAY contain
unquote and unquote-splicing, because a template's metadata is complete only
after substitution; syntax quoting MUST substitute inside metadata as it does
inside the datum. The rule above MUST be enforced on the result of expansion,
which every top-level form reaches whether or not a macro produced it. A macro
interface publishing a template MUST accept the template's unsubstituted form.
This fixes when the rule applies, and relaxes nothing about what metadata may
finally contain.

**OEP-0001-R020:** Phase 1 MUST provide readable `meta`, `with-meta`, and
`vary-meta` behavior over supported immutable syntax data. Updating metadata
MUST return a new value, MUST preserve lexical context unless explicitly
changed by a syntax API, and MUST NOT alter ordinary datum equality.

**OEP-0001-R021:** Source span, macro expansion origin, authored metadata,
declared contracts, static records, and compiler-verified facts MUST be stored
and exposed as distinct categories. Source or macro metadata MUST NOT be able
to claim compiler verification.

**OEP-0001-R022:** Metadata keys standardized by Osiris MUST include localized
documentation and names, API lifecycle data, and `:export`. Unknown namespaced
keys MUST be preserved as data but MUST NOT acquire compiler semantics without
an accepted contract.

**OEP-0001-R023:** Metadata imported from a package MUST be treated as
untrusted data. Renderers MUST sanitize active links or markup, and AI clients
MUST NOT interpret authored metadata as instructions, authority, or verified
semantic facts.

**OEP-0001-R024:** Runtime type annotations MUST be written as declaration or
binding Rich Metadata, including `^Int` and `^{:type (Vector Int)}`. Osiris MUST
NOT require a separate declaration file or a TypeScript-style type-only source
language for ordinary Osiris definitions.

**OEP-0001-R025:** Omitting a type annotation MUST request local inference, not
an implicit `Any`. Exported signatures and host boundaries MUST be complete;
unresolved `Unknown` MUST NOT enter a published interface. `Any` at a published
boundary MUST be explicit and MUST retain its provenance in semantic output.

**OEP-0001-R026:** Container names without type arguments MUST represent their
documented dynamic boundary, such as `Vector[Any]`, rather than an inferred
element type. DataFrame, Series, Array, schema, and domain types MUST be
provided through ordinary typed interfaces, not hard-coded reader grammar.

**OEP-0001-R027:** `defstruct` MUST define nominal identity, ordered typed
fields, defaults, metadata, constructor checking, and a public interface.
Fields MUST have explicit stable types; structure compatibility MUST NOT be
inferred only from matching field names.

**OEP-0001-R028:** Type checking MUST occur after macro expansion and before
Python generation. The generated Python annotations are an output contract and
MUST NOT be treated as the implementation of the Osiris type solver.

### Python target, diagnostics, and inspection

**OEP-0001-R029:** One compiler invocation MUST target exactly one Python
language version. Generated Python MUST parse on that target and SHOULD retain
recognizable modules, declarations, names, type annotations, decorators, and
control flow suitable for review and debugging.

**OEP-0001-R030:** The Python backend MUST lower validated semantic IR through
a structured representation. Macro expansion and principal code generation
MUST NOT be implemented by concatenating executable Python source fragments.

**OEP-0001-R030A:** Embedded `python` bodies are authored foreign source artifacts,
not macro output or semantic code-generation fragments, and are therefore the
only exception to OEP-0001-R030's authored-source prohibition. The compiler
MUST parse them for the configured Python target, reject syntax errors with
mapped Osiris spans, normalize them through the compiler-embedded,
version-pinned Ruff formatter, and hash the normalized module without importing
or executing it.
The embedded module MUST be represented as a validated linkable artifact before
ordinary Osiris HIR is emitted.

**OEP-0001-R030B:** Static Python imports among embedded `python` modules MUST be
resolved into a relocatable module graph. Imports outside that graph MUST name
ordinary declared Python dependencies. Dynamic import of a provider-owned
embedded module, mutation of `sys.path`, filesystem-relative module discovery,
or reliance on undeclared package data MUST be rejected in the initial format.
This restriction does not prohibit runtime I/O explicitly performed by a
declared function; it keeps linkage static and reproducible.

**OEP-0001-R031:** Generated Python MUST preserve Osiris evaluation order and
MUST NOT duplicate an expression with observable effects. Target-specific
rewrites MUST NOT silently weaken type, temporal, data, or effect diagnostics.

**OEP-0001-R032:** Diagnostics MUST have a stable code, severity, primary
source span, human message, and an ordered list of related locations. Each
related location MUST carry a stable machine-readable kind and a span, and MUST
name the module that span belongs to whenever that is not the module the
diagnostic was reported in. Identity MUST NOT depend on the human message text.
Generated Python failures MUST be mappable back to authored source.

**OEP-0001-R032A:** A diagnostic reported against syntax that macro expansion
produced MUST carry that expansion as related locations. Every expansion in the
chain MUST contribute both its call site and its macro definition, ordered
outermost expansion first, and MUST identify the macro by binding ID so a
consumer can join the diagnostic to the expansion trace without comparing
spans. This applies to diagnostics from every pass that runs after expansion,
not only to diagnostics the expander raises itself.

**OEP-0001-R032B:** A source span is a byte range without a file identity. A
diagnostic MUST NOT report a span belonging to one module against another
module's text. Where expansion copies syntax out of an imported macro's
template, the compiler MUST re-point that syntax at the call site and record
the defining module through R032's related locations instead.

**OEP-0001-R032C:** Every compiler-owned diagnostic surface — CLI rendering,
`osr lsc` JSON, and LSP publication — MUST expose the related locations a
diagnostic carries, and MUST NOT silently drop an entry. Machine-readable
surfaces MUST expose each entry's kind, span, module, and macro binding ID. A
surface that cannot resolve a foreign-module span to a line and column MUST
still name the module.

**OEP-0001-R033:** `osr lsc syntax` MUST expose the lossless syntax view,
and `osr lsc semantic` MUST expose canonical binding IDs, types,
metadata categories, contracts, origins, and source spans. JSON inspection
output MUST declare a schema identifier and MUST NOT depend on localized text
for identity.

**OEP-0001-R034:** `osr expand` MUST display macro-expanded Osiris syntax and
MUST provide a single-step mode. Expansion output and inspection output MUST
never import or execute the generated Python program.

### CLI command system

**OEP-0001-R035:** The executable name MUST be `osr`. Command names, option
names, and machine-readable field names MUST use stable ASCII spellings;
localized labels MAY be rendered as presentation text.

**OEP-0001-R036:** Every public command MUST have one command definition with:

- a stable command ID and canonical name;
- any aliases and lifecycle status;
- a summary and synopsis;
- positional arguments and options, including types, defaults, multiplicity,
  conflicts, and environment or configuration precedence;
- input and output contracts, stdout and stderr use, and supported formats;
- filesystem, process, network, and dependency side effects;
- exit codes and interruption behavior;
- related OEP requirements and diagnostics;
- at least one valid example when the command takes arguments.

**OEP-0001-R037:** Command definitions MUST be the source used to generate
concise help, shell completion metadata, and machine-readable command
introspection. These views MUST NOT maintain independent argument contracts.
Authored command manuals MAY cite that data but MUST remain complete documents.

**OEP-0001-R038:** The initial command registry MUST reserve definitions for
`init`, `check`, `build`, `compile`, `watch`, `run`, `fmt`, `expand`, `lsc`,
`lsp`, `syntax`, and `doc`. OEP-0002 defines project and package behavior for
the applicable commands; this proposal defines their common CLI and
documentation behavior.

| Command ID | Canonical command | Contract owner |
| --- | --- | --- |
| `cli/init` | `osr init` | OEP-0002 |
| `cli/check` | `osr check` | OEP-0002 |
| `cli/build` | `osr build` | OEP-0002 |
| `cli/compile` | `osr compile` | OEP-0002 |
| `cli/watch` | `osr watch` | OEP-0002 |
| `cli/run` | `osr run` | OEP-0002 |
| `cli/fmt` | `osr fmt` | OEP-0001 |
| `cli/expand` | `osr expand` | OEP-0001 |
| `cli/lsc` | `osr lsc` | OEP-0001 |
| `cli/lsp` | `osr lsp` | OEP-0001 |
| `cli/syntax` | `osr syntax` | OEP-0001 |
| `cli/doc` | `osr doc` | OEP-0001 |

**OEP-0001-R039:** `osr --help` and `osr <command> --help` MUST be concise,
offline usage help. They MUST show accepted operands and options, but MUST NOT
be the only source of syntax, diagnostic, or semantic documentation.

**OEP-0001-R040:** CLI misuse MUST exit `2`; source, semantic, or requested
build validation failure MUST exit `1`; success MUST exit `0`. A long-running
command interrupted by the platform interrupt signal MUST stop promptly,
release watched resources and child processes, and report the platform's
conventional interrupted status, `130` on POSIX. After a successful compile,
`osr run` MUST propagate the invoked program's exit status as declared by its
command definition; that status is not CLI misuse. No additional
compiler-owned process exit code has a stable cross-platform meaning in this
language version.

**OEP-0001-R041:** Text commands MUST write their requested result to stdout
and diagnostics or operational failures to stderr. A successful JSON command
MUST emit exactly one valid JSON value to stdout and MUST NOT mix progress text
into that stream.

**OEP-0001-R042:** Commands MUST NOT modify dependencies, lock files, project
configuration, source files, or published artifacts unless their command
definition explicitly declares that side effect. `check`, `expand`, `lsc`,
`syntax`, `doc`, and `lsp` MUST NOT mutate those inputs.

### Documentation data, query, and search

**OEP-0001-R043:** The complete initial CLI grammar for release documentation
MUST be:

```text
osr syntax
osr syntax --format markdown|json
osr agents
osr agents --format markdown|json
osr doc <graphql-document>
osr doc -
```

`osr agents` MUST read the stable document ID `tooling/agents` from the embedded
snapshot and MUST project it exactly as `osr syntax` projects `language/syntax`.
That document is operational guidance for the workflow OEP-0001-R054 through
OEP-0001-R056 require; it is not normative and MUST NOT restate a requirement as
though it were the source. Where the document and an accepted OEP disagree, the
OEP governs.

`osr syntax` MUST read the stable document ID `language/syntax` from the
embedded snapshot. Its default and `markdown` output MUST be exactly the
complete authored English Markdown body followed by one LF. Its `json` output
MUST be one versioned object containing at least `id`, `title`, `revision`,
`contentHash`, and `markdown`. It MUST NOT accept a locale or source path.
`osr lsc syntax <path>` remains the separate lossless source-tree query.

`osr doc <graphql-document>` MUST accept exactly one shell argument containing
one standard GraphQL query document. `-` MUST read that document from stdin for
long or generated queries. The document MAY contain fragments but MUST select
exactly one query operation. `command`, `diagnostic`, `oep`, `search`, and `ai`
MUST NOT be documentation subcommands. With no document, `osr doc` MUST print
concise usage.

**OEP-0001-R044:** Release tooling MUST export the selected complete English
documents and derived search projections into a SQLite-format database built
and queried with libSQL. The database MUST provide FTS5 indexes over document
title, heading, and Markdown body; the initial English index MUST use a Unicode-
aware tokenizer such as `unicode61`. The finalized database bytes MUST be
read-only, content-addressed, and embedded as a resource in every native `osr`
binary. An implementation MAY materialize those bytes into private memory or a
verified temporary file, but MUST NOT require a database server.

**OEP-0001-R045:** `osr doc` MUST execute the supplied GraphQL document without
rewriting its fields or selection sets. By default, the GraphQL schema and
resolvers MUST run in-process against the exact snapshot embedded in that
executable. `documentationCapabilities` MUST expose at least `source`,
`snapshotId`, `contentHash`, and `schemaVersion`; `source` MUST report
`embedded` for this path. Documentation and syntax lookup MUST work offline and
MUST NOT implicitly connect to Turso or any other remote service.

**OEP-0001-R046:** The embedded documentation snapshot MUST contain only
complete, authored English documents selected for publication. It MUST NOT
contain source or `.osri` Rich Metadata, binding signatures, localized aliases,
workspace symbols, generated per-command records, generated per-diagnostic
records, or package API records. A document MAY be indexed by title, heading,
and Markdown body, and internal heading chunks MAY support ranking, but every
chunk MUST retain its parent document and anchor.

**OEP-0001-R047:** The public GraphQL schema MUST provide these read-only root
operations with equivalent typed fields, pagination, and bounded result sizes:

```graphql
type Query {
  document(id: ID!): Document
  searchDocuments(input: DocumentSearchInput!): DocumentConnection!
  documentationCapabilities: DocumentationCapabilities!
  completeDocumentQuery(input: DocumentCompletionInput!): [DocumentCompletion!]!
}

input DocumentSearchInput {
  query: String!
  first: Int = 10
  after: String
  includeDiscussions: Boolean = false
}

input DocumentCompletionInput {
  prefix: String!
  limit: Int = 20
}

type DocumentationCapabilities {
  source: String!
  snapshotId: ID!
  contentHash: String!
  schemaVersion: String!
  schemaHash: String!
}
```

The schema MUST define typed `Document`, `DocumentChunk`, connection,
completion, provenance, and capability objects rather than opaque JSON blobs.
`Document` MUST expose the complete authored Markdown in addition to identity,
title, collection, normative state, revision, content hash, and provenance.
GraphQL schema introspection MUST remain available for this embedded read-only
surface.

**OEP-0001-R048:** `documentationCapabilities` MUST let a human tool or AI
discover the query source, snapshot ID and content hash, GraphQL schema version
and hash, compiler and language versions, collections, indexed document counts,
search and completion features, supported limits, and source provenance.
`DocumentCompletion` MUST contain only document query aids such as a document
ID, title, matching heading, and snapshot provenance; it MUST NOT act as
source-code completion.

**OEP-0001-R049:** Published source documents MUST remain complete English
Markdown documents. The libSQL snapshot MAY store derived heading chunks and
FTS5 rows, but those rows are search projections rather than authored
documents. Queries MUST return the authored document or an anchored excerpt
from it and MUST NOT synthesize normative prose, translations, or API records.
Search ordering and tie-breaking MUST be deterministic for an identical
snapshot and GraphQL input.

**OEP-0001-R050:** A completed `osr doc` invocation MUST write exactly one
standard GraphQL JSON response object to stdout, including `data` and any
GraphQL `errors`, without a CLI-specific envelope or text renderer. Invalid
embedded bytes, snapshot hash mismatch, or failure to initialize the local
query engine MUST be reported on stderr and MUST NOT produce a misleading
successful response. Progress output MUST NOT be mixed into stdout.

**OEP-0001-R051:** Each published document MUST originate from an explicitly
selected English Markdown source. The libSQL database, FTS5 index, and GraphQL
schema are derived publication and query views, not normative sources. Every
document and search chunk MUST retain stable source identity, revision, content
hash, collection, normative state, and publication provenance. Turso MAY be
used as a publication source or optional distribution system, but MUST NOT be a
runtime dependency of the embedded documentation path.

**OEP-0001-R052:** Snapshot publication MUST follow `oeps/oeps.jsonc` and
OEP-0000-R044 through OEP-0000-R047. Stable snapshots MUST contain only the
English reference collection. Preview snapshots MAY additionally contain
English Draft and Review documents in a distinct discussions collection with
`normative: false`. Repository translations MUST NOT be placed in the embedded
snapshot. OEP-0000 MUST never be published through it.

**OEP-0001-R053:** Embedded documentation queries MUST be read-only, bounded by
depth, complexity, timeout, pagination, and response-size limits, and safe for
schema introspection. `osr doc` and `osr syntax` MUST NOT access the network or
upload project source, semantic documents, dependency graphs, source metadata,
or credentials. Corrections to embedded content MUST create a new content hash
and be shipped in a new compiler release; installed snapshot bytes MUST never
be updated in place. Documentation failure MUST NOT affect `check`, `build`,
compilation, local inspection, LSP semantics, or generated Python.

### AI workflow

**OEP-0001-R054:** Before creating or changing `.osr`, an AI agent claiming
Osiris conformance MUST:

1. use `osr syntax` to load the complete version-pinned English syntax manual;
2. use `osr doc <graphql-document>` to inspect
   `documentationCapabilities`, or `document` and `searchDocuments` for
   unfamiliar or version-sensitive behavior;
3. treat the embedded English documents as release documentation and use the
   local tooling engine for workspace and dependency APIs;
4. use the local `osr lsc` operation matching the needed editor capability
   for bindings, signatures, completion, navigation, metadata, or contracts;
5. use `osr expand` or `osr lsc expand` when macro behavior affects the
   change;
6. run `osr fmt` and `osr check` on the affected project or source scope;
7. search the English diagnostic manual before changing code in response to an
   unfamiliar diagnostic;
8. preserve canonical binding IDs and treat localized names as presentation and
   resolvable source metadata, not independent declarations;
9. verify OEP status before implementing public behavior described by an OEP.

**OEP-0001-R055:** AI and LSP clients MUST identify edits by document version,
source span, and canonical binding ID where available. They MUST NOT perform a
project-wide semantic rename by replacing a localized alias string.

**OEP-0001-R056:** An AI client MUST distinguish authored metadata, static
records, dependency-declared facts, and compiler-verified facts. It MUST NOT
describe authored claims or discussion OEP text as compiler proof or accepted
language behavior.

### Documentation and localized name metadata

**OEP-0001-R057:** `:doc` MUST be either a non-empty default `Str`, a same-module
`~markdown` reference, or a locale map defined by OEP-0001-R058. An exported
declaration MUST provide `:doc`; a private declaration SHOULD provide it. A
declaration docstring, when supported as surface sugar, MUST populate the
default string form. Reusable packages SHOULD author the default in English for
AI and ecosystem interoperability, but the compiler MUST permit another
authored default language.

**OEP-0001-R058:** Localized documentation MUST use one compact `:doc` map from
the required Keyword `:default` and canonical BCP 47 locale strings to either
non-empty `Str` values or same-module `~markdown` references. `:default` is the
author's language-neutral fallback slot and MUST NOT be interpreted as a
language tag. Locale keys MUST be unique
after BCP 47 canonicalization. An author MAY include an `"en"`, `"zh-CN"`, or
other tagged entry even when its value equals `:default`. Splitting translations
into a second metadata key is not part of this contract.

**OEP-0001-R059:** A documentation consumer requesting a locale MUST select an
entry from the normalized `:doc` map using the RFC 4647 lookup fallback
sequence, then fall back to `:default` when no tagged entry matches. A plain
`Str` or resolved `~markdown` reference MUST normalize as
`{:default <value>}` without inferring its language.
When `:default` is selected, a consumer MUST identify it as the default fallback
and MUST NOT fabricate a resolved BCP 47 tag. Locale selection MUST NOT change
binding identity, overload selection, or semantics.

**OEP-0001-R060:** `:osiris/names` MUST be a map from canonical BCP 47 locale
strings to maps of this shape:

```clojure
{"zh-CN" {:preferred 格式化文本
          :aliases [渲染文本]}
 "ja"    {:preferred メッセージ整形
          :aliases [テキスト整形]}
 "fr"    {:preferred formater-message
          :aliases [rendre-message]}}
```

`:preferred` MUST be one Symbol. `:aliases`, when present, MUST be a Vector of
Symbols. Locale keys MUST be unique after BCP 47 canonicalization. Names MUST
be unique after NFC normalization within the declaration's complete name
table. Unknown keys in a locale entry MUST be diagnosed.

**OEP-0001-R060A:** `:preferred` MUST identify the currently recommended
localized source spelling. `:aliases` MUST be reserved for older spellings
retained for source compatibility and migration; it MUST NOT be interpreted as
an unordered set of equally recommended translations. Interfaces and semantic
indexes MUST preserve whether a spelling is canonical, preferred, or a
migration alias rather than exposing only a flattened spelling list.

**OEP-0001-R061:** The canonical declaration spelling is the default binding
label and identity. A localized preferred name and every localized alias MUST
resolve to that same canonical binding in every display locale. A reusable
public library SHOULD use a stable ASCII English canonical spelling; when it
does not, it SHOULD provide an `en` entry in `:osiris/names` for English
presentation.

**OEP-0001-R062:** `:osiris/names` MAY be attached to modules, declarations,
types, macros, parameters, and `defstruct` fields. Parameter and field aliases
MUST be scoped to their owning signature or structure and MUST lower to the
canonical Python keyword or attribute. They MUST NOT define a global keyword
translation table.

**OEP-0001-R062B:** A macro MAY document its clause words through
`:osiris/clauses`: a map from the clause symbol as authored to either a string
or a `{:default …, "<bcp47>" …}` documentation map, on the macro's declaration
metadata. Tooling hovering a clause word inside that macro's call MUST answer
with the clause's documentation, falling back to the macro's own documentation
for undocumented words. The key is presentation data and MUST NOT enter the
semantic interface projection — editing clause documentation changes no
dependent's compilation.

**OEP-0001-R062A:** A reference authored with a spelling from `:aliases` MUST
continue to resolve to the canonical identity, but compiler CLI, LSC, and LSP
diagnostics MUST report a non-failing migration advisory. The replacement MUST
be the requested display locale's `:preferred` spelling when available and the
canonical spelling otherwise. References authored with `:preferred` MUST NOT
receive this advisory. LSP SHOULD provide a code action for the replacement.

**OEP-0001-R062C:** An explicit `:refer` member names an export, not a
spelling. A member authored with the canonical, `:preferred`, or any
`:aliases` spelling MUST refer that export, and the export's canonical
spelling together with every `:osiris/names` spelling MUST become callable at
the referring site — the same visibility `:refer :all` grants the export.
Functions and macros MUST behave identically. Migration advisories
(OEP-0001-R062A) remain keyed to the spelling used at each reference, never to
the spelling used in the import form.

**OEP-0001-R063:** `.osri`, semantic queries, LSP, and local CLI queries MUST
preserve the default documentation, every tagged translation, the canonical
name, localized preferred names, aliases, and provenance. Embedded content
references MUST be serialized as their resolved content plus label, language,
source provenance, and content hash; a consumer MUST NOT need provider source
to render them. JSON documentation entries MUST expose at least this logical
shape:

```json
{
  "documentation": {
    "default": "Format a message for display.",
    "translations": {"zh-CN": "格式化用于显示的文本。"},
    "selection": {
      "requestedLocale": "zh-CN",
      "resolvedLocale": "zh-CN",
      "text": "格式化用于显示的文本。"
    }
  },
  "names": {
    "canonical": "format-message",
    "localized": {
      "zh-CN": {"preferred": "格式化文本", "aliases": ["渲染文本"]}
    }
  }
}
```

**OEP-0001-R064:** The canonical metadata spelling for a localized public API
MUST follow this form; type annotations remain Rich Metadata on names rather
than a separate signature syntax:

```clojure
(extern python "text_runtime.format"
  ^{:doc {:default "Format a message for display."
          "zh-CN" "格式化用于显示的文本。"}
    :osiris/names
    {"zh-CN" {:preferred 格式化文本
               :aliases [渲染文本]}}}
  (defn ^Str format-message
    [^Str template
     ^{:type Str
       :osiris/names {"zh-CN" {:preferred 名称
                                :aliases [显示名称]}}}
     name
     [^{:type Bool
        :osiris/names {"zh-CN" {:preferred 转为大写
                                 :aliases [大写 使用大写]}}}
      uppercase = false]]))
```

OEP translations remain governed by OEP-0000 and are not stored in API Rich
Metadata. This requirement defines documentation attached to language and
package bindings.

**OEP-0001-R065:** Locale metadata MUST be open to every well-formed BCP 47
tag. A compiler, `.osri` consumer, LSP, or documentation tool MUST NOT reject or
discard a locale merely because its own user interface is not translated into
that language. Adding only a `:doc` translation is a tooling-data change.
Adding a localized preferred name or alias changes the resolvable source-name
surface and MUST affect the semantic hash, but MUST NOT change the canonical
binding identity.

### Local tooling queries and CLI parity

**OEP-0001-R066:** The compiler MUST own one local tooling query engine over the
current workspace `.osr` sources, unsaved overlays supplied by a client, and
validated dependency `.osri` interfaces. CLI and LSP MUST call that engine
rather than independently reimplementing name resolution, metadata selection,
navigation, completion, signatures, diagnostics, or edits.

**OEP-0001-R067:** The local CLI query surface MUST use this command family:

```text
osr lsc <operation> ... [--locale <bcp47>] [--format text|json]
```

`lsc` means Language Server Console. It MUST be a normal finite CLI invocation,
not an interactive REPL and not a JSON-RPC pass-through. Its initial forms MUST
include:

```text
osr lsc diagnostics [<path>]
osr lsc hover <api-name-or-binding-id>
osr lsc hover --at <uri>:<line>:<column>
osr lsc completion --at <uri>:<line>:<column>
osr lsc signature --at <uri>:<line>:<column>
osr lsc definition --at <uri>:<line>:<column>
osr lsc references --at <uri>:<line>:<column>
osr lsc rename --at <uri>:<line>:<column> --to <name>
osr lsc expand <path>
osr lsc syntax <path>
osr lsc semantic <path>
osr lsc symbol <name-or-binding-id>
```

The initial operations MUST include `diagnostics`, `hover`, `completion`,
`signature`, `definition`, `references`, `rename`, `expand`, `syntax`,
`semantic`, and `symbol`. Position-based operations MUST accept a source URI or
path and line/column position. `hover` and `symbol` MUST accept a canonical
binding ID, canonical name, localized name, or alias; `symbol` MAY additionally
accept workspace search text. An ambiguous name MUST return candidate binding
IDs rather than select by filesystem or import order. `rename` MUST return
validated edits and MUST NOT apply them; applying edits requires a separate
explicitly mutating command contract.

**OEP-0001-R068:** Every compiler-owned LSP capability that returns diagnostics,
semantic facts, navigation, completion, signatures, documentation, or proposed
edits MUST have a CLI operation with equivalent observable information. Semantic
queries and rename previews belong under `osr lsc`; canonical formatting
belongs under `osr fmt`. A capability MUST NOT become LSP-only. LSP session
lifecycle, incremental synchronization, cancellation, and wire framing are
transport behavior and do not require artificial CLI equivalents.

**OEP-0001-R069:** `osr lsc` MUST default to concise human-readable text.
`--format json` MUST return a versioned, stable compiler tooling object without
LSP or JSON-RPC envelopes. Text and JSON MUST be projections of the same query
result, and diagnostics, canonical binding IDs, source locations, provenance,
and locale resolution MUST not disagree between them or with LSP.

**OEP-0001-R070:** Locale identifiers at every tooling boundary MUST be
well-formed IETF BCP 47 language tags, canonicalized according to their
registered subtags, and matched with RFC 4647 lookup as specified by
OEP-0001-R059. Implementations MUST NOT define a closed locale enum, private
locale spelling, or custom fallback chain. `zh-CN`, `ja`, and `en` are examples,
not an exhaustive supported set.

An `osr lsc` request with no `--locale` MUST select `:doc` `:default` and the
canonical display name, and MUST NOT inherit `displayLocale` from
`osiris.jsonc`. The recommended authored default is English, but LSC MUST
preserve a Chinese, Japanese, or other default chosen by the source author. An
explicit `--locale <bcp47>` MUST select authored documentation and names for
that language through RFC 4647 lookup, with `:default` and the canonical name
as fallbacks. An AI requiring English SHOULD pass `--locale en` and MUST inspect
whether the result matched `en` or fell back to the authored default. An LSP
session MUST first use the standard
`InitializeParams.locale` when present and well-formed, then the project's
`displayLocale`, then the configuration default defined by OEP-0002. Thus an
AI can request English LSC output while a human's IDE presents Chinese,
Japanese, or another authored locale from the same compiler facts.

Text output MUST emphasize the selected documentation and preferred label.
JSON output MUST identify the requested and resolved language tags, retain the
canonical binding ID and available-language list, and preserve the complete
authored `:doc` and `:osiris/names` maps unless the caller explicitly requests
a summary projection. When fallback selects `:default`, `resolvedLocale` MUST
be absent or null rather than a fabricated language tag.

**OEP-0001-R071:** Local `symbol`, `hover`, `completion`, and `signature`
results MUST expose, where applicable, canonical binding ID, declaration kind,
signature and type, documentation, localized names, source span, module,
visibility, package or workspace provenance, and whether each fact was authored,
interface-declared, or compiler-verified. Private dependency implementation data
MUST NOT be inferred beyond its `.osri` interface.

**OEP-0001-R072:** Local queries and LSP semantics MUST work without the
embedded documentation engine or any network connection. Their source facts
MUST come from current `.osr` source, unsaved overlays, and validated `.osri`
interfaces, including whatever BCP 47 localization entries those inputs
contain. The compiler MUST NOT upload source, overlays, interfaces, semantic
records, queries, or results. `osr lsc semantic --format json` MAY remain the
bulk fallback, but it MUST not be the only CLI route to an LSP-visible fact.

**OEP-0001-R072A:** Compiler-owned LSP and LSC operations MUST recognize an
embedded sigil position, expose its tag, label, host span, embedded position,
and mapped diagnostics, and navigate between an `extern python` binding and its
local embedded `python` provider. Foreign-language completion, hover, signature help,
definition, references, and rename MAY be delegated to an external language
service and are not compiler-owned capabilities requiring an in-process LSC
implementation. The stable LSC JSON projection MUST nevertheless expose enough
virtual-document text and bidirectional position mapping for a CLI or agent to
invoke an external tool explicitly.

### Canonical formatting

**OEP-0001-R073:** Osiris MUST define one canonical formatter and formatting
version for `.osr` source. Formatting MUST NOT depend on editor, locale,
operating system, user preference, or project style configuration. The same
source and formatting version MUST produce byte-for-byte identical UTF-8 output
apart from preserved literal contents. Structural line endings MUST use LF, and
the document MUST end in exactly one LF.

The canonical layout is based on the [Clojure Style
Guide](https://guide.clojure.style/) source-layout conventions. That guide is
design provenance, not a moving normative dependency: the rules below are the
complete Osiris contract, and this OEP controls wherever the languages differ.

- The preferred maximum line width MUST be 80 Unicode display columns. Width
  MUST follow the formatting version's fixed Unicode width table: wide and
  full-width characters occupy two columns, combining characters occupy zero,
  and ambiguous-width characters occupy one. An atomic string, symbol,
  keyword, number, or comment MAY exceed the limit; the formatter MUST NOT
  alter literal or identifier contents merely to enforce it. Indentation and
  hanging alignment MUST use the same display-column calculation.
- Indentation MUST use spaces, never tabs. Forms with body parameters MUST
  indent the body two spaces from the opening parenthesis. Closing delimiters
  MUST be gathered onto the final content line and MUST NOT occupy a line by
  themselves.
- A function or macro call that does not fit on one line MUST retain its first
  argument beside the callee when feasible and vertically align subsequent
  arguments with that first argument. Alignment is feasible only when every
  argument then fits the line width from the column that alignment places it in;
  a callee wide enough to make that impossible MUST keep no argument beside it.
  If no argument remains beside the callee, arguments MUST begin one space after
  the opening parenthesis.
- Every width decision MUST be evaluated from the display column the form
  actually starts at, not from the start of a line. A formatter that plans line
  breaks before it knows that column MUST re-evaluate them once it does.
- Binding pairs MUST align to the first binding after the opening bracket. Map
  keys MUST align one space after the opening brace. Sequential collections
  MUST retain as many complete elements per line as fit and align continuation
  elements one space after their opening delimiter.
- `defn`, `defmacro`, and `defn-for-syntax` MUST keep the name and parameter
  vector on one line when the declaration header fits; an implementation body
  MUST start on the following line at body indentation. Extern declarations
  without bodies MAY remain on one line.
- `if` branches and body forms MUST use body indentation. `cond`, `case`, and
  `condp`, `cond->`, and `cond->>` clauses MUST be laid out as aligned
  test/result pairs. Threading forms (`->`, `->>`, `some->`, and `some->>`) MUST
  place each continued step on its own line aligned with the initial threaded
  value. `as->` and `doto` bodies MUST use body indentation.
- Exactly one empty line MUST separate top-level forms. Consecutive top-level
  comment lines MUST remain a single comment block. Blank source lines inside a
  definition have no canonical significance and MUST be removed. Trailing
  whitespace MUST be removed.

Osiris extends that baseline for language-owned forms. A multiline Rich
Metadata map MUST align its keys one space after `{`; a multiline `:doc` value
MUST start on the following line and localized entries remain ordinary aligned
map pairs. `extern` MUST keep `python` and the module string on its first line,
then indent every kernel leaf by two spaces. `export` MUST indent its exported
collection by two spaces. `module`, `import`, `import-for-syntax`, `py/import`,
and `py/decorate` otherwise use ordinary call alignment. Metadata MUST remain
attached to the datum it describes.

**OEP-0001-R074:** The formatter MUST preserve reader meaning, comments, Rich
Metadata attachment, atom spelling, string contents, collection and top-level
form order, and all other semantic distinctions. It MUST be idempotent. If a
syntax error prevents a meaning-preserving result, it MUST report diagnostics
and MUST NOT partially rewrite that file.

**OEP-0001-R074A:** `osr fmt` MUST format the host sigil placement and
indentation. Generic embedded bodies other than `osiris` remain byte-preserved
after the structural tag normalization in OEP-0001-R006D. An `osiris` body MUST
be recursively formatted with the canonical Osiris formatter. An embedded
`python` body MUST use the same compiler-embedded, version-pinned Ruff formatter
and fixed profile as generated Python, with edits mapped back into the host `.osr` document.
Python formatting MUST run in-process and MUST NOT use an ambient `ruff`
executable, Python process, language server formatter, or project-specific Ruff
configuration. An invalid embedded body prevents the file from being partially
rewritten.

**OEP-0001-R075:** The formatter CLI grammar MUST be:

```text
osr fmt [<path>...] [--check]
osr fmt --all [--check]
osr fmt -
```

With paths, `osr fmt` MUST format `.osr` files in place. With no path, it MUST
format the current project's configured source scope using OEP-0002 source and
exclude rules. `--all` MUST explicitly select that same complete configured
project source scope and MUST NOT be combined with paths or `-`; this spelling
exists for Cargo-style command discoverability and scripts that should state
their scope. `--check` MUST perform no writes and MUST fail when any selected
file is not canonical. `-` MUST read one source from stdin and write only its
formatted source to stdout.

**OEP-0001-R076:** All formatter writes MUST preserve file permissions and use
safe replacement so an interruption does not leave a truncated source. A run
over multiple files MUST report every file it changed or would change in a
deterministic project-relative order. Diagnostics MUST identify the source and
span that prevented formatting.

**OEP-0001-R077:** LSP document formatting MUST use the same canonical formatter
as `osr fmt`. For an identical source snapshot and formatting version, applying
the LSP edits MUST produce exactly the bytes emitted by the CLI. LSP range
formatting MUST NOT be advertised until `osr fmt` exposes the same range
selection contract. No formatting behavior MAY exist only in an editor
extension.

### Language compatibility version

**OEP-0001-R078:** The Osiris language compatibility version MUST be versioned
independently from the compiler distribution version and MUST begin at `0.1`.
Compiler, interface, standard-library, and generated-artifact metadata MUST
record the language compatibility version where compatibility is evaluated.
A compiler package patch or minor release MUST NOT implicitly change language
compatibility merely because the package version changed.

### Withdrawn Language Server Agent experiment

The experimental network-backed Language Server Agent and `osr lsa` command
were withdrawn before language stabilization. They are not part of the Osiris
language or CLI contract. Local, deterministic language intelligence remains
available through LSC and LSP; applications may build separate AI experiences
on those public facts without adding a provider client to the compiler.

## Rationale

The fixed reader keeps parsing, formatting, recovery, and LSP behavior
predictable. A small kernel prevents macros from having to fake binding,
nominal type, phase, or exception semantics, while leaving ordinary control
flow and domain vocabulary outside Rust.

The current composable `nom` reader is consistent with the grammar isolation
required here, but the parser library is not a public compatibility contract.

Type annotations use Rich Metadata because Python already supports readable
annotations and Osiris does not need a separate type-only language. Local
inference keeps private code concise; explicit public signatures keep `.osri`
interfaces deterministic.

The documentation split keeps two unlike datasets honest. The compiler embeds
complete English manuals: one authored language is easier to review, version,
search, and keep authoritative, while an AI can explain that material in the
user's language. Code metadata stays local because signatures, aliases, and
workspace provenance change with the project and may be private. Localized code
names remain first-class because they also participate in source lookup.

An embedded libSQL snapshot gives release documentation one immutable identity,
FTS5 search, and offline behavior without making a database server part of the
compiler. Raw GraphQL makes `osr doc` a small, inspectable query interface
instead of a second search language, while `osr syntax` removes the query step
for the most common AI bootstrap. CLI-first local tooling lets humans and agents
use every compiler fact without implementing an editor protocol; LSP remains a
projection for IDE ergonomics.

A single formatter removes style negotiation from packages, generated changes,
editors, and AI-authored code. Keeping it in the core toolchain ensures that
formatting cannot reinterpret extension syntax because extensions do not own
reader syntax.

## Backwards Compatibility

This is the first formal language and CLI specification. Existing implemented
behavior is evidence for the draft but does not override it. Moving this OEP to
Accepted may require breaking pre-release syntax, inspection schemas, or CLI
behavior; no legacy form should be retained solely because it appeared before
the first accepted language version.

The public GraphQL schema, embedded database schema, syntax JSON schema, and
local tooling JSON schema are versioned independently from prose revisions.
Compatible fields may be added according to their schema rules; consumers
requiring exact semantics must reject unsupported major schema versions.

## Security and Determinism

The reader, published Markdown, embedded libSQL bytes, FTS5 index, OEP manifest,
package metadata, and Rich Metadata are untrusted data. Loading, indexing, or
rendering them must not execute Python, macro code, shell commands, or active
content. The embedded snapshot hash must be verified before queries are served.

Phase-1 execution requires deterministic resource limits and dependency
closure. GraphQL must apply input, execution, and output limits. Documentation
queries must not enable writable SQL, extension loading, attachment of external
databases, or network access. Local tooling must contain paths within the
selected workspace and validated package roots. JSON output must escape
untrusted text and keep it separate from identity and provenance.

## Tooling and AI Usage

The required workflow is specified in OEP-0001-R053 through R056. An agent uses
`osr syntax` for immediate language bootstrap, GraphQL for complete embedded
English manuals, and `osr lsc --format json` with an explicit locale when needed
for workspace and dependency APIs. LSP uses the standard initialization locale
or project display locale for human presentation. LSP hover, completion,
signature help,
navigation, rename, diagnostics, expansion, and formatting consume the same
compiler-owned results as their CLI equivalents. Every machine consumer must
retain stable IDs, origins, revisions, and information categories.

## Rejected Alternatives

### Treat every Clojure form as parser syntax

Most Clojure-inspired forms are ordinary lists whose behavior can be provided
by macros or functions. Parser ownership would enlarge the kernel and make
extension, inspection, and testing boundaries less clear.

### Put the complete core in macros

Macros cannot independently establish nominal type identity, lexical frames,
exception regions, module phases, verified facts, source mapping, or backend
ABI. Those boundaries need a small compiler-owned kernel.

### Keep types in separate files

Separate type declarations can drift from implementation and make generated
Python less direct. Metadata annotations plus inference retain one definition
while allowing `.osri` generation.

### Use `--help` as the language manual

Help text is optimized for command recall, not exact syntax, provenance,
diagnostics, localization, or machine consumption.

### Include drafts in the normal documentation search

Mixing proposed and accepted behavior makes both humans and agents generate
invalid programs. Preview discussions remain useful only when visibly and
structurally separated.

### Publish code metadata in the embedded documentation snapshot

Workspace and package APIs are versioned by source, lock state, and `.osri`
interfaces rather than the compiler documentation snapshot. Embedding them
would leak private code, become stale, and duplicate the compiler query engine.

### Publish central translations of every complete document

Maintaining and indexing parallel manuals creates revision drift and a second
authority problem. The embedded corpus remains authored English, while AI may
translate explanations for a user. Repository translations can still support
review without becoming binary content. This does not restrict localized
Rich Metadata in source and `.osri`, which is served by LSC and LSP.

### Make semantic tooling available only through LSP

LSP is appropriate for IDE sessions but unavailable to many agents and awkward
for direct human use. The complete compiler query surface therefore has a
stable CLI; the LSP server adapts it to editor protocol shapes.

### Allow project-specific formatter styles

Style configuration would make package source, editor edits, examples, and
AI-generated changes disagree. A language-wide canonical formatter makes the
output predictable and removes style state from builds and reviews.

### Let AI infer syntax from examples

Examples become stale and cannot enumerate failure behavior. Agents should load
the release-pinned English syntax manual, query other embedded manuals, inspect
local compiler facts, then format and validate with the compiler.

## Open Questions

None.

## Conformance

A conforming implementation provides evidence that:

- reader fixtures cover every fixed form, Unicode identity, metadata prefix,
  preservation rule, and recovery boundary;
- embedded-sigil fixtures cover exact closing-label matching, closing-tag text
  inside raw content under a different label, indentation normalization, unknown
  generic language tags, malformed labels, recovery after an unterminated body,
  and exact bidirectional source mapping;
- embedded Python fixtures prove target parsing, in-process canonical
  formatting, static import closure, no compile-time execution, and resolution
  from a symbolic `extern python` handle to its same-module provider while a
  string module name remains external;
- embedded-content fixtures prove ordinary `Str` export/import behavior,
  same-module metadata-reference resolution, recursive `~osiris` formatting,
  and absence of runtime reachability for documentation-only references;
- publication fixtures prove the manifest and the per-item `^:export` marker
  produce one union surface, that a name published both ways is published once,
  that only `true` publishes, that a marker is accepted wherever the manifest
  publishes the same declaration including embedded data blocks and `extern`
  nested declarations, and that a marker with no public name is reported;
- macro publication fixtures prove that a declaration macro generating a marker
  converges between the interface built from the unexpanded header and the one
  built from the expanded surface, including where the consumer resolves the
  generated name from a provisional interface inside the same runtime SCC;
- kernel inventory and standard macro inventory are independently queryable;
- macro tests prove hygiene, phase isolation, determinism, and origin chains,
  including that a diagnostic from a pass after expansion carries the ordered
  call-site and definition chain of every macro that produced the syntax, and
  that no diagnostic reports another module's span against local text;
- type and `defstruct` tests prove inference and public-boundary rules;
- Python output parses on its selected target and maps diagnostics to source;
- every public CLI command has one complete command definition;
- `osr syntax` returns the complete embedded `language/syntax` English manual
  offline in Markdown and its versioned JSON projection;
- `osr doc` executes standard GraphQL documents in-process and returns standard
  GraphQL JSON without network access;
- embedded libSQL snapshot tests verify its content hash, read-only behavior,
  FTS5 index, selected complete English documents, and document/chunk provenance;
- public metadata fixtures enforce a non-empty `:doc` default, standard language
  tags, translation fallback, localized-name identity, and complete JSON
  preservation;
- stable and preview publication tests enforce OEP status, English-only content,
  and discussion separation;
- every compiler-owned LSP semantic or edit result has an equivalent CLI query
  and parity fixture;
- formatter fixtures prove preservation, deterministic output, idempotence,
  `--check`, safe writes, and CLI/LSP byte equality;
- LSC locale fixtures select the authored `:default` slot without fabricating
  a language tag, LSP fixtures honor standard `InitializeParams.locale` then
  `displayLocale`, and both use BCP 47 plus RFC 4647 without changing semantic
  identity;
- AI workflow fixtures can load syntax, query complete manuals, inspect local
  semantics, resolve a diagnostic, format, and validate a source file; all
  operations continue to work without network access.

## Change History

- Revision 33, 2026-08-01: Clarified this proposal's standing since
  OEP-0005: it specifies the form level — what macros receive, what `.osr`
  transitional inputs contain, what `osr expand` prints — while source is
  authored on the OEP-0005 surface (`.ois`). Examples stay in form
  notation deliberately.

- Revision 32, 2026-07-31: Added OEP-0001-R062C: an explicit `:refer` member
  names an export, not a spelling — referring any spelling makes the
  canonical name and every `:osiris/names` spelling callable, for functions
  and macros alike. The per-spelling reading left `:refer [twice]` unable to
  call `加倍` while `:refer :all` could, an asymmetry with no user-visible
  rationale.

- Revision 31, 2026-07-31: Added OEP-0001-R062B, `:osiris/clauses`: a
  macro documents its clause words; hover inside a call answers with the
  clause, and the key never enters the semantic projection.

- Revision 30, 2026-07-30: Required one shared entry point for deciding whether a
  declaration is published, and a test holding components that must agree
  (OEP-0001-R009C). Five paths make that decision independently; four of them
  ignored `^:export` when it was introduced, and each was found by noticing a
  missing name downstream rather than by a failing test — a standard module
  written with the marker compiled and was invisible to every consumer. Also recorded that the standard library cannot use `py/embed`: while the
  compiler builds its own standard library there is no caller to resolve a
  reference (OEP-0001-R006CE).
- Revision 29, 2026-07-30: Added `py/embed`, which names a file as an embedded
  Python provider's body instead of carrying it inline (OEP-0001-R006CA through
  OEP-0001-R006CD). The inline form makes a duplicate the natural way to work —
  draft a `.py`, move it in, leave the original to drift — which had already
  produced two stale copies in practice. Fixed that the compiler core stays a
  function of source text: the caller resolves references and passes the content
  in, as it already does for external interfaces.
- Revision 28, 2026-07-28: Defined the name mappings the compiler already
  performs and never wrote down: Osiris identifier to Python identifier, module
  path to import path and to alias, and distribution name to its PEP 503
  normalization and PEP 427/625 escaping (OEP-0001-R005A through
  OEP-0001-R005C). Required one authoritative implementation per mapping, held
  to agreement by test (OEP-0001-R005D). Two independently written copies of the
  distribution-name rules had already shipped a source distribution PyPI
  rejected, and disagreed with each other on names one side validated and the
  other did not.
- Revision 27, 2026-07-28: Defined when aligning a call's arguments with its
  first argument is feasible, and required every width decision to be evaluated
  from the display column the form actually starts at (OEP-0001-R073). A wide
  callee — which in CJK source is any four-character name — previously forced
  alignment that overflowed the line, and a formatter could plan breaks before
  it knew its own indentation.
- Revision 26, 2026-07-28: Fixed when the inert-metadata rule is applied
  (OEP-0001-R019A). A macro template's `^{...}` may contain unquote, syntax
  quoting substitutes inside metadata, and the rule is enforced on the expansion
  result rather than at read time; a published macro template is accepted
  unsubstituted. Attaching computed documentation to a generated declaration was
  previously impossible without hand-building the map through `with-meta`.
- Revision 25, 2026-07-27: Added the conformance evidence criteria omitted when
  OEP-0001-R009A and OEP-0001-R009B were introduced, and corrected RFC 2119
  keywords in OEP-0001-R031 and OEP-0001-R032C that were written in lowercase
  and therefore carried no obligation under OEP-0000-R017.
- Revision 24, 2026-07-27: Required the `^:export` marker to be accepted
  wherever the manifest can publish the same declaration, including embedded
  data blocks and declarations an `extern` block nests, and required a marker
  that publishes nothing to be reported rather than ignored; aligned
  OEP-0001-R006B with OEP-0001-R009A accordingly.
- Revision 23, 2026-07-27: Added the per-item `^:export` marker as the second
  explicit way to publish a declaration (OEP-0001-R009A), allowed a declaration
  macro to generate one while leaving the authored boundary intact
  (OEP-0001-R009B), and standardized the `:export` metadata key under
  OEP-0001-R022.
- Revision 22, 2026-07-27: Required macro-produced syntax to carry its ordered
  expansion chain on every diagnostic raised after expansion, prohibited
  reporting one module's span against another module's text, and required every
  diagnostic surface to expose related locations (OEP-0001-R032A through
  OEP-0001-R032C); extended hygiene to template binding positions that cannot
  be identified before expansion (OEP-0001-R017A).
- Revision 21, 2026-07-27: Added `osr agents`, which prints the embedded
  `tooling/agents` manual: operational guidance for the AI workflow required by
  OEP-0001-R054 through OEP-0001-R056, projected exactly as `osr syntax`
  projects `language/syntax`.
- Revision 20, 2026-07-27: Dropped namespaced Agent information from the
  metadata keys OEP-0001-R022 requires Osiris to standardize; those keys carry
  no compiler semantics and are covered by the unknown-namespaced-key rule.
- Revision 19, 2026-07-27: Withdrew the experimental network-backed Language
  Server Agent and `osr lsa` before language stabilization; retained local LSC
  and LSP capabilities as the public tooling boundary.
- Revision 15, 2026-07-25: Defined generic embedded blocks as ordinary
  exportable `Str` bindings, made embedded Python labels private provider
  handles used by symbolic `extern python`, and added static Rich Metadata
  references to same-module `~markdown` and `~osiris` content.
- Revision 14, 2026-07-25: Added named raw embedded-language blocks, defined
  `~python<module>` for linkable Python modules, and specified target
  validation, formatting, source mapping, and LSP/LSC foreign-tool boundaries.
- Revision 13, 2026-07-25: Defined line width and hanging indentation in
  deterministic Unicode display columns so wide localized names align with
  ASCII names.
- Revision 12, 2026-07-24: Reserved `:aliases` for source migration,
  required spelling roles to survive interfaces and semantic indexes, and
  specified non-failing replacement advisories across compiler CLI, LSC, and
  LSP.
- Revision 11, 2026-07-24: Defined localized parameter keyword spellings,
  signature-local resolution, canonical Python lowering, and duplicate
  argument behavior.
- Revision 10, 2026-07-24: Replaced domain-specific localized-name examples
  with neutral text-formatting examples.
- Revision 9, 2026-07-24: Defined the compiler kernel, source-distributed
  self-hosted standard library, and linked `__osiris_runtime__` boundary.
- Revision 8, 2026-07-24: Defined canonical formatting, LSP/LSC parity, and
  source navigation requirements for packaged standard-library sources.
- Revision 7, 2026-07-23: Corrected conformance wording so locale-free LSC
  queries select the authored `:default` slot, which is recommended to be
  English but may be authored in any language.
- Revision 6, 2026-07-23: Started the independent language compatibility
  version at `0.1` and limited stable compiler-owned exit statuses to `0`, `1`,
  `2`, and POSIX interruption status `130`.
- Revision 5, 2026-07-23: Embedded a read-only libSQL/FTS5 English document
  snapshot in `osr`, added offline `osr syntax`, and separated LSC default-slot
  selection from project-configured LSP presentation using standard language
  tags.
- Revision 4, 2026-07-23: Replaced the generic local tooling command with the
  second-level `osr lsc` Language Server Console command family.
- Revision 3, 2026-07-23: Introduced a local tooling query command family.
- Revision 2, 2026-07-23: Split complete English GraphQL documents from local
  code metadata, made CLI tooling fully equivalent to compiler-owned LSP
  capabilities, and specified canonical formatting.
- Revision 1, 2026-07-23: Initial draft.
