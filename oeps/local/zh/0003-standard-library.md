---
oep: 3
title: 标准库架构与初始 API
description: Osiris 标准库的公开命名空间、phase 边界、初始 function/macro、runtime contract、artifact 与工具要求。
author: MJ
status: Draft
type: Standards Track
areas:
  - Language
  - Standard Library
  - Packaging
  - Tooling
created: 2026-07-23
updated: 2026-07-24
revision: 10
language: zh-CN
source: ../../0003-standard-library.md
source-revision: 10
translation-status: Current
requires: [0, 1, 2]
replaces: []
superseded-by: null
resolution: null
---

# OEP-0003：标准库架构与初始 API

本翻译不是规范性来源。[英文原文](../../0003-standard-library.md)是唯一规范源。

## 摘要 (Abstract)

本提案定义 Osiris 标准库架构和公开 contract。它分开 compile-time bootstrap、面向用户
的 `osiris.core` namespace、显式 standard module 与 private Linkable helper；
同时定义 macro phase boundary、隐式 core referral 与显式 override、sequence semantic、type/contract
visibility、distributed artifact、localization metadata 与 tooling behavior。

它还固定首个 callable/macro catalog。仅列名称不是 API contract：本提案规定初始 core、
collection、sequence、string、math、concurrency 和 Python interoperation surface 的 source
call shape、evaluation strategy、return family、edge behavior、effect 与 ownership。

实现严格分为两个 ownership layer。Kernel 是 compiler 内嵌的最小 form、intrinsic、
Phase-1 primitive 与 target helper 集合，只承载没有 privileged compiler/target access
就无法实现的能力。公开标准库是由该 Kernel 自举的普通 Osiris 源码包。Public macro、
普通函数、签名和 Rich Metadata 都在 `.osr` 中编写；reachable target support 编译到每个
consuming output。

## 动机 (Motivation)

Osiris 既需要小型 compiler，也需要有表达力的数据中心语言。把每个便利能力加入 Rust
会扩大复杂度、在 compiler/LSP 重复逻辑并模糊 extension boundary；把所有 operation
放入 Python runtime 又会使标准库难以 type、inspect、translate 和跳转源码。

规范化标准库应让程序简洁，同时保持 macro expansion 确定且 sandboxed、runtime
function 为 first-class value、solver 可见展开语义、Python 输出可读、API 暴露稳定
`.osri` identity/双语 metadata，并使 Pandas、Polars、NumPy 和领域行为保持普通扩展。

## 范围 (Scope)

本提案规定标准库分层与 public namespace boundary、standard macro 可用 compile-time
subset、macro dependency/output restriction、初始 public module 和 `osiris.core`
显式 core import、lazy/eager sequence 命名、公开 type/effect/contract/metadata/`.osri` 要求，
以及 distribution/loading/version/conformance，并规定初始 public macro/function inventory
与其可观察行为。

它不规定 internal Rust layout、evaluator algorithm、Python helper file、optimization、
特定 lazy data structure、DataFrame/Array/Series/axis/schema/领域 API、PyPI/uv 之外的
package resolution、custom reader/plugin，或由后续 OEP 加入的 standard module API。

## 术语 (Terminology)

- **Kernel**：普通 Osiris source 缺少 privileged compiler/target access 时无法实现的、
  封闭且最小的 compiler-owned form、intrinsic、Phase-1 primitive 与 target helper；它不
  是 public namespace。
- **Standard source package**：包含所有 public `osiris.*` namespace、wrapper、macro、
  signature 与 localized Rich Metadata 的普通 Osiris package；由 Osiris 编译并携带
  authored `.osr` source 分发。
- **Phase 1**：在 syntax data 上运行 macro/helper 的确定性 compile-time evaluation。
- **Phase 0**：降低为 typed HIR 与 generated Python 的普通 runtime semantic。
- **Bootstrap library**：user-facing standard macro 加载前可用的最小 Phase-1 function。
- **Core namespace**：公开 `osiris.core` 及经审核、可显式 import 的 binding 集合。
- **Standard module**：随标准库发布的公开 `osiris.*` namespace。
- **Implementation namespace**：类似 `osiris.core.kernel` 的层级 namespace，用来组织
  public facade 背后的源码分发 implementation contract。Namespace 层级只表达组织，
  不表达 binding privacy。
- **Linkable helper**：direct lowering 不足时，用于生成 distribution-private Python
  implementation 的 compiler-owned target-neutral HIR。
- **Intrinsic**：typed HIR 已知 type/effect/temporal/data/control-flow contract 的版本化
  operation。
- **Core export manifest**：module import mechanism 可用的版本化 public
  `osiris.core` binding 列表。
- **Facade identity**：不依赖内部源码组织的稳定 public binding identity。
- **Generated support package**：根据 reachable Linkable helper 在一个 output package 内
  生成的 reserved `__osiris_runtime__` Python package。

## 规范 (Specification)

### 标准库分层与 ownership

**OEP-0003-R001：** 实现必须恰好包含两个 standard ownership layer：compiler 内嵌
Kernel 与源码分发的 standard package。面向用户的 package 必须公开 `osiris.core`。
Compiler-owned Kernel/Bootstrap operation 不是 public user API；源码分发的
implementation namespace 必须从 public standard namespace/API catalog 中省略。

**OEP-0003-R002：** Generated Python 不得依赖 shared `osiris.prelude`、`osiris` 或
`osiris-lang` runtime package。Standard macro/function/intrinsic 必须直接降低为 ordinary
Python，或把 reachable support implementation 链接到 consuming distribution 的 private
`__osiris_runtime__`。

**OEP-0003-R003：** Kernel 必须内嵌在 native compiler。标准库必须作为
`osiris-lang` distribution 内的普通 Osiris package resource 构建和发布，并完整保留
authored `.osr` source tree。Generated `.osri`、macro IR、source index 与 Linkable
artifact 必须可由 matching compiler 从这些 source 重现。生成的 Python 必须在 consuming
output 内 self-contained，runtime 不得需要任何 Osiris distribution。

**OEP-0003-R004：** 标准库必须保持 domain-neutral。Pandas、Polars、NumPy、DataFrame
schema、量化规则和其他 framework/domain contract 必须由 PyPI extension 提供，不得被
标准库或 Rust Kernel 识别。

**OEP-0003-R005：** Convenience API 必须实现在能保留其 semantic 的最高层，依次为
hygienic macro、普通 Osiris function、ordinary Python lowering、Linkable helper、typed
intrinsic，最后才是新 Kernel form。

**OEP-0003-R005A：** 每个供标准库使用的 Kernel operation 必须由普通 public standard
binding 包装，或由 standard macro 私有消费。Kernel spelling、module layout 与 helper
identity 不得出现在 public signature/user import 中。Public wrapper 必须携带完整 Rich
Metadata，包括 authored `:doc` `:default` 与 `"zh-CN"` translation。

**OEP-0003-R005B：** Kernel declaration 必须遵循最小 metadata 原则，只携带正确编译
必需的 identity、phase、type、effect、intrinsic/linker contract 与 provenance。它不得携带
localized name、localized documentation、`^:doc` 或 convenience metadata。Kernel
diagnostic 是 OEP-0001 管理的 compiler diagnostic，不是 API documentation record。

**OEP-0003-R005C：** Public standard declaration、signature、Rich Metadata 和
implementation body 必须以分发的 `.osr` source 为 normative source of truth。Rust table、
generated facade、Python template 或 checked-in `.osri` 不得独立定义 public standard API。
Generated artifact 可以缓存或内嵌以优化启动，但 release build 必须根据 packaged source
验证并拒绝 stale/divergent artifact。

**OEP-0003-R005D：** 包装 Kernel leaf 的 public callable/value 必须是 standard source
package 中手写的普通 `defn`/`def`；public declaration 自身不得是 `extern`。Leaf 的 typed
`extern python "osiris.kernel"` declaration 必须位于层级 implementation namespace
`<public-namespace>.kernel`。每个 public standard module 必须用 `:refer :all` import 该
namespace；public facade 不得直接包含 `extern python "osiris.kernel"` boundary。Declaration
只能使用 R005B 规定的最小 Kernel metadata，并且必须由 public Osiris implementation body
引用。因此 facade 的编译与链接必须经过普通 Osiris frontend、type checker、HIR 和 Python
backend，不得在 Rust 中定义 public API。

**OEP-0003-R005E：** 一个 standard-library source file 必须只声明一个 module，一个
module 也必须只有一个 normative source file。禁止通过多个文件重复 `(module name)` 来拼接
module。大型实现必须用层级 namespace 与普通 import 拆分。Declaration 移入
implementation namespace 时，不得改变其 public facade identity。

Standard package 可以在 `osiris.core` module metadata 中声明
`:osiris/facade-modules`，列出编写其 Phase-0 declaration、Phase-1 helper 与 macro 的层级
implementation module；并且必须用 `:osiris/facade-macros` 精确列出 public macro 名称。
Standard artifact builder 可以根据这些 registered module 中解析后的 declaration span，组合
一个派生的 `osiris.core` compilation unit。这是 standard-package build rule，不是通用 module
re-export syntax。组合必须保留每个 authored declaration 的 body 与 metadata、赋予 public
`osiris.core` identity、拒绝未注册 module，并使每个 authored file 仍是其自身 declared
module 的唯一 normative source。

**OEP-0003-R005F：** Implementation namespace 必须携带 module metadata
`^{:osiris/internal true}`。它可以 export facade 或同级 implementation namespace 编译所需
的 binding，但这些 export 是 internal compilation contract：必须从 public standard
namespace/API catalog 中省略，不强制携带面向用户的 `:doc`，也没有 public compatibility
保证。在 package access-control OEP 规定 enforcement 之前，本 OEP 不要求 compiler 拒绝
外部 package 显式命名这种 namespace；catalog omission 与 compatibility status 定义边界。

**OEP-0003-R005G：** Osiris privacy 必须遵循 Clojure 的 binding model。Namespace
component、source path、前导/尾随下划线都不得表示 privacy。Namespace-private function
必须用 `defn-` 编写并记录 `:private true`；其他 private declaration 必须使用各自规定的
authored `:private true` form。禁止引入 `osiris._core` 之类的名字来编码 access control。
必须从 `osiris.core.kernel` 跨 namespace 提供给 `osiris.core` facade 的 binding 不能是
`defn-`；它应按 R005F 从 internal namespace export。

### Phase-1 Bootstrap contract

**OEP-0003-R006：** Standard macro implementation 必须只在 package macro 使用的同一
sandboxed Phase-1 evaluator 中执行。Compiler 不得为 standard macro 提供第二套
privileged expansion engine。

**OEP-0003-R007：** Bootstrap library 必须为 `None`、`Bool`、`Int`、有限 `Float`、
`Str`、`Keyword`、`Symbol`、`List`、`Vector`、`Map`、`Set`、`Syntax`、`Span`、
`Metadata` 提供不可变 syntax-data operation。

**OEP-0003-R008：** Bootstrap library 必须提供足够的 pure operation 来 construct、
inspect、traverse、combine、annotate syntax，包括 sequence construction/access、
`apply`、`map`、`reduce`、`gensym`、metadata/span access 与 structured syntax error。

**OEP-0003-R009：** Phase 1 禁止暴露 Python import/call、file/network access、
environment variable、clock、randomness、subprocess、thread、runtime dynamic binding、
project mutation 或 inferred Phase-0 type/effect result。

**OEP-0003-R010：** Phase-1 execution 必须对 expansion count、evaluation step、call
depth、result node 和 metadata resource 强制确定限制。同一 source/interface/option 必须
产生相同 expanded syntax 或相同 deterministic diagnostic。

**OEP-0003-R011：** Standard Phase-1 dependency 必须形成 acyclic graph，依次从 Kernel
到 Bootstrap helper、core binding/control macro、higher standard macro、extension
macro。Runtime SCC support 不得放宽 Phase-1 acyclicity。

### Macro output boundary

**OEP-0003-R012：** Macro implementation 必须终止于 Phase-1 Kernel form 与 Bootstrap
function；其 expanded result 必须终止于 Phase-0 Kernel form、普通 callable binding 或
typed intrinsic。

**OEP-0003-R013：** Standard macro 可以生成 expression 和 declaration macro 已允许的
受控声明，包括 `def`、`defn`、`defstruct`、`extern`、`py/decorate`、`static-record`。

**OEP-0003-R014：** Standard macro 禁止生成或修改 `module`、`import`、
`import-for-syntax`、`py/import`、`export`、`alias`、`defmacro`、`defn-for-syntax`、
`defstatic-schema`，因为它们在 expansion 前建立 module/phase/public-binding/static-schema
graph。

**OEP-0003-R015：** Standard macro 可以生成 intrinsic 或 Linkable helper call，但
Phase 1 不得执行。Macro expansion 必须保持 contract 承诺的 single-evaluation、
evaluation order、lexical scope、metadata policy 和完整 macro-origin chain。

### Public namespace

**OEP-0003-R016：** 初始 stable namespace set 必须包含：

```text
osiris.core
osiris.collection
osiris.sequence
osiris.string
osiris.math
osiris.concurrent
osiris.python
```

Pre-stable release 可以分阶段实现，但全部 namespace 一致前不得把本 OEP 标为 Final。

**OEP-0003-R017：** Public binding 必须使用不随 private helper module/source file 移动
而变化的 facade identity。Public `.osri` signature 不得暴露 private bootstrap 或
implementation binding。

**OEP-0003-R018：** `osiris.collection` 与 `osiris.sequence` 必须操作 logical Osiris
collection/sequence protocol，不得对 Pandas、Polars、NumPy runtime type 分支。

**OEP-0003-R019：** `osiris.python` 必须在 source/typed HIR 中明确标识 dynamic Python
boundary，不得提供 compile-time reflection escape hatch 或隐式 import 任意 Python module。

### 隐式 core referral contract

**OEP-0003-R020：** 没有显式 `osiris.core` import 的普通 Osiris module 必须自动
获得 public `osiris.core` surface，其语义等价于隐式 `:refer :all`。该规则必须一致
适用于 runtime binding、type 和 phase-1 macro，保留每个 binding 的 canonical
`osiris.core` identity，且不得产生对 compiler 或 standard package 的 runtime import。

**OEP-0003-R021：** 初始 core export manifest 必须包含这些 macro binding：

```text
and or
when when-not if-not
cond case condp
if-let when-let if-some when-some when-first
-> ->> some-> some->> cond-> cond->> as-> doto
defn- letfn loop recur for forv doseq dotimes while trampoline
lazy-seq lazy-cat delay force deref realized?
binding with-open assert throw comment time
```

**OEP-0003-R022：** 初始 core export manifest 必须包含这些普通 function binding：

```text
identity constantly comp partial juxt complement
map mapv mapcat mapcatv filter filterv remove removev
reduce fold reductions reduced reduced? unreduced
first rest next nth count empty empty? seq seq? coll? sequential?
cons concat range repeat repeatedly iterate cycle sequence
take drop take-while drop-while take-last drop-last
partition partition-all partition-by interleave interpose
distinct dedupe flatten keep keep-indexed map-indexed
run! doall dorun some every? not-every? not-any?
get assoc dissoc update get-in assoc-in update-in select-keys
nil? some? true? false?
number? string? list? vector? map? set? sequence?
```

初始 core export manifest 还必须包含以下 nominal protocol type binding，确保公开签名
不会引用 private 或缺失的 type：

```text
Reduced Future Promise Delay Lock
```

在 runtime language 拥有不同 Keyword/Symbol value 前，`keyword?`/`symbol?` 必须保持为
Bootstrap/Phase-1 predicate。Target representation 已擦除该 identity 后，它们不得伪装成
仍能区分两者。

**OEP-0003-R023：** 增加、删除、重命名 core export 或改变其 identity 是
semantic compatibility change，必须改变 standard-library semantic hash。

**OEP-0003-R024：** 显式 `osiris.core` import 完全覆盖隐式 referral policy。规范性的
可配置形式是：

```clojure
(import osiris.core
  :refer :all
  :exclude [map]
  :rename {reduce fold-left})
```

`:refer :all` import 所有 public core export；省略的 `:exclude`/`:rename` 等同于空集合。
`:exclude` 在创建 local name 前删除 canonical export；`:rename` 把剩余 canonical export
映射到 local name。Unknown、duplicate、同时 exclude/rename 或 local collision 的 name 必须
产生确定 diagnostic。这些 clause 必须在普通 name lookup 前解析，使 binding identity 不依赖
import order。

没有显式 core import 时，同名 local declaration 或 local macro 必须只遮蔽该隐式 referral
spelling；qualified `osiris.core/name` lookup 仍必须可用。显式 core import 保留普通
import collision rule，并必须对冲突的 local name 报错。需要限制 surface 的 module
必须使用显式 `:refer`，或使用 `:refer :all` 配合 `:exclude`/`:rename`。多个显式
runtime `osiris.core` import 仍属非法，不得自动合并。

保留的 standard-package bootstrap namespace（`osiris.core` 以及本 OEP 列出的 standard
namespace 和 implementation descendant）必须显式编写其 core dependency。Compiler 在构建
embedded standard interface 时不得向它们提供隐式 core referral；该例外用于避免
interface initialization cycle，普通 package 不能使用该例外。

### 初始 module contract

**OEP-0003-R025：** `osiris.collection` 必须提供 coherent associative-data 初始操作集，
包括 `merge`、`merge-with`、`group-by`、`frequencies`、`index-by`、`select-keys`、
`rename-keys`、`update-keys`、`update-vals`、`zipmap`、`invert`。

**OEP-0003-R026：** `osiris.sequence` 必须提供 coherent sequence producer/transform/
consumer 初始集，包括 `range`、`repeat`、`repeatedly`、`iterate`、`take`、`drop`、
`take-while`、`drop-while`、`partition`、`partition-all`、`partition-by`、`interleave`、
`interpose`、`distinct`、`dedupe`、`flatten`、`mapcat`、`keep`、`keep-indexed`、
`map-indexed`。

**OEP-0003-R027：** `osiris.string` 必须提供 deterministic Unicode-string subset，
包括 trim、`split`、`join`、`replace`、prefix/suffix/inclusion predicate、basic case
conversion、`blank?` 与 line splitting。Locale-sensitive/regex behavior 必须说明 Python
target dependency，或留在初始 module 之外。

**OEP-0003-R028：** `osiris.math` 初始必须保持 scalar API。Array broadcasting、
missing-value policy、vectorization 和 framework-specific NaN normalization 必须由扩展负责。

**OEP-0003-R029：** `osiris.concurrent` 必须组合由 Linkable helper 生成到
`__osiris_runtime__` 的 versioned Future、Promise、dereference、cancellation、lock 与
parallel-map semantic contract，不得依赖 global scheduler package 或隐藏 thread、timeout、
exception、cancellation boundary。

### 宏、函数与 intrinsic

**OEP-0003-R030：** 当 expansion 无需新 Kernel rule 即可保持 semantic 时，syntax
organization 和 source-level evaluation control 应使用 hygienic macro。

**OEP-0003-R031：** 用户可能 pass、store、return 或动态选择的 runtime data operation
必须是普通 first-class Osiris function。`map`、`filter`、`reduce`、`fold` 不得只作为宏。

**OEP-0003-R032：** Macro-specific optimized path 只有在保持普通 function 的可观测
evaluation count/order、exception behavior、typing、effect、temporal 与 data-property
contract 时才可以存在。

**OEP-0003-R033：** Solver 必须通过 stable binding identity、ordinary signature、
operator/protocol instance 和 versioned intrinsic contract 识别标准操作。禁止从
unqualified function-name string 推断 semantic。

**OEP-0003-R034：** Standard macro 不得自行授予 verified type、effect、temporal、
data-property、trust 或 availability fact。这些事实必须来自 expanded typed HIR 和
validated contract。

### Sequence semantic

**OEP-0003-R035：** `map`、`filter`、`remove`、`take`、`drop` 必须返回 logical lazy
sequence；`mapv`、`filterv` 必须返回 eager vector；`reduce`、`fold`、`count`、`group-by`
必须是 eager consumer。

**OEP-0003-R036：** `reduce` 与 `fold` 必须支持 `Reduced T` early termination protocol。
Reduced marker 只能终止最近 compatible reduction，且不得把 public result type 从 `T`
改为其他类型。

**OEP-0003-R037：** Multi-input mapping 与对应 zip-like operation 必须在最短 input
结束，除非独立 API 显式规定 padding 或严格等长。

**OEP-0003-R038：** 所有 standard operation 的 lazy-sequence realization/cache semantic
必须统一。Lazy sequence 不得隐藏 file/network access、thread creation 或未表示的 external
effect。

**OEP-0003-R039：** List、Vector、Map、Set、Sequence 的 logical semantic 必须独立于
codegen 选择的具体 Python container。Standard function 不得通过检查 private Python
container implementation 建立第二套 type system。

### Type、effect 与 contract

**OEP-0003-R040：** 每个 public ordinary function 必须发布完整稳定 `.osri` signature。
Public higher-order function 必须保留 callback parameter/result type、latent effect、
temporal summary、data property 与相关 collection-shape relation。

**OEP-0003-R041：** Standard macro 必须让 typed HIR 看见结果 operation。宏不得在
metadata 或 opaque helper 后隐藏 IO、state、time read、alignment change、materialization、
shape change 或 Python dynamic boundary。

**OEP-0003-R042：** Linkable helper/intrinsic 必须有显式 versioned compiler-input/
solver contract，不得建立 deployed shared runtime ABI。Unknown Python behavior 必须保持
`Any` 和 unknown-effect/unknown-data，除非独立 validated contract 证明更多事实。

### Rich Metadata 与 interface data

**OEP-0003-R043：** 每个 public standard binding 必须在其 authored `.osr` declaration
中提供 normalized Rich Metadata，至少包含 English `:default`、`zh-CN` tagged
documentation entry、category、首个 `since`
version、deprecation state 与 stable source location。Callable binding 还必须提供 public
argument information。Documentation/localized name 必须使用 OEP-0001-R057 到
OEP-0001-R065 的 metadata contract；鼓励增加其他 BCP 47 translation，但不得改变
binding identity。

**OEP-0003-R044：** Standard metadata 可以提供 localized name、example、Agent intent
与 Agent tag。Authored metadata 不得制造 inferred/verified fact。

**OEP-0003-R045：** Standard `.osri` 必须包含 public signature、macro signature、
validated macro IR、所需 private Phase-1 helper closure、Rich Metadata、facade identity、
source location 和 semantic/tooling hash。

### Distribution、loading 与 versioning

**OEP-0003-R046：** `osiris-lang` release 必须使用 OEP-0002 package layout，把标准库
作为普通 source distribution resource 打包。它必须包含 `pyproject.toml`、`osiris.jsonc`、
所有 public `.osr` source 和 `osiris_build` 所需 metadata。该 package 是发布 reusable
Osiris source 的 normative example。Generated standard Python 属于 consuming output，
不属于 shared `osiris-lang` runtime package。

**OEP-0003-R047：** Standard artifact 和 linked support 必须可从 packaged normative
`.osr` 与 compiler-owned Kernel source 重现。相同 compiler、source、interface、Python
target、reachable binding set、option 的两次 build 必须产生相同 semantic interface，并在
格式承诺确定性时产生 byte-identical artifact。

**OEP-0003-R048：** Compiler loading order 必须为：

```text
Phase-1 Bootstrap
-> packaged standard source and its validated interface cache
-> explicitly imported standard modules
-> uv.lock-reachable extension interfaces
-> project modules
```

后层不得修改前层 interface 或 core export manifest。

**OEP-0003-R049：** User project 不得声明 standard-library 或 Osiris runtime dependency。
作为 build tool 安装 matching `osiris-lang` 必须提供 compiler、packaged standard source、
validated generated artifact cache 和 build backend。完成的 Python output 在删除
`osiris-lang`/`osr` 后必须仍可运行，
仅依赖 project 的普通 declared Python dependency。

**OEP-0003-R050：** `.osri` 和 build metadata 必须标识 compiler ABI、language ABI、
standard-library ABI、Linkable-helper format 与 Python target。不匹配必须在 macro
execution/codegen 前失败。Support 已链接到 output 后不得要求 runtime ABI negotiation。
本 OEP 把初始 standard-library ABI 定义为整数 `1`。

**OEP-0003-R051：** Macro behavior、public signature、evaluation order、laziness、
exception behavior、core export 或 solver-visible contract 的改变必须影响 semantic
hash。仅 documentation translation/display ordering 必须只影响 tooling hash。

## 理由 (Rationale)

`osiris.core` 为用户提供传统 language-level namespace，linker 则把 reachable
implementation 变成 ordinary distribution-private Python。分开 compile-time facade identity
与 generated support name，可以防止 helper layout 成为 Osiris API 或 external runtime
dependency。

宏与函数按 semantic boundary 划分。`when`、threading、comprehension 控制 source
structure/evaluation，适合宏；`map`、`filter`、`reduce` 是 higher-order value，必须保持函数。

在同一 release 中发布 standard source package 与 compiler，可以避免 macro IR、interface
schema、Linkable-helper format、solver contract 的不支持组合，同时保持语言实现可检查。
把 compiled standard source 和 Kernel helper 链接到每个 consuming output，会以少量
tree-shaken 重复换取 standalone deployment，并删除 cross-distribution runtime conflict。
初始 module 仍聚焦通用数据转换，不让语言识别具体 tabular framework。

## 向后兼容 (Backwards Compatibility)

Osiris 尚未 stable，因此本提案删除 user-facing 和 generated-Python 的
`osiris.prelude` dependency，不保留 compatibility alias。Existing pre-release output 必须
重新 build；保留 shared runtime 会留下本提案要删除的 deployment coupling。

`map`/`filter` 采用 lazy semantic 可能与已有 eager helper 不同；实现 release 必须说明
影响并在移除旧行为前提供显式 eager form。显式 import 可能产生名称 collision；首次实现
必须通过 OEP-0003-R024 的 import contract 确定性诊断。

## 安全与确定性 (Security and Determinism)

标准库与 installed extension 使用相同 macro trust boundary。Embedded/packaged macro IR
在 format、ABI、semantic hash、resource limit 与 dependency identity 校验前是不可信输入。

Compilation 不得 import/执行 standard-library Python module 来 discovery macro/type/
metadata/contract/interface；输入只能是 static source、`.osri`、versioned manifest 或
validated compiler-owned data。Lazy operation 必须暴露 effect，不得把 repeated realization
静默变成 repeated external IO。Linked support 必须保持 evaluation/exception order，且
runtime 不得 discovery/load compiler data。加载必须在 expansion/codegen 前拒绝缺失、
重复、不匹配、path-escaping 或篡改的 standard resource。

## 工具与 AI 使用 (Tooling and AI Usage)

**OEP-0003-R052：** LSP 与 semantic-inspection API 必须区分 Bootstrap helper、standard
macro、ordinary standard function、Linkable helper 和 user/extension binding。

**OEP-0003-R053：** Tooling 必须支持从 standard binding 跳转到 distributed `.osr`
source，并从 `.osri` metadata 提供 localized hover、completion、signature help。

**OEP-0003-R054：** Macro tooling 必须同时保留 folded standard-macro operation 与其
expanded primitive view，并提供完整 origin chain/source map。

**OEP-0003-R055：** AI-facing semantic data 必须将 stable binding identity、module、
phase、public signature、authored metadata、declared contract、verified fact 与 macro
origin 作为独立 field 暴露。本地化名称不得替换 canonical identity。

**OEP-0003-R056：** Conformance test 与 implementation work item 应引用本 OEP 的
requirement ID。AI Agent 实现 public standard-library contract 前必须检查 OEP status 与
Open Questions。

## 初始 Public API 清单

### 记号与共同语义

**OEP-0003-R057：** 本节表格是规范要求。`A`、`B`、`K`、`V` 表示 type variable；
`Seq[A]` 表示可重复遍历的逻辑 sequence；`LazySeq[A]` 表示延迟、memoized sequence；
`Coll[A]` 表示逻辑 collection；`Assoc[K, V]` 表示 associative data。`form...`、
`body...`、`coll...` 表示零个或多个源码 operand。这些只是 contract notation，不是新增
source type constructor。

所有 callback 必须从左到右调用，并传播 exception 与 solver-visible summary。Lazy
operation 只能按需求值 callback，并且必须记忆已经 realized 的 value/exception。Eager
operation 必须在返回前完成文档规定的输入消费。除非表格另有规定，`none` 视为空
sequence，负 count 选择空 prefix；静态已知的 type/arity 错误产生确定 typed diagnostic，
`Any` boundary 则产生文档规定的 runtime exception。

### Core 宏

**OEP-0003-R058：** 可 import 的 core macro 必须提供以下源码形式和能力：

| Binding | Source shape | 必需行为 |
| --- | --- | --- |
| `and`、`or` | `(and form...)`、`(or form...)` | 按 Clojure truthiness 从左到右短路；零参数分别返回 `true`/`none`；返回被选中的 operand value。 |
| `when`、`when-not`、`if-not` | `(when test body...)`、`(when-not test body...)`、`(if-not test then [else])` | `test` 只求值一次；缺少的 branch result 是 `none`。 |
| `cond` | `(cond test result ... [:else result])` | 按顺序测试 pair；没有匹配且没有 `:else` 时返回 `none`。 |
| `case` | `(case value test result ... default)` | `value` 只求值一次；literal test 唯一，list 表示一组 constant，final default 必填。 |
| `condp` | `(condp pred value test result ... :else result)` | `pred`/`value` 各求值一次，按顺序测试并要求 `:else`；`test :>> handler` 接收成功 predicate result。 |
| `if-let`、`when-let` | `(if-let [pattern value] then [else])`、`(when-let [pattern value] body...)` | `value` 只求值一次，按 Clojure truthiness 决定是否 binding。 |
| `if-some`、`when-some` | `(if-some [pattern value] then [else])`、`(when-some [pattern value] body...)` | 除 `none` 外都 binding，必须保留 `false`。 |
| `when-first` | `(when-first [pattern coll] body...)` | `coll` 只求值一次，非空时 binding first item，lazy probe 不得丢失该 item。 |
| `->`、`->>` | `(-> value step...)`、`(->> value step...)` | 把 prior value 插到第一个或最后一个 call argument。 |
| `some->`、`some->>` | `(some-> value step...)`、`(some->> value step...)` | 同上，但只在 `none` 时停止；`false` 继续。 |
| `cond->`、`cond->>` | `(cond-> value test step ...)`、`(cond->> value test step ...)` | 按顺序判断 test，并对 single-evaluated accumulator 条件 threading。 |
| `as->` | `(as-> value name form...)` | 每一步把 prior result 绑定到 `name`。 |
| `doto` | `(doto value call...)` | `value` 只求值一次，作为每个 call 的第一参数，最后返回原值。 |
| `defn-` | `(defn- name params body...)` | 产生 non-exported `defn` 并携带 authored `:private true`。 |
| `letfn` | `(letfn [(name params body...) ...] body...)` | 预声明全部 local function，支持 single-arity self/mutual recursion。 |
| `loop`、`recur` | `(loop [pattern init ...] body...)`、`(recur value...)` | Initializer 各求值一次；`recur` 指向最近 lexical loop 或当前函数，只允许 tail position，并检查 arity/type，以常量栈运行。 |
| `for`、`forv`、`doseq` | `(for [clauses...] body...)`、`(forv [clauses...] body...)`、`(doseq [clauses...] body...)` | 支持多 binding、`:let`、`:when`、`:while`；`for` 返回 memoized LazySeq，`forv` 返回 eager Vector，`doseq` 执行 effect 后返回 `none`，顺序均按嵌套规则。 |
| `dotimes`、`while` | `(dotimes [name count] body...)`、`(while test body...)` | Count 只求值一次并迭代 `0..count-1`；或每轮重新求值 `test`；返回 `none`。 |
| `trampoline` | `(trampoline f arg...)` | 反复调用返回的零参数 callable，直到得到非 callable result。 |
| `lazy-seq`、`lazy-cat` | `(lazy-seq body...)`、`(lazy-cat coll...)` | 延迟并记忆 sequence production；拼接输入时不得 eager realization。 |
| `delay`、`force`、`deref`、`realized?` | `(delay body...)`、`(force value)`、`(deref value [timeout-ms timeout-value])`、`(realized? value)` | 提供一次性 memoized evaluation 和共享 dereference protocol，value/exception 都缓存。 |
| `binding` | `(binding [dynamic-var value ...] body...)` | 按顺序求值 override，安装 context-local dynamic value，并在任何 exit path 恢复旧值。 |
| `with-open` | `(with-open [name resource ...] body...)` | 按源码顺序获得 resource，用嵌套 `finally` 逆序关闭非 `none` resource。 |
| `assert`、`throw`、`comment`、`time` | `(assert test [message])`、`(throw value)`、`(comment form...)`、`(time body...)` | 提供稳定 assertion/exception；Phase 1 丢弃 comment；用 monotonic clock 计时至少一个表达式并返回 final value。 |

`for`/`doseq` 的每个 binding collection 只能求值一次。`:when` 跳过一个 candidate；
`:while` 只停止词法上最近的 collection。Destructuring 不得改变这些规则。

### Core function 与 predicate

**OEP-0003-R059：** Core functional/predicate API 必须提供：

| Binding | Logical call/result | 必需行为 |
| --- | --- | --- |
| `identity` | `(identity value) -> A` | 返回原值。 |
| `constantly` | `(constantly value) -> Fn[Any..., A]` | 返回忽略参数的函数。 |
| `comp` | `(comp f...) -> Fn` | 从右到左组合；零函数等价 `identity`。 |
| `partial` | `(partial f arg...) -> Fn` | 捕获 leading argument 一次，调用时追加新 argument。 |
| `juxt` | `(juxt f...) -> Fn[..., Vector[Any]]` | 用相同参数从左到右调用所有函数。 |
| `complement` | `(complement pred) -> Fn[..., Bool]` | 对 predicate result 的 Clojure truthiness 取反。 |
| `nil?`、`some?` | `A -> Bool` | 只测试 `none`；`some?` 是反值。 |
| `true?`、`false?` | `A -> Bool` | 测试精确 Bool，不用 general truthiness。 |
| `number?` | `A -> Bool` | 识别 `Int`/`Float`，排除 `Bool`。 |
| `string?`、`list?`、`vector?`、`map?`、`set?` | `A -> Bool` | 测试对应 logical runtime type。 |
| `sequence?`、`seq?`、`coll?`、`sequential?` | `A -> Bool` | 测试文档规定的逻辑 family，不检查 private Python implementation。 |

Phase-1 `keyword?`/`symbol?` 必须测试不同 syntax datum。在 Keyword/Symbol 没有不同
runtime representation 前，Phase 0 API 不得宣称仍可区分它们。

### Associative collection

**OEP-0003-R060：** `osiris.core` 与 `osiris.collection` 必须提供以下 associative
operation。R022 所列 row 可以由 core facade-refer；其余 canonical definition 属于
`osiris.collection`。

| Binding | Logical call | 必需行为 |
| --- | --- | --- |
| `get` | `(get assoc key [not-found])` | 返回 value 或 `none`/显式 fallback，不插入。 |
| `assoc` | `(assoc assoc key value ...)` | 返回新值，pair 从左到右应用。 |
| `dissoc` | `(dissoc assoc key...)` | 返回移除 key 的新值。 |
| `update` | `(update assoc key f arg...)` | 用 current value 或 `none` 加 `arg...` 调用 `f`。 |
| `get-in` | `(get-in assoc keys [not-found])` | 逐 key traverse，首个缺失 path 返回 fallback。 |
| `assoc-in` | `(assoc-in assoc keys value)` | 创建缺失 map node；空 keys 替换 root。 |
| `update-in` | `(update-in assoc keys f arg...)` | 组合 `get-in`、单次 callback 与 `assoc-in`。 |
| `select-keys` | `(select-keys assoc keys)` | 保留 requested key order，省略 missing key。 |
| `merge` | `(merge assoc...)` | 从左到右 merge，后值覆盖；`none` 是 empty map。 |
| `merge-with` | `(merge-with f assoc...)` | 用 `f` 从左到右组合 duplicate value。 |
| `group-by` | `(group-by f coll)` | Group 内保持 input order，key 保持 first encounter order。 |
| `frequencies` | `(frequencies coll)` | 按 Osiris equality 计数。 |
| `index-by` | `(index-by f coll)` | Duplicate derived key 必须报错，不得静默丢数据。 |
| `rename-keys` | `(rename-keys assoc renames)` | 只 rename existing key，并拒绝 collision。 |
| `update-keys`、`update-vals` | `(update-keys f assoc)`、`(update-vals f assoc)` | 按确定 iteration order transform；key collision 报错。 |
| `zipmap` | `(zipmap keys values)` | 按顺序配对，在较短输入结束。 |
| `invert` | `(invert assoc)` | 交换 key/value，并拒绝 duplicate output key。 |

以上函数不得修改输入。Equality/key 必须遵循逻辑 Osiris value，包括区分 `false` 与数值零。

### Sequence 与 reduction

**OEP-0003-R061：** 初始 sequence producer/transform contract 必须是：

| Binding | Accepted shape | Result/behavior |
| --- | --- | --- |
| `range` | `(range end)`、`(range start end [step])` | Lazy、end-exclusive numeric sequence；`step` 不能为零。 |
| `repeat` | `(repeat value)`、`(repeat count value)` | Infinite/finite lazy repetition；value 只求值一次。 |
| `repeatedly` | `(repeatedly f)`、`(repeatedly count f)` | 每个 realized item 调用一次零参数 `f`。 |
| `iterate` | `(iterate f initial)` | `initial`、`f(initial)` 等组成的 infinite lazy sequence。 |
| `cycle` | `(cycle coll)` | 重复 finite input；空输入仍为空。 |
| `sequence` | `(sequence coll)` | 返回 repeatable lazy view，不 eager materialize。 |
| `cons`、`concat` | `(cons value coll)`、`(concat coll...)` | Lazy prefix 和按源码顺序拼接。 |
| `map`、`mapcat` | `(map f coll...)`、`(mapcat f coll...)` | Lazy transform；多输入按最短停止；`mapcat` flatten 一层返回 sequence。 |
| `mapv`、`mapcatv` | 同上 | 具有同样 callback order 的 eager Vector variant。 |
| `filter`、`remove` | `(filter pred coll)`、`(remove pred coll)` | 按 Clojure truthiness lazy selection。 |
| `filterv`、`removev` | 同上 | Eager Vector variant。 |
| `keep`、`keep-indexed` | `(keep f coll)`、`(keep-indexed f coll)` | Lazy transform，只丢弃 `none`；indexed callback 先接收 zero-based `Int`。 |
| `map-indexed` | `(map-indexed f coll)` | Callback 先接收 index 再接收 item 的 lazy transform。 |
| `take`、`drop` | `(take n coll)`、`(drop n coll)` | Lazy prefix/suffix，只消费需要的 prefix。 |
| `take-while`、`drop-while` | `(take-while pred coll)`、`(drop-while pred coll)` | 以 Clojure truthiness 选择 lazy boundary。 |
| `take-last`、`drop-last` | `(take-last n coll)`、`(drop-last [n] coll)` | `take-last` 必须耗尽 finite input；`drop-last` 使用 bounded lookahead，默认一项。 |
| `partition` | `(partition n coll)`、`(partition n step coll)`、`(partition n step pad coll)` | Lazy Vector window；没有 `pad` 时省略不完整 tail。 |
| `partition-all` | `(partition-all n coll)`、`(partition-all n step coll)` | 包含不完整 tail 的 lazy Vector window。 |
| `partition-by` | `(partition-by f coll)` | 相邻 equal key 的 lazy run；每项只调用一次 `f`。 |
| `interleave`、`interpose` | `(interleave coll...)`、`(interpose separator coll)` | 按最短输入 lazy interleave 或插入 separator。 |
| `distinct`、`dedupe` | `(distinct coll)`、`(dedupe coll)` | 保留首次全局出现或删除相邻重复，并保持顺序。 |
| `flatten` | `(flatten value)` | 只递归 flatten sequential value；string/map/set 是 leaf。 |

Partition size/step 必须是正整数。Finite-count API 必须拒绝 `Bool`、fractional number 和
string，不能 coercion。

**OEP-0003-R062：** 初始 sequence consumer/reduction 必须提供：

| Binding | Accepted shape | 必需行为 |
| --- | --- | --- |
| `first`、`rest`、`next` | 各接收 `(binding coll)` | 分别返回 first-or-`none`、可为空 remainder、或没有 remainder 时的 `none`。 |
| `nth` | `(nth coll index [not-found])` | Zero-based lookup；没有 fallback 时越界抛 `IndexError`，有则返回 fallback。 |
| `count` | `(count coll)` | Eager count；`none` 为零。 |
| `empty` | `(empty coll)` | 可定义时返回相同 logical family 的 empty collection。 |
| `empty?` | `(empty? coll)` | Lazy input 最多 probe 一项并保留它。 |
| `seq` | `(seq coll)` | 空输入返回 `none`，否则返回 non-empty repeatable view。 |
| `reduce` | `(reduce f coll)`、`(reduce f initial coll)` | Ordered eager reduction；无 initial 的空输入调用 zero-arity `f`。 |
| `fold` | `(fold f initial coll)` | Ordered reduction 的精确 explicit-initial alias contract。 |
| `reductions` | `(reductions f coll)`、`(reductions f initial coll)` | Intermediate accumulator 的 lazy sequence。 |
| `reduced`、`reduced?`、`unreduced` | Marker 构造、测试、单层 unwrap | 只停止最近 reduction，对外结果是 `T` 而不是 `Reduced[T]`。 |
| `run!` | `(run! f coll)` | 按顺序为 effect 调用并返回 `none`。 |
| `doall`、`dorun` | `(doall [n] coll)`、`(dorun [n] coll)` | Realize 全部或 prefix；返回原 collection 或 `none`。 |
| `some` | `(some pred coll)` | 返回首个 truthy predicate result，否则 `none`。 |
| `every?`、`not-every?`、`not-any?` | `(predicate pred coll)` | 按 Clojure truthiness 短路。 |

### String 与 scalar math

**OEP-0003-R063：** `osiris.string` 初始必须公开：

| Binding | Logical call | 必需行为 |
| --- | --- | --- |
| `trim`、`trim-left`、`trim-right` | `(binding text)` | 删除两侧、左侧或右侧 Unicode whitespace。 |
| `split` | `(split text separator [limit])` | 按 literal non-empty `Str` 分割；正 `limit` 限制 result count；保留 empty field。 |
| `split-lines` | `(split-lines text [keep-ends?])` | 识别 Unicode/Python line boundary，可保留 line ending。 |
| `join` | `(join separator strings)` | 按 input order 拼接 `Str` item；拒绝非 string。 |
| `replace` | `(replace text old new)` | 替换全部 literal occurrence，不解释 regex。 |
| `starts-with?`、`ends-with?`、`includes?` | `(binding text fragment)` | 精确 code-point substring test。 |
| `lower`、`upper`、`capitalize` | `(binding text)` | Locale-independent Unicode case conversion。 |
| `blank?` | `(blank? text)` | Empty text 或仅 Unicode whitespace 时为真。 |

Case conversion/line boundary 必须使用 target Python version 的 Unicode database，并在
`.osri`/build data 记录 target。Regex 和 locale-sensitive collation 不在初始 API 中。

**OEP-0003-R064：** `osiris.math` 必须公开 scalar constant `pi`、`e`、`tau`、`inf`、
`nan`；单参数 scalar transform `abs`、`floor`、`ceil`、`trunc`、`sqrt`、`exp`、
`log10`、`sin`、`cos`、`tan`、`asin`、`acos`、`atan`；`(round value [digits])`、
`(log value [base])`、`(pow base exponent)`、`(atan2 y x)`；以及单参数 predicate
`finite?`、`infinite?`、`nan?`。Numeric promotion、domain error、NaN behavior、overflow
必须遵循 versioned scalar numeric contract 和 target Python，不得采用 framework array
coercion。Array dispatch 必须使用 extension-owned static operator/function instance。

### Concurrency 与显式 Python boundary

**OEP-0003-R065：** `osiris.concurrent` 必须公开：

| Binding | Source shape | 必需行为 |
| --- | --- | --- |
| `future`、`future-call` | `(future body...)`、`(future-call f)` | 提交一个零参数 task 并返回 `Future[A]`；传播捕获的 dynamic binding context。 |
| `future-done?`、`future-cancelled?`、`future-cancel` | 一个 Future operand | 查询或请求取消，不隐藏 failure。 |
| `pmap` | `(pmap f coll...)` | Eager submit、按最短输入、保持 result order，传播首个 observed dereference exception；不得隐式取消已提交 sibling。 |
| `pvalues`、`pcalls` | `(pvalues form...)`、`(pcalls f...)` | 并发求值并返回 ordered eager Vector；零 form 返回 `[]`。 |
| `promise`、`deliver` | `(promise)`、`(deliver promise value)` | 第一次 delivery 生效并返回同一 Promise；后续 delivery 保留原值，也返回该 Promise。 |
| `deref` | `(deref value [timeout-ms timeout-value])` | 读取 Delay/Future/Promise；timeout 单位毫秒；省略则等待。 |
| `lock`、`locking` | `(lock)`、`(locking lock body...)` | 创建 reentrant lock，并保证 exit 时 release。 |

Concurrency operation 必须在 typed HIR 携带 thread、blocking、timeout、cancellation 和
unknown callback effect。每个 consuming output 必须把一个 private executor implementation
链接到 `__osiris_runtime__`；不得承诺 fairness、task start order 或取消已开始 work。

**OEP-0003-R066：** `osiris.python` 只能公开以下显式 dynamic boundary：

| Binding | Logical call | Result |
| --- | --- | --- |
| `get-attr`、`get-attr-or`、`has-attr?` | `(get-attr object name)`、`(get-attr-or object name fallback)`、`(has-attr? object name)` | `Any`、fallback-or-`Any` 或 `Bool`；attribute name 是 `Str`。 |
| `set-attr!`、`del-attr!` | `(set-attr! object name value)`、`(del-attr! object name)` | 显式 mutation，返回 `none`。 |
| `get-item`、`set-item!`、`del-item!` | 对应 object/key form | Read 返回 `Any`；mutation 返回 `none`。 |
| `call` | `(call callable args [kwargs])` | `args` 是 `Seq[Any]`，`kwargs` 是 `Map[Str, Any]`；返回 `Any` 并保留 Python argument/exception behavior。 |
| `iter` | `(iter object)` | 返回 `Sequence[Any]` 的 logical one-pass dynamic boundary。 |
| `type-name` | `(type-name object)` | 返回 diagnostic display `Str`，绝不是 binding identity。 |

Read/call 返回显式 `Any`，除非 typed `extern` 或 extension facade 证明更精确结果；
mutation/call 携带 unknown effect。这些函数不得按字符串 import module、执行代码文本、
检查 compiler state 或在 Phase 1 运行。Static Python import 使用 `py/import`，可复用
typed boundary 使用 `extern`。

### API 发布

**OEP-0003-R067：** R021、R022、R057 到 R066 命名的每个 binding 都必须有可通过
command 查询的 API record，包含 canonical binding ID、owning namespace、macro/function/
value kind、source call shape、generic `.osri` signature、evaluation strategy（`macro`、
`eager`、`lazy` 或 `consumer`）、effect、exception behavior、`since`、deprecation state、
authored default documentation、tagged translation 和 source location。`osr lsc hover`/
`osr lsc signature` 必须从 matching installed interface 返回记录，不 import Python。

**OEP-0003-R068：** 只有 placeholder、untyped `Any` shim 或 name-only entry 的初始
standard binding 不得作为 conforming API export。Pre-stable staged release 可以暂时缺少
API，但 capability manifest/documentation 必须标记 unavailable；完整 catalog conform 前，
本 OEP 不能成为 Final。

### Distribution-private linking

**OEP-0003-R069：** Macro expansion、name resolution 和 typed HIR validation 完成后，
linker 必须按 stable identity 收集所有 referenced standard binding、intrinsic、Linkable
helper，并计算 transitive runtime dependency closure。Closure 内的 public standard
function 必须从 packaged `.osr` source 编译；Kernel leaf 必须 direct lowering 或链接其
compiler-owned target helper。每个 selected public binding 必须以其手写 standard-source
item 作为 linking root；选择一个 binding 不得隐式选择其 namespace 的全部 public item。
只允许为该 reachable closure 生成 support。Filesystem order、unqualified spelling 和
Python import side effect 不得影响选择。

**OEP-0003-R070：** Output 需要独立 generated support 时，reserved package name 必须
恰好是 `__osiris_runtime__`。Publishable distribution 必须把它放在每个 owning generated
Python package root 内：

```text
<python-package>/__osiris_runtime__/
  __init__.py
  sequence.py
  concurrency.py
  dynamic.py
```

只生成 reachable closure 需要的 module。Compiled standard module 应放在
`__osiris_runtime__/stdlib/` 下；Kernel helper 是 linker 选择的 private sibling。无需
shared helper state/reuse 时，compiler 可以把 helper 直接降低到 owning output module，
但不得选择其他 support-package name。

**OEP-0003-R071：** Generated user module 和 `__osiris_runtime__` 不得 import `osiris`、
`osiris.prelude`、`osiris-lang`、compiler executable、`.osri`、macro IR 或 documentation
database。它们可以 import Python standard library 和 project 的普通 declared Python
dependency。成功的 Python build 在删除全部 Osiris build tooling 和 source-only compiler
artifact 后必须仍可执行。

**OEP-0003-R072：** Kernel control flow 和简单 standard operation 应直接降低为 readable
Python。LazySeq、Reduced、Delay、Future、Promise、dynamic binding、lock 等 stateful/
reusable facility 可以生成 private support module。Generated support 只能包含 ordinary
Python，禁止在 runtime 做 interface discovery、macro expansion、semantic analysis、package
scan 或 ABI negotiation。

**OEP-0003-R073：** `__osiris_runtime__` 是 compiler-reserved source/output path。User
module 或 package member 会占用该 path 时，必须在写 output 前诊断。一个 distribution
不得 import 另一个 distribution 的 generated support、跨 installed distribution deduplicate
helper，或把 support name 暴露成 public `.osri` binding。

**OEP-0003-R074：** Linker 必须生成一份 deterministic private support manifest，包含
Python target、standard-library semantic hash、reachable binding ID、helper content hash 和
source-map identity。Manifest 是 build provenance/cache input，不是 runtime compatibility
contract；普通 Python import/execution 不得读取它。

**OEP-0003-R075：** Standard-library source span 和 macro origin 必须通过 linked support
继续映射到 `.py.map`/local tooling result。Provenance 可以标识 Osiris build input，但
generated Python execution contract 必须保持与 Osiris tooling 无关。

## 被拒绝方案 (Rejected Alternatives)

### 把整个 core 实现成宏

宏不是普通 first-class runtime value。把 `map`、`filter`、`reduce` 只做成宏会阻止动态
选择和 higher-order use、扩大代码，并模糊 compile-time/runtime semantic。

### 把全部 standard behavior 放入 Phase-1 evaluator

Evaluator 在 deterministic sandbox 中操作 syntax data；runtime collection、Python object、
IO、concurrency、inferred type 不属于该阶段。扩成第二 runtime 会扩大 trusted surface 并
破坏 phase separation。

### 保留 shared `osiris.prelude` runtime

Shared package 会让 generated Python 依赖 compiler distribution、要求 runtime ABI
negotiation，并让分别编译的 extension 争用同一个 installed helper version。
Distribution-private linked support 可以避免这些问题。

### 把 DataFrame framework 放入标准库

Framework release cadence、null semantic、schema、axis、execution model 不是稳定 language
primitive；PyPI extension 允许独立演进。

### 发布独立版本的 standard-library distribution

ABI 稳定前，独立 version 的 standard source package 会制造不支持的 compiler/library
组合并增加 build isolation 复杂度。因此 source package 虽然遵循普通 OEP-0002 layout，
仍随 matching `osiris-lang` release 一起发布。

### 从 function name 推断 standard semantic

Name-based solver 会破坏 alias、import、extension replacement 和 stable identity。正确接口
是 signature、binding ID、protocol 与 intrinsic contract。

## 开放问题 (Open Questions)

无。

## 一致性 (Conformance)

一个 Osiris release 在以下条件全部满足时符合本 OEP：

- 每条 MUST/MUST NOT requirement 都有 mapped evidence；
- 所有 required namespace/core export 解析到稳定 `.osri` identity；
- Standard Phase-1 dependency acyclic，且 sandbox restriction 有测试；
- Standard macro expansion 确定并保留 origin；
- Public ordinary function 通过 type/effect/temporal/data-summary 测试；
- lazy/eager 与 early-termination 行为符合规范；
- Native compiler 只包含最小 Kernel standard surface，`osiris-lang` wheel 包含完整普通
  standard source package；
- 每个 public standard API record、signature、macro、implementation 都映射到 packaged
  `.osr`，generated cache 根据该 source 校验；
- Kernel declaration 不含 localized API documentation，每个 public Kernel wrapper 都有
  authored default 与 `zh-CN` documentation；
- Conformance project build 包含 readable generated Python、source map 和
  distribution-private linked support；
- Reachable support 只生成在 `__osiris_runtime__` 下，不 import Osiris runtime package，
  并且离开 build tooling 后仍可执行；
- 两次 clean standard-library build 满足 artifact determinism；
- LSP/semantic inspection 通过双语 metadata/source-navigation 测试；
- framework-specific behavior 留在 Rust Kernel/standard module 外；
- implementation release 标识自身并把 evidence 链接到 OEP-0003。

缺少任何 required initial namespace 时，本 OEP 不能成为 Final。

## 修订历史 (Change History)

- Revision 10，2026-07-24：恢复 Clojure 风格的隐式 `osiris.core` referral，将显式
  core import 定义为完整覆盖，并规定 local shadowing、qualified lookup 以及
  runtime/type/macro phase 一致的可见性。
- Revision 9，2026-07-24：要求每个 public standard module 把 typed target boundary 隔离到
  `<namespace>.kernel`，并规定通过解析后的 `:osiris/facade-modules` 与
  `:osiris/facade-macros` 组合拆分后的 `osiris.core`，同时保持 public identity。
- Revision 8，2026-07-24：要求使用层级 implementation namespace、
  `:osiris/internal true` 与 Clojure 风格的 `defn-`/`:private` semantic，并禁止使用下划线
  表示 namespace privacy。
- Revision 7，2026-07-24：要求 public Kernel facade 是基于 private、minimal-metadata
  Kernel leaf declaration 编写的 Osiris `defn`/`def` implementation，并规定 public source
  item 是 standard linker root。
- Revision 6，2026-07-24：把实现拆为最小 embedded Kernel 与自举、source-distributed
  standard package；规定 public `.osr` declaration 是 normative source，并要求用带文档的
  public wrapper 包装无文档 Kernel operation。
- Revision 5，2026-07-23：加入 reduction、delayed evaluation 与 concurrency 公开签名
  所需的 public nominal protocol type。
- Revision 4，2026-07-23：用显式 `:refer :all`/`:exclude`/`:rename` import contract
  替换 implicit core auto-referral，并把初始 standard-library ABI 固定为 `1`。
- Revision 3，2026-07-23：用按 reachability 链接到每个 output private
  `__osiris_runtime__` 的机制替换 shared `osiris.prelude` runtime，并把 `for` 改为 lazy、
  增加 eager `forv`。
- Revision 2，2026-07-23：定义初始 macro/function API catalog，包括 source shape、
  evaluation、return、collection、sequence、string、math、concurrency 和显式 Python
  boundary contract。
- Revision 1，2026-07-23：初始草案。
