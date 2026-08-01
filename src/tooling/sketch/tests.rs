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

#[test]
fn kebab_case_identifiers_survive_and_subtraction_needs_spaces() {
    // OEP-0005: infix yields to the ecosystem's kebab-case names. `-` glues
    // into an identifier when a name character follows without space.
    assert_eq!(
        translated("pct-rank(short-mom) <= keep-n"),
        "(<= (pct-rank short-mom) keep-n)\n"
    );
    assert_eq!(translated("a - b"), "(- a b)\n");
    assert_eq!(translated("100 - 市值 * 2"), "(- 100 (* 市值 2))\n");
    // Glued: one identifier, exactly like Lisp's `x-1`.
    assert_eq!(translated("x-1"), "x-1\n");
    assert_eq!(
        translated("import-for-syntax macros.select, refer: [if-else]"),
        "(import-for-syntax macros.select :refer [if-else])\n"
    );
}

#[test]
fn slash_qualified_names_glue_and_division_needs_spaces() {
    // OEP-0005 R004 (revision 2): `/` follows the same yield rule as `-`.
    assert_eq!(
        translated("py/import builtins, as: py"),
        "(py/import builtins :as py)\n"
    );
    assert_eq!(translated("a / b"), "(/ a b)\n");
    assert_eq!(translated("value / 2"), "(/ value 2)\n");
}

#[test]
fn backtick_names_spell_operator_members() {
    // OEP-0005 R004: backticks spell names the identifier grammar cannot
    // carry — refer lists of operator macros, operator-named call heads.
    assert_eq!(
        translated("import-for-syntax macros.pandas, refer: [`>`, `<=`, if-else]"),
        "(import-for-syntax macros.pandas :refer [> <= if-else])\n"
    );
    assert_eq!(translated("`+`(a, b, c)"), "(+ a b c)\n");
}

#[test]
fn doc_attributes_carry_localized_text() {
    let source = r#"@doc default: "Return the input.", zh-CN: "返回输入。"
def identity(value :: Any) :: Any do
  value
end"#;
    assert_eq!(
        translated(source),
        "^{:doc {:default \"Return the input.\" \"zh-CN\" \"返回输入。\"}}\n(defn ^Any identity [^Any value]\n  value)\n"
    );
}

#[test]
fn postfix_member_chains_translate_to_member_forms() {
    // OEP-0005 R008A: chains on evaluated bases become OEP-0001-R079 member
    // forms; plain name paths stay statically qualified symbols.
    assert_eq!(
        translated("df.rolling(5).mean().pct-change()"),
        "(.pct-change (.mean (df.rolling 5)))\n"
    );
    assert_eq!(
        translated("df.rolling(5).values"),
        "(.-values (df.rolling 5))\n"
    );
    assert_eq!(translated("(a + b).hex()"), "(.hex (+ a b))\n");
    assert_eq!(
        translated("df.rolling(window: 5).mean()"),
        "(.mean (df.rolling :window 5))\n"
    );
    // No chain: still one statically resolved qualified name.
    assert_eq!(translated("df.iloc.values"), "df.iloc.values\n");
}
