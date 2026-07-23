use super::{
    BindingKind, CONFUSABLE_IDENTIFIER, INVISIBLE_IDENTIFIER, MIXED_SCRIPT_IDENTIFIER,
    NameAllocator, lint_forms_strict, lint_identifier_strict, python_identifier,
};
use crate::{reader::read, source::Span};

#[test]
fn maps_lisp_and_unicode_names_deterministically() {
    assert_eq!(python_identifier("rolling-mean"), "rolling_mean");
    assert_eq!(python_identifier("empty?"), "empty_p");
    assert_eq!(python_identifier("归一化数据"), "归一化数据");
    assert_eq!(python_identifier("class"), "class_");
}

#[test]
fn aliases_share_the_target_binding() {
    let mut allocator = NameAllocator::default();
    let target = allocator
        .declare(
            "example",
            "rolling-mean",
            BindingKind::Function,
            Span::default(),
        )
        .expect("canonical declaration should succeed");
    allocator
        .alias("时序均值", &target, Span::default())
        .expect("alias should succeed");

    assert_eq!(allocator.resolve("时序均值"), Some(&target.id));
}

#[test]
fn rejects_nfc_collisions() {
    let mut allocator = NameAllocator::default();
    allocator
        .declare("example", "e\u{301}", BindingKind::Value, Span::default())
        .expect("first spelling should succeed");
    let error = allocator
        .declare("example", "é", BindingKind::Value, Span::default())
        .expect_err("NFC-equivalent spelling must collide");
    assert_eq!(error.code, "OSR-N0001");
}

#[test]
fn rejects_python_nfkc_collisions() {
    let mut allocator = NameAllocator::default();
    allocator
        .declare("example", "K", BindingKind::Value, Span::default())
        .expect("first spelling should succeed");
    let error = allocator
        .declare("example", "Ｋ", BindingKind::Value, Span::default())
        .expect_err("Python NFKC-equivalent spelling must collide");
    assert_eq!(error.code, "OSR-N0002");
}

#[test]
fn strict_unicode_lint_accepts_chinese_and_east_asian_latin_names() {
    assert!(lint_identifier_strict("数据处理流程", Span::new(0, 18)).is_empty());
    assert!(lint_identifier_strict("API接口", Span::new(0, 9)).is_empty());
    assert!(lint_identifier_strict("価格Series", Span::new(0, 12)).is_empty());
}

#[test]
fn strict_unicode_lint_reports_confusable_and_mixed_scripts() {
    let spelling = "pаypal"; // The second character is Cyrillic small a.
    let lints = lint_identifier_strict(spelling, Span::new(10, 10 + spelling.len()));
    let codes = lints.iter().map(|lint| lint.code).collect::<Vec<_>>();
    assert!(codes.contains(&CONFUSABLE_IDENTIFIER));
    assert!(codes.contains(&MIXED_SCRIPT_IDENTIFIER));
    assert_eq!(
        lints
            .iter()
            .find(|lint| lint.code == CONFUSABLE_IDENTIFIER)
            .expect("confusable warning")
            .span,
        Span::new(11, 13)
    );
}

#[test]
fn strict_unicode_lint_reports_invisible_characters() {
    let spelling = "alpha\u{200d}";
    let lints = lint_identifier_strict(spelling, Span::new(4, 4 + spelling.len()));
    let invisible = lints
        .iter()
        .find(|lint| lint.code == INVISIBLE_IDENTIFIER)
        .expect("invisible warning");
    assert_eq!(invisible.span, Span::new(9, 12));
}

#[test]
fn strict_unicode_lint_walks_recovered_source_forms() {
    let document = read("(def pаypal alpha\u{200d})");
    let lints = lint_forms_strict(&document.forms);
    assert!(lints.iter().any(|lint| lint.code == CONFUSABLE_IDENTIFIER));
    assert!(
        lints
            .iter()
            .any(|lint| lint.code == MIXED_SCRIPT_IDENTIFIER)
    );
    assert!(lints.iter().any(|lint| lint.code == INVISIBLE_IDENTIFIER));
}
