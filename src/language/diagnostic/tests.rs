use super::*;
use crate::text::terminal_width;

/// Column the caret row's first `^` sits at, and how many columns it covers.
fn caret(rendered: &str) -> (usize, usize) {
    let caret_row = rendered
        .lines()
        .find(|line| line.contains('^'))
        .expect("a rendered diagnostic has a caret row");
    let body = caret_row
        .split_once("| ")
        .expect("the caret row has a gutter")
        .1;
    (
        terminal_width(&body[..body.len() - body.trim_start().len()]),
        body.trim().chars().count(),
    )
}

/// Column the span starts at inside the quoted source row, and its width.
fn quoted_span(rendered: &str, needle: &str) -> (usize, usize) {
    let source_row = rendered
        .lines()
        .find(|line| line.contains(needle))
        .expect("the quoted source row contains the span");
    let body = source_row
        .split_once("| ")
        .expect("the source row has a gutter")
        .1;
    let offset = body.find(needle).expect("needle is in the quoted row");
    (terminal_width(&body[..offset]), terminal_width(needle))
}

fn error(source: &str, needle: &str) -> String {
    let start = source.find(needle).expect("needle is in the source");
    let diagnostic = Diagnostic::error("OSR-T0000", "test", Span::new(start, start + needle.len()));
    render("example.osr", source, &diagnostic)
}

#[test]
fn the_caret_lands_on_the_span_across_wide_characters() {
    let source = "(module demo)\n(+ 收盘价 最高价 不存在的名字)\n";
    let rendered = error(source, "不存在的名字");
    assert_eq!(caret(&rendered), quoted_span(&rendered, "不存在的名字"));
    // Twelve columns, not the six characters the name is written with.
    assert_eq!(caret(&rendered).1, 12);
}

#[test]
fn the_gutter_grows_with_the_line_number_and_the_caret_row_follows() {
    let mut source = String::from("(module demo)\n");
    for index in 0..120 {
        source.push_str(&format!("(def v{index} {index})\n"));
    }
    source.push_str("(def r 缺失)\n");
    let rendered = error(&source, "缺失");
    assert_eq!(caret(&rendered), quoted_span(&rendered, "缺失"));

    // Every frame row must agree on where the gutter pipe sits.
    let pipes = rendered
        .lines()
        .filter(|line| line.contains(" |"))
        .map(|line| terminal_width(line.split_once('|').expect("pipe").0))
        .collect::<Vec<_>>();
    assert_eq!(pipes.len(), 3, "{rendered}");
    assert!(pipes.iter().all(|column| *column == pipes[0]), "{rendered}");
}

#[test]
fn tabs_are_expanded_so_the_caret_agrees_with_the_quoted_line() {
    let source = "(module demo)\n(def 值\t缺失)\n";
    let rendered = error(source, "缺失");
    assert!(!rendered.contains('\t'), "{rendered}");
    assert_eq!(caret(&rendered), quoted_span(&rendered, "缺失"));
}

#[test]
fn a_long_line_is_windowed_around_the_span() {
    let filler = (1..12)
        .map(|index| format!("财务费用TTM第{index}项"))
        .collect::<Vec<_>>()
        .join(" ");
    let source = format!("(module demo)\n(横向求和 {filler} 缺失项TTM)\n");
    let rendered = error(&source, "缺失项TTM");

    let source_row = rendered
        .lines()
        .find(|line| line.contains("缺失项TTM"))
        .expect("quoted row");
    assert!(source_row.starts_with(" 2 | ..."), "{rendered}");
    assert!(
        terminal_width(source_row) <= QUOTED_LINE_BUDGET + 8,
        "{source_row}"
    );
    // Windowing must not move the caret off the span.
    assert_eq!(caret(&rendered), quoted_span(&rendered, "缺失项TTM"));
}

#[test]
fn related_locations_render_as_notes_and_name_a_foreign_module() {
    let source = "(module demo)\n(def value 1)\n";
    let start = source.find("value").expect("needle");
    let diagnostic = Diagnostic::error("OSR-T0000", "test", Span::new(start, start + 5))
        .with_related(vec![
            Related::new(
                RelatedKind::MacroCallSite,
                "expanded from here",
                Span::new(1, 7),
            ),
            Related::new(
                RelatedKind::MacroDefinition,
                "macro `emit` is defined in `dep.other`",
                Span::new(4000, 4010),
            )
            .in_module(Some("dep.other".to_owned())),
        ]);
    let rendered = render("example.osr", source, &diagnostic);

    assert!(
        rendered.contains("  = note: expanded from here (example.osr:1:2)"),
        "{rendered}"
    );
    // A span from another module has no position here, so none is invented.
    assert!(
        rendered.contains("  = note: macro `emit` is defined in `dep.other`\n"),
        "{rendered}"
    );
}
