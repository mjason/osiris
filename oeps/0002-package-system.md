---
oep: 2
title: Project and Package System
description: Minimal project configuration, uv and PyPI integration, build commands, extension discovery, and distributable Osiris artifacts.
author: MJ
status: Draft
type: Standards Track
areas:
  - Projects
  - Packaging
  - Extensions
  - CLI
  - Python
created: 2026-07-23
updated: 2026-07-30
revision: 20
requires: [0, 1]
replaces: []
superseded-by: null
resolution: null
translations:
  zh: local/zh/0002-package-system.md
---

# OEP-0002: Project and Package System

## Abstract

This proposal defines how an Osiris project is configured, built, watched,
initialized, and published through the Python ecosystem. `osiris.jsonc` owns a
small compiler configuration; `pyproject.toml` owns project and dependency
metadata; `uv` resolves and locks dependencies; PyPI-compatible distributions
carry extension interfaces and generated Python.

Osiris does not introduce a package manager. An extension is an ordinary Python
distribution with static Osiris marker and interface artifacts. The compiler
discovers those artifacts deterministically without importing package code.

## Motivation

Osiris programs compile to Python and are expected to use ordinary Python data
libraries. A second dependency resolver or registry would duplicate uv and
PyPI, complicate reproducibility, and make extensions harder to distribute.

At the same time, Python metadata alone does not describe Osiris modules,
macros, Rich Metadata, types, contracts, or source maps. The project needs a
minimal compilation config and a static wheel contract while leaving package
ownership where Python developers expect it.

## Scope

This proposal specifies:

- `osiris.jsonc`, source selection, exclusions, output, Python target, strict
  mode, and display locale;
- new-project and existing-uv-project initialization;
- module-to-file and output mapping;
- `check`, `build`, `compile`, `watch`, and `run` project behavior;
- `pyproject.toml`, `uv.lock`, PyPI, wheel, and sdist ownership;
- the `osiris_build` PEP 517 backend;
- static extension markers, interfaces, source, records, and source maps;
- deterministic discovery and lock validation;
- distribution of the native `osr` executable.

This proposal does not specify:

- a custom registry, installer, resolver, lockfile, or publish service;
- standard-library API semantics;
- domain-specific DataFrame, numerical, financial, or rendering packages;
- multiple simultaneous code-generation targets;
- arbitrary Python build-backend composition in the initial version.

## Terminology

- **Project root**: the directory containing `osiris.jsonc` and the associated
  `pyproject.toml`.
- **Source root**: a configured project-relative directory containing `.osr`
  modules.
- **Output directory**: the project-relative build destination.
- **Artifact set**: all outputs produced for one module graph and target.
- **Extension distribution**: a Python distribution whose wheel carries an
  Osiris marker and at least one public `.osri` interface.
- **Package**: an ordinary Python distribution created with `osr init
  --package`; it may expose one or more public Osiris modules and is then an
  extension distribution.
- **Linkable backend**: a package-owned, non-public Osiris runtime closure that
  can be selected by macro expansion and copied into a consumer artifact. It
  is not an installed shared Python runtime.
- **Runtime closure**: the transitive set of linkable backend bindings and
  modules required by one generated artifact after macro expansion.
- **Marker**: static `dist-info/osiris.toml` data listing extension artifacts.
- **Effective dependency graph**: the locked runtime dependency closure used by
  a project build.
- **Development dependency**: a dependency used to build or edit a project but
  not automatically exposed as an extension to its consumers.

## Specification

### Ownership and configuration

**OEP-0002-R001:** An Osiris project MUST use `osiris.jsonc` at its project root
as the compiler configuration file. Tools given a source path MUST search that
path and its ancestors for the nearest configuration and MUST NOT merge
multiple ancestor configurations.

**OEP-0002-R002:** `osiris.jsonc` MUST accept JSON comments and trailing commas,
MUST reject duplicate object keys, and MUST be parsed as non-executable data.
Unknown fields MUST produce a diagnostic in strict configuration validation.

**OEP-0002-R003:** The initial configuration surface MUST contain the following
fields plus the optional `exclude` field defined by OEP-0002-R005, and no other
compiler fields:

```jsonc
{
  "$schema": "https://raw.githubusercontent.com/mjason/osiris/main/schemas/osiris.schema.json",
  "source": ["src"],
  "outDir": "dist",
  "targetPython": "3.11",
  "strict": true,
  "displayLocale": "zh-CN"
}
```

`$schema` is an editor aid. `source`, `outDir`, `targetPython`, `strict`, and
`displayLocale` are compiler fields. The config MUST NOT contain `watch`,
`extensions`, `buildGroups`, `trust`, or `agent` fields in this version.

**OEP-0002-R004:** `source` MUST be a non-empty ordered set of distinct,
project-relative source directories. Absolute paths, paths escaping the project
root, output directories nested as source, and duplicate normalized paths MUST
be rejected.

**OEP-0002-R005:** An optional `exclude` field MAY contain project-relative glob
patterns. A pattern without glob syntax MUST exclude that path and descendants.
Patterns MUST be evaluated against normalized project-relative paths after
source inclusion and MUST support exclusions inside a source root. Exclusion
semantics MUST be identical for build, watch, check, format, LSC, module
discovery, and LSP.

**OEP-0002-R006:** `outDir` MUST be a project-relative directory and MUST
default to `dist`. It MUST be excluded from source discovery even when a broad
source or glob would otherwise include it.

**OEP-0002-R007:** `targetPython` MUST identify the single Python language
version for analysis and code generation and MUST default to `3.11`. One build
MUST produce artifacts for exactly one target. Interfaces, caches, source maps,
and build records MUST include the target so changing it invalidates
target-sensitive results.

**OEP-0002-R008:** The configuration shape MUST NOT encode a permanent Python
backend assumption beyond `targetPython`. A future accepted OEP MAY add another
target discriminator, but the initial compiler and invocation MUST optimize for
one Python target rather than maintain unused multi-target machinery.

**OEP-0002-R009:** `strict` MUST control whether unresolved dynamic boundaries,
incomplete public contracts, and unsupported configuration are errors according
to the language rules. It MUST NOT alter reader syntax or accepted semantics.

**OEP-0002-R010:** `displayLocale` MUST accept any well-formed BCP 47 locale tag
and MUST default to `zh-CN`. It controls localized tooling labels and
documentation only; it MUST NOT alter canonical bindings, resolution,
artifacts, or generated Python. An LSP session MUST use the standard
`InitializeParams.locale` when present and well-formed, otherwise this project
value. `osr lsc` MUST NOT inherit this field: it uses the authored default unless
its request supplies `--locale <bcp47>`, as defined by OEP-0001-R070. Missing
localized data MUST use RFC 4647 lookup and the authored `:default` fallback in
OEP-0001-R059. Implementations MUST NOT replace BCP 47 tags with a closed enum
or project-specific locale key.

**OEP-0002-R011:** `pyproject.toml` MUST remain the authority for distribution
name, version, Python requirements, dependencies, build backend, indexes, and
publish metadata. `osiris.jsonc` MUST NOT duplicate these fields.

### Initialization

**OEP-0002-R012:** `osr init <project>` MUST create a new uv-compatible Python
project, add `osiris.jsonc`, create a valid source root and starter `.osr`
module, and add the installed Osiris distribution as a development dependency.
It MUST refuse to overwrite an existing target directory.

**OEP-0002-R013:** `osr init --existing [directory]` MUST add Osiris to an
existing uv project without replacing existing `pyproject.toml` metadata,
dependencies, comments, lock policy, source files, or a pre-existing valid
Osiris configuration. Repeating the command MUST be idempotent.

**OEP-0002-R014:** Initialization MUST use uv's public command or file contract
to add the compiler dependency and update the lock. A failed uv operation MUST
be reported and MUST NOT leave a configuration that claims successful setup
while the dependency update is incomplete.

**OEP-0002-R015:** `osr init --package <project>` and
`osr init --existing --package [directory]` MUST configure the project to
build a Python distribution with `osiris_build`, a canonical Osiris namespace,
and at least one public Osiris module. Converting an existing project MUST
refuse an incompatible build backend rather than silently replace it. A package
is the author-facing lifecycle concept; an extension distribution is the
compiler discovery classification for a package exposing public interfaces.

### Modules and project commands

**OEP-0002-R016:** A module file's path relative to exactly one source root,
with the `.osr` suffix removed and path separators replaced by dots, MUST
determine its canonical module path. An explicit source module declaration MUST
match that path. Ambiguous ownership by multiple roots MUST be rejected.

**OEP-0002-R017:** Generated `.py`, `.osri`, and `.py.map` files MUST preserve
the module-relative path beneath `outDir`. Build tools MUST diagnose two source
modules that would map to the same canonical module or target path.

**OEP-0002-R017A:** Source-root layout spells module components the Osiris way
(OEP-0002-R016); every path beneath `outDir` and every file path inside a built
distribution spells them the Python way, by OEP-0001-R005B. A component that
compares a name from one side of this boundary against a path from the other
MUST translate before comparing and MUST NOT compare spellings literally. A
literal comparison passes for every module name the mapping leaves unchanged
and then rejects the first name it does not — `(module my-pkg.core)` compiled
and could never be packaged, because the build backend demanded the declared
name equal the translated archive path. Directory components beneath a source
root are module components and translate; a non-module file's basename is not,
and is preserved as authored so runtime resource lookup finds it under the
name the author wrote.

**OEP-0002-R018:** `osr check [path]` MUST discover the applicable project,
parse and expand the selected module graph, validate names, types, contracts,
interfaces, extension records, and target compatibility, and MUST NOT publish
build artifacts or change dependency state.

**OEP-0002-R019:** `osr build [directory]` MUST compile the complete configured
source scope after all `check` gates pass. It MUST publish one coherent artifact
set to `outDir` atomically enough that a failed build does not leave a mixture
of new and prior module artifacts presented as one build.

**OEP-0002-R020:** `osr compile <file>...` MUST remain the lower-level explicit
source command. It MAY override output directory and artifact selection, but it
MUST apply the same semantic, target, dependency, and interface gates as build.
Project configuration MUST apply when a containing project exists.

**OEP-0002-R021:** `osr watch [directory]` MUST be a command, not a configuration
field. It MUST perform the same operation as `build`, then rebuild when an
included `.osr`, configuration, lock, or selected static interface input
changes. It MUST use the same `source` and `exclude` rules as build.

**OEP-0002-R022:** Watch is a native compiler operation. It MUST NOT require or
launch Python merely to observe files, resolve Osiris source, or compile a
change. It MUST coalesce duplicate filesystem events, avoid watching `outDir`,
and obey the interruption contract in OEP-0001-R040.

**OEP-0002-R023:** `osr run <file> -- <arguments>...` MUST first apply the same
validation and Python generation contract as compile in an isolated staging
layout, then invoke the selected target Python environment with the remaining
arguments. It MUST NOT modify dependency or lock state and MUST propagate the
invoked program's exit status after successful compilation.

**OEP-0002-R024:** Every project command MUST document its configuration search,
source scope, artifact writes, Python process use, dependency use, and watch or
interruption behavior through the command definition and a complete authored
English command manual published by the Documentation service.

### Dependency management and discovery

**OEP-0002-R025:** Osiris MUST use Python requirement metadata, PyPI-compatible
indexes, and uv for dependency declaration, resolution, installation, and
locking. The project MUST NOT define an Osiris package registry, dependency
syntax, resolver, installer, lockfile, or publish protocol.

**OEP-0002-R026:** A build MUST select extensions only from the validated
effective runtime dependency graph in `uv.lock`. Development tools or unrelated
installed distributions MUST NOT become visible merely because they exist in a
site-packages directory.

**OEP-0002-R027:** Lock validation MUST use the distribution identity, version,
source descriptor, dependency edges, and integrity data that uv actually
provides for that source kind. It MUST NOT require a fabricated source hash for
editable, workspace, path, or other uv entries whose valid lock representation
does not contain one.

**OEP-0002-R028:** The compiler MUST discover an extension from static
`dist-info/osiris.toml` and referenced `.osri` resources. Discovery, check,
build, watch, LSC, and LSP MUST NOT import or execute extension Python code.

**OEP-0002-R029:** Distribution names MUST be compared using the Python package
name normalization rules. Module and binding identities MUST remain case- and
Unicode-aware according to OEP-0001 and MUST NOT be normalized as distribution
names.

**OEP-0002-R030:** If the lock graph, installed distribution metadata, marker,
interface hashes, target, or declared dependencies disagree, compilation MUST
fail with a stable diagnostic. Discovery MUST NOT select a best effort package
by filesystem order or import precedence.

### Package build, linkable backend, and artifacts

**OEP-0002-R031:** `osiris_build` MUST be a PEP 517 build backend distributable
with Osiris. An Osiris package's `[build-system]` MUST pin a compatible
`osiris-lang` build requirement and name `osiris_build` as its backend.

**OEP-0002-R032:** Building an Osiris package sdist MUST include `pyproject.toml`,
`osiris.jsonc`, all required `.osr` source, and files needed for a reproducible
wheel build. An sdist build MUST NOT depend on unlisted files outside the
project root.

**OEP-0002-R033:** An extension wheel MUST contain readable generated Python,
public `.osri` interfaces, the authored `.osr` source corresponding to those
interfaces, required `.py.map` source maps, and any public static-record
sidecars. Reachable standard support MUST be compiled into each owning Python
package's private `__osiris_runtime__` package according to OEP-0003. Runtime
semantics and dependency compilation MUST use validated `.osri`, not recompile
packaged source implicitly.

**OEP-0002-R033A:** An extension wheel MUST additionally contain a validated,
versioned linkable-backend artifact for every package-owned Osiris runtime
binding reachable through a public interface or exported macro expansion. The
artifact MUST contain the complete binding and module closure, stable binding
identities, relocatable internal module edges, source-map identities, and its
semantic interface dependency hashes. This rule applies equally to an ordinary
public Osiris function and to a private function introduced by a macro.
Authored `.osr` source is navigation and audit material only; a consumer MUST
NOT recompile it to obtain backend behavior.

**OEP-0002-R033B:** A macro interface that can emit a package-private runtime
binding MUST declare that binding's linkable backend closure in its validated
macro artifact. Expansion MUST preserve the stable binding identity. It MUST
fail before code generation when the closure is absent, incompatible, invalid,
or does not cover every emitted private runtime binding. Macro expansion MUST
NOT silently turn a private binding into a public interface binding.

**OEP-0002-R033C:** During consumer compilation, the linker MUST take the
transitive runtime closure selected by expanded HIR and release it only under
the owning output package's reserved
`__osiris_runtime__/packages/<provider-id>/` directory. `<provider-id>` MUST
be deterministic, collision-safe, and derived from the locked provider
distribution identity and validated artifact hash. All internal backend imports
MUST be relocated to that directory. Generated consumer Python MUST NOT import
the provider package's generated Osiris Python module, `osiris-lang`, source
`.osr`, `.osri`, macro IR, or documentation data at runtime.

**OEP-0002-R033D:** A package MAY provide a package-owned Python backend only as
an embedded `python` module governed by OEP-0001. Such a module is compiler
input carried by the linkable artifact, not an import from the provider's
installed Python namespace. Its private provider handle MUST NOT appear in a
public interface; only validated `extern` bindings MAY create downstream
reachability into it. It MAY import explicitly declared ordinary Python
dependencies such as `pandas`; those providers remain normal `Requires-Dist`
dependencies and are never copied into `__osiris_runtime__`. A large, native,
independently versioned, or generally reusable Python implementation SHOULD be
a separate Python distribution. Undeclared same-distribution `.py` files MUST
NOT become runtime providers implicitly.

**OEP-0002-R033E:** A package MAY contain public facade modules, private
implementation modules, Phase-1 macro helpers, and linkable backend modules.
Only explicitly exported declarations are public to ordinary downstream source.
Unpublished declarations are available to a package macro only through its
declared linkable closure and remain unavailable for consumer imports,
completion, or direct reference.

**OEP-0002-R033F:** Package source layout MUST follow normal module mapping
under an `osiris.jsonc` source root. For example, `src/acme_text/core.osr` and
`src/acme_text/backend/normalize.osr` define `acme_text.core` and
`acme_text.backend.normalize`. The build backend MUST derive linkable closures
from resolved binding reachability rather than a second backend directory,
configuration list, filename convention, or consumer source recompilation.
Generated Python shipped for inspection MUST NOT be selected as the runtime
provider of those Osiris bindings.

**OEP-0002-R033G:** The provider wheel MUST store its relocatable closure graph
at `<distribution>.dist-info/osiris/linkable-backend.hir.json`, embedded Python
modules at `<distribution>.dist-info/osiris/python-runtime/<logical-path>.py`,
and authored `.osr` files named by the graph at their canonical module paths
for navigation. `osiris.toml` MUST record `linkable_backend`,
`linkable_backend_hash`, its schema/ABI, Python target, closure-root binding
identities, embedded Python module paths and hashes, and source-map identities.
Files below `dist-info/osiris/` are compiler data, not installed import targets,
and MUST never be imported or executed during discovery or compilation. The
consumer linker reads them, selects only reachable closures, emits readable
formatted `.py` below `__osiris_runtime__/packages/<provider-id>/`, and records
the selected hashes in the consumer runtime manifest.

**OEP-0002-R033H:** Osiris runtime code MUST be selected at binding-level
transitive reachability. Embedded Python MUST be selected at module-level
transitive static-import reachability: if any declared binding provided by one
embedded module is reachable, the complete normalized module and every
provider-owned embedded module in its static import closure MUST be released.
The initial linker MUST NOT attempt Python function-level tree shaking. A
provider wheel carries its complete validated graph once; each consumer output
carries only the selected Osiris bindings and embedded Python modules.

**OEP-0002-R034:** The wheel backend MUST generate
`<distribution>.dist-info/osiris.toml`; authors MUST NOT maintain this marker by
hand. The marker MUST identify its schema version, provider distribution and
version, target Python compatibility, every interface and source path, semantic
hashes, source-map hashes, linked-support manifest and hash, records artifacts,
the linkable-backend fields from OEP-0002-R033G, and dependency identities
needed for deterministic validation.

**OEP-0002-R035:** Each `.osri` MUST contain the public module interface needed
for downstream parsing, macro expansion, name and alias resolution, types,
Rich Metadata, static records, and declared contracts. Private implementation
details MUST NOT become public only because source is included in the wheel.

**OEP-0002-R036:** Artifact serialization and marker ordering MUST be
deterministic for the same source, compiler, target, lock, and configuration.
Paths stored in published artifacts MUST be normalized and MUST NOT expose
machine-specific project-root or temporary-directory paths.

**OEP-0002-R037:** The backend MUST validate every produced artifact and hash
before assembling a wheel, and the consumer MUST validate them before use. A
partial, stale, path-escaping, duplicate, oversized, or hash-mismatched artifact
MUST cause failure rather than fallback to Python import or source execution.

**OEP-0002-R038:** Public interface dependencies and separately published Python
providers used by a linkable backend MUST be declared as ordinary runtime
requirements in `pyproject.toml` so Python wheel metadata preserves the same
dependency closure. A development-only dependency MUST NOT satisfy a published
interface reference or a linkable backend import.

**OEP-0002-R039:** Packages MUST be installed and published with ordinary
tools, for example `uv add`, `uv build`, and `uv publish`. The compiler MAY
provide diagnostics and project scaffolding but MUST NOT wrap these operations
in a second package lifecycle.

### Compiler distribution

**OEP-0002-R040:** The PyPI distribution name for this project MUST be
`osiris-lang`, while the installed executable remains `osr` and the build
backend remains `osiris_build`. Generated projects use their own Python package
names; their reserved private support package is `__osiris_runtime__`, never a
shared `osiris` runtime package.

**OEP-0002-R041:** The `osiris-lang` wheel MUST install `osr` as a native Rust
executable. Python packaging MAY deliver the executable and PEP 517 backend,
but generated output MUST NOT require this wheel at runtime, and the CLI,
watcher, and LSP MUST NOT be Python-hosted processes.

**OEP-0002-R042:** Compiler, build backend, interface format, marker format,
standard-library ABI, and Linkable-helper format compatibility MUST be recorded
so an incompatible build combination fails before macro expansion or code
generation. Once support is linked, ordinary Python execution MUST require no
Osiris runtime compatibility check. Documentation snapshot and GraphQL schema
compatibility MUST be recorded, but MUST be validated only by documentation
clients and MUST NOT gate check, build, or execution.

**OEP-0002-R043:** A wheel source map MUST reference its required packaged
`.osr` member by normalized wheel path and content hash and MUST encode source
spans against that member. It MUST NOT duplicate authored source text inside
the map. A consumer MUST validate the member hash before using mapped spans.

**OEP-0002-R044:** Editable package installation MUST use a standard PEP 660
editable wheel produced by the configured build backend. The compiler MUST NOT
discover extensions through a custom editable-directory or source-tree scan
outside the validated wheel metadata contract.

**OEP-0002-R045:** Before stable versioning, generated project and package
scaffolds MUST constrain `osiris-lang` to the compiler package's current minor
release line. The `0.3.0` release therefore generates `osiris-lang>=0.3,<0.4`.
Artifacts and markers MUST additionally record the exact language, interface,
standard-library, and helper ABI values required for compilation.

**OEP-0002-R046:** Project compilation MAY reuse one bounded, versioned cache
under `.osiris/cache/`. The cache MUST remain separate from `outDir`, MUST NOT
be published, and MUST be safe to delete at any time. A cache identity MUST
cover source bytes and module identities, semantic project and lock inputs,
the target and strict mode, compiler/language/interface ABIs, generated-Python
formatter ABI, trust-policy hash, selected artifact kinds, standard-library
identity, and every selected external interface's semantic, tooling, and
content hashes. Implementations MUST NOT use modification times as correctness
evidence. They MUST validate static dependencies before lookup, treat an
unknown, malformed, partial, oversized, path-escaping, or hash-mismatched entry
as a miss, replace cache state atomically, and never let a failed build poison
the last successful entry. A hit MAY leave an already identical `outDir`
untouched or restore the same artifact paths atomically when output is absent
or stale; cache use MUST NOT change the artifact contract.

**OEP-0002-R047:** Every generated `.py` artifact, including source-compiled
standard facades and compiler-linked private runtime modules, MUST use the one
compiler-pinned Ruff formatting profile. Formatting MUST complete before
generated-source hashes and `.py.map` positions are calculated. The formatter
MUST be embedded in the native compiler; builds MUST NOT depend on an ambient
`ruff` executable, Python process, or project-specific Ruff configuration.

**OEP-0002-R048:** Generated Python MUST remain an inspectable source target,
not merely an executable transport. Authored fallback `:doc` metadata on a
runtime declaration MUST become its Python docstring. Authored identifiers
MUST retain their canonical readable Python mapping; compiler-owned hygienic
bindings and helpers MUST use recognizable purpose-based private names rather
than escaped internal identities. The backend SHOULD collapse expression-only
control flow, terminal temporaries, and redundant imports when doing so
preserves evaluation order and module initialization. These projections MUST
NOT reconstruct standard macros as compiler syntax: macro expansion and the
standard-library/kernel boundary remain authoritative.

**OEP-0002-R049:** Compiler configuration MUST NOT contain network-provider,
credential, model, or conversation settings. The compiler MUST NOT load a
project `.env` for language tooling. The withdrawn Language Server Agent
experiment creates no package, cache, configuration, or credential contract.

**OEP-0002-R051:** The published JSON Schema for `osiris.jsonc` MUST describe
every accepted field, its default, effect, and representative values. Editor
integrations MUST use the standard JSON language service and this schema for
configuration completion, hover, and structural validation. The Osiris LSP
MUST report project-loading failures that prevent source analysis, but MUST NOT
implement a competing JSONC editor service.

**OEP-0002-R052:** LSC MUST maintain a project semantic graph at
`.osiris/cache/language-graph.sqlite3` using embedded libSQL. Nodes MUST cover
the configured project's modules and symbols plus statically discovered,
lock-reachable Osiris interface symbols actually present in analysis. Edges
MUST cover at least `imports`, `exports`, `calls`, `references`, and `alias-of`
when the compiler can establish them. Types, Rich Metadata documentation,
examples, localized names, and source provenance MUST be searchable node data.
The graph MUST NOT index ordinary Python packages or raw embedded-Python bodies;
Python capability enters the graph only through its declared Osiris interface.

The graph is a disposable tooling cache, never an artifact. Its identity MUST
include the graph schema, compiler/language versions, and deterministic compiler
facts. A changed identity MUST atomically replace graph contents. Project paths
stored in nodes, edges, LSC results, or other tooling output MUST use stable
`osiris-workspace:///` URIs rather than machine-specific absolute paths.

Cache validation MUST happen before complete workspace analysis. Its input
identity MUST cover configured, non-excluded Osiris sources, project
configuration and lock data, the selected Python target and strictness, and the
complete content of lock-reachable static interfaces. When that identity and
the graph schema match, graph-only operations MUST open libSQL directly and
MUST NOT initialize a language server. Changed inputs, a missing cache, or an
invalid cache MUST trigger automatic analysis and atomic replacement; users do
not manually refresh the normal cache lifecycle.

Validation MUST scale with workspace file count without rereading every source
on each query. The cache MUST persist a per-input manifest containing stable
identity, byte size, high-resolution modification stamp, and content hash.
Normal validation MUST reuse the content hash when the identity, size, and
stamp match, and MUST read and hash only new or changed inputs. Source metadata
inspection and hashing MAY run in bounded parallel workers. This manifest is
only a validation accelerator: content hashes remain the authoritative graph
identity, and automatic semantic analysis after a changed identity remains a
complete dependency-correct workspace analysis unless a future OEP specifies
incremental compiler interfaces and reverse-dependency invalidation.

Complete semantic analysis MUST retain dependency and phase ordering, but
implementations SHOULD schedule all ready strongly connected components in one
deterministic topological layer and use bounded parallel workers for independent
provisional-interface construction, macro expansion, and frontend analysis.

`osr lsc cache status` MUST report whether the current input identity is
`fresh`, `stale`, `missing`, or `invalid` without building the graph.
It MUST also report the total manifest input count and how many content hashes
were reused or recomputed during validation, so large-workspace behavior is
observable.

`osr lsc cache rebuild` MUST ignore any matching cache identity, perform a full
workspace analysis, and atomically replace the complete graph. It is a recovery
and diagnostic escape hatch, not a required step after source edits.

**OEP-0002-R053:** LSC MUST be the public CLI boundary for composite local
language services. It MUST provide bounded `workspace-search`, `symbol-context`,
and `source-context` operations backed by the semantic graph and applicable LSP
requests. `symbol-context` MUST use advertised capabilities for hover,
definition, signature help, references, document symbols, and workspace symbols,
and MUST represent unavailable capabilities, missing services, ambiguity,
multiple definitions, invalid URIs, and request errors explicitly rather than
guessing. Source context MUST be limited to configured, non-excluded Osiris
sources or validated packaged standard source and to a bounded enclosing form.

## Rationale

The configuration is deliberately small. `source` already defines the watch
scope, so a separate watch field would permit contradictory trees. Extensions
already exist in Python dependencies, so listing them again would create drift.
`trust` is a semantic contract concern, not a package-selection switch, and
build groups add no value while one build has one source scope and one target.

Shipping `.osr` in extension wheels enables source navigation and audit while
`.osri` remains the static compilation authority. This prevents installed
source from becoming an implicit executable plugin or causing consumer builds
to depend on the extension author's source layout.

Package-owned backends follow the same deployment direction as the standard
library: validated, relocatable Osiris closures and small explicit embedded
Python modules move from provider metadata into the consumer's private
`__osiris_runtime__`. They are never imported from the provider's installed
generated namespace. Independently managed Python implementations remain
ordinary packages and visible dependencies.

One target per invocation keeps the compiler fast and the artifact identity
clear. Recording the target and avoiding a hard-coded backend enum leaves room
for a later target proposal without implementing speculative machinery now.

## Backwards Compatibility

This proposal deliberately allows breaking changes before the first accepted
project/package contract. Deprecated pre-release fields such as `watch`,
`extensions`, `buildGroups`, and `trust` are not reserved and should be rejected
rather than silently ignored.

After acceptance, config and artifact schema evolution must be versioned.
Additive JSONC fields require defined defaults; wheel consumers must reject
unsupported major marker or interface schemas rather than guess.

## Security and Determinism

All paths from config, locks, markers, archives, and interfaces must be
normalized and contained in their declared roots before filesystem access.
Archive extraction must reject path traversal, duplicate normalized members,
links that escape the staging tree, and resource-limit violations.

Package discovery is static and lock-scoped. Neither a package's Python import
side effects nor an ambient site-packages entry can affect compilation.
Artifacts must be written through staging and validation so interruptions do
not publish a misleading mixed result.

## Tooling and AI Usage

The complete English command manual discoverable through `osr doc` must state
the side effects and Python process behavior of `init`, `build`, `compile`,
`watch`, and `run`. `osr lsc semantic --format json` should expose each
imported interface's provider, version, hashes, and lock provenance without
exposing local absolute paths.

An AI agent adding an extension must edit ordinary `pyproject.toml`
requirements and use uv. It must not invent an `extensions` config field,
hand-write `osiris.toml`, scan site-packages, or assume included `.osr` source
overrides a validated `.osri` interface.

## Rejected Alternatives

### Build a dedicated Osiris package manager

It would duplicate dependency resolution, indexes, credentials, publishing,
and lock semantics already supplied by the Python ecosystem.

### List extension packages in `osiris.jsonc`

This would let compiler selection disagree with runtime requirements and
`uv.lock`. A package becomes an extension because its locked wheel carries a
valid static marker.

### Configure a separate watch tree

Watch compiles the project, so it must observe exactly the build inputs. A
second scope creates missed rebuilds and unnecessary events.

### Execute Python entry points for discovery

Entry-point execution makes compilation dependent on arbitrary package code,
ambient environment state, and import order.

### Require one source hash shape for every uv lock entry

uv represents registry, URL, Git, workspace, editable, and path sources
differently. Validation must check the integrity fields defined for the actual
source descriptor rather than reject valid projects using an invented field.

### Support arbitrary PEP 517 backend composition initially

Backend composition introduces ambiguous ownership of generated files and
metadata. The initial extension distribution has one backend; complex native
packages can use a separate distribution until a dedicated proposal exists.

## Open Questions

None.

## Conformance

A conforming implementation provides evidence that:

- JSONC fixtures cover comments, trailing commas, duplicate keys, minimum
  config, exclusions inside source, and rejected legacy fields;
- init fixtures cover a new project, an existing uv project, idempotence,
  package setup, and rollback on uv failure;
- module mapping and atomic artifact tests cover collisions and stale outputs;
- cache fixtures prove unchanged reuse, output restoration, source/config/
  target/interface invalidation, corruption fallback, and failed-build safety;
- semantic-graph fixtures prove libSQL persistence and invalidation, multilingual
  Rich Metadata search, cross-file calls/references, ambiguity, dependency-interface
  discovery, workspace-URI redaction, and exclusion of raw Python bodies;
- generated Python and linked runtime fixtures are idempotent under the pinned
  Ruff profile, and source maps refer to the formatted line layout;
- generated-source fixtures retain fallback docstrings, readable hygienic
  names, idiomatic expression-only control flow, and nonredundant imports;
- watch tests prove build-equivalent scope, output exclusion, event coalescing,
  config/lock changes, and prompt interruption without Python;
- lock fixtures cover registry, Git, URL, workspace, editable, and path sources;
- malicious marker, interface, wheel, and path fixtures fail closed without
  importing package code;
- an sdist can build a deterministic wheel containing source, Python,
  interfaces, maps, records, and a generated marker;
- a direct public Osiris call and an exported macro that emits a private runtime
  binding each cause exactly their validated transitive backend closure to be
  linked below the consumer's `__osiris_runtime__`, with no provider-package or
  Osiris runtime import;
- a missing, stale, incompatible, or incomplete linkable closure fails before
  Python generation, and an undeclared same-distribution Python backend is
  rejected;
- an embedded `python` module is copied only when one of its declared bindings is
  reachable, brings its complete static embedded-module import closure, and
  leaves ordinary dependencies such as `pandas` outside the runtime package;
- a consumer installed with uv can import an extension interface and compile a
  dependent project solely from its validated locked artifacts.

## Change History

- Revision 20, 2026-07-30: Added OEP-0002-R017A, naming the spelling boundary
  between source-root layout (Osiris) and built-distribution paths (Python)
  and requiring translation before any comparison across it. Recorded because
  the build backend compared literally and rejected every module name the
  OEP-0001-R005A mapping changes, so `(module my-pkg.core)` compiled but could
  never be packaged.
- Revision 19, 2026-07-27: Corrected an RFC 2119 keyword in OEP-0002-R033D
  that was written in lowercase and therefore carried no obligation under
  OEP-0000-R017.
- Revision 18, 2026-07-27: Restated OEP-0002-R033E in terms of unpublished
  declarations after `defn-` was removed from the standard macro surface under
  OEP-0003-R005G; package visibility was never determined by the declaration
  form, so no package behavior changes.
- Revision 17, 2026-07-27: Withdrew the experimental Language Server Agent,
  removed provider/session configuration, and retained the semantic graph as a
  local LSC/LSP implementation detail.
- Revision 16, 2026-07-26: Added a persistent per-input manifest so normal
  semantic graph validation stats all configured inputs but reads and hashes
  only new or changed files; added observable validation counts and
  dependency-layer parallel full analysis while clarifying that invalidated
  semantic analysis is still full-workspace analysis.
- Revision 15, 2026-07-26: Required pre-analysis semantic graph validation,
  lazy language-server startup, automatic input-aware replacement, cache status,
  and an explicit full `osr lsc cache rebuild` recovery path.
- Revision 14, 2026-07-26: Added the disposable libSQL project semantic graph,
  graph-backed LSC composite queries, workspace URI privacy, and
  compiler-owned language-service evidence.
- Revision 10, 2026-07-25: Made embedded Python provider handles module-private
  implementation details and required consumer reachability to begin at their
  validated Osiris `extern` bindings.
- Revision 9, 2026-07-25: Distinguished author-facing packages from extension
  discovery, replaced `init --extension` with `init --package`, specified
  consumer-owned linkable backend closures, and allowed explicit embedded `python`
  modules with module-level reachability while external Python dependencies
  remain ordinary packages.
- Revision 8, 2026-07-25: Required generated Python to preserve fallback
  docstrings and readable names while removing safe structural boilerplate
  without reconstructing standard macros in the compiler.
- Revision 7, 2026-07-25: Defined the bounded project-local artifact cache and
  required embedded, compiler-pinned Ruff formatting before Python hashes and
  source-map generation.
- Revision 6, 2026-07-23: Required hash-validated references to packaged source
  in wheel maps, standard PEP 660 editable wheels, and current-minor pre-stable
  scaffold dependency ranges with exact ABI metadata.
- Revision 5, 2026-07-23: Required each generated Python distribution to link
  reachable standard support into its private `__osiris_runtime__` package and
  removed the deployed `osiris` runtime dependency.
- Revision 4, 2026-07-23: Defined `displayLocale` as the LSP project fallback,
  kept LSC on its authored default unless explicitly localized, and required
  standard BCP 47 locale identifiers.
- Revision 3, 2026-07-23: Adopted `osr lsc` as the Language Server Console
  entry point for local compiler tooling.
- Revision 2, 2026-07-23: Aligned project tooling with canonical formatting,
  complete English document publication, raw GraphQL access, and the local
  inspect CLI.
- Revision 1, 2026-07-23: Initial draft.
