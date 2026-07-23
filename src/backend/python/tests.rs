use std::process::Command;

use super::compile_module;
use crate::{ast::lower_document, hir::lower_module, reader::read, types::PythonVersion};

fn compile(source: &str) -> String {
    let document = read(source);
    let ast = lower_document(&document);
    let result = lower_module(&ast.module, "example");
    assert!(
        document.diagnostics.is_empty(),
        "{:?}",
        document.diagnostics
    );
    assert!(ast.diagnostics.is_empty(), "{:?}", ast.diagnostics);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    compile_module(&result.module, PythonVersion::PYTHON_3_9)
        .expect("backend should compile")
        .source
}

#[test]
fn emits_readable_typed_function_and_value() {
    let source =
        compile("(defn square [[x Float]] -> Float (* x x)) (def answer Float (square 3.0))");
    assert!(
        source.contains("def square(x: float) -> float:"),
        "{source}"
    );
    assert!(source.contains("return x * x"), "{source}");
    assert!(source.contains("answer: float = square(3.0)"), "{source}");
}

#[test]
fn emits_explicit_python_decorators_with_arguments_and_stable_order() {
    let source = compile(
        r#"(py/import host.runtime :as host)
           (py/decorate publish
             (host.register :extra-data {"columns" ["value" "year"]}))
           (defn ^Any publish
             [^Any context [^Str field = "value"]]
             (context.emit field))
           (py/decorate Point host.component)
           (defstruct Point [value Int])"#,
    );
    assert!(source.contains("import host.runtime as host"), "{source}");
    assert!(
        source.contains(
            "@host.register(extra_data={\"columns\": (\"value\", \"year\")})\n\
             def publish(context: Any, field: str = \"value\") -> Any:"
        ),
        "{source}"
    );
    assert!(
        source.contains("@host.component\n@dataclass(frozen=True)\nclass Point:"),
        "{source}"
    );
}

#[test]
fn lowers_control_flow_and_structured_collections() {
    let source = compile("(defn choose [[x Int]] -> Int (let [y (+ x 1)] (if (> y 0) y 0)))");
    assert!(source.contains("y = x + 1"), "{source}");
    assert!(source.contains("if y > 0:"), "{source}");
    assert!(source.contains("return y"), "{source}");
}

#[test]
fn lowers_nested_runtime_destructuring_to_readable_assignments() {
    let source = compile(
        r#"(defn total [[entry (Map Str Int)]] -> Int
                 (let [{:keys [left right] :or {right 5} :as whole} entry
                       [first second] [left right]]
                   (+ first second)))"#,
    );
    assert!(source.contains("[\"left\"]"), "{source}");
    assert!(source.contains(".get(\"right\", 5)"), "{source}");
    assert!(source.contains("whole ="), "{source}");
    assert!(source.contains("first ="), "{source}");
    assert!(source.contains("second ="), "{source}");
    assert!(source.contains("return first + second"), "{source}");

    let structure = compile(
        r#"(defstruct Point [x Int] [y Int])
               (defn point-total [[point Point]] -> Int
                 (let [{:keys [x y]} point] (+ x y)))"#,
    );
    assert!(structure.contains(".x"), "{structure}");
    assert!(structure.contains(".y"), "{structure}");

    let parameters = compile(
        r#"(defn entry-total [[{:keys [left right]} (Map Str Int)]] -> Int
                 (+ left right))
               (defn pair-total [[[left right] (Vector Int)]] -> Int
                 (+ left right))"#,
    );
    assert!(
        parameters.contains("def entry_total(_u0_arg0: dict[str, int]) -> int:"),
        "{parameters}"
    );
    assert!(
        parameters.contains("def pair_total(_u0_arg1: tuple[int, ...]) -> int:"),
        "{parameters}"
    );
    assert!(parameters.contains("[\"left\"]"), "{parameters}");
    assert!(parameters.contains("[0]"), "{parameters}");
}

#[test]
fn emits_frozen_struct_with_invariant_and_factory_default() {
    let source = compile(
        "(defstruct Point [x Int] [child Any = (+ 1 2)] (check (> x 0) \"x must be positive\"))\n             (def point Point (Point :x 1))",
    );
    assert!(
        source.contains("from dataclasses import dataclass, field"),
        "{source}"
    );
    assert!(source.contains("@dataclass(frozen=True)"), "{source}");
    assert!(source.contains("class Point:"), "{source}");
    assert!(source.contains("default_factory=lambda"), "{source}");
    assert!(
        source.contains("def __post_init__(self) -> None:"),
        "{source}"
    );
    assert!(source.contains("x must be positive"), "{source}");
}

#[test]
fn maps_struct_type_variables_to_python_generic_parameters() {
    let source = compile("(defstruct (Box T) [value T])");
    assert!(source.contains("T = TypeVar(\"T\")"), "{source}");
    assert!(
        source.contains("from typing import Generic, TypeVar"),
        "{source}"
    );
    assert!(source.contains("class Box(Generic[T]):"), "{source}");
    assert!(source.contains("value: T"), "{source}");
    let output = Command::new("python3")
        .arg("-c")
        .arg(&source)
        .output()
        .expect("python3 should execute generated generic struct");
    assert!(
        output.status.success(),
        "generated generic struct failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        source
    );
}

#[test]
fn emits_parseable_literal_annotations_for_axes_and_frame_schema() {
    let source = compile(
        r#"(defstruct (Array T Axes) [values Any])
               (defstruct (Frame Schema KeyMarker KeyValue OrderMarker OrderValue)
                 [values Any])
               (defn array-id
                  [[values (Array Float [:time :feature])]]
                  -> (Array Float [:time :feature])
                  values)
               (defn frame-id
                  [[frame (Frame {:value Float :time Datetime :category Str}
                                 :key [:time :category]
                                 :order [:time])]]
                  -> (Frame {:category Str :value Float :time Datetime}
                            :key [:time :category]
                            :order [:time])
                  frame)"#,
    );
    assert!(
        source.lines().any(|line| {
            line.strip_prefix("from typing import ")
                .is_some_and(|names| names.split(", ").any(|name| name == "Literal"))
        }),
        "{source}"
    );
    assert!(source.contains("Literal[\"[:time :feature]\"]"), "{source}");
    assert!(
        source.contains("Literal[\"{:category Str :time Datetime :value Float}\"]"),
        "{source}"
    );
    let output = Command::new("python3")
        .arg("-c")
        .arg(&source)
        .output()
        .expect("python3 should parse generated literal annotations");
    assert!(
        output.status.success(),
        "generated Python failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        source
    );
}

#[test]
fn keeps_complex_lambda_helpers_in_their_closure_scope() {
    let source = compile(
        "(defn make [[base Int]] -> Any\n \
                 (fn [[x Int]] (let [y (+ base x)] y)))\n \
             (def result Any ((make 2) 3))",
    );
    // The helper must be nested under `make`; placing it after the module
    // definition would leave `base` unresolved when the callback runs.
    assert!(
        source.contains("def make(base: int) -> Any:\n    def _osr_lambda_"),
        "{source}"
    );
    let script = format!("{source}\nprint(result)\n");
    let output = Command::new("python3")
        .arg("-c")
        .arg(script)
        .output()
        .expect("python3 should execute generated closure");
    assert!(
        output.status.success(),
        "generated Python failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        source
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "5");
}
