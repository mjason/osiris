---
oep: 0
title: OEP Purpose and Process
description: Governance, document structure, lifecycle, and translation rules for Osiris Enhancement Proposals.
author: MJ
status: Draft
type: Process
areas:
  - Governance
created: 2026-07-23
updated: 2026-07-23
revision: 5
requires: []
replaces: []
superseded-by: null
resolution: null
translations:
  zh: local/zh/0000-oep-process.md
---

# OEP-0000: OEP Purpose and Process

## Abstract

This proposal defines the Osiris Enhancement Proposal process. OEPs record
design decisions before implementation, expose stable requirements to human
reviewers and AI agents, and connect accepted specifications to later code,
tests, documentation, and compatibility decisions.

English OEPs are the normative source. Synchronized translations provide
localized review without creating multiple competing specifications.

## Motivation

Osiris spans a language, compiler, macro evaluator, type and effect solvers,
Python code generation, package interfaces, an LSP, and extension contracts.
Informal discussion is not sufficient to keep these surfaces coherent.

A proposal system is needed to:

- separate requirements from implementation accidents;
- review breaking changes before code commits make them expensive;
- preserve the rationale and rejected alternatives behind decisions;
- give AI agents stable identifiers and machine-readable metadata;
- make conformance tests traceable to normative requirements;
- maintain English and Chinese views without silent translation drift.

## Scope

An OEP is required for a change that does one or more of the following:

- changes language syntax or observable evaluation semantics;
- adds or changes a public standard-library contract;
- changes `.osri`, macro IR, runtime ABI, package marker, or artifact formats;
- changes type, effect, temporal, data-property, or trust semantics;
- changes project configuration or dependency-resolution contracts;
- introduces or changes a public LSP, Agent, or semantic-inspection protocol;
- creates a compatibility commitment that affects extension authors or users.

Routine bug fixes that restore already specified behavior, internal refactors,
tests, editorial documentation changes, and release automation do not require
a new OEP.

## Terminology

- **Author**: the person or group responsible for writing and revising an OEP.
- **Editor**: a maintainer who checks process, metadata, scope, and status.
- **Reviewer**: a participant who evaluates the technical proposal.
- **Normative text**: requirements that define conformance.
- **Non-normative text**: rationale, examples, history, and implementation notes.
- **Source OEP**: the normative English document in `oeps/`.
- **Translation**: a localized, non-normative rendering under `oeps/local/`.
- **Revision**: the monotonically increasing integer identifying source text
  synchronization, not the OEP lifecycle status.

## Specification

### Proposal identity and storage

**OEP-0000-R001:** The project MUST use the name **Osiris Enhancement Proposal**
and the abbreviation **OEP** for documents governed by this process.

**OEP-0000-R002:** Normative source documents MUST live directly under
`oeps/` and use the filename form `NNNN-short-title.md`, where `NNNN` is a
zero-padded, permanent proposal number.

**OEP-0000-R003:** A number MUST NOT be reused after a proposal is published,
including after rejection, withdrawal, or supersession. Proposal numbers do
not imply priority or acceptance.

**OEP-0000-R004:** The next proposal number MUST be allocated from the next
unused integer. Renaming a proposal MUST preserve its number.

### Proposal types

**OEP-0000-R005:** Every OEP MUST declare exactly one of these types:

- **Standards Track**: observable language, standard-library, ABI, artifact,
  configuration, extension, or tooling contracts;
- **Process**: governance, release, review, or project-wide development rules;
- **Informational**: guidance or architecture records that do not define
  conformance requirements.

### Lifecycle

**OEP-0000-R006:** Every OEP MUST use exactly one of these statuses:

- **Draft**: actively written and not ready for a decision;
- **Review**: complete enough for a decision and frozen except for review edits;
- **Accepted**: approved as the contract that implementation may target;
- **Final**: accepted and shown by conformance evidence to be implemented;
- **Active**: an accepted Process OEP that remains continuously applicable;
- **Deferred**: valid work intentionally postponed;
- **Rejected**: reviewed and declined;
- **Withdrawn**: withdrawn by its author before acceptance;
- **Superseded**: replaced by another accepted OEP.

**OEP-0000-R007:** Normal Standards Track progression MUST be:

```text
Draft -> Review -> Accepted -> Final
```

Deferred proposals may return to Draft. Accepted proposals may return to Draft
only through an explicit recorded decision explaining the compatibility risk.

**OEP-0000-R008:** Implementation MUST NOT be treated as the source of truth
for a Draft or Review proposal. Experimental code may inform review, but only
Accepted text defines the target behavior.

**OEP-0000-R009:** Accepted means implementation is authorized, not complete.
Final requires links to conformance evidence and a released implementation.

**OEP-0000-R010:** A Process OEP that has been accepted and is continuously
applicable SHOULD use Active rather than Final.

### Front matter

**OEP-0000-R011:** Every source OEP MUST begin with YAML front matter containing
these fields in this order:

```yaml
oep: <integer>
title: <English title>
description: <one-line English description>
author: <name or names>
status: <lifecycle status>
type: <proposal type>
areas:
  - <area>
created: <YYYY-MM-DD>
updated: <YYYY-MM-DD>
revision: <positive integer>
requires: [<OEP numbers>]
replaces: [<OEP numbers>]
superseded-by: <OEP number or null>
resolution: <decision URL or null>
translations:
  <language>: <relative path>
```

**OEP-0000-R012:** Dates MUST use ISO 8601 calendar form. OEP references in
metadata MUST use integer proposal numbers. Empty relationships MUST use an
empty array, and an absent single relationship MUST use `null`.

**OEP-0000-R013:** `revision` MUST increase whenever the English source changes
after its first review publication, including editorial changes. A revision
MUST NOT decrease or be reused.

**OEP-0000-R014:** Unknown metadata fields MUST use an `x-` prefix until this
process OEP standardizes them. Consumers MUST preserve unknown `x-` fields and
MUST NOT infer normative meaning from them.

### Required document structure

**OEP-0000-R015:** Standards Track OEPs MUST use these top-level sections in
this order:

1. Abstract
2. Motivation
3. Scope
4. Terminology
5. Specification
6. Rationale
7. Backwards Compatibility
8. Security and Determinism
9. Tooling and AI Usage
10. Rejected Alternatives
11. Open Questions
12. Conformance
13. Change History

Process and Informational OEPs MAY omit sections that are inapplicable, but
MUST retain Abstract, Motivation, Specification, and Change History.

**OEP-0000-R016:** Normative requirements in a Standards Track or Process OEP
MUST have stable identifiers in the form `OEP-NNNN-RMMM`. Identifiers MUST NOT
be renumbered after Review begins. Deleted requirements remain reserved.

**OEP-0000-R017:** Requirement statements MUST use the key words MUST, MUST
NOT, SHOULD, SHOULD NOT, or MAY with the meanings defined by RFC 2119 and RFC
8174 when, and only when, they appear in uppercase.

**OEP-0000-R018:** Examples MUST be labeled normative only when exact syntax or
output is part of the contract. Otherwise examples are explanatory and MUST
NOT override requirement text.

**OEP-0000-R019:** Open questions MUST be resolved or explicitly deferred
before a proposal moves to Accepted. An Accepted OEP MUST NOT make conformance
depend on an unresolved question.

### Specification versus implementation

**OEP-0000-R020:** Normative text MUST define observable behavior, invariants,
interfaces, failure behavior, compatibility boundaries, and conformance. It
MUST NOT require an internal file layout, algorithm, Rust type, or Python helper
unless that detail is itself a public compatibility contract.

**OEP-0000-R021:** Implementation discussion MUST be placed in a clearly marked
non-normative section or a separate implementation document. Implementation
notes MUST NOT silently add requirements.

**OEP-0000-R022:** A proposal MAY define migration and acceptance criteria
before implementation, but MUST describe them in terms of observable outcomes.

### Translation policy

**OEP-0000-R023:** English source OEPs are normative. A translation MUST state
that it is non-normative and MUST link back to the English source.

**OEP-0000-R024:** Translations MUST live at
`oeps/local/<language>/NNNN-short-title.md`. The initial Simplified Chinese
translation directory is `oeps/local/zh/`; its metadata language value is
`zh-CN`.

**OEP-0000-R025:** A source OEP MUST be written before its translation. Draft
translations MAY be prepared in the same change after the English source has a
revision number.

**OEP-0000-R026:** A translation MUST copy the proposal number, status, type,
created date, updated date, and source revision. It MUST add:

```yaml
language: <locale>
source: <relative path to English OEP>
source-revision: <English revision translated>
translation-status: Current | Stale
```

**OEP-0000-R027:** A translation is Current only when `source-revision` equals
the English `revision` and all normative and substantive non-normative sections
have been translated. Otherwise it MUST be marked Stale.

**OEP-0000-R028:** A Standards Track proposal MUST have a Current `zh-CN`
translation before moving to Review, Accepted, or Final. This requirement can
be changed only by revising this Process OEP.

Translations for locales other than `zh-CN` MAY be provided, but they are not
required for any status transition.

**OEP-0000-R029:** Translation fixes that do not change English meaning update
the translation's `updated` date but do not change the English revision. If a
translation exposes ambiguity in the source, the English source MUST be fixed
first and its revision incremented.

### AI-readable authoring rules

**OEP-0000-R030:** Each OEP MUST be understandable from its own metadata,
terminology, requirements, and explicit dependencies. It SHOULD avoid relying
on conversation history or unstated repository knowledge.

**OEP-0000-R031:** Terms with Osiris-specific meaning MUST be defined once and
used consistently. Normative text SHOULD prefer explicit nouns over ambiguous
pronouns such as “it”, “this”, or “the system”.

**OEP-0000-R032:** Cross-document references MUST use stable OEP and requirement
identifiers when the referenced statement is normative.

**OEP-0000-R033:** Conformance evidence SHOULD map tests, diagnostics, public
symbols, and release notes back to requirement identifiers. AI-generated
implementation plans MUST cite the requirements they intend to satisfy.

**OEP-0000-R034:** OEP front matter and requirement identifiers are structured
interfaces. Automation MAY index them, but generated indexes MUST be checked
against source OEPs and MUST NOT become a second source of truth.

### Review and acceptance

**OEP-0000-R035:** Moving from Draft to Review requires complete metadata,
required sections, stable requirement identifiers, resolved internal
contradictions, and a Current Chinese translation.

**OEP-0000-R036:** Review MUST evaluate semantics, compatibility, determinism,
failure behavior, tooling impact, extension impact, and conformance criteria.
Code style or implementation convenience alone is not sufficient rationale.

**OEP-0000-R037:** Acceptance MUST record a durable resolution URL or repository
decision reference in front matter. The accepted revision MUST be immutable;
later normative changes require a revision and, when compatibility changes, a
new OEP or an explicit reopening decision.

**OEP-0000-R038:** Final status requires conformance evidence, released artifact
versions, current required translations, and no unresolved normative TODOs.

### Repository index

**OEP-0000-R039:** `oeps/README.md` MUST list every source OEP with number,
title, type, status, revision, and available translations.

**OEP-0000-R040:** The index is a navigation aid. If it conflicts with source
front matter, source front matter wins and the index MUST be corrected.

### Machine-readable manifest and documentation publication

**OEP-0000-R041:** The repository MUST provide `oeps/oeps.jsonc` as the
machine-readable documentation manifest. The manifest MUST declare its schema
version, normative source locale, known translations, OEP source paths,
complete manual source paths and stable document IDs, and documentation
publication artifact and channel policies.

**OEP-0000-R042:** The manifest MUST use JSON with comments and trailing commas
permitted. Consumers MUST parse it as data, reject duplicate object keys, and
MUST NOT execute content while loading it.

**OEP-0000-R043:** The manifest MUST NOT duplicate proposal status, revision,
title, requirements, or other authoritative front matter. Those values MUST be
read from the referenced source OEP. If a manifest path and an OEP disagree,
the source OEP wins and the manifest MUST be corrected.

**OEP-0000-R044:** A stable embedded documentation snapshot MUST contain only
complete authored English manuals and complete English source documents for
Accepted, Active, or Final OEPs in its normative `reference` collection. Draft,
Review, Deferred, Rejected, Withdrawn, and Superseded OEPs MUST be excluded.
Every manual MUST have a stable document ID independent of its repository path.

**OEP-0000-R045:** A preview or testing embedded documentation snapshot MAY publish
complete English source documents for Draft and Review OEPs only in a separate
`discussions` collection. Every such document and result MUST expose its status
and a machine-readable `normative: false` field, and its text MUST NOT override
the release's normative reference collection. Authored English manuals selected
by the manifest remain reference documents and MUST NOT be synthesized from
discussion text.

**OEP-0000-R046:** OEP-0000 MUST be excluded from every embedded documentation
snapshot, including preview discussions. Process and governance documents
remain repository-maintainer material rather than published language reference.
Repository translations MUST remain available for review but MUST NOT be
included in the binary snapshot. This exclusion applies to translations of
manuals as well as translations of OEPs; localized source and `.osri` metadata
is governed separately by OEP-0001 and is not documentation snapshot input.

**OEP-0000-R047:** A final distribution version without a prerelease or
development identifier MUST select the `stable` publication channel. A
prerelease or development distribution version MUST select the `preview`
channel. Release tooling MUST export the selected English documents into the
read-only, content-addressed libSQL/FTS5 artifact defined by OEP-0001 and embed
that artifact into the native `osr` binary. Local validation MAY build either
snapshot but MUST declare the selected channel rather than infer it from an
unversioned working tree.

## Rationale

Stable requirement identifiers let design, implementation, tests, diagnostics,
and AI-produced work plans refer to the same unit of intent. A monotonically
increasing source revision is simpler than commit hashes for detecting stale
translations and works before a proposal is committed.

English is chosen as the normative source because package ecosystems, protocol
standards, and external contributors commonly consume English specifications.
Requiring a current Chinese translation before review preserves the project's
primary review workflow without creating two normative texts.

Translations remain repository review aids rather than embedded binary content.
This lets each compiler release carry one authored, searchable English corpus
while multilingual AI clients explain it in the user's language. Localized
source APIs remain available independently through LSC and LSP.

Accepted and Final are separate because approval must precede implementation.
This prevents existing code from being treated as an accidental specification.

## Backwards Compatibility

OEP-0000 introduces a new process and has no language-runtime compatibility
impact. Existing design documents remain informative until their content is
adopted, replaced, or explicitly referenced by an accepted OEP.

## Security and Determinism

OEP documents are data and MUST NOT contain executable metadata. Tooling that
indexes YAML front matter must use a safe parser and must not instantiate
language-specific objects from tags.

External text, generated summaries, and translations cannot grant acceptance
or change normative meaning. Status changes require repository review and a
durable resolution record.

## Tooling and AI Usage

AI agents should consume OEPs in this order:

1. read source front matter and dependencies;
2. read Terminology and Specification;
3. map work to requirement identifiers;
4. use Rationale and Rejected Alternatives to avoid reopening settled choices;
5. check Open Questions and status before proposing implementation;
6. use a translation only when its source revision is Current.

An AI agent must not infer that Draft text authorizes implementation. When
asked to implement an OEP, the agent should report its status and unresolved
questions before changing public behavior.

## Rejected Alternatives

### Keep all design in one language-design document

One large document makes independent decisions difficult to review, version,
supersede, or map to tests. It remains useful as an overview but not as the only
decision record.

### Make translations normative

Two normative languages create unavoidable conflict-resolution ambiguity.
The synchronized non-normative translation model provides accessibility while
retaining one authority.

### Use filenames or headings without metadata

Prose-only documents are harder for tools and AI agents to index reliably.
YAML front matter provides a small structured contract while Markdown remains
human-readable.

### Require implementation before acceptance

That reverses the desired workflow and causes implementation details to decide
the specification. Experimental prototypes may exist, but acceptance evaluates
the contract.

## Open Questions

None.

## Conformance

The OEP repository conforms to this proposal when:

- every OEP has valid required front matter;
- numbers and requirement identifiers are unique and permanent;
- the README index matches source front matter;
- Review-or-later Standards Track OEPs have Current Chinese translations;
- translations declare and match source revisions;
- status transitions and acceptance resolutions satisfy this process.

## Change History

- Revision 5, 2026-07-23: Retained the required Current Chinese review
  translation and made every additional translation locale optional.
- Revision 4, 2026-07-23: Defined complete manuals and OEPs as inputs to an
  English-only libSQL/FTS5 snapshot embedded in each native `osr` release;
  repository manual translations remain review artifacts.
- Revision 3, 2026-07-23: Replaced CLI bundles with English-only central
  documentation publication snapshots; repository translations remain review
  artifacts.
- Revision 2, 2026-07-23: Added the JSONC manifest, version-selected stable and
  preview documentation channels, and the OEP-0000 CLI exclusion.
- Revision 1, 2026-07-23: Initial draft.
