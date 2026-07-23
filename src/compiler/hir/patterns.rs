use super::*;

pub(in crate::hir) fn indexed_type(value: &Type) -> Type {
    match value {
        Type::List(item) | Type::Vector(item) | Type::Set(item) => (**item).clone(),
        Type::Map(_, value) => (**value).clone(),
        Type::Tuple(items) => Type::union(items.iter().cloned()),
        Type::Str => Type::Str,
        Type::Bytes => Type::Int,
        Type::Any => Type::Any,
        _ => Type::Unknown,
    }
}

pub(in crate::hir) fn non_nil_type(value: &Type) -> Type {
    match value {
        Type::Option(inner) => (**inner).clone(),
        Type::None => Type::Never,
        other => other.clone(),
    }
}

pub(in crate::hir) fn pattern_name(pattern: &ast::Pattern) -> Option<&str> {
    match &pattern.kind {
        PatternKind::Name(name) => Some(name.canonical.as_str()),
        _ => None,
    }
}

pub(in crate::hir) fn pattern_binding_name(pattern: &ast::Pattern) -> Option<&str> {
    pattern_name(pattern).map(|name| name.rsplit('/').next().unwrap_or(name))
}

pub(in crate::hir) fn pattern_keyword(pattern: &ast::Pattern) -> Option<&str> {
    let PatternKind::Literal(Form {
        kind: FormKind::Keyword(name),
        ..
    }) = &pattern.kind
    else {
        return None;
    };
    Some(name.canonical.trim_start_matches(':'))
}

pub(in crate::hir) fn pattern_static_key(pattern: &ast::Pattern) -> Option<String> {
    let PatternKind::Literal(form) = &pattern.kind else {
        return None;
    };
    match &form.kind {
        FormKind::Keyword(name) => Some(name.canonical.trim_start_matches(':').to_owned()),
        FormKind::String(value) => Some(value.clone()),
        _ => None,
    }
}

pub(in crate::hir) fn destructured_local_name(source: &Name) -> Name {
    Name {
        spelling: source
            .spelling
            .rsplit('/')
            .next()
            .unwrap_or(&source.spelling)
            .to_owned(),
        canonical: source
            .canonical
            .rsplit('/')
            .next()
            .unwrap_or(&source.canonical)
            .to_owned(),
    }
}

pub(in crate::hir) fn join_summaries<'a>(
    summaries: impl IntoIterator<Item = &'a CallSummaries>,
) -> CallSummaries {
    summaries
        .into_iter()
        .fold(CallSummaries::pure_scalar(), |left, right| left.join(right))
}

pub(in crate::hir) fn core_reduced_type_binding() -> BindingId {
    BindingId::new("osiris.prelude", "Reduced", BindingKind::Type)
}

pub(in crate::hir) fn core_delay_type_binding() -> BindingId {
    BindingId::new("osiris.prelude", "Delay", BindingKind::Type)
}

pub(in crate::hir) fn core_future_type_binding() -> BindingId {
    BindingId::new("osiris.prelude", "Future", BindingKind::Type)
}

pub(in crate::hir) fn core_promise_type_binding() -> BindingId {
    BindingId::new("osiris.prelude", "Promise", BindingKind::Type)
}

pub(in crate::hir) fn future_type(value: Type) -> Type {
    Type::Nominal {
        binding: core_future_type_binding().as_str().to_owned(),
        args: vec![value],
    }
}

pub(in crate::hir) fn promise_type(value: Type) -> Type {
    Type::Nominal {
        binding: core_promise_type_binding().as_str().to_owned(),
        args: vec![value],
    }
}

pub(in crate::hir) fn async_value_type(ty: &Type) -> Type {
    match ty {
        Type::Nominal { binding, args }
            if (binding == core_delay_type_binding().as_str()
                || binding == core_future_type_binding().as_str()
                || binding == core_promise_type_binding().as_str())
                && args.len() == 1 =>
        {
            args[0].clone()
        }
        Type::Unknown => Type::Any,
        other => other.clone(),
    }
}

pub(in crate::hir) fn reduced_type(value: Type) -> Type {
    Type::Nominal {
        binding: core_reduced_type_binding().as_str().to_owned(),
        args: vec![value],
    }
}

pub(in crate::hir) fn unreduced_type(ty: &Type) -> Type {
    match ty {
        Type::Nominal { binding, args }
            if binding == core_reduced_type_binding().as_str() && args.len() == 1 =>
        {
            args[0].clone()
        }
        Type::Union(members) => Type::union(members.iter().map(unreduced_type)),
        Type::Option(inner) => Type::option(unreduced_type(inner)),
        other => other.clone(),
    }
}

pub(in crate::hir) fn split_access_name(name: &str) -> Option<(&str, Vec<&str>)> {
    if let Some((base, member)) = name.split_once('/') {
        return Some((base, vec![member]));
    }
    let mut parts = name.split('.');
    let base = parts.next()?;
    let members = parts.collect::<Vec<_>>();
    (!members.is_empty()).then_some((base, members))
}
