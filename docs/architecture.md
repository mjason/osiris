# Osiris Architecture

Osiris keeps the compiler kernel deliberately smaller than the language users
see. Most Clojure-inspired syntax is ordinary Osiris code loaded during macro
expansion. Rust owns a form only when a macro cannot preserve its semantics or
when the form establishes a compiler boundary.

## Language Layers

### Kernel

The closed kernel is declared in `src/language/core_forms.rs`. A form belongs
here only when it establishes one of these boundaries:

- module, import, export, alias, or phase separation;
- a runtime binding, nominal type layout, external ABI, or static interface;
- lexical evaluation semantics that macros cannot implement without a hidden
  compiler primitive.

The authored boundary forms are `module`, `import`, `import-for-syntax`,
`py/import`, `export`, `alias`, `defmacro`, `defn-for-syntax`, and
`defstatic-schema`. Declaration macros may emit `def`, `defn`, `defstruct`,
`extern`, `static-record`, and `py/decorate`. The expression kernel is `fn`,
`let`, `if`, `do`, `try`, and `raise`.

`defstruct` remains in the kernel because it creates nominal identity, field
layout, interface ABI, and Python backend declarations. `defstatic-schema`
and `static-record` are data-only extension primitives: they let packages
publish compiler-readable static facts without teaching the parser or backend
about a domain.

### Standard Macros

`src/stdlib/macros/` contains the versioned surface language. Threading,
conditionals, comprehensions, `loop`/`recur`, resource helpers, concurrency
helpers, and sequence conveniences expand into the kernel plus typed runtime
intrinsics. Adding a surface form here must not add an AST or Python backend
branch unless it exposes a genuinely new kernel semantic.

### Extension Packages

Extensions are normal Python distributions resolved and locked by `uv`. Their
`osiris.toml` marker and `.osri` interfaces advertise macros, phase-one
helpers, types, extern contracts, schemas, and static records. Discovery is
data-only: the compiler does not import extension Python modules and an
extension cannot register a parser production or backend callback.

This keeps package management in PyPI/`uv`, keeps the compiler deterministic,
and lets domain libraries evolve independently from the language kernel.

## Source Ownership

| Directory | Responsibility |
| --- | --- |
| `src/language/` | Lossless source model, lexer/reader, forms, AST, names, types, diagnostics, and the closed kernel form table. |
| `src/compiler/` | Macro expansion, module graph construction, semantic lowering, and typed HIR. |
| `src/backend/python/` | HIR-to-Python lowering, Python AST printing, and generated source maps. It does not decide surface syntax. |
| `src/stdlib/macros/` | Surface language implemented as hygienic Osiris macros. |
| `src/extensions/` | Generic static extension mechanisms implemented by this distribution, never domain-specific packages. |
| `src/packaging/` | Projects, dependency locks, extension discovery, artifacts, `.osri`, and interface graphs. |
| `src/tooling/` | CLI, LSP, semantic inspection, and user-facing printers. |
| `src/support/` | Small domain-neutral utilities shared across ownership boundaries. |
| `src/osiris/` | Python runtime package used by generated programs. |
| `src/osiris_build/` | PEP 517 integration used by Python builds. |

Public crate module names currently remain stable through explicit `#[path]`
entries in `src/lib.rs`. The physical tree communicates ownership without
forcing downstream Rust callers to migrate during this refactor.

## Dependency Direction

The intended direction is:

```text
language -> compiler -> backend
    |           |          |
    +------ packaging -----+
                |
             tooling
```

`language` cannot depend on tooling or a backend. The Python backend consumes
typed HIR and interfaces; it cannot inspect raw reader forms to invent syntax.
Tooling composes public compiler services rather than duplicating parsing,
macro expansion, or type checking. Generic extension data may be consumed by
the compiler, packaging, and tooling, but must not depend on a business domain.

## File And Module Policy

Checked-in Rust files, including tests, should normally stay below 500 lines. A
file that grows past that point should be split by responsibility, not by
numbered fragments. Large dispatchers should delegate to semantic modules
rather than accumulating branches. Tests are grouped by behavior such as
metadata, imports, control flow, or encoding.

Exceptions require a concrete reason in the module documentation. Generated
files are not checked in as a way to avoid this rule.

Python follows the same ownership rule. Runtime sequence operations are split
by core protocol, transforms, partitions, eager operations, and consumers;
the build backend is split by API, model, project loading, requirements,
interfaces, and wheel assembly.
