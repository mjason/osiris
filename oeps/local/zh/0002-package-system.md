---
oep: 2
title: 项目与包体系
description: 最小项目配置、uv/PyPI 集成、构建命令、扩展发现和可发布 Osiris 产物。
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
revision: 21
language: zh-CN
source: ../../0002-package-system.md
source-revision: 21
translation-status: Current
requires: [0, 1]
replaces: []
superseded-by: null
resolution: null
---

# OEP-0002：项目与包体系

本翻译不是规范性来源。[英文原文](../../0002-package-system.md)是唯一规范源。

## 摘要 (Abstract)

本提案定义 Osiris 项目如何通过 Python 生态配置、构建、watch、初始化和发布。
`osiris.jsonc` 管理小型 compiler config；`pyproject.toml` 管理项目与 dependency
metadata；`uv` 负责 resolve 与 lock；PyPI-compatible distribution 携带 extension
interface 与生成 Python。

Osiris 不引入 package manager。扩展是携带静态 Osiris marker/interface artifact 的普通
Python distribution。Compiler 确定性地发现这些 artifact，不 import package code。

## 动机 (Motivation)

Osiris 程序编译到 Python，并使用普通 Python data library。第二套 resolver 或 registry
会重复 uv/PyPI、破坏 reproducibility，也让扩展更难发布。但 Python metadata 本身不能
描述 Osiris module、macro、Rich Metadata、type、contract 或 source map，因此需要最小
compile config 与静态 wheel contract，同时保留 Python 用户熟悉的 package ownership。

## 范围 (Scope)

本提案规定 `osiris.jsonc`、初始化、module/file 映射、项目命令、uv/PyPI ownership、
`osiris_build` PEP 517 backend、静态 extension artifact、确定性 discovery，以及 native
`osr` distribution。

本提案不规定自定义 registry/installer/resolver/lock/publish service、标准库 API、领域
包、多 target 同时 codegen 或初始版本中的任意 PEP 517 backend composition。

## 术语 (Terminology)

- **Project root**：包含 `osiris.jsonc` 及关联 `pyproject.toml` 的目录。
- **Source root**：配置的、包含 `.osr` module 的项目相对目录。
- **Output directory**：项目相对 build 目标目录。
- **Artifact set**：为一个 module graph 与 target 生成的全部输出。
- **Extension distribution**：wheel 携带 Osiris marker 和至少一个公开 `.osri` 的 Python
  distribution。
- **Package**：由 `osr init --package` 创建的普通 Python distribution；它可以暴露一个
  或多个 public Osiris module，此时也是 extension distribution。
- **Linkable backend**：由 package 所有、非 public、可由 macro expansion 选择并复制到
  consumer artifact 的 Osiris runtime closure；它不是 installed shared Python runtime。
- **Runtime closure**：宏展开后一个 generated artifact 所需 linkable backend binding 与
  module 的传递集合。
- **Marker**：列出 extension artifact 的静态 `dist-info/osiris.toml` 数据。
- **Effective dependency graph**：项目 build 使用的 locked runtime dependency closure。
- **Development dependency**：用于 build/edit，但不会自动暴露给 consumer 的依赖。

## 规范 (Specification)

### Ownership 与配置

**OEP-0002-R001：** Osiris 项目必须在 project root 使用 `osiris.jsonc` 作为 compiler
configuration。工具拿到 source path 后必须从该路径向 ancestor 查找最近配置，禁止合并
多个 ancestor configuration。

**OEP-0002-R002：** `osiris.jsonc` 必须接受 JSON comment 和 trailing comma、拒绝
重复 object key，并作为不可执行数据解析。Strict configuration validation 必须诊断
未知 field。

**OEP-0002-R003：** 初始 configuration surface 只能包含下列 field，以及
OEP-0002-R005 定义的可选 `exclude` field，不得包含其他 compiler field：

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

`$schema` 是 editor aid；`source`、`outDir`、`targetPython`、`strict`、`displayLocale` 是
compiler field。本版本 config 禁止包含 `watch`、`extensions`、`buildGroups`、`trust` 或
`agent`。

**OEP-0002-R004：** `source` 必须是非空、有序、互不重复的 project-relative source
directory 集合。必须拒绝 absolute path、逃逸 project root 的路径、作为 source 的 output
directory 和 normalize 后重复的 path。

**OEP-0002-R005：** 可选 `exclude` field 可以包含 project-relative glob。没有 glob
syntax 的 pattern 必须排除该 path 及 descendant。Pattern 必须在 source inclusion 后对
normalized project-relative path 求值，并支持 source root 内部排除。Build、watch、check、
format、LSC、module discovery 与 LSP 必须使用相同语义。

**OEP-0002-R006：** `outDir` 必须是 project-relative directory，默认 `dist`。即使宽泛
source/glob 会包含它，也必须从 source discovery 排除。

**OEP-0002-R007：** `targetPython` 必须指定分析和 codegen 的单个 Python language
version，默认 `3.11`。一次 build 只能生成一个 target 的 artifact。Interface、cache、
source map 与 build record 必须记录 target，修改它必须使 target-sensitive result 失效。

**OEP-0002-R008：** 除 `targetPython` 外，configuration shape 不得编码永久 Python
backend 假设。以后 Accepted OEP 可以增加 target discriminator；初始 compiler/invocation
应优化单 Python target，不维护未使用的 multi-target machinery。

**OEP-0002-R009：** `strict` 必须根据语言规则控制 unresolved dynamic boundary、
不完整 public contract 和 unsupported configuration 是否为 error；不得改变 reader syntax
或 Accepted semantic。

**OEP-0002-R010：** `displayLocale` 必须接受任意合法 BCP 47 locale tag，默认
`zh-CN`。它只控制 tooling label/documentation，不得改变 canonical binding、resolution、
artifact 或生成 Python。LSP session 必须优先使用标准 `InitializeParams.locale`（如果存在
且 well-formed），否则使用该 project value。`osr lsc` 不得继承该字段：它按
OEP-0001-R070 使用 authored default，除非 request 提供 `--locale <bcp47>`。缺少
localized data 时必须使用 OEP-0001-R059 的 RFC 4647 lookup 和 authored `:default`
fallback。实现不得用 closed enum 或 project-specific locale key 取代 BCP 47 tag。

**OEP-0002-R011：** `pyproject.toml` 必须继续管理 distribution name/version、Python
requirement、dependency、build backend、index 和 publish metadata。`osiris.jsonc` 禁止
重复这些 field。

### 初始化

**OEP-0002-R012：** `osr init <project>` 必须创建 uv-compatible Python project、
`osiris.jsonc`、有效 source root/starter `.osr` module，并把已安装 Osiris distribution
加入 development dependency。目标目录已存在时必须拒绝覆盖。

**OEP-0002-R013：** `osr init --existing [directory]` 必须在不替换既有
`pyproject.toml` metadata、dependency、comment、lock policy、source file 或有效 Osiris
config 的前提下加入 Osiris。重复执行必须幂等。

**OEP-0002-R014：** 初始化必须使用 uv 的公开 command/file contract 添加 compiler
dependency 并更新 lock。uv 失败时必须报告，且不得留下宣称 setup 成功但 dependency
update 未完成的配置。

**OEP-0002-R015：** `osr init --package <project>` 与
`osr init --existing --package [directory]` 必须配置使用 `osiris_build`、canonical
Osiris namespace 和至少一个 public Osiris module 的 Python distribution。转换 existing
project 时遇到 incompatible build backend 必须拒绝，不能静默替换。Package 是 author
面对的 lifecycle 概念；extension distribution 是 compiler 对暴露 public interface 的
package 所作的 discovery 分类。

### Module 与项目命令

**OEP-0002-R016：** Module file 相对唯一 source root 的 path，移除 `.osr` suffix 并把
separator 替换成 dot 后，必须决定 canonical module path。显式 source module declaration
必须匹配。多个 root 造成 ownership ambiguity 时必须拒绝。

**OEP-0002-R017：** 生成 `.py`、`.osri`、`.py.map` 必须在 `outDir` 下保留 module
relative path。两个 source module 映射到同一 canonical module 或 target path 时必须诊断。

**OEP-0002-R017A：** Source root 布局按 Osiris 方式拼写 module 分量（OEP-0002-R016）；
`outDir` 之下的每个 path 以及构建产物 distribution 内的每个 file path 按 Python 方式拼写，
依 OEP-0001-R005B。任何组件将边界一侧的 name 与另一侧的 path 相比较时，必须先翻译再比较，
不得按字面比较拼写。字面比较对映射不改变的每个 module name 都通过，然后拒绝第一个被映射
改变的 name——`(module my-pkg.core)` 能编译却永远打不出包，因为 build backend 要求
declared name 与翻译后的 archive path 字面相等。Source root 之下的目录分量是 module
分量，必须翻译；非 module file 的 basename 不是，按作者所写保留，使 runtime resource
查找能以作者写下的名字找到它。

**OEP-0002-R018：** `osr check [path]` 必须发现适用 project，parse/expand 所选 module
graph，验证 name、type、contract、interface、extension record 和 target compatibility，
不得发布 build artifact 或修改 dependency state。

**OEP-0002-R019：** `osr build [directory]` 必须在全部 `check` gate 通过后编译完整
configured source scope，并以足够原子的方式把一套 coherent artifact 发布到 `outDir`，
使失败 build 不会把新旧 module artifact 混成同一个 build。

**OEP-0002-R020：** `osr compile <file>...` 必须保留为低层 explicit source command。
它可以 override output directory/artifact selection，但必须使用与 build 相同的 semantic、
target、dependency 和 interface gate。存在 containing project 时必须应用其配置。

**OEP-0002-R021：** `osr watch [directory]` 必须是命令，不是 configuration field。
它必须执行与 `build` 相同的操作，并在 included `.osr`、configuration、lock 或所选 static
interface input 改变时重建，且与 build 使用相同 `source`/`exclude`。

**OEP-0002-R022：** Watch 是 native compiler operation。为了观察 file、resolve
Osiris source 或编译 change，不得要求或启动 Python。它必须合并重复 filesystem event、
不 watch `outDir`，并遵守 OEP-0001-R040 的 interruption contract。

**OEP-0002-R023：** `osr run <file> -- <arguments>...` 必须先在 isolated staging layout
中应用与 compile 相同的 validation/Python generation contract，再用其余 argument 启动
所选 target Python environment。不得修改 dependency 或 lock state；成功完成
compilation 后必须传播所启动程序的 exit status。

**OEP-0002-R024：** 每条项目命令必须通过 command definition 和 Documentation service
发布的完整 authored English command manual，说明 configuration search、source scope、
artifact write、Python process use、dependency use，以及 watch/interruption behavior。

### Dependency management 与 discovery

**OEP-0002-R025：** Osiris 必须使用 Python requirement metadata、PyPI-compatible
index 和 uv 声明、resolve、install、lock dependency。项目禁止定义 Osiris package
registry、dependency syntax、resolver、installer、lockfile 或 publish protocol。

**OEP-0002-R026：** Build 只能从 `uv.lock` 中已验证的 effective runtime dependency
graph 选择扩展。Development tool 或无关 installed distribution 不能仅因存在于
site-packages 就变得可见。

**OEP-0002-R027：** Lock validation 必须使用 uv 对实际 source kind 提供的 distribution
identity、version、source descriptor、dependency edge 和 integrity data。对于合法 lock
representation 不含 source hash 的 editable、workspace、path 等 entry，禁止要求虚构
source hash。

**OEP-0002-R028：** Compiler 必须从静态 `dist-info/osiris.toml` 和引用的 `.osri`
resource 发现 extension。Discovery、check、build、watch、LSC 与 LSP 禁止 import 或
执行 extension Python code。

**OEP-0002-R029：** Distribution name 必须按 Python package name normalization 比较。
Module/binding identity 仍按 OEP-0001 处理 case/Unicode，禁止按 distribution name
normalize。

**OEP-0002-R030：** Lock graph、installed distribution metadata、marker、interface hash、
target 或 declared dependency 不一致时，compilation 必须以 stable diagnostic 失败。
Discovery 禁止按 filesystem order 或 import precedence 选择 best effort package。

### Package build、linkable backend 与 artifact

**OEP-0002-R031：** `osiris_build` 必须是随 Osiris 发布的 PEP 517 build backend。
Osiris package 的 `[build-system]` 必须 pin compatible `osiris-lang` build requirement，
并指定 `osiris_build` 为 backend。

**OEP-0002-R032：** Osiris package sdist 必须包含 `pyproject.toml`、`osiris.jsonc`、全部所需
`.osr` source 和 reproducible wheel build 所需文件。Sdist build 不得依赖 project root
之外未列出的文件。

**OEP-0002-R033：** Extension wheel 必须包含可读 generated Python、public `.osri`、
与这些 interface 对应的 authored `.osr`、所需 `.py.map` 和 public static-record sidecar。
Reachable standard support 必须依 OEP-0003 编译到每个 owning Python package 的 private
`__osiris_runtime__`。Runtime semantic 与 dependency compilation 必须使用 validated
`.osri`，不能隐式重新编译 packaged source。

**OEP-0002-R033A：** 对每个可从 public interface 或 exported macro expansion 到达的
package-owned Osiris runtime binding，extension wheel 还必须包含 validated、versioned
linkable-backend artifact。Artifact 必须包含完整 binding/module closure、stable binding
identity、可 relocate 的 internal module edge、source-map identity 和 semantic interface
dependency hash。该规则同样适用于普通 public Osiris function 与 macro 引入的 private
function。Authored `.osr` 只用于 navigation/audit；consumer 禁止通过重新编译它获得
backend behavior。

**OEP-0002-R033B：** 可能生成 package-private runtime binding 的 macro interface 必须在
validated macro artifact 中声明该 binding 的 linkable backend closure。Expansion 必须
保留 stable binding identity。Closure 缺失、不兼容、无效或不能覆盖全部 emitted private
runtime binding 时，必须在 codegen 前失败。Macro expansion 禁止把 private binding 静默
变成 public interface binding。

**OEP-0002-R033C：** Consumer compilation 期间，linker 必须取得 expanded HIR 选择的
transitive runtime closure，并且只释放到 owning output package 的 reserved
`__osiris_runtime__/packages/<provider-id>/`。`<provider-id>` 必须由 locked provider
distribution identity 与 validated artifact hash 确定性生成，并避免碰撞。全部 internal
backend import 必须 relocate 到该目录。Generated consumer Python 在 runtime 禁止 import
provider package 的 generated Osiris Python module、`osiris-lang`、`.osr` source、`.osri`、
macro IR 或 documentation data。

**OEP-0002-R033D：** Package 只能通过 OEP-0001 的 embedded `python` module 提供
package-owned Python backend。该 module 是 linkable artifact 携带的 compiler input，
不是对 provider installed Python namespace 的 import。其 private provider handle 禁止出现
在 public interface 中；只有 validated `extern` binding 可以建立 downstream reachability。
它可以 import 明确声明的普通
Python dependency（例如 `pandas`）；这些 provider 保持普通 `Requires-Dist` dependency，
绝不复制到 `__osiris_runtime__`。大型、native、独立 version 或通用可复用的 Python
实现应该作为独立 Python distribution。未声明的 same-distribution `.py` file 禁止隐式
成为 runtime provider。

**OEP-0002-R033E：** Package 可以包含 public facade module、private implementation
module、Phase-1 macro helper 和 linkable backend module。只有 explicit export declaration
对普通 downstream source 公开。未公开的 declaration 只能由 package macro 通过其
declared linkable closure 使用，并且不能被 consumer import、completion 或直接引用。

**OEP-0002-R033F：** Package source layout 必须遵循 `osiris.jsonc` source root 下的普通
module mapping。例如 `src/acme_text/core.osr` 与
`src/acme_text/backend/normalize.osr` 分别定义 `acme_text.core` 和
`acme_text.backend.normalize`。Build backend 必须根据 resolved binding reachability 推导
linkable closure，禁止引入第二个 backend directory、configuration list、filename
convention 或 consumer source recompilation。为 inspection 随 wheel 发布的 generated
Python 禁止被选为这些 Osiris binding 的 runtime provider。

**OEP-0002-R033G：** Provider wheel 必须把 relocatable closure graph 存在
`<distribution>.dist-info/osiris/linkable-backend.hir.json`，把 embedded Python module
存在 `<distribution>.dist-info/osiris/python-runtime/<logical-path>.py`，并把 graph 指名的
authored `.osr` 按 canonical module path 放入 wheel 供 navigation。`osiris.toml` 必须记录
`linkable_backend`、`linkable_backend_hash`、schema/ABI、Python target、closure-root
binding identity、embedded Python module path/hash 与 source-map identity。
`dist-info/osiris/` 下文件是 compiler data，不是 installed import target，在 discovery 或
compilation 时绝不能 import/execute。Consumer linker 读取它们，只选择 reachable closure，
在 `__osiris_runtime__/packages/<provider-id>/` 下生成可读且已 format 的 `.py`，并把
selected hash 记录到 consumer runtime manifest。

**OEP-0002-R033H：** Osiris runtime code 必须按 binding-level transitive reachability
选择。Embedded Python 必须按 module-level transitive static-import reachability 选择：一个
embedded module 提供的任意 declared binding 可达时，必须释放完整 normalized module 及其
static import closure 中全部 provider-owned embedded module。初始 linker 禁止尝试 Python
function-level tree shaking。Provider wheel 只携带一次完整 validated graph；每个 consumer
output 只携带 selected Osiris binding 与 embedded Python module。

**OEP-0002-R033I：** `pyproject.toml` 的 `[tool.osiris].wheel-exclude` 可以给出针对
Osiris module path 的 fnmatch pattern。匹配的 module 仍然必须随完整 project graph 编译，
仍然必须进入 sdist，但它的任何 artifact——authored source、生成的 Python、interface、
source map、`osiris.toml` 条目——都不得进入 wheel。项目用它把 test module 挡在发行物
之外：wheel 装什么是打包配置，因此在这里决定，而不由编译器决定。会剔除所有 module 的
配置必须被拒绝；未知的 `[tool.osiris]` 字段必须被拒绝，使拼错的 pattern 列表不会静默地
什么都不剔除。

**OEP-0002-R034：** Wheel backend 必须生成
`<distribution>.dist-info/osiris.toml`，作者禁止手工维护。Marker 必须标识 schema version、
provider distribution/version、target Python compatibility、每个 interface/source path、
semantic/source-map hash、linked-support manifest/hash、records artifact，以及确定性验证
所需 dependency identity 与 OEP-0002-R033G 的 linkable-backend field。

**OEP-0002-R035：** 每个 `.osri` 必须包含下游 parsing、macro expansion、name/alias
resolution、type、Rich Metadata、static record 和 declared contract 所需 public module
interface。Private detail 不得仅因 source 被打入 wheel 就变成 public。

**OEP-0002-R036：** 相同 source、compiler、target、lock 和 configuration 的 artifact
serialization/marker ordering 必须确定。Published artifact 中的 path 必须 normalize，且
不得暴露 machine-specific project-root 或 temp directory path。

**OEP-0002-R037：** Backend 在组装 wheel 前必须验证所有生成 artifact/hash，consumer
使用前也必须验证。Partial、stale、path-escaping、duplicate、oversized 或 hash mismatch
artifact 必须失败，不能 fallback 到 Python import 或 source execution。

**OEP-0002-R038：** Public interface dependency 与 linkable backend 使用的独立 Python
provider 必须作为普通 runtime requirement 声明在 `pyproject.toml`，使 Python wheel
metadata 保留同一 dependency closure。Development-only dependency 不得满足 published
interface reference 或 linkable backend import。

**OEP-0002-R039：** Package 必须使用 `uv add`、`uv build`、`uv publish` 等普通工具
install/publish。Compiler 可以提供 diagnostic/scaffold，但禁止包装成第二套 package
lifecycle。

### Compiler distribution

**OEP-0002-R040：** 本项目 PyPI distribution name 必须是 `osiris-lang`，installed
executable 保持 `osr`，build backend 保持 `osiris_build`。Generated project 使用自己的
Python package name；reserved private support package 是 `__osiris_runtime__`，绝不是
shared `osiris` runtime package。

**OEP-0002-R041：** `osiris-lang` wheel 必须把 `osr` 安装为 native Rust executable。
Python packaging 可以投递 executable 与 PEP 517 backend，但 generated output 在 runtime
不得需要该 wheel；CLI、watcher、LSP 不得是 Python-hosted process。

**OEP-0002-R042：** 必须记录 compiler、build backend、interface format、marker format、
standard-library ABI 和 Linkable-helper format compatibility，使不兼容 build combination
在宏展开或 code generation 前失败。Support 链接完成后，普通 Python execution 不得需要
Osiris runtime compatibility check。还必须记录 documentation snapshot/GraphQL schema
compatibility，但只能由 documentation client 验证，不得作为 check、build 或 execution gate。

**OEP-0002-R043：** Wheel source map 必须使用 normalized wheel path 与 content hash 引用
其必需的 packaged `.osr` member，并针对该 member 编码 source span；不得在 map 内重复
authored source text。Consumer 使用 mapped span 前必须验证 member hash。

**OEP-0002-R044：** Editable package installation 必须使用 configured build backend 生成的
标准 PEP 660 editable wheel。Compiler 不得绕过 validated wheel metadata contract，通过自创
editable-directory 或 source-tree scan 发现 extension。

**OEP-0002-R045：** Stable versioning 之前，generated project/package scaffold 必须把
`osiris-lang` 限制在 compiler package 当前 minor release line。因此 `0.3.0` release 生成
`osiris-lang>=0.3,<0.4`。Artifact/marker 还必须记录 compilation 所需的确切 language、
interface、standard-library 与 helper ABI value。

**OEP-0002-R046：** Project compilation 可以复用 `.osiris/cache/` 下一个有界且版本化的
cache。Cache 必须与 `outDir` 分离、不得发布，并且在任何时候删除都不影响正确性。Cache
identity 必须覆盖 source bytes/module identity、语义 project/lock input、target/strict、
compiler/language/interface ABI、generated-Python formatter ABI、trust-policy hash、所选
artifact kind、standard-library identity，以及每个所选 external interface 的 semantic、
tooling 和 content hash。实现不得把 modification time 当作正确性依据；lookup 前必须验证
静态 dependency；unknown、malformed、partial、oversized、path-escaping 或 hash-mismatch
entry 必须视为 miss；cache state 必须原子替换；失败 build 不得污染最后一次成功 entry。
命中时可以保持完全相同的 `outDir` 不动，也可以在 output 缺失或 stale 时原子恢复同一套
artifact path；使用 cache 不得改变 artifact contract。

**OEP-0002-R047：** 每个生成 `.py` artifact，包括由源码编译的 standard facade 和由
compiler 链接的 private runtime module，都必须使用 compiler 固定的统一 Ruff formatting
profile。Formatting 必须在计算 generated-source hash 与 `.py.map` position 之前完成。
Formatter 必须嵌入 native compiler；build 不得依赖 ambient `ruff` executable、Python
process 或项目自己的 Ruff configuration。

**OEP-0002-R048：** 生成的 Python 必须是可检查的 source target，而不只是可执行的
transport。Runtime declaration 上 authored fallback `:doc` metadata 必须成为 Python
docstring。Authored identifier 必须保留 canonical、可读的 Python mapping；compiler-owned
hygienic binding/helper 必须使用能看出用途的 private name，不得泄漏转义后的内部 identity。
在保持 evaluation order 与 module initialization 时，backend 应折叠 expression-only control
flow、terminal temporary 和重复 import。这些 projection 不得在 compiler 中把 standard
macro 重建为语法；macro expansion 以及 standard-library/kernel boundary 仍是权威定义。

**OEP-0002-R049：** Compiler configuration 禁止包含 network provider、credential、model
或 conversation setting。Compiler 不得为 language tooling 加载 project `.env`。已撤回的
Language Server Agent 实验不形成 package、cache、configuration 或 credential contract。

**OEP-0002-R051：** 发布的 `osiris.jsonc` JSON Schema 必须说明每个已接受 field 的 default、
effect 与代表性 value。Editor integration 必须通过标准 JSON language service 与该 schema
提供 configuration completion、hover 和 structural validation。Osiris LSP 必须报告阻止
source analysis 的 project-loading failure，但不得实现一套相互竞争的 JSONC editor service。

**OEP-0002-R052：** LSC 必须使用嵌入式 libSQL 在
`.osiris/cache/language-graph.sqlite3` 维护 project semantic graph。Node 必须覆盖配置范围内的
project module/symbol，以及静态发现、lock-reachable 且实际进入分析的 Osiris interface symbol。
Compiler 能确定时，edge 至少必须覆盖 `imports`、`exports`、`calls`、`references` 与
`alias-of`。Type、Rich Metadata 文档、example、本地化名称与 source provenance 必须成为可
检索 node data。Graph 禁止索引普通 Python package 或 raw embedded-Python body；Python 能力
只能通过声明的 Osiris interface 进入 graph。

该 graph 是可删除 tooling cache，绝不是 artifact。Identity 必须覆盖 graph schema、
compiler/language version 与确定性的 compiler fact；identity 改变时必须原子替换 graph 内容。
Node、edge、LSC result 或其他 tooling output 中的 project path 必须使用稳定的
`osiris-workspace:///` URI，不得保存机器绝对路径。

Cache validation 必须发生在完整 workspace analysis 之前。input identity 必须覆盖配置范围内、
未被排除的 Osiris source、project configuration 与 lock data、选定的 Python target 与 strictness，
以及 lock-reachable static interface 的完整内容。identity 与 graph schema 匹配时，graph-only
operation 必须直接打开 libSQL，不得初始化 language server。input 改变、cache 缺失或 cache
无效时，必须自动执行 analysis 并原子替换；正常 cache lifecycle 不要求用户手工刷新。

Validation 必须能随 workspace 文件数量扩展，不能在每次 query 时重新读取全部 source。Cache
必须持久化逐 input manifest，记录稳定 identity、byte size、高精度 modification stamp 与 content
hash。正常 validation 在 identity、size 与 stamp 匹配时必须复用 content hash，只读取并 hash
新增或已变化的 input；source metadata inspection 与 hashing 可以使用有界并行 worker。该 manifest
只用于加速 validation：content hash 仍是 graph identity 的权威依据；identity 改变后的自动
semantic analysis 仍是 dependency-correct 的完整 workspace analysis，除非未来 OEP 另行定义
incremental compiler interface 与 reverse-dependency invalidation。

完整 semantic analysis 必须保持 dependency 与 phase ordering；实现应把所有 ready strongly
connected component 放入同一个确定性 topological layer，并以有界并行 worker 处理相互独立的
provisional-interface construction、macro expansion 与 frontend analysis。

`osr lsc cache status` 必须在不构建 graph 的情况下报告当前 input identity 是 `fresh`、
`stale`、`missing` 还是 `invalid`；还必须报告 manifest input 总数以及 validation 中复用或重新
计算的 content hash 数量，使大型 workspace 的行为可观察。`osr lsc cache rebuild` 必须忽略
已有的匹配 identity，执行完整 workspace analysis，并原子替换整张 graph。它是故障恢复和
排障后路，不是源码修改后的
日常步骤。

**OEP-0002-R053：** LSC 必须作为复合本地 language service 的公开 CLI boundary，并提供由
semantic graph 和适用 LSP request 支持的有界 `workspace-search`、`symbol-context` 与
`source-context` 复合操作。`symbol-context` 必须根据 advertised capability 使用 hover、
definition、signature help、references、document symbols 与 workspace symbols；language
service/capability 不可用、歧义、多定义、非法 URI 与 request error 必须显式表示，禁止猜测。
Source context 只能来自配置范围内未排除的 Osiris source 或验证过的 packaged standard source，
并限制为有界 enclosing form。

## 理由 (Rationale)

配置故意保持小型。`source` 已定义 watch scope，第二个 watch field 会产生矛盾 tree；
extension 已存在于 Python dependency，重复列表会漂移；`trust` 属于 semantic contract，
不是 package-selection switch；单 scope/target 下 `buildGroups` 没有价值。

Wheel 携带 `.osr` 方便 source navigation 与 audit，`.osri` 仍是静态 compilation authority，
从而避免 installed source 变成隐式 executable plugin。一次 invocation 一个 target 能保持
compiler 快速、artifact identity 清晰，同时为以后 OEP 留出扩展空间。

Package-owned backend 与 standard library 使用同一 deployment 方向：validated、可
relocate Osiris closure 与小型 explicit embedded Python module 从 provider metadata 进入
consumer private `__osiris_runtime__`，绝不从 provider installed generated namespace
import。独立管理的 Python 实现仍是普通 package 和明确 dependency。

## 向后兼容 (Backwards Compatibility)

第一个 Accepted project/package contract 前允许 breaking change。`watch`、`extensions`、
`buildGroups`、`trust` 等 pre-release field 不保留，应拒绝而非忽略。Accepted 后 config 与
artifact schema 必须 version；consumer 遇到不支持 major schema 必须拒绝，不能猜测。

## 安全与确定性 (Security and Determinism)

访问 filesystem 前必须 normalize config、lock、marker、archive、interface 中的 path，并
限制在声明 root。Archive extraction 必须拒绝 traversal、normalized duplicate、逃逸 staging
tree 的 link 与 resource-limit violation。

Package discovery 必须静态且 lock-scoped。Python import side effect 和 ambient
site-packages 不得影响 compilation。Artifact 必须经过 staging/validation write，避免中断
发布误导性的 mixed result。

## 工具与 AI 使用 (Tooling and AI Usage)

可以通过 `osr doc` 发现的完整英文 command manual 必须说明 `init`、`build`、`compile`、
`watch` 和 `run` 的 side effect/Python process behavior。`osr lsc semantic --format json`
应暴露 imported interface provider、version、hash 与 lock provenance，同时隐藏 local
absolute path。

AI 添加 extension 时必须编辑普通 `pyproject.toml` requirement 并使用 uv；不得发明
`extensions` config field、手写 `osiris.toml`、扫描 site-packages，或假设 packaged `.osr`
覆盖 validated `.osri`。

## 被拒绝方案 (Rejected Alternatives)

### 构建专用 Osiris package manager

这会重复 Python 生态已有的 dependency resolution、index、credential、publish 与 lock。

### 在 `osiris.jsonc` 列出 extension

这会让 compiler selection 与 runtime requirement/`uv.lock` 不一致。Locked wheel 携带
有效 static marker 时，该 package 自然成为 extension。

### 配置独立 watch tree

Watch 编译同一个项目，所以必须观察 build input；第二个 scope 会造成漏 rebuild 或多余
event。

### 执行 Python entry point 来 discovery

Entry-point execution 会使 compilation 依赖任意 package code、ambient environment 和
import order。

### 对所有 uv lock entry 要求同一种 source hash

uv 对 registry、URL、Git、workspace、editable 和 path 使用不同表示。Validation 必须验证
真实 descriptor 定义的 integrity field，而不是虚构字段。

### 初始版本支持任意 PEP 517 backend composition

Backend composition 会产生 generated file/metadata ownership 歧义。初始 extension
distribution 只有一个 backend；复杂 native package 可以拆分 distribution，直到专门 OEP。

## 开放问题 (Open Questions)

无。

## 一致性 (Conformance)

一致实现应提供以下证据：

- JSONC fixture 覆盖 comment、trailing comma、duplicate key、最小 config、source 内
  exclusion 与被拒绝 legacy field；
- init fixture 覆盖新项目、existing uv project、幂等、package setup 与 uv failure rollback；
- module mapping/atomic artifact 测试覆盖 collision/stale output；
- cache fixture 证明 unchanged reuse、output restore、source/config/target/interface
  invalidation、corruption fallback 和 failed-build safety；
- semantic-graph fixture 证明 libSQL persistence/invalidation、多语言 Rich Metadata search、
  cross-file call/reference、歧义、dependency-interface discovery、workspace URI 脱敏与 raw
  Python body 排除；
- generated Python/runtime fixture 对固定 Ruff profile 保持 idempotent，source map 指向
  formatting 后的 line layout；
- generated-source fixture 保留 fallback docstring、可读 hygienic name、惯用的
  expression-only control flow 与非重复 import；
- watch 测试证明与 build 相同 scope、排除 output、合并 event、响应 config/lock change、
  无 Python 且可立即中断；
- lock fixture 覆盖 registry、Git、URL、workspace、editable 与 path source；
- 恶意 marker/interface/wheel/path fail closed 且不 import package code；
- sdist 能构建含 source、Python、interface、map、record 与 generated marker 的确定 wheel；
- direct public Osiris call 与 exported macro 生成 private runtime binding 时，都只把其
  validated transitive backend closure 链接到 consumer `__osiris_runtime__` 下，且不
  import provider package 或 Osiris runtime；
- missing、stale、incompatible、incomplete linkable closure 在 Python generation 前失败，
  undeclared same-distribution Python backend 也必须被拒绝；
- embedded `python` module 只在其 declared binding 可达时复制，携带完整 static embedded-module
  import closure，而 `pandas` 等普通 dependency 留在 runtime package 之外；
- uv 安装的 consumer 能只依赖 validated locked artifact 导入 extension interface 并编译。

## 修订历史 (Change History)

- Revision 21，2026-07-30：新增 OEP-0002-R033I，`[tool.osiris].wheel-exclude`：以
  module path 的 fnmatch pattern 指定不进 wheel 的 module；它们照常编译、照常进 sdist。
  wheel 装什么是打包配置，所以这个开关在 `pyproject.toml` 与 build backend，不在编译器。
- Revision 20，2026-07-30：新增 OEP-0002-R017A，命名 source root 布局（Osiris 拼写）与
  构建产物 path（Python 拼写）之间的拼写边界，要求跨边界比较前必须翻译。记录在案的原因：
  build backend 曾按字面比较，拒绝 OEP-0001-R005A 映射会改变的每一个 module name，
  `(module my-pkg.core)` 能编译却永远打不出包。
- Revision 19，2026-07-27：修正 OEP-0002-R033D 中本意规范却写成小写的 RFC 2119
  关键词，按 OEP-0000-R017 它此前不具约束力。
- Revision 18，2026-07-27：在 OEP-0003-R005G 从标准宏面移除 `defn-` 之后，改用「未公开的
  declaration」重述 OEP-0002-R033E；package 可见性从来不由声明形式决定，因此 package
  行为不变。
- Revision 17，2026-07-27：撤回实验性的 Language Server Agent，移除 provider/session
  configuration，并保留 semantic graph 作为本地 LSC/LSP implementation detail。
- Revision 16，2026-07-26：加入持久化逐 input manifest；正常 semantic graph validation 会 stat
  所有配置 input，但只读取和 hash 新增或变化文件；加入可观察 validation count 与 dependency
  layer 并行完整分析，同时明确 cache 失效后的 semantic analysis 仍是完整 workspace analysis。
- Revision 15，2026-07-26：要求 analysis 前验证 semantic graph、惰性启动 language server、
  根据 input 自动替换、提供 cache status，并加入显式完整 `osr lsc cache rebuild` 恢复入口。
- Revision 14，2026-07-26：加入可删除的 libSQL project semantic graph、graph-backed LSC
  复合查询、workspace URI 隐私与 compiler-owned language-service evidence。
- Revision 10，2026-07-25：把 embedded Python provider handle 规定为 module-private
  implementation detail，并要求 consumer reachability 从 validated Osiris `extern` binding
  开始。
- Revision 9，2026-07-25：区分 author-facing package 与 extension discovery，以
  `init --package` 替换 `init --extension`，规定 consumer-owned linkable backend closure，
  并允许按 module-level reachability 链接 explicit embedded `python` module；external Python
  dependency 继续保持普通 package。
- Revision 8，2026-07-25：要求生成 Python 保留 fallback docstring 与可读名称，并在不把
  standard macro 重建进 compiler 的前提下移除安全的结构样板。
- Revision 7，2026-07-25：定义有界的 project-local artifact cache，并要求在 Python
  hash/source map 生成前使用嵌入 compiler、固定版本的 Ruff formatter。
- Revision 6，2026-07-23：要求 wheel map hash-validated 引用 packaged source、使用标准
  PEP 660 editable wheel，并采用带确切 ABI metadata 的当前 minor pre-stable scaffold
  dependency range。
- Revision 5，2026-07-23：要求每个 generated Python distribution 把 reachable standard
  support 链接到 private `__osiris_runtime__`，并删除 deployed `osiris` runtime dependency。
- Revision 4，2026-07-23：把 `displayLocale` 定义为 LSP project fallback；LSC 在没有
  显式 locale 时继续使用 authored default，并要求标准 BCP 47 locale identifier。
- Revision 3，2026-07-23：采用 `osr lsc` 作为本地 compiler tooling 的 Language Server
  Console 入口。
- Revision 2，2026-07-23：与 canonical formatting、完整英文文档发布、raw GraphQL access
  和 local inspect CLI 对齐。
- Revision 1，2026-07-23：初始草案。
