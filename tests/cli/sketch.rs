//! EXPLORATORY — end-to-end proof for the Elixir-flavoured surface sketch:
//! `.osrx` text translates with `osr sketch`, expands through the unchanged
//! macro system (called by its `:osiris/names` spelling), type-checks, and
//! emits Python.

use super::*;

#[test]
fn sketch_surface_compiles_through_the_unchanged_macro_pipeline() {
    let strategy_sketch = r#"module app.strategy

import_for_syntax macros.select, refer: [选股]

@doc "小市值示范:市值升序打分。"
选股 小市值示范 do
  slot 市值
  select 100 - 市值 * 2
end

@doc "管道示范。"
def 折算(市值 :: Float) :: Float do
  市值 |> 小市值示范() |> half()
end

@doc "Half."
def half(value :: Float) :: Float do
  value / 2
end

export [小市值示范, 折算, half]
"#;
    let fixture = SourceFixture::new(strategy_sketch);
    let sketch_path = fixture.write("src/app/strategy.osrx", strategy_sketch);
    fixture.write(
        "src/macros/select.osr",
        r#"(module macros.select)

^{:doc {:default "Tiny defselect: slot becomes the parameter, select the body."}
  :osiris/names {"zh-CN" {:preferred 选股}}}
(defmacro defselect [name slot-clause select-clause]
  `(defn ^Float ~name [^Float ~(first (rest slot-clause))]
     ~(first (rest select-clause))))

(export [defselect])
"#,
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"sketch-demo\"\nversion = \"0.1.0\"\n",
    )
    .expect("project configuration should be written");
    fs::write(
        fixture.directory.join("osiris.jsonc"),
        r#"{"source":["src"]}"#,
    )
    .expect("Osiris configuration should be written");

    let translated_path = fixture.directory.join("src/app/strategy.osr");
    let output = osr(&[
        "sketch",
        path_argument(&sketch_path),
        "-o",
        path_argument(&translated_path),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let translated = fs::read_to_string(&translated_path).expect("translated source should exist");
    assert!(translated.contains("(选股 小市值示范"), "{translated}");
    assert!(
        translated.contains("(half (小市值示范 市值))"),
        "{translated}"
    );

    let out_dir = fixture.directory.join("sketch-build");
    let output = osr(&[
        "compile",
        path_argument(&translated_path),
        path_argument(&fixture.directory.join("src/macros/select.osr")),
        "--out-dir",
        path_argument(&out_dir),
        "--emit",
        "py,osri",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let generated =
        fs::read_to_string(out_dir.join("app/strategy.py")).expect("generated Python should exist");
    assert!(
        generated.contains("def 小市值示范(市值: float) -> float:"),
        "{generated}"
    );
    assert!(generated.contains("return 100 - 市值 * 2"), "{generated}");
    assert!(
        generated.contains("小市值示范:市值升序打分。"),
        "{generated}"
    );
}
