use super::*;

pub(super) fn parse_phase_declaration(
    form: &Form,
    kind: PhaseDeclarationKind,
) -> Result<FunctionDef, String> {
    let FormKind::List(items) = &form.kind else {
        return Err("phase-1 declaration must be a list".to_owned());
    };
    let declaration = match kind {
        PhaseDeclarationKind::Macro => "defmacro",
        PhaseDeclarationKind::Function => "defn-for-syntax",
    };
    let name = items
        .get(1)
        .and_then(symbol_canonical)
        .ok_or_else(|| format!("`{declaration}` requires a symbol name"))?
        .to_owned();
    let mut index = 2;
    if matches!(
        items.get(index).map(|item| &item.kind),
        Some(FormKind::String(_))
    ) {
        index += 1;
    }
    let parameter_form = items
        .get(index)
        .ok_or_else(|| format!("`{declaration}` requires a parameter vector"))?;
    let params = parse_parameters(parameter_form)?;
    index += 1;
    if items
        .get(index)
        .and_then(symbol_canonical)
        .is_some_and(|name| name == "->")
    {
        if items.get(index + 1).is_none() {
            return Err(format!(
                "`{declaration}` return annotation is missing a type"
            ));
        }
        index += 2;
    }
    let body = items[index..].to_vec();
    if body.is_empty() {
        return Err(format!("`{declaration}` requires a body"));
    }
    Ok(FunctionDef {
        source_name: name.clone(),
        name,
        macro_binding_id: None,
        namespace: None,
        imported: false,
        params,
        body,
        span: form.span,
    })
}

pub(super) fn scoped_phase_name(namespace: &str, name: &str) -> String {
    format!("{namespace}/{name}")
}

pub(super) fn parse_parameters(form: &Form) -> Result<Parameters, String> {
    let FormKind::Vector(items) = &form.kind else {
        return Err("phase-1 parameters must be a vector".to_owned());
    };
    let mut fixed = Vec::new();
    let mut rest = None;
    let mut index = 0;
    while index < items.len() {
        if symbol_canonical(&items[index]) == Some("&") {
            if rest.is_some() || index + 2 != items.len() {
                return Err("`&` must precede the final variadic parameter".to_owned());
            }
            rest = Some(Box::new(parse_pattern(&items[index + 1])?));
            index += 2;
            continue;
        }
        fixed.push(parse_pattern(&items[index])?);
        index += 1;
    }
    Ok(Parameters { fixed, rest })
}

pub(super) fn parse_pattern(form: &Form) -> Result<Pattern, String> {
    match &form.kind {
        FormKind::Symbol(name) if name.canonical == "_" => Ok(Pattern::Ignore),
        FormKind::Symbol(name) if name.canonical != "&" => {
            Ok(Pattern::Bind(name.canonical.clone()))
        }
        FormKind::Vector(_) => parse_parameters(form).map(Pattern::Vector),
        FormKind::Map(items) => parse_map_pattern(items).map(Pattern::Map),
        _ => Err(
            "phase-1 parameters support symbols, vector destructuring, and map destructuring"
                .to_owned(),
        ),
    }
}

pub(super) fn parse_map_pattern(items: &[Form]) -> Result<MapPattern, String> {
    if items.len() % 2 != 0 {
        return Err("phase-1 map destructuring requires key/value pairs".to_owned());
    }

    let mut entries = Vec::new();
    let mut defaults = BTreeMap::new();
    let mut whole = None;
    let mut seen_options = BTreeSet::new();

    for pair in items.chunks_exact(2) {
        let option = match &pair[0].kind {
            FormKind::Keyword(name) => Some(name.canonical.as_str()),
            _ => None,
        };
        match option {
            Some(":keys" | ":strs" | ":syms") => {
                let option = option.expect("matched above");
                if !seen_options.insert(option.to_owned()) {
                    return Err(format!("duplicate `{option}` in phase-1 map destructuring"));
                }
                let FormKind::Vector(names) = &pair[1].kind else {
                    return Err(format!("`{option}` in map destructuring must be a vector"));
                };
                for name_form in names {
                    let FormKind::Symbol(name) = &name_form.kind else {
                        return Err(format!(
                            "`{option}` entries in map destructuring must be symbols"
                        ));
                    };
                    let local = name
                        .canonical
                        .rsplit('/')
                        .next()
                        .unwrap_or(&name.canonical)
                        .to_owned();
                    let lookup = match option {
                        ":keys" => {
                            named_form(true, &format!(":{}", name.canonical), name_form.span)
                        }
                        ":strs" => string(&name.canonical, name_form.span),
                        ":syms" => named_form(false, &name.canonical, name_form.span),
                        _ => unreachable!(),
                    };
                    entries.push(MapPatternEntry {
                        binding: Pattern::Bind(local),
                        lookup,
                    });
                }
            }
            Some(":or") => {
                if !seen_options.insert(":or".to_owned()) {
                    return Err("duplicate `:or` in phase-1 map destructuring".to_owned());
                }
                let FormKind::Map(values) = &pair[1].kind else {
                    return Err("`:or` in map destructuring must be a map".to_owned());
                };
                if values.len() % 2 != 0 {
                    return Err("`:or` defaults must contain key/value pairs".to_owned());
                }
                for default in values.chunks_exact(2) {
                    let Some(name) = symbol_canonical(&default[0]) else {
                        return Err("`:or` default keys must be binding symbols".to_owned());
                    };
                    if defaults
                        .insert(name.to_owned(), default[1].clone())
                        .is_some()
                    {
                        return Err(format!(
                            "duplicate default for `{name}` in map destructuring"
                        ));
                    }
                }
            }
            Some(":as") => {
                if !seen_options.insert(":as".to_owned()) {
                    return Err("duplicate `:as` in phase-1 map destructuring".to_owned());
                }
                whole = Some(Box::new(parse_pattern(&pair[1])?));
            }
            _ => entries.push(MapPatternEntry {
                binding: parse_pattern(&pair[0])?,
                lookup: pair[1].clone(),
            }),
        }
    }

    let bound_names = entries
        .iter()
        .flat_map(|entry| pattern_binding_names(&entry.binding))
        .chain(
            whole
                .iter()
                .flat_map(|pattern| pattern_binding_names(pattern)),
        )
        .collect::<BTreeSet<_>>();
    if let Some(unknown) = defaults.keys().find(|name| !bound_names.contains(*name)) {
        return Err(format!(
            "`:or` provides a default for unknown destructured binding `{unknown}`"
        ));
    }

    Ok(MapPattern {
        entries,
        defaults,
        whole,
    })
}

pub(super) fn pattern_binding_names(pattern: &Pattern) -> Vec<String> {
    match pattern {
        Pattern::Bind(name) => vec![name.clone()],
        Pattern::Ignore => Vec::new(),
        Pattern::Vector(parameters) => parameters
            .fixed
            .iter()
            .flat_map(pattern_binding_names)
            .chain(
                parameters
                    .rest
                    .iter()
                    .flat_map(|pattern| pattern_binding_names(pattern)),
            )
            .collect(),
        Pattern::Map(pattern) => pattern
            .entries
            .iter()
            .flat_map(|entry| pattern_binding_names(&entry.binding))
            .chain(
                pattern
                    .whole
                    .iter()
                    .flat_map(|pattern| pattern_binding_names(pattern)),
            )
            .collect(),
    }
}

pub(super) fn builtin_name(name: &str) -> Option<&'static str> {
    let short = name.rsplit('/').next().unwrap_or(name);
    match short {
        "identity" => Some("identity"),
        "reduced" => Some("reduced"),
        "reduced?" => Some("reduced?"),
        "unreduced" => Some("unreduced"),
        "list" => Some("list"),
        "vector" | "vec" => Some("vector"),
        "hash-map" => Some("hash-map"),
        "hash-set" | "set" => Some("hash-set"),
        "not" => Some("not"),
        "=" => Some("="),
        "not=" => Some("not="),
        "+" => Some("+"),
        "-" => Some("-"),
        "*" => Some("*"),
        "/" => Some("/"),
        "inc" => Some("inc"),
        "dec" => Some("dec"),
        "<" => Some("<"),
        "<=" => Some("<="),
        ">" => Some(">"),
        ">=" => Some(">="),
        "cons" => Some("cons"),
        "concat" => Some("concat"),
        "conj" => Some("conj"),
        "first" => Some("first"),
        "rest" => Some("rest"),
        "next" => Some("next"),
        "nth" => Some("nth"),
        "count" => Some("count"),
        "empty?" => Some("empty?"),
        "seq" => Some("seq"),
        "get" => Some("get"),
        "contains?" => Some("contains?"),
        "assoc" => Some("assoc"),
        "dissoc" => Some("dissoc"),
        "keys" => Some("keys"),
        "vals" => Some("vals"),
        "meta" => Some("meta"),
        "with-meta" => Some("with-meta"),
        "vary-meta" => Some("vary-meta"),
        "gensym" => Some("gensym"),
        "syntax-error" => Some("syntax-error"),
        "symbol" => Some("symbol"),
        "keyword" => Some("keyword"),
        "name" => Some("name"),
        "namespace" => Some("namespace"),
        "str" => Some("str"),
        "nil?" => Some("nil?"),
        "some?" => Some("some?"),
        "symbol?" => Some("symbol?"),
        "keyword?" => Some("keyword?"),
        "list?" => Some("list?"),
        "vector?" => Some("vector?"),
        "map?" => Some("map?"),
        "set?" => Some("set?"),
        "sequential?" => Some("sequential?"),
        "apply" => Some("apply"),
        "map" => Some("map"),
        "mapv" => Some("mapv"),
        "mapcat" => Some("mapcat"),
        "mapcatv" => Some("mapcatv"),
        "filter" => Some("filter"),
        "filterv" => Some("filterv"),
        "reduce" => Some("reduce"),
        "fold" => Some("fold"),
        _ => None,
    }
}
