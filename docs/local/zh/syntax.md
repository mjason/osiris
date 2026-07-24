---
document-id: language/syntax
title: Osiris 语法
language: zh-CN
revision: 3
source: ../../syntax.md
source-revision: 3
translation-status: Current
---

# Osiris 语法

本文件是 Osiris 简明语法手册的中文审阅翻译。英文原文是版本化发布源，原生
`osr` 会以稳定 ID `language/syntax` 内嵌完整英文文档；`osr syntax` 无需联网即可
输出英文原文。本中文文件保留在仓库中供审阅，不进入二进制文档数据库。

## 源文件

Osiris 源文件使用 `.osr` 扩展名和 UTF-8。分号开始一条持续到行尾的注释。字符串
外的逗号等同于空白，所以 `[1, 2]` 和 `[1 2]` 含义相同。

Reader 只读取数据。List 随后才会被解释成核心形式、宏调用或函数调用。包不能增加
reader 语法；扩展语言应使用普通函数和卫生宏。

## 数据形式

```clojure
none                         ; 空值
true false                   ; 布尔值
42 -7 3.14                   ; 数值
"text\nnext"                  ; 字符串
:ready                       ; keyword
(f x y)                      ; list：调用、宏或核心形式
[x y z]                      ; vector
{:name "sample" :value 0.2}  ; map
#{:new :ready}               ; set
```

集合可以直接嵌套。Keyword 在集合中是数据，在调用位置可以表示关键字参数：

```clojure
(configure :window 20 :strict true)
```

Phase 1 reader 形式包括：

```clojure
'form       ; quote
`template   ; 卫生 syntax quote
~value      ; syntax quote 内的 unquote
~@values    ; syntax quote 内的 unquote-splicing
```

## 名称

名称可以使用 Unicode。名称身份使用 Unicode NFC，诊断仍保留作者拼写。常见形式有：

```clojure
rolling-mean          ; Lisp 风格名称
ready?                ; 谓词命名惯例
series/rolling-mean   ; 限定 Osiris 名称
row.value             ; 静态字段或 Python 属性
时序均值               ; Unicode 源码名称
```

本地化名称是同一个 canonical binding 的别名，不是独立定义。Locale 不会改变名称解析。

## 模块与导入

模块通常以 canonical module name、显式 import 和 export 开始：

```clojure
(module analytics.pipeline)

(import analytics.transforms :as transforms :refer [sum-values])
(import-for-syntax analytics.macros :as macros :refer [unless])
(py/import math :as math)

(export [normalize summarize])
(alias 汇总 summarize)
```

- `import` 读取另一个 Osiris 模块的 `.osri` 接口。
- `import-for-syntax` 导入宏和 phase-1 helper。
- `py/import` 生成 Python runtime import；编译时不会执行 Python。
- `export` 定义模块的公开接口。
- `alias` 为已有 binding 增加另一个可解析拼写。

项目配置把源码路径映射成模块名。源码中的 `module` 声明必须与该映射一致。

## 定义与函数

```clojure
(def answer 42)

(defn ^Int add
  [^Int left ^Int right]
  (+ left right))

(defn ^Int clamp-low
  [^Int value [^Int minimum = 0]]
  (if (< value minimum) minimum value))

(def increment
  (fn [value] (+ value 1)))
```

`def` 绑定值，`defn` 定义具名函数，`fn` 创建匿名函数。参数使用 vector。在参数末尾
使用 `& rest` 表示变参。默认参数写成 `[name = expression]`，类型 metadata 仍附着在
`name` 上，如上例所示。

所有 form 都是表达式。函数和 `do` 返回最后一个表达式。求值顺序为从左到右；宏展开
和代码生成不得重复求值带 effect 的表达式。

## Rich Metadata

`^` reader prefix 把不可变、不可执行的 Rich Metadata 附着到紧随其后的受支持 syntax
node：

```clojure
^:deprecated
^{:doc {:default "Increment every integer."
        "zh-CN" "将每个整数加一。"}
  :since "0.1"
  :osiris/names
  {"zh-CN" {:preferred 全部加一
             :aliases [逐项加一]}}
  :agent/tags [:data :transform]}
(defn ^{:type (Vector Int)} increment-all
  [^{:type (Vector Int)} values]
  (mapv (fn [^Int value] (+ value 1)) values))
```

支持与 Clojure 一致的简写：

```clojure
^:flag       ; {:flag true}
^TypeTag     ; {:tag TypeTag}
^"tag"       ; {:tag "tag"}
^[A B _]     ; {:param-tags [A B _]}
```

在 `:doc` 中，`:default` 是作者选择的无语言标签回退内容。可复用包推荐用英文编写，
但任何语言都合法。其他 key 必须是标准 BCP 47 language tag，例如 `"en"`、
`"zh-CN"` 或 `"ja"`。工具按 RFC 4647 lookup，最后回退到 `:default`，且不能伪称
这个无标签回退属于某个 locale。

Phase 1 可以用 `meta`、`with-meta` 和 `vary-meta` 读取并以不可变方式更新 metadata。
Metadata 不能冒充编译器验证过的类型、effect、temporal 或 data fact。

## 类型

类型通过 Rich Metadata 附着到声明和 binding。`^Int` 是紧凑 tag 写法；参数化类型
使用 `^{:type ...}`。

```clojure
^Int
^{:type (Vector Int)}
^{:type (Map Str Float)}
^{:type (Option Str)}
^{:type (Union Int Float)}
^{:type (Fn [Int Int] -> Int)}
```

核心类型名包括 `Bool`、`Int`、`Float`、`Str`、`Bytes`、`None`、`Any` 和
`Never`。核心类型构造器包括 `List`、`Vector`、`Map`、`Set`、`Option`、`Union`、
`Tuple` 和 `Fn`。名义类型和数据包类型使用相同 type form，并来自普通接口；Reader
不会硬编码 DataFrame、Series、NumPy、Pandas 或 Polars 行为。

省略标注表示请求局部推断，不是隐式 `Any`。Exported signature 和 Python host boundary
必须完整。真正的动态边界应显式写 `Any`。

## 结构体

`defstruct` 创建包含有序 typed field、可选 default 和 constructor check 的名义不可变
结构：

```clojure
(defstruct Threshold
  "A closed threshold range."
  [minimum Float]
  [maximum Float]
  [enabled Bool = true]

  (check (<= minimum maximum)
         "minimum must not exceed maximum"))

(def threshold
  (Threshold :minimum 0.0 :maximum 1.0))

threshold.maximum
```

泛型 struct 把 type parameter 与名称写在一起：

```clojure
(defstruct (Pair A B)
  [left A]
  [right B])
```

`[field Type = default]` 是 `defstruct` field 专用形态。即使字段完全相同，不同 struct
声明仍是不同名义类型。

## 核心表达式

编译器拥有的小型表达式 kernel 包含 `fn`、`let`、`if`、`do`、`try` 和 `raise`：

```clojure
(let [^Int x 10
      ^Int y (+ x 2)]
  (if (> y 10)
    (do (record y) y)
    0))
```

核心 `if` 要求 `Bool`。`when`、`cond`、`if-let`、`and` 等 `osiris.core` 条件宏使用
Clojure truthiness：只有 `none` 和 `false` 为假；零、空字符串和空集合仍为真。

Vector 和 map 可以在 binding 位置解构：

```clojure
(let [[first-value second-value] values
      {:keys [name count]} options]
  (combine name count first-value second-value))
```

## 卫生宏

使用 `defmacro` 定义宏。宏接收 syntax，并且必须返回可以展开成普通 Osiris form 的
syntax：

```clojure
(defmacro unless [condition & body]
  `(if (not ~condition)
     (do ~@body)
     none))

(defmacro twice [expression]
  `(let [value# ~expression]
     (+ value# value#)))
```

`value#` 这样的自动 gensym 在每次展开时创建全新 binding。模板名称在宏定义处解析，
unquote 插入的 syntax 保留调用方 context。宏运行在确定、受限的 phase 1，不能 import
Python、访问网络、读取运行时值或绕过普通类型和语义检查。

使用 `defn-for-syntax` 定义编译期 helper，使用 `import-for-syntax` 导入编译期依赖。
使用 `osr expand <path>` 检查展开结果。

## Threading 与控制流宏

没有显式 core import 时，public `osiris.core` surface 会被自动 refer，threading 和
控制流 form 可以直接使用：

```clojure
(->> events
     (map event-value)
     (reduce add 0))
```

显式 core import 会完全替代该默认规则。可以用它选择更小的 surface，或排除、
重命名冲突的 spelling：

```clojure
(import osiris.core
  :refer :all
  :exclude [map]
  :rename {reduce fold-left})
```

省略的 `:exclude`/`:rename` 等同于空集合。Local declaration 只遮蔽隐式 core
spelling，`osiris.core/map` 仍可通过 qualified name 访问；显式 import 遇到 local
collision 则报错。Core 提供 Clojure 风格 threading macro：

```clojure
(-> value
    (clean)
    (normalize options))

(->> events
     (map event-value)
     (reduce add 0))
```

`->` 把前一步结果插到第一个参数，`->>` 插到最后一个参数。`cond->`、`cond->>`、
`some->`、`some->>`、`as->` 和 `doto` 分别提供条件、可空、具名和副作用式数据流。
初始表达式只求值一次。

`for` 支持多个 collection binding 和穿插的子句，并返回 memoized LazySeq：

```clojure
(for [left left-values
      right right-values
      :let [sum (+ left right)]
      :when (> sum 0)
      :while (< sum 100)]
  sum)
```

- Binding pair 按从左到右的顺序引入嵌套迭代。
- `:let` 为当前组合引入局部 binding。
- `:when` 为假时跳过当前结果。
- `:while` 为假时停止词法上最近的 collection。

`forv` 接受相同形式并 eager 返回 Vector。`doseq` 对 effect 使用相同 binding 子句，
并返回 `none`。

## 递归与序列

显式常量栈状态使用 `loop` 和尾位置 `recur`：

```clojure
(loop [index 0
       total 0]
  (if (= index 100)
    total
    (recur (+ index 1) (+ total index))))
```

函数中的尾位置 `recur` 在没有更近 `loop` 时指向当前函数。编译器检查 arity、state
type 和尾位置。两种形式都降低为常量栈 Python 控制流。相互递归使用 `trampoline`。

常用序列操作包括 `map`、`mapv`、`mapcat`、`filter`、`reduce`、`fold`、`take`、
`drop`、`partition`、`some` 和 `every?`。`reduce` 接受 `(reduce f coll)` 或
`(reduce f initial coll)`；`fold` 必须提供 initial value。使用 `reduced` 提前终止归约；
使用 `lazy-seq` 显式延迟并记忆大型或无限序列的生成。

## 异常

```clojure
(try
  (parse value)
  (catch ValueError error
    (recover error))
  (catch Exception error
    (report error))
  (finally
    (cleanup)))

(raise error)
```

`try` 接受零个或多个 `catch`，最后可以有一个 `finally`。Prelude 的 `throw` 是
`raise` 的 Clojure 风格拼写。

## Python 互操作

使用显式边界，确保编译过程不会 import 或执行 Python：

```clojure
(py/import host.runtime :as host)

(extern python "host.runtime"
  (defn ^Any register
    [^{:type (Map Str (Vector Str))} extra-data]))

(py/decorate publish
  (register :extra-data {"columns" ["value" "year"]}))

(defn ^Any publish
  [^Any context [^Str field = "value"]]
  (context.emit field))
```

`extern` 声明 typed Python ABI。`py/decorate` 把 Python decorator 附着到生成的声明；
decorator 是 runtime 行为，不是 Rich Metadata。已知 keyword argument 会经过检查，并
使用 canonical Python name 生成。

Generated Python 对 Osiris 保持 standalone。Reachable standard operation 需要可复用
support 时，linker 在 owning package 的 reserved `__osiris_runtime__` 下生成 ordinary
Python。Osiris source 禁止声明该 package 或直接 import 其中的 private name。

## 完整最小模块

```clojure
(module sample.stats)

(export [Summary positive-sums summarize])

(defstruct Summary
  [count Int]
  [total Int])

^{:doc {:default "Return positive Cartesian sums."
        "zh-CN" "返回笛卡尔组合中的正数和。"}
  :osiris/names
  {"zh-CN" {:preferred 正数组合}}}
(defn ^{:type (Vector Int)} positive-sums
  [^{:type (Vector Int)} left-values
   ^{:type (Vector Int)} right-values]
  (forv [left left-values
        right right-values
        :let [sum (+ left right)]
        :when (> sum 0)]
    sum))

^{:doc "Summarize a vector of integers."}
(defn ^Summary summarize
  [^{:type (Vector Int)} values]
  (Summary
    :count (count values)
    :total (reduce + 0 values)))
```

写完或修改源码后运行 `osr fmt` 和 `osr check`。使用 `osr lsc` 可以从 CLI 访问与
IDE 通过 LSP 获得的相同诊断、hover、completion、signature、navigation、symbol 和
semantic fact。
