---
oep: 5
title: 主表层语法
description: Elixir 风格表层语法（.oxr）作为 Osiris 唯一书写表层：文法、到 form 的映射、S 表达式记法退居内部表示。
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
revision: 3
language: zh-CN
source: ../../0005-primary-surface-syntax.md
source-revision: 3
translation-status: Current
requires: [0, 1]
replaces: []
superseded-by: null
resolution: null
---
# OEP-0005：主表层语法

## 摘要

Osiris 用 Elixir 风格的表层语法书写，扩展名 `.oxr`。这是全面迁移而非共
存：`.oxr` 是唯一的书写表层，编辑器、格式化、LSP、模板、文档全部以它为
目标。S 表达式记法退回它本来的位置——宏操作的 form 数据结构的文本形
态。它仍作为展开调试格式可见（`osr expand` 打印它），`.osr` 文件在既有
代码手工改写期间仍可编译（过渡输入），但不再用于书写新代码。接口层
（`.osri`）不变。

## 动机

S 表达式既是 Lisp 宏成立的原因，也是多数程序员不愿多看一眼的原因。
Elixir 给出了解法：宏保留同构的**数据**表示，人用主流观感的表层。
Osiris 的宏收到的是 form 而非文本，所以表层可以更换，而宏系统、接口格
式、别名机制（OEP-0001-R060…R062C）、文档管线全都不动。

原型（`explore/elixir-surface` 分支，已合并）已端到端验证：一个 `.oxr`
策略使用未改动的 qlab `defselect` 宏——按 `:osiris/names` 拼写调用、
因子名全 kebab——翻译、展开、类型检查，生成与 S 表达式孪生版相同的
Python。

## 规范

### R001 —— 唯一书写表层

`.oxr` 承载 Osiris 的书写表层，由本 OEP 定义。OEP-0001 的 S 表达式记法
是语言的内部 form 表示：宏收到它，`osr expand` 打印它，过渡输入 `.osr`
包含它。两种记法读入同一 form 数据结构。文件必须按其扩展名指定的语法书
写；实现不得嗅探内容。迁移窗口期内项目源码发现必须接受两个扩展名，模块
名由路径推导，两侧规则一致。

### R002 —— 全面迁移

所有面向用户的材料呈现 `.oxr`：`osr init` 模板、文档示例、工具片段、诊
断。工具链——格式化器、LSP、编辑器扩展、sourcemap——必须以 `.oxr` 为
目标表层。既有 `.osr` 源码作为过渡输入继续编译，项目按自己的节奏手工改
写；过渡输入不再获得新的表层特性。宏书写在 R012 的 quote 映射落地前仍
用 `.osr`，落地后新宏也用 `.oxr` 书写。

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

标识符由字母（任意文字系统）、数字（非首位）、`_`、`-`、`/`、`?`、`!`
组成。`-` 或 `/` 后紧跟名字字符（无空白）时属于标识符；减法与除法必须
用空白包围。`pct-rank` 和 `py/import` 都是单个名字；`a - b` 是减法、
`a / b` 是除法；`x-1` 是一个名字，与 S 表达式表层完全一致。这一取舍让
既有 kebab-case 导出与所有斜杠限定名在主表层全部直接可调——无需别名、
改名或绕行。下划线拼写（`import_for_syntax`、`defn_for_syntax`）是对应
连字符核心形式的可接受写法。

反引号逐字拼写标识符文法无法承载的名字：``refer: [`>`, `<=`, if-else]``
引用运算符名宏，`` `+`(a, b, c) `` 在二元中缀形状之外调用它。反引号名内
不得含反引号或换行。

### R005 —— 定义

`def 名字(形参 :: 类型, …) :: 返回 do 体 end` 翻译为
`(defn ^返回 名字 [^类型 形参 …] 体…)`；`defmacro` 同理。前置
`@doc "…"` 附加 `^{:doc "…"}` 元数据；关键字形式
`@doc default: "…", zh-CN: "…"` 附加本地化文档 map
`^{:doc {:default "…" "zh-CN" "…"}}`。多条体语句按序成为定义体。
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

翻译失败必须报告源码行号。针对 `.oxr` 单元的编译诊断必须写 `.oxr` 路
径。在 reader 原生集成之前（见路线图），翻译单元内的 span 指向翻译后文
本；实现应携带行映射，使面向用户的位置落到 `.oxr` 坐标。

### R012 —— 主表层尚未定义的部分

`quote`/`unquote`（Elixir `quote do … end` ↔ syntax-quote 映射）、形参
解构、`defstruct`、内嵌 provider、一般元数据属性尚不属于主表层。需要它
们的宏用 `.osr` 书写；从 `.oxr` 消费这些宏则完整支持。每一项按
OEP-0000 以本 OEP 修订形式先行定义、后实现。

## 路线图

1. **加载期翻译（已完成）：** `.oxr` 源码在 workspace 加载时翻译为规范
   文本进入不变的管线。`osr sketch FILE` 暴露翻译结果供查看。
   `osr init` 生成 `.oxrr`。
2. **编辑器迁移（本修订）：** VS Code 扩展与 LSP 识别 `.oxr`；原生
   reader 落地前诊断按行级保真落到 `.oxr` 位置。
3. **原生 reader：** 翻译器升级为一等 reader，产出携带真实 `.oxr`
   span 的 form；诊断、LSP 悬停/跳转/改名、sourcemap 获得完整保真；
   `osr fmt` 支持 `.oxr`。
4. **宏书写：** quote/unquote 映射，使书写表层的 `defmacro` 达到完整
   能力；此后 `.osr` 过渡输入可按项目逐个退役。

## 向后兼容

迁移窗口期内 `.osr` 源码、接口、缓存与所有既有项目不变编译。混合项目按
文件粒度支持；改写为手工、按文件进行。接口与缓存与表层无关。

## 修订历史 (Change History)

- Revision 3，2026-08-01：全面迁移：`.osrx` 改为 `.oxr`，成为唯一书写表
  层；S 表达式记法退居内部 form 表示，`.osr` 作为过渡输入、由手工改写。
  R004 新增反引号逐字名；R005 新增本地化 `@doc`；工具链（LSP、编辑器扩
  展）以 `.oxr` 为目标。（`.ox` 曾被考虑并否决：它是计量经济学语言 Ox
  的源码扩展名，其用户群与 Osiris 的量化受众重叠。）

- Revision 2，2026-08-01：R004 让位规则扩展到 `/`：斜杠并入标识符
  （`py/import`、斜杠限定名），除法必须空白包围。`osr init` 模板按
  R002 生成 `.oxrr`。

- Revision 1，2026-08-01：初版：Elixir 风格表层为主（`.oxr`）、一切皆
  调用的翻译、中缀让位 kebab-case 标识符、首参插入管道、do 块即
  named-body 宏调用、共存规则与路线图。
