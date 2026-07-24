---
oep: 3
title: Standard Library Architecture and Initial API
description: Public namespaces, phase boundaries, initial functions and macros, runtime contracts, artifacts, and tooling requirements for the Osiris standard library.
author: MJ
status: Draft
type: Standards Track
areas:
  - Language
  - Standard Library
  - Packaging
  - Tooling
created: 2026-07-23
updated: 2026-07-24
revision: 9
requires: [0, 1, 2]
replaces: []
superseded-by: null
resolution: null
translations:
  zh: local/zh/0003-standard-library.md
---

# OEP-0003: Standard Library Architecture and Initial API

## Abstract

This proposal defines the architecture and public contract of the Osiris
standard library. It separates the compile-time bootstrap environment, the
user-facing `osiris.core` namespace, explicit standard modules, and private
Linkable helpers. It also defines macro phase boundaries, explicit core
imports, sequence semantics, type and contract visibility, distributed
artifacts, localization metadata, and tooling behavior.

It also fixes the initial callable and macro catalog. Names alone are not an API
contract: this proposal specifies source call shapes, evaluation strategy,
return families, edge behavior, effects, and ownership for the initial core,
collection, sequence, string, math, concurrency, and Python-interoperation
surfaces.

The implementation is divided into exactly two ownership layers. The Kernel is
a minimal, compiler-embedded set of forms, intrinsics, Phase-1 primitives, and
target helpers that cannot be implemented without privileged compiler or target
access. The public standard library is an ordinary, source-distributed Osiris
package bootstrapped by that Kernel. Public macros and ordinary functions are
authored in `.osr`; reachable target support is compiled into each consuming
output.

## Motivation

Osiris needs a compact compiler and an expressive data-oriented language.
Adding every convenience to Rust would increase compiler complexity, duplicate
logic across the compiler and LSP, and make extension boundaries unclear.
Placing every operation in the Python runtime would make the standard library
harder to type, inspect, translate, and navigate from Osiris source.

A specified standard library should make common Osiris programs concise while
preserving these properties:

- macro expansion remains deterministic and sandboxed;
- runtime functions remain first-class values;
- type, effect, temporal, and data-property solvers see expanded semantics;
- generated Python remains readable;
- standard APIs expose stable `.osri` identities and bilingual metadata;
- Pandas, Polars, NumPy, and domain behavior remain ordinary extensions.

## Scope

This proposal specifies:

- the standard-library layers and public namespace boundary;
- the compile-time subset available to standard macro implementations;
- the dependency and output restrictions on standard macros;
- the initial public standard modules and explicit `osiris.core` import surface;
- the initial public macro/function inventory and its observable behavior;
- lazy and eager sequence naming conventions;
- public type, effect, contract, metadata, and `.osri` requirements;
- standard-library distribution, loading, versioning, and conformance.

This proposal does not specify:

- internal Rust module layout, evaluator algorithms, or Python helper files;
- optimization strategy or a particular lazy-sequence data structure;
- DataFrame, Array, Series, axis, schema, or domain-specific APIs;
- package resolution beyond the existing PyPI and uv contract;
- custom reader syntax or parser plugins;
- APIs for standard modules added by a later OEP.

## Terminology

- **Kernel**: the closed, minimal compiler-owned forms, intrinsics, Phase-1
  primitives, and target helpers that ordinary Osiris source cannot implement
  without privileged compiler or target access. It is not a public namespace.
- **Standard source package**: the ordinary Osiris package containing every
  public `osiris.*` namespace, wrapper, macro, signature, and localized Rich
  Metadata record. It is compiled by Osiris and distributed with authored
  `.osr` source.
- **Phase 1**: deterministic compile-time evaluation used to run macros and
  compile-time helpers over syntax data.
- **Phase 0**: ordinary runtime program semantics that lower to typed HIR and
  generated Python.
- **Bootstrap library**: the minimal Phase-1 functions available before
  user-facing standard macros are loaded.
- **Core namespace**: the public `osiris.core` module and its reviewed set of
  explicitly importable bindings.
- **Standard module**: a public `osiris.*` namespace shipped as part of the
  standard library.
- **Implementation namespace**: a hierarchical namespace such as
  `osiris.core.kernel` that organizes source-distributed implementation
  contracts behind a public facade. Namespace hierarchy is organization, not
  binding privacy.
- **Linkable helper**: compiler-owned, target-neutral HIR used to generate a
  distribution-private Python implementation when direct lowering is not
  sufficient.
- **Intrinsic**: a versioned operation whose type, effect, temporal, data, or
  control-flow contract is known to typed HIR.
- **Core export manifest**: the versioned list of public `osiris.core` bindings
  available to the module import mechanism.
- **Facade identity**: the stable public binding identity exposed by a standard
  namespace independently of internal source organization.
- **Generated support package**: the reserved `__osiris_runtime__` Python
  package generated inside one output package from reachable Linkable helpers.

## Specification

### Library layers and ownership

**OEP-0003-R001:** The implementation MUST contain exactly two standard
ownership layers: a compiler-embedded Kernel and a source-distributed standard
package. The user-facing package MUST expose `osiris.core` as its core
namespace. Compiler-owned Kernel and Bootstrap operations are not public user
APIs. Source-distributed implementation namespaces MUST be omitted from the
public standard namespace and API catalogs.

**OEP-0003-R002:** Generated Python MUST NOT depend on a shared
`osiris.prelude`, `osiris`, or `osiris-lang` runtime package. Standard macros,
functions, and intrinsics MUST either lower directly to ordinary Python or link
their reachable support implementation into the consuming distribution's
private `__osiris_runtime__` package.

**OEP-0003-R003:** The Kernel MUST be embedded in the native compiler. The
standard library MUST instead be built and published as an ordinary Osiris
package resource in the `osiris-lang` distribution, with its complete authored
`.osr` source tree preserved. Its generated `.osri`, macro IR, source indexes,
and Linkable artifacts MUST be reproducible from that source with the matching
compiler. Python generated from them MUST be self-contained within the
consuming output and MUST NOT require an Osiris distribution at runtime.

**OEP-0003-R004:** The standard library MUST remain domain-neutral. Pandas,
Polars, NumPy, DataFrame schemas, quantitative rules, and other framework or
domain contracts MUST be provided by PyPI extensions rather than recognized by
the standard library or Rust Kernel.

**OEP-0003-R005:** A convenience API MUST be implemented at the highest layer
that preserves its semantics: hygienic macro, ordinary Osiris function,
ordinary Python lowering, Linkable helper, typed intrinsic, and only then a new
Kernel form.

**OEP-0003-R005A:** Every Kernel operation intended for standard-library use
MUST be wrapped by an ordinary public standard binding or consumed privately by
a standard macro. Kernel spelling, module layout, and helper identity MUST NOT
appear in a public signature or user import. Public wrappers MUST carry complete
Rich Metadata with an authored `:doc` `:default` and a `"zh-CN"` translation.

**OEP-0003-R005B:** Kernel declarations MUST follow a minimal-metadata rule.
They MUST carry only identity, phase, type, effect, intrinsic/linker contract,
and provenance data required for correct compilation. They MUST NOT carry
localized names, localized documentation, `^:doc`, or convenience metadata.
Kernel diagnostics are compiler diagnostics governed by OEP-0001, not API
documentation records.

**OEP-0003-R005C:** Public standard declarations, their signatures, their Rich
Metadata, and their implementation bodies MUST have the distributed `.osr`
source as the normative source of truth. A Rust table, generated facade, Python
template, or checked-in `.osri` file MUST NOT independently define a public
standard API. Generated artifacts MAY be cached or embedded for startup, but a
release build MUST validate them against the packaged source and reject stale
or divergent artifacts.

**OEP-0003-R005D:** A public callable or value that wraps a Kernel leaf MUST be
an authored ordinary `defn` or `def` in the standard source package; the public
declaration itself MUST NOT be an `extern`. A typed `extern python
"osiris.kernel"` declaration for the leaf MUST reside in the hierarchical
implementation namespace `<public-namespace>.kernel`. Every public standard
module MUST import that namespace with `:refer :all`; a public facade MUST NOT
contain a direct `extern python "osiris.kernel"` boundary. The declaration MUST
use only the minimal Kernel metadata from R005B and MUST be referenced by the
public Osiris implementation body. Consequently, compiling and linking the
facade exercises the ordinary Osiris frontend, type checker, HIR, and Python
backend rather than defining the public API in Rust.

**OEP-0003-R005E:** One standard-library source file MUST declare exactly one
module, and one module MUST have exactly one normative source file. A module
MUST NOT be assembled by repeating `(module name)` across multiple files.
Large implementations MUST be split with hierarchical namespaces and ordinary
imports. Moving a declaration into an implementation namespace MUST NOT change
its public facade identity.

The standard package MAY declare `:osiris/facade-modules` metadata on
`osiris.core` to list the hierarchical implementation modules that author its
Phase-0 declarations, Phase-1 helpers, and macros. It MUST declare the exact
public macro names in `:osiris/facade-macros`. The standard artifact builder MAY
assemble a derived `osiris.core` compilation unit from parsed declaration spans
in those registered modules. This is a standard-package build rule, not general
module re-export syntax. It MUST preserve each authored declaration body and
metadata, assign the public `osiris.core` identity, reject unregistered modules,
and leave every authored file as the sole normative source for its own declared
module.

**OEP-0003-R005F:** An implementation namespace MUST carry module metadata
`^{:osiris/internal true}`. It MAY export bindings required by a facade or a
sibling implementation namespace, but those exports are internal compilation
contracts: they MUST be omitted from the public standard namespace/API catalog,
need not carry user-facing `:doc`, and have no public compatibility guarantee.
This OEP does not require the compiler to reject an external package that
names such a namespace; catalog omission and compatibility status define the
boundary until a package-access-control OEP specifies enforcement.

**OEP-0003-R005G:** Osiris privacy MUST follow the Clojure binding model.
Namespace components, source paths, leading underscores, and trailing
underscores MUST NOT imply privacy. A namespace-private function MUST be
authored with `defn-`, which records `:private true`; other private declarations
MUST use their specified authored `:private true` form. Names such as
`osiris._core` MUST NOT be introduced to encode access control. A binding that
must cross from `osiris.core.kernel` to the `osiris.core` facade cannot be
`defn-`; it is exported from an internal namespace under R005F instead.

### Phase-1 bootstrap contract

**OEP-0003-R006:** A standard macro implementation MUST execute only in the
same sandboxed Phase-1 evaluator used for package macros. The compiler MUST NOT
provide a second privileged expansion engine for standard macros.

**OEP-0003-R007:** The Bootstrap library MUST provide immutable syntax-data
operations over `None`, `Bool`, `Int`, finite `Float`, `Str`, `Keyword`,
`Symbol`, `List`, `Vector`, `Map`, `Set`, `Syntax`, `Span`, and `Metadata`.

**OEP-0003-R008:** The Bootstrap library MUST provide enough pure operations to
construct, inspect, traverse, combine, and annotate syntax, including sequence
construction and access, `apply`, `map`, `reduce`, `gensym`, metadata access,
span access, and structured syntax errors.

**OEP-0003-R009:** Phase 1 MUST NOT expose Python imports or calls, file or
network access, environment variables, clocks, randomness, subprocesses,
threads, runtime dynamic bindings, project mutation, or inferred Phase-0 type
and effect results.

**OEP-0003-R010:** Phase-1 execution MUST enforce deterministic limits for
expansion count, evaluation steps, call depth, result nodes, and metadata
resources. The same source, interfaces, and options MUST produce the same
expanded syntax or the same deterministic diagnostic.

**OEP-0003-R011:** Standard Phase-1 dependencies MUST form an acyclic graph.
The graph MUST progress from the Kernel to Bootstrap helpers, core binding and
control macros, higher standard macros, and then extension macros. Runtime SCC
support MUST NOT relax the Phase-1 acyclicity rule.

### Macro output boundary

**OEP-0003-R012:** A macro implementation MUST terminate in Phase-1 Kernel
forms and Bootstrap functions. Its expanded result MUST terminate in Phase-0
Kernel forms, ordinary callable bindings, or typed intrinsics.

**OEP-0003-R013:** Standard macros MAY generate expressions and the controlled
declarations already permitted to declaration macros, including `def`, `defn`,
`defstruct`, `extern`, `py/decorate`, and `static-record`.

**OEP-0003-R014:** Standard macros MUST NOT generate or change `module`,
`import`, `import-for-syntax`, `py/import`, `export`, `alias`, `defmacro`,
`defn-for-syntax`, or `defstatic-schema`, because these forms establish module,
phase, public-binding, or static-schema graphs before expansion.

**OEP-0003-R015:** A standard macro MAY generate a call to an intrinsic or
Linkable helper, but Phase 1 MUST NOT execute it. Macro expansion MUST preserve
single-evaluation, evaluation order, lexical scope, metadata policy, and the
complete macro-origin chain promised by the macro contract.

### Public namespaces

**OEP-0003-R016:** The initial stable namespace set MUST include:

```text
osiris.core
osiris.collection
osiris.sequence
osiris.string
osiris.math
osiris.concurrent
osiris.python
```

An implementation MAY stage these modules across pre-stable releases, but it
MUST NOT mark this OEP Final until every namespace is conforming.

**OEP-0003-R017:** Public bindings MUST use facade identities that do not change
when private helper modules or source files move. A public `.osri` interface
MUST NOT expose a private bootstrap or implementation binding in a public
signature.

**OEP-0003-R018:** `osiris.collection` and `osiris.sequence` MUST operate on
logical Osiris collections and sequence protocols. They MUST NOT branch on
Pandas, Polars, or NumPy runtime types.

**OEP-0003-R019:** `osiris.python` MUST make dynamic Python boundaries explicit
in source and typed HIR. It MUST NOT provide a compile-time reflection escape
hatch or implicitly import arbitrary Python modules.

### Explicit core import contract

**OEP-0003-R020:** An ordinary Osiris module MUST explicitly import every
`osiris.core` binding it uses. The compiler MUST NOT make `osiris.core`
bindings visible through an implicit or automatic refer policy.

**OEP-0003-R021:** The initial core export manifest MUST contain these macro
bindings:

```text
and or
when when-not if-not
cond case condp
if-let when-let if-some when-some when-first
-> ->> some-> some->> cond-> cond->> as-> doto
defn- letfn loop recur for forv doseq dotimes while trampoline
lazy-seq lazy-cat delay force deref realized?
binding with-open assert throw comment time
```

**OEP-0003-R022:** The initial core export manifest MUST contain these ordinary
function bindings:

```text
identity constantly comp partial juxt complement
map mapv mapcat mapcatv filter filterv remove removev
reduce fold reductions reduced reduced? unreduced
first rest next nth count empty empty? seq seq? coll? sequential?
cons concat range repeat repeatedly iterate cycle sequence
take drop take-while drop-while take-last drop-last
partition partition-all partition-by interleave interpose
distinct dedupe flatten keep keep-indexed map-indexed
run! doall dorun some every? not-every? not-any?
get assoc dissoc update get-in assoc-in update-in select-keys
nil? some? true? false?
number? string? list? vector? map? set? sequence?
```

The initial core export manifest MUST also contain these nominal protocol type
bindings so public signatures never refer to a private or missing type:

```text
Reduced Future Promise Delay Lock
```

`keyword?` and `symbol?` MUST remain Bootstrap/Phase-1 predicates until the
runtime language has distinct Keyword and Symbol values. They MUST NOT pretend
to distinguish values after a target representation has erased that identity.

**OEP-0003-R023:** Adding, removing, renaming, or changing the identity of a
core export is a semantic compatibility change and MUST change the
standard-library semantic hash.

**OEP-0003-R024:** The normative all-core import shape is:

```clojure
(import osiris.core
  :refer :all
  :exclude [map]
  :rename {reduce fold-left})
```

`:refer :all` imports every public core export; omitted `:exclude` and
`:rename` clauses behave as empty collections. `:exclude` removes canonical
exports before local names are created. `:rename` maps remaining canonical
export names to local names. Unknown, duplicate, excluded-and-renamed, or
locally colliding names MUST produce deterministic diagnostics. These clauses
MUST be resolved before ordinary name lookup so binding identity does not
depend on import order.

### Initial module contracts

**OEP-0003-R025:** `osiris.collection` MUST provide a coherent initial set of
associative-data operations including `merge`, `merge-with`, `group-by`,
`frequencies`, `index-by`, `select-keys`, `rename-keys`, `update-keys`,
`update-vals`, `zipmap`, and `invert`.

**OEP-0003-R026:** `osiris.sequence` MUST provide a coherent initial set of
sequence producers, transforms, and consumers including `range`, `repeat`,
`repeatedly`, `iterate`, `take`, `drop`, `take-while`, `drop-while`,
`partition`, `partition-all`, `partition-by`, `interleave`, `interpose`,
`distinct`, `dedupe`, `flatten`, `mapcat`, `keep`, `keep-indexed`, and
`map-indexed`.

**OEP-0003-R027:** `osiris.string` MUST provide a deterministic Unicode-string
subset including trim operations, `split`, `join`, `replace`, prefix, suffix,
and inclusion predicates, basic case conversion, `blank?`, and line splitting.
Locale-sensitive and regular-expression behavior MUST document its Python
target dependency or remain outside the initial module.

**OEP-0003-R028:** `osiris.math` MUST initially remain a scalar API. Array
broadcasting, missing-value policy, vectorization, and framework-specific NaN
normalization MUST remain extension responsibilities.

**OEP-0003-R029:** `osiris.concurrent` MUST compose the versioned Future,
Promise, dereference, cancellation, lock, and parallel-map semantic contracts
from Linkable helpers emitted into `__osiris_runtime__`. It MUST NOT depend on a
global scheduler package or hide thread, timeout, exception, or cancellation
boundaries.

### Macros, functions, and intrinsics

**OEP-0003-R030:** Syntax organization and source-level evaluation control
SHOULD be hygienic macros when expansion can preserve semantics without a new
Kernel rule.

**OEP-0003-R031:** Runtime data operations that users may pass, store, return,
or select dynamically MUST exist as ordinary first-class Osiris functions.
`map`, `filter`, `reduce`, and `fold` MUST NOT exist only as macros.

**OEP-0003-R032:** A macro-specific optimized path MAY exist only if it
preserves the observable evaluation count, ordering, exception behavior,
typing, effect, temporal, and data-property contract of the ordinary function.

**OEP-0003-R033:** Solvers MUST identify standard operations through stable
binding identities, ordinary signatures, operator or protocol instances, and
versioned intrinsic contracts. Solvers MUST NOT infer semantics from unqualified
function-name strings.

**OEP-0003-R034:** A standard macro MUST NOT grant itself a verified type,
effect, temporal, data-property, trust, or availability fact. Such facts MUST
be derived from expanded typed HIR and validated contracts.

### Sequence semantics

**OEP-0003-R035:** `map`, `filter`, `remove`, `take`, and `drop` MUST return
logical lazy sequences. `mapv` and `filterv` MUST return eager vectors. `reduce`,
`fold`, `count`, and `group-by` MUST be eager consumers.

**OEP-0003-R036:** `reduce` and `fold` MUST support the `Reduced T` early
termination protocol. A reduced marker MUST terminate only the nearest
compatible reduction and MUST NOT change the public result type from `T`.

**OEP-0003-R037:** Multi-input mapping and corresponding zip-like sequence
operations MUST stop at the shortest input unless a distinct API explicitly
specifies padding or strict equal length.

**OEP-0003-R038:** Lazy-sequence realization and caching semantics MUST be
uniform across standard operations. A lazy sequence MUST NOT hide file access,
network access, thread creation, or an unrepresented external effect.

**OEP-0003-R039:** The logical semantics of List, Vector, Map, Set, and Sequence
MUST be independent of the concrete Python container selected by code
generation. Standard functions MUST NOT create a second type system by testing
private Python container implementations.

### Types, effects, and contracts

**OEP-0003-R040:** Every public ordinary function MUST publish a complete,
stable `.osri` signature. Public higher-order functions MUST preserve callback
parameter and result types, latent effects, temporal summaries, data properties,
and relevant collection-shape relationships.

**OEP-0003-R041:** Standard macros MUST expose their resulting operations to
typed HIR. A macro MUST NOT hide IO, state, time reads, alignment changes,
materialization, shape changes, or Python dynamic boundaries behind metadata
or an opaque helper.

**OEP-0003-R042:** Linkable helpers and intrinsics MUST have explicit,
versioned compiler-input and solver contracts. They MUST NOT create a deployed
shared runtime ABI. Unknown Python behavior MUST remain `Any` and unknown-effect
or unknown-data state unless an independently validated contract proves more.

### Rich Metadata and interface data

**OEP-0003-R043:** Every public standard binding MUST provide normalized Rich
Metadata in its authored `.osr` declaration with an English `:default`, a
`zh-CN` tagged documentation entry,
category, first `since` version, deprecation state, and stable source location.
Callable bindings MUST also provide public argument information. Documentation
and localized names MUST use the OEP-0001-R057 through OEP-0001-R065 metadata
contract; additional BCP 47 translations are encouraged and must not alter
binding identity.

**OEP-0003-R044:** Standard metadata MAY provide localized names, examples,
Agent intent, and Agent tags. Authored metadata MUST NOT manufacture inferred
or verified facts.

**OEP-0003-R045:** Standard `.osri` interfaces MUST contain public signatures,
macro signatures, validated macro IR, required private Phase-1 helper closures,
Rich Metadata, facade identities, source locations, and semantic/tooling hashes.

### Distribution, loading, and versioning

**OEP-0003-R046:** The `osiris-lang` release MUST package the standard library
as an ordinary source distribution resource using the OEP-0002 package layout.
It MUST contain `pyproject.toml`, `osiris.jsonc`, every public `.osr` source,
and the metadata needed by `osiris_build`. The package is the normative example
of publishing reusable Osiris source. Generated standard Python belongs to the
consuming output, not to a shared `osiris-lang` runtime package.

**OEP-0003-R047:** Standard artifacts and linked support MUST be reproducibly
generated from the packaged normative `.osr` source and compiler-owned Kernel
source.
Two builds with the same compiler, source, interfaces, Python target, reachable
binding set, and options MUST produce identical semantic interfaces and
byte-identical artifacts where the format promises determinism.

**OEP-0003-R048:** Compiler loading order MUST be:

```text
Phase-1 Bootstrap
-> packaged standard source and its validated interface cache
-> explicitly imported standard modules
-> uv.lock-reachable extension interfaces
-> project modules
```

No later layer may mutate an earlier interface or core export manifest.

**OEP-0003-R049:** User projects MUST NOT declare a standard-library or Osiris
runtime dependency. Installing the matching `osiris-lang` release as a build
tool MUST provide the compiler, packaged standard source, validated generated
artifact cache, and build backend. The completed Python output MUST run after
`osiris-lang` and `osr` are removed, subject only to the project's ordinary
declared Python dependencies.

**OEP-0003-R050:** `.osri` and build metadata MUST identify compiler ABI,
language ABI, standard-library ABI, Linkable-helper format, and Python target.
A mismatch MUST fail before macro execution or code generation. No runtime ABI
negotiation MAY be required after support has been linked into an output. This
OEP defines the initial standard-library ABI as integer `1`.

**OEP-0003-R051:** Changes to macro behavior, public signatures, evaluation
order, laziness, exception behavior, core exports, or solver-visible
contracts MUST affect the semantic hash. Documentation translation and display
ordering alone MUST affect only the tooling hash.

## Rationale

`osiris.core` gives users a conventional language-level namespace while the
linker turns its reachable implementation into ordinary distribution-private
Python. Keeping compile-time facade identities separate from generated support
names prevents helper layout from becoming either an Osiris API or an external
runtime dependency.

The split between macros and functions follows a semantic boundary rather than
an implementation preference. `when`, threading forms, and comprehensions
control source structure and evaluation, so macros are appropriate. `map`,
`filter`, and `reduce` are higher-order values, so they must remain functions.

Shipping the standard source package and compiler in one release avoids
unsupported combinations of macro IR, interface schema, Linkable helper format,
and solver contracts while keeping the language implementation inspectable.
Linking compiled standard source and Kernel helpers into each consuming output
trades small, tree-shaken duplication for standalone deployment and removes
cross-distribution runtime conflicts.

The initial module set emphasizes general data transformation without teaching
the language about a specific tabular framework. Extensions can build richer
DataFrame APIs while sharing standard collection, function, metadata, and
contract conventions.

## Backwards Compatibility

Osiris is pre-stable, so this proposal removes the user-facing and generated-
Python `osiris.prelude` dependency without a compatibility alias. Existing
pre-release output that imports it MUST be rebuilt; preserving a shared runtime
would retain the deployment coupling this proposal rejects.

Adopting lazy semantics for `map` and `filter` may differ from existing eager
helpers. The implementation release must document affected APIs and provide
explicit eager forms before removing an existing behavior.

Explicit imports can create source-name collisions. The first release
implementing this OEP must diagnose those collisions deterministically through
the import contract in OEP-0003-R024.

## Security and Determinism

The standard library has the same macro trust boundary as an installed
extension. Embedded or packaged macro IR is untrusted input until its format,
ABI, semantic hash, resource limits, and dependency identities are validated.

Compilation must not import or execute standard-library Python modules to
discover macros, types, metadata, contracts, or interfaces. All compilation
inputs must be static source, `.osri`, versioned manifests, or validated
compiler-owned data.

Lazy operations must expose effects and must not silently turn repeated
realization into repeated external IO. Linked support must preserve defined
evaluation and exception ordering and must not discover or load compiler data
at runtime.

Artifact loading must reject missing, duplicate, mismatched, path-escaping, or
tampered standard resources before expansion or code generation.

## Tooling and AI Usage

**OEP-0003-R052:** LSP and semantic-inspection APIs MUST distinguish Bootstrap
helpers, standard macros, ordinary standard functions, Linkable helpers, and
user or extension bindings.

**OEP-0003-R053:** Tooling MUST support definition navigation from a standard
binding to distributed `.osr` source and MUST provide localized hover,
completion, and signature help from `.osri` metadata.

**OEP-0003-R054:** Macro tooling MUST preserve both the folded standard-macro
operation and its expanded primitive view, with a complete origin chain and
source map.

**OEP-0003-R055:** AI-facing semantic data MUST expose stable binding identity,
module, phase, public signature, authored metadata, declared contracts,
verified facts, and macro origin as separate fields. Localized names MUST NOT
replace canonical identities.

**OEP-0003-R056:** Conformance tests and implementation work items SHOULD cite
the requirement identifiers from this OEP. An AI agent MUST inspect OEP status
and Open Questions before implementing a public standard-library contract.

## Initial Public API Catalog

### Notation and common behavior

**OEP-0003-R057:** The tables in this section are normative. `A`, `B`, `K`, and
`V` denote type variables; `Seq[A]` denotes a repeatable logical sequence;
`LazySeq[A]` denotes a deferred, memoized sequence; `Coll[A]` denotes a logical
collection; and `Assoc[K, V]` denotes associative data. `form...`, `body...`,
and `coll...` denote zero or more source operands. These names are contract
notation, not additional source type constructors.

Every callback MUST be invoked from left to right. Callback exceptions and
solver-visible summaries MUST propagate. A lazy operation MUST invoke a
callback only as demanded and MUST memoize realized values and exceptions. An
eager operation MUST finish its documented input consumption before returning.
Unless a row states otherwise, `none` is the empty sequence, a negative count
selects an empty prefix, and an invalid type or arity raises a deterministic
typed diagnostic when statically known or a documented runtime exception at an
`Any` boundary.

### Core macros

**OEP-0003-R058:** The importable core macros MUST provide these source
forms and capabilities:

| Bindings | Source shape | Required behavior |
| --- | --- | --- |
| `and`, `or` | `(and form...)`, `(or form...)` | Left-to-right short circuit using Clojure truthiness; zero forms return `true` and `none`, respectively; return the selected operand value. |
| `when`, `when-not`, `if-not` | `(when test body...)`, `(when-not test body...)`, `(if-not test then [else])` | Evaluate `test` once with Clojure truthiness; missing branch result is `none`. |
| `cond` | `(cond test result ... [:else result])` | Test pairs in order; return `none` when no pair matches and no `:else` exists. |
| `case` | `(case value test result ... default)` | Evaluate `value` once; literal tests are unique; a list groups constants; the final default is required. |
| `condp` | `(condp pred value test result ... :else result)` | Evaluate `pred` and `value` once, test in order, require `:else`; `test :>> handler` receives the successful predicate result. |
| `if-let`, `when-let` | `(if-let [pattern value] then [else])`, `(when-let [pattern value] body...)` | Evaluate `value` once and bind on Clojure truthiness. |
| `if-some`, `when-some` | `(if-some [pattern value] then [else])`, `(when-some [pattern value] body...)` | Evaluate `value` once and bind unless it is `none`; `false` is retained. |
| `when-first` | `(when-first [pattern coll] body...)` | Evaluate `coll` once, bind the first item when non-empty, and retain a probed lazy item. |
| `->`, `->>` | `(-> value step...)`, `(->> value step...)` | Insert the prior value as the first or last call argument. |
| `some->`, `some->>` | `(some-> value step...)`, `(some->> value step...)` | Thread as above and stop only on `none`; `false` continues. |
| `cond->`, `cond->>` | `(cond-> value test step ...)`, `(cond->> value test step ...)` | Evaluate each test in order and conditionally thread one single-evaluated accumulator. |
| `as->` | `(as-> value name form...)` | Bind each prior result to `name` for the next form. |
| `doto` | `(doto value call...)` | Evaluate `value` once, insert it first into each call, and return the original value. |
| `defn-` | `(defn- name params body...)` | Produce a non-exported `defn` carrying authored `:private true` intent. |
| `letfn` | `(letfn [(name params body...) ...] body...)` | Predeclare all local functions and support single-arity self and mutual recursion. |
| `loop`, `recur` | `(loop [pattern init ...] body...)`, `(recur value...)` | Evaluate initializers once; `recur` targets the nearest lexical loop or current function, is tail-only, arity/type checked, and constant-stack. |
| `for`, `forv`, `doseq` | `(for [clauses...] body...)`, `(forv [clauses...] body...)`, `(doseq [clauses...] body...)` | Support multiple bindings plus `:let`, `:when`, and `:while`; `for` returns a memoized LazySeq, `forv` returns an eager Vector, and `doseq` executes effects then returns `none`, all in nested order. |
| `dotimes`, `while` | `(dotimes [name count] body...)`, `(while test body...)` | Evaluate count once and iterate `0..count-1`, or reevaluate `test` each iteration; return `none`. |
| `trampoline` | `(trampoline f arg...)` | Repeatedly invoke returned zero-argument callables until a non-callable result is produced. |
| `lazy-seq`, `lazy-cat` | `(lazy-seq body...)`, `(lazy-cat coll...)` | Delay and memoize sequence production; concatenate inputs without eager realization. |
| `delay`, `force`, `deref`, `realized?` | `(delay body...)`, `(force value)`, `(deref value [timeout-ms timeout-value])`, `(realized? value)` | Provide one-time memoized evaluation and the shared dereference protocol; cache either the value or exception. |
| `binding` | `(binding [dynamic-var value ...] body...)` | Evaluate override values in order, install context-local dynamic values, and restore prior values on every exit path. |
| `with-open` | `(with-open [name resource ...] body...)` | Acquire in source order and close non-`none` resources in reverse order through nested `finally`. |
| `assert`, `throw`, `comment`, `time` | `(assert test [message])`, `(throw value)`, `(comment form...)`, `(time body...)` | Raise stable assertion/exception behavior, discard comments at Phase 1, and time one or more expressions with a monotonic clock while returning their final value. |

For `for` and `doseq`, each binding collection MUST be evaluated once. `:when`
skips one candidate; `:while` stops only the lexically nearest collection.
Destructuring MUST not change those rules.

### Core functions and predicates

**OEP-0003-R059:** The core functional and predicate APIs MUST provide:

| Bindings | Logical call and result | Required behavior |
| --- | --- | --- |
| `identity` | `(identity value) -> A` | Return `value`. |
| `constantly` | `(constantly value) -> Fn[Any..., A]` | Return a function that ignores its arguments. |
| `comp` | `(comp f...) -> Fn` | Compose right to left; zero functions produce `identity`. |
| `partial` | `(partial f arg...) -> Fn` | Capture leading arguments once and append later arguments. |
| `juxt` | `(juxt f...) -> Fn[..., Vector[Any]]` | Call every function left to right with the same arguments. |
| `complement` | `(complement pred) -> Fn[..., Bool]` | Negate Clojure truthiness of the predicate result. |
| `nil?`, `some?` | `A -> Bool` | Test only `none`; `some?` is its complement. |
| `true?`, `false?` | `A -> Bool` | Test the exact Boolean values, not general truthiness. |
| `number?` | `A -> Bool` | Recognize `Int` and `Float`, excluding `Bool`. |
| `string?`, `list?`, `vector?`, `map?`, `set?` | `A -> Bool` | Test the corresponding logical runtime type. |
| `sequence?`, `seq?`, `coll?`, `sequential?` | `A -> Bool` | Test the documented logical sequence/collection families without inspecting private Python implementations. |

Phase-1 `keyword?` and `symbol?` MUST test distinct syntax data. No Phase-0 API
may claim that distinction until Keyword and Symbol have distinct runtime
representations.

### Associative collections

**OEP-0003-R060:** `osiris.core` and `osiris.collection` MUST provide these
associative operations. Core MAY facade-refer the rows listed by R022; the
canonical definitions of the remaining rows belong to `osiris.collection`.

| Binding | Logical call | Required behavior |
| --- | --- | --- |
| `get` | `(get assoc key [not-found]) -> V\|A` | Return the value or `none`/explicit fallback without inserting. |
| `assoc` | `(assoc assoc key value ...) -> Assoc` | Return a new value with pairs applied left to right. |
| `dissoc` | `(dissoc assoc key...) -> Assoc` | Return a new value without those keys. |
| `update` | `(update assoc key f arg...) -> Assoc` | Call `f` with the current value or `none`, followed by `arg...`. |
| `get-in` | `(get-in assoc keys [not-found])` | Traverse keys; return the fallback at the first missing path. |
| `assoc-in` | `(assoc-in assoc keys value)` | Return nested associative data, creating missing map nodes; empty keys replace the root. |
| `update-in` | `(update-in assoc keys f arg...)` | Combine `get-in`, `f`, and `assoc-in` with one callback call. |
| `select-keys` | `(select-keys assoc keys) -> Map` | Preserve requested key order and omit missing keys. |
| `merge` | `(merge assoc...) -> Map` | Merge left to right; later values win. `none` is an empty map. |
| `merge-with` | `(merge-with f assoc...) -> Map` | Combine duplicate values left to right with `f`. |
| `group-by` | `(group-by f coll) -> Map[K, Vector[A]]` | Preserve input order inside each group and first-key encounter order. |
| `frequencies` | `(frequencies coll) -> Map[A, Int]` | Count values with Osiris equality. |
| `index-by` | `(index-by f coll) -> Map[K, A]` | Reject duplicate derived keys instead of silently losing data. |
| `rename-keys` | `(rename-keys assoc renames) -> Map` | Rename existing keys and reject collisions. |
| `update-keys`, `update-vals` | `(update-keys f assoc)`, `(update-vals f assoc)` | Transform in deterministic iteration order; key collisions are errors. |
| `zipmap` | `(zipmap keys values) -> Map` | Pair in order and stop at the shorter input. |
| `invert` | `(invert assoc) -> Map[V, K]` | Swap keys and values and reject duplicate output keys. |

These functions MUST NOT mutate their inputs. Equality and key behavior MUST
follow logical Osiris values, including keeping `false` distinct from numeric
zero.

### Sequences and reductions

**OEP-0003-R061:** The initial sequence producer and transform contracts MUST
be:

| Bindings | Accepted shapes | Result and behavior |
| --- | --- | --- |
| `range` | `(range end)`, `(range start end [step])` | Lazy numeric sequence; end-exclusive; `step` cannot be zero. |
| `repeat` | `(repeat value)`, `(repeat count value)` | Infinite or finite lazy repetition; value is evaluated once. |
| `repeatedly` | `(repeatedly f)`, `(repeatedly count f)` | Invoke zero-argument `f` once per realized item. |
| `iterate` | `(iterate f initial)` | Infinite lazy sequence of `initial`, `f(initial)`, and so on. |
| `cycle` | `(cycle coll)` | Repeat a finite input; an empty input stays empty. |
| `sequence` | `(sequence coll)` | Return a repeatable lazy view; do not eagerly materialize. |
| `cons`, `concat` | `(cons value coll)`, `(concat coll...)` | Lazy prefix and concatenation in source order. |
| `map`, `mapcat` | `(map f coll...)`, `(mapcat f coll...)` | Lazy transform; multi-input forms stop at the shortest input; `mapcat` flattens one returned sequence level. |
| `mapv`, `mapcatv` | same shapes | Eager Vector variants with the same callback order. |
| `filter`, `remove` | `(filter pred coll)`, `(remove pred coll)` | Lazy selection using Clojure truthiness. |
| `filterv`, `removev` | same shapes | Eager Vector variants. |
| `keep`, `keep-indexed` | `(keep f coll)`, `(keep-indexed f coll)` | Lazy transform dropping only `none`; indexed callback receives zero-based `Int`. |
| `map-indexed` | `(map-indexed f coll)` | Lazy transform whose callback receives index then item. |
| `take`, `drop` | `(take n coll)`, `(drop n coll)` | Lazy prefix or suffix; consume at most the needed prefix. |
| `take-while`, `drop-while` | `(take-while pred coll)`, `(drop-while pred coll)` | Lazy boundary selected with Clojure truthiness. |
| `take-last`, `drop-last` | `(take-last n coll)`, `(drop-last [n] coll)` | `take-last` must exhaust finite input; `drop-last` uses bounded lookahead and defaults to one. |
| `partition` | `(partition n coll)`, `(partition n step coll)`, `(partition n step pad coll)` | Lazy Vector windows; omit an incomplete tail unless `pad` supplies it. |
| `partition-all` | `(partition-all n coll)`, `(partition-all n step coll)` | Lazy Vector windows including the incomplete tail. |
| `partition-by` | `(partition-by f coll)` | Lazy runs of adjacent items with equal keys; call `f` once per item. |
| `interleave`, `interpose` | `(interleave coll...)`, `(interpose separator coll)` | Lazy ordered interleaving (shortest input) or separator insertion. |
| `distinct`, `dedupe` | `(distinct coll)`, `(dedupe coll)` | Keep first global occurrence or remove adjacent duplicates, preserving order. |
| `flatten` | `(flatten value)` | Recursively flatten sequential values only; strings, maps, and sets are leaves. |

Partition sizes and steps MUST be positive integers. Finite-count APIs MUST
reject `Bool`, fractional numbers, and strings rather than coercing them.

**OEP-0003-R062:** Initial sequence consumers and reductions MUST provide:

| Binding | Accepted shapes | Required behavior |
| --- | --- | --- |
| `first`, `rest`, `next` | `(first coll)`, `(rest coll)`, `(next coll)` | Return first-or-`none`, an empty-capable remainder, or `none` for no remainder. |
| `nth` | `(nth coll index [not-found])` | Zero-based lookup; without fallback, out-of-range raises `IndexError`; with fallback, return it. |
| `count` | `(count coll) -> Int` | Eagerly count; `none` is zero. |
| `empty` | `(empty coll)` | Return an empty logical collection of the same family when defined. |
| `empty?` | `(empty? coll) -> Bool` | Probe at most one lazy item and retain it. |
| `seq` | `(seq coll) -> Option[Seq[A]]` | Return `none` for empty input and a non-empty repeatable view otherwise. |
| `reduce` | `(reduce f coll)`, `(reduce f initial coll)` | Ordered eager reduction; no-initial empty input calls zero-arity `f`. |
| `fold` | `(fold f initial coll)` | Exact explicit-initial alias contract for ordered reduction. |
| `reductions` | `(reductions f coll)`, `(reductions f initial coll)` | Lazy sequence of intermediate accumulator values. |
| `reduced`, `reduced?`, `unreduced` | marker construction, test, and one-layer unwrap | Stop only the nearest reduction and expose final `T`, not `Reduced[T]`. |
| `run!` | `(run! f coll) -> None` | Call for effects in order and return `none`. |
| `doall`, `dorun` | `(doall [n] coll)`, `(dorun [n] coll)` | Realize all or a prefix; return the original collection or `none`. |
| `some` | `(some pred coll)` | Return the first truthy predicate result, otherwise `none`. |
| `every?`, `not-every?`, `not-any?` | `(predicate pred coll) -> Bool` | Short-circuit with Clojure truthiness. |

### String and scalar math

**OEP-0003-R063:** `osiris.string` MUST initially expose:

| Binding | Logical call | Required behavior |
| --- | --- | --- |
| `trim`, `trim-left`, `trim-right` | `(binding text) -> Str` | Remove Unicode whitespace from both, left, or right edges. |
| `split` | `(split text separator [limit]) -> Vector[Str]` | Split on a literal non-empty `Str`; positive `limit` bounds the result count; preserve empty fields. |
| `split-lines` | `(split-lines text [keep-ends?]) -> Vector[Str]` | Recognize Unicode/Python line boundaries and optionally retain endings. |
| `join` | `(join separator strings) -> Str` | Join `Str` items in input order; reject non-strings. |
| `replace` | `(replace text old new) -> Str` | Replace all literal occurrences; no regex interpretation. |
| `starts-with?`, `ends-with?`, `includes?` | `(binding text fragment) -> Bool` | Perform exact code-point substring tests. |
| `lower`, `upper`, `capitalize` | `(binding text) -> Str` | Apply locale-independent Unicode case conversion. |
| `blank?` | `(blank? text) -> Bool` | True for empty text or text containing only Unicode whitespace. |

Case conversion and line boundaries MUST use the target Python version's
Unicode database and MUST record that target in `.osri`/build data. Regex and
locale-sensitive collation are outside this initial API.

**OEP-0003-R064:** `osiris.math` MUST expose scalar constants `pi`, `e`, `tau`,
`inf`, and `nan`; one-argument scalar transforms `abs`, `floor`, `ceil`,
`trunc`, `sqrt`, `exp`, `log10`, `sin`, `cos`, `tan`, `asin`, `acos`, and
`atan`; `(round value [digits])`, `(log value [base])`, `(pow base exponent)`,
and `(atan2 y x)`; and one-argument predicates `finite?`, `infinite?`, and
`nan?`. Numeric promotion, domain errors, NaN behavior, and overflow MUST
follow the versioned scalar numeric contract and target Python, not framework
array coercion. Array dispatch requires an extension-owned static
operator/function instance.

### Concurrency and explicit Python boundaries

**OEP-0003-R065:** `osiris.concurrent` MUST expose these capabilities:

| Bindings | Source shape | Required behavior |
| --- | --- | --- |
| `future`, `future-call` | `(future body...)`, `(future-call f)` | Submit one zero-argument task and return `Future[A]`; propagate captured dynamic binding context. |
| `future-done?`, `future-cancelled?`, `future-cancel` | one Future operand | Query or request cancellation without hiding failure. |
| `pmap` | `(pmap f coll...) -> Vector[B]` | Submit eagerly, stop at shortest input, preserve result order, and propagate the first observed dereference exception; do not implicitly cancel submitted siblings. |
| `pvalues`, `pcalls` | `(pvalues form...)`, `(pcalls f...)` | Evaluate concurrently and return an ordered eager Vector; zero forms return `[]`. |
| `promise`, `deliver` | `(promise)`, `(deliver promise value)` | The first delivery wins and returns the same Promise; later deliveries leave that value unchanged and also return the Promise. |
| `deref` | `(deref value [timeout-ms timeout-value])` | Read Delay, Future, or Promise; timeout is milliseconds; absent timeout waits. |
| `lock`, `locking` | `(lock)`, `(locking lock body...)` | Create a reentrant lock and always release it on exit. |

Concurrency operations MUST carry thread, blocking, timeout, cancellation, and
unknown callback effects in typed HIR. Each consuming output MUST link one
private executor implementation into `__osiris_runtime__`; the module MUST NOT
promise fairness, task start order, or cancellation of work that has already
begun.

**OEP-0003-R066:** `osiris.python` MUST expose only these explicit dynamic
boundaries:

| Binding | Logical call | Result |
| --- | --- | --- |
| `get-attr`, `get-attr-or`, `has-attr?` | `(get-attr object name)`, `(get-attr-or object name fallback)`, `(has-attr? object name)` | `Any`, fallback-or-`Any`, or `Bool`. Attribute names are `Str`. |
| `set-attr!`, `del-attr!` | `(set-attr! object name value)`, `(del-attr! object name)` | Mutate explicitly and return `none`. |
| `get-item`, `set-item!`, `del-item!` | corresponding object/key forms | Read as `Any`, or mutate and return `none`. |
| `call` | `(call callable args [kwargs]) -> Any` | `args` is `Seq[Any]`; `kwargs` is `Map[Str, Any]`; preserve Python argument and exception behavior. |
| `iter` | `(iter object) -> Sequence[Any]` | Adapt an iterable into the logical one-pass dynamic boundary. |
| `type-name` | `(type-name object) -> Str` | Return a diagnostic display name, never a binding identity. |

Reads and calls return explicit `Any` unless a typed `extern` or extension
facade proves a more precise result; mutation and calls carry unknown effects.
These functions MUST NOT import a module by string, execute code text, inspect
compiler state, or run during Phase 1. Static Python imports use `py/import`;
reusable typed boundaries use `extern`.

### API publication

**OEP-0003-R067:** Every binding named in R021, R022, and R057 through R066
MUST have a command-queryable API record containing its canonical binding ID,
owning namespace, macro/function/value kind, source call shapes, generic `.osri`
signature, evaluation strategy (`macro`, `eager`, `lazy`, or `consumer`),
effects, exception behavior, `since`, deprecation state, authored default
documentation, tagged translations, and source location. `osr lsc hover` and
`osr lsc signature` MUST return these records from the installed matching
interface without importing Python.

**OEP-0003-R068:** A release MUST NOT export an initial standard binding as
conforming when its behavior is only a placeholder, untyped `Any` shim, or
name-only entry. Missing APIs MAY be absent in a pre-stable staged release, but
its capability manifest and documentation MUST report them as unavailable; this
OEP cannot advance to Final until the complete catalog conforms.

### Distribution-private linking

**OEP-0003-R069:** After macro expansion, name resolution, and typed HIR
validation, the linker MUST collect every referenced standard binding,
intrinsic, and Linkable helper by stable identity and compute its transitive
runtime dependency closure. Public standard functions in that closure MUST be
compiled from the packaged `.osr` source; Kernel leaves MUST lower directly or
link their compiler-owned target helpers. Each selected public binding MUST
retain its authored standard-source item as the linking root; selecting one
binding MUST NOT implicitly select every public item in its namespace. It MUST
generate support only for that reachable closure. Filesystem order,
unqualified spelling, and Python import side effects MUST NOT affect selection.

**OEP-0003-R070:** When an output needs separately generated support, its
reserved package name MUST be exactly `__osiris_runtime__`. A publishable
distribution MUST place it inside each owning generated Python package root:

```text
<python-package>/__osiris_runtime__/
  __init__.py
  sequence.py
  concurrency.py
  dynamic.py
```

Only modules required by the reachable closure MUST be emitted. Compiled
standard modules SHOULD be placed below `__osiris_runtime__/stdlib/`; Kernel
helpers remain private siblings selected by the linker. The compiler MAY lower
a helper directly into its owning output module when no shared helper state or
reuse is required, but MUST NOT choose another support-package name.

**OEP-0003-R071:** Generated user modules and `__osiris_runtime__` MUST NOT
import `osiris`, `osiris.prelude`, `osiris-lang`, the compiler executable,
`.osri`, macro IR, or the documentation database. They MAY import the Python
standard library and the project's ordinary declared Python dependencies. A
successful Python build MUST remain executable after all Osiris build tooling
and source-only compiler artifacts are removed.

**OEP-0003-R072:** Kernel control flow and simple standard operations SHOULD
lower directly to readable Python. Stateful or reusable facilities such as
LazySeq, Reduced, Delay, Future, Promise, dynamic binding, and locks MAY emit
private support modules. Generated support MUST contain ordinary Python only;
it MUST NOT perform runtime interface discovery, macro expansion, semantic
analysis, package scanning, or ABI negotiation.

**OEP-0003-R073:** `__osiris_runtime__` is a compiler-reserved source and output
path. A user module or package member that would occupy that path MUST produce a
diagnostic before writing output. One distribution MUST NOT import another
distribution's generated support package, deduplicate helpers across installed
distributions, or expose support names as public `.osri` bindings.

**OEP-0003-R074:** The linker MUST generate one deterministic private support
manifest containing the Python target, standard-library semantic hash, reachable
binding IDs, helper content hashes, and source-map identities. The manifest is
build provenance and cache input, not a runtime compatibility contract. Normal
Python import or execution MUST NOT read it.

**OEP-0003-R075:** Standard-library source spans and macro origins MUST remain
mapped through linked support into `.py.map` and local tooling results. This
provenance MAY identify Osiris build inputs, but the generated Python execution
contract MUST remain independent of Osiris tooling.

## Rejected Alternatives

### Implement the entire core as macros

Macros are not ordinary first-class runtime values. Making `map`, `filter`, or
`reduce` macro-only would prevent dynamic selection and higher-order use, expand
code unnecessarily, and blur compile-time and runtime semantics.

### Put all standard behavior in the Phase-1 evaluator

The evaluator operates on syntax data under a deterministic sandbox. Runtime
collections, Python objects, IO, concurrency, and inferred types do not belong
there. Expanding the evaluator into a second runtime would increase the trusted
compiler surface and break phase separation.

### Keep a shared `osiris.prelude` runtime

A shared package would make generated Python depend on the compiler
distribution, require runtime ABI negotiation, and let separately compiled
extensions conflict over one installed helper version. Distribution-private
linked support avoids those problems.

### Put DataFrame frameworks in the standard library

Framework release cadence, null semantics, schemas, axes, and execution models
are not stable language primitives. PyPI extensions preserve compiler
neutrality and allow independent evolution.

### Publish a separately versioned standard-library distribution

Before ABI stabilization, independently versioning the standard source package
creates unsupported compiler/library combinations and complicates build
isolation. The source package therefore ships inside the matching
`osiris-lang` release even though it follows the ordinary OEP-0002 layout.

### Infer standard semantics from function names

Name-based solver behavior breaks aliases, imports, extension replacement, and
stable identities. Signatures, binding IDs, protocols, and intrinsic contracts
are the correct semantic interfaces.

## Open Questions

None.

## Conformance

An Osiris release conforms to this OEP when all of the following are true:

- every MUST or MUST NOT requirement has mapped conformance evidence;
- all required namespaces and core exports resolve to stable `.osri`
  identities;
- standard Phase-1 dependencies are acyclic and sandbox restrictions are tested;
- standard macro expansion is deterministic and preserves origins;
- public ordinary functions pass type, effect, temporal, and data-summary tests;
- lazy/eager and early-termination behavior matches this specification;
- the native compiler contains only the minimal Kernel standard surface and the
  `osiris-lang` wheel contains the complete ordinary standard source package;
- every public standard API record, signature, macro, and implementation maps
  back to packaged `.osr`, and generated caches validate against that source;
- Kernel declarations contain no localized API documentation, while every
  public Kernel wrapper has authored default and `zh-CN` documentation;
- conformance project builds contain readable generated Python, source maps,
  and their distribution-private linked support;
- reachable support is generated only under `__osiris_runtime__`, imports no
  Osiris runtime package, and remains executable without build tooling;
- two clean standard-library builds satisfy artifact determinism requirements;
- LSP and semantic inspection pass bilingual metadata and source-navigation tests;
- framework-specific behavior remains outside the Rust Kernel and standard modules;
- the implementing release identifies itself and links evidence to OEP-0003.

This OEP cannot become Final while any required initial namespace is missing.

## Change History

- Revision 9, 2026-07-24: Required every public standard module to isolate its
  typed target boundary in `<namespace>.kernel`, and specified parsed
  `:osiris/facade-modules`/`:osiris/facade-macros` composition for a split
  `osiris.core` without changing public identities.
- Revision 8, 2026-07-24: Required hierarchical implementation namespaces,
  `:osiris/internal true`, and Clojure-style `defn-`/`:private` semantics;
  prohibited underscore-based namespace privacy.
- Revision 7, 2026-07-24: Required public Kernel facades to be authored Osiris
  `defn`/`def` implementations over private minimal-metadata Kernel leaf
  declarations, and fixed public source items as the standard linker roots.
- Revision 6, 2026-07-24: Split the implementation into a minimal embedded
  Kernel and a self-bootstrapped, source-distributed standard package; made
  public `.osr` declarations normative and required documented public wrappers
  around undocumented Kernel operations.
- Revision 5, 2026-07-23: Added the public nominal protocol types required by
  reduction, delayed evaluation, and concurrency signatures.
- Revision 4, 2026-07-23: Replaced implicit core auto-referral with the explicit
  `:refer :all`/`:exclude`/`:rename` import contract and fixed the initial
  standard-library ABI at `1`.
- Revision 3, 2026-07-23: Replaced the shared `osiris.prelude` runtime with
  reachability-based linking into each output's private `__osiris_runtime__`
  package, and made `for` lazy with an explicit eager `forv` form.
- Revision 2, 2026-07-23: Defined the initial macro/function API catalog with
  source shapes, evaluation, return, collection, sequence, string, math,
  concurrency, and explicit Python-boundary contracts.
- Revision 1, 2026-07-23: Initial draft.
