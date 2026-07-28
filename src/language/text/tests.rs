use super::*;

#[test]
fn wide_characters_measure_two_columns_and_marks_measure_none() {
    assert_eq!(canonical_width("abc"), 3);
    // Han, full-width forms, and emoji are unambiguously wide.
    assert_eq!(canonical_width("因子定义"), 8);
    assert_eq!(canonical_width("ＡＢ"), 4);
    assert_eq!(canonical_width("最高价_复权"), 11);
    // Half-width katakana stays one column.
    assert_eq!(canonical_width("ｱｲｳ"), 3);
    // A combining mark rides on its base character.
    assert_eq!(canonical_width("e\u{301}"), 1);
}

#[test]
fn tabs_advance_to_the_next_stop_from_their_own_column() {
    assert_eq!(expand_tabs("\tx"), format!("{}x", " ".repeat(8)));
    assert_eq!(expand_tabs("ab\tx"), format!("ab{}x", " ".repeat(6)));
    // A wide character consumes two columns before the stop is computed.
    assert_eq!(expand_tabs("值\tx"), format!("值{}x", " ".repeat(6)));
    assert_eq!(expand_tabs("no tabs"), "no tabs");
}

#[test]
fn a_short_line_is_returned_whole() {
    let window = line_window("(+ 收盘价 最高价)", 3, 9, 80);
    assert_eq!(window.text, "(+ 收盘价 最高价)");
    assert_eq!(window.start_column, 0);
    assert!(!window.elided_before);
    assert!(!window.elided_after);
}

#[test]
fn a_long_line_keeps_the_focus_visible_within_the_budget() {
    let line = format!("{}目标{}", "宽".repeat(40), "尾".repeat(40));
    let focus_start = 80;
    let focus_end = 84;
    let window = line_window(&line, focus_start, focus_end, 40);

    assert!(window.elided_before);
    assert!(window.elided_after);
    assert!(terminal_width(&window.text) <= 40, "{}", window.text);
    assert!(window.text.contains("目标"), "{}", window.text);

    // The caret offset computed against the window must land on the focus.
    let caret_column = focus_start - window.start_column;
    let rendered = &window.text;
    let prefix_width = split_at_column(rendered, caret_column).0;
    assert_eq!(terminal_width(prefix_width), caret_column);
    assert!(
        rendered[prefix_width.len()..].starts_with("目标"),
        "{rendered}"
    );
}

#[test]
fn a_focus_at_the_line_start_does_not_elide_its_left_context() {
    let line = format!("目标{}", "尾".repeat(60));
    let window = line_window(&line, 0, 4, 40);
    assert!(!window.elided_before);
    assert!(window.elided_after);
    assert_eq!(window.start_column, 0);
    assert!(window.text.starts_with("目标"), "{}", window.text);
}

#[test]
fn splitting_never_separates_a_mark_from_its_base() {
    let text = "ae\u{301}b";
    let (head, tail) = split_at_column(text, 2);
    assert_eq!(head, "ae\u{301}");
    assert_eq!(tail, "b");
}

#[test]
fn canonical_width_ignores_the_ambiguous_width_setting() {
    // The canonical format is a compiler contract, so it must measure the same
    // way regardless of how the reader's terminal is configured.
    assert_eq!(canonical_width("→"), 1);
    assert_eq!(canonical_width("±"), 1);
}
