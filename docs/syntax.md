---
document-id: language/syntax
title: Osiris Syntax
language: en
revision: 3
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

## Names

Names may use Unicode. Name identity uses Unicode NFC while diagnostics retain
the authored spelling. Common spellings include:

```clojure
rolling-mean          ; Lisp-style name
ready?                ; predicate convention
series/rolling-mean   ; qualified Osiris name
row.value             ; statically resolved field or Python attribute
时序均值               ; Unicode source name
```

Localized names are aliases of one canonical binding, not independent
definitions. Locale never changes name resolution.

## Modules and Imports

A module normally starts with its canonical module name and explicit imports
and exports:

```clojure
(module analytics.pipeline)

(import analytics.transforms :as transforms :refer [sum-values])
(import-for-syntax analytics.macros :as macros :refer [unless])
(py/import math :as math)

(export [normalize summarize])
(alias 汇总 summarize)
```

- `import` reads another Osiris module's `.osri` interface.
- `import-for-syntax` imports macros and phase-1 helpers.
- `py/import` emits a Python runtime import; it does not execute Python while
  compiling.
- `export` defines the public module interface.
- `alias` adds another resolvable spelling for an existing binding.

The project configuration maps source paths to module names. A written
`module` declaration must agree with that mapping.

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
^:deprecated
^{:doc {:default "Increment every integer."
        "zh-CN" "将每个整数加一。"}
  :since "0.1"
  :osiris/names
  {"zh-CN" {:preferred 全部加一
             :aliases [逐项加一]}}
  :agent/tags [:data :transform]}
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

An automatic gensym such as `value#` creates a fresh binding on every
expansion. Template names resolve at the macro definition; unquoted syntax
retains the caller's context. Macros run in a deterministic, restricted phase 1
and cannot import Python, access the network, inspect runtime values, or bypass
normal type and semantic checks.

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

Generated Python is standalone with respect to Osiris. When a reachable
standard operation needs reusable support, the linker emits ordinary Python
under the owning package's reserved `__osiris_runtime__` package. Osiris source
MUST NOT declare that package or import its private names directly.

## Complete Minimal Module

```clojure
(module sample.stats)

(export [Summary positive-sums summarize])

(defstruct Summary
  [count Int]
  [total Int])

^{:doc {:default "Return positive Cartesian sums."
        "zh-CN" "返回笛卡尔组合中的正数和。"}
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

^{:doc "Summarize a vector of integers."}
(defn ^Summary summarize
  [^{:type (Vector Int)} values]
  (Summary
    :count (count values)
    :total (reduce + 0 values)))
```

Run `osr fmt` and `osr check` after writing or changing source. Use `osr lsc`
for CLI access to the same diagnostics, hover, completion, signatures,
navigation, symbols, and semantic facts that IDEs receive through LSP.
