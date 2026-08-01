---
oep: 5
title: Primary Surface Syntax
description: The Elixir-flavoured surface syntax (.osrx) as the primary way to write Osiris, its grammar, its mapping onto forms, and coexistence with the S-expression surface.
author: MJ
status: Draft
type: Standards Track
areas:
  - Language
  - Compiler
  - CLI
  - Tooling
created: 2026-08-01
updated: 2026-08-01
revision: 1
requires: [0, 1]
replaces: []
superseded-by: null
resolution: null
translations:
  zh: local/zh/0005-primary-surface-syntax.md
---
# OEP-0005: Primary Surface Syntax

## Abstract

Osiris adopts an Elixir-flavoured surface syntax as the primary way to write
Osiris source, carried by the `.osrx` extension. The macro system is
unchanged: surface text translates to the same form data structure macros
already operate on, so `defselect 名字 do … end` reaches the very same
named-body macros the S-expression surface calls. The S-expression surface
(`.osr`) remains fully supported as the canonical data notation — it is what
macros see, what `osr expand` prints, and what existing sources continue to
use. One file has one syntax; the extension decides; the interface layer
(`.osri`) is shared and identical for both.

## Motivation

S-expressions are the reason Lisp macros work and the reason most programmers
never look twice. Elixir demonstrated the resolution: keep the homoiconic
*data* representation for macros, give humans a surface that reads like the
mainstream. Osiris macros receive forms, not text, so the surface is
exchangeable without touching the macro system, the interface format, the
alias machinery (OEP-0001-R060…R062C), or the documentation pipeline.

The prototype (branch `explore/elixir-surface`, now merged) validated this
end to end: a `.osrx` strategy using the unmodified qlab `defselect` macro —
called through its `:osiris/names` spelling, with kebab-case factor names —
translates, expands, type-checks, and emits the same Python as its
S-expression twin.

## Specification

### R001 — Two surfaces, one form language

Osiris has exactly two surface syntaxes. `.osrx` carries the primary
surface defined by this OEP; `.osr` carries the S-expression surface defined
by OEP-0001. Both read to the same form data structure. A file MUST be
written in the syntax its extension names; implementations MUST NOT sniff
content. Project source discovery MUST accept both extensions, and a module's
name derives from its path identically for both.

### R002 — Primary means default

New user-facing material defaults to the primary surface: `osr init`
templates, documentation examples, and tooling snippets SHOULD present
`.osrx`. The S-expression surface remains fully supported indefinitely — it
is the macro data notation and the expansion/debug format (`osr expand`
output stays S-expression) — and existing `.osr` sources MUST keep compiling
with no migration requirement.

### R003 — Everything is a call

The primary surface has exactly four special forms: `def`, `defmacro`,
`@doc`, and `if`. Every other construct is a call. `module`, `import`,
`import-for-syntax`, and `export` are ordinary paren-less calls that land on
the corresponding OEP-0001 core forms:

| Surface | Form |
| --- | --- |
| `module app.策略` | `(module app.策略)` |
| `import lib.marks, refer: [加倍]` | `(import lib.marks :refer [加倍])` |
| `import-for-syntax m.select, refer: :all` | `(import-for-syntax m.select :refer :all)` |
| `export [f, g]` | `(export [f g])` |
| `f(a, b)` / `m.f(a)` | `(f a b)` / `(m.f a)` |
| `key: value` (in arguments) | `:key value` |
| `:word` | `:word` |
| `[a, b]` | `[a b]` |

A statement-position identifier followed by an argument (with no operator or
call parenthesis between) begins a paren-less call whose arguments extend to
the end of the line or to a `do` block. Expression keywords (`if`, `not`,
`do`, `else`, `end`) never begin a paren-less call.

### R004 — Identifiers: infix yields to kebab-case

Identifiers consist of letters (any script), digits (non-initial), `_`, `-`,
`?`, and `!`. A `-` is part of the identifier when a name character follows
it without intervening whitespace; subtraction MUST therefore be written
with surrounding whitespace. `pct-rank` is one name; `a - b` subtracts;
`x-1` is one name exactly as in the S-expression surface. This choice keeps
every existing kebab-case export callable from the primary surface with no
alias, no rename, and no qualified detour. Underscore names (`import_for_syntax`,
`defn_for_syntax`) are accepted spellings of the corresponding hyphenated
core forms.

### R005 — Definitions

`def name(param :: Type, …) :: Ret do body end` translates to
`(defn ^Ret name [^Type param …] body…)`; `defmacro` likewise. A preceding
`@doc "…"` attaches `^{:doc "…"}` metadata to the definition. Multiple body
statements become the definition body in order. Richer metadata
(`:osiris/names`, `:osiris/clauses`, `:export` markers) on the primary
surface is deferred to a revision of this OEP; until then declarations
needing it are authored in `.osr`.

### R006 — Operators

Binary operators `+ - * / == != < <= > >=` translate to prefix calls on the
names `+ - * / = not= < <= > >=`; `and`, `or`, and unary `not` translate to
their form names; unary minus translates to `(- 0 x)`. Precedence from
loosest to tightest: `|>`; comparisons; `+ -`; `* /`; unary. Operators are
names like any other: when a module imports `<=` as a macro (pandas-style),
the infix spelling reaches that macro.

### R007 — The pipe

`x |> f(a, b)` translates to `(f x a b)`; `x |> f` to `(f x)`. The piped
value inserts as the FIRST argument (Elixir semantics), matching
data-frame-style signatures where the data comes first. `|>` binds loosest,
so `x |> f() |> g()` chains left to right.

### R008 — do-blocks are named-body macro calls

`head args do statements end` translates to `(head args (statement)…)`: each
inner statement becomes one clause form. This is exactly the named-body macro
shape (OEP-0001 named-body conventions, `:osiris/clauses` hover), so
declarative DSLs — `defselect`, query builders, config blocks — work on the
primary surface with zero macro changes:

```elixir
defselect 小市值 do
  slot short-mom, weight: rank-threshold
  with is-top?, if-else(rank(short-mom) <= rank-threshold, 1, 0)
  where pct-rank(long-mom) > pct-floor
  select rank(market-cap)
end
```

### R009 — `if`

`if condition do consequent else alternative end` translates to
`(if condition consequent alternative)`; the `else` branch is optional.

### R010 — Comments

`#` opens a comment that runs to the end of the line, translating to no
form. (S-expression `;;` comments are unchanged in `.osr`.)

### R011 — Diagnostics and provenance

Translation failures MUST report the source line. Compilation diagnostics
against a `.osrx` unit MUST name the `.osrx` path. Until the reader
integrates natively (see Roadmap), spans inside translated units refer to
the translated text; implementations SHOULD carry a line map so user-facing
positions land in `.osrx` coordinates.

### R012 — What the primary surface does not yet define

`quote`/`unquote` (the Elixir `quote do … end` ↔ syntax-quote mapping),
destructuring parameters, `defstruct`, embedded providers, and general
metadata attributes are not yet part of the primary surface. Macros
requiring them are authored in `.osr`; consuming those macros from `.osrx`
is fully supported. Each lands as a revision to this OEP before

implementation, per OEP-0000.

## Roadmap

1. **Translate-at-load (this revision):** `.osrx` sources translate to
   canonical text at workspace load and enter the unchanged pipeline.
   `osr sketch FILE` exposes the translation for inspection.
2. **Native reader:** the translator becomes a first-class reader producing
   forms with real `.osrx` spans; diagnostics, LSP hover/definition/rename,
   and sourcemaps gain full fidelity.
3. **Formatter and templates:** `osr fmt` formats `.osrx`; `osr init`
   emits `.osrx` templates; documentation examples flip per R002.
4. **Macro authoring:** quote/unquote mapping so `defmacro` on the primary
   surface reaches full parity.

## Backwards Compatibility

`.osr` sources, interfaces, caches, and every existing project compile
unchanged. The new extension is additive; no flag days. Mixed projects are
supported at file granularity.

## Change History

- Revision 1, 2026-08-01: Initial version: the Elixir-flavoured surface as
  primary (`.osrx`), everything-is-a-call translation, infix-yields-to-
  kebab-case identifiers, first-argument pipe, do-blocks as named-body macro
  calls, coexistence and roadmap.
