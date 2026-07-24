# Osiris examples

`osiris.jsonc` 中的 `source = ["examples"]` 将本目录设为 Osiris source
root。模块名由相对路径确定：

```text
examples/hello.osr               -> hello
examples/tutorial/transforms.osr -> tutorial.transforms
examples/tutorial/macros.osr     -> tutorial.macros
examples/tutorial/app.osr        -> tutorial.app
```

入口模块可用三种不同的导入形式：

```clojure
(import tutorial.transforms :as transforms :refer [sum-values])
(import-for-syntax tutorial.macros :refer [unless])
(py/import math :as math)
```

- `import` 读取另一个 `.osr` 模块导出的 `.osri` 接口，并形成运行时模块依赖。
- `import-for-syntax` 读取编译期宏及其 phase-1 helper，不生成 Python import。
- `py/import` 面向 Python 标准库或由 uv 管理的 Python 包，并生成 Python import。

没有显式 `osiris.core` import 时，core 的公开 binding 会自动 refer。示例只有在需要
限制 surface、排除名称或重命名时才显式 import core。

Public API 的文档示例遵循 OEP-0004：外层 vector 保存多个 example，内层 vector
每个 string 保存一行格式化后的 Osiris source：

```clojure
^{:doc {:default "Sum three integers."}
  :examples
  [["(sum-three 2 3 5)"
    ";; => 10"]]}
(defn ^Int sum-three [^Int left ^Int middle ^Int right]
  (+ left middle right))
```

可以直接检查或编译多文件教程：

```console
cargo run --bin osr -- check examples/tutorial/app.osr
cargo run --bin osr -- build
```

`compile` 以当前 project/distribution 为发布单元，因此 source root 中的模块会
一起生成到 `dist/`，而不只是单独生成入口文件。运行时依赖
`tutorial.transforms` 会出现在 `app.py` 的 Python import 中；
`tutorial.macros` 只参与编译期展开，不会成为 `app.py` 的运行时 import。
