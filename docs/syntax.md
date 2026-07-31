---
document-id: language/syntax
title: Osiris Syntax
language: en
revision: 14
---

# Osiris Syntax

This is the concise, release-versioned syntax manual for Osiris. The native
`osr` executable embeds this complete English document under the stable ID
`language/syntax`; `osr syntax` prints it without network access.

## Source Files

Osiris source files use the `.osr` extension and UTF-8. A semicolon starts a
comment that continues to the end of the line. Commas outside strings are
whitespace, so `[1, 2]` and `[1 2]` have the same meaning.

The reader recognizes data. A list is interpreted later as a core form, macro
invocation, or function call. Packages cannot add reader syntax; they extend the
language with ordinary functions and hygienic macros.

## Data Forms

```clojure
none                         ; absence
true false                   ; booleans
42 -7 3.14                   ; numbers
"text\nnext"                  ; string
:ready                       ; keyword
(f x y)                      ; list: call, macro, or core form
[x y z]                      ; vector
{:name "sample" :value 0.2}  ; map
#{:new :ready}               ; set
```

Collections can nest without special delimiters. Keywords are data in
collections and name keyword arguments in a call:

```clojure
(configure :window 20 :strict true)
```

The phase-1 reader forms are:

```clojure
'form       ; quote
`template   ; hygienic syntax quote
~value      ; unquote inside syntax quote
~@values    ; unquote-splicing inside syntax quote
```

## Embedded Language Blocks

A named embedded block keeps foreign or multi-line text readable without
string escaping:

```clojure
~json<settings>
{"theme": "dark", "compact": true}
</settings>

(export [settings])
```

The language tag is lowercase and the closing label must exactly match the
opening label. The label declares a module-private `Str` binding for generic
languages such as `json`, `markdown`, `sql`, `html`, `css`, `javascript`,
`typescript`, `toml`, and `yaml`. It crosses modules only through ordinary
`export` and `import`. Unknown well-formed language tags are also valid raw
text; they do not load compiler extensions.

The body is raw. Quotes, semicolons, delimiters, and `~` have no Osiris meaning
inside it. In the multi-line form above, the opening/closing structural
newlines and the closing tag's common indentation are removed. Choose another
label when the body itself must contain the exact closing text.

`python` is different: its label is a private provider handle, not a `Str` and
not an exportable binding. Use it only from a symbolic local `extern`:

```clojure
~python<text-backend>
def normalize(value: str) -> str:
    return value.strip().casefold()
</text-backend>

(extern python text-backend
  (defn ^Str normalize [^Str value]))
```

A provider may name a file instead of carrying its body inline, which keeps one
source of truth and lets ordinary Python tooling read it:

```clojure
(py/embed text-backend "backend/text.py")

(extern python text-backend
  (defn ^Str normalize [^Str value]))
```

The path is relative to the `.osr` that names it, must stay inside a source root,
must end in `.py`, and must not be a symlink. One file backs one provider. The two
forms are indistinguishable downstream — same relocation, same hashes, same
interface — so choose by how you prefer to edit.

`extern python text-backend` links that local authored module into the
distribution-private `__osiris_runtime__`. By contrast,
`extern python "package.module"` names an external Python module supplied by
uv/PyPI and is never copied. Embedded modules may statically import one another;
the compiler relocates the reachable private module graph and never executes it
while compiling.

## Names

Names may use Unicode. Name identity uses Unicode NFC while diagnostics retain
the authored spelling. Common spellings include:

```clojure
format-message        ; Lisp-style name
ready?                ; predicate convention
text/format-message   ; qualified Osiris name
row.value             ; statically resolved field or Python attribute
格式化文本             ; Unicode source name
```

Localized names are aliases of one canonical binding, not independent
definitions. Locale never changes name resolution.

### Names in generated Python

An Osiris name is spelled Lisp-style; Python is not. The compiler maps between
them deterministically, and the result is what a Python caller imports, so the
mapping is a stable part of the interface rather than an internal detail.

| Osiris | Python |
| --- | --- |
| `-` | `_` |
| `?` | `_p` |
| `!` | `_bang` |
| letter or digit, including non-ASCII | unchanged |
| anything else | `_u<hex>_` |

```text
rolling-mean   ->  rolling_mean
missing?       ->  missing_p
reset!         ->  reset_bang
column*        ->  column_u2a_
均线            ->  均线
```

A name that would start with a digit gains a leading `_`, one that collides
with a Python keyword gains a trailing `_`, and compiler-internal names carry an
`_osr_` prefix so they cannot collide with yours. Two Osiris names that would
produce one Python name are a diagnostic, not a silent merge.

Module paths map component by component — `dm.dsl.pandas` stays
`dm.dsl.pandas` — except where the whole path is used as one identifier, such as
an import alias, where the dots map too: `dm_u2e_dsl_u2e_pandas`.

Distribution names follow Python packaging instead: PEP 503 normalizes
`Osiris_Pandas` to `osiris-pandas`, and PEP 427/625 escape that to
`osiris_pandas` wherever a filename or directory carries it.

## Modules and Imports

A module normally starts with its canonical module name and explicit imports
and exports:

```clojure
(module analytics.pipeline)

(import analytics.transforms :as transforms :refer [sum-values])
(import-for-syntax analytics.macros :as macros :refer [unless])
(py/import math :as math)

(export [normalize summarize])
(alias summarize-legacy summarize)
```

- `import` reads another Osiris module's `.osri` interface.
- `import-for-syntax` imports macros and phase-1 helpers.
- `py/import` emits a Python runtime import; it does not execute Python while
  compiling.
- `export` defines the public module interface.
- `alias` retains a migration spelling for an existing binding. References
  remain valid but receive a non-failing replacement advisory. Use
  `:osiris/names` with `:preferred` for a recommended localized spelling.

The project configuration maps source paths to module names, and the source
tree is spelled the Osiris way: a file or directory name is the module
component as written, `-` included, matching the `module` declaration
literally. Only generated output switches to the Python spelling:

```text
src/osiris-test/core.osr        (module osiris-test.core)     ← authored, Osiris spelling
dist/osiris_test/core.py        import osiris_test.core       ← generated, Python spelling
```

Do not pre-translate a directory name to `_` yourself — the compiler rejects a
source path that does not match the declared module name, and `osr lsc name`
shows what any name becomes on the Python side.

### Documenting macro clause words

A macro whose calls carry clause forms — `(选股定义 name (因子 …) (输出 …))` —
documents those words with `:osiris/clauses` on its declaration:

```clojure
^{:doc {:default "Define a selection strategy."}
  :osiris/clauses
  {因子 {:default "Declare one factor input." "zh-CN" "声明一个因子输入。"}
   输出 {:default "The output expression." "zh-CN" "输出表达式。"}}}
(defmacro 选股定义 [name & clauses] …)
```

Hovering a clause word inside a call then answers with that clause's
documentation; words the macro leaves undocumented answer with the macro
itself. The key is presentation data and never enters the semantic interface,
so editing clause documentation recompiles no dependents.

## Publishing a Declaration

Nothing is public unless it says so, and there are two explicit ways to say it.
The `(export [...])` manifest above names declarations from one place. The
per-item marker rides on the declaration itself:

```clojure
^:export (def ^Int limit 20)

^{:doc "Advance one step." :export true}
(defn ^Int step [^Int value] (+ value 1))
```

`^:export` reads as `^{:export true}`, and metadata written before a form is
merged with metadata written on the declared name, so `^:export (def x 1)` and
`(def ^:export x 1)` are the same declaration. Only `true` publishes; any other
value stays ordinary authored metadata.

The public surface is the union of the two, and a name written both ways is
published once. A declaration published by neither is module private — there is
no separate private form, and no metadata key asserts privacy.

A marker works wherever the manifest does, including an embedded data block and
a declaration nested inside `extern`. Marking something with no public name — a
bare expression, an import, an embedded Python provider handle — is an error
(`OSR-N0016`) rather than a marker that quietly does nothing.

The marker is the one of the two a macro can produce. `export` is an authored
boundary form fixed before expansion, so a macro cannot generate one; a marker
is ordinary metadata on an ordinary declaration, so a declaration macro can
publish what it generates instead of requiring every call site to repeat the
names:

```clojure
(defmacro define-factor [label]
  `(do ^{:doc "Factor name." :export true} (def ^Str factor-name ~label)
       ^{:doc "Compute." :export true} (defn ^Int calculate [^Int value] value)))
```

## Definitions and Functions

```clojure
(def answer 42)

(defn ^Int add
  [^Int left ^Int right]
  (+ left right))

(defn ^Int clamp-low
  [^Int value [^Int minimum = 0]]
  (if (< value minimum) minimum value))

(def increment
  (fn [value] (+ value 1)))
```

`def` binds a value. `defn` defines a named function and `fn` creates an
anonymous function. Parameters are a vector. Put `& rest` at the end for a
variadic parameter. A defaulted parameter uses `[name = expression]`, with any
type metadata attached to `name` as shown above.

Every form is an expression. A function and `do` return their final expression.
Evaluation is left to right, and an expression with effects is not duplicated
by macro expansion or code generation.

## Rich Metadata

The `^` reader prefix attaches immutable, non-executable Rich Metadata to the
following supported syntax node:

```clojure
~osiris<increment-all-example>
(increment-all [1 2 3])
;; => [2 3 4]
</increment-all-example>

^:deprecated
^{:doc {:default "Increment every integer."
        "zh-CN" "将每个整数加一。"}
  :since "0.1"
  :osiris/names
  {"zh-CN" {:preferred 全部加一
             :aliases [逐项加一]}}
  :examples [increment-all-example]}
(defn ^{:type (Vector Int)} increment-all
  [^{:type (Vector Int)} values]
  (mapv (fn [^Int value] (+ value 1)) values))
```

Supported shorthand follows Clojure:

```clojure
^:flag       ; {:flag true}
^TypeTag     ; {:tag TypeTag}
^"tag"       ; {:tag "tag"}
^[A B _]     ; {:param-tags [A B _]}
```

In `:doc`, `:default` is the author's untagged fallback. Reusable packages
should write it in English, but any language is valid. Other keys must be
standard BCP 47 language tags such as `"en"`, `"zh-CN"`, or `"ja"`. Tooling
uses RFC 4647 lookup and falls back to `:default` without pretending that the
fallback has a language tag.

Metadata holds inert data only: no reader forms, embedded blocks, non-finite
numbers, or metadata on metadata.

A macro template is the one place `^{...}` may contain unquote, because a
template's metadata is complete only after substitution. Syntax quoting
substitutes inside metadata exactly as it does inside the datum, and the inert
rule is applied to what expansion produces:

```clojure
(defmacro documented [name text]
  `^{:doc {:default ~text}} (defn ~name [value] value))
```

This is how a declaration macro attaches documentation it computes. Nothing is
relaxed about the result — expanded metadata that is not inert data is still
rejected.

Documentation examples use named `~osiris` blocks. `:examples` is a vector of
unquoted, same-module block names:

```clojure
~osiris<reduce-example>
(reduce + 0 [1 2 3 4])
;; => 10
</reduce-example>

^{:examples [reduce-example]}
(defn reduce ...)
```

Each block is one complete, canonically formatted snippet. A reference used
only by metadata is not emitted as a runtime string. LSP, LSC, package
interfaces, and Agent-facing JSON retain the resolved content together with
its language, label, source span, and content hash.

Long documentation may similarly reference same-module `~markdown` blocks.
Literals remain convenient for short text, and every locale may choose either
form:

```clojure
~markdown<normalize-doc>
Normalize text for stable comparison.
</normalize-doc>

^{:doc {:default normalize-doc
        "zh-CN" "规范化文本以便稳定比较。"}}
(defn normalize ...)
```

`:osiris/names` may also be attached directly to a function parameter. This
publishes localized keyword spellings as part of that function's static
signature:

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

Given a statically known signature, localized parameter names are accepted as
keyword arguments:

```clojure
(format-message "Hello, {name}!" :显示名称 "Osiris" :大写 true)
(格式化文本 "Hello, {name}!" :名称 "Osiris")
```

Every parameter locale entry has the same shape as a declaration locale entry:
`:preferred` is one Symbol and optional `:aliases` is a Vector of Symbols.
Use `:preferred` for the normal localized spelling. Reserve `:aliases` for old
spellings that remain accepted during source migration; do not use aliases as
an unordered list of equally recommended translations. Tooling and the
compiler should suggest replacing an authored alias with the locale's
preferred spelling, or with the canonical name when no preferred spelling is
available.

Parameter aliases belong only to their declaring function signature; they are
not global keyword substitutions. The compiler resolves every preferred name
and alias to the canonical parameter identity before checking missing,
unknown, duplicate, and mistyped arguments. `:uppercase`, `:转为大写`, `:大写`,
and `:使用大写` all identify the same parameter; passing any two of them is a
duplicate-argument error. The example call using `:大写` remains valid but is a
migration use and should receive a non-failing replacement diagnostic.
Generated Python always uses the canonical Python parameter name, regardless
of the source spelling or display locale. Calls through `Any` or an untyped
Python boundary have no static signature, so their keyword names cannot be
translated automatically.

Phase 1 can read and immutably update metadata with `meta`, `with-meta`, and
`vary-meta`. Metadata cannot claim compiler-verified type, effect, temporal, or
data facts.

## Types

Types are attached to declarations and bindings as Rich Metadata. The compact
`^Int` spelling is a tag; parameterized types use `^{:type ...}`.

```clojure
^Int
^{:type (Vector Int)}
^{:type (Map Str Float)}
^{:type (Option Str)}
^{:type (Union Int Float)}
^{:type (Fn [Int Int] -> Int)}
```

Core type names include `Bool`, `Int`, `Float`, `Str`, `Bytes`, `None`, `Any`,
and `Never`. Core constructors include `List`, `Vector`, `Map`, `Set`, `Option`,
`Union`, `Tuple`, and `Fn`. Nominal and data-library types use the same type-form
syntax and come from ordinary interfaces; DataFrame, Series, NumPy, Pandas, and
Polars behavior is not hard-coded into the reader.

Omitting an annotation requests local inference, not implicit `Any`. Exported
signatures and Python host boundaries must be complete. Use explicit `Any` at a
genuinely dynamic boundary.

## Structures

`defstruct` creates a nominal, immutable structure with ordered typed fields,
optional defaults, and constructor checks:

```clojure
(defstruct Threshold
  "A closed threshold range."
  [minimum Float]
  [maximum Float]
  [enabled Bool = true]

  (check (<= minimum maximum)
         "minimum must not exceed maximum"))

(def threshold
  (Threshold :minimum 0.0 :maximum 1.0))

threshold.maximum
```

A generic struct puts its type parameters with its name:

```clojure
(defstruct (Pair A B)
  [left A]
  [right B])
```

The `[field Type = default]` shape is specific to `defstruct` fields. Structs
are nominal even when two declarations have identical fields.

## Core Expressions

The small compiler-owned expression kernel contains `fn`, `let`, `if`, `do`,
`try`, and `raise`:

```clojure
(let [^Int x 10
      ^Int y (+ x 2)]
  (if (> y 10)
    (do (record y) y)
    0))
```

Core `if` requires `Bool`. `osiris.core` condition macros such as `when`, `cond`,
`if-let`, and `and` use Clojure truthiness: only `none` and `false` are false;
zero, empty strings, and empty collections are true.

Vectors and maps can be destructured in binding positions. For example:

```clojure
(let [[first-value second-value] values
      {:keys [name count]} options]
  (combine name count first-value second-value))
```

## Hygienic Macros

Define a macro with `defmacro`. It receives syntax and must return syntax that
expands to normal Osiris forms:

```clojure
(defmacro unless [condition & body]
  `(if (not ~condition)
     (do ~@body)
     none))

(defmacro twice [expression]
  `(let [value# ~expression]
     (+ value# value#)))
```

A name a template binds in `let` or `fn` is hygienic: it receives a fresh
identity on every expansion, so caller syntax spliced in through unquote can
never be captured by it. Writing `value#` makes that identity explicit and is
still required when one name must be shared across two separate templates,
because `value#` is stable only within a single syntax quote.

Reaching a call-site name is deliberate and needs the explicit `~'name`
operation:

```clojure
(defmacro with-it [expression body]
  `(let [~'it ~expression] ~body))    ; `it` is visible to the caller's body
```

Two shapes stay outside this pass because they cannot be read statically: a
binding vector built by unquote-splicing (`` `(let [~@pairs] ...) ``) and map
destructuring in a template binding position. Names there keep their authored
spelling, so use `value#` or `(gensym)` when writing them.

Unquoted syntax retains the caller's context. A template name exported by the
defining module resolves at that module; a name defined in the same module as
the macro currently resolves at the call site, so qualify it or pass it through
unquote when the caller might shadow it. Macros run in a deterministic,
restricted phase 1 and cannot import Python, access the network, inspect
runtime values, or bypass normal type and semantic checks.

Use `defn-for-syntax` for a compile-time helper and `import-for-syntax` for a
compile-time dependency. Use `osr expand <path>` to inspect expansion.

## Threading and Control Macros

The public `osiris.core` surface is referred automatically when a module has no
explicit core import. Threading and control forms therefore work directly:

```clojure
(->> events
     (map event-value)
     (reduce add 0))
```

An explicit core import completely replaces that default. Use it to select a
smaller surface, or to exclude and rename conflicting spellings:

```clojure
(import osiris.core
  :refer :all
  :exclude [map]
  :rename {reduce fold-left})
```

An omitted `:exclude` or `:rename` is empty. Local declarations shadow only
implicit core spellings; `osiris.core/map` remains available through its
qualified name. Explicit imports diagnose local collisions. Core supplies
Clojure-style threading macros:

```clojure
(-> value
    (clean)
    (normalize options))

(->> events
     (map event-value)
     (reduce add 0))
```

`->` inserts the previous result as the first argument. `->>` inserts it as the
last argument. `cond->`, `cond->>`, `some->`, `some->>`, `as->`, and `doto`
provide their corresponding conditional, optional, named, and side-effecting
flows. The initial expression is evaluated once.

`for` supports multiple collection bindings and interleaved clauses and returns
a memoized LazySeq:

```clojure
(for [left left-values
      right right-values
      :let [sum (+ left right)]
      :when (> sum 0)
      :while (< sum 100)]
  sum)
```

- A binding pair introduces nested iteration from left to right.
- `:let` introduces local bindings for the current combination.
- `:when` skips the current result when its condition is false.
- `:while` stops the lexically nearest collection when its condition is false.

`forv` accepts the same shape and eagerly returns a Vector. `doseq` uses the
same binding clauses for effects and returns `none`.

## Recursion and Sequences

Use `loop` with tail-position `recur` for explicit constant-stack state:

```clojure
(loop [index 0
       total 0]
  (if (= index 100)
    total
    (recur (+ index 1) (+ total index))))
```

Tail-position `recur` in a function targets that function when no nearer `loop`
exists. Arity, state types, and tail position are checked. Both forms lower to
constant-stack Python control flow. Use `trampoline` for mutual recursion.

Common sequence operations include `map`, `mapv`, `mapcat`, `filter`, `reduce`,
`fold`, `take`, `drop`, `partition`, `some`, and `every?`. `reduce` accepts
`(reduce f coll)` or `(reduce f initial coll)`; `fold` requires an initial value.
Use `reduced` for early termination and `lazy-seq` for explicit deferred,
memoized production of large or infinite sequences.

## Exceptions

```clojure
(try
  (parse value)
  (catch ValueError error
    (recover error))
  (catch Exception error
    (report error))
  (finally
    (cleanup)))

(raise error)
```

`try` accepts zero or more `catch` clauses followed by at most one `finally`.
The core `throw` form is the Clojure-style spelling of `raise`.

## Python Interoperation

Use explicit boundaries so compilation never imports or executes Python:

```clojure
(py/import host.runtime :as host)

(extern python "host.runtime"
  (defn ^Any register
    [^{:type (Map Str (Vector Str))} extra-data]))

(py/decorate publish
  (register :extra-data {"columns" ["value" "year"]}))

(defn ^Any publish
  [^Any context [^Str field = "value"]]
  (context.emit field))
```

`extern` declares a typed Python ABI. `py/decorate` attaches Python decorators
to a generated declaration; decorators are runtime behavior, not Rich Metadata.
Known keyword arguments are checked and emitted with canonical Python names.

Use a `~python` provider when a small package-owned backend must be distributed
with generated output; use a string module name for a normal installed Python
dependency. `osr fmt` formats embedded Python in-process with the compiler's
pinned Ruff profile. In VS Code, completion, hover, navigation, diagnostics,
rename, and range formatting inside an embedded block are delegated to the
installed language support for its tag. Missing foreign language support does
not affect Osiris compilation or tooling.

Generated Python is standalone with respect to Osiris. When a reachable
standard operation needs reusable support, the linker emits ordinary Python
under the owning package's reserved `__osiris_runtime__` package. Osiris source
MUST NOT declare that package or import its private names directly.

## Standard Library Resources

The compiler embeds only Kernel and Bootstrap source. Public standard-library
modules are ordinary `.osr` resources shipped in the matching `osiris-lang`
distribution. Compilation, LSP, LSC, source maps, and linking read them through
one validated resource provider.

The compiler carries the SHA-256 identity of the complete standard resource
tree. Missing or modified resources are an invalid installation; the compiler
does not silently use a second public-source copy from its executable.
`osiris-stdlib:///osiris/core/transform.osr` is a stable logical URI resolved
through that provider, not evidence that the source is embedded in the binary.
Generated Python still has no runtime dependency on `osiris-lang`.

## Complete Minimal Module

```clojure
(module sample.stats)

(export [Summary positive-sums summarize])

(defstruct Summary
  [count Int]
  [total Int])

~osiris<positive-sums-example>
(positive-sums [-2 1] [1 3])
;; => [1 2 4]
</positive-sums-example>

^{:doc {:default "Return positive Cartesian sums."
        "zh-CN" "返回笛卡尔组合中的正数和。"}
  :examples [positive-sums-example]
  :osiris/names
  {"zh-CN" {:preferred 正数组合}}}
(defn ^{:type (Vector Int)} positive-sums
  [^{:type (Vector Int)} left-values
   ^{:type (Vector Int)} right-values]
  (forv [left left-values
        right right-values
        :let [sum (+ left right)]
        :when (> sum 0)]
    sum))

~osiris<summarize-example>
(summarize [2 3 5])
;; => (Summary :count 3 :total 10)
</summarize-example>

^{:doc {:default "Summarize a vector of integers."}
  :examples [summarize-example]}
(defn ^Summary summarize
  [^{:type (Vector Int)} values]
  (Summary
    :count (count values)
    :total (reduce + 0 values)))
```

Run `osr fmt` and `osr check` after writing or changing source. Use `osr lsc`
for CLI access to the same diagnostics, hover, completion, signatures,
navigation, symbols, and semantic facts that IDEs receive through LSP.
Use `osr check` to validate complete examples; diagnostics remain local and
deterministic.
