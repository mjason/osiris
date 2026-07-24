# Osiris Enhancement Proposals

Osiris Enhancement Proposals (OEPs) are the design record for changes to the
Osiris language, standard library, compiler contracts, package format, and
tooling protocols. An accepted OEP defines behavior before implementation
begins.

English OEPs in this directory are normative. Translations live under
`local/<language>/` and are non-normative synchronized views. The initial
translation locale is Simplified Chinese at `local/zh/`.

## Index

| OEP | Title | Type | Status | Revision | Chinese |
| --- | --- | --- | --- | --- | --- |
| [0000](0000-oep-process.md) | OEP Purpose and Process | Process | Draft | 5 | [zh](local/zh/0000-oep-process.md) |
| [0001](0001-language-and-cli.md) | Language and CLI Documentation Foundation | Standards Track | Draft | 7 | [zh](local/zh/0001-language-and-cli.md) |
| [0002](0002-package-system.md) | Project and Package System | Standards Track | Draft | 6 | [zh](local/zh/0002-package-system.md) |
| [0003](0003-standard-library.md) | Standard Library Architecture and Initial API | Standards Track | Draft | 8 | [zh](local/zh/0003-standard-library.md) |

Use [template.md](template.md) for new English proposals. Translation rules
and lifecycle requirements are defined by OEP-0000.

[`oeps.jsonc`](oeps.jsonc) is the machine-readable document manifest. It does
not duplicate OEP status or revision: source front matter remains authoritative.
Its `stable` publication channel admits complete authored English manuals and
complete English source documents for Accepted, Active, or Final reference
text. Its `preview` channel may additionally expose English Draft and Review
documents only as explicitly non-normative discussions. Release tooling builds
the selected corpus as a read-only libSQL/FTS5 snapshot embedded in `osr`.
Repository translations remain review aids, and OEP-0000 is never embedded.

The syntax manual source is [English](../docs/syntax.md), with a repository-only
[Chinese review translation](../docs/local/zh/syntax.md).
