use super::LineIndex;

#[test]
fn locations_count_unicode_characters_not_bytes() {
    let source = "ab\n中x";
    let index = LineIndex::new(source);
    assert_eq!(index.line_column(source, 3), (2, 1));
    assert_eq!(index.line_column(source, 6), (2, 2));
}

#[test]
fn recognizes_lf_crlf_and_cr_line_endings() {
    let source = "a\nb\r\nc\rd";
    let index = LineIndex::new(source);

    assert_eq!(index.line_column(source, 2), (2, 1));
    assert_eq!(index.line_column(source, 5), (3, 1));
    assert_eq!(index.line_column(source, 7), (4, 1));
}
