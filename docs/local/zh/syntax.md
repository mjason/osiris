---
document-id: language/syntax
title: Osiris 语法
language: zh-CN
revision: 11
source: ../../syntax.md
source-revision: 11
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

## 嵌入语言块

具名嵌入块可以直接保存外部语言或多行文本，不需要字符串转义：

```clojure
~json<settings>
{"theme": "dark", "compact": true}
</settings>

(export [settings])
```

Language tag 使用小写；closing label 必须与 opening label 完全一致。对于 `json`、
`markdown`、`sql`、`html`、`css`、`javascript`、`typescript`、`toml`、`yaml`
等通用语言，label 声明一个 module-private `Str` binding。它只能通过普通 `export`
和 `import` 跨模块。格式正确的未知 language tag 也是合法 raw text，不会加载
compiler extension。

Body 是 raw content，其中的引号、分号、delimiter 和 `~` 都没有 Osiris 含义。多行
形式会去掉 opening/closing 的结构性换行，并从每个非空行移除 closing tag 的公共
缩进。如果正文必须包含完全相同的 closing text，应改用另一个 label。

`python` 是特例：它的 label 是 private provider handle，不是 `Str`，也不能 export。
它只能由 symbolic local `extern` 使用：

```clojure
~python<text-backend>
def normalize(value: str) -> str:
    return value.strip().casefold()
</text-backend>

(extern python text-backend
  (defn ^Str normalize [^Str value]))
```

`extern python text-backend` 会把本地模块链接到 distribution-private
`__osiris_runtime__`。相反，`extern python "package.module"` 表示由 uv/PyPI 提供的
外部 Python module，compiler 不会复制它。嵌入模块之间可以使用静态 import；compiler
只重定位 reachable private module graph，编译时绝不执行它。

## 名称

名称可以使用 Unicode。名称身份使用 Unicode NFC，诊断仍保留作者拼写。常见形式有：

```clojure
format-message        ; Lisp 风格名称
ready?                ; 谓词命名惯例
text/format-message   ; 限定 Osiris 名称
row.value             ; 静态字段或 Python 属性
格式化文本             ; Unicode 源码名称
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
(alias 旧汇总 summarize)
```

- `import` 读取另一个 Osiris 模块的 `.osri` 接口。
- `import-for-syntax` 导入宏和 phase-1 helper。
- `py/import` 生成 Python runtime import；编译时不会执行 Python。
- `export` 定义模块的公开接口。
- `alias` 为已有 binding 保留迁移拼写；引用仍然有效，但会收到不阻断编译的替换提示。
  推荐的本地化拼写应使用带 `:preferred` 的 `:osiris/names`。

项目配置把源码路径映射成模块名。源码中的 `module` 声明必须与该映射一致。

## 公开一个声明

不写明就不公开，而写明的方式有两种。上面的 `(export [...])` 清单在一处集中列出
名字；逐项标记则随声明本身走：

```clojure
^:export (def ^Int limit 20)

^{:doc "前进一步。" :export true}
(defn ^Int step [^Int value] (+ value 1))
```

`^:export` 读作 `^{:export true}`；写在形式前的元数据会与写在名字上的元数据合并，
因此 `^:export (def x 1)` 与 `(def ^:export x 1)` 是同一件事。只有 `true` 生效，其他
值仍是普通 authored metadata。

公开面是二者的并集，两种方式都写的名字只公开一次。两种都没写的声明就是模块私有——
没有单独的私有形式，也没有任何元数据键能断言私有。

清单能用的位置标记都能用，包括嵌入数据块和 `extern` 内嵌的声明。给没有公开名的东西
加标记——裸表达式、import、嵌入 Python 的 provider handle——是错误（`OSR-N0016`），
而不是一个悄无声息不起作用的标记。

两者之中只有标记是宏能产出的。`export` 是固定在展开之前的作者边界形式，宏不得生成；
而标记只是普通声明上的普通元数据，因此声明宏可以直接公开它生成的东西，不必要求每个
调用点重抄一遍名字：

```clojure
(defmacro define-factor [label]
  `(do ^{:doc "因子名。" :export true} (def ^Str factor-name ~label)
       ^{:doc "计算。" :export true} (defn ^Int calculate [^Int value] value)))
```

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
~osiris<increment-all-example>
(increment-all [1 2 3])
;; => [2 3 4]
</increment-all-example>

^:deprecated
^{:doc {:default "Increment every integer."
        "zh-CN" "将每个整数加一。"}
  :since "0.1"
  :osiris/names
  {"zh-CN" {:preferred 全部加一
             :aliases [逐项加一]}}
  :examples [increment-all-example]}
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

文档示例使用具名 `~osiris` block；`:examples` 是由同模块、未加引号的 block name
组成的 vector：

```clojure
~osiris<reduce-example>
(reduce + 0 [1 2 3 4])
;; => 10
</reduce-example>

^{:examples [reduce-example]}
(defn reduce ...)
```

每个 block 是一段完整、符合 canonical formatter 的源码。只被 metadata 引用的 block
不会作为 runtime string 释放。LSP、LSC、package interface 和面向 Agent 的 JSON
都会保留 resolved content、language、label、source span 和 content hash。

长文档也可以引用同模块 `~markdown` block；短文档仍可直接用 literal，每个 locale
可以独立选择：

```clojure
~markdown<normalize-doc>
Normalize text for stable comparison.
</normalize-doc>

^{:doc {:default normalize-doc
        "zh-CN" "规范化文本以便稳定比较。"}}
(defn normalize ...)
```

`:osiris/names` 也可以直接附着到函数参数。这样声明的本地化 keyword spelling 属于
该函数的静态签名：

```clojure
(extern python "text_runtime.format"
  ^{:doc {:default "Format a message for display."
          "zh-CN" "格式化用于显示的文本。"}
    :osiris/names
    {"zh-CN" {:preferred 格式化文本
               :aliases [渲染文本]}}}
  (defn ^Str format-message
    [^Str template
     ^{:type Str
       :osiris/names {"zh-CN" {:preferred 名称
                                :aliases [显示名称]}}}
     name
     [^{:type Bool
        :osiris/names {"zh-CN" {:preferred 转为大写
                                 :aliases [大写 使用大写]}}}
      uppercase = false]]))
```

调用目标具有静态签名时，可以使用本地化参数名作为 keyword argument：

```clojure
(format-message "Hello, {name}!" :显示名称 "Osiris" :大写 true)
(格式化文本 "Hello, {name}!" :名称 "Osiris")
```

参数的 locale entry 与声明的结构完全相同：`:preferred` 是一个 Symbol，可选的
`:aliases` 是 Symbol vector。正常使用的本地化拼写应该写在 `:preferred`；
`:aliases` 只保留源码迁移期间仍需兼容的旧拼写，不表示多个同等推荐的翻译。工具和
编译器看到源码使用 alias 时，应该建议改成当前 locale 的 preferred spelling；没有
preferred 时回退到 canonical name。

参数别名只属于声明它的函数签名，不是全局 keyword 替换。编译器先把 preferred name
和 alias 解析到 canonical parameter identity，再检查参数缺失、未知、重复和类型错误。
`:uppercase`、`:转为大写`、`:大写` 和 `:使用大写` 都指向同一个参数；任意两个同时
传入都会产生重复参数诊断。示例中的 `:大写` 仍能正常编译，但属于迁移用法，应该产生
不阻断编译的替换提示。无论源码使用哪种 spelling 或 display locale，生成的 Python
始终使用 canonical Python parameter name。
通过 `Any` 或 untyped Python boundary 调用时不存在静态签名，因此不能自动翻译其
keyword name。

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

模板在 `let` 或 `fn` 中绑定的名称是卫生的：每次展开都获得全新身份，因此通过 unquote
插入的调用方 syntax 不会被它捕获。写成 `value#` 让该身份显式化；当同一个名称要跨两个
独立模板共享时仍然必须这样写，因为 `value#` 只在单个 syntax quote 内保持一致。

使用调用方名称是刻意行为，需要显式的 `~'name` 操作：

```clojure
(defmacro with-it [expression body]
  `(let [~'it ~expression] ~body))    ; `it` 对调用方 body 可见
```

有两种形态无法静态识别，因此不在该处理范围内：由 unquote-splicing 拼出的 binding
vector（`` `(let [~@pairs] ...) ``），以及模板 binding 位置上的 map 解构。其中的名称
保持原样，请自行使用 `value#` 或 `(gensym)`。

unquote 插入的 syntax 保留调用方 context。模板中由定义模块导出的名称在该模块解析；
与宏定义在同一模块的名称目前仍在调用处解析，调用方可能遮蔽时请使用限定名或通过
unquote 传入。宏运行在确定、受限的 phase 1，不能 import Python、访问网络、读取运行时
值或绕过普通类型和语义检查。

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

小型、package-owned backend 应使用 `~python` provider 并随生成产物分发；普通已安装
Python dependency 应使用 string module name。`osr fmt` 使用 compiler 内置且版本固定的
Ruff profile 在进程内格式化嵌入 Python。VS Code 中嵌入块内的 completion、hover、
navigation、diagnostics、rename 和 range formatting 委托给该 tag 对应的已安装语言支持。
缺少外部语言服务不会影响 Osiris compilation 或 tooling。

Generated Python 对 Osiris 保持 standalone。Reachable standard operation 需要可复用
support 时，linker 在 owning package 的 reserved `__osiris_runtime__` 下生成 ordinary
Python。Osiris source 禁止声明该 package 或直接 import 其中的 private name。

## 标准库资源

Compiler 只内嵌 Kernel 与 Bootstrap source。Public standard-library module 是随匹配
`osiris-lang` distribution 发布的普通 `.osr` resource。Compilation、LSP、LSC、source
map 和 linking 统一通过一个 validated resource provider 读取它们。

Compiler 携带完整 standard resource tree 的 SHA-256 identity。Resource 缺失或被修改
表示安装损坏，compiler 不会静默使用 executable 中另一份 public source。
`osiris-stdlib:///osiris/core/transform.osr` 是由 provider 解析的稳定 logical URI，不表示
源码内嵌在 binary 中。Generated Python 仍然不依赖 `osiris-lang` runtime。

## 完整最小模块

```clojure
(module sample.stats)

(export [Summary positive-sums summarize])

(defstruct Summary
  [count Int]
  [total Int])

~osiris<positive-sums-example>
(positive-sums [-2 1] [1 3])
;; => [1 2 4]
</positive-sums-example>

^{:doc {:default "Return positive Cartesian sums."
        "zh-CN" "返回笛卡尔组合中的正数和。"}
  :examples [positive-sums-example]
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

~osiris<summarize-example>
(summarize [2 3 5])
;; => (Summary :count 3 :total 10)
</summarize-example>

^{:doc {:default "Summarize a vector of integers."}
  :examples [summarize-example]}
(defn ^Summary summarize
  [^{:type (Vector Int)} values]
  (Summary
    :count (count values)
    :total (reduce + 0 values)))
```

写完或修改源码后运行 `osr fmt` 和 `osr check`。使用 `osr lsc` 可以从 CLI 访问与
IDE 通过 LSP 获得的相同诊断、hover、completion、signature、navigation、symbol 和
semantic fact。
使用 `osr check` 验证完整示例；诊断保持本地且确定。
