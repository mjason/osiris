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
            .all(|line| line.chars().count() <= MAX_LINE_WIDTH),
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
    let expected = concat!(
        "(filter even?\n",
        "        (range 1 1000000000000000000000000000000000000000000000000000000000000000))\n\n",
        "^{:doc\n  {:default \"Return the value.\" \"zh-CN\" \"返回该值。\"}\n",
        "  :category \"example\"\n  :since \"0.3.0\"}\n",
        "(defn ^{:type A} identity [^{:type A} value]\n  value)\n",
    );
    let formatted = format_source(source).expect("valid source");
    assert_eq!(formatted, expected);
    assert_eq!(format_source(&formatted).unwrap(), formatted);
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
