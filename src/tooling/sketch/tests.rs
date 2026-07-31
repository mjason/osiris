use super::translate;

fn translated(source: &str) -> String {
    translate(source).expect("translation should succeed")
}

#[test]
fn a_module_header_is_an_ordinary_parenless_call() {
    assert_eq!(
        translated("module 策略库.小市值.周黎明"),
        "(module 策略库.小市值.周黎明)\n"
    );
}

#[test]
fn imports_use_keyword_arguments() {
    assert_eq!(
        translated("import lib.marks, refer: [加倍]"),
        "(import lib.marks :refer [加倍])\n"
    );
    assert_eq!(
        translated("import_for_syntax macros.select, refer: :all"),
        "(import-for-syntax macros.select :refer :all)\n"
    );
    assert_eq!(
        translated("import lib.marks, as: m"),
        "(import lib.marks :as m)\n"
    );
}

#[test]
fn pipes_insert_the_value_as_first_argument() {
    assert_eq!(
        translated("收盘 |> pct_change(5) |> rank()"),
        "(rank (pct_change 收盘 5))\n"
    );
    assert_eq!(translated("x |> f"), "(f x)\n");
}

#[test]
fn infix_operators_become_prefix_calls() {
    assert_eq!(
        translated("rank(市值) <= 门槛 + 1"),
        "(<= (rank 市值) (+ 门槛 1))\n"
    );
    assert_eq!(translated("a == b"), "(= a b)\n");
    assert_eq!(translated("a != b"), "(not= a b)\n");
}

#[test]
fn definitions_carry_types_and_documentation() {
    let source = r#"@doc "Add one."
def 加一(value :: Int) :: Int do
  value + 1
end"#;
    assert_eq!(
        translated(source),
        "^{:doc \"Add one.\"}\n(defn ^Int 加一 [^Int value]\n  (+ value 1))\n"
    );
}

#[test]
fn a_do_block_call_becomes_a_named_body_macro_call() {
    let source = r#"defselect 小市值 do
  slot short_mom, weight: rank_threshold
  slot market_cap
  with is_top?, if_else(rank(short_mom) <= rank_threshold, 1, 0)
  where pct_rank(short_mom) > pct_floor
  select rank(market_cap)
end"#;
    let expected = "(defselect 小市值\n  \
                    (slot short_mom :weight rank_threshold)\n  \
                    (slot market_cap)\n  \
                    (with is_top? (if_else (<= (rank short_mom) rank_threshold) 1 0))\n  \
                    (where (> (pct_rank short_mom) pct_floor))\n  \
                    (select (rank market_cap)))\n";
    assert_eq!(translated(source), expected);
}

#[test]
fn if_expressions_translate_to_if_forms() {
    assert_eq!(translated("if a > b do 1 else 0 end"), "(if (> a b) 1 0)\n");
}

#[test]
fn translated_text_parses_with_the_osiris_reader() {
    let source = r#"module app.main

@doc "Doubled and bumped."
def 加倍再加一(value :: Int) :: Int do
  value |> 加倍() |> 加一()
end

def 加倍(value :: Int) :: Int do
  value * 2
end

def 加一(value :: Int) :: Int do
  value + 1
end

export [加倍再加一, 加倍, 加一]
"#;
    let text = translated(source);
    let document = crate::reader::read(&text);
    assert!(
        document.diagnostics.is_empty(),
        "reader diagnostics on translated text: {:?}\n{text}",
        document.diagnostics
    );
}
