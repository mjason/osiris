---
oep: 1
title: 语言与 CLI 文档基础
description: Osiris 核心语言边界、源码语义、命令定义、内嵌英文文档和本地 AI 工具契约。
author: MJ
status: Draft
type: Standards Track
areas:
  - Language
  - Compiler
  - CLI
  - Documentation
  - AI
created: 2026-07-23
updated: 2026-07-24
revision: 9
language: zh-CN
source: ../../0001-language-and-cli.md
source-revision: 9
translation-status: Current
requires: [0]
replaces: []
superseded-by: null
resolution: null
---

# OEP-0001：语言与 CLI 文档基础

本翻译不是规范性来源。[英文原文](../../0001-language-and-cli.md)是唯一规范源。

## 摘要 (Abstract)

本提案定义 Osiris 语言及其 `osr` 命令行接口的稳定基础。它固定源码与 reader
边界、模块和 binding identity、编译器 kernel、宏阶段、Rich Metadata、类型标注、
`defstruct`、Python 生成原则、诊断和语义检查。它还把 canonical source formatting
定义为核心工具契约，而不是 editor preference。

它还定义结构化 CLI 命令模型和两套有意分离的文档体系。每个原生 `osr` executable
都内嵌一份 content-addressed、read-only 的 libSQL snapshot，其中只包含完整 authored
English document。`osr syntax` 直接打印当前 release 的语法手册，`osr doc` 则在同一
snapshot 上进程内执行 GraphQL。本地 compiler query engine 通过人类可读 CLI 和 LSP
暴露源码内嵌 API 文档，不上传项目源码。

## 动机 (Motivation)

Osiris 需要保持小型，同时允许表面语言通过宏和包持续增长。因此必须明确边界：
reader 和语义不变量属于语言；threading、comprehension 等便利能力通常属于经过审核
的宏；领域行为属于扩展。

一门面向 AI 辅助开发的语言也需要可查询的事实来源。简短 `--help` 无法学习语言，且
许多 AI Agent 不支持 editor protocol。完整手册和实时代码事实具有不同 ownership/privacy
边界：手册可以随 compiler 发布，workspace binding、signature、alias 和 metadata 必须
留在本地。因此 Osiris 对前者使用 release-pinned embedded document snapshot，对后者
使用一套由 compiler 拥有、可通过 CLI 和 LSP 访问的本地 query model。

## 范围 (Scope)

本提案规定：

- 源码、reader、binding、模块、phase 和 kernel 边界；
- Rich Metadata、别名、类型标注和名义结构；
- 宏展开与 Python backend 不变量；
- canonical formatting、诊断、source map、语法检查和语义检查；
- 公开 `osr` 命令族，以及什么构成一条命令定义；
- 内嵌完整英文文档的 libSQL snapshot、FTS5 search 和进程内 GraphQL access；
- 通过 CLI 和 LSP 进行本地 source-documentation query；
- 本地化、版本、退出行为和机器可读输出；
- AI 编写和检查 Osiris 源码时必须遵循的流程。

本提案不规定完整标准宏/函数清单（属于 OEP-0003）、项目和包体系（属于
OEP-0002）、本提案没有专门规定的标准 LSP protocol 行为、特定 Rust parser type、
private libSQL table layout、Python AST 库，以及任意 Python 值的运行时 metadata wrapper。

## 术语 (Terminology)

- **Reader form**：宏展开前由固定语法识别的 form。
- **Syntax value**：Phase 1 使用的、带源码与词法 context 的不可变 datum。
- **Kernel form**：建立普通宏无法保持之语义的 compiler-owned form。
- **Surface form**：可由宏实现的用户可见语法。
- **Phase 1**：宏和编译期 helper 的确定性求值阶段。
- **Phase 0**：由 typed HIR 和生成 Python 表示的运行时求值阶段。
- **Canonical binding ID**：声明、参数、字段、类型或宏与 locale 无关的身份。
- **Authored metadata**：源码或宏提供、与编译器证明分离的不可执行不可变数据。
- **Command definition**：一条 CLI 命令的结构化公开契约。
- **Reference collection**：为 release snapshot 发布的已接受规范英文文档。
- **Discussions collection**：为 preview release snapshot 可选发布的非规范英文
  Draft/Review 文档。
- **Documentation snapshot**：为一个 compiler release/publication channel 选择的、
  不可变、content-addressed 的 libSQL database，包含完整 authored English document
  和 derived FTS5 index。
- **Embedded documentation engine**：在原生 `osr` binary 内嵌 snapshot 上运行的
  in-process read-only GraphQL query engine。
- **Local code documentation engine**：由 compiler 拥有、根据当前 workspace `.osr`
  和 dependency `.osri` 构建，并通过 CLI/LSP 暴露的 query model。

## 规范 (Specification)

### 源码与 reader 边界

**OEP-0001-R001：** Osiris 源文件必须使用 `.osr` 扩展名，并按 UTF-8 解释。可以
接受 byte-order mark，但它不得影响源码位置或 binding spelling。

**OEP-0001-R002：** Reader 必须把 list、vector、map、set、symbol、keyword、
string、number、boolean、`none`、comment、quote、syntax quote、unquote、
unquote-splicing 和 Rich Metadata prefix 识别为固定 reader form。字符串外的逗号
必须视为空白。

**OEP-0001-R003：** Reader 必须在 lossless source model 中保留原始 spelling、
trivia 和 byte span，同时暴露规范化 datum。存在确定恢复点时，可恢复 reader error
必须保留后续 form。

**OEP-0001-R004：** 用户代码和扩展包禁止注册 tokenizer rule、reader macro、
tagged literal、parser production 或 backend lowering callback。除非以后 Accepted
OEP 修改固定 reader，否则新增用户语法必须使用普通 data form 和 Phase-1 宏。

**OEP-0001-R005：** Symbol 比较和 binding lookup 必须使用 Unicode NFC canonical
spelling，诊断和 source map 则保留 authored spelling。生成 Python 的名称碰撞检查
还必须考虑目标 Python 的 identifier normalization 规则。

**OEP-0001-R006：** 每个固定 prefix 与 collection form 必须有显式、可独立测试的
grammar/recovery contract。Malformed fixed form 不得静默 fallback 成含义不同的 atom，
也不得通过 atom-token ambiguity 引入领域语法。

### 模块、名称与别名

**OEP-0001-R007：** 每个可编译 source module 必须有唯一 canonical module
identity。文件到模块的映射由 OEP-0002 规定；源码存在 `(module ...)` 声明时必须与
映射一致。

**OEP-0001-R008：** Osiris module import、Phase-1 import 和 Python runtime import
必须保持为不同操作。加载 Osiris 接口不得 import 或执行对应 Python module。

**OEP-0001-R009：** 跨模块公开访问必须由显式 export 和已解析 interface 决定。
源码顺序、文件系统枚举、display locale 和 Python import side effect 不得改变名称解析。

**OEP-0001-R010：** 每个声明、参数、字段、类型和宏必须有一个与 locale 无关的
canonical binding ID。中文或其他本地化 preferred name 与 alias 必须解析到该身份，
不能建立第二个声明。

**OEP-0001-R011：** Alias declaration 必须无歧义地标识目标、参与重复与碰撞诊断，
并在 semantic output 和生成 keyword argument 中保留 canonical target。本地化名称
只是展示数据，不得成为类型相等或 linkage 的依据。

### Kernel 与 phase 边界

**OEP-0001-R012：** Compiler-owned kernel 必须封闭且版本化。只有建立模块/phase
分离、词法求值、binding/nominal identity、静态接口数据、Python ABI、异常控制流，
或其他宏无法保持的语义边界时，一个 form 才属于 kernel。

**OEP-0001-R013：** Kernel 必须为 module/import/export 和 alias declaration、
runtime/Phase-1 binding、`fn`、`let`、`if`、`do`、结构化异常流与 raise、
`defstruct`、外部 Python binding 与 decorator、宏定义，以及 typed static
schema/record declaration 提供语义。Declaration macro 可以提供 compiler-owned
declaration 的 authored spelling。

**OEP-0001-R014：** Threading、普通条件便利形式、comprehension、递归便利形式、
资源 helper 和数据序列组合应该是标准或扩展宏/函数。除非独立 Accepted OEP 确立缺失
的 kernel semantic，否则增加这些 form 不得增加 reader syntax 或 backend branch。

**OEP-0001-R015：** Phase 1 必须接收不可变 Syntax value；在 compiler、source、
interface 与 configuration 输入相同时必须保持确定；不得 import Python module、访问
网络、修改项目或观察未声明 ambient state。

**OEP-0001-R016：** 宏必须返回通过常规 compiler pipeline 完成 resolve、expand、
type-check 和 lowering 的 Osiris syntax。宏禁止返回 Python source text、Python AST、
untyped HIR 或 backend callback。

**OEP-0001-R017：** 宏创建的 identifier 必须默认卫生。故意使用 call-site name 必须
经过显式 syntax operation，展开结果必须同时保留 definition 与 call-site origin chain。

**OEP-0001-R018：** Phase-1 package interface 必须静态声明 macro code 和编译期
dependency。宏展开只能依赖 OEP-0002 选择的 locked、validated interface；所需 macro
code 缺失或不兼容时必须 fail closed。

### Rich Metadata 与类型

**OEP-0001-R019：** Rich Metadata 必须在支持的 syntax node 上使用与 Clojure 兼容
的 `^{:key value}`、`^:flag`、`^TypeTag`、`^"tag"` 和 `^[Tag ...]` prefix。Reader
必须把它们规范化为不可变 metadata map，且不得执行内容。

**OEP-0001-R020：** Phase 1 必须为支持的不可变 syntax data 提供可读取的 `meta`、
`with-meta` 和 `vary-meta` 行为。更新 metadata 必须返回新值；除非 syntax API 显式
修改，否则必须保留 lexical context；并且不得改变普通 datum equality。

**OEP-0001-R021：** Source span、macro expansion origin、authored metadata、
declared contract、static record 和 compiler-verified fact 必须分别保存和暴露。源码
或宏 metadata 不得宣称 compiler verification。

**OEP-0001-R022：** Osiris 标准化的 metadata key 必须包含本地化文档与名称、API
lifecycle data 和 namespaced Agent information。未知 namespaced key 必须作为数据
保留，但没有 Accepted contract 时不得获得 compiler semantic。

**OEP-0001-R023：** 从包导入的 metadata 必须视为不可信数据。Renderer 必须清理
active link 或 markup；AI client 不得把 authored metadata 解释成指令、权限或已验证
semantic fact。

**OEP-0001-R024：** Runtime type annotation 必须写成 declaration 或 binding 的
Rich Metadata，包括 `^Int` 和 `^{:type (Vector Int)}`。普通 Osiris definition 不得
要求独立声明文件或 TypeScript 风格的 type-only source language。

**OEP-0001-R025：** 省略类型标注必须请求 local inference，而不是隐式 `Any`。
Exported signature 与 host boundary 必须完整；未解析 `Unknown` 不得进入已发布接口。
公开边界的 `Any` 必须由作者显式写出，并在 semantic output 中保留 provenance。

**OEP-0001-R026：** 没有 type argument 的 container name 必须表示其文档定义的
dynamic boundary，例如 `Vector[Any]`，不得表示推断出的 element type。DataFrame、
Series、Array、schema 和 domain type 必须通过普通 typed interface 提供，不能硬编码
为 reader grammar。

**OEP-0001-R027：** `defstruct` 必须定义 nominal identity、有序 typed field、
default、metadata、constructor checking 和 public interface。字段必须有显式稳定类型；
不得只根据相同 field name 推断 structure compatibility。

**OEP-0001-R028：** Type checking 必须在宏展开后、Python 生成前进行。生成的
Python annotation 是输出契约，不得作为 Osiris type solver 的实现。

### Python target、诊断与检查

**OEP-0001-R029：** 一次 compiler invocation 必须只面向一个 Python language
version。生成 Python 必须能被该 target 解析，并且应该保留可识别的 module、
declaration、name、type annotation、decorator 和 control flow，方便审核与调试。

**OEP-0001-R030：** Python backend 必须通过结构化表示降低已验证 semantic IR。
宏展开和主要 codegen 禁止通过拼接可执行 Python source fragment 实现。

**OEP-0001-R031：** 生成 Python 必须保留 Osiris evaluation order，不得重复带可观测
effect 的 expression。Target-specific rewrite 不得静默削弱类型、temporal、data 或
effect 诊断。

**OEP-0001-R032：** 诊断必须有稳定 code、severity、primary source span、human
message，以及适用时的 machine-readable related information。宏诊断必须携带
expansion trace，生成 Python 的 failure 必须能够映射回 authored source。

**OEP-0001-R033：** `osr lsc syntax` 必须暴露 lossless syntax view；
`osr lsc semantic` 必须暴露 canonical binding ID、type、metadata category、
contract、origin 和 source span。JSON inspection output 必须声明 schema identifier，
且 identity 不得依赖本地化文本。

**OEP-0001-R034：** `osr expand` 必须显示 macro-expanded Osiris syntax，并提供
single-step mode。Expansion 与 inspection output 绝不能 import 或执行生成的 Python
程序。

### CLI 命令体系

**OEP-0001-R035：** 可执行文件名必须是 `osr`。Command name、option name 和机器
可读 field name 必须使用稳定 ASCII spelling；本地化 label 可以作为展示文本。

**OEP-0001-R036：** 每条公开命令必须有唯一 command definition，且包含：

- 稳定 command ID、canonical name；
- alias 和 lifecycle status；
- summary 和 synopsis；
- positional argument 与 option，包括 type、default、multiplicity、conflict 及
  environment/configuration precedence；
- input/output contract、stdout/stderr 用法和支持格式；
- filesystem、process、network 和 dependency side effect；
- exit code 和 interruption behavior；
- 相关 OEP requirement 与 diagnostic；
- 命令带参数时至少一个有效 example。

**OEP-0001-R037：** Command definition 必须是生成简短 help、shell completion metadata
和机器可读 command introspection 的来源。这些视图不得分别维护 argument contract。
Authored command manual 可以引用这些数据，但必须保持为完整文档。

**OEP-0001-R038：** 初始 command registry 必须为 `init`、`check`、`build`、
`compile`、`watch`、`run`、`fmt`、`expand`、`lsc`、`lsp`、`syntax` 和 `doc`
保留定义。OEP-0002 定义适用命令的 project/package 行为；本提案定义共同 CLI 与
文档行为。

| Command ID | Canonical command | Contract owner |
| --- | --- | --- |
| `cli/init` | `osr init` | OEP-0002 |
| `cli/check` | `osr check` | OEP-0002 |
| `cli/build` | `osr build` | OEP-0002 |
| `cli/compile` | `osr compile` | OEP-0002 |
| `cli/watch` | `osr watch` | OEP-0002 |
| `cli/run` | `osr run` | OEP-0002 |
| `cli/fmt` | `osr fmt` | OEP-0001 |
| `cli/expand` | `osr expand` | OEP-0001 |
| `cli/lsc` | `osr lsc` | OEP-0001 |
| `cli/lsp` | `osr lsp` | OEP-0001 |
| `cli/syntax` | `osr syntax` | OEP-0001 |
| `cli/doc` | `osr doc` | OEP-0001 |

**OEP-0001-R039：** `osr --help` 与 `osr <command> --help` 必须提供简洁、离线的
usage help。它们必须显示 Accepted operand 和 option，但不得成为语法、诊断或语义
文档的唯一来源。

**OEP-0001-R040：** CLI misuse 必须以 `2` 退出；source、semantic 或所请求 build
validation failure 必须以 `1` 退出；成功必须以 `0` 退出。长运行命令收到平台 interrupt
signal 后必须立即停止、释放 watcher resource 和 child process，并报告平台惯用的中断
状态，在 POSIX 上为 `130`。成功 compile 后，`osr run` 必须按 command definition
传播所启动程序的 exit status；该状态不是 CLI misuse。当前 language version 不赋予
其他 compiler-owned process exit code 稳定的跨平台含义。

**OEP-0001-R041：** 文本命令必须把请求结果写到 stdout，把诊断或运行 failure 写到
stderr。成功的 JSON 命令必须向 stdout 输出恰好一个有效 JSON value，且不得混入
progress text。

**OEP-0001-R042：** 除非 command definition 显式声明，命令不得修改 dependency、
lock file、project configuration、source file 或 published artifact。`check`、`expand`、
`lsc`、`syntax`、`doc` 和 `lsp` 不得修改这些输入。

### 文档数据、查询与搜索

**OEP-0001-R043：** Release documentation 的完整初始 CLI grammar 必须是：

```text
osr syntax
osr syntax --format markdown|json
osr doc <graphql-document>
osr doc -
```

`osr syntax` 必须从 embedded snapshot 读取 stable document ID `language/syntax`。默认
输出和 `markdown` 输出必须是完整 authored English Markdown body，并以一个 LF 结尾。
`json` 输出必须是一个 versioned object，至少包含 `id`、`title`、`revision`、
`contentHash` 和 `markdown`。它不得接受 locale 或 source path。`osr lsc syntax <path>`
仍是另一条用于 lossless source-tree query 的命令。

`osr doc <graphql-document>` 必须接受恰好一个包含标准 GraphQL query document 的 shell
argument。`-` 必须从 stdin 读取 document，以支持长查询或生成查询。Document 可以包含
fragment，但必须选择恰好一个 query operation。`command`、`diagnostic`、`oep`、`search`
和 `ai` 不得成为 documentation subcommand。没有 document 时 `osr doc` 必须打印简洁
usage。

**OEP-0001-R044：** Release tooling 必须把选中的完整 English document 和 derived
search projection 导出为由 libSQL 构建和查询的 SQLite-format database。Database 必须
为 document title、heading 和 Markdown body 提供 FTS5 index；初始 English index 必须
使用 `unicode61` 等 Unicode-aware tokenizer。最终 database bytes 必须 read-only、
content-addressed，并作为 resource 内嵌到每个原生 `osr` binary。实现可以把 bytes
materialize 到 private memory 或经过验证的 temporary file，但不得要求 database server。

**OEP-0001-R045：** `osr doc` 必须执行所给 GraphQL document，不得重写 field 或
selection set。默认情况下，GraphQL schema/resolver 必须在进程内针对当前 executable
内嵌的精确 snapshot 运行。`documentationCapabilities` 至少必须暴露 `source`、
`snapshotId`、`contentHash` 和 `schemaVersion`；该路径的 `source` 必须报告
`embedded`。Documentation/syntax lookup 必须离线工作，且不得隐式连接 Turso 或其他
remote service。

**OEP-0001-R046：** Embedded documentation snapshot 只能包含明确选中发布的完整
authored English document。不得包含 source/`.osri` Rich Metadata、binding signature、
localized alias、workspace symbol、生成的逐 command/diagnostic record 或 package API
record。文档可以按 title、heading 和 Markdown body 建索引，也可以用内部 heading
chunk 排名，但每个 chunk 必须保留 parent document 与 anchor。

**OEP-0001-R047：** 公开 GraphQL schema 必须提供下列 read-only root operation，以及
等价 typed field、pagination 和 bounded result size：

```graphql
type Query {
  document(id: ID!): Document
  searchDocuments(input: DocumentSearchInput!): DocumentConnection!
  documentationCapabilities: DocumentationCapabilities!
  completeDocumentQuery(input: DocumentCompletionInput!): [DocumentCompletion!]!
}

input DocumentSearchInput {
  query: String!
  first: Int = 10
  after: String
  includeDiscussions: Boolean = false
}

input DocumentCompletionInput {
  prefix: String!
  limit: Int = 20
}

type DocumentationCapabilities {
  source: String!
  snapshotId: ID!
  contentHash: String!
  schemaVersion: String!
  schemaHash: String!
}
```

Schema 必须定义 typed `Document`、`DocumentChunk`、connection、completion、provenance
和 capability object，不得返回 opaque JSON blob。`Document` 除 identity、title、collection、
normative state、revision、content hash 和 provenance 外，还必须暴露完整 authored Markdown。
Embedded read-only surface 必须允许 GraphQL schema introspection。

**OEP-0001-R048：** `documentationCapabilities` 必须让 human tool/AI 发现 query
source、snapshot ID/content hash、GraphQL schema version/hash、compiler/language version、
collection、indexed document count、search/completion feature、支持 limit 和 source
provenance。`DocumentCompletion` 只能包含 document ID、title、matching heading、snapshot
provenance 等 document query aid，不得充当 source-code completion。

**OEP-0001-R049：** Published source document 必须保持为完整 English Markdown
document。LibSQL snapshot 可以存储 derived heading chunk 和 FTS5 row，但这些 row 是
search projection，不是 authored document。Query 必须返回 authored document 或其中有
anchor 的 excerpt，不得合成 normative prose、translation 或 API record。相同 snapshot
和 GraphQL input 的 search ordering/tie-breaking 必须确定。

**OEP-0001-R050：** 完成的 `osr doc` invocation 必须向 stdout 写入恰好一个标准
GraphQL JSON response object，包括 `data` 和存在时的 GraphQL `errors`，不得增加
CLI-specific envelope 或 text renderer。Embedded bytes 无效、snapshot hash mismatch 或
本地 query engine 初始化失败必须写入 stderr，不得生成误导性的成功 response。Stdout
不得混入 progress output。

**OEP-0001-R051：** 每份 published document 必须来自明确选中的 English Markdown
source。LibSQL database、FTS5 index 和 GraphQL schema 是 derived publication/query
view，不是 normative source。每个 document/search chunk 必须保留 stable source identity、
revision、content hash、collection、normative state 和 publication provenance。Turso 可以
作为 publication source 或可选 distribution system，但不得成为 embedded documentation
path 的 runtime dependency。

**OEP-0001-R052：** Snapshot publication 必须遵守 `oeps/oeps.jsonc` 及
OEP-0000-R044 到 OEP-0000-R047。Stable snapshot 只能包含 English reference collection；
preview snapshot 可以在独立 `discussions` collection 中增加带 `normative: false` 的 English
Draft/Review document。Repository translation 不得放入 embedded snapshot。OEP-0000
永远不得通过它发布。

**OEP-0001-R053：** Embedded documentation query 必须 read-only，并受 depth、
complexity、timeout、pagination、response-size limit 约束，且 schema introspection 必须
安全。`osr doc` 和 `osr syntax` 不得访问网络或上传 project source、semantic document、
dependency graph、source metadata 或 credential。修正 embedded content 必须产生新的
content hash，并随新的 compiler release 发布；installed snapshot bytes 永远不得原地更新。
Documentation failure 不得影响 `check`、`build`、compilation、local inspection、LSP
semantic 或 generated Python。

### AI 工作流

**OEP-0001-R054：** 在创建或修改 `.osr` 前，声称符合 Osiris 的 AI Agent 必须：

1. 使用 `osr syntax` 加载完整、version-pinned 的 English syntax manual；
2. 使用 `osr doc <graphql-document>` 检查 `documentationCapabilities`，或者针对陌生、
   version-sensitive behavior 使用 `document`/`searchDocuments`；
3. 把 embedded English document 视为 release documentation；workspace/dependency API
   使用本地 tooling engine；
4. 需要 binding、signature、completion、navigation、metadata 或 contract 时，使用与所需
   editor capability 对应的本地 `osr lsc` operation；
5. 宏行为影响修改时使用 `osr expand` 或 `osr lsc expand`；
6. 对受影响 project/source scope 运行 `osr fmt` 和 `osr check`；
7. 针对不熟悉的 diagnostic 修改代码前，搜索英文 diagnostic manual；
8. 保留 canonical binding ID，把本地化名称视为 presentation/resolvable source metadata，
   而不是独立 declaration；
9. 实现 OEP 描述的公开行为前验证 OEP status。

**OEP-0001-R055：** AI 与 LSP client 必须尽可能用 document version、source span 和
canonical binding ID 标识 edit。不得通过替换本地化 alias 字符串执行 project-wide
semantic rename。

**OEP-0001-R056：** AI client 必须区分 authored metadata、static record、
dependency-declared fact 和 compiler-verified fact。不得把 authored claim 或 discussion
OEP 文本描述为 compiler proof 或 Accepted language behavior。

### 文档与本地化名称 metadata

**OEP-0001-R057：** `:doc` 必须是非空 default `Str`，或 OEP-0001-R058 定义的
locale map。Exported declaration 必须提供 `:doc`；private declaration 应该提供。
Declaration docstring 如果作为 surface sugar 得到支持，必须填充 default string 形式。
为了 AI 和生态互操作，可复用 package 应该用英文编写 default，但 compiler 必须允许
作者选择其他 default language。

**OEP-0001-R058：** 本地化文档必须使用一个紧凑的 `:doc` map，其结构是从 canonical
BCP 47 locale string 以及必需 Keyword `:default` 到非空 `Str`。`:default` 是作者选择的
language-neutral fallback slot，禁止把它解释为 language tag。Locale key 经 BCP 47
canonicalization 后必须唯一。即使值与 `:default` 相同，作者也可以增加 `"en"`、
`"zh-CN"` 或其他 tagged entry。本 contract 不把 translation 拆到第二个 metadata key。

**OEP-0001-R059：** Documentation consumer 请求 locale 时，必须从 normalized `:doc`
map 按 RFC 4647 lookup fallback sequence 选择 entry；没有 tagged entry 匹配时再回退到
`:default`。Plain `Str` 必须 normalize 为 `{:default <value>}`，不得推断其 language。
选中 `:default` 时，consumer 必须标记它来自 default fallback，不得伪造 resolved BCP 47
tag。Locale selection 不得改变 binding identity、overload selection 或 semantic。

**OEP-0001-R060：** `:osiris/names` 必须是从 canonical BCP 47 locale string 到以下
结构 map 的映射：

```clojure
{"zh-CN" {:preferred 时序均值
          :aliases [滚动均值]}
 "ja"    {:preferred 移動平均
          :aliases [ローリング平均]}
 "fr"    {:preferred moyenne-mobile
          :aliases [moyenne-glissante]}}
```

`:preferred` 必须是一个 Symbol。可选 `:aliases` 必须是 Symbol Vector。Locale key 经
BCP 47 canonicalization 后必须唯一；一个 declaration 的完整 name table 中，名称经过
NFC normalize 后必须唯一。Locale entry 中的未知 key 必须诊断。

**OEP-0001-R061：** Canonical declaration spelling 是默认 binding label 和 identity。
Localized preferred name 与所有 localized alias 必须在每种 display locale 下解析到同一个
canonical binding。可复用 public library 应使用稳定 ASCII English canonical spelling；
不这样做时，应该在 `:osiris/names` 中提供 `en` entry 用于英文展示。

**OEP-0001-R062：** `:osiris/names` 可以附着到 module、declaration、type、macro、
parameter 和 `defstruct` field。Parameter/field alias 必须限制在所属 signature/structure，
并降低为 canonical Python keyword/attribute；不得定义全局 keyword translation table。

**OEP-0001-R063：** `.osri`、semantic query、LSP 与 local CLI query 必须保留 default
documentation、所有 tagged translation、canonical name、localized preferred name、alias
与 provenance。JSON documentation entry 必须至少暴露以下逻辑结构：

```json
{
  "documentation": {
    "default": "Return the mean over the most recent window.",
    "translations": {"zh-CN": "返回最近窗口的均值。"},
    "selection": {
      "requestedLocale": "zh-CN",
      "resolvedLocale": "zh-CN",
      "text": "返回最近窗口的均值。"
    }
  },
  "names": {
    "canonical": "rolling-mean",
    "localized": {
      "zh-CN": {"preferred": "时序均值", "aliases": ["滚动均值"]}
    }
  }
}
```

**OEP-0001-R064：** 本地化 public API 的 canonical metadata spelling 必须遵循以下形式；
类型标注继续使用名称上的 Rich Metadata，不使用独立 signature syntax：

```clojure
(extern python "data_runtime.series"
  ^{:doc {:default "Return the mean over the most recent window."
          "zh-CN" "返回最近窗口的均值。"}
    :osiris/names
    {"zh-CN" {:preferred 时序均值
               :aliases [滚动均值]}}}
  (defn ^Series rolling-mean
    [^Series values
     ^{:type Int
       :osiris/names {"zh-CN" {:preferred 周期}}}
     window
     [^{:type Int
        :osiris/names {"zh-CN" {:preferred 最小样本}}}
      min-samples = window]]))
```

OEP translation 仍由 OEP-0000 管理，不存储在 API Rich Metadata 中。本 requirement
定义附着到语言和 package binding 的文档。

**OEP-0001-R065：** Locale metadata 必须向所有合法 BCP 47 tag 开放。Compiler、
`.osri` consumer、LSP 或 documentation tool 不得仅因自身 UI 没有翻译成某种语言就拒绝
或丢弃该 locale。只增加 `:doc` translation 是 tooling-data change；增加 localized
preferred name 或 alias 会改变可解析 source-name surface，因此必须影响 semantic hash，
但不得改变 canonical binding identity。

### 本地工具查询与 CLI 对等

**OEP-0001-R066：** Compiler 必须拥有一套本地 tooling query engine，输入是当前
workspace `.osr` source、client 提供的未保存 overlay 和 validated dependency `.osri`
interface。CLI/LSP 必须调用该 engine，不得分别重新实现 name resolution、metadata
selection、navigation、completion、signature、diagnostic 或 edit。

**OEP-0001-R067：** 本地 CLI query surface 必须使用以下 command family：

```text
osr lsc <operation> ... [--locale <bcp47>] [--format text|json]
```

`lsc` 表示 Language Server Console。它必须是普通、有限的 CLI invocation，不是交互式
REPL，也不是 JSON-RPC pass-through。初始形式必须包括：

```text
osr lsc diagnostics [<path>]
osr lsc hover <api-name-or-binding-id>
osr lsc hover --at <uri>:<line>:<column>
osr lsc completion --at <uri>:<line>:<column>
osr lsc signature --at <uri>:<line>:<column>
osr lsc definition --at <uri>:<line>:<column>
osr lsc references --at <uri>:<line>:<column>
osr lsc rename --at <uri>:<line>:<column> --to <name>
osr lsc expand <path>
osr lsc syntax <path>
osr lsc semantic <path>
osr lsc symbol <name-or-binding-id>
```

初始 operation 必须包含 `diagnostics`、`hover`、`completion`、`signature`、`definition`、
`references`、`rename`、`expand`、`syntax`、`semantic` 和 `symbol`。基于位置的 operation
必须接受 source URI/path 和 line/column position。`hover`/`symbol` 必须接受 canonical
binding ID、canonical name、localized name 或 alias；`symbol` 还可以接受 workspace search
text。名称有歧义时必须返回 candidate binding ID，不得按 filesystem/import order 选择。
`rename` 必须返回经过验证的 edit 且不得应用；应用 edit 需要另一份显式 mutating command
contract。

**OEP-0001-R068：** 每个返回 diagnostic、semantic fact、navigation、completion、
signature、documentation 或 proposed edit 的 compiler-owned LSP capability，都必须有
observable information 等价的 CLI operation。Semantic query/rename preview 归入
`osr lsc`，canonical formatting 归入 `osr fmt`。任何 capability 都不得成为 LSP-only。
LSP session lifecycle、incremental synchronization、cancellation 和 wire framing 属于
transport behavior，不需要伪造 CLI 等价物。

**OEP-0001-R069：** `osr lsc` 必须默认输出简洁、human-readable text。
`--format json` 必须返回 versioned stable compiler tooling object，不得包含 LSP/JSON-RPC
envelope。Text/JSON 必须投影自同一 query result；diagnostic、canonical binding ID、source
location、provenance 和 locale resolution 不得互相矛盾，也不得与 LSP 矛盾。

**OEP-0001-R070：** 所有 tooling boundary 上的 locale identifier 都必须是 well-formed
IETF BCP 47 language tag，按 registered subtag canonicalize，并依 OEP-0001-R059 使用
RFC 4647 lookup。实现不得定义 closed locale enum、private locale spelling 或自创 fallback
chain。`zh-CN`、`ja` 和 `en` 只是示例，不是完整支持清单。

`osr lsc` request 不带 `--locale` 时必须选择 `:doc` `:default` 和 canonical display name，
且不得继承 `osiris.jsonc` 的 `displayLocale`。推荐 authored default 是英文，但 LSC 必须
保留源码作者选择的中文、日文或其他 default。显式 `--locale <bcp47>` 必须通过 RFC 4647
选择该语言的 authored documentation/name，最后回退到 `:default` 和 canonical name。
要求英文的 AI 应传 `--locale en`，并检查结果是匹配 `en` 还是回退到了 authored default。
LSP session 必须优先使用标准 `InitializeParams.locale`（如果存在且 well-formed），然后
使用 project `displayLocale`，最后使用 OEP-0002 的 configuration default。因此 AI 可以
请求英文 LSC 输出，而人的 IDE 可以从同一 compiler fact 展示中文、日文或其他 locale。

Text output 必须突出 selected documentation/preferred label。JSON output 必须标明
requested/resolved language tag，保留 canonical binding ID、available-language list 和完整
authored `:doc`/`:osiris/names` map，除非 caller 显式请求 summary projection。回退到
`:default` 时，`resolvedLocale` 必须缺失或为 null，不得伪造 language tag。

**OEP-0001-R071：** 本地 `symbol`、`hover`、`completion` 和 `signature` result 在适用时
必须暴露 canonical binding ID、declaration kind、signature/type、documentation、localized
name、source span、module、visibility、package/workspace provenance，以及每个事实属于
authored、interface-declared 还是 compiler-verified。不得越过 `.osri` interface 推断
dependency private implementation data。

**OEP-0001-R072：** Local query/LSP semantic 必须在 embedded documentation engine
不可用或没有 network 时工作。其 source fact 必须来自当前 `.osr` source、未保存 overlay
和 validated `.osri` interface，包括这些输入实际包含的任意 BCP 47 localization entry。
Compiler 不得上传 source、overlay、interface、semantic record、query 或 result。
`osr lsc semantic --format json` 可以保留为 bulk fallback，但不得成为 LSP-visible fact
的唯一 CLI route。

### Canonical formatting

**OEP-0001-R073：** Osiris 必须为 `.osr` source 定义唯一 canonical formatter 和
formatting version。Formatting 不得依赖 editor、locale、operating system、user preference
或 project style configuration。除保留的 literal content 外，相同 source/formatting version
必须产生 byte-for-byte 相同的 UTF-8 output。Structural line ending 必须使用 LF，document
必须以恰好一个 LF 结尾。

Canonical layout 以 [Clojure Style Guide](https://guide.clojure.style/) 的 source layout
惯例为基线。该指南只说明设计来源，不是会随上游变化的动态规范依赖；下面的规则构成
完整 Osiris contract，两门语言存在差异时以本 OEP 为准。

- 建议最大行宽必须是 80 个 Unicode scalar value。单个 string、symbol、keyword、number
  或 comment 可以超过此限制；formatter 不得为了行宽改变 literal 或 identifier content。
- 缩进必须使用 space，禁止 tab。带 body parameter 的 form 必须从 opening parenthesis
  向内缩进两个 space。Closing delimiter 必须收拢在最后一个 content line，禁止单独占行。
- Function/macro call 无法放入一行时，应尽量把第一个 argument 留在 callee 同一行，后续
  argument 必须与第一个 argument 纵向对齐。如果 callee 同行没有 argument，argument
  必须从 opening parenthesis 后一个 space 的位置开始。
- Binding pair 必须与 opening bracket 后的第一个 binding 对齐。Map key 必须从 opening
  brace 后一个 space的位置对齐。Sequential collection 每行必须保留尽可能多的完整
  element，continuation element 从 opening delimiter 后一个 space 的位置对齐。
- `defn`、`defmacro` 和 `defn-for-syntax` 的 declaration header 能放入一行时，name 和
  parameter vector 必须保持同一行；implementation body 必须从下一行按 body indentation
  开始。没有 body 的 extern declaration 可以保持一行。
- `if` branch 和 body form 必须使用 body indentation。`cond`、`case`、`condp`、`cond->`
  和 `cond->>` clause 必须排列为对齐的 test/result pair。Threading form（`->`、`->>`、
  `some->`、`some->>`）的每个续接 step 必须单独占行，并与初始 threaded value 对齐；
  `as->` 和 `doto` body 必须使用 body indentation。
- Top-level form 之间必须恰有一个空行。连续的 top-level comment line 必须保留为一个
  comment block。Definition 内的 source blank line 没有 canonical significance，必须移除；
  trailing whitespace 也必须移除。

Osiris 对 language-owned form 增加以下规则：多行 Rich Metadata map 的 key 必须从 `{`
后一个 space 的位置对齐；多行 `:doc` value 必须从下一行开始，localized entry 仍按普通
map pair 对齐。`extern` 第一行必须保留 `python` 和 module string，每个 kernel leaf 向内
缩进两个 space。`export` 的 exported collection 必须向内缩进两个 space。`module`、
`import`、`import-for-syntax`、`py/import` 和 `py/decorate` 使用普通 call alignment。
Metadata 必须继续附着到它描述的 datum。

**OEP-0001-R074：** Formatter 必须保留 reader meaning、comment、Rich Metadata attachment、
atom spelling、string content、collection/top-level form order 和所有其他 semantic
distinction，并且必须幂等。如果 syntax error 阻止 meaning-preserving result，必须报告
diagnostic，且不得部分重写该文件。

**OEP-0001-R075：** Formatter CLI grammar 必须是：

```text
osr fmt [<path>...] [--check]
osr fmt --all [--check]
osr fmt -
```

指定 path 时，`osr fmt` 必须原地格式化 `.osr` file；没有 path 时，必须按 OEP-0002
source/exclude rule 格式化当前 project 的 configured source scope。`--all` 必须显式选择
同一个完整 configured project source scope，且不得与 path 或 `-` 组合；这一写法用于提供
Cargo 风格的命令可发现性，并让脚本明确表达处理范围。`--check` 不得写文件，且任何
selected file 不是 canonical 时必须失败。`-` 必须从 stdin 读取一份 source，并且只向
stdout 写 formatted source。

**OEP-0001-R076：** Formatter write 必须保留 file permission，并使用 safe replacement，
使中断不会留下 truncated source。多文件 run 必须按确定的 project-relative order 报告每个
已经或本应修改的 file。Diagnostic 必须标识阻止 formatting 的 source/span。

**OEP-0001-R077：** LSP document formatting 必须使用与 `osr fmt` 相同的 canonical
formatter。对于相同 source snapshot/formatting version，应用 LSP edit 后必须得到与 CLI
输出完全相同的 byte。在 `osr fmt` 暴露相同 range selection contract 前，LSP 不得声明
支持 range formatting。任何 formatting behavior 都不得只存在于 editor extension。

### 语言兼容版本

**OEP-0001-R078：** Osiris language compatibility version 必须独立于 compiler
distribution version 递增，并从 `0.1` 开始。Compiler、interface、standard-library 和
generated-artifact metadata 必须在判断 compatibility 的位置记录 language compatibility
version。Compiler package 的 patch/minor release 不得仅因 package version 改变就隐式改变
language compatibility。

## 理由 (Rationale)

固定 reader 让解析、格式化、恢复与 LSP 行为可预测。小型 kernel 避免宏伪造
binding、nominal type、phase 或 exception semantic，同时让普通控制流与领域词汇留在
Rust 之外。

当前组合式 `nom` reader 符合这里要求的 grammar isolation，但 parser library 不是公开
compatibility contract。

类型标注使用 Rich Metadata，是因为 Python 已支持可读 annotation，Osiris 不需要
独立 type-only language。Local inference 让 private code 简洁，显式 public signature
让 `.osri` interface 确定。

文档分层让两种不同 dataset 保持真实边界。Compiler 内嵌完整英文手册：单一 authored
language 更容易审核、version、搜索和维持权威，AI 则可以用用户语言解释内容。Code
metadata 留在本地，因为 signature、alias 和 workspace provenance 会随项目变化，也可能
是私有数据。本地化代码名称仍是一等能力，因为它们还参与 source lookup。

Embedded libSQL snapshot 让 release documentation 拥有单一 immutable identity、FTS5
search 和离线能力，不会把 database server 变成 compiler 的组成部分。Raw GraphQL 让
`osr doc` 成为小型、可检查的 query interface，而不是第二种 search language；
`osr syntax` 则让最常见的 AI bootstrap 不需要先写 query。CLI-first local tooling 让人和
Agent 无需实现 editor protocol 就能使用全部 compiler fact；LSP 仍负责 IDE ergonomics。

唯一 formatter 消除了 package、generated change、editor 和 AI-authored code 之间的 style
协商。Formatter 属于 core toolchain，也保证 extension 无法借 formatting 重新解释 syntax，
因为 extension 不拥有 reader syntax。

## 向后兼容 (Backwards Compatibility)

这是第一份正式语言与 CLI 规范。现有实现是 draft 的证据，但不能覆盖本提案。进入
Accepted 前可以破坏 pre-release syntax、inspection schema 或 CLI behavior；不能只因
某个 legacy form 曾出现在首个 Accepted language version 之前就保留它。

Public GraphQL schema、embedded database schema、syntax JSON schema 和 local tooling
JSON schema 与 prose revision 独立 version。可以按各自 schema rule 添加兼容字段；需要
精确 semantic 的 consumer 必须拒绝不支持的 major schema version。

## 安全与确定性 (Security and Determinism)

Reader、published Markdown、embedded libSQL bytes、FTS5 index、OEP manifest、package
metadata 和 Rich Metadata 都是不可信数据。加载、索引或渲染时不得执行 Python、macro
code、shell command 或 active content。提供 query 前必须验证 embedded snapshot hash。

Phase-1 执行需要确定的 resource limit 和 dependency closure。GraphQL 必须限制 input、
execution 和 output。Documentation query 禁止启用 writable SQL、extension loading、
external database attachment 或 network access。Local tooling 必须把 path 限制在 selected
workspace 和 validated package root。JSON 必须转义不可信文本，并将其与
identity/provenance 分离。

## 工具与 AI 使用 (Tooling and AI Usage)

OEP-0001-R053 到 R056 规定必需流程。Agent 使用 `osr syntax` 快速了解语言，用 GraphQL
查询其他完整 embedded English manual，并在需要时用带显式 locale 的本地
`osr lsc --format json` 查询 workspace/dependency API。LSP 为人类展示使用标准初始化
locale 或 project display locale。LSP hover、completion、signature help、navigation、
rename、diagnostic、expansion 和 formatting 必须消费与 CLI 等价入口相同的
compiler-owned result。机器 consumer 必须保留 stable ID、origin、revision 和信息类别。

## 被拒绝方案 (Rejected Alternatives)

### 把每个 Clojure form 都当成 parser syntax

多数 Clojure-inspired form 是普通 list，其行为可由宏或函数提供。交给 parser 会扩大
kernel，使 extension、inspection 和测试边界更模糊。

### 把完整 core 放入宏

宏无法独立建立 nominal type identity、lexical frame、exception region、module phase、
verified fact、source mapping 或 backend ABI。这些边界需要小型 compiler-owned kernel。

### 把类型放入独立文件

独立 type declaration 可能与实现漂移，也使生成 Python 不够直接。Metadata annotation
加 inference 能保留单一定义，同时生成 `.osri`。

### 把 `--help` 当作语言手册

Help text 适合回忆命令，不适合精确 syntax、provenance、diagnostic、localization 或
machine consumption。

### 在普通文档搜索中包含 draft

混合 proposed 与 Accepted behavior 会让人和 Agent 生成无效程序。Preview discussion
只有在视觉和结构上完全分离时才有用。

### 把 code metadata 发布到 embedded documentation snapshot

Workspace/package API 由 source、lock state 和 `.osri` interface version，而不是 compiler
documentation snapshot version。内嵌它们会泄露 private code、产生 stale data 并重复
compiler query engine。

### 集中发布每份完整文档的翻译

维护和索引平行 manual 会造成 revision drift 和第二权威问题。Embedded corpus 保持
authored English，AI 可以为用户翻译解释。Repository translation 仍可支持 review，但不
成为 binary content。这不限制 source/`.osri` 中的 localized Rich Metadata；后者由 LSC
和 LSP 提供。

### 只通过 LSP 提供 semantic tooling

LSP 适合 IDE session，但许多 Agent 无法使用，人类直接调用也不方便。因此完整 compiler
query surface 必须有稳定 CLI；LSP server 只负责适配 editor protocol shape。

### 允许项目自定义 formatter style

Style configuration 会让 package source、editor edit、example 和 AI-generated change 不一致。
全语言统一 canonical formatter 让输出可预测，并从 build/review 中移除 style state。

### 让 AI 从 example 推断语法

Example 会过期，也不能枚举 failure behavior。Agent 应加载 release-pinned English syntax
manual、查询其他 embedded manual、检查 local compiler fact，然后用 compiler
format/validate。

## 开放问题 (Open Questions)

无。

## 一致性 (Conformance)

一致实现应提供以下证据：

- reader fixture 覆盖所有固定 form、Unicode identity、metadata prefix、保留与恢复规则；
- kernel inventory 与 standard macro inventory 可以独立查询；
- 宏测试证明 hygiene、phase isolation、determinism 和 origin chain；
- type/`defstruct` 测试证明 inference 与 public boundary rule；
- Python output 可被选定 target 解析，并能把 diagnostic 映射回 source；
- 每条公开 CLI 命令有一份完整 command definition；
- `osr syntax` 离线返回完整 embedded `language/syntax` English manual 的 Markdown 和
  versioned JSON projection；
- `osr doc` 在进程内执行标准 GraphQL document，并在不联网时返回标准 GraphQL JSON；
- embedded libSQL snapshot 测试验证 content hash、read-only behavior、FTS5 index、
  selected complete English document 和 document/chunk provenance；
- public metadata fixture 强制非空 `:doc` default、标准 language tag、translation fallback、
  localized-name identity 和完整 JSON 保留；
- stable/preview publication 测试强制 OEP status、English-only content 和 discussion
  separation；
- 每个 compiler-owned LSP semantic/edit result 都有等价 CLI query 和 parity fixture；
- formatter fixture 证明 preservation、deterministic output、idempotence、`--check`、safe
  write 和 CLI/LSP byte equality；
- LSC locale fixture 默认选择 authored `:default`，LSP fixture 依次使用标准
  `InitializeParams.locale` 和 `displayLocale`；两者都使用 BCP 47/RFC 4647 且不改变
  semantic identity；
- AI workflow fixture 能加载 syntax、查询完整 manual、检查 local semantic、解释
  diagnostic、format 并验证 source file；所有 operation 在没有 network 时仍可工作。

## 修订历史 (Change History)

- Revision 7，2026-07-23：修正一致性措辞；未指定 locale 的 LSC 查询选择 authored
  `:default` slot。该 slot 推荐使用英文，但允许作者使用任意语言。
- Revision 6，2026-07-23：独立 language compatibility version 从 `0.1` 开始，并把稳定的
  compiler-owned exit status 限定为 `0`、`1`、`2` 与 POSIX 中断状态 `130`。
- Revision 5，2026-07-23：在 `osr` 内嵌 read-only libSQL/FTS5 English document
  snapshot，增加离线 `osr syntax`，并以标准 language tag 区分 LSC authored default
  selection 和 project-configured LSP presentation。
- Revision 4，2026-07-23：用二级 `osr lsc` Language Server Console command family 取代通用
  local tooling command。
- Revision 3，2026-07-23：引入 local tooling query command family。
- Revision 2，2026-07-23：分离完整 English GraphQL document 与 local code metadata，
  使 CLI tooling 完全等价于 compiler-owned LSP capability，并规定 canonical formatting。
- Revision 1，2026-07-23：初始草案。
