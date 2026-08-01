# 探索:Elixir 风格表层语法(sketch)

> 状态:已转正——见 OEP-0005(主表层语法)。本文保留为探索记录。
> 原型:`src/tooling/sketch/`,CLI `osr sketch FILE [-o OUT]`,
> 端到端测试 `tests/cli/sketch.rs`。

## 论点

S 表达式让大多数程序员没有熟悉感,这是 Lisp 系语言几十年的采纳障碍。
Elixir 证明了一件事:**宏系统需要的不是 S 表达式文本,而是同构的数据表
示**。Elixir 的表层看起来是"普通代码"(`def`、`do…end`、`f(a, b)`、中
缀运算符),但每段代码都对应一棵可引用、可改写的 AST(`{form, meta,
args}` 三元组),宏的能力一点没少——Phoenix、Ecto 全是宏写的。

Osiris 恰好具备同样的前提:宏收到的是 **Form 数据**,不是文本。表层语
法只是 reader 的输入格式,可以更换而不动宏体系。本探索验证:给 Osiris
一个 Elixir 观感的第二表层(`.osrx`),翻译成规范 S 表达式后走完全不变
的管线。

## 核心设计:一切皆调用

翻译器只认四个特殊形式:`def`/`defmacro`(签名语法)、`@doc`(文档元
数据)、`if`(表达式)。**其余全部是调用**——`module`、`import`、
`export` 都不是关键字,而是无括号调用,天然落到既有核心形式上:

| 新表层 | 规范 Osiris |
| --- | --- |
| `module app.策略` | `(module app.策略)` |
| `import lib.marks, refer: [加倍]` | `(import lib.marks :refer [加倍])` |
| `import_for_syntax m.select, refer: :all` | `(import-for-syntax m.select :refer :all)` |
| `export [f, g]` | `(export [f g])` |
| `f(a, b)` / `m.f(a)` | `(f a b)` / `(m.f a)` |
| `rank(市值) <= 门槛 + 1` | `(<= (rank 市值) (+ 门槛 1))` |
| `a == b` / `a != b` | `(= a b)` / `(not= a b)` |
| `x \|> pct_change(5) \|> rank()` | `(rank (pct_change x 5))` |
| `if c do a else b end` | `(if c a b)` |
| `key: value`(调用实参) | `:key value` |

`def` 的类型标注用 `::`(Elixir typespec 观感),落到 `^Type` 元数据:

```elixir
@doc "Add one."
def 加一(value :: Int) :: Int do
  value + 1
end
```

```clojure
^{:doc "Add one."}
(defn ^Int 加一 [^Int value]
  (+ value 1))
```

## `|>`:选 Elixir 语义(首参插入)

`x |> f(a)` 翻译为 `(f x a)`——被管道的值插到**第一个**实参位。这与
Clojure `->` 一致(线程首位),与 pandas 式"数据在前、参数在后"的函
数签名契合。SQL 感的数据变换天然成链:

```elixir
收盘 |> pct_change(20) |> rank() |> 分位()
```

## do-block = named-body 宏调用 = SQL 式声明

`名字 实参 do … end` 翻译为 `(名字 实参 (子句…) (子句…) …)`,块内每条
语句成为一个子句列表——**正是现有 named-body 宏(defselect)的调用形
状**。qlab 的选股宏一行不改就能被新表层调用:

```elixir
选股 小市值_周黎明 do
  slot 短动量, weight: 排名门槛
  slot 长动量, weight: 分位下限
  slot 行业, classifier: true, weight: 保留行业数
  slot 市值
  with 入选, if_else(rank(短动量) <= 排名门槛, 1, 0)
  with 行业强度, group_sum(入选, 行业)
  where rank_groups(行业强度, 行业, direction: 行业) <= 保留行业数
  where pct_rank(长动量) > 分位下限
  select rank(市值)
end
```

对照现在的 S 表达式版(`策略库/小市值/周黎明.osr`),信息一一对应,但
对没接触过 Lisp 的人,上面这段就是"带 do 块的普通配置",和 Ecto 的
query DSL、SQL 的 SELECT…WHERE 一个观感。配合 `:osiris/clauses` 的子句
悬停文档,声明式体验完整。

## 端到端证明(原型已跑通)

`tests/cli/sketch.rs`:上述形状的 `.osrx` 经 `osr sketch` 翻译,宏按
**中文别名**(`:osiris/names` 的 `选股`)触发展开,类型检查通过,生成:

```python
def 小市值示范(市值: float) -> float:
    """小市值示范:市值升序打分。"""
    return 100 - 市值 * 2
```

宏体系、`:osiris/names`、`@doc` 文档、类型标注、管道全部在同一条不变
的管线里复合。

## 关键发现:`-` 标识符是唯一的硬边界

中缀语法把 `-` 让给了减法,标识符只能用 `_`、CJK、`?`、`!`(Elixir 的
同款取舍)。而 Osiris 规范名只做 NFC 折叠,`short_mom` 与 `short-mom`
是**不同的名字**——连字符命名的既有生态(`pct-rank`、`if-else`)在新
表层无法作为裸标识符引用。三条出路,可叠加:

1. **CJK 命名完全绕开该问题**——中文名没有连字符,与"中文优先"的整
   体方向天然契合。全中文策略源码在两个表层下拼写完全一致。
2. **`:osiris/names` 补拼写**——连字符名加一个下划线 spelling,本探索
   期间落地的 R062/R062A/R062C 别名机制正好是现成的桥。
3. 新库直接用下划线/CJK 命名。

## 未做与风险

- **`quote`/`unquote` 未实现**:defmacro 体只支持普通 phase-1 表达式。
  完整方案需把 Elixir 的 `quote do … end` 映射到 syntax-quote(``` ` ```
  /`~`/`~@`),是后续最大的一块。
- **工具链双份成本**:fmt、LSP(悬停/跳转/改名)、sourcemap 都要认第
  二表层。原型的 span 只有行号粒度,诊断精度不够。
- **生态分裂风险**:两种表层读同一生态,但一个项目内混用会撕裂代码评
  审和示例文档。若推进,建议规则:**扩展名定表层**(`.osr`/`.osrx`),
  单文件单语法,`.osri` 接口与宏生态完全共享。
- 无括号调用的歧义边界(`if`/`not` 等表达式关键字要从调用头排除)在原
  型里用启发式处理,正式设计需要一份精确的文法。

## 结论

可行,且比预期干净:因为 Osiris 宏在 Form 层工作,新表层只是一个约
600 行的前端,**编译器、宏、接口、别名、文档管线零改动**。`do` 块与
named-body 宏的形状同构,让 SQL 式声明"免费"获得。真正要投入的是
quote 映射与工具链适配;真正的采纳杠杆是 CJK/下划线命名把 `-` 问题化
解为空集。若把这条路走成正式特性,建议以 OEP 定义 `.osrx` 文法与
「扩展名定表层、接口层合流」的共存规则。
