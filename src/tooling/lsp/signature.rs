use super::*;

#[derive(Clone, Debug)]
pub(super) struct CallableSignature {
    pub(super) canonical: String,
    pub(super) parameters: Vec<CallableSignatureParameter>,
    pub(super) return_type: Type,
}

#[derive(Clone, Debug)]
pub(super) struct CallableSignatureParameter {
    pub(super) canonical: String,
    pub(super) source_spelling: String,
    pub(super) python: String,
    pub(super) aliases: Vec<String>,
    pub(super) ty: Type,
    pub(super) has_default: bool,
    pub(super) default_text: Option<String>,
    pub(super) variadic: bool,
}

#[derive(Clone, Copy)]
pub(super) struct SourceArgument<'form> {
    span: Span,
    keyword: Option<&'form str>,
}

pub(super) struct MacroSignature<'form> {
    canonical: &'form str,
    parameters: &'form Form,
    variadic: bool,
}

pub(super) fn macro_signature_help(
    document: &OpenDocument,
    trace: &crate::macro_expand::ExpansionTrace,
    offset: usize,
    locale: &str,
) -> Option<SignatureHelp> {
    let signature = macro_signature(document, &trace.macro_binding_id)?;
    let call_form = find_call_form(&document.analysis.document.forms, trace.call_span)?;
    let FormKind::List(items) = &call_form.kind else {
        return None;
    };
    let invoked_name = items
        .first()
        .and_then(form_name)
        .unwrap_or(signature.canonical);
    let parameters = phase_parameter_labels(
        signature.parameters,
        is_chinese_locale(locale),
        signature.variadic,
    )?;
    let arguments = source_macro_arguments(items);
    let active_argument = active_source_argument(items, &arguments, offset);
    let active_parameter =
        (!parameters.is_empty()).then(|| active_argument.min(parameters.len() - 1) as u32);
    let label = format!("{}({})", invoked_name, parameters.join(", "));
    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            parameters: parameters
                .into_iter()
                .map(|label| ParameterInformation { label })
                .collect(),
            active_parameter,
        }],
        active_signature: 0,
        active_parameter,
    })
}

pub(super) fn macro_signature<'document>(
    document: &'document OpenDocument,
    binding_id: &str,
) -> Option<MacroSignature<'document>> {
    for item in &document.analysis.surface.items {
        let crate::ast::ItemKind::Defmacro(macro_) = &item.kind else {
            continue;
        };
        let local_id = crate::name::BindingId::new(
            &document.analysis.hir.name,
            &macro_.name.canonical,
            BindingKind::Macro,
        );
        if local_id.as_str() == binding_id {
            return Some(MacroSignature {
                canonical: &macro_.name.spelling,
                parameters: phase_parameter_form(&macro_.phase_form)?,
                variadic: macro_
                    .params
                    .last()
                    .is_some_and(|parameter| parameter.variadic),
            });
        }
    }
    document
        .macro_interfaces
        .get(binding_id)
        .map(|macro_| MacroSignature {
            canonical: &macro_.canonical,
            parameters: &macro_.parameters,
            variadic: macro_.variadic,
        })
}

pub(super) fn phase_parameter_form(declaration: &Form) -> Option<&Form> {
    let FormKind::List(items) = &declaration.kind else {
        return None;
    };
    let mut index = 2;
    if matches!(
        items.get(index).map(|item| &item.kind),
        Some(FormKind::String(_))
    ) {
        index += 1;
    }
    items
        .get(index)
        .filter(|parameters| matches!(parameters.kind, FormKind::Vector(_)))
}

pub(super) fn phase_parameter_labels(
    parameters: &Form,
    chinese: bool,
    declared_variadic: bool,
) -> Option<Vec<String>> {
    let FormKind::Vector(items) = &parameters.kind else {
        return None;
    };
    let mut labels = Vec::new();
    let mut variadic = false;
    for item in items {
        if form_name(item).is_some_and(|name| name == "&") {
            variadic = true;
            continue;
        }
        let localized = chinese
            .then(|| {
                metadata_aliases(&item.metadata, "")
                    .into_iter()
                    .find(|alias| contains_cjk(alias))
            })
            .flatten();
        let label = localized.unwrap_or_else(|| display_form(item));
        labels.push(if variadic {
            format!("& {label}")
        } else {
            label
        });
        variadic = false;
    }
    debug_assert_eq!(
        labels.last().is_some_and(|label| label.starts_with("& ")),
        declared_variadic
    );
    Some(labels)
}

pub(super) fn display_form(form: &Form) -> String {
    match &form.kind {
        FormKind::None => "none".to_owned(),
        FormKind::Bool(value) => value.to_string(),
        FormKind::Integer(value) | FormKind::Float(value) => value.clone(),
        FormKind::String(value) => {
            serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned())
        }
        FormKind::Keyword(name) | FormKind::Symbol(name) => name.spelling.clone(),
        FormKind::List(items) => format!("({})", display_forms(items)),
        FormKind::Vector(items) => format!("[{}]", display_forms(items)),
        FormKind::Map(items) => format!("{{{}}}", display_forms(items)),
        FormKind::Set(items) => format!("#{{{}}}", display_forms(items)),
        FormKind::ReaderMacro { macro_kind, form } => format!(
            "{}{}",
            match macro_kind {
                crate::syntax::ReaderMacroKind::Quote => "'",
                crate::syntax::ReaderMacroKind::SyntaxQuote => "`",
                crate::syntax::ReaderMacroKind::Unquote => "~",
                crate::syntax::ReaderMacroKind::UnquoteSplicing => "~@",
            },
            display_form(form)
        ),
        FormKind::Error(message) => format!("#<error:{message}>"),
    }
}

pub(super) fn display_forms(forms: &[Form]) -> String {
    forms.iter().map(display_form).collect::<Vec<_>>().join(" ")
}

pub(super) fn callable_signature(
    document: &OpenDocument,
    binding_id: &str,
) -> Option<CallableSignature> {
    for item in &document.analysis.hir.items {
        let hir::ItemKind::Function(function) = &item.kind else {
            continue;
        };
        if function.binding.as_str() == binding_id {
            return Some(local_callable_signature(
                document,
                binding_id,
                &function.parameters,
                &function.return_type,
            ));
        }
    }
    for function in &document.analysis.hir.extern_functions {
        if function.binding.as_str() == binding_id {
            return Some(local_callable_signature(
                document,
                binding_id,
                &function.parameters,
                &function.return_type,
            ));
        }
    }
    document
        .function_interfaces
        .get(binding_id)
        .map(interface_callable_signature)
}

pub(super) fn local_callable_signature(
    document: &OpenDocument,
    binding_id: &str,
    parameters: &[hir::Parameter],
    return_type: &Type,
) -> CallableSignature {
    let interface = document.function_interfaces.get(binding_id);
    let parameters = parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            let binding = document
                .analysis
                .hir
                .bindings
                .iter()
                .find(|binding| binding.name.id == parameter.binding);
            let published = interface.and_then(|interface| interface.parameters.get(index));
            let canonical = binding.map_or_else(
                || {
                    published.map_or_else(
                        || format!("arg{}", index + 1),
                        |parameter| parameter.canonical.clone(),
                    )
                },
                |binding| binding.name.canonical.clone(),
            );
            let mut aliases = binding
                .map(|binding| metadata_aliases(&binding.metadata, &canonical))
                .unwrap_or_default();
            if let Some(published) = published {
                aliases.extend(published.aliases.iter().cloned());
            }
            aliases.sort();
            aliases.dedup();
            CallableSignatureParameter {
                source_spelling: binding.map_or_else(
                    || canonical.clone(),
                    |binding| binding.source_spelling.clone(),
                ),
                python: binding.map_or_else(
                    || crate::name::python_identifier(&canonical),
                    |binding| binding.name.python.clone(),
                ),
                canonical,
                aliases,
                ty: parameter.ty.clone(),
                has_default: parameter.default.is_some(),
                default_text: parameter.default.as_ref().and_then(|default| {
                    source_slice(&document.text, default.span).map(normalize_inline_source)
                }),
                variadic: parameter.variadic,
            }
        })
        .collect();
    CallableSignature {
        canonical: binding_id
            .rsplit("::")
            .next()
            .unwrap_or(binding_id)
            .to_owned(),
        parameters,
        return_type: return_type.clone(),
    }
}

pub(super) fn interface_callable_signature(
    function: &interface::FunctionInterface,
) -> CallableSignature {
    CallableSignature {
        canonical: function
            .binding
            .rsplit("::")
            .next()
            .unwrap_or(&function.binding)
            .to_owned(),
        parameters: function
            .parameters
            .iter()
            .map(|parameter| CallableSignatureParameter {
                canonical: parameter.canonical.clone(),
                source_spelling: parameter.canonical.clone(),
                python: crate::name::python_identifier(&parameter.canonical),
                aliases: parameter.aliases.clone(),
                ty: parameter.ty.clone(),
                has_default: parameter.has_default,
                default_text: None,
                variadic: parameter.variadic,
            })
            .collect(),
        return_type: function.return_type.clone(),
    }
}

pub(super) fn signature_parameter_label(
    parameter: &CallableSignatureParameter,
    chinese: bool,
) -> String {
    let name = if chinese {
        parameter
            .aliases
            .iter()
            .find(|alias| contains_cjk(alias))
            .unwrap_or(&parameter.source_spelling)
    } else {
        &parameter.source_spelling
    };
    let variadic = if parameter.variadic { "& " } else { "" };
    let default = if let Some(value) = &parameter.default_text {
        format!(" = {value}")
    } else if parameter.has_default {
        " = ...".to_owned()
    } else {
        String::new()
    };
    format!("{variadic}{name}: {}{default}", parameter.ty)
}

pub(super) fn source_slice(source: &str, span: Span) -> Option<&str> {
    (span.start <= span.end
        && span.end <= source.len()
        && source.is_char_boundary(span.start)
        && source.is_char_boundary(span.end))
    .then(|| &source[span.start..span.end])
    .filter(|value| !value.trim().is_empty())
}

pub(super) fn normalize_inline_source(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn find_call_form(forms: &[Form], span: Span) -> Option<&Form> {
    forms
        .iter()
        .filter_map(|form| find_call_form_in(form, span))
        .min_by_key(|form| form.span.end.saturating_sub(form.span.start))
}

pub(super) fn find_call_form_in(form: &Form, span: Span) -> Option<&Form> {
    if form.span.start > span.start || form.span.end < span.end {
        return None;
    }
    let children = match &form.kind {
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => items.as_slice(),
        FormKind::ReaderMacro { form, .. } => std::slice::from_ref(form.as_ref()),
        FormKind::None
        | FormKind::Bool(_)
        | FormKind::Integer(_)
        | FormKind::Float(_)
        | FormKind::String(_)
        | FormKind::Keyword(_)
        | FormKind::Symbol(_)
        | FormKind::Error(_) => &[],
    };
    children
        .iter()
        .filter_map(|child| find_call_form_in(child, span))
        .min_by_key(|child| child.span.end.saturating_sub(child.span.start))
        .or_else(|| matches!(form.kind, FormKind::List(_)).then_some(form))
}

pub(super) fn form_name(form: &Form) -> Option<&str> {
    match &form.kind {
        FormKind::Keyword(name) | FormKind::Symbol(name) => Some(&name.spelling),
        _ => None,
    }
}

pub(super) fn source_arguments(items: &[Form]) -> Vec<SourceArgument<'_>> {
    let mut arguments = Vec::new();
    let mut index = 1;
    while index < items.len() {
        if let FormKind::Keyword(keyword) = &items[index].kind {
            let end = items
                .get(index + 1)
                .map_or(items[index].span.end, |value| value.span.end);
            arguments.push(SourceArgument {
                span: Span::new(items[index].span.start, end),
                keyword: Some(keyword.canonical.trim_start_matches(':')),
            });
            index += usize::from(index + 1 < items.len()) + 1;
        } else {
            arguments.push(SourceArgument {
                span: items[index].span,
                keyword: None,
            });
            index += 1;
        }
    }
    arguments
}

pub(super) fn source_macro_arguments(items: &[Form]) -> Vec<SourceArgument<'_>> {
    items
        .iter()
        .skip(1)
        .map(|argument| SourceArgument {
            span: argument.span,
            keyword: None,
        })
        .collect()
}

pub(super) fn active_source_argument(
    items: &[Form],
    arguments: &[SourceArgument<'_>],
    offset: usize,
) -> usize {
    let Some(callee) = items.first() else {
        return 0;
    };
    if offset <= callee.span.end {
        return 0;
    }
    let mut previous_end = callee.span.end;
    for (index, argument) in arguments.iter().enumerate() {
        if previous_end <= offset && offset <= argument.span.end {
            return index;
        }
        previous_end = argument.span.end;
    }
    arguments.len()
}

pub(super) fn active_parameter(
    parameters: &[CallableSignatureParameter],
    arguments: &[SourceArgument<'_>],
    active_argument: usize,
) -> Option<usize> {
    if parameters.is_empty() {
        return None;
    }
    if let Some(argument) = arguments.get(active_argument) {
        if let Some(keyword) = argument.keyword {
            return parameters.iter().position(|parameter| {
                parameter.canonical == keyword
                    || parameter.python == keyword
                    || parameter.aliases.iter().any(|alias| alias == keyword)
            });
        }
        let positional = arguments[..=active_argument]
            .iter()
            .filter(|argument| argument.keyword.is_none())
            .count()
            .saturating_sub(1);
        return Some(positional.min(parameters.len() - 1));
    }

    let positional = arguments
        .iter()
        .filter(|argument| argument.keyword.is_none())
        .count();
    let keyword_parameters = arguments
        .iter()
        .filter_map(|argument| argument.keyword)
        .collect::<Vec<_>>();
    parameters
        .iter()
        .enumerate()
        .skip(positional)
        .find(|(_, parameter)| {
            !keyword_parameters.iter().any(|keyword| {
                parameter.canonical == *keyword
                    || parameter.python == *keyword
                    || parameter.aliases.iter().any(|alias| alias == keyword)
            })
        })
        .map(|(index, _)| index)
        .or(Some(parameters.len() - 1))
}
