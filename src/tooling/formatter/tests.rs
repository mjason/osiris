use super::*;

#[test]
fn formatting_is_idempotent_and_preserves_lossless_contents() {
    let source = "; heading\r\n ^{:doc \"a  b\"}foo,   [1  2 ; item\n3]  ";
    let formatted = format_source(source).expect("valid source");
    assert_eq!(
        formatted,
        "; heading\n^{:doc \"a  b\"} foo\n\n[1 2 ; item\n  3]\n"
    );
    assert_eq!(format_source(&formatted).unwrap(), formatted);
}

#[test]
fn invalid_source_is_never_formatted() {
    let error = format_source("(def value [1 2)").expect_err("invalid source");
    assert!(!error.diagnostics.is_empty());
}

#[test]
fn long_forms_wrap_deterministically_without_changing_reader_meaning() {
    let source = "(extern python \"osiris.kernel\" ^{:python/name \"first_operation\"} (defn first-operation [first second third fourth] (+ first second third fourth)) ^{:python/name \"second_operation\"} (defn second-operation [value] value))\n";
    let formatted = format_source(source).expect("valid source");
    assert!(formatted.lines().count() > 1, "{formatted}");
    assert_eq!(format_source(&formatted).unwrap(), formatted);
    assert!(
        formatted
            .lines()
            .filter(|line| !line.contains("first_operation"))
            .all(|line| crate::text::canonical_width(line) <= MAX_LINE_WIDTH),
        "{formatted}"
    );
}

#[test]
fn uses_clojure_semantic_indentation_for_core_forms() {
    let source = concat!(
        "(defn add-values [left right] (+ left right))\n",
        "(let [thing1 \"some stuff\" thing2 \"other stuff\"] (foo thing1 thing2))\n",
        "(if (ready? value) (publish value) (wait value))\n",
        "(cond (neg? n) \"negative\" (pos? n) \"positive\" :else \"zero\")\n",
        "(->> (range 1 10) (filter even?) (map (partial * 2)))\n",
    );
    let expected = concat!(
        "(defn add-values [left right]\n  (+ left right))\n\n",
        "(let [thing1 \"some stuff\"\n      thing2 \"other stuff\"]\n",
        "  (foo thing1 thing2))\n\n",
        "(if (ready? value)\n  (publish value)\n  (wait value))\n\n",
        "(cond\n  (neg? n) \"negative\"\n  (pos? n) \"positive\"\n",
        "  :else \"zero\")\n\n",
        "(->> (range 1 10)\n     (filter even?)\n     (map (partial * 2)))\n",
    );
    let formatted = format_source(source).expect("valid source");
    assert_eq!(formatted, expected);
    assert_eq!(format_source(&formatted).unwrap(), formatted);
}

#[test]
fn aligns_long_calls_and_osiris_metadata_extensions() {
    let source = concat!(
        "(filter even? (range 1 1000000000000000000000000000000000000000000000000000000000000000))\n",
        "^{:doc {:default \"Return the value.\" \"zh-CN\" \"返回该值。\"} ",
        ":category \"example\" :since \"0.3.0\"} ",
        "(defn ^{:type A} identity [^{:type A} value] value)\n",
    );
    // Aligning under `even?` would put the `(range ...)` argument at column 82.
    // The call is a named body form — a symbol then lists — so the name stays
    // on the head line and the body indents two (FORMAT_VERSION 8).
    let expected = concat!(
        "(filter even?\n",
        "  (range 1 1000000000000000000000000000000000000000000000000000000000000000))\n\n",
        "^{:doc\n  {:default \"Return the value.\" \"zh-CN\" \"返回该值。\"}\n",
        "  :category \"example\"\n  :since \"0.3.0\"}\n",
        "(defn ^{:type A} identity [^{:type A} value]\n  value)\n",
    );
    let formatted = format_source(source).expect("valid source");
    assert_eq!(formatted, expected);
    assert_eq!(format_source(&formatted).unwrap(), formatted);
    assert!(
        formatted
            .lines()
            .all(|line| crate::text::canonical_width(line) <= MAX_LINE_WIDTH),
        "{formatted}"
    );
}

#[test]
fn a_named_body_call_with_many_clauses_indents_them_two() {
    let source = concat!(
        "(filter-out-values even-predicate (range-values 1 10) ",
        "(range-values 20 30) (range-values 40 50))\n",
    );
    let expected = concat!(
        "(filter-out-values even-predicate\n",
        "  (range-values 1 10)\n",
        "  (range-values 20 30)\n",
        "  (range-values 40 50))\n",
    );
    assert_eq!(format_source(source).unwrap(), expected);
}

#[test]
fn groups_extern_leaves_and_keeps_comment_blocks() {
    let source = concat!(
        ";; Kernel declarations.\n;; Kept together.\n",
        "(extern python \"osiris.kernel\" ",
        "^{:python/name \"first\"} (defn first [value]) ",
        "^{:python/name \"second\"} (defn second [value]))\n",
    );
    let expected = concat!(
        ";; Kernel declarations.\n;; Kept together.\n",
        "(extern python \"osiris.kernel\"\n",
        "  ^{:python/name \"first\"}\n  (defn first [value])\n",
        "  ^{:python/name \"second\"}\n  (defn second [value]))\n",
    );
    assert_eq!(format_source(source).unwrap(), expected);
}

#[test]
fn aligns_wide_unicode_callees_by_display_column() {
    let source = concat!(
        "(规则定义 FormatText ",
        "(参数 [^Int length = 20]) ",
        "(节点 规范文本 ",
        "(-> 输入文本 (截取 :长度 length :最小长度 1) (转为大写))) ",
        "(输出 (-> 规范文本 (追加后缀 \"!\"))))\n",
    );
    let formatted = format_source(source).expect("valid source");
    assert_eq!(format_source(&formatted).unwrap(), formatted);
    assert_eq!(crate::text::canonical_width("界"), 2);
    assert_eq!(crate::text::canonical_width("e\u{301}"), 1);
    assert_eq!(crate::text::canonical_width("·"), 1);

    let lines = formatted.lines().collect::<Vec<_>>();
    // `规则定义` is eight columns wide, so aligning its arguments under the
    // first one would indent every following line by ten and push the widest
    // argument past 80. The call is a named body form, so the name keeps the
    // head line and every clause indents two (FORMAT_VERSION 8).
    assert_eq!(lines[0], "(规则定义 FormatText", "{formatted}");
    for line in lines.iter().skip(1).filter(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("(参数") || trimmed.starts_with("(节点") || trimmed.starts_with("(输出")
    }) {
        assert_eq!(line.len() - line.trim_start().len(), 2, "{formatted}");
    }

    let threaded_value_column = lines
        .iter()
        .find(|line| line.contains("(-> 输入文本"))
        .map(|line| display_column(line, "输入文本"))
        .expect("threaded value");
    for line in lines.iter().filter(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("(截取") || trimmed.starts_with("(转为大写")
    }) {
        assert_eq!(line.len() - line.trim_start().len(), threaded_value_column);
    }
    assert!(
        lines
            .iter()
            .all(|line| crate::text::canonical_width(line) <= MAX_LINE_WIDTH)
    );
}

#[test]
fn nested_collections_are_packed_against_the_column_they_start_at() {
    // A vector nested this deep starts near column 20. Packing it against
    // column zero, as a plan made before anything was emitted must, overflows
    // the line; CJK items make the gap large enough to see plainly.
    let source = concat!(
        "(defn ^Int 计算 [^Int x]\n",
        "  (选择 账户\n",
        "    [[\"机构\" 机构资金买入额] [\"大户\" 大户资金买入额] [\"中户\" 中户资金买入额] ",
        "[\"散户\" 散户资金买入额] [\"主力\" 主力资金买入额]]))\n",
    );
    let formatted = format_source(source).expect("valid source");
    assert_eq!(format_source(&formatted).unwrap(), formatted);
    assert!(
        formatted
            .lines()
            .all(|line| crate::text::canonical_width(line) <= MAX_LINE_WIDTH),
        "{formatted}"
    );
}

#[test]
fn an_embedded_block_keeps_its_own_language_width() {
    // Ruff's canonical width is 88 columns. Counting those lines against the
    // Osiris budget would make the layout loop chase a target it cannot reach,
    // and re-breaking them is not this formatter's business.
    let long_call = format!("value_{}", "x".repeat(60));
    let source = format!(
        "~python<backend>\ndef normalize({long_call}: str) -> str:\n    return {long_call}\n</backend>\n"
    );
    let formatted = format_source(&source).expect("valid embedded source");
    assert_eq!(format_source(&formatted).unwrap(), formatted);
    assert!(formatted.contains(&long_call), "{formatted}");
}

#[test]
fn formats_embedded_osiris_and_python_with_their_canonical_formatters() {
    let source = concat!(
        "~osiris<example>\n(reduce + 0 [1  2 3])\n</example>\n",
        "~python<backend>\ndef normalize(value:str)->str:\n return value.strip()\n</backend>\n",
    );
    let expected = concat!(
        "~osiris<example>\n(reduce + 0 [1 2 3])\n</example>\n\n",
        "~python<backend>\ndef normalize(value: str) -> str:\n    return value.strip()\n</backend>\n",
    );
    let formatted = format_source(source).expect("valid embedded source");
    assert_eq!(formatted, expected);
    assert_eq!(format_source(&formatted).unwrap(), formatted);
}

#[test]
fn preserves_generic_embedded_body_content() {
    let source = "~json<settings>\n{\"theme\":  \"dark\"}\n</settings>\n";
    let formatted = format_source(source).expect("valid embedded source");
    assert_eq!(formatted, source);
}

fn display_column(line: &str, needle: &str) -> usize {
    let byte_offset = line.find(needle).expect("text on formatted line");
    crate::text::canonical_width(&line[..byte_offset])
}

#[test]
fn a_named_body_macro_call_keeps_the_name_and_indents_clauses() {
    let source = "(选股定义 小市值_周黎明 (因子 短动量 :权重位 排名门槛) (因子 长动量 :权重位 分位下限) (节点 排名靠前 (按条件取值 (小于等于 (排名 短动量 :同组 [交易日期]) 排名门槛) 1 0)))\n";
    let formatted = format_source(source).expect("formats");
    let lines = formatted.lines().collect::<Vec<_>>();
    assert!(
        lines[0].starts_with("(选股定义 小市值_周黎明"),
        "the name stays on the head line: {formatted}"
    );
    assert!(
        lines[1].starts_with("  (因子") && lines[3].starts_with("  (节点"),
        "top-level clauses indent exactly two: {formatted}"
    );
    // The three-element clause keeps its expression opening on the head line
    // and folds inside it.
    assert!(
        lines[3].starts_with("  (节点 排名靠前 (按条件取值"),
        "{formatted}"
    );
    assert_eq!(
        format_source(&formatted).expect("idempotent"),
        formatted,
        "reformatting must be a fixed point"
    );
}
