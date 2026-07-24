# Osiris 增强提案中文翻译

本目录保存 OEP 英文规范源的简体中文翻译。英文文档位于 `oeps/`，是唯一规范性
来源；中文文档用于本地化审核和理解，不建立第二套规范。

翻译元数据中的 `source-revision` 必须等于英文源的 `revision`，且
`translation-status` 必须为 `Current`，才能视为同步完成。

## 索引

| OEP | 标题 | 状态 | 英文源 |
| --- | --- | --- | --- |
| [0000](0000-oep-process.md) | OEP 目的与流程 | Draft | [English](../../0000-oep-process.md) |
| [0001](0001-language-and-cli.md) | 语言与 CLI 文档基础 | Draft | [English](../../0001-language-and-cli.md) |
| [0002](0002-package-system.md) | 项目与包体系 | Draft | [English](../../0002-package-system.md) |
| [0003](0003-standard-library.md) | 标准库架构与初始 API | Draft | [English](../../0003-standard-library.md) |

新翻译使用 [template.md](template.md)。翻译规则由 OEP-0000 定义。

机器可读清单位于 [`../../oeps.jsonc`](../../oeps.jsonc)。清单不重复 OEP 状态或
revision，仍以英文源 front matter 为准。`stable` publication channel 收录完整 authored
English manual，以及 Accepted、Active 或 Final 的完整英文源文档；`preview` 可以额外把
English Draft/Review 放入明确标记为非规范的 discussions。Release tooling 把所选 corpus
构建为内嵌到 `osr` 的 read-only libSQL/FTS5 snapshot。Repository translation 仅用于审核，
OEP-0000 永远不进入 snapshot。

Syntax manual 的发布源是[英文原文](../../../docs/syntax.md)，仓库还保留一份
[中文审阅翻译](../../../docs/local/zh/syntax.md)。
