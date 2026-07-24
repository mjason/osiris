---
oep: 4
title: Documentation Metadata and Tooling Presentation
description: Authored examples, human-readable LSP and LSC projections, localization, and machine-readable documentation contracts.
author: MJ
status: Draft
type: Standards Track
areas:
  - Language
  - Documentation
  - Tooling
  - Standard Library
  - Packaging
created: 2026-07-24
updated: 2026-07-24
revision: 1
requires: [0, 1, 2, 3]
replaces: []
superseded-by: null
resolution: null
translations:
  zh: local/zh/0004-documentation-and-tooling-presentation.md
---

# OEP-0004: Documentation Metadata and Tooling Presentation

## Abstract

Osiris documentation is part of the language interface, not decoration added
by one editor. This OEP defines authored documentation and example metadata,
the information hierarchy shared by LSP and LSC, localization fallback, and
the boundary between concise human output and lossless machine output.

The design follows the documentation philosophy associated with Rails: begin
with a concrete task, show executable source early, explain the common path in
plain language, and move implementation detail out of the reader's way.

## Motivation

A hover that prints `Any`, an internal binding ID, and raw effect JSON is
technically complete but practically empty. It does not tell a person what the
name represents, how to call it, why a boundary is dynamic, or what to write
next. The same failure harms an Agent that receives an unstructured wall of
text.

Osiris has several documentation surfaces:

- source Rich Metadata;
- standard and extension `.osri` interfaces;
- IDE hover, completion, and signature help through LSP;
- equivalent terminal operations through LSC;
- structured JSON requested by tools and Agents;
- long-form English documents in the embedded documentation database.

They need one authored contract and deliberate projections rather than
independent formatting rules.

## Goals

- Make the first screen answer “what is this?” and “how do I use it?”.
- Treat examples as versioned, queryable documentation data.
- Keep LSP and LSC semantically equivalent while respecting their media.
- Preserve complete semantic facts for JSON clients without dumping them into
  human output.
- Support authored default-language documentation and BCP 47 translations.
- Let extension packages publish documentation without compiler-specific code.

## Non-goals

- This OEP does not define a tutorial site generator.
- Examples do not replace compiler tests or executable package examples.
- Documentation metadata cannot claim inferred effects, types, or temporal
  facts.
- Human projections do not expose every field available in semantic JSON.

## Terminology

- **Summary**: the short authored `:doc` text selected for a locale.
- **Usage shape**: a source-level callable form such as
  `(reduce function initial collection)`.
- **Example**: authored Osiris source demonstrating a concrete task, optionally
  followed by expected output in comments.
- **Human projection**: Markdown LSP hover or plain-text LSC output.
- **Machine projection**: versioned JSON containing the lossless documentation
  and semantic record.
- **Dynamic boundary explanation**: guidance explaining why a value is `Any`
  and how to provide a typed boundary.

## Rich Metadata contract

**OEP-0004-R001:** Public callable, macro, type, and value documentation MUST
use the OEP-0001 `:doc` contract. `:default` is authored content, not a language
code. Translation keys MUST be canonical BCP 47 tags.

**OEP-0004-R002:** Documentation examples MUST use an `:examples` vector of
examples, where each example is a vector containing one source line per string:

```clojure
^{:doc
  {:default "Eagerly reduce values in order."
   "zh-CN" "按顺序立即归约值。"}
  :examples
  [["(reduce + 0 [1 2 3 4])"
    ";; => 10"]]}
(defn reduce ...)
```

The outer vector contains distinct examples. Each inner vector contains one
complete, canonically formatted Osiris snippet without escaped newline
characters. Lines MUST be joined with a line-feed character when projected.
Expected values or output SHOULD use Osiris comments so the snippet remains
valid source when copied with the expectation line.

**OEP-0004-R003:** Public standard-library callables and macros MUST provide at
least one example before the standard-library OEP becomes Final. Public
extension APIs SHOULD provide an example for every non-trivial callable or
macro. Types and constants MAY provide examples when construction or use is not
obvious.

**OEP-0004-R004:** Examples MUST prefer a concrete common task over placeholder
names such as `foo`, `bar`, or `x`. An example MUST be deterministic, must not
require network access, and MUST disclose any Python or effectful boundary it
uses.

**OEP-0004-R005:** Package validation MUST reject a non-vector `:examples`
value, non-vector example members, non-string line members, empty examples,
empty source lines other than intentional blank lines, and examples that exceed
metadata resource limits. Standard-library validation MUST additionally join,
parse, and format every example as Osiris source. A package MAY run examples as
a stronger test.

**OEP-0004-R006:** Examples are tooling metadata. Changing only examples or
translated documentation MUST change tooling/content hashes but MUST NOT change
binding identity or semantic ABI hashes.

## Human information hierarchy

**OEP-0004-R007:** A human projection MUST present information in this order
when available:

1. localized label and human-readable binding kind;
2. one-sentence summary;
3. source-level usage shapes;
4. one or more concrete examples;
5. concise type information;
6. canonical qualified name.

It MUST NOT lead with an internal binding ID, source URI, evaluation enum,
semantic hash, or raw JSON.

**OEP-0004-R008:** Human projections MUST use source syntax, whitespace,
headings, and code blocks appropriate to their medium. LSP MUST emit Markdown.
LSC MUST emit clean plain text without Markdown punctuation or ANSI escape
sequences unless an explicit color mode is introduced later.

**OEP-0004-R009:** Effects, temporal facts, data properties, provenance,
source locations, hashes, and binding IDs MUST remain available in the machine
projection. Human hover MAY summarize a non-empty or safety-relevant fact in
plain language, but MUST NOT serialize semantic objects inline.

**OEP-0004-R010:** Unknown information MUST be explained when the explanation
changes user action. A Python module or dynamic Python value MUST say that
attributes and calls remain `Any` unless the program declares a typed `extern`
or installs a typed extension interface. Merely printing `Type: Any` is
insufficient.

**OEP-0004-R011:** Canonical names are navigation aids, not headings. Human
output SHOULD render `osiris.core/reduce`; it SHOULD NOT render implementation
identities such as `osiris.core::function::reduce` unless a diagnostic concerns
identity itself.

## LSP and LSC equivalence

**OEP-0004-R012:** LSP hover and `osr lsc hover` MUST project the same selected
summary, usage shapes, examples, type, and canonical name for the same source
snapshot and locale. Layout syntax may differ between Markdown and plain text.

**OEP-0004-R013:** `osr lsc hover NAME` and
`osr lsc hover --at PATH:LINE:COLUMN` MUST both use the human hierarchy in
R007. `--format json` MUST return the versioned machine projection instead.

**OEP-0004-R014:** LSP uses the effective `displayLocale` from
`osiris.jsonc`, client locale, and authored fallback rules. LSC defaults to the
authored `:default` and accepts `--locale BCP47`. Locale selection MUST NOT
change type or semantic data.

**OEP-0004-R015:** Completion detail MUST be brief. Hover or signature help is
the place for examples and full usage shapes. Completion MUST NOT eagerly
construct the complete documentation catalog merely to list names.

## Machine-readable API

**OEP-0004-R016:** Standard and extension API JSON MUST carry a versioned
schema and include canonical identity, kind, usage shapes, examples, complete
documentation translations, selected locale fields, type, semantic summaries,
source provenance, and compatibility hashes when those facts exist.

**OEP-0004-R017:** Adding the `examples` field changes the standard API query
schema to `osiris.standard-api/v2`. Consumers MUST ignore unknown fields within
a recognized compatible schema and MUST reject an unknown major schema.

**OEP-0004-R018:** Human presentation MUST be derived from the same API record
used by the machine projection. LSP and LSC MUST NOT maintain independent
documentation copies.

**OEP-0004-R019:** Human and agent-facing defaults MUST follow progressive
disclosure. Hover returns only the summary, usage, examples, concise public
types, optional plain-language behavior, and canonical name. Definition,
references, rename, and semantic commands return the additional facts required
by their operation. A machine projection MUST be operation-scoped; JSON format
does not justify returning every known fact in every response.

**OEP-0004-R020:** Internal binding IDs and evaluation enums MUST NOT appear in
default hover. A useful evaluation property MAY be rendered as plain-language
behavior, such as `Consumes its input eagerly.` Source locations belong to
definition results and machine projections. Standard-library locations MUST
identify the actual distributed source module and MUST be openable through the
`osiris-stdlib:` virtual document provider.

## Long-form documentation

**OEP-0004-R021:** Long-form documents served by `osr doc` remain authored in
English and embedded in the read-only libSQL documentation snapshot. They
provide guides and concepts; hover examples remain interface metadata so they
travel with source packages and `.osri` files.

**OEP-0004-R022:** Long-form guides SHOULD follow a task-first structure:
working example, explanation, variations, boundary conditions, and links to
the exact API identities involved.

## Validation and acceptance

An implementation of this OEP is complete when:

- LSP and LSC golden tests cover standard functions, macros, local symbols,
  Python modules, locale fallback, and absent optional fields;
- no default human hover contains serialized effects, temporal, or data JSON;
- examples round-trip through `.osri` and standard API JSON;
- standard examples pass reader and canonical formatter validation;
- machine JSON retains the full facts hidden by human projections;
- documentation output is snapshot-tested for stable, readable layout.

## Open Questions

- Should a future `osr example API` command execute copied examples in an
  isolated temporary project?
- Should extension package validation require runnable examples or only
  reader/formatter validity?

## Change History

- Revision 2, 2026-07-24: Defined multi-line example vectors and progressive,
  operation-scoped disclosure for human and agent tooling.
- Revision 1, 2026-07-24: Initial documentation metadata and tooling
  presentation contract.
