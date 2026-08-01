---
oep: 5
title: 主表层语法
description: Elixir 风格表层语法（.osrx）作为书写 Osiris 的主要方式：文法、到 form 的映射、与 S 表达式表层的共存。
author: MJ
status: Draft
type: Standards Track
areas:
  - Language
  - Compiler
  - CLI
  - Tooling
created: 2026-08-01
updated: 2026-08-01
revision: 1
language: zh-CN
source: ../../0005-primary-surface-syntax.md
source-revision: 1
translation-status: Current
requires: [0, 1]
replaces: []
superseded-by: null
resolution: null
---
# OEP-0005：主表层语法

## 摘要

Osiris 采用 Elixir 风格的表层语法作为书写源码的主要方式，扩展名
`.osrx`。宏体系不变：表层文本翻译成宏一直在操作的同一 form 数据结构，
`defselect 名字 do … end` 触达的就是 S 表达式表层调用的同一批
named-body 宏。S 表达式表层（`.osr`）继续完整支持，作为规范数据记法
——它是宏看到的东西、`osr expand` 打印的东西、既有源码继续使用的东西。
单文件单语法，扩展名决定表层；接口层（`.osri`）两侧共享且完全一致。

## 动机

S 表达式既是 Lisp 宏成立的原因，也是多数程序员不愿多看一眼的原因。
Elixir 给出了解法：宏保留同构的**数据**表示，人用主流观感的表层。
Osiris 的宏收到的是 form 而非文本，所以表层可以更换，而宏系统、接口格
式、别名机制（OEP-0001-R060…R062C）、文档管线全都不动。

原型（`explore/elixir-surface` 分支，已合并）已端到端验证：一个 `.osrx`
策略使用未改动的 qlab `defselect` 宏——按 `:osiris/names` 拼写调用、
因子名全 kebab——翻译、展开、类型检查，生成与 S 表达式孪生版相同的
Python。

## 规范

### R001 —— 两个表层，一个 form 语言

Osiris 恰有两个表层语法。`.osrx` 承载本 OEP 定义的主表层；`.osr` 承载
OEP-0001 定义的 S 表达式表层。两者读入同一 form 数据结构。文件必须按其
扩展名指定的语法书写；实现不得嗅探内容。项目源码发现必须接受两个扩展
名，模块名由路径推导，两侧规则一致。

### R002 —— 主即默认

新的面向用户材料默认使用主表层：`osr init` 模板、文档示例、工具片段应
展示 `.osrx`。S 表达式表层无限期完整支持——它是宏数据记法与展开调试格
式（`osr expand` 输出保持 S 表达式）——既有 `.osr` 源码必须继续编译，
无迁移要求。

### R003 —— 一切皆调用

主表层恰有四个特殊形式：`def`、`defmacro`、`@doc`、`if`。其余一切构造
都是调用。`module`、`import`、`import-for-syntax`、`export` 是普通的无
括号调用，落到 OEP-0001 对应核心形式：

| 表层 | Form |
| --- | --- |
| `module app.策略` | `(module app.策略)` |
| `import lib.marks, refer: [加倍]` | `(import lib.marks :refer [加倍])` |
| `import-for-syntax m.select, refer: :all` | `(import-for-syntax m.select :refer :all)` |
| `export [f, g]` | `(export [f g])` |
| `f(a, b)` / `m.f(a)` | `(f a b)` / `(m.f a)` |
| `key: value`（实参位） | `:key value` |
| `:word` | `:word` |
| `[a, b]` | `[a b]` |

语句位置的标识符后跟实参（中间无运算符、无调用括号）即开始无括号调用，
实参延伸到行尾或 `do` 块。表达式关键字（`if`、`not`、`do`、`else`、
`end`）不得作为无括号调用头。

### R004 —— 标识符：中缀让位给 kebab-case

标识符由字母（任意文字系统）、数字（非首位）、`_`、`-`、`?`、`!` 组
成。`-` 后紧跟名字字符（无空白）时属于标识符；减法必须用空白包围。
`pct-rank` 是一个名字；`a - b` 是减法；`x-1` 是一个名字，与 S 表达式表
层完全一致。这一取舍让既有 kebab-case 导出在主表层全部直接可调——无需
别名、改名或限定名绕行。下划线拼写（`import_for_syntax`、
`defn_for_syntax`）是对应连字符核心形式的可接受写法。

### R005 —— 定义

`def 名字(形参 :: 类型, …) :: 返回 do 体 end` 翻译为
`(defn ^返回 名字 [^类型 形参 …] 体…)`；`defmacro` 同理。前置
`@doc "…"` 为定义附加 `^{:doc "…"}` 元数据。多条体语句按序成为定义体。
更丰富的元数据（`:osiris/names`、`:osiris/clauses`、`:export` 标记）在
主表层的写法留待本 OEP 后续修订；在此之前需要它们的声明用 `.osr` 书写。

### R006 —— 运算符

二元运算符 `+ - * / == != < <= > >=` 翻译为对名字
`+ - * / = not= < <= > >=` 的前缀调用；`and`、`or`、一元 `not` 翻译为
同名 form；一元负号翻译为 `(- 0 x)`。优先级从松到紧：`|>`；比较；
`+ -`；`* /`；一元。运算符就是普通名字：模块把 `<=` 作为宏导入
（pandas 风格）时，中缀拼写触达的就是那个宏。

### R007 —— 管道

`x |> f(a, b)` 翻译为 `(f x a b)`；`x |> f` 翻译为 `(f x)`。被管道值插
入**第一个**实参位（Elixir 语义），与"数据在前"的 data-frame 式签名
契合。`|>` 结合最松，`x |> f() |> g()` 从左到右成链。

### R008 —— do 块即 named-body 宏调用

`头 实参 do 语句… end` 翻译为 `(头 实参 (语句)…)`：块内每条语句成为一
个子句 form。这正是 named-body 宏的形状（OEP-0001 named-body 约定、
`:osiris/clauses` 悬停），声明式 DSL——`defselect`、查询构建器、配置
块——在主表层零宏改动即可用：

```elixir
defselect 小市值 do
  slot short-mom, weight: rank-threshold
  with is-top?, if-else(rank(short-mom) <= rank-threshold, 1, 0)
  where pct-rank(long-mom) > pct-floor
  select rank(market-cap)
end
```

### R009 —— `if`

`if 条件 do 结果 else 备选 end` 翻译为 `(if 条件 结果 备选)`；`else`
分支可省略。

### R010 —— 注释

`#` 开启行注释，直到行尾，不产生 form。（`.osr` 中的 `;;` 注释不变。）

### R011 —— 诊断与溯源

翻译失败必须报告源码行号。针对 `.osrx` 单元的编译诊断必须写 `.osrx` 路
径。在 reader 原生集成之前（见路线图），翻译单元内的 span 指向翻译后文
本；实现应携带行映射，使面向用户的位置落到 `.osrx` 坐标。

### R012 —— 主表层尚未定义的部分

`quote`/`unquote`（Elixir `quote do … end` ↔ syntax-quote 映射）、形参
解构、`defstruct`、内嵌 provider、一般元数据属性尚不属于主表层。需要它
们的宏用 `.osr` 书写；从 `.osrx` 消费这些宏则完整支持。每一项按
OEP-0000 以本 OEP 修订形式先行定义、后实现。

## 路线图

1. **加载期翻译（本修订）：** `.osrx` 源码在 workspace 加载时翻译为规
   范文本进入不变的管线。`osr sketch FILE` 暴露翻译结果供查看。
2. **原生 reader：** 翻译器升级为一等 reader，产出携带真实 `.osrx`
   span 的 form；诊断、LSP 悬停/跳转/改名、sourcemap 获得完整保真。
3. **格式化与模板：** `osr fmt` 支持 `.osrx`；`osr init` 生成 `.osrx`
   模板；文档示例按 R002 切换。
4. **宏书写：** quote/unquote 映射，使主表层的 `defmacro` 达到完整能力。

## 向后兼容

`.osr` 源码、接口、缓存与所有既有项目不变编译。新扩展名是增量的，没有
切换日。混合项目按文件粒度支持。

## 修订历史 (Change History)

- Revision 1，2026-08-01：初版：Elixir 风格表层为主（`.osrx`）、一切皆
  调用的翻译、中缀让位 kebab-case 标识符、首参插入管道、do 块即
  named-body 宏调用、共存规则与路线图。
