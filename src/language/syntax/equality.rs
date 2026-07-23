pub(crate) fn source_form_eq(left: &Form, right: &Form) -> bool {
    left.metadata.len() == right.metadata.len()
        && left
            .metadata
            .iter()
            .zip(&right.metadata)
            .all(|(left, right)| {
                source_form_eq(&left.key, &right.key) && source_form_eq(&left.value, &right.value)
            })
        && match (&left.kind, &right.kind) {
            (FormKind::None, FormKind::None) => true,
            (FormKind::Bool(left), FormKind::Bool(right)) => left == right,
            (FormKind::Integer(left), FormKind::Integer(right))
            | (FormKind::Float(left), FormKind::Float(right))
            | (FormKind::String(left), FormKind::String(right))
            | (FormKind::Error(left), FormKind::Error(right)) => left == right,
            (FormKind::Keyword(left), FormKind::Keyword(right))
            | (FormKind::Symbol(left), FormKind::Symbol(right)) => left == right,
            (FormKind::List(left), FormKind::List(right))
            | (FormKind::Vector(left), FormKind::Vector(right))
            | (FormKind::Map(left), FormKind::Map(right))
            | (FormKind::Set(left), FormKind::Set(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| source_form_eq(left, right))
            }
            (
                FormKind::ReaderMacro {
                    macro_kind: left_kind,
                    form: left,
                },
                FormKind::ReaderMacro {
                    macro_kind: right_kind,
                    form: right,
                },
            ) => left_kind == right_kind && source_form_eq(left, right),
            _ => false,
        }
}

pub(crate) fn datum_eq(left: &Form, right: &Form) -> bool {
    match (&left.kind, &right.kind) {
        (FormKind::None, FormKind::None) => true,
        (FormKind::Bool(left), FormKind::Bool(right)) => left == right,
        (FormKind::Integer(left), FormKind::Integer(right))
        | (FormKind::Float(left), FormKind::Float(right))
        | (FormKind::String(left), FormKind::String(right))
        | (FormKind::Error(left), FormKind::Error(right)) => left == right,
        (FormKind::Keyword(left), FormKind::Keyword(right))
        | (FormKind::Symbol(left), FormKind::Symbol(right)) => left.canonical == right.canonical,
        (FormKind::List(left), FormKind::List(right))
        | (FormKind::Vector(left), FormKind::Vector(right)) => sequence_eq(left, right),
        (FormKind::Map(left), FormKind::Map(right)) => map_eq(left, right),
        (FormKind::Set(left), FormKind::Set(right)) => unordered_eq(left, right),
        (
            FormKind::ReaderMacro {
                macro_kind: left_kind,
                form: left,
            },
            FormKind::ReaderMacro {
                macro_kind: right_kind,
                form: right,
            },
        ) => left_kind == right_kind && datum_eq(left, right),
        _ => false,
    }
}

fn sequence_eq(left: &[Form], right: &[Form]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| datum_eq(left, right))
}

fn unordered_eq(left: &[Form], right: &[Form]) -> bool {
    left.len() == right.len() && match_unordered(left, right, datum_eq)
}

fn map_eq(left: &[Form], right: &[Form]) -> bool {
    if left.len() != right.len() || left.len() % 2 != 0 {
        return false;
    }

    let left_entries = left.chunks_exact(2).collect::<Vec<_>>();
    let right_entries = right.chunks_exact(2).collect::<Vec<_>>();
    match_unordered(&left_entries, &right_entries, |left, right| {
        datum_eq(&left[0], &right[0]) && datum_eq(&left[1], &right[1])
    })
}

fn match_unordered<T>(left: &[T], right: &[T], equals: impl Fn(&T, &T) -> bool) -> bool {
    let mut matched = vec![false; right.len()];
    left.iter().all(|item| {
        right
            .iter()
            .enumerate()
            .find(|(index, candidate)| !matched[*index] && equals(item, candidate))
            .is_some_and(|(index, _)| {
                matched[index] = true;
                true
            })
    })
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
