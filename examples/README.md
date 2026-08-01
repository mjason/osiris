# Osiris examples

[`surface.ois`](surface.ois) is the authoring-surface tour (OEP-0005): the
`.ois` syntax every new module uses — calls, infix operators, `|>`, macro
consumption, and wrapper-free Python chains — importing the form-level
tutorial modules from the same project. Start there.

The `.osr` files below demonstrate **form-level** constructs the surface
does not carry yet (OEP-0005 R012): rich metadata, documentation example
blocks, `fn`/`forv`/`let`, macro definitions, and embedded Python
providers. A `.ois` consumer imports them with no difference.

[`demo-project`](demo-project) is the canonical runnable example. It is a
complete `uv` project with its own `pyproject.toml`, `osiris.jsonc`, source
root, multiple imported modules, and embedded Python boundary:

```console
cd examples/demo-project
uv sync
uv run osr check
uv run osr run src/demo/main.osr
```

The remaining files are compiler-repository fixtures collected by the root
`osiris.jsonc`. They stay useful for focused syntax and generated-artifact
inspection, but they are not a substitute for the complete demo project.

`osiris.jsonc` 中的 `source = ["examples"]` 将本目录设为 Osiris source
root。模块名由相对路径确定：

```text
examples/hello.osr               -> hello
examples/tutorial/transforms.osr -> tutorial.transforms
examples/tutorial/macros.osr     -> tutorial.macros
examples/tutorial/embedded.osr   -> tutorial.embedded
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

Public API 的文档示例遵循 OEP-0004：每个完整示例使用一个有名称的
`~osiris` 块，`:examples` 通过未加引号的 symbol 静态引用它：

```clojure
~osiris<sum-three-example>
(sum-three 2 3 5)
;; => 10
</sum-three-example>

^{:doc {:default "Sum three integers."}
  :examples [sum-three-example]}
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

教程还包含一个有类型边界的嵌入 Python 模块：

```clojure
~python<text-tools>
def normalize_text(value: str) -> str:
    return value.strip().casefold()
</text-tools>

(extern python text-tools
  (defn ^Str normalize-text [^Str value]))
```

`tutorial.embedded/normalize` 将这个 foreign function 包装为普通的
Osiris 函数。生成的 Python 会把嵌入模块释放到 `__osiris_runtime__`，不需要
安装 `osiris` Python runtime 包。仓库内的兼容性套件使用同一条编译、分发和执行
路径：

```console
cargo test --test compatibility
cargo test --test compatibility behavior_embedded_python
```
