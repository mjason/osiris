---
oep: 5
title: Primary Surface Syntax
description: The Elixir-flavoured surface syntax (.ois) as the one authoring surface of Osiris, its grammar, its mapping onto forms, and the S-expression notation's retreat to internal representation.
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
revision: 4
requires: [0, 1]
replaces: []
superseded-by: null
resolution: null
translations:
  zh: local/zh/0005-primary-surface-syntax.md
---
# OEP-0005: Primary Surface Syntax

## Abstract

Osiris is written in the Elixir-flavoured surface syntax, carried by the
`.ois` extension. This is a full migration, not a coexistence: `.ois` is the
one authoring surface, and every editor, formatter, LSP feature, template,
and document targets it. The S-expression notation retreats to what it
always was underneath — the form data structure macros operate on. It
remains visible as the expansion/debug format (`osr expand` prints it) and
`.osr` files remain compilable as transitional inputs while existing code
is rewritten by hand, but no new Osiris source is authored in it. The
interface layer (`.osri`) is unchanged.

## Motivation

S-expressions are the reason Lisp macros work and the reason most programmers
never look twice. Elixir demonstrated the resolution: keep the homoiconic
*data* representation for macros, give humans a surface that reads like the
mainstream. Osiris macros receive forms, not text, so the surface is
exchangeable without touching the macro system, the interface format, the
alias machinery (OEP-0001-R060…R062C), or the documentation pipeline.

The prototype (branch `explore/elixir-surface`, now merged) validated this
end to end: a `.ois` strategy using the unmodified qlab `defselect` macro —
called through its `:osiris/names` spelling, with kebab-case factor names —
translates, expands, type-checks, and emits the same Python as its
S-expression twin.

## Specification

### R001 — One authoring surface

`.ois` carries the authoring surface of Osiris, defined by this OEP. The
S-expression notation of OEP-0001 is the language's internal form
representation: it is what macros receive, what `osr expand` prints, and
what `.osr` transitional inputs contain. Both notations read to the same
form data structure. A file MUST be written in the syntax its extension
names; implementations MUST NOT sniff content. Project source discovery
MUST accept both extensions during the migration window, and a module's
name derives from its path identically for both.

### R002 — Full migration

All user-facing material presents `.ois`: `osr init` templates,
documentation examples, tooling snippets, and diagnostics. Tooling —
formatter, LSP, editor extensions, sourcemaps — MUST treat `.ois` as its
target surface. Existing `.osr` sources keep compiling as transitional
inputs so projects can be rewritten by hand at their own pace; they receive
no new surface features. Macro authoring stays in `.osr` only until R012's
quote mapping lands, at which point new macros are authored in `.ois` too.

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
`/`, `?`, and `!`. A `-` or `/` is part of the identifier when a name
character follows it without intervening whitespace; subtraction and
division MUST therefore be written with surrounding whitespace. `pct-rank`
and `py/import` are single names; `a - b` subtracts and `a / b` divides;
`x-1` is one name exactly as in the S-expression surface. This choice keeps
every existing kebab-case export and every slash-qualified name callable
from the primary surface with no alias, no rename, and no detour.
Underscore names (`import_for_syntax`, `defn_for_syntax`) are accepted
spellings of the corresponding hyphenated core forms.

Backticks spell a name verbatim when the identifier grammar cannot carry
it: ``refer: [`>`, `<=`, if-else]`` refers operator-named macros, and
`` `+`(a, b, c) `` calls one outside its binary infix shape. A backticked
name MUST NOT contain a backtick or newline.

### R005 — Definitions

`def name(param :: Type, …) :: Ret do body end` translates to
`(defn ^Ret name [^Type param …] body…)`; `defmacro` likewise. A preceding
`@doc "…"` attaches `^{:doc "…"}` metadata; the keyword form
`@doc default: "…", zh-CN: "…"` attaches the localized documentation map
`^{:doc {:default "…" "zh-CN" "…"}}`. Multiple body statements become the
definition body in order. Richer metadata
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

### R008A — Postfix member chains

A postfix expression continues with `.name` and `.name(args…)`. While the
base is still a plain name path, dots extend the statically resolved
qualified name exactly as before (`df.iloc.values`, `m.f(x)`). Once the
base is an evaluated expression — a call result or a parenthesized
expression — `.name(args…)` translates to the member call
`(.name base args…)` and a bare `.name` to the member access
`(.-name base)` (OEP-0001-R079):

```elixir
df.rolling(5).mean().pct-change()   # (.pct-change (.mean (df.rolling 5)))
df.rolling(5).values                # (.-values (df.rolling 5))
(a + b).hex()                       # (.hex (+ a b))
```

On `Any`-typed subjects this is wrapper-free Python: chains reach any
attribute or method with no `extern` declaration, typed as `Any` at the
dynamic boundary. Typed subjects keep their static field checks.

### R009 — `if`

`if condition do consequent else alternative end` translates to
`(if condition consequent alternative)`; the `else` branch is optional.

### R010 — Comments

`#` opens a comment that runs to the end of the line, translating to no
form. (S-expression `;;` comments are unchanged in `.osr`.)

### R011 — Diagnostics and provenance

Translation failures MUST report the source line. Compilation diagnostics
against a `.ois` unit MUST name the `.ois` path. Until the reader
integrates natively (see Roadmap), spans inside translated units refer to
the translated text; implementations SHOULD carry a line map so user-facing
positions land in `.ois` coordinates.

### R012 — What the primary surface does not yet define

`quote`/`unquote` (the Elixir `quote do … end` ↔ syntax-quote mapping),
destructuring parameters, `defstruct`, embedded providers, and general
metadata attributes are not yet part of the primary surface. Macros
requiring them are authored in `.osr`; consuming those macros from `.ois`
is fully supported. Each lands as a revision to this OEP before

implementation, per OEP-0000.

## Roadmap

1. **Translate-at-load (done):** `.ois` sources translate to canonical
   text at workspace load and enter the unchanged pipeline. `osr sketch
   FILE` exposes the translation for inspection. `osr init` emits `.ois`.
2. **Editor migration (this revision):** the VS Code extension and LSP
   recognise `.ois`; diagnostics carry `.ois` positions at line fidelity
   until the native reader lands.
3. **Native reader:** the translator becomes a first-class reader producing
   forms with real `.ois` spans; diagnostics, LSP hover/definition/rename,
   and sourcemaps gain full fidelity. `osr fmt` formats `.ois`.
4. **Macro authoring:** quote/unquote mapping so `defmacro` on the
   authoring surface reaches full parity; `.osr` transitional inputs can
   then be retired project by project.

## Backwards Compatibility

`.osr` sources, interfaces, caches, and every existing project compile
unchanged during the migration window. Mixed projects are supported at file
granularity; rewriting is by hand and per file. Interfaces and caches are
surface-independent.

## Change History

- Revision 4, 2026-08-01: Added R008A, postfix member chains:
  `df.rolling(5).mean()` translates to OEP-0001-R079 member forms, making
  Python interop wrapper-free on `Any` subjects.

- Revision 3, 2026-08-01: Full migration: `.osrx` becomes `.ois`, the one
  authoring surface; the S-expression notation retreats to internal form
  representation with `.osr` as transitional input, rewritten by hand.
  R004 gains the backtick escape for operator-named members; R005 gains
  localized `@doc`; tooling (LSP, editor extensions) targets `.ois`.
  (`.ox` was considered and rejected: it is the source extension of the Ox
  econometrics language, whose users overlap Osiris's quant audience.
  `.oxr` was briefly chosen before `.ois` — the natural contraction of
  Osiris — whose only known use is an OriginLab internal theme file.)

- Revision 2, 2026-08-01: R004 extends the yield rule to `/`: slash glues
  into identifiers (`py/import`, slash-qualified names), division requires
  surrounding whitespace. `osr init` templates emit `.ois` per R002.

- Revision 1, 2026-08-01: Initial version: the Elixir-flavoured surface as
  primary (`.ois`), everything-is-a-call translation, infix-yields-to-
  kebab-case identifiers, first-argument pipe, do-blocks as named-body macro
  calls, coexistence and roadmap.
