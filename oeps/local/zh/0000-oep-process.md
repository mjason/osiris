---
oep: 0
title: OEP 目的与流程
description: Osiris 增强提案的治理、文档结构、生命周期和翻译规则。
author: MJ
status: Draft
type: Process
areas:
  - Governance
created: 2026-07-23
updated: 2026-07-23
revision: 5
language: zh-CN
source: ../../0000-oep-process.md
source-revision: 5
translation-status: Current
requires: []
replaces: []
superseded-by: null
resolution: null
---

# OEP-0000：OEP 目的与流程

本翻译不是规范性来源。[英文原文](../../0000-oep-process.md)是唯一规范源。

## 摘要 (Abstract)

本提案定义 Osiris Enhancement Proposal（Osiris 增强提案）流程。OEP 在开始实现
前记录设计决策，为人工审核者和 AI Agent 提供稳定要求，并把已接受的规范连接到
后续代码、测试、文档和兼容性决策。

英文 OEP 是规范源。同步翻译提供本地化审核能力，但不会形成互相竞争的多份规范。

## 动机 (Motivation)

Osiris 同时涉及语言、编译器、宏求值器、类型与 effect 求解器、Python 代码生成、
包接口、LSP 和扩展 contract。非正式讨论不足以让这些表面长期保持一致。

项目需要一套提案系统来：

- 区分规范要求与偶然实现细节；
- 在代码提交让破坏性修改变昂贵之前完成审核；
- 保存决策理由和被拒绝方案；
- 为 AI Agent 提供稳定标识和机器可读元数据；
- 让一致性测试可以追溯到规范要求；
- 同时维护英文和中文视图，并避免翻译静默过期。

## 范围 (Scope)

满足以下任一条件的修改必须建立 OEP：

- 修改语言语法或可观测求值语义；
- 增加或修改公开标准库 contract；
- 修改 `.osri`、macro IR、runtime ABI、包 marker 或产物格式；
- 修改类型、effect、temporal、data-property 或 trust 语义；
- 修改项目配置或依赖解析 contract；
- 增加或修改公开 LSP、Agent 或语义检查协议；
- 建立会影响扩展作者或用户的兼容性承诺。

恢复既有规范行为的普通 bug 修复、内部重构、测试、编辑性文档修改和发布自动化
不要求新 OEP。

## 术语 (Terminology)

- **作者 (Author)**：负责撰写和修订 OEP 的个人或群体。
- **编辑 (Editor)**：检查流程、元数据、范围和状态的维护者。
- **审核者 (Reviewer)**：评估技术提案的参与者。
- **规范性文本 (Normative text)**：定义一致性要求的文本。
- **非规范性文本 (Non-normative text)**：理由、示例、历史和实现说明。
- **源 OEP (Source OEP)**：`oeps/` 中作为规范源的英文文档。
- **翻译 (Translation)**：`oeps/local/` 下本地化、非规范性的版本。
- **修订号 (Revision)**：单调递增、用于识别源文本同步状态的整数，不是生命周期状态。

## 规范 (Specification)

### 提案标识与存储

**OEP-0000-R001：** 项目必须使用 **Osiris Enhancement Proposal** 名称和 **OEP**
缩写表示受本流程治理的文档。

**OEP-0000-R002：** 规范源必须直接放在 `oeps/` 下，并使用
`NNNN-short-title.md` 文件名；`NNNN` 是补零后的永久提案编号。

**OEP-0000-R003：** 提案一旦发布，其编号不得复用，包括被拒绝、撤回或取代后。
编号不表示优先级或接受状态。

**OEP-0000-R004：** 新提案必须分配下一个尚未使用的整数。提案改名不得改变编号。

### 提案类型

**OEP-0000-R005：** 每个 OEP 必须且只能声明以下一种类型：

- **Standards Track**：可观测的语言、标准库、ABI、产物、配置、扩展或工具 contract；
- **Process**：治理、发布、审核或项目级开发规则；
- **Informational**：不定义一致性要求的指导或架构记录。

### 生命周期

**OEP-0000-R006：** 每个 OEP 必须且只能使用以下一种状态：

- **Draft**：仍在编写，尚未准备好决策；
- **Review**：内容足以决策，除审核修改外冻结；
- **Accepted**：已经批准，可以作为实现目标；
- **Final**：已经接受，并有一致性证据证明已实现；
- **Active**：持续适用的已接受 Process OEP；
- **Deferred**：有效工作被有意推迟；
- **Rejected**：已经审核并拒绝；
- **Withdrawn**：作者在接受前撤回；
- **Superseded**：由另一份已接受 OEP 取代。

**OEP-0000-R007：** Standards Track 的正常流转必须是：

```text
Draft -> Review -> Accepted -> Final
```

Deferred 可以回到 Draft。Accepted 只有在明确记录重新打开决策并说明兼容性风险后，
才可以回到 Draft。

**OEP-0000-R008：** Draft 或 Review 阶段的实现不得被视为规范源。实验代码可以
帮助审核，但只有 Accepted 文本定义目标行为。

**OEP-0000-R009：** Accepted 表示授权实现，而不是已经完成。Final 需要链接到
一致性证据和已发布实现。

**OEP-0000-R010：** 已接受且持续适用的 Process OEP 应使用 Active，而不是 Final。

### Front Matter

**OEP-0000-R011：** 每个源 OEP 必须以 YAML front matter 开始，并按以下顺序包含字段：

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

**OEP-0000-R012：** 日期必须使用 ISO 8601 日历格式。元数据中的 OEP 引用必须
使用整数编号。空的多值关系必须使用空数组，缺失的单值关系必须使用 `null`。

**OEP-0000-R013：** 英文源第一次进入公开审核后，每次修改都必须增加 `revision`，
包括编辑性修改。修订号不得降低或复用。

**OEP-0000-R014：** 未标准化的元数据字段必须使用 `x-` 前缀。消费者必须保留
未知 `x-` 字段，但不得从中推导规范性含义。

### 必需文档结构

**OEP-0000-R015：** Standards Track OEP 必须按顺序使用以下顶层章节：

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

Process 和 Informational OEP 可以省略不适用章节，但必须保留 Abstract、Motivation、
Specification 和 Change History。

**OEP-0000-R016：** Standards Track 或 Process OEP 的规范要求必须具有
`OEP-NNNN-RMMM` 格式的稳定标识。进入 Review 后不得重新编号。删除的要求编号保持保留。

**OEP-0000-R017：** 要求语句必须使用 MUST、MUST NOT、SHOULD、SHOULD NOT 或 MAY；
这些词仅在大写时采用 RFC 2119 和 RFC 8174 含义。中文翻译保留相同要求 ID，并以
“必须”“不得”“应”“不应”“可以”表达对应强度。

**OEP-0000-R018：** 只有当精确语法或输出属于 contract 时，示例才能标记为规范性。
其他示例仅作解释，不得覆盖要求文本。

**OEP-0000-R019：** 提案进入 Accepted 前，开放问题必须解决或明确延期。Accepted
OEP 的一致性不得依赖未解决问题。

### 规范与实现的边界

**OEP-0000-R020：** 规范性文本必须定义可观测行为、不变量、接口、失败行为、兼容
边界和一致性要求。除非内部文件布局、算法、Rust 类型或 Python helper 本身属于公开
兼容 contract，否则不得把它们写成规范要求。

**OEP-0000-R021：** 实现讨论必须放在明确标记的非规范性章节或独立实现文档中。
实现说明不得静默增加要求。

**OEP-0000-R022：** 提案可以在实现前定义迁移和验收标准，但必须用可观测结果描述。

### 翻译策略

**OEP-0000-R023：** 英文源 OEP 是规范性来源。翻译必须声明其非规范性，并链接到英文源。

**OEP-0000-R024：** 翻译必须放在
`oeps/local/<language>/NNNN-short-title.md`。初始简体中文翻译目录为
`oeps/local/zh/`，其元数据语言值是 `zh-CN`。

**OEP-0000-R025：** 必须先写源 OEP，再写翻译。英文源具有修订号后，可以在同一次
修改中准备 Draft 翻译。

**OEP-0000-R026：** 翻译必须复制提案编号、状态、类型、创建日期、更新日期和源修订号，
并增加：

```yaml
language: <locale>
source: <relative path to English OEP>
source-revision: <English revision translated>
translation-status: Current | Stale
```

**OEP-0000-R027：** 只有当 `source-revision` 等于英文 `revision`，且所有规范性和
实质性非规范章节都已翻译时，翻译才能标记 Current；否则必须标记 Stale。

**OEP-0000-R028：** Standards Track 提案进入 Review、Accepted 或 Final 前，必须有
Current 的 `zh-CN` 翻译。只有修订本 Process OEP 才能改变此要求。

可以提供 `zh-CN` 之外 locale 的翻译，但任何状态流转都不要求这些翻译。

**OEP-0000-R029：** 不改变英文含义的翻译修正只更新翻译 `updated` 日期，不改变英文
revision。若翻译暴露源文本歧义，必须先修正英文源并增加修订号。

### 面向 AI 的写作规则

**OEP-0000-R030：** 每份 OEP 必须能通过自身元数据、术语、要求和显式依赖被理解。
不应依赖对话历史或未声明的仓库知识。

**OEP-0000-R031：** 具有 Osiris 特定含义的术语必须定义一次并一致使用。规范文本应
优先使用明确名词，避免依赖含糊的“它”“这个”或“系统”。

**OEP-0000-R032：** 跨文档引用规范性语句时，必须使用稳定 OEP 和 requirement ID。

**OEP-0000-R033：** 一致性证据应把测试、诊断、公开符号和 release notes 映射回
requirement ID。AI 生成的实现计划必须引用准备满足的要求。

**OEP-0000-R034：** OEP front matter 和 requirement ID 是结构化接口。自动化工具可以
索引它们，但生成索引必须与源 OEP 校验，且不得成为第二规范源。

### 审核与接受

**OEP-0000-R035：** 从 Draft 进入 Review 需要完整元数据、必需章节、稳定 requirement
ID、已消除的内部矛盾和 Current 中文翻译。

**OEP-0000-R036：** 审核必须评估语义、兼容性、确定性、失败行为、工具影响、扩展
影响和一致性标准。代码风格或实现方便不能单独构成理由。

**OEP-0000-R037：** 接受提案必须在 front matter 中记录持久 resolution URL 或仓库
决策引用。已接受修订必须不可变；后续规范性修改需要增加 revision，改变兼容性时还
需要新 OEP 或明确重新打开决策。

**OEP-0000-R038：** Final 需要一致性证据、已发布产物版本、最新必需翻译，且不能有
未解决的规范性 TODO。

### 仓库索引

**OEP-0000-R039：** `oeps/README.md` 必须列出每个源 OEP 的编号、标题、类型、状态、
revision 和可用翻译。

**OEP-0000-R040：** 索引只是导航。如果与源 front matter 冲突，以源 front matter
为准，并必须修正索引。

### 机器可读清单与文档发布

**OEP-0000-R041：** 仓库必须提供 `oeps/oeps.jsonc` 作为机器可读 documentation
manifest。清单必须声明 schema 版本、规范源 locale、已知翻译、OEP source path、完整
manual source path/stable document ID，以及 documentation publication artifact/channel
policy。

**OEP-0000-R042：** 清单必须使用允许注释和尾随逗号的 JSON。消费者必须把它解析
为数据、拒绝重复 object key，并且加载时禁止执行其中内容。

**OEP-0000-R043：** 清单禁止重复提案状态、revision、标题、requirements 或其他
权威 front matter。这些值必须从被引用的源 OEP 读取。如果清单路径与 OEP 冲突，以
源 OEP 为准，并必须修正清单。

**OEP-0000-R044：** Stable embedded documentation snapshot 只能包含完整 authored
English manual，以及规范 `reference` collection 中 Accepted、Active 或 Final OEP 的
完整英文源文档。Draft、Review、Deferred、Rejected、Withdrawn 和 Superseded OEP 必须
排除。每份 manual 必须有独立于 repository path 的 stable document ID。

**OEP-0000-R045：** Preview/testing embedded documentation snapshot 只能在独立 `discussions`
collection 中发布 Draft/Review OEP 的完整英文源文档。每份此类 document/result 必须
暴露 status 和机器可读 `normative: false`，且其文本不得覆盖该 release 的 normative
reference collection。Manifest 选中的 authored English manual 仍是 reference document，
不得从 discussion text 合成。

**OEP-0000-R046：** 所有 embedded documentation snapshot 都必须排除 OEP-0000，包括
preview discussion。流程/治理文档属于 repository maintainer material，不属于 published
language reference。Repository translation 必须继续用于审核，但不得进入 binary snapshot。
该排除同时适用于 manual/OEP translation；localized source/`.osri` metadata 由 OEP-0001
另行管理，不是 documentation snapshot input。

**OEP-0000-R047：** 不带 prerelease 或 development identifier 的 final distribution
version 必须选择 `stable` publication channel。Prerelease 或 development distribution
version 必须选择 `preview` channel。Release tooling 必须把选中的 English document 导出为
OEP-0001 定义的 read-only、content-addressed libSQL/FTS5 artifact，并把 artifact 内嵌到
原生 `osr` binary。本地 validation 可以构建任一 snapshot，但必须显式声明所选 channel，
不能从没有版本的 working tree 推断。

## 理由 (Rationale)

稳定 requirement ID 让设计、实现、测试、诊断和 AI 生成的工作计划引用同一个意图
单元。单调递增 revision 比 commit hash 更适合检测翻译过期，并且在提交前就可使用。

选择英文作为规范源，是因为包生态、协议标准和外部贡献者普遍消费英文规范。审核前
要求当前中文翻译，可以保留项目的主要中文审核流程，同时避免两份规范源。

Translation 是 repository review aid，不是 embedded binary content。这样每个 compiler
release 可以携带一套 authored、searchable English corpus，多语言 AI client 则用用户语言
解释它。本地化 source API 继续独立通过 LSC/LSP 提供。

区分 Accepted 和 Final，是因为批准必须发生在实现之前。这可以阻止现有代码意外成为规范。

## 向后兼容 (Backwards Compatibility)

OEP-0000 引入新流程，不影响语言 runtime 兼容性。既有设计文档在被已接受 OEP 采用、
取代或明确引用前保持 informational 状态。

## 安全与确定性 (Security and Determinism)

OEP 文档是数据，不得包含可执行元数据。索引 YAML front matter 的工具必须使用安全
解析器，不得通过 YAML tag 实例化语言特定对象。

外部文本、生成摘要和翻译不能授予 Accepted 状态或改变规范含义。状态修改需要仓库
审核和持久 resolution 记录。

## 工具与 AI 使用 (Tooling and AI Usage)

AI Agent 应按以下顺序消费 OEP：

1. 读取源 front matter 和依赖；
2. 读取 Terminology 与 Specification；
3. 把工作映射到 requirement ID；
4. 使用 Rationale 和 Rejected Alternatives 避免重新讨论已关闭选择；
5. 在提出实现前检查 Open Questions 和状态；
6. 只在 source revision 为 Current 时使用翻译。

AI Agent 不得推断 Draft 文本已经授权实现。被要求实现 OEP 时，应先报告状态和未解决
问题，再修改公开行为。

## 被拒绝方案 (Rejected Alternatives)

### 把所有设计保留在单一 language-design 文档

单一大文档难以独立审核、版本化、取代各项决策或映射测试。它仍适合作为总览，但不
适合作为唯一决策记录。

### 让翻译也具有规范性

两种规范语言会导致无法消除的冲突解释歧义。同步的非规范翻译兼顾可访问性和单一权威。

### 只使用文件名或标题，不使用元数据

纯 prose 文档不利于工具和 AI 稳定索引。YAML front matter 提供小型结构化 contract，
同时保持 Markdown 对人可读。

### 接受前必须先实现

这会颠倒预期流程，让实现细节决定规范。实验原型可以存在，但接受过程审核的是 contract。

## 开放问题 (Open Questions)

无。

## 一致性 (Conformance)

OEP 仓库满足本提案需要：

- 每个 OEP 都有有效的必需 front matter；
- 编号和 requirement ID 唯一且永久；
- README 索引与源 front matter 一致；
- Review 或以后状态的 Standards Track OEP 有 Current 中文翻译；
- 翻译声明并匹配 source revision；
- 状态流转和接受 resolution 满足本流程。

## 修订历史 (Change History)

- Revision 5，2026-07-23：保留必需的 Current 中文审核翻译，并明确其他 locale 的翻译
  全部可选。
- Revision 4，2026-07-23：把完整 manual/OEP 定义为内嵌到每个原生 `osr` release 的
  English-only libSQL/FTS5 snapshot input；repository manual translation 继续作为审核资料。
- Revision 3，2026-07-23：用 English-only 中央 documentation publication snapshot 取代
  CLI bundle；repository translation 继续作为审核资料。
- Revision 2，2026-07-23：增加 JSONC 清单、按版本选择的正式与预览文档通道，并从
  CLI 中排除 OEP-0000。
- Revision 1，2026-07-23：初始草案。
