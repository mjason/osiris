use super::{Form, FormKind, datum_eq};
use crate::source::Span;

fn integer(value: &str) -> Form {
    Form::new(FormKind::Integer(value.to_owned()), Span::default())
}

#[test]
fn map_equality_preserves_key_value_pairs() {
    let left = Form::new(
        FormKind::Map(vec![integer("1"), integer("2")]),
        Span::default(),
    );
    let swapped = Form::new(
        FormKind::Map(vec![integer("2"), integer("1")]),
        Span::default(),
    );

    assert!(!datum_eq(&left, &swapped));
}

#[test]
fn unordered_equality_does_not_reuse_a_match() {
    let repeated = Form::new(
        FormKind::Set(vec![integer("1"), integer("1")]),
        Span::default(),
    );
    let distinct = Form::new(
        FormKind::Set(vec![integer("1"), integer("2")]),
        Span::default(),
    );

    assert!(!datum_eq(&repeated, &distinct));
}
