---
oep: 4
title: 文档元数据与工具展示规范
description: Authored example、LSP/LSC 人类可读投影、localization 与机器可读文档 contract。
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
language: zh-CN
source: ../../0004-documentation-and-tooling-presentation.md
source-revision: 1
translation-status: Current
requires: [0, 1, 2, 3]
replaces: []
superseded-by: null
resolution: null
---

# OEP-0004：文档元数据与工具展示规范

本翻译不是规范性来源。[英文原文](../../0004-documentation-and-tooling-presentation.md)是唯一规范源。

## 摘要

Osiris 文档是语言 interface 的一部分，不是某个编辑器追加的装饰。本 OEP 定义 authored
documentation/example metadata、LSP 与 LSC 共享的信息层级、locale fallback，以及简洁的
人类输出与无损机器输出之间的边界。

设计借鉴 Rails 的文档哲学：从具体任务开始，尽早展示可执行源码，用直白语言解释常见
路径，并把实现细节移出读者的主要视线。

## 动机

只显示 `Any`、内部 binding ID 与原始 effect JSON 的 hover 虽然字段齐全，却几乎没有
使用价值。它没有回答这个名字是什么、怎么调用、为什么是动态边界，以及下一步该写什么。

Rich Metadata、`.osri`、LSP、LSC、Agent JSON 和内嵌长文档必须服从同一个 authored
contract，而不是各自维护一套展示规则。

## Rich Metadata contract

**OEP-0004-R001：** Public callable、macro、type 与 value 必须使用 OEP-0001 的 `:doc`
contract。`:default` 表示 authored fallback content，不是语言代码；翻译 key 必须是规范
BCP 47 tag。

**OEP-0004-R002：** 文档 example 必须使用两层 vector：外层表示多个 example，内层以
每个字符串表示一行源码：

```clojure
^{:doc
  {:default "Eagerly reduce values in order."
   "zh-CN" "按顺序立即归约值。"}
  :examples
  [["(reduce + 0 [1 2 3 4])"
    ";; => 10"]]}
(defn reduce ...)
```

每个内层 vector 必须组成一段完整、经过 canonical format 的 Osiris source，不得在字符串
中嵌入转义换行。投影时以换行符连接各行。期望值或输出应该写成 Osiris comment，使复制
包含期望值的代码后仍是有效源码。

**OEP-0004-R003：** 在标准库 OEP 成为 Final 前，每个 public standard-library callable
与 macro 必须至少提供一个 example。Public extension API 应为每个非平凡 callable/macro
提供 example；构造或使用不直观的 type/constant 也可以提供。

**OEP-0004-R004：** Example 必须优先展示具体常见任务，不应使用 `foo`、`bar`、`x` 等
占位名称；example 必须确定、不得依赖网络，并必须显式呈现 Python 或 effectful boundary。

**OEP-0004-R005：** Package validation 必须拒绝非 vector 的 `:examples`、非 vector 的
example、非字符串的行、空 example、非有意留白的空源码行，以及超过 metadata resource
limit 的内容。标准库还必须连接各行，用 Reader 解析并用 canonical formatter 检查每个
example。Package 可以进一步执行 example。

**OEP-0004-R006：** Example 属于 tooling metadata。只修改 example 或翻译必须改变
tooling/content hash，但不得改变 binding identity 或 semantic ABI hash。

## 人类信息层级

**OEP-0004-R007：** 人类输出在字段存在时必须按以下顺序展示：

1. localized label 与人类可读 binding kind；
2. 一句话 summary；
3. source-level usage shape；
4. 一个或多个具体 example；
5. 简洁 type 信息；
6. canonical qualified name。

不得以内部 binding ID、source URI、evaluation enum、semantic hash 或原始 JSON 开头。

**OEP-0004-R008：** 人类输出必须使用适合媒介的源码语法、空白、标题和代码块。LSP
必须输出 Markdown；LSC 必须输出干净 plain text，除非未来加入显式 color mode，否则不得
输出 Markdown 标点或 ANSI escape。

**OEP-0004-R009：** Effect、temporal fact、data property、provenance、source location、
hash 与 binding ID 必须保留在机器投影中。人类 hover 可以用自然语言概括非空或安全相关
事实，但不得内联序列化 semantic object。

**OEP-0004-R010：** Unknown 信息只有在解释会改变用户行为时才应展示。Python module 或
动态 Python value 必须说明 attribute/call 在 typed `extern` 或 extension interface 证明前
保持 `Any`；只打印 `Type: Any` 不合格。

**OEP-0004-R011：** Canonical name 是导航辅助，不是标题。人类输出应该显示
`osiris.core/reduce`，不应显示 `osiris.core::function::reduce` 这样的 implementation
identity，除非 diagnostic 本身讨论 identity。

## LSP 与 LSC 对等

**OEP-0004-R012：** 对同一个 source snapshot 与 locale，LSP hover 和 `osr lsc hover`
必须投影相同的 summary、usage shape、example、type 与 canonical name；Markdown/plain text
布局可以不同。

**OEP-0004-R013：** `osr lsc hover NAME` 与
`osr lsc hover --at PATH:LINE:COLUMN` 都必须使用 R007 的人类信息层级；`--format json`
必须返回 versioned machine projection。

**OEP-0004-R014：** LSP 使用 `osiris.jsonc` 的有效 `displayLocale`、client locale 与 authored
fallback。LSC 默认使用 authored `:default`，并接受 `--locale BCP47`。Locale 选择不得改变
type 或 semantic data。

**OEP-0004-R015：** Completion detail 必须简短。Example 与完整 usage shape 属于 hover 或
signature help。Completion 不得仅为列出名称就急切构造完整文档 catalog。

## 机器可读 API

**OEP-0004-R016：** Standard/extension API JSON 必须使用 versioned schema，并在事实存在时
包含 canonical identity、kind、usage shape、example、完整翻译、locale selection、type、
semantic summary、source provenance 与 compatibility hash。

**OEP-0004-R017：** 新增 `examples` 字段后，standard API query schema 升级为
`osiris.standard-api/v2`。Consumer 必须忽略已识别兼容 schema 中的未知字段，并拒绝未知
major schema。

**OEP-0004-R018：** 人类展示必须派生自机器投影使用的同一个 API record。LSP 与 LSC
不得维护独立的文档副本。

**OEP-0004-R019：** 面向人类和 agent 的默认输出必须渐进披露。Hover 只返回 summary、
usage、example、简洁 public type、可选的自然语言行为和 canonical name。Definition、
references、rename、semantic 命令分别返回其操作所需的额外事实。机器投影也必须按操作
限定范围；使用 JSON 不代表每个响应都应返回全部已知事实。

**OEP-0004-R020：** 默认 hover 不得显示内部 binding ID 和 evaluation enum。有用的求值
属性可以转换为自然语言行为，例如“立即消费输入集合”。Source location 属于 definition
结果与机器投影。标准库位置必须指向实际发布的源码模块，并能通过 `osiris-stdlib:`
虚拟文档 provider 打开。

## 长文档

**OEP-0004-R021：** `osr doc` 的长文档继续使用英文 authored source，并嵌入只读 libSQL
snapshot。长文档负责 guide/concept；hover example 留在 interface metadata 中，随 source
package 与 `.osri` 一起分发。

**OEP-0004-R022：** 长文档应该使用 task-first 结构：可运行 example、解释、变化形式、
边界条件，以及精确 API identity 链接。

## 验收标准

- LSP/LSC golden test 覆盖 standard function、macro、local symbol、Python module、locale
  fallback 与缺少 optional field；
- 默认人类 hover 不包含序列化的 effect/temporal/data JSON；
- example 能通过 `.osri` 与 standard API JSON round-trip；
- standard example 通过 Reader 与 canonical formatter validation；
- machine JSON 保留人类输出隐藏的完整事实；
- 文档输出通过稳定、可读的 snapshot test。

## Open Questions

- 未来是否增加 `osr example API`，在隔离临时项目内执行 example？
- Extension package validation 应要求 example 可执行，还是只要求 Reader/formatter valid？

## 修订历史

- Revision 2，2026-07-24：定义多行 example vector，以及面向人类和 agent、按操作限定的
  渐进披露规则。
- Revision 1，2026-07-24：初始 documentation metadata 与 tooling presentation contract。
