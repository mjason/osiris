# Osiris 语言设计规范（草案）

- 状态：实现基线（持续演进）
- 文档版本：0.7
- 日期：2026-07-23
- 项目：`osiris`
- 编译器命令：`osr`
- 源文件：`.osr`
- 编译接口：`.osri`

本文描述 Osiris 语言、编译器、扩展系统和工具链的设计与当前实现边界。规范性要求以 `oeps/` 下的英文 OEP 为准；本文是与当前 Draft OEP 和实现同步的中文设计总览。

文中的“必须”“应当”和“可以”分别表示强制约束、默认选择和可选能力。设计问题通过 OEP 修订处理，本文不维护第二份开放问题清单。

## 1. 定位

Osiris 是一门以数据处理为中心、AOT 编译到 Python 的静态类型 Lisp。它与 Python 的关系接近 TypeScript 与 JavaScript，但生成的 Python 会保留完整类型标注，并且必须适合人直接阅读、调试和继续使用。

Osiris 吸收 Clojure 的以下设计经验：

- 编译期代码即数据和 S-expression。
- `->`、`->>`、`cond->` 等 threading macros。
- 通过宏扩展领域语法。
- 使用可读取、可传播的 Rich Metadata 描述代码和数据语义。
- 不可变数据和显式状态变换优先。
- 使用小型语言核心承载稳定语义。

Osiris 不以兼容 Clojure 为目标，也不复制 JVM、Clojure 持久化集合或 Clojure 的宿主语义。Python 是 Osiris 的运行时平台和互操作边界。

### 1.1 已确定的方向

1. 编译器使用 Rust 实现，命令名为 `osr`。
2. 单一 `.osr` 源码生成带类型的 `.py`、`.osri` 和源码映射；项目存在 public static records 时还生成 distribution 级纯数据 runtime manifest。
3. 类型系统、`defstruct`、宏展开和源码位置属于编译器核心。
4. `defstruct` 是原生声明，不实现为普通库宏。
5. 宏使用 Clojure 风格表面写法，但默认卫生，并在受限的 phase-1 环境中执行。
6. Pandas、Polars 和 NumPy 不硬编码进 Rust 编译器，通过库和扩展提供。
7. PyPI 负责分发，`uv` 负责解析、锁定、安装、构建和发布。
8. `osr` 不实现包解析器、注册表、安装器或第二份锁文件。
9. 数据扩展可以在 typed HIR 上检查时间依赖和不可用数据读取。
10. LSP 与命令行编译器复用同一套解析、展开、类型和诊断实现。
11. Unicode 名称、公开别名和参数/字段别名属于名称解析系统，所有拼写共享同一个稳定 binding id。
12. Rich Metadata 对齐 Clojure 1.12 的 `^` reader 表面和不可变读取 API，并进入 `.osri`，供宏、LSP、渲染器和 Agent 使用。

## 2. 设计目标与非目标

### 2.1 目标

- **小核心**：核心只包含求值、绑定、类型、结构、模块、宏和 Python 互操作所必需的语义。
- **可读 Python**：生成代码保留模块、函数、局部变量、类型和主要控制流，不生成难以审计的字符串 DSL。
- **数据优先**：结构化数据、数组、表、schema、流水线和状态扫描是主要使用场景。
- **静态可理解**：编译器和 LSP 能理解名称、字段、函数签名、宏来源、副作用、时间依赖和数据属性。
- **多语言可读**：源码可以直接使用中文标识符，也可以由 CLI/LSP 在不改动源码的前提下显示中文领域名称和数据流。
- **Agent 可消费**：工具可以查询结构化签名、别名、文档、来源和编译器已证明的语义，不依赖解析生成的 Python 或黑盒 DSL 字符串。
- **领域可扩展**：数据框和业务 DSL 主要通过 Osiris 宏、普通函数和接口包实现。
- **Python 原生互操作**：生成代码直接调用 Python 包，不引入另一套数据运行时。
- **因果可审计**：时序读取、窗口方向、缺失值规则和状态递推必须能显式表达和检查。
- **确定性构建**：相同源码、接口、锁文件和编译选项应生成相同产物。

### 2.2 非目标

- 不实现 Clojure 兼容层或 JVM 语义。
- 不在 Rust 中重写 Pandas、Polars、NumPy 或领域运行时。
- 不通过宏隐藏重要的 I/O、缺失值、时间对齐和状态语义。
- 不默认执行安装包中的 Python 代码来发现扩展。
- 不在第一版支持 reader macro、任意语法扩展、原生动态链接编译器插件或 Python procedural macro。
- 不以激进优化为目标，尤其不改变 NumPy 浮点运算顺序和状态更新顺序。
- 不为包管理提供 `osr add`、`osr lock`、`osr sync` 或 `osr publish` 的平行命令。

## 3. 总体架构

```text
.osr -> 容错 CST -> 模块/phase 解析 -> 加载依赖 .osri
                                      |
                                      v
                            phase-1 卫生宏展开
                                      |
                                      v
                         runtime 名称解析/HIR -> 类型检查
                                                     |
                                                     v
                                    effect/时间因果/数据契约检查
                                                     |
                                                     v
                                                Python AST
                                          /        |        |        \
                                        .py      .osri   .py.map   .records.json
```

编译器必须使用结构化 AST/HIR 和 Python AST，不得通过拼接 Python 源码字符串实现主要 codegen。宏展开生成的是 Osiris 核心语法，不直接生成 Python 字符串。

Reader 使用 `nom` 组合子解析 lossless lexer 产生的 token slice。lexer 只识别固定标点、原子、字符串和 trivia，并保留原始 UTF-8 spelling/span；grammar production 负责集合、reader prefix、Rich Metadata、datum 校验和局部错误恢复。新增固定 reader form 时应增加独立、可测试的 production，不得把领域 DSL 逻辑塞入 lexer 或单体递归分支。用户级语言扩展仍通过 phase-1 宏完成，不开放任意 parser plugin。

## 4. 词法与表面语法

以下语法为第一版规范草案；`nom` reader production 是当前可执行语法定义，冻结发布 ABI 前再由这些 production 导出并校对精确 EBNF。

### 4.1 数据形式

```clojure
none                         ; 空值
true false                   ; 布尔值
42 3.14                      ; 数值
"text"                       ; 字符串
:value                      ; keyword
(f x y)                      ; 调用或特殊形式
[x y z]                      ; 顺序集合
{:name "dataset" :value 0.2}  ; 映射
#{:long :short}              ; 集合
'(a b c)                     ; phase-1 quote
```

`;` 到行尾是注释。字符串、数值和转义规则应尽量与 Python 对应，但由 Osiris 解析器独立定义。

`'`、`` ` ``、`~`、`~@`、`^` 和 `#{...}` 是语言固定的 reader grammar，不属于用户可扩展 reader macro。`^` 读取 Rich Metadata，包括 Clojure 1.12 的 `^[...]` 参数标签简写，精确定义见第 8.8 节。第一版不提供 `#()`、tagged literal 或任何修改 tokenizer/parser 的入口。

### 4.2 名称

- 源码必须支持 Unicode 标识符。
- 标识符比较使用 Unicode NFC 规范化结果，同时保留原始 spelling 供诊断和 source map 使用。
- `q/scan` 表示模块或命名空间限定名称。
- `event.value` 表示静态字段或 Python 属性访问。
- Lisp 风格名称可以包含 `-`，谓词可以使用尾部 `?`。
- 生成 Python 时必须使用确定、可检查冲突的名称映射。
- Python 会按 NFKC 规范化标识符；codegen 必须在名称映射后再按目标 Python 的 NFKC 结果做第二次碰撞检查。
- 两个 Osiris 名称映射到同一个 Python 名称时必须编译失败，不能静默覆盖。

名称的精确编码规则仍需确认，见第 19 节。

### 4.3 模块示例

```clojure
(module analytics.pipeline)

(import osiris.data :as data)
(import analytics.transforms :as transforms)
(py/import numpy :as np)

(export [normalize DatasetOptions])
```

Osiris 模块导入与 Python 运行时导入必须在语义上区分。普通 `(import ...)` 读取 Osiris 编译接口；`(py/import ...)` 生成 Python import，并且不得在编译阶段 import 目标 Python 模块。

### 4.4 核心声明与表达式

核心形式是闭集，规范性清单位于 `src/language/core_forms.rs`，目录职责和
纳入准则见 [`architecture.md`](architecture.md)。只有建立模块/阶段边界、
运行时绑定、名义类型与 ABI、静态接口，或宏无法保持的词法求值语义时，
形式才进入 Rust kernel。新增方便语法应首先实现为 `src/stdlib/macros/`
中的卫生宏；扩展包只能通过 `.osri` 发布宏、类型、extern contract、schema
和静态 record，不能注册 parser 或 backend 分支。

第一版核心至少包含：

- `module`、`import`、`import-for-syntax`、`py/import`、`py/decorate`、`export`、`alias`、`extern`
- `def`、`defn`、`defn-`、`fn`
- `let`、`if`、`do`
- phase-1 `quote` 和 syntax quote
- `defstruct`、`defstatic-schema`、`static-record`
- `defmacro`、`defn-for-syntax`
- `try`、`raise`
- 属性、索引、普通调用和关键字调用

`match`、protocol 和 async 语义不进入最小核心，待实际用例证明后再加入。`loop/recur`、Clojure 风格序列操作和 `for`/`forv` 属于显式导入的 `osiris.core` 控制流层：它们通过卫生宏和少量有契约的 Linkable helper 提供，不增加 Reader 的特殊分支。

没有显式 core import 的模块自动 refer 完整 `osiris.core` surface；显式
`(import osiris.core ...)` 则完全覆盖默认规则。`and`、`or`、`cond`、`when`、
binding 条件、集合迭代和 threading macros 在 core 中以卫生宏或普通函数提供，
不是未声明的编译器语法。`:refer`、`:exclude` 和 `:rename` 在 name lookup 前处理。

`defn-` 是 `osiris.core` 的声明宏：它展开为普通 `defn` 并附加 `:private true` authored metadata。Osiris 的跨模块可见性仍由显式 `export` 决定，因此该标记用于 Rich Metadata/LSP 展示，不绕过或替代接口导出规则。

### 4.5 Threading macros

`->`、`->>`、`cond->`、`cond->>`、`some->`、`some->>`、`as->` 和 `doto` 是 `osiris.core` 中的卫生宏，不是 Rust 编译器特殊形式。

```clojure
(-> value
    (clean)
    (normalize params))

(->> events
     (map event-budget)
     (reduce add base-budget))

(cond-> values
  options.clip-enabled
  (clip options.range)

  options.normalize-enabled
  (normalize))

(as-> raw value
  (normalize value)
  (combine value benchmark))

(doto recorder
  (record :started)
  (record :finished))
```

- `->` 将前一步结果插入下一形式的第一个参数位置。
- `->>` 将结果插入最后一个参数位置。
- `cond->` 按源码顺序判断条件并应用变换。
- `cond->>` 与 `cond->` 相同，但把值放在每一步参数末尾；`as->` 通过显式局部名称允许把值放在任意位置。
- `doto` 把同一个卫生临时值放到每个调用的第一个参数，按顺序执行调用，最后返回原值而不是最后一次调用的结果。
- 初始表达式只能求值一次。

### 4.5.1 Clojure 风格控制流与数据流

控制流形式先在 phase 1 展开，再进入正常的 AST、类型和语义摘要检查。Reader 只读取普通 list；因此领域包可以继续用宏组合这些形式，而不需要修改 Rust parser。

```clojure
;; 多重 binding、:let、:when 和 :while 按源码顺序展开；每个 collection 只求值一次。
(for [left lefts
      right rights
      :let [sum (+ left right)]
      :when (> sum 0)
      :while (< sum 100)]
  sum)

;; loop 的 binding vector 必须是 pattern/value 对；recur 的值对应同一状态向量。
(loop [index 0
       total 0]
  (if (= index 100)
    total
    (recur (+ index 1) (+ total index))))

;; 只在非 none 时继续 threading；false 会继续进入下一步。
(some-> maybe-row
        normalize-row
        (select-fields [:open :close]))

;; binding 条件使用 Clojure truthiness：只有 none 和 false 为假。
(if-let [row (lookup-row id)]
  (consume row)
  :missing)

;; case 的 dispatch 只求值一次；v0 要求显式 default。
(case status
  (:new :pending) :open
  :filled :closed
  :unknown)
```

- `for` 是返回 memoized `LazySeq` 的卫生宏；单一 binding 展开到 lazy `map`，需要过滤、早停或多重 binding 时组合 lazy `mapcat`。`forv` 使用相同 clause contract，但 eager 返回 Vector。`:let` 保持局部求值顺序，`:when` 丢弃当前项后继续，`:while` 在谓词首次为假时停止词法上最近的 collection binding；内层早停不会终止外层 collection。解构 pattern 使用卫生临时参数。
- `when`、`when-not`、`and`、`or`、`cond`、`cond->`/`cond->>`、`for`/`forv`/`doseq` 的 `:when`/`:while` 和 `while` 条件使用 Clojure truthiness：只有 `none` 与 `false` 为假，`0`、空字符串和空集合仍为真。它们通过 `truthy*` guard 接入严格 Bool 的 `if`，因此普通 `if` 的静态 Bool 约束不被放宽。
- `if-let`、`when-let` 和 `when-first` 是结构化卫生宏，不引入隐藏 callback 作用域，因此其 body 中处于尾位置的 `recur` 仍指向词法上所属的 `loop` 或函数。`if-let`/`when-let` 只有在值为 `none` 或 `false` 时走空分支；`0`、空字符串和空集合都是真值。`when-first` 先把输入转换为一次求值的 `seq`，判断集合是否非空，而不是判断首项是否为真，因此首项为 `none` 或 `false` 时仍执行 body；它接受普通容器、惰性序列和一般 iterable，探测到的首项保留在返回的 memoized sequence 中，不会在绑定前丢失。
- `nil?` 与 `some?` 是公开的 nil 谓词宏，分别映射到 `nil*` 和其否定；它们只检查 `none`，不会采用 Python 空集合/空字符串的假值规则。
- `if-some`/`when-some` 只把 `none` 视为空，因此 `false` 仍会绑定并执行真分支；`if-not`/`when-not` 对条件使用同一 Clojure truthiness 的否定。这些形式同样展开为结构化 `let`/`if`，不改变 `recur` 的词法目标。
- `some->` 和 `some->>` 在每一步前只检查 `none` 并短路，`false` 会继续传给下一步；每一步和初始值都只求值一次。HIR 的内部 `present*` intrinsic 在已由宏生成的 guard 后把 `Option[T]` 收窄为 `T`，不改变运行时值。
- `case` 把 dispatch 绑定到卫生临时值，再生成短路的等值分支；list 形式的测试常量表示一组候选值，重复常量在 phase 1 报错。由于 v0 尚未增加“无匹配”异常 intrinsic，`case` 当前必须提供显式 default；Symbol、Vector 和 Map 还不能作为 runtime case 常量。
- `condp` 把 predicate 和 dispatch expression 各绑定一次，按源码顺序短路测试；测试成功时返回对应结果，`:>>` 形式与 Clojure 一致，把 predicate 的非空返回值传给 handler，而不是传入 dispatch value。v0 要求显式 `:else`，predicate 结果使用 Clojure truthiness，缺少结果、错误的 `:else` 位置和不完整的 `:>>` 都在 phase 1 报诊断。
- `letfn` 由 compiler-owned lexical frame 实现，先预声明全部局部函数再降低 lambda，因此支持 canonical `(name [args] body ...)` 以及显式 `(name (fn ...))` 两种写法，并支持 self/mutual recursion；v0 暂不支持 Clojure multi-arity function clauses。
- `doseq` 复用 `for` 的 binding、解构、`:let`、`:when` 和 `:while` 规则，按顺序执行 body 并始终返回 `none`。它展开到嵌套的 `doseq*` 顺序循环，不构造或保留 callback 结果集合；`:while` 同样只停止最近 binding。`dotimes` 把 count 求值一次，以 `loop/recur` 从 `0` 迭代到 `count - 1`；`while` 每轮重新求值源码条件，再通过 `truthy*` guard 和同一常量栈协议执行，二者都返回 `none`。
- `loop`/`recur` 使用 compiler-owned lexical target/state HIR。`recur` 指向词法上最近的 `loop` binding 帧；没有内层 `loop` 时则指向当前 `defn`/`fn` 参数帧，并且只能出现在该目标的尾位置。HIR 检查所属函数深度、状态数量、每个状态的类型和尾位置；显式 loop 与函数级 recur 直接降低为 Python `while`，不进行 Python 自调用，因此栈空间为 `O(1)`。相互递归仍使用 `trampoline`。
- `letfn` 是一个需要 compiler-owned lexical frame 的卫生宏：HIR 先预声明同一 binding vector 中的全部函数名，再降低 lambda 和 body，因此支持单 arity 的局部自递归/相互递归与闭包 shadowing；多 arity `fn` dispatch 在 v0 报稳定诊断。它仍降低为普通 Python nested helpers，不引入第二套调度 runtime。
- `trampoline` 先调用首个函数，随后反复调用返回的零参数函数，直到得到非 callable 值；其小型 Python loop helper 按 reachability 生成到当前 distribution 的 `__osiris_runtime__`。
- `lazy-seq` 展开到一个零参数 thunk。Reachable `LazySeq` helper 生成到 `__osiris_runtime__`，延迟并记忆迭代器产生的元素，支持重复遍历而不重复执行 thunk；无限序列必须由调用方按需消费。`delay` 使用同样 distribution-private 的线程安全 `Delay[T]` helper，`force`/`deref` 只求值一次并缓存成功值或异常，`realized?` 查询是否已经求值。
- `try` 是核心结构化异常表达式，支持多个 `(catch Type name ...)` 和一个 `(finally ...)`；body-only `(try expr ...)` 在 Python 端直接透传，不生成空的 `try` 语句，且 `finally` 之后不能再出现 `catch`。`catch` 的异常类型使用封闭的 Python builtin whitelist（例如 `Exception`、`ValueError`、`TypeError`），未知 nominal 名称仍然报错。`assert` 直接生成不会被 `python -O` 消除的显式 `AssertionError` 分支，可选消息只在失败时求值。`time` 接受一个或多个 body expression，按顺序求值并返回最后值；其 monotonic-clock helper 按需生成到 `__osiris_runtime__`，body 异常原样传播。
- `with-open` 是卫生宏，按源码顺序求值资源并嵌套 `try/finally`，按逆序调用普通 Python `close`；`None` 资源跳过关闭。`future`/`future-call`、Promise、lock 和 `contextvars` dynamic binding 所需 helper 按可达闭包生成到当前 package 的 `__osiris_runtime__`，并以 unknown effect 标记异步边界；生成产物不依赖共享 Osiris runtime。`with-bindings`、`with-local-vars`、`with-redefs` 仍需更完整的 Var/root mutation semantic，暂不伪装成词法 `let`。
- `map`/`mapcat`/`filter`/`remove` 返回可重复遍历的 memoized `LazySeq`，`mapv`/`mapcatv`/`filterv`/`removev` 是 eager Vector variant；这些多 collection 入口按最短输入停止，callback 参数按 collection 顺序对齐。Lazy callback 只在 realization 时按需调用一次，value/exception 都必须记忆。动态 callback 边界沿用 Clojure truthiness，typed HIR 仍要求静态 `filter` callback 返回 `Bool`。`reduce` 接受 `(reduce f coll)` 或 `(reduce f init coll)`，`fold` 是明确初值的精确 alias contract。它们的 callback effects、temporal 和 data summaries 在 typed HIR 中合并，宏本身不重复实现数值逻辑。完整初始 API 由 [OEP-0003](../oeps/0003-standard-library.md) 定义。
- `cons`、`concat`、`lazy-cat`、`take`/`drop`、`take-while`/`drop-while`、`keep`/`remove`、`distinct`/`dedupe`、`partition`/`partition-all`/`partition-by`、`interleave`/`interpose`、`take-last`/`drop-last`、`iterate`/`repeat`/`repeatedly`/`cycle` 等序列组合返回可重复遍历的 memoized `LazySeq`。`distinct` 按 Clojure 值相等保留首次出现项，`dedupe` 只删除相邻重复项；二者都区分 `false` 与 `0` 并支持不可哈希值。窗口组合只保留当前窗口，`partition` 丢弃不足尾块或用显式 pad 填充，`partition-all` 保留所有尾块；`partition-by` 的每个分组本身也是 `LazySeq`，对每个输入只调用一次 key function，因此 key 恒定的无限输入也能立即返回首个分组。`interleave` 在最短输入结束时停止；`drop-last` 用固定尾缓冲，因此可消费无限序列，`take-last` 在首次输出前必须耗尽输入，所以对无限序列不会产生值。`none` 在所有序列入口都视为空序列。`nth` 按需推进 iterator，不会为了有限索引物化无限序列；两参数形式越界抛出 `IndexError`，只有显式第三参数形式才返回 fallback（包括显式 `none`）。`doall`/`dorun` 接受 `(coll)` 或 `(n coll)`，分别返回原集合或 `none`，只实现请求的前缀。`some`/`every?` 系列按 Clojure truthiness 短路。
- `reduced`、`reduced?` 和 `unreduced` 实现 Clojure 的归约提前终止协议。callback 返回 `(reduced value)` 时，最近的 `reduce`/`fold` 立即停止遍历并返回 `value`；`reduced?` 只检测 marker，`unreduced` 只移除一层 marker 且对普通值原样返回。类型层把 marker 表示为 `(Reduced T)`，因此 reducer 可以声明返回 `(Union T (Reduced T))`，而整个归约表达式仍是 `T`；marker 中的类型必须可赋给累加器类型。phase-1 `reduce` 使用同一协议，宏也能在遍历语法数据时提前结束。
- `throw` 是 `(raise value)` 的 Clojure 风格别名；当 value 的类型在编译期已知且不是 builtin exception nominal 时，HIR 报错；`Any`/未知 Python boundary 仍交给运行时检查。`comment` 在 phase 1 丢弃其 body 并生成 `none`，body 不参与运行时求值或宏展开。异常语义仍由核心 `raise` 和 Python 边界定义。

`empty?`、`seq?`、`coll?` 和 `sequential?` 是公开的 typed sequence predicates，均返回 `Bool`。`empty?` 把 `none` 视为空序列；有大小的容器走非消耗检查，`LazySeq` 及一般 iterable 只探测一个元素（`LazySeq` 会记忆该探测值），标量在运行时报告 `TypeError`。`seq?` 只识别 Osiris `List`（Python `list`）和 memoized `LazySeq`；`sequential?` 识别 `List`、`Vector`（Python `tuple`）和 `LazySeq`；`coll?` 在此基础上增加 map/set。字符串、bytes、原始 one-shot iterator 和标量都不是这三个谓词认可的集合，需先通过 `sequence` 接入可重复遍历的 seq 边界。`repeat`/`repeatedly` 的有限 count 必须是严格整数（`bool`、浮点和字符串不会隐式转换），负数产生空序列。`pmap` 至少需要一个 collection；无参数的 `pcalls`/`pvalues` 返回空 tuple。

这些形式通过 stable binding/intrinsic ID 进入 HIR，但不会形成 deployed shared runtime ABI。能直接表达的 guard、loop 和 exception 生成普通 Python；`LazySeq`、`Reduced`、Delay、Future 等可复用 helper 只按可达闭包生成到 `__osiris_runtime__`。`for :while`、`forv :while` 和 `doseq :while` 的停止 token 只由最近的生成循环消费；HIR 把 token 产生式标为 `Never`，避免污染普通分支类型。

### 4.5.2 Clojure 控制流覆盖矩阵与分期

下表按“是否已经有可测试的 Osiris 语义”划分 Clojure 常用控制流。**已实现**表示当前版本可直接写入 `.osr`，并由宏展开、HIR 和 standalone generated Python 测试共同锁定；**运行时接入中**表示表面形式可以先冻结，但需要对应 Linkable helper 完成后才允许进入 stable core；**后续序列层**表示不改变编译器核心，优先作为普通函数或 distribution-private helper 补齐；**延期**表示不能用词法 `let` 或普通函数假装实现。

| 类别 | 已实现（v0） | 运行时接入中 | 后续序列层 | 明确延期/扩展 |
| --- | --- | --- | --- | --- |
| 条件与绑定 | `and`、`or`、`when`、`when-not`、`if-not`、`cond`、`if-let`、`when-let`、`if-some`、`when-some`、`when-first`、`nil?`、`some?`、`case`、`condp` | 无 | 无 | `match`/模式协议（待真实用例） |
| 线程与状态 | `loop`、`recur`（显式/函数级，O(1) 栈）、`letfn`、`trampoline`、`while`、`dotimes`、`^:dynamic` Value、`binding`、`future`/`future-call`（含 binding context 传播）、`pmap`/`pcalls`/`pvalues`（eager、保序、显式异常/取消边界）、`promise`/`deliver`、带 timeout/default 的 `deref`、`future-done?`/`future-cancelled?`/`future-cancel`、`lock`/`locking` | 无 | 无 | `with-bindings`、`with-local-vars`、`with-redefs`（需要完整 Var/root mutation ABI）；完整 Agent/`send`/`send-off`/`await` API（需要共享可变状态与错误模型） |
| 资源与异常 | `try`/多 `catch`/`finally`、`raise`/`throw`、`assert`、`comment`、`time`、`with-open` | suppressed-exception 聚合（若以后需要） | 无 | `with-redefs`、事务 `dosync`/`sync`/Ref（需要可变共享状态模型） |
| 迭代与数据流 | lazy `for` / eager `forv`（多 binding、`:let`、`:when`、`:while`）、`doseq`、`map`/`mapv`、`mapcat`/`mapcatv`、`filter`/`filterv`、`reduce`/`fold`、`reduced` 协议、`lazy-seq`、`cons`、`concat`/`lazy-cat`、`count`/`first`/`rest`/`next`/`nth`/`seq`/`empty`、`empty?`/`seq?`/`coll?`/`sequential?`、`take`/`drop`、`take-while`/`drop-while`、`keep`/`keep-indexed`、`remove`/`removev`、`distinct`/`dedupe`、`partition`/`partition-all`/`partition-by`、`interleave`/`interpose`、`take-last`/`drop-last`、`map-indexed`、`iterate`/`repeat`/`repeatedly`/`cycle`/`sequence`、`reductions`、`run!`、`doall`/`dorun`（含可选计数）、`some`/`every?`/`not-every?`/`not-any?`、`delay`/`force`/`realized?` | 无 | `transduce`/`eduction` | 完整 Clojure Seq/ChunkedSeq/Transducer protocol（当前以明确的 `Iterable`/memoized `LazySeq` contract 为边界） |

序列层采用明确的 lazy/eager 边界：`map`、`filter`、`remove`、`take`、`drop` 等 producer 返回线程安全、记忆化的 `LazySeq`；`mapv`、`filterv`、`removev`、`forv` 返回 eager `Vector`；`reduce`/`fold` 是 eager consumer。`sequence` 显式把任意 `Iterable` 接入该协议。窗口与尾缓冲组合已经在这一 ABI 上完成，下一层是独立的 transducer/chunk protocol。每个新增入口必须同时声明：是否 eager、是否保持输入 identity、空值/`none` 规则、callback 求值次数、返回容器类型以及对 `reduced` 的响应；不能只凭 Clojure 函数名映射到 Python `list`。

并发类形式的顺序也固定：`Future`/`Promise`/`Lock` 的 Python ABI、异常传播和取消/超时语义已经由显式 runtime intrinsic 定义，表面 `future`、`future-call`、`promise`、`deliver`、`deref` timeout/default 和 `locking` 可以安全使用；`binding` 通过稳定 BindingId + `contextvars` map 实现动态作用域，并由 HIR 校验目标与类型，绝不能退化成普通 `let`。`pmap`、`pcalls`、`pvalues` 只在宏层组合这个 ABI，提交全部任务后按确定顺序读取，失败不隐式取消其余任务。更强的 `with-bindings`/`with-local-vars`/`with-redefs` 仍需 root Var 与可变共享状态协议。

### 4.6 别名与多语言名称

Osiris 的别名是名称解析能力，不是运行时注册表，也不是第二个函数或变量：

```clojure
(import data.series :as ts)
(import osiris.math :as math)

(alias 时序均值 ts/rolling-mean)
(alias 绝对值 math/abs)

(时序均值 values window :最小样本 1)
```

`alias` 是必须直接出现在模块中的声明，左侧创建当前模块可见的拼写，右侧必须解析到已有 binding。两者具有完全相同的 binding id、kind、phase、类型和三类语义摘要；别名链在接口中扁平化到唯一 canonical binding，循环是编译错误。别名本身不生成 Python 赋值、wrapper 或第二份实现。

API 作者可以通过第 8.8 节的标准 metadata 发布本地化名称：

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

当 canonical declaration 被 `export` 时，直接附着在该源码声明上的 `:osiris/names` 自动成为可解析 public aliases 并进入 `.osri`，不要求在 `export` 中重复列出；所以上例可以由消费者写成 `ts/rolling-mean`、`ts/时序均值` 或 `ts/滚动均值`。`preferred` 状态只影响显示和补全排序，不改变这些名称的解析身份；编译器绝不能根据当前 locale 为同一 token 选择不同含义。独立 `(alias local-name target)` 默认只在当前模块可见；若要公开，必须显式 export 该 alias，且 target 的 canonical binding 也已经 export。

名称表可以附着到模块级 value、type、macro、`defstruct` 字段和函数参数。已知静态签名的调用会先把参数别名归一到 canonical parameter id，再检查缺失、重复和类型，并在 Python AST 中生成 canonical keyword；同一次调用同时传 `:min-samples` 和 `:最小样本` 必须报告重复参数。动态 `Any`/Python 调用没有可验证签名，不能自动翻译关键字，必须使用目标 Python 的真实参数名或显式 typed wrapper。

第一版遵守以下约束：

- 同一作用域内，canonical name、别名、import 和本地定义经过 NFC 后不能指向不同 binding；冲突必须显式改名。
- public alias 的 target 必须是同一模块已显式 export 的 canonical binding，不能借 alias 意外公开私有实现；public alias 必须来自直接源码中的 `alias`、declaration 或顶层 declaration-macro 调用上的 `:osiris/names`。宏可以保留调用方直接写出的 public names，但不能自行发明或修改 export table；宏生成的其他名称 metadata 只能用于其卫生作用域和展示。
- 一个源代码拼写在所有 locale 中只有一个含义，不提供全局“中文词典”或按 locale 切换的解析器。
- 参数别名属于具体函数签名，字段别名属于具体 struct；不使用类似 `周期 -> n` 的全局关键字替换表。
- struct 字段别名可以用于静态字段访问和构造器关键字，并降低为 canonical Python attribute；DataFrame 的物理列名是运行时数据，不能被字段别名静默改写，必须由 schema/adapter 显式声明映射。
- module alias 继续使用 `(import ... :as q)`；普通 keyword、字符串和枚举值不是 binding alias，领域包必须用 typed constant、schema 或宏明确处理。
- `go-to-definition` 从别名跳到 canonical definition，find-references 可以按 binding id 汇总所有拼写；rename 必须区分“仅重命名当前 alias”和“重命名 canonical API”。
- alias 可以带 `:since`、`:deprecated`、`:replacement` 和迁移原因；deprecated alias 继续解析但产生 lint/LSP warning，并提供改写到 preferred/canonical name 的 code action。增加 public alias 是兼容变化，删除或改绑 public alias 是 source-breaking change。
- strict lint 应提示 Unicode confusable、不可见字符和高风险混合文字标识符，但不能把正常中文名称视为错误；真正的 NFC/NFKC 碰撞仍是编译错误。
- 公共库推荐使用稳定的 ASCII canonical name 加本地化 public names；应用内部的局部绑定可以直接使用中文。两种风格都是真实源码，不要求 LSP 才能编译。

生成 Python 时，import、公开调用和参数名使用 canonical Python binding；中文局部定义若本身是 canonical binding，则在通过目标 Python NFKC 碰撞检查后可以原样保留。若未来需要把 Osiris alias 同时发布为 Python API，必须使用显式 opt-in，不能改变默认 codegen。

### 4.7 Python 装饰器

Python 装饰器会在模块加载时执行，因此不能由不可执行的 Rich Metadata
隐式触发。Osiris 使用显式、类似 `alias` 的顶层声明，把一个或多个 Python
表达式附着到当前模块生成的函数或 struct：

```clojure
(py/import host.runtime :as host)

(py/decorate publish
  host.trace
  (host.register
    :extra-data {"columns" ["value" "year"]}))

(defn ^Any publish
  [^Any context [^Str field = "value"]]
  (context.emit field))
```

以上声明按源码顺序生成：

```python
@host.trace
@host.register(extra_data={"columns": ("value", "year")})
def publish(context: Any, field: str = "value") -> Any:
    return context.emit(field)
```

`py/decorate` 的 target 按 binding id 解析，所以本地 alias 可以作为 target，
但不能装饰 value、extern/import binding 或其他模块的声明。一个 target 只能有
一个 `py/decorate` 声明；多个 decorator 必须集中写在同一声明中，其顺序与
Python 的 `@` 顺序一致。声明可以位于 target 前后，也可以由顶层声明宏生成，
因此扩展包可以提供 `defcomponent` 一类宏而无需注册 parser 或 backend 插件。

Decorator 必须能降低为单个 Python expression；需要语句、临时变量或控制流的
表达式会被拒绝。Decorator 负责保持 Osiris 声明的参数与返回类型契约；返回
`Any` 的宿主 decorator 是显式动态边界，不能被编译器当作类型证明。Decorator
表达式的求值发生在 Python 模块加载阶段，其 effect 不混入被装饰函数的调用
summary。自定义 struct decorator 生成在 `@dataclass(frozen=True)` 外层，因此
接收到的是已经完成 dataclass 转换的 class。

## 5. 运行时语义

### 5.1 Python 值模型

Osiris 默认使用 Python 运行时值，不引入自有 VM、持久化集合或对象系统：

- `none` 生成 `None`。
- `Bool`、`Int`、`Float`、`Str` 和 `Bytes` 映射到对应 Python 类型。
- 顺序集合、映射和集合生成 Python 兼容对象。
- 普通异常映射为 Python 异常并按 Python 规则传播。
- 函数参数按从左到右顺序求值。
- `let` 绑定按源码从左到右依次求值，后续绑定可以引用先前绑定；这保证命名数据流水线不是无序 map。

第一版推荐把 `quote`/syntax object 限制在 phase 1；运行时代码不能构造或检查 quoted Symbol。`:name` 在宏求值中是独立 `Keyword` datum，在运行时 map/set 值位置降低为 Python 字符串，在调用参数位置表示 Python keyword argument。这样 `:name` 与 `"name"` 在运行时相等是明确语义，而不是偶然实现。若需要运行时保留 Symbol/Keyword 身份，必须引入单独的最小 runtime value 设计，不能把三者都静默编码为字符串。

编译器不得在未获明确许可时重排有副作用的表达式、数组运算或浮点运算。

### 5.2 可变性

- `defstruct` 实例默认不可变。
- 词法绑定默认不可重新赋值。
- NumPy 数组、Pandas/Polars 对象和其他 Python 对象遵循其运行时可变性。
- 对结构体的更新使用返回新值的 `with`/`assoc` 操作。
- v0 不提供显式 `set!`；未来引入可变局部绑定需要单独 OEP。

### 5.3 条件

核心 `if` 的条件在静态类型已知时必须是 `Bool`。`osiris.core` 的 `and`、`or`、`cond`、`when`、`when-not`、`if-not`、`while` 以及 `for`/`forv`/`doseq` guard 接受任意静态值，并先通过有类型契约的 `truthy*` 降低为 `Bool`；它们采用 Clojure truthiness，只有 `none` 和 `false` 为假，`0`、空字符串和空集合仍为真。`if-let`/`when-let` 使用同一 truthiness，`if-some`/`when-some` 则只把 `none` 视为空。普通 `if` 不会隐式采用这些规则。

## 6. 类型系统

### 6.1 基本模型

Osiris 使用静态、严格但渐进的类型系统：

- 默认对 Osiris 代码进行静态检查。
- `Any` 是显式退出静态检查的 Python 互操作逃生口。
- `Unknown` 仅用于推断过程和 LSP 未完成代码，不能出现在发布接口中。
- `Error` 是诊断恢复类型，用于阻止级联错误。
- `Never` 表示不返回的表达式。

类型检查发生在宏展开之后、Python codegen 之前。生成的 Python 保留尽可能完整的标准 `typing` 标注，但 Python 类型标注不是 Osiris 类型检查器的实现基础。

`Any` 不能静默赋给具体静态类型。`Any -> T` 必须经过显式 checked cast、schema validation 或标记原因的 unsafe cast；`T -> Any` 可以隐式发生，但类型来源必须保留在 HIR 和 LSP 中。公开接口中的 `Any` 必须由作者显式写出，不能由失败的推断自动产生。

### 6.2 第一版类型

```text
Bool Int Float Str Bytes None Any Never
(Option T)
(Union A B ...)
(Tuple A B ...)
(List T) (Vector T) (Map K V) (Set T)
(Fn [A B ...] -> R)
名义 struct 类型
显式泛型类型
结构化 record/schema 类型
```

`Array`、`Series` 和 `Frame` 是标准数据接口提供的参数化类型，不是 Rust 编译器硬编码的 Pandas/Polars 类型。

```clojure
(Array Float [:time :feature])

(Frame
  {:time Datetime
   :source Str
   :value Float}
  :key [:time :source]
  :order [:time])
```

核心类型系统只需要支持名义泛型、结构化字段类型和必要的 literal type 参数。具体数组 dtype promotion、DataFrame 表达式和后端能力由接口包声明。

名义类型在 typed HIR 和 `.osri` 中必须以定义点的完整稳定 type `BindingId` 标识，规范形式为 `module::type::Name`。短类型名只用于源码/LSP 展示和生成 Python annotation，不能作为类型相等、alias、operator instance 或跨模块接口解析的身份。类型引用必须唯一解析到该 `BindingId`；未知或存在多个候选都必须产生编译诊断，不能按 import 顺序、短名或 Python annotation 猜测。

`(Option T)` 表示可能不存在的值，运行时直接映射为 `T | None`，不引入 `Some` wrapper。目标 Python 最低为 3.11，因此生成代码可以直接使用该 union 语法。`Float` 中的 NaN 仍然是浮点值，不能等同于 `none`。DataFrame null、NaN 和 backend-specific missing value 必须由数据接口分别建模。

### 6.3 推断与公开接口

- 局部 `let`、私有函数体和 lambda 返回值应当支持双向局部推断。
- 导出的函数参数和返回值必须有明确类型。
- `defstruct` 字段必须有明确类型。
- 泛型参数必须显式声明，第一版不做跨模块全局 Hindley-Milner 推断。
- `Union` 的收窄由 `if`、空值检查和后续 `match` 支持。
- Python 动态属性、`getattr`、`eval`、`exec` 和未知 callback 必须产生 `Any` 以及 unknown/dynamic 语义摘要。

### 6.4 函数标注

```clojure
(defn ^{:type (Array Float [:row])} apply-threshold
  [^{:type (Array Float [:row])} values
   ^{:type ThresholdConfig} options]

  (if options.enabled
    (clip values options.range)
    values))
```

参数、返回值和 `let` binding 使用 Rich Metadata 的 `:type` 或 Clojure `:tag` 表面，例如 `^{:type (Vector Int)} values`、`^Int value`、`^{:type Int} local`。私有函数可以省略可推断部分；公开函数和宿主边界必须给出完整、稳定的类型。裸 `^Vector` 等容器标签按第 8.8 节解释为 `Any` 参数化容器。`defmacro` 与 `defn-for-syntax` 的参数天然是 phase-1 `Syntax`，不要求运行时类型标注。

### 6.5 数值规则

- `Int` 对应 Python 任意精度整数。
- `Float` 对应 Python `float`。
- 标量数值提升必须由语言规范固定，不能依赖偶然的 Python 实现行为。
- NumPy dtype promotion 由 NumPy 接口声明，编译器不自行模拟。
- 默认禁止 fast-math、结合律重排和跨调用代数化简。

#### 6.5.1 封闭的静态 operator capabilities

数据中心语言不能把 `+`、`-`、`*`、`/`、比较和 `abs` 限定为 core scalar，否则 Array 和 Series 的普通公式无法在严格类型下成立。v0 因此提供一个最小、封闭、纯静态的 operator capability 层；它不是完整 protocol 或运行时 overload 系统。

- `osiris.core` 为每个 operator 定义稳定 operator id，并内置 scalar instances。
- Array/Series/Frame 等类型的所有者可以在 `.osri` 声明有限或受约束的静态 instance，例如 `(Series Float) * Float -> (Series Float)`，同时给出 effect/temporal/data transfer 和 canonical runtime/intrinsic binding。
- instance 声明必须至少拥有一个 operand 的最外层名义类型，禁止第三方 orphan instance；同一 operand type tuple 在有效依赖图中只能有一个 instance，选择结果不能取决于 import 顺序。
- typed HIR 必须在编译期选出唯一 instance 并记录其 binding/contract id；零个或多个候选都是编译错误。`Any` 边界只能走显式 dynamic Python 调用，不能假装完成静态选择。
- 变参 `+`/`*` 和多操作数比较按源码顺序降低为已解析的二元 instance，并保持每个 operand 只求值一次；不得为优化重新结合。
- `abs` 虽然表面是普通 core 函数，也可以使用相同的封闭 capability 选择 Series/Array instance；alias 仍解析到该 canonical binding，不建立第二套分派。
- instance 来自依赖 wheel 时仍只是第 8.8.1 节的 declared contract；只有其 contract DSL 能被当前编译器完整验证时，严格 causal/schema 区域才能把 summaries 用作证明。

开放式 user protocol、多方法、运行时 dispatch、任意第三方 overload 和完整 Python operator emulation 延后。该最小层只覆盖数据公式所需、由类型所有者静态发布的 operator instances，使编译器核心无需硬编码 Pandas、Polars 或 NumPy。

### 6.6 函数的 latent summaries

函数类型除参数和返回类型外，还必须携带“调用该函数时”产生的 latent `EffectRow`、`TemporalSummary` 和 `DataProperties` transfer；`TemporalSummary` 同时组合 event-time bounds 与第 13.1 节的 availability 子摘要。求值函数值本身的摘要与调用它的 latent summary 是两个概念。

源码中的裸函数类型 `(Fn [A B ...] -> R)` 若没有显式 latent effect、temporal 和 data summaries，三者必须采用 `unknown` 保守上界，不能默认成 pure、无未来依赖或 scalar data。该规则适用于函数类型表达式和 callback 标注；普通 `defn` 仍从可见函数体推断 summaries。

高阶函数接口可以参数化 summary 变量，并使用有限的声明式组合表达式：effect union、temporal join/shift、literal constraint，以及 data-property preserve/replace/reshape。`.osri` 必须序列化这些变量、约束和 transfer expression，不能把它们提前折叠为 unknown。

例如 `map` 的 contract 应表达：结果类型由 callback 返回类型决定；调用 effects 合并输入求值和 callback latent effects；时间摘要合并输入与 callback 依赖；长度和主 axis 保持。`reduce` 还要声明顺序归约；`lag(n)` 在 `n` 是非负常量时产生有限过去偏移，在只能证明 `n >= 0` 时产生符号过去边界，在连方向都无法证明时才产生 temporal unknown。标准 `for` 宏展开到这些有契约的高阶 primitive，因此不另造分析规则。

contract 语言不是可执行 analyzer。第一版若无法用上述有限代数表达某项传播，必须保守返回 unknown。普通函数的 summaries 可以从函数体推断；公开 higher-order API 和 `extern` contract 中未闭合的 summary 变量必须显式声明。

## 7. `defstruct`

`defstruct` 是 Osiris 核心声明，为类型系统、LSP 和 Python codegen 提供稳定的名义结构。

### 7.1 声明

```clojure
(defstruct (Range T)
  "闭区间。"
  [min T]
  [max T])

(defstruct ThresholdConfig
  [enabled Bool = true]
  [range (Range Float) = (Range 0.0 0.0)]

  (check (<= range.min range.max)
         "范围下界不能大于上界"))
```

### 7.2 语义

- struct 是名义类型，不因字段恰好相同而自动兼容。
- 字段有固定顺序、类型、默认值、文档和源码位置。
- 默认表达式必须是 pure、类型正确，并在每次省略该字段的构造中求值；第一版不能引用其他字段或未完成的 `self`。
- 构造器支持位置参数和关键字参数，并静态检查缺失、重复和未知字段。
- 默认不可变，并生成可读 `repr`；只有所有字段都声明标量 equality capability 时才提供结构化相等性，NumPy 的逐元素比较不能被误当成 struct 布尔相等。
- 不可变只约束 struct 字段不能重新绑定；若字段引用 NumPy 数组或其他可变 Python 对象，不自动获得深不可变性。
- 泛型参数可以出现在字段类型中。
- `check` 表达式必须是 pure 且具有 `Bool` 类型，并生成运行时构造不变量。
- 带 `check` 的构造器具有 `throw` effect；检查失败生成确定的构造异常。
- 编译器可以对全字面量构造提前执行常量检查，但不能假装证明运行期数据。
- 第一版不支持 struct 继承和 struct 内方法，优先使用组合和普通函数。

### 7.3 使用

```clojure
(def options
  (ThresholdConfig
    :enabled true
    :range (Range 0.0 0.2)))

options.range.max

(let [{:keys [enabled range]} options]
  ...)

(with options :enabled false)
```

### 7.4 Python 输出

```python
from __future__ import annotations

from dataclasses import dataclass, field


@dataclass(frozen=True, slots=True)
class ThresholdConfig:
    enabled: bool = True
    range: Range[float] = field(default_factory=lambda: Range(0.0, 0.0))

    def __post_init__(self) -> None:
        if not self.range.min <= self.range.max:
            raise ValueError("范围下界不能大于上界")
```

除编译器证明可以安全共享的 Python 常量外，字段默认表达式必须使用 `default_factory` 或等价构造路径，以保持“每次构造求值”的 Osiris 语义。

以上 Python 示例使用 `dataclass(slots=True)`。Osiris 的目标 Python 最低为 3.11，codegen 不需要保留 3.9 的兼容分支，并且不能输出目标解释器无法解析的代码。

## 8. 宏系统

### 8.1 定位

宏是 Osiris 的主要领域扩展机制，但不是绕过类型和语义检查的机制。宏接收并返回带源码位置和词法上下文的 `Syntax`，展开结果必须降为可正常解析、解析名称并类型检查的 Osiris 核心形式。

`defstruct`、名称解析、类型、语义摘要和 Python codegen 不通过普通宏实现。

### 8.2 创建宏

```clojure
(defmacro unless [condition & body]
  `(if (not ~condition)
     (do ~@body)
     none))
```

- `` ` `` 创建 syntax template。
- `~` 插入一个调用方语法对象。
- `~@` 展开并插入一组调用方语法对象。
- 宏参数隐式具有 `Syntax` 或 `Vector[Syntax]` 类型。
- 宏展开完成后才检查生成表达式的运行时类型。

每次宏调用还隐式绑定 Clojure 风格的 `&form`：它是包含 head、参数、metadata、span 和词法上下文的完整调用 `Syntax`，所以声明宏可以用 `(meta &form)` 读取调用点 metadata。v0 不提供可枚举或可修改的 `&env`；宏仍不能观察推断类型或运行时值。

声明宏示例：

```clojure
(module data.resources)
(import-for-syntax osiris.syntax :as syntax)
(export [with-resource])

(defmacro with-resource [binding & body]
  (if (syntax/valid-binding? binding)
    `(let [~(first binding) ~(second binding)]
       (do ~@body))
    (syntax-error &form "with-resource requires one binding pair")))
```

宏只负责建立语法结构，运行时行为仍由展开后的普通核心形式承担。领域包可以继续组合这种声明宏，但不能在宏中隐藏不可追踪的运行时逻辑。

### 8.3 编译期辅助函数

```clojure
(defn-for-syntax parse-fields [forms]
  ...)

(defmacro defschema [name & fields]
  (build-schema-expansion name (parse-fields fields)))
```

`defn-for-syntax` 只存在于 phase 1。运行时函数不能被宏隐式执行；编译期函数也不能泄漏为 Python 运行时调用。

### 8.4 卫生

Osiris 宏必须默认卫生：

```clojure
(defmacro twice [expr]
  `(let [value# ~expr]
     (+ value# value#)))
```

- `value#` 每次展开产生唯一绑定。
- 模板中直接写出的名称按宏定义位置解析。
- 通过 `~` 和 `~@` 插入的语法保留调用方上下文。
- 宏不能意外捕获或覆盖调用方局部名称。
- 必须使用显式低层 API 才能有意引入调用方可见绑定；该 API 应当被 lint 标记。

### 8.5 phase-1 求值器

宏不编译成 Python 再执行。Rust 编译器提供一个小型、确定性的 phase-1 求值器，第一版支持：

- `let`、`if`、`do`、函数调用和有限递归。
- `Bool`、`Int`、`Float`、`Str`、`Keyword`、`Symbol`、List、Vector、Map、Set 和 `Syntax`。
- syntax 构造、拆解、模式匹配、遍历、metadata 读取/更新、`gensym` 和 `syntax-error`。
- 纯编译期辅助函数。

phase 1 禁止：

- Python import 或执行 Python 函数。
- 文件、网络、环境变量、时钟、随机数、线程和子进程访问。
- 修改项目文件或依赖环境。
- 读取推断后的运行时类型。

编译器必须限制宏递归深度、求值步数、内存和展开结果大小。相同输入和接口必须得到相同展开结果。

### 8.6 第一版限制

- 只允许 list head 位置的宏和顶层声明宏。
- `module`、`import`、`import-for-syntax`、`py/import`、`export`、`alias`、`defmacro`、`defn-for-syntax` 和 `defstatic-schema` 必须直接出现在源码 header/phase-1 声明中，宏不能生成这些形式或改变已经建立的依赖图。
- 顶层声明宏只能生成 phase-0 `def`、`defn`、`defstruct`、`extern`、`py/decorate` 和非执行的 `static-record` 等声明；源码中的直接 `export` 可以提前引用展开后才产生的名称。
- 不允许 reader macro 或修改 tokenizer/parser。
- 不允许 Python、Rust 动态库或 WASM procedural macro。
- 不允许宏根据类型推断结果改变展开；类型相关检查在 typed HIR 阶段完成。
- 宏展开必须保留完整 origin chain，供诊断、LSP 和 source map 使用。

### 8.7 可观测性

```console
osr expand source.osr
osr expand source.osr --once
```

LSP 必须提供宏定义跳转、调用签名、展开预览和“诊断来自哪次展开”的信息。宏不应成为不可检查的黑盒。

### 8.8 Rich Metadata

Rich Metadata 是附着在语法上的不可执行数据。v0 对齐 Clojure 1.12 当前 `^` reader 在 Osiris 支持节点上的表面和不可变 API，但不复制旧式 `#^`、JVM type hint、Var 或任意运行时对象 metadata。示例：

```clojure
^:deprecated
^{:doc {:default "Normalize measurements."
        "zh-CN" "归一化测量值。"}
  :since "0.1"
  :osiris/names
  {"zh-CN" {:preferred 归一化
             :aliases [标准化]}}
  :agent/intent :data/normalize
  :agent/tags [:data :transform]}
(defn normalize ...)
```

reader 接受与 Clojure 1.12 相同的五种 metadata 输入，并统一为 map：

- `^{:key value}` 直接使用 map。
- `^:flag` 等价于 `^{:flag true}`。
- `^TypeTag` 等价于 `^{:tag TypeTag}`。
- `^"tag"` 等价于 `^{:tag "tag"}`。
- `^[TagA TagB _]` 等价于 `^{:param-tags [TagA TagB _]}`。

metadata 参数只能是 Map、Symbol、Keyword、Str 或 Vector；其他形态是 reader error。metadata 附着到紧随其后的 Symbol、List、Vector、Map 或 Set syntax node；不允许附着到数字、字符串、keyword 等不支持 metadata 的标量 datum。多个前缀按 Clojure 规则从右向左合并，源码中更靠左的前缀在重复 key 上胜出。`^TypeTag` 和 `^[...]` 默认产生 authored metadata。为降低 Clojure 学习成本，声明位置上的 `:type`/`:tag` metadata 也可作为类型表面：`^:type (Vector Int)`、`^{:tag (Vector Int)}` 可标注函数返回值、参数或 `let` binding；显式 `[name Type]` 与 `-> Type` 优先，冲突会产生稳定诊断。裸 `Vector`、`List`、`Set`、`Option` 标签只表示动态容器边界（元素/内部类型为 `Any`），裸 `Map` 表示 `Map[Any, Any]`，不会凭名称猜测元素类型；其他 `^TypeTag` 仍只是文档 metadata。

phase 1 必须提供与 Clojure 对应的读取和更新 API：

```clojure
(meta syntax)                    ; -> Map 或 none
(with-meta syntax metadata)      ; 替换 metadata，datum/context 不变
(vary-meta syntax f & args)       ; metadata = (apply f (meta syntax) args)
```

`meta`、`with-meta` 和 `vary-meta` 也适用于 phase-1 的 Symbol、List、Vector、Map 和 Set 值。`with-meta` 接受 Map 或用 `none` 清除 metadata；`vary-meta` 中的 `f` 必须是 phase-1 纯函数。三者都返回不可变的新值，不执行 metadata 中的内容。datum equality 和普通 hash 不因 metadata 不同而改变，但宏展开缓存和确定性 syntax hash 必须包含 metadata，因为宏可以读取它。syntax quote 保留模板节点 metadata，`~`/`~@` 保留插入 syntax 的调用方 metadata；宏若重建节点，应显式决定传播、替换或丢弃 metadata，编译器始终补充不可伪造的 expansion origin。

直接核心声明把整个声明 form 的 metadata 归到 declaration，把 name、parameter 和 field syntax 上的 metadata 分别归到对应实体。声明宏没有隐式复制规则：它通过 `(meta &form)` 读取调用点，再用 `with-meta`/`vary-meta` 明确附着到生成的声明或第 8.9 节的静态记录。`osr expand`、`osr lsc` 和 LSP 使用可被同一 reader 再读入的规范化 `^{...}` 形式显示 authored metadata。

Clojure reader 会给部分 form 添加 `:line`/`:column`，Osiris 则把 source span、macro expansion origin 和 authored metadata 分开保存；宏用只读 `syntax/span` 查询位置，不能用 `with-meta` 伪造 origin。v0 不提供运行时 Var metadata、`alter-meta!`、`reset-meta!` 或 `*print-meta*`，这不影响 phase-1 reader/API 与工具读取闭环。

以下 metadata key 由语言或工具标准化：

- `:doc`：无翻译时可直接写非空 default string；有翻译时写包含 `:default` 无标签回退内容和 BCP 47 locale translation 的 map。可复用 package 推荐用英文写 `:default`，但 compiler 允许中文、日文或其他 authored default。声明 docstring 是 default string 形式的语法糖。
- `:since`、`:deprecated`、`:replacement`：API 生命周期信息；deprecated alias 可以给出独立替代名和原因。
- `:osiris/names`：第 4.6 节的本地化 preferred name 和 alias 表。
- `:agent/intent`、`:agent/summary`、`:agent/tags`、`:agent/examples`：供检索、选择和说明使用的作者 metadata，不是编译器证明。
- 其他扩展 key 必须使用 namespace，例如 `:data/unit`、`:service/endpoint`、`:render/group`；未知 namespaced key 被保留，但不获得编译语义。

`:doc`、`:osiris/names` 的规范结构、locale fallback 和 JSON 投影由
[OEP-0001](../oeps/0001-language-and-cli.md) 定义。

metadata 值必须是可规范化序列化的 phase-1 datum：`none`、Bool、有限数值、Str、Keyword、Symbol，以及由它们组成的 List/Vector/Map/Set。metadata 不能包含函数、Syntax context、Python 对象、可执行代码，metadata datum 内也不能再附着 metadata。v0 对单个 syntax target 限制为最多 32 层 datum 嵌套、128 个 authored metadata entry（多个 reader layer 合计）、2048 个 datum node 和 64 KiB 规范化 UTF-8 数据；一个公开声明连同其参数或字段合计最多 512 entries、8192 nodes 和 256 KiB；一个 `.osri` 接口合计最多 4096 entries、65536 nodes 和 2 MiB。Reader 超限产生可恢复诊断并丢弃该 target 的 metadata，不吞掉 target 或后续 form；phase-1 `with-meta` 使用相同单 target 限制。`.osri` build 和 read 必须在规范化、hash 或递归处理前独立验证单 target、单声明和整接口预算，直接 API 构造或伪造接口也必须 fail closed。这些预算只约束 metadata，不约束普通业务数据 form。

#### 8.8.1 作者信息、静态记录、声明契约与已证明事实

工具必须把四类信息分开呈现：

1. **authored metadata** 是作者或宏提供的名称、文档、意图、单位、示例和展示提示。
2. **static records** 是第 8.9 节经过 schema 形状和 index 校验的 manifest data；它们结构有效，但不是语义证明。
3. **declared facts** 是源码或依赖 `.osri` 中的 extern/intrinsic contract 所声明的类型、effect、temporal、data 和 availability；它们有来源，但尚未因写进 wheel 而获得信任。
4. **verified facts** 是当前编译器从本地函数体推导，或由 typed HIR pass 从可完整检查的 contract DSL 推导得到的事实。

源码或宏不能通过写 `^{:pure true}`、`^{:lookahead 0}`、`:osiris/type` 或相似 key 伪造 verified facts。扩展 metadata 只有在声明宏明确降低为第 8.9 节的 schema-checked `static-record` 或带 stable contract/intrinsic id 的既有核心形式后，才获得结构化扩展含义；未知 namespaced key 仍只是被保留的 authored metadata。phase-1 macro 不能读取推断类型或签发语义事实。后续 typed HIR pass 必须检查展开后的 contract DSL，只有检查成功才能提升状态；原 metadata、static record 和 declared fact 始终保留来源。LSP 和 Agent API 必须分别返回 `authored`、`records`、`declared`、`verified` 和 `origin`，不能合并成一张无来源 map。

来自依赖包的文档和 Agent metadata 按不可信数据处理：编译器从不执行它，LSP 渲染前清理命令链接和不安全 HTML，Agent 不得把其中的自然语言提升为系统指令或权限。静态资源仍遵守第 14.2 节的路径和大小限制。

#### 8.8.2 保留与运行时边界

- CST 保留所有 metadata、原始 spelling 和 span，phase-1 `Syntax` 可以读取它。
- 展开后的 HIR 保留仍有目标节点的 metadata 和完整 macro origin chain。
- `.osri` 保存公开 declaration、macro、type、parameter、field 和 schema element 的规范化 authored metadata，以及与其分离、带 provenance 的 declared/producer-derived facts；“在当前环境已 verified”是加载方状态，不能由接口文件自我授予。
- `.py.map` 保留 metadata/source/expansion 的关联；任意 metadata 默认不生成 Python decorator、wrapper 或全局 registry。
- 可以将明确标准化的 `:doc` 生成 Python docstring，或由扩展显式把字段 metadata 降为 `typing.Annotated`/dataclass metadata，但该映射必须进入 contract 和目标 Python 兼容性检查。

v0 的 `meta`/`with-meta`/`vary-meta` 是 phase-1 能力，工具通过 compiler database 或 `.osri` 读取公开 metadata。普通 phase-0 Python 值，尤其 NumPy Array、Pandas/Polars 对象和内建容器，不自动获得 Clojure 式运行时 metadata；否则必须包装所有宿主值，违背 Python 原生互操作和小 runtime 目标。若业务确实需要运行时 metadata，应以后以显式 `Meta[T]`/schema adapter 设计，不得悄悄给任意 Python 对象加 side table。

### 8.9 类型化静态 schema 与 records

扩展经常需要把一部分 authored metadata 变成可校验、可合并的领域 manifest，例如组件 ID、兼容 ID 和宿主入口。Osiris 为此只提供两个领域无关的核心 form，不为具体框架增加编译器分支：

```clojure
(defstatic-schema ComponentDescriptor
  :schema-id "example.component/descriptor"
  :version 1
  :fields
  {:component/id             {:type Str :required true}
   :component/legacy-ids     {:type (Vector Str) :default []}
   :component/entrypoints    {:type (Set (OneOf :service/run
                                                :service/batch))
                              :required true}}
  :indexes
  [{:id "example.component/runtime-id"
    :scope :effective-dependency-graph
    :keys [{:field :component/id :role :canonical}
           {:each :component/legacy-ids :role :legacy}]}])

(static-record component/ComponentDescriptor normalize
  {:component/id "example.normalize"
   :component/legacy-ids ["example.standardize"]
   :component/entrypoints #{:service/run :service/batch}})
```

`defstatic-schema` 必须是直接顶层声明，可以被 export/import。它由 namespaced stable schema id、正整数 version、默认封闭的字段集合、静态默认值和可选的声明式 index 组成；未知字段、缺失必填字段和类型不匹配都是编译错误。schema 不能包含函数、动态默认值、Python 对象、正则 validator 或 analyzer callback。v0 字段类型只覆盖第 8.8 节的可序列化 datum、`OneOf`、`Optional` 及其 List/Vector/Map/Set 组合。breaking schema 变更必须提高 version；规范化 schema 及其 semantic hash 进入 `.osri`。effective graph 中 `(schema-id, version)` 相同且 body hash 相同的 schema 可以去重，body hash 不同必须报错；不同 version 是不同 schema，record 必须引用精确 version/body hash，不能隐式选择“最高版本”。

`static-record Schema owner {...}` 是只能出现在顶层的非执行声明，`owner` 必须解析为当前模块的顶层 declaration，并保存为稳定 binding id。同一 owner 对同一 schema 最多有一个 record。它不创建 phase-0 值，不单独生成 Python，也不执行字段内容；声明宏可以生成它。宏模板中的 Schema 按定义位置卫生解析并以稳定 schema binding id 打包进 macro IR，owner 则可以由调用方 syntax 插入，所以消费者无需重新猜测扩展内部 alias。编译器在宏展开和名称解析后按 schema 校验字段、实体化静态默认值、规范化 map/set、记录 source/macro origin，并给 record 分配由 schema binding/id/version/body hash 和 owner binding id 构成的稳定 record id。record-body hash 覆盖全部规范化字段及由 schema 投影出的 index id、projection role、normalized/raw key claims。

规范的 `RecordOccurrenceId` 是 `(distribution, version, interface-member-id, semantic-interface-hash, stable-record-id, record-body-hash)`。它在接口 group hash 完成后组装；同 stable id 来自不同 provider envelope 或不同 record-body hash 时不能去重，并由第 9.3 节的门禁或 index collision 拒绝。这个 post-hash provider envelope 不进入产生该 hash 的 semantic/tooling/interface body；SCC 内 re-export 在 pre-hash body 中只记录 defining member id、stable record id 和 record-body hash，group hash 完成后再补完整 `RecordOccurrenceId`，避免接口 hash 自引用。

owner 被导出时 record 才是 public：它作为 defining distribution 的 owned public record 进入公开 `.osri`、跨包 index 和 runtime record manifest，且 schema 及 record 引用的声明必须可由下游接口解析。private owner 的 record 只留在当前模块的 compiler database/build 输入，只参加 module-local schema/index 校验，不与依赖 public records 冲突，也不得进入 wheel runtime manifest。re-export 只写指向原 occurrence identity 的 `record-ref`，不能在 re-exporter 的 owned-record section 或 sidecar 复制注册项。

声明式 index 只做静态 datum 的投影、规范化、唯一性和确定性排序。`:field` 投影单值，`:each` 投影集合中的每个值；`:role` 是 schema 提供的通用诊断标签，不赋予领域语义。相同 index id 的所有 key 共用一个 keyspace。编译器先拒绝单个 record 内 canonical/legacy 等不同 projection claim 产生的重复 key，再对第 9.3 节 effective graph 中的 public records 合并。任何不同 record occurrence identity 的重复 key 都是编译错误，即使 owner 相同而 schema 不同；只有完全相同的 occurrence identity 和 index claim 因菱形依赖或 re-export 被再次到达时才去重。不提供 first-wins、依赖优先级或隐式 override。

static schema 只证明“数据满足声明的形状和索引约束”，不把 record 字段提升成类型、effect、temporal、availability 或 data-property fact。宏必须另行展开到语言已有的 typed contract/intrinsic 核心形式，后续 typed HIR pass 才能按第 8.8.1 节验证；schema、record 和 dependency metadata 始终是不可信静态数据。任意 namespaced metadata 也不会自动变成 record，声明宏必须从 `(meta &form)` 明确选择字段并生成 `static-record`。

`.osri` 和 compiler database 分区保存 schema、records、owner、origin 和 index entries；`osr lsc semantic --format json` 与 LSP 返回同一版本化结构。`.osri` 是编译和工具的唯一权威来源；`osr compile` 的通用 encoder 按第 14.2 节把本 distribution 的 public records 机械投影为确定性的纯数据 runtime manifest，`osiris_build` 只校验并打包同一产物。声明宏可以为 public record 生成薄 adapter：生成模块把 owner Python callable、完整 `RecordOccurrenceId` 和预期 `record-set-hash` 一起传给扩展 runtime loader，由 loader 校验 sidecar descriptor 后绑定 callable。loader 依据 occurrence 中的精确 distribution/version 直接定位该 distribution marker，核对 interface/records hashes 后读取声明路径；不得枚举环境或按同名 package 猜测。sidecar 不承担 Python import/name locator，何时 import 生成模块由扩展宿主负责。adapter 不能另嵌一份可漂移的 ID/descriptor map；引用 private record 必须编译失败，private owner 只能保留本地 kernel/工具记录。构建期间不得 import/执行扩展 Python 代码。这样领域包可以扩展 registry 和渲染数据，而 Rust 核心只理解通用静态记录。

## 9. 模块、项目与依赖图

### 9.1 项目根

项目根由最近的 `osiris.jsonc` 确定；相邻 `pyproject.toml` 只负责 Python
distribution 元数据、依赖和 PEP 517 backend。

```jsonc
{
  "source": ["src"],
  "outDir": "dist",
  "exclude": ["src/**/generated/**"],
  "targetPython": "3.11",
  "strict": true,
  "displayLocale": "zh-CN"
}
```

`src/foo/bar.osr` 默认映射到 Osiris 模块 `foo.bar` 和 Python 产物 `foo/bar.py`。模块声明必须与文件路径一致。Python 发行版名称和 Python/Osiris 模块名称是两个概念，不得混用。

`displayLocale` 接受任意合法 BCP 47 locale tag，默认 `"zh-CN"`。它是 LSP project fallback，只决定 hover、completion 和 signature help 在 Rich Metadata 中优先选择的本地化 label/doc，不改变 binding identity、类型检查或生成的 Python。LSP 标准 `InitializeParams.locale` 优先于项目值。`osr lsc` 不继承该配置：没有 `--locale` 时选择 authored `:default` 和 canonical name；显式 locale 使用 RFC 4647 lookup，缺少匹配翻译时回退到无标签 `:default`，且不得伪造 resolved locale。

领域宏可以把高层规则展开为版本化 contract DSL，但宏自身不能签发 `verified` 事实。展开后的 DSL 必须由 typed HIR pass 依据编译器已知的保守规则验证；无法检查的外部 Python 行为保持 `declared/unknown`，不能参与因果性或未来数据证明。compiler ABI 固定的内建规则和当前项目源码中直接推导的事实无需项目 allowlist。

### 9.2 导出

顶层定义默认私有，公共 API 使用显式 `export`。这样 `.osri` 稳定、重构边界明确，也避免把宏辅助函数意外发布。

### 9.3 编译阶段依赖

- 运行时导入形成 runtime module graph。
- 宏和 `defn-for-syntax` 形成 phase-1 graph。
- phase-1 graph 必须无环。
- runtime cycle 可以映射 Python 行为，但编译器应当诊断高风险循环导入。
- 编译期导入不得隐式依赖调用方运行时初始化。

本文的 **effective dependency graph** 是：当前项目模块，以及普通 Python 依赖中通过 runtime import、公开接口引用和扩展 marker 到达的 `.osri` 传递闭包。扩展不需要项目配置开关：编译器只自动读取当前 target 下 `uv.lock` 可达 distribution wheel 内的 `osiris.toml`，不得扫描环境中偶然安装但不可达的 wheel。仅被 `import-for-syntax` 到达、且没有向当前模块展开 public static record 的宏工具包不进入 runtime registry index。相同 interface 经多条路径到达时去重；同一 interface id 同时解析到不同 distribution version、wheel hash 或 semantic hash 时必须 fail closed。

编译器必须先解析 module、import、export 和 phase-1 binding，加载依赖 `.osri`，然后才能执行卫生宏展开。展开完成后再解析运行时局部绑定和生成 typed HIR。普通 import 可以依据 `.osri` 中的符号种类导入函数、类型或宏。宏实现自身需要的 phase-1 helper 使用显式 `import-for-syntax`，避免阶段混淆。

## 10. `.osri` 编译接口

`.osri` 是从 `.osr` 自动生成的 Osiris compilation interface，不是作者维护的头文件，也不是 Python `.pyi` 的替代品。

### 10.1 内容

`.osri` 至少包含：

- format schema、compiler ABI 和 language ABI。
- 模块名称、公开文档和导出表。
- canonical export、稳定 binding id，以及按 phase/kind 分组的 public alias、locale、preferred/deprecated 状态和 replacement。
- 参数和 struct 字段的 canonical id、静态 keyword/field aliases，以及 alias 自身的 since/deprecation 信息。
- `defstruct` 布局、泛型参数、字段类型、默认值签名和不变量签名。
- 类型别名，以及公开函数的类型、effect、temporal 和 data-property 摘要。
- 导出宏的调用模式、文档和编译后的 phase-1 IR。
- 导出宏所需私有 phase-1 helper 的闭包。
- 宏模板中定义点引用的稳定 binding id、目标模块/export、phase 和可重定位 origin。
- 公开 authored metadata、metadata schema version 和来源；declared/producer-derived facts 使用独立字段，不能混入 authored map，也不能携带可让包自我授信的权威 trust class。
- 导出的 static schema：schema binding/id/version、字段类型、静态默认值、index 投影、schema body hash 和 provider provenance。
- owned public static records：稳定 record id、schema identity/body hash、owner binding id、规范化字段，以及包含 index id、projection field/role、normalized key/raw spelling 的 index claims 和 source/macro origin；re-exported records 只保存 defining member + stable record/body hash reference。owned records、re-export refs、authored metadata 与 declared facts 分区保存。
- 不含 hash/dependency/post-hash provider-envelope section 的 module interface/semantic/tooling body hashes、hash-dependency SCC semantic/tooling group hashes、外部依赖 hashes，以及计算时省略自身字段的文件完整性 hash。

`.osri` 不包含普通运行时函数体，也不作为 Python 运行时代码执行。

### 10.2 属性

- `.osri` 必须由 `osr` 确定性生成，禁止手工编辑。
- 本地源码存在时可以随时删除并重建。
- 发布 Osiris 库或宏扩展时必须随 wheel 携带。
- 下游只读取 `.osri` 即可完成名称解析、类型检查、LSP 索引和宏展开。
- `.osri` 兼容性由 schema、compiler ABI 和 language ABI 门禁，不承诺跨不兼容 ABI 读取。
- 来自 wheel 的 macro IR 仍是不可信输入，必须经过格式、ABI、hash 和资源上限校验，并在同一个 sandbox evaluator 中执行。
- 来自 wheel 的类型和 intrinsic summaries 默认是带 provenance 的 declared contracts；格式、ABI、hash 或 lock pin 只证明读到了预期字节，不证明 contract 正确。
- static schema 和 public static records 进入 semantic interface body；扩展不能自行把 registry/build 字段标成 tooling-only。schema/hash 只证明结构与预期字节一致，不证明 record 内容真实，也不授予 contract trust。

导出宏在模板中引入的运行时名称只能引用语言核心或定义模块可导入的公开 runtime export；第一版禁止引用定义模块的私有 phase-0 helper。编译器在生成 `.osri` 时将别名解析为稳定的定义点 binding id，下游展开后据此生成明确 import。私有 phase-1 helper 可以随 macro IR 闭包打包，因为它们不进入生成 Python。

binding id 和 origin 不得包含安装机器的绝对路径。它们至少由 distribution/module、公开名称、phase 和 ABI 身份构成；source origin 使用模块相对位置与 span，保证 wheel 安装后的卫生解析和诊断仍可重定位。

依赖 contract 只有在其 DSL 可由当前编译器完整验证时，才能用于严格 causal/schema proof。无法验证的 contract 仍可作为普通互操作声明使用，但在要求证明的区域必须保持 declared/unknown 并产生诊断。`osiris.toml`、`.osri`、宏或包内 metadata 都不能声明自己受信任，安装扩展也不等于接受其安全结论。

编译器先规范化不含 hash、dependency 和 post-hash provider-envelope section 的完整 **interface body**，并计算 `interface-body-hash`。再从同一 body 作两个确定性投影：类型、contract、binding、可解析的 public/parameter/field alias、public macro signature/phase-1 IR/private-helper closure、展开可观察的 macro metadata、static schema 和 records 构成 **semantic body** 并计算 `semantic-body-hash`；纯文档、显示顺序、标签和 Agent 摘要构成 **tooling body** 并计算 `tooling-body-hash`。format schema 必须明确每个字段属于 semantic、tooling 或两者，扩展不能自行选择；任何会改变下游宏展开的字节都必须进入 semantic body。这样改中文说明会刷新 LSP/Agent 索引，但不会无谓使所有下游 typed HIR 失效。规范化 metadata map 必须稳定排序；实现可以用 `MetaId` intern/结构共享并延迟解码 tooling section。

跨模块接口再区分 SCC 级 `semantic-interface-hash`、`tooling-metadata-hash` 与文件级 `content-integrity-hash`。post-hash record provider envelope 和 dependency section 可以携带已经完成的 group hashes，但它们不反向进入三种 body hash。`content-integrity-hash` 对将该字段省略后的最终 canonical 文件字节计算，验证时使用同一规则，避免文件 hash 自引用；wheel `RECORD` 仍独立校验最终文件字节。

hash-relevant interface graph 是 public runtime imports/interface references 与 phase-1 macro/helper imports 的带 edge-kind 并图；edge kind 和目标 member id 都进入规范化图。runtime graph 或 phase-1 graph 单独无环不能证明该并图无环，例如 `A runtime-> B` 与 `B phase-1-> A`。编译器完成每个 module 的三种 body hash 后，把这个并图缩成 SCC DAG；这只是 hash 计算分组，绝不放宽第 9.3 节“phase-1 evaluator graph 必须无环”的规则：

1. 每个 SCC 的 `semantic-interface-hash` 由排序后的 `(module-id, semantic-body-hash)`、SCC 内带 kind 的边集，以及所有 SCC 外部带 kind 边及其 semantic interface hashes 计算；SCC 内成员共享该 group hash。
2. `tooling-metadata-hash` 由排序后的 `(module-id, tooling-body-hash)`、本 SCC 的 `semantic-interface-hash` 和 SCC 外部依赖的 tooling hashes 计算；单模块无环节点视为大小为 1 的 SCC。
3. group hashes 完成后，编译器补齐 dependency section 与 record provider envelopes：SCC 内边/record provider 使用 member id + 当前 group hash，对外边使用 dependency member/SCC hash。它们不进入 body hashes。
4. `content-integrity-hash` 最后按“省略自身字段”的规则计算，仅用于当前文件完整性，绝不被其他接口的 semantic hash 引用。

这样 `A <-> B` 可以先共同生成 body、再一次计算 group hash，不需要寻找不存在的拓扑顺序。phase-1 graph 仍必须无环，不使用 SCC 规则放宽宏依赖。

第一版推荐使用规范化 S-expression 作为物理格式，以复用解析器并保持可检查；未来可以在不改变语义模型的情况下切换为二进制表示。

## 11. Python codegen 与互操作

### 11.1 Python 主产物

`.py` 是唯一可执行主产物，必须：

- 使用项目指定的目标 Python 版本语法。
- 保留标准 Python 类型标注。
- 为可发布包生成或携带 `py.typed`。
- 保留函数、结构体和主要局部变量的可读名称。
- 使用稳定格式和确定性 import 顺序。
- 生成常规 Python 控制流，而不是运行时解释 Osiris AST。

所有 Osiris alias reference 在 lowering 前已经解析为 binding/parameter/field id；codegen 只为 canonical id 分配 Python 名称。普通 alias 不产生 `中文名 = canonical_name` 或重复 wrapper。metadata 默认也不产生运行时对象；只有第 8.8.2 节明确允许的映射可以进入 Python。

普通情况下不额外生成 `.pyi`。只有动态实现、C extension 或明确的 Python stub 发布场景才需要 `.pyi`。

### 11.2 Python 导入

```clojure
(py/import numpy :as np)
(py/import pandas :as pd)
```

这只声明运行时 import。编译器不得 import `numpy` 或 `pandas` 来完成编译。

没有 Osiris 类型绑定的 Python 值默认为显式 `Any`。扩展包可以提供声明：

```clojure
(extern python "math"
  ^{:doc "Return whether a scalar is finite."}
  (defn ^Bool isfinite [^Float value]))
```

`extern` 是显式 declared FFI contract，不会因为写在源码或 `.osri` 中就自动成为 verified，也不能为了方便伪造确定的返回类型；只有编译器能完整检查其 contract DSL 时，才能在严格证明区域提升它。例如任意 `Any` 输入的 `np.asarray` dtype/rank 都不确定，必须使用静态 instance/overload、显式 dtype 参数和可验证 transfer contract，或保守返回动态类型。长期可以从高质量 `.pyi` 生成这些声明，但第一版不要求 Rust 编译器理解完整 Python typing 生态。

### 11.3 调用与异常

- Python 属性、索引、位置参数和关键字参数必须有直接表达方式。
- Python 异常默认原样传播。
- `defstruct check` 生成运行时检查；普通类型默认不生成全量运行时检查。
- 从任意 Python 进入 Osiris 生成函数的边界可以选择生成验证 wrapper。
- 动态 import、`eval`、`exec`、反射式字段访问和未知 callback 必须标记 dynamic/unsafe。

### 11.4 宿主适配层

装饰器注册、框架全局变量、任意 Python 反射、文件系统上下文和 DataFrame/矩阵归一化应优先留在薄 Python adapter 或扩展运行时。Osiris 编译器不为单个框架内置这些语义。

## 12. 数据中心设计

### 12.1 分层

建议的包层次是：

```text
osr / osiris          编译器与核心语言
osiris.data           逻辑数据类型和通用操作词汇
osiris-numpy          Array/NumPy 后端
osiris-pandas         Frame/Pandas 后端
osiris-polars         Frame/Polars 后端
osiris-domain         可选领域宏、语义契约和框架 adapter
```

这些包通过 PyPI/uv 安装。编译器只理解它们公开的 `.osri`，不在 Rust 中识别 Pandas 或 Polars 类。

### 12.2 Array 与 Frame

- NumPy Array 是按 dtype、rank/axes 组织的数值容器。
- DataFrame 是按命名列、row schema、可选逻辑 key 和 order 组织的关系数据；逻辑 key 不是 Pandas index。
- 两者不能因为都能二维索引就使用同一套隐式语义。
- backend 扩展必须在接口中声明操作的输入、输出、语义摘要和 codegen/runtime binding。
- Pandas adapter 可以把逻辑 key 映射到 index，Polars adapter 可以保留普通列；语言层不能要求 Polars 模拟 Pandas index alignment。

### 12.3 可读 codegen

生成代码应直接使用目标后端的公共 API，例如 `np.asarray`、`pd.read_csv`、`pl.col`，而不是生成通用反射调用或运行时字符串表达式。

下列行为必须在源码中显式出现：

- 时间 join/as-of 的方向和容忍区间。
- 缺失值、NaN、重复逻辑 key 和排序规则。
- pivot 前的唯一性要求。
- 数据从表到数组时的列顺序和 axes。
- 是否允许后端执行 lazy、parallel 或重排优化。

### 12.4 扩展语义契约

只提供普通 Python 函数类型不足以支持 schema、布局和因果检查。`.osri` 可以为扩展 primitive 声明稳定 intrinsic id，以及静态的类型和第 6.6 节定义的 summary transfer contract。typed HIR 使用通用 `IntrinsicCall` 引用这些契约，Rust 编译器不因此硬编码 `pandas`、`polars` 或 `numpy` 名称。

第一版只接受标准或显式信任扩展中的声明式 contract，不执行第三方 analyzer 代码。没有契约的 Python 调用仍可使用，但其 effect、时间和数据属性均为 unknown，不能在严格 causal/schema 区域中被当作已证明安全。

## 13. 副作用、时间因果与数据属性

### 13.1 三类独立摘要

typed HIR 中的每个表达式至少携带 `TypeId`、`EffectRow`、`TemporalSummary`、`DataProperties`、源码 span 和 macro expansion id。三类语义摘要不能合并：

1. `EffectRow` 回答求值是否观察或改变外部世界。
2. `TemporalSummary` 回答结果依赖哪个 event-time 范围的数据，以及这些数据最早何时可被当前决策观察。
3. `DataProperties` 回答 schema、axes、alignment、排序、唯一性和形状变化。

第一版 `EffectRow` 至少预留：

```text
pure = empty set
io/read
io/write
python/dynamic
hidden-state
nondeterministic
throw
```

`TemporalSummary` 由两个正交子项组成：

1. `EventTimeBounds` 分别记录 past/future bound。每侧边界可以是 `finite(n)`、受约束的 `symbolic(expr)`、`unbounded` 或 `unknown`；`past(unbounded)` 仍然可以是 causal，只有无法证明读取方向时才是 temporal unknown。例如运行期窗口若类型/refinement 已证明非负，可以得到 symbolic past bound 和 `future = 0`。
2. `AvailabilitySummary` 记录依赖值相对计算点最早可观察的时点，可以是受扩展定义顺序约束的 `known(expr)`、`symbolic(expr)` 或 `unknown`。例如扩展可以声明 `ingest-start < ingest-complete < next-cycle`，以及 payload 在 `ingest-complete` 可用；编译器核心只处理已声明的符号、偏移、偏序和约束，不内置外部调度系统。

组合表达式时，event-time bounds 取 dependency join/shift，availability 取所有依赖中最晚的 required point；窗口、lag、as-of 和高阶 callback 的 transfer 必须同时传播两者。当前事件读取只有在 `availability <= evaluation-point` 可证明时才是 causal。来自 `.osri` 的 availability 仍遵守第 8.8.1 节的 declared/verified 信任分层。

v0 必须表示静态 literal/symbolic availability、完成基础 join/shift/约束检查并在 `.osri` 传播；依赖外部调度、动态发布时间或运行时 session 才能比较的表达式保持 `unknown`。`DataProperties` 至少可以表达 schema/axes、alignment 方式、是否保持长度、是否 materialize/reshape，以及排序和唯一键要求。

编译器可以推断局部摘要；导出函数的三类摘要进入 `.osri`。未知 Python 调用至少具有 `python/dynamic`、temporal unknown 和 data-properties unknown，除非接口声明更精确的契约。

这些概念必须保持正交：pure 函数仍可能读取未来数据；读取已经发布的历史文件具有 `io/read` 但可以是 causal；显式传入并返回状态的 scan 可以是 pure，隐藏的进程全局状态才是 `hidden-state`。

### 13.2 因果区域

数据扩展可以定义 causal function。在该区域中：

- `lag`、backward as-of 和只读历史窗口可以被证明为因果。
- `lead`、centered window、负 lag 和未来索引产生正的 future bound。
- future bound 大于零、future direction unknown 或数据 availability unknown 默认是编译错误；past bound 无界本身不是未来数据错误。
- 当前事件的数据是否可用于当前计算由数据可用性契约决定，不能仅凭索引 `t` 判断。
- 缺失数据回填方向、最大陈旧时间和外部 session 必须参与分析。
- 显式 unsafe escape 必须记录原因和完整 provenance，不能把底层摘要伪装成 safe。

### 13.3 状态递推

依赖上一步结果的计算必须使用显式 scan/recurrence：

```clojure
(data/state-scan [index previous]
  :input values
  :initial initial
  :start start-index
  :emit-init true
  :next (step index previous))
```

对显式传入、长度为 `N` 的输入序列，`start` 必须位于 `[0, N]`，结果始终与完整输入等长：

- `index < start` 的输出为结果类型定义的 zero value。
- `emit-init = true` 且 `start < N` 时，`output[start] = initial`；第一次调用 step 是 `step(start + 1, initial)`。
- `emit-init = false` 且 `start < N` 时，第一次调用是 `step(start, initial)`，返回值写入 `output[start]`。
- 此后 `step(t, previous)` 的 `previous` 是上一输出期的结果，返回值写入同一个 `t` 槽位。
- `start = N` 时不调用 step，完整输出均为 zero value。

它不能伪装成可交换、可并行重排的普通向量表达式。`initial`、起始时刻、是否输出初值、冻结行为以及 step 的三类摘要都是状态机契约。通过参数显式传递的前序状态不自动产生 `hidden-state` effect，但 scan 仍然具有不可重排的顺序语义。

### 13.4 宏与语义摘要

宏本身只在 phase 1 构造语法，不得隐藏运行时语义。宏展开后的 I/O、隐藏状态、时间读取、alignment 和形状变化必须在 typed HIR 中仍然可见，并能追溯到宏调用位置。

## 14. 扩展与包管理

### 14.1 唯一包管理体系

- `pyproject.toml` 是项目和 Python 依赖 manifest。
- `uv.lock` 是唯一依赖锁文件。
- PyPI、私有 Python index、Git/path dependency、wheel 和缓存全部由 `uv` 处理。
- 运行时依赖放在标准 `[project]` 表的 `dependencies` 键。
- 根应用私有、且不会出现在公开 `.osri` 中的编译期宏/工具可以放在 PEP 735 dependency group。
- 公开 `.osri` 引用的类型、宏或 intrinsic 所属 distribution 必须放在标准 `[project]` 表的 `dependencies` 键，使 wheel 通过 `Requires-Dist` 向下游传递接口依赖。
- 生成 `.py` 会 import 的 distribution 必须是运行时依赖；编译期 group 不能代替运行时依赖声明。

```toml
[project]
dependencies = [
  "numpy>=2",
  "pandas>=2",
  "data-runtime>=0.1",
]

[dependency-groups]
osiris = [
  "osiris-data-ext>=0.1",
  "osiris-pandas>=0.1",
]
```

以上 dependency group 示例适用于最终应用的私有编译依赖。若一个发布库的公开函数签名、struct 字段或导出宏引用 `osiris-data-ext`，它必须移入该库 `[project]` 表的 `dependencies`。第一版不通过 vendoring `.osri` 隐藏传递依赖。

典型工作流：

```console
uv add numpy pandas
uv add --group osiris osiris-data-ext osiris-pandas
uv sync --locked --group osiris
uv run --group osiris osr check
uv run --group osiris osr compile
uv export --locked --no-default-groups --group osiris \
  --no-emit-project --output-file dist/build-constraints.txt
uv build --python 3.11 \
  --build-constraints dist/build-constraints.txt
```

### 14.2 静态扩展标记

包含 Osiris 编译接口或宏的 wheel 在 `<distribution>.dist-info/osiris.toml` 中放置唯一静态 marker，并将其写入 wheel `RECORD`。该文件只能由 `osiris_build` 生成，不能由作者手工维护。

```toml
schema = 2
compiler_abi = 1
language_abi = 2
language_version = "0.1"
standard_library_abi = 1
linkable_helper_format = 1
distribution = "osiris-data-ext"
version = "0.3.0"
python_target = "3.11"
dependencies = ["numpy>=2"]
records = "osiris_data_ext/data.records.json"
records_hash = "sha256:4444444444444444444444444444444444444444444444444444444444444444"

[[extension]]
id = "osiris_data_ext.data"
interface = "osiris_data_ext/data.osri"
interface_hash = "sha256:1111111111111111111111111111111111111111111111111111111111111111"
source = "osiris_data_ext/data.osr"
source_hash = "sha256:2222222222222222222222222222222222222222222222222222222222222222"
source_map = "osiris_data_ext/data.py.map"
source_map_hash = "sha256:3333333333333333333333333333333333333333333333333333333333333333"
```

- 编译器只读取标准 distribution metadata、`osiris.toml` 和声明的静态资源。
- 扩展发现阶段绝不 import 包中的 Python 模块。
- 未显式启用或导入的扩展不能自动执行宏。
- resource path 必须是 wheel 内相对路径，禁止绝对路径和 `..`。
- interface id 必须唯一，ABI 不兼容必须报错。
- distribution 中任一 interface 存在 public static records 时，marker 顶层必须声明唯一的 `records` 和 `records_hash`，该 sidecar 汇总本 distribution 所有 interfaces 自己拥有的 public records；没有 public records 时省略两项。多个 `[[extension]]` 不得各自声明或复制 sidecar。
- Python 依赖仍以标准 `Requires-Dist` 为权威来源；marker 的 `dependencies` 是构建器生成的有序校验副本，consumer 必须核对两者完全一致。
- 没有 marker 的普通 Python 包仍可作为运行时依赖，不视为 Osiris 扩展。

`osiris.toml` 只定位 `.osri` 和由它生成的纯数据 runtime record manifest，不声明 schema 脚本、Python entry point 或 validator 路径。编译器的 static schema、records 和 index claims 只从接口 section 读取，绝不把 JSON sidecar 反向当作编译输入。producer 在生成单个接口时先验证本地 record 与 index。

consumer 的顺序是规范性的：先只用 source/`.osri`/marker/lock 发现 hash-relevant graph，完成宏展开产生的本地 records，并按第 10.2 节计算或重验 body/SCC group hashes；再用 final group hash 组装并校验全部 `RecordOccurrenceId`；随后只从每个 distribution 的 owned public-record sections 重建该 distribution 的 canonical payload，re-export refs 不进入 payload，并核对 sidecar header、`record-set-hash`、marker `records_hash` 和最终字节；最后才沿 refs 去重并合并 public index claims，计算 `effective-record-index-hash`。sidecar 缺失、额外、字段不一致或任一 hash 错误都 fail closed。这样 runtime/phase-1 cycle 不会要求在 group hash 产生前重建 occurrence，同时也不能把“当前 wheel 内无冲突”当成全局结论。

合并时，index identity 与规范化 key 共同构成查找键；因此不同领域 schema 可以使用不同 namespaced index，同一组件的 canonical/legacy ID 则由同一个 schema index 约束。v0 的 Str index key 使用 Unicode NFC、区分大小写且不做 locale case folding/NFKC，manifest 同时保留 normalized key 与 raw spelling；通用 index validator 拒绝空字符串、首尾空白和控制字符，长度服从静态资源上限，runtime lookup 必须使用同一 NFC 规则。同一 occurrence identity 和 index claim 因 re-export 或菱形依赖重复出现时合并为一项；其他重复必须同时报告双方 distribution、module、owner、schema-provided projection role、origin 和 dependency path，并在写任何产物前失败。不存在 import-order、direct-dependency、first-wins 或隐式 override 规则。

合并后的 records 和 index entries 必须稳定排序并计算 consumer-only 的 `effective-record-index-hash`，该 hash 进入 check/build 结果、LSP workspace、诊断和增量缓存键，但不写回依赖 `.osri`。`osiris_build` 必须把当前 distribution 拥有的 public records 编码为 versioned tagged canonical JSON sidecar；Keyword、Symbol、Set 和非字符串 Map key 使用结构化 tag/entry 表示，不能依赖 Python `repr` 或有歧义的字符串前缀。sidecar header 携带 format version、源 interface semantic hashes、record identities 和 `record-set-hash`；`record-set-hash` 对稳定排序后的 canonical JSON records payload 计算，不包含 header 自身，marker 的 `records_hash` 则校验最终 sidecar 全部字节。backend 必须从 `.osri` 重新生成并逐项核对，禁止手工维护或反向覆盖接口；sidecar 不复制传递依赖 records。最终应用可以额外生成带 `effective-record-index-hash` 的 graph-merged 只读视图，但它不成为下游编译输入。该操作不能 import 扩展或调用 schema-specific generator。领域 runtime registry manifest 与 PyPI package registry 是两件事：前者只是扩展拥有的静态 ID/adapter 描述，Osiris 仍不提供包下载、解析或发布 registry。

v0 的 tagged datum encoding 以 RFC 8785 JCS 生成无 BOM、无额外空白的 UTF-8 字节，并在解码时拒绝重复 JSON member name 和非法 Unicode scalar。`none`、Bool、Str 直接使用 JSON null/bool/string；Int 使用 `{"$osiris":"int","value":"<canonical-decimal>"}`，Float 使用保存精确 IEEE-754 binary64 bits 的 16 位小写十六进制 `value`；Keyword/Symbol 分别使用 `keyword`/`symbol` tag 和 canonical spelling，已解析 Symbol 还携带 stable binding id。List、Vector、Set 使用带对应 `$osiris` tag 的 `items` 数组，Map 使用 `{"$osiris":"map","entries":[[key,value],...]}`。List/Vector 保序；Set items 与 Map entries 按子 datum 的 canonical JCS bytes 逐字节排序，重复 canonical Set item 或 Map key 必须报错。`record-set-hash` 和 `records_hash` 均使用带 `sha256:` 前缀的 SHA-256；前者对 `records` payload 的 canonical bytes 计算，后者对完整 sidecar bytes 计算。tagged encoding 的任何 breaking change必须提高 sidecar format version。

未来 analyzer capability 可以扩展 marker schema；第一版不加载 Python procedural analyzer。

### 14.3 PEP 517 构建闭环

发布 Osiris 库或扩展时，`uv build` 必须通过最小 `osiris_build` PEP 517 backend 在隔离的临时 staging 目录调用 `osr`。backend 必须先验证 effective dependency graph 的 static-record/index 合并，再生成任何文件；这项全图门禁不代表把依赖 records 嵌入当前 wheel。wheel 必须包含生成的 `.py`、公开 `.osri`、包级 `py.typed`、选择发布的 source map，以及存在 public records 时从本 distribution 接口生成的纯数据 record manifest，并全部写入 wheel `RECORD`；sdist 必须包含 `.osr` 源码、构建配置和从 sdist 重建 wheel 所需的静态资源。普通 Python backend 不会自动收集 `dist`，因此该 build integration 属于 v0 发布能力，不能只依赖手工预编译。

```toml
[build-system]
requires = ["osiris-lang==<osr-version>"]
build-backend = "osiris_build"
```

`osiris_build` 随 PyPI distribution `osiris-lang` 一起发布，不存在独立的 `osiris_build` distribution。`osr init --extension` 必须把 `requires` 精确固定到当前编译器版本，并生成与 distribution 名对应的合法 Osiris/Python 模块目录；例如 `acme-osiris` 生成 `src/acme_osiris/core.osr` 和模块 `acme_osiris.core`。backend 必须实现 `get_requires_for_build_wheel`：读取标准 `[project].dependencies`，在已校验为最新的 `uv.lock` 中找到对应锁定项，并返回精确的 PEP 508 requirements。该 hook 只投影锁文件，不自行解析版本；依赖缺失、范围不匹配或 lock 过期必须失败。PEP 517 frontend 随后把这些接口/宏依赖安装进隔离 build environment；backend 不能假设根环境已经 `uv sync`。

`[build-system].requires` 应精确固定包含 backend 和 ABI 匹配 `osr` 编译器的 `osiris-lang`。规范的 `uv` 工作流还必须把同一 `uv.lock` 导出的 requirements 文件传给 `uv build --build-constraints`，约束 backend 自身的隔离安装，并以 `uv build --python <target>` 选择与 `osiris.jsonc` 的 `targetPython` 相同 major/minor 的 build interpreter。backend 在开始编译时再次核对实际 `sys.version_info`、目标 Python、lock fork、已安装 compiler/extension 版本和 lock hash，任一不符都必须失败，不能从同一 lock 因构建解释器不同而选择出不同接口依赖。

sdist 必须携带 `uv.lock`、生成的 build constraints 和它们的 hash；从 sdist 构建 wheel 时，hook 使用这些锁定项。这样“相同源码、lock 和编译选项产生相同产物”的承诺适用于规范的 locked build；没有 build constraints 且无法验证隔离环境的构建不标记为 reproducible。

`osiris_build` 只负责标准构建钩子、依赖需求桥接、编译 staging 和 wheel 内容，不自行解析或安装依赖；依赖解析仍由 `uv`/PEP 517 frontend 完成。`osiris` 编译器自身可以继续使用 maturin 构建，不要求改用该 backend。

## 15. 编译产物与源码映射

### 15.1 产物

```text
foo.osr
  -> foo.py
  -> foo.osri
  -> foo.py.map

public static records in project
  -> <normalized-distribution>.records.json
```

- `.py`：可运行、可发布的 Python 主产物。
- `.osri`：供 Osiris 编译器和 LSP 使用的编译接口。
- `.py.map`：generated line/column 到 `.osr` span 的映射，并保留宏 origin chain。
- `.records.json`：仅在项目含 public static records 时生成的 distribution 级、versioned tagged canonical JSON runtime manifest；它是 `.osri` 的派生产物。

产物默认写入 `dist/`，也可以由 `outDir` 修改，不污染源码目录。`osr compile` 在同一次原子 staging 中聚合当前 distribution 的 public records 并生成 sidecar。

### 15.2 确定性与安全

- 产物必须稳定排序、稳定换行并避免写入机器绝对路径。
- source hash、接口 hash、`trust-policy-hash`、`effective-record-index-hash`、language ABI 和 target Python 必须进入构建元数据与相关缓存键。
- 写入应使用临时文件加原子替换，失败不能留下部分产物。
- 编译器不能因生成可读代码而改变表达式求值次数或顺序。

### 15.3 Python traceback

第一版保证 source map 可用于工具查询。自动改写 Python traceback 可以后续实现，不应阻塞编译器核心。

## 16. Compiler tooling CLI 与 LSP

Rust compiler database 拥有全部 tooling query。CLI 是人和 Agent 都能直接使用的完整
入口；LSP 只是同一能力的 IDE adapter，不能拥有 CLI 无法访问的 semantic fact 或 edit。

### 16.1 完整能力

- 容错解析和未完成代码诊断。
- 名称、字段、DataFrame schema 和函数参数补全。
- hover 显示 canonical name、本地化名称、参数/字段 aliases、推断类型、effect、event-time bound、availability、数据属性、contract trust 状态、文档和宏来源。
- go-to-definition、find-references 和 rename。
- `defstruct` 构造器和字段错误诊断。
- 宏签名帮助、展开预览和 expansion origin。
- 使用与 `osr fmt` 完全相同实现的 document formatting；CLI 没有对应 range contract 前，
  LSP 不声明 range formatting。
- 跨包读取 `.osri`，不 import Python 包。
- 因果错误定位到具体时间读取和调用点。
- 从 `.osr` 跳转到生成 `.py`，以及根据 `.py.map` 反向定位。

v0 的 schema 补全只覆盖显式 `defstruct`/`Frame` schema 和接口声明；因果诊断传播 `.osri` 已声明的 event-time + 静态 availability contract，按本地 trust context 标记 declared/verified，并拒绝 causal 区域中的 future/availability unknown。动态 Pandas schema 推断，以及依赖 calendar、外部发布时间或运行时 session 的 availability analyzer 属于后续能力；LSP 不得在缺少证明时显示为安全。

### 16.2 增量模型

解析树必须保留 trivia、稳定 node identity 和错误节点。宏展开、名称解析、类型检查和接口生成应按 module/query 缓存，单文件编辑不应重建整个依赖图。

`Unknown` 和 `Error` 类型只用于编辑恢复；LSP 不应因为一个未闭合 list 而丢失整个文件的字段补全。

### 16.3 多语言语义视图与 Agent 查询

推荐把“中文编程”和“中文阅读”分成两种同等受支持的工作流：

1. 源码直接使用中文 canonical identifier 或 alias，编译器按正常 token 处理。
2. 源码保留稳定 canonical name，LSP 根据 `display-locale = "zh-CN"` 显示 preferred 中文名、文档和参数名称。

locale 只影响 completion 排序、hover、inlay hint、outline 和渲染，绝不改动 parser/name resolution。LSP 不能在编辑 buffer 中无痕替换 token，因为那会使 diff、诊断 span、格式化和协作结果不可信；选择本地化 completion 时，应插入真实可解析 alias，并按需添加 import/alias 声明。

除标准 LSP 能力外，server 应提供只读的 `Osiris Semantic View` 查询。编辑器插件可以把 expanded typed HIR 渲染为中文数据操作图或结构化伪代码，每个节点至少携带：

- binding id、canonical name、当前 locale preferred name 和实际 source spelling。
- 类型、参数角色、effect、temporal summary、data properties 和 availability。
- schema/axis 变化、materialize/reshape 边界、macro origin 和源 span。
- authored metadata、typed static records、declared facts、当前 trust context 下的 verified facts 和各自 provenance。

视图必须允许在 folded macro call 与 expanded primitive 之间切换，并能点击回到真实 `.osr` 源码。中文 label 可以帮助阅读，但不能覆盖 unsafe、future dependency、unknown schema 或 materialization 等诊断。

Agent 不应直接依赖 `.osri` 的物理 S-expression 格式。compiler database、LSP 自定义查询和 `osr lsc <operation> --format json` 应返回版本化的同一结构化 symbol/semantic model；Agent 以 `binding id + document version + source span` 定位修改，不按中文字符串全局替换。返回值必须明确区分不可信 authored metadata、schema-checked static records、dependency-declared facts 和当前本地信任配置下的 compiler-verified facts。

每个 compiler-owned LSP 能力都必须有 CLI 等价入口，包括 diagnostics、hover、completion、
signature、definition、references、rename edit preview、expand、syntax、semantic 和 symbol。
LSP session lifecycle 与增量同步是 transport behavior，不需要伪造成一次性命令。

## 17. CLI 边界

建议第一版命令：

```console
osr check [PATH]
osr compile [PATH] [--out-dir DIR] [--emit py,osri,map,records]
osr run FILE -- [ARGS]
osr fmt [PATH...] [--check]
osr fmt -
osr expand FILE [--once]
osr lsc diagnostics [PATH]
osr lsc hover API
osr lsc hover --at URI:LINE:COLUMN
osr lsc completion --at URI:LINE:COLUMN
osr lsc signature --at URI:LINE:COLUMN
osr lsc definition --at URI:LINE:COLUMN
osr lsc references --at URI:LINE:COLUMN
osr lsc rename --at URI:LINE:COLUMN --to NAME
osr lsc expand FILE
osr lsc syntax FILE
osr lsc semantic FILE
osr lsc symbol NAME
osr syntax
osr syntax --format markdown|json
osr doc '<GraphQL document>'
osr doc -
osr lsp
```

- `check` 解析、展开、解析名称，校验 static schema/records 和 effective dependency graph indexes，并检查类型、副作用、时间因果和数据契约，不生成主产物。
- `compile` 完成与 `check` 相同的全部门禁后生成 Python、接口、source map，以及存在 public records 时的 distribution runtime manifest；任何 record/index 冲突都必须在原子写入前失败。`--emit` 必须满足产物依赖闭包：若生成的 Python 含 runtime-record adapter，显式请求 `py` 却省略 `records` 必须报错，不能留下不可运行的 `.py`。显式请求 `--emit records` 而项目没有 public records 时生成规范化空 manifest，默认模式则省略它。
- `run` 使用当前已准备好的 Python 环境，在临时 build layout 中生成与 `compile` 相同的 Python 和 records sidecar 后运行，不修改依赖或锁文件；它从已验证 `uv.lock`/effective graph 构造精确的 `(distribution, version, interface-member-id, semantic-interface-hash) -> (records path, records hash)` resolver，当前项目指向 staging sidecar，依赖指向各自已校验 wheel marker。runtime loader 只使用该 resolver，不能回退扫描源码或枚举任意已安装 manifest。
- `fmt` 是全语言唯一的 canonical formatter：默认安全地原地更新，`--check` 只检查，`-` 使用 stdin/stdout。项目不能配置另一套 style；LSP formatting 必须产生相同字节。
- `expand` 输出可读宏展开结果。
- `lsc`（Language Server Console）从 source/compiler database 或 `.osri` 提供全部 LSP 等价查询；默认输出 human-readable text 并选择 authored `:default`，`--locale <bcp47>` 可按 RFC 4647 选择中文、日文或其他 tagged metadata，JSON 形式供 Agent 和其他工具稳定消费，不 import/执行目标 Python package。`hover` 同时接受 API name 和 `--at URI:LINE:COLUMN`。
- `syntax` 从内嵌 read-only libSQL snapshot 直接输出 stable ID `language/syntax` 的完整英文手册，不需要网络。中文翻译只保留在仓库用于审核，不进入 binary。
- `doc` 在进程内对同一 content-addressed libSQL/FTS5 snapshot 执行标准 GraphQL document，并原样输出标准 GraphQL JSON。Embedded corpus 只包含完整 authored English Markdown；源码内 `:doc`、alias 和 signature 仍由本地 `lsc`/LSP 查询。
- `lsp` 启动复用编译器前端的语言服务器。

Python package/wheel 构建继续由 `uv build` 驱动，并由 `osiris_build` PEP 517 backend 完成编译 staging；`osr` 不提供包解析、安装、锁定或发布命令。

v0 的 `check` 对显式类型、接口 contract 和基础摘要传播负责；需要动态 schema、外部调度或数据发布时间才能判断的规则必须报告 unknown，不能假装已经完成完整领域分析。


## 18. 第一版实施范围

### 18.1 v0 必须完成

1. 容错 lexer/parser、CST、span 和稳定诊断。
2. 模块、import/export、Unicode 名称、binding/parameter/field alias 解析和 Python 名称映射。
3. 基础表达式、函数、调用、异常和可读 Python codegen。
4. 基础类型、局部推断、函数签名、泛型、`defstruct` 和类型所有者发布的封闭静态 operator capabilities。
5. 卫生 `defmacro`、syntax quote、Clojure 1.12 metadata reader/API、宏 `&form`、phase-1 求值器和 expansion trace。
6. Clojure 风格 core 控制流和数据流：完整 threading 宏组、truthy 条件宏、`nil?`/`some?`、`if-not`/`when-not`、`if-let`/`when-let`、`if-some`/`when-some`、`when-first`、`case`/`condp`、`doseq`、`dotimes`、`while`、`assert`、`throw`/`comment`/`time`、`try/catch/finally` 边界、`with-open` 资源清理、lazy `for` / eager `forv` 多 binding/`:let`/`:when`/`:while`、`loop/recur`（O(1) 栈）、`trampoline`、`lazy-seq`、`delay`/`force`/`deref`/`realized?`、`future`/`future-call`/`promise`/`deliver`/`locking`、`map`/`mapv`/`mapcat`/`filter`/`reduce`/`fold`，以及 distribution-private `__osiris_runtime__` linking。
7. `defstatic-schema`、不可执行 `static-record`、declaration-macro lowering，以及 `.osri` 中 public schema/record 的生成与读取。
8. `.py` 类型标注、`.py.map` 和确定性构建。
9. public names、authored/records/declared/verified 分层、可验证 contract DSL 和宏 phase-1 IR 打包。
10. 三类语义摘要的表示、event-time + 静态 availability 基础传播、信任提升和 `.osri` 声明式 contract。
11. 静态扩展发现、effective dependency graph 和通用 unique-index 合并，但不执行 Python plugin。
12. 最小 `osiris_build` PEP 517 backend，确保 `.py`、`.osri`、`py.typed` 和 public records 的 canonical JSON runtime manifest 进入 wheel。
13. CLI/LSP/Agent 最小闭环：诊断、双语补全/hover、跳转、宏展开、中文 semantic view，且每项 compiler-owned LSP 能力都有结构化 `lsc` CLI 等价入口。
14. Canonical formatter、`osr fmt`/`--check`/stdin 模式，以及与 LSP formatting 的逐字节一致性。

### 18.2 后续能力

- 完整 row/schema inference 和命名 axes 检查。
- 开放式 protocol、用户定义/运行时 overload 和更完整的 Python `.pyi` 导入。
- 完整 causal analyzer、动态窗口，以及依赖外部调度、发布时间或 session 的 availability analyzer；静态 availability contract 与基础传播属于 v0。
- 自动 traceback remap 和调试器集成。
- 可隔离的 typed analyzer 扩展协议。
- 更完整的可定制渲染器和多编辑器 Semantic View UI；server 的结构化查询协议必须先稳定。

### 18.3 明确延后

- reader macro 和自定义 parser。
- 任意 Python/Rust procedural compiler plugin。
- struct 继承和复杂对象系统。
- JIT、原生机器码和自有 runtime。
- 自动代数优化、隐式并行和浮点重排。
- 自有包管理器或包下载/发布 registry；扩展自己的 runtime static manifest 不属于包管理。
- 给任意 Python/NumPy/Pandas 值自动附加运行时 metadata 的 wrapper 或进程级 side table。

## 19. 验收标准

语言核心进入实现阶段后，至少使用以下纵向样例验收：

1. `defstruct` 参数、默认值、不变量、泛型和 Python dataclass 输出。
2. `->`、`->>`、`cond->`、自定义宏、卫生和展开诊断。
3. Clojure 1.12 metadata reader 矩阵：Map/Keyword/Symbol/Str/Vector 简写、链式右到左合并、`meta`/`with-meta`/`vary-meta`、宏 `&form` 和规范化打印再读取。
4. 一个带导出接口的跨模块项目，生成并消费 `.osri`。
5. 一个 NumPy 数组流水线，验证 dtype/axes 接口和运算顺序。
6. 一个 Pandas 或 Polars 表流水线，验证 schema、缺失值和可读 Python。
7. 一个三步 `state-scan`，验证状态反馈和 future-read 诊断。
8. 一个带时间窗口的通用数据流水线，验证 availability、中文 Semantic View 和可读 Python。
9. 两个直接/传递扩展 wheel 的 static records，覆盖 canonical/legacy key 冲突、同 owner 不同 schema、schema id/version body-hash 冲突、同 record 菱形去重和遍历顺序无关的确定性诊断。

每个样例必须同时检查：Osiris 诊断、宏展开、canonical/alias binding identity、authored/records/declared/verified 分层、`.osri`、生成 Python 的可读性、Python 类型标注和运行结果。static-record 测试还必须验证 private record 不进入跨包 index/sidecar、同 provider 的菱形路径只去重一次、不同 interface hash/version 不去重，以及本地和 dependency wheel 的 JSON sidecar 都可从 `.osri` 确定性重建，字段漂移、缺失/额外 record 或任一 hash 不符会失败。含 public adapter 时 `compile --emit py` 缺少 `records` 必须失败，`run` resolver 也必须拒绝 version/member/interface hash 不匹配；registry ID 冲突必须 fail closed，record 不能自授 availability/verified fact。
