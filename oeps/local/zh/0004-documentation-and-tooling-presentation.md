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
updated: 2026-07-28
revision: 5
language: zh-CN
source: ../../0004-documentation-and-tooling-presentation.md
source-revision: 5
translation-status: Current
requires: [0, 1, 2, 3]
replaces: []
superseded-by: null
resolution: null
---
# OEP-0004：文档元数据与工具展示规范

本翻译不是规范性来源。[英文原文](../../0004-documentation-and-tooling-presentation.md)是唯一规范源。

## 摘要 (Abstract)

Osiris 文档是语言 interface 的一部分，不是某个编辑器追加的装饰。本 OEP 定义 authored
documentation/example metadata、LSP 与 LSC 共享的信息层级、locale fallback，以及简洁的
人类输出与无损机器输出之间的边界。

设计借鉴 Rails 的文档哲学：从具体任务开始，尽早展示可执行源码，用直白语言解释常见
路径，并把实现细节移出读者的主要视线。

## 动机 (Motivation)

只显示 `Any`、内部 binding ID 与原始 effect JSON 的 hover 虽然字段齐全，却几乎没有
使用价值。它没有回答这个名字是什么、怎么调用、为什么是动态边界，以及下一步该写什么。

Rich Metadata、`.osri`、LSP、LSC、Agent JSON 和内嵌长文档必须服从同一个 authored
contract，而不是各自维护一套展示规则。

因此本 contract 的目标是：

- 让第一屏就回答「这是什么」和「怎么用」。
- 把 example 当作可版本化、可查询的文档数据。
- 让 LSP 与 LSC 在语义上对等，同时各自尊重其媒介。
- 为 JSON 客户端保留完整语义事实，而不把它们倾倒进人类输出。
- 支持 authored 默认语言文档与 BCP 47 翻译。
- 让扩展包无需编译器专属代码即可发布文档。

## 范围 (Scope)

本 OEP 覆盖 authored documentation 与 example metadata、LSP 与 LSC 共享的信息层级、
locale 选择与 fallback、版本化的机器投影、embedded-language tooling 委派，以及随 interface
传递的文档与 `osr doc` 所服务长文档之间的关系。

它不覆盖文档数据库格式与发布通道（由 OEP-0000 规定）、`:doc` 与 `:osiris/names` 的
metadata 语法（由 OEP-0001 规定）、package validation 机制（由 OEP-0002 规定），以及
标准库自身的文档义务（由 OEP-0003 规定）。本 OEP 约束的是这些事实如何被书写、投影与委派。

不在本提案范围内：

- 本 OEP 不定义教程站点生成器。
- Example 不替代编译器测试或可执行的 package example。
- 文档 metadata 不得声称推断出的 effect、type 或 temporal 事实。
- 人类投影不暴露 semantic JSON 中的每一个字段。

## 术语 (Terminology)

- **Summary**：为某个 locale 选中的简短 authored `:doc` 文本。
- **Usage shape**：源码层面的可调用形式，例如 `(reduce function initial collection)`。
- **Example**：演示一项具体任务的 authored Osiris 源码，其后可选地以注释给出期望输出。
- **人类投影**：Markdown 形式的 LSP hover，或纯文本形式的 LSC 输出。
- **机器投影**：包含无损文档与语义记录的版本化 JSON。
- **动态边界解释**：说明某个值为何是 `Any`、以及如何提供有类型边界的指引。

## 规范 (Specification)

### Rich Metadata contract

**OEP-0004-R001：** Public callable、macro、type 与 value 必须使用 OEP-0001 的 `:doc`
contract。`:default` 表示 authored fallback content，不是语言代码；翻译 key 必须是规范
BCP 47 tag。

**OEP-0004-R002：** 文档 example 必须写成 named `~osiris` block，并通过由未加引号
Symbol 组成的 `:examples` vector 引用：

```clojure
~markdown<reduce-doc>
Eagerly reduce values in order.
</reduce-doc>

~markdown<reduce-doc-zh>
按顺序立即归约值。
</reduce-doc-zh>

~osiris<reduce-example>
(reduce + 0 [1 2 3 4])
;; => 10
</reduce-example>

^{:doc
  {:default reduce-doc
   "zh-CN" reduce-doc-zh}
  :examples [reduce-example]}
(defn reduce ...)
```

每个 reference 必须按 OEP-0001-R006E 静态解析到 same-module `~osiris` binding，其 body
必须是一段完整且经过 canonical format 的 Osiris snippet。期望值或输出应该写成 Osiris
comment，使复制后仍是有效源码。`:doc` 可以用同一机制引用 same-module `~markdown`
binding；短文档仍可直接使用 literal string。Reference 表示 resolved content，不执行
metadata，也不建立 runtime dependency edge。

**OEP-0004-R003：** 在标准库 OEP 成为 Final 前，每个 public standard-library callable
与 macro 必须至少提供一个 example。Public extension API 应为每个非平凡 callable/macro
提供 example；构造或使用不直观的 type/constant 也可以提供。

**OEP-0004-R004：** Example 必须优先展示具体常见任务，不应使用 `foo`、`bar`、`x` 等
占位名称；example 必须确定、不得依赖网络，并必须显式呈现 Python 或 effectful boundary。

**OEP-0004-R005：** Package validation 必须拒绝非 vector 的 `:examples`、非 Symbol member、
missing/cross-module reference、指向非 `osiris` language 的 reference、空 example，以及
超过 metadata resource limit 的内容。每个 resolved example 必须能作为完整 Osiris source
snippet parse，并且已经符合 canonical formatter。`:doc` reference 同样必须解析到非空
same-module `markdown` block。Package 可以进一步执行 example。

**OEP-0004-R006：** Example 属于 tooling metadata。只修改仅由 example 引用的 content
或 translated documentation，必须改变 tooling/content hash，但不得改变 binding identity、
runtime reachability 或 semantic ABI hash。Generic block 同时被 ordinary code 使用时，
继续采用其 `Str` binding 的普通 runtime hash 行为。

### 人类信息层级

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

### LSP 与 LSC 对等

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

### 机器可读 API

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

### Embedded-language tooling

**OEP-0004-R020A：** LSP semantic token、document symbol、folding、selection、diagnostic
和 formatting 必须把每个 embedded sigil 当作 mapped language region，而不是 opaque Osiris
string。Host delimiter/label 保持 Osiris token；client 支持时 body token 使用 sigil language
identifier。缺少 foreign tool 不得禁用 Osiris parsing、formatting、navigation 或 compilation。

**OEP-0004-R020B：** VS Code extension 必须把每个打开的 embedded region 暴露为 versioned
virtual document，其 stable identity 由 host URI、host document version、block identity、
language tag 和 label 生成。它必须维护 lossless 双向 position/edit mapping，并在 host
version 改变时丢弃 stale foreign result。Foreign server 不支持 virtual URI scheme 时可以
使用 `.osiris/lsp/` 下 private mirror；该 mirror 必须排除在 build/watch/package input 外、
content-addressed，并在没有 session owner 时删除。

**OEP-0004-R020C：** 打开 `~python<label>` block 或在其中请求 language feature 时，必须 lazy activate
用户配置的 Python language support，并把 virtual Python document 路由到其 language
server。Server 提供能力时，adapter 必须把 Python diagnostic、completion、hover、signature
help、definition、references、rename、semantic token 和 formatting edit 映射回 host `.osr`
region。仅执行 `osr check`、build、watch、CLI formatting，或者 workspace 未打开/没有
Python request 时，禁止启动 Python。Python language server 缺失或失败时，只降级 delegated
IDE feature。Adapter 禁止用 compiler-owned analysis 或 formatting 模拟缺失的 Python
language-server feature。

**OEP-0004-R020D：** `markdown`、`sql`、`json` 等 generic tag 必须使用同一 virtual-document
protocol。对应 language support 已安装并配置时，extension 必须 lazy activate，并委托它
声明的全部 capability，包括 completion、diagnostic、navigation、semantic token 与
formatting。Delegation 不得授予 compile-time execution、filesystem authority、reader
extension 或 runtime linkage。逃逸 embedded body、在没有 host-language edit 时改变
label/delimiter，或指向 stale document version 的 foreign edit 必须拒绝。缺失 language
service 时只能降级对应 IDE feature，Osiris extension 禁止用 ad hoc emulation 替代。

### 长文档

**OEP-0004-R021：** `osr doc` 的长文档继续使用英文 authored source，并嵌入只读 libSQL
snapshot。长文档负责 guide/concept；hover example 留在 interface metadata 中，随 source
package 与 `.osri` 一起分发。

**OEP-0004-R022：** 长文档应该使用 task-first 结构：可运行 example、解释、变化形式、
边界条件，以及精确 API identity 链接。

## 理由 (Rationale)

文档是被投影出来的，不是存两份。每个人类表面都派生自机器投影所用的同一份 API 记录
（R018），因为两份各自格式化的副本必然漂移，而只拿到其中一份的读者看不出漂移。

Example 写成 named `~osiris` block 而非字符串字面量（R002），这样它就是 Reader、
formatter 与 interface 本就理解的普通源码。字面量无法做语法检查、无法被语言的 formatter
重排，也无法在它所记录的 API 变化后验证是否仍能解析；block 可以，而 R005 正是这样要求的。
代价是书写处多一层间接，收益是一个不再能编译的 example 会成为构建失败，而不是过期的散文。

R007 的信息层级按「读者接下来要做什么」排序。身份、来源与语义摘要是最容易按需取回、
放在最前又最无用的事实，因此它们移到与之相关的操作上（R019），而不是塞进每一次 hover。

解释未知被当作要求而非礼貌（R010），因为 Python 边界上的 `Any` 不是关于该值的事实，而是
关于「程序尚未声明什么」的事实；只有后一种读法才告诉读者该写什么。

嵌入区域委派给拥有它的语言，而不是由 Osiris 近似模拟（R020C、R020D）。对外部语言服务的
近似比它缺席更糟：它产出真实工具会反驳的、语气笃定的结果，而读者无从分辨自己看到的是哪一种。

## 向后兼容 (Backwards Compatibility)

本 OEP 为 standard API query schema 增加 `examples` 字段，R017 因此把它升为
`osiris.standard-api/v2`。遵循 R017 规则的消费者——在已识别的兼容 schema 内忽略未知字段、
拒绝未知 major schema——不受此新增影响。

文档与 example 内容属于 tooling metadata。按 R006，只被 example 或翻译文档引用的内容发生
变化时，tooling/content hash 变化，而 binding identity、runtime reachability 与 semantic
ABI hash 不变，因此一次文档修改不会迫使下游重新编译。若某个 generic block 同时被普通代码
读取，它保持其 `Str` binding 的正常 runtime hash 行为，因为此时内容是程序数据而非文档。

对 example 的强制是分阶段的：R003 只在 OEP-0003 转为 Final 之前约束标准库，对扩展 API
则停留在 SHOULD。

Embedded-language 委派是增量的。R020A 与 R020C 要求外部语言服务缺失或失败时只降级被委派的
IDE 功能，Osiris 的解析、格式化、导航、编译与 `osr check` 保持不变。

## 安全与确定性 (Security and Determinism)

Example 是数据，编译器从不执行它。R004 要求 example 必须确定，不得访问网络，并且必须
披露它使用的任何 Python 或带副作用的边界，使读者仅凭 example 本身即可判断运行它是否会
离开本进程。

Example 与文档引用是静态解析的。R002 限定引用只能按 OEP-0001-R006E 解析到一个 same-module
block；它是被解析的内容，不是 metadata 求值，因此文档不能引入依赖边、不能在编译期执行、
也不能观察环境状态。R005 强制其形状，并拒绝跨模块引用与超出 metadata 资源上限的内容。

委派不授予权限。按 R020D，把嵌入区域路由给外部语言服务不授予编译期执行、文件系统权限、
Reader 扩展或运行时链接。逃出嵌入 body、在没有宿主语言编辑的情况下改动其 label/分隔符、
或指向过期文档版本的外部编辑必须被拒绝，因此外部工具无法借由自己的区域改写宿主程序。

对同一份源码快照与 locale，投影是确定的。R012 要求 LSP 与 LSC 对同一快照与 locale 投影
同样的事实，R014 要求 locale 选择不改变 type 或语义数据，因此显示设置不能改变工具的报告内容。

长文档是只读且离线的。按 R021，它由内嵌的 libSQL snapshot 提供，随编译器 release 一同分发。

## 工具与 AI 使用 (Tooling and AI Usage)

面向 Agent 的输出与面向人的输出遵循同一 contract，只是详略不同。R019 让渐进披露成为
两者的默认：hover 返回 summary、usage、example、简洁的公开类型、可选的直白行为说明与
canonical name；而 definition、references、rename 与语义操作返回各自操作所需的额外事实。
机器投影必须按操作限定范围；仅仅因为格式是 JSON，并不足以让每次响应都返回全部已知事实。

需要完整记录的 Agent 应显式索取。R013 规定 `--format json` 是同一操作的版本化机器投影，
R016 规定该记录在事实存在时必须承载哪些内容。R017 规定消费者必须如何对待 schema 演进：
在已识别的兼容 schema 内忽略未知字段，拒绝未知 major schema。

Example 随 interface 传递，而不是随文档数据库传递（R021），因此读取 `.osri` 或 standard API
记录的 Agent 看到的 example 与编辑器展示的一致，无需文档查询，也无需网络访问。

文档 metadata 始终只是 authored 主张。它不能断言推断出的 effect、type 或 temporal 事实，
Agent 也不得把它当作编译器已验证的结论呈现。来自包的 metadata 适用 OEP-0001-R023：那是
不可信数据，不是指令。

## 被拒绝方案 (Rejected Alternatives)

**把语义对象序列化进 hover。** Effect、temporal 事实、data 属性与 hash 字段齐全却无法
阅读，而且它们的存在会挤掉读者打开 hover 本来要看的 summary 与 usage。R009 把它们留在
机器投影中，只允许对安全相关的事实做直白语言的概括。

**让 LSP 与 LSC 各自格式化文档。** 同一份 authored 源码配两个渲染器，是通向两个不同答案的
最短路径。R018 要求二者派生自同一份 API 记录，R008 只允许它们在媒介上不同——LSP 用
Markdown，LSC 用干净的纯文本。

**把 example 写成 metadata 里的字符串字面量。** 更好写，也无法校验：字面量不会被当作源码
读取、不会被 canonical formatter 格式化，在它所演示的 API 变化时也不会被检查。R002 要求
named `~osiris` block，R005 要求每个解析出的 example 都能解析且已符合 formatter。

**在每次机器响应中返回全部已知事实。** 统一的最大响应规范简单、对每个消费者昂贵，而消费者
随后只能自己实现协议拒绝去做的过滤。R019 改为按操作限定投影范围。

**用编译器自有的分析去模拟缺失的外部语言服务。** 由 Osiris 写出的 Python 或 Markdown 工具
近似物，会产出真实工具会反驳的结果，而结果里没有任何东西告诉读者他拿到的是哪一种。
R020C 与 R020D 禁止这种模拟，并要求其缺席只降级被委派的功能。

**把实现身份当作标题渲染。** `osiris.core::function::reduce` 精确，却不回答读者提出的任何
问题。R011 渲染 `osiris.core/reduce`，并把实现身份保留给真正关乎身份的诊断。

## 开放问题 (Open Questions)

- 未来是否增加 `osr example API`，在隔离临时项目内执行 example？
- Extension package validation 应要求 example 可执行，还是只要求 Reader/formatter valid？

## 一致性 (Conformance)

一个符合本 OEP 的实现需提供以下证据：

- LSP/LSC golden test 覆盖 standard function、macro、local symbol、Python module、locale
  fallback 与缺少 optional field；
- 默认人类 hover 不包含序列化的 effect/temporal/data JSON；
- example 能通过 `.osri` 与 standard API JSON round-trip；
- standard example 通过 Reader 与 canonical formatter validation；
- VS Code integration test 通过 `~python<label>` virtual document 映射 Python diagnostic/edit、
  lazy start Python support、拒绝 stale/escaping edit，并在没有 Python server 时保留 compiler
  syntax/formatting；
- Markdown、SQL、JSON fixture 获得 embedded tokenization/optional delegation，且不改变其
  runtime `Str` value；
- machine JSON 保留人类输出隐藏的完整事实；
- 文档输出通过稳定、可读的 snapshot test。

## 修订历史 (Change History)

- Revision 5，2026-07-28：按 OEP-0000-R015 对 Standards Track 提案的章节顺序要求重排全文。
  二十六条 requirement 内容不变，现作为「规范」的子节；Goals 并入「动机」，Non-goals 并入
  新增的「范围」，「验收标准」成为「一致性」。「理由」「向后兼容」「安全与确定性」
  「工具与 AI 使用」「被拒绝方案」由既有条款写出，不引入任何新义务。同时修正
  OEP-0004-R004 中本意规范却写成小写的 RFC 2119 关键词，按 OEP-0000-R017 它此前不具约束力。
- Revision 4，2026-07-25：定义对 named `~osiris` example block 与 named `~markdown`
  documentation block 的 static reference，且不建立 runtime reachability。
- Revision 3，2026-07-25：定义 mapped embedded-language region、virtual document、`~python<label>`
  block 的 lazy Python language-server activation、graceful fallback 和 generic language
  sigil 的安全 delegation。
- Revision 2，2026-07-24：定义面向人类和 agent、按操作限定的渐进披露规则。
- Revision 1，2026-07-24：初始 documentation metadata 与 tooling presentation contract。
