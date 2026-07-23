use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_pattern_bindings(
        &mut self,
        pattern: &ast::Pattern,
        value: Expr,
        metadata: &[MetadataEntry],
        scope: &mut Scope,
        lowered: &mut Vec<LetBinding>,
    ) {
        match &pattern.kind {
            PatternKind::Name(name) => {
                let summaries = value.summaries.clone();
                let id = self.declare_local(
                    name,
                    BindingKind::Value,
                    value.ty.clone(),
                    metadata.to_vec(),
                    pattern.span,
                    scope,
                );
                self.local_value_summaries.insert(id.clone(), summaries);
                lowered.push(LetBinding { binding: id, value });
            }
            PatternKind::Ignore => {
                let _ = self.bind_pattern_temporary(value, pattern.span, scope, lowered);
            }
            PatternKind::Vector(patterns) => {
                let root = self.stabilize_pattern_root(value, pattern.span, scope, lowered);
                let mut index = 0_usize;
                while index < patterns.len() {
                    if pattern_name(&patterns[index]).is_some_and(|name| name == "&") {
                        self.error(
                            "OSR-H0003",
                            "vector rest destructuring is not supported in v0",
                            patterns[index].span,
                        );
                        break;
                    }
                    if pattern_keyword(&patterns[index]).is_some_and(|name| name == "as") {
                        let Some(alias) = patterns.get(index + 1) else {
                            self.error(
                                "OSR-H0003",
                                "`:as` in vector destructuring requires a pattern",
                                patterns[index].span,
                            );
                            break;
                        };
                        self.lower_pattern_bindings(
                            alias,
                            root.clone(),
                            &alias.metadata,
                            scope,
                            lowered,
                        );
                        index += 2;
                        continue;
                    }
                    let element = Expr {
                        span: patterns[index].span,
                        ty: indexed_type(&root.ty),
                        summaries: root.summaries.clone(),
                        kind: ExprKind::Index {
                            value: Box::new(root.clone()),
                            index: Box::new(Expr::pure(
                                patterns[index].span,
                                Type::Int,
                                ExprKind::Integer(index.to_string()),
                            )),
                        },
                    };
                    self.lower_pattern_bindings(
                        &patterns[index],
                        element,
                        &patterns[index].metadata,
                        scope,
                        lowered,
                    );
                    index += 1;
                }
            }
            PatternKind::Map(entries) => {
                let root = self.stabilize_pattern_root(value, pattern.span, scope, lowered);
                self.lower_map_pattern(entries, &root, scope, lowered);
            }
            PatternKind::Literal(_) | PatternKind::Error(_) => {
                self.error(
                    "OSR-H0003",
                    "binding pattern must contain names, vector patterns, or map patterns",
                    pattern.span,
                );
                let _ = self.bind_pattern_temporary(value, pattern.span, scope, lowered);
            }
        }
    }

    /// Reuse stable local roots; evaluate arbitrary destructuring inputs once.
    pub(in crate::hir) fn stabilize_pattern_root(
        &mut self,
        value: Expr,
        span: Span,
        scope: &mut Scope,
        lowered: &mut Vec<LetBinding>,
    ) -> Expr {
        if matches!(&value.kind, ExprKind::Binding(_)) {
            value
        } else {
            self.bind_pattern_temporary(value, span, scope, lowered)
        }
    }

    pub(in crate::hir) fn bind_pattern_temporary(
        &mut self,
        value: Expr,
        span: Span,
        scope: &mut Scope,
        lowered: &mut Vec<LetBinding>,
    ) -> Expr {
        let spelling = format!("\0destructure{}", self.next_scope);
        let name = Name {
            spelling: spelling.clone(),
            canonical: spelling,
        };
        let ty = value.ty.clone();
        let summaries = value.summaries.clone();
        let binding = self.declare_local(
            &name,
            BindingKind::Value,
            ty.clone(),
            Vec::new(),
            span,
            scope,
        );
        self.local_value_summaries
            .insert(binding.clone(), summaries.clone());
        lowered.push(LetBinding {
            binding: binding.clone(),
            value,
        });
        Expr {
            span,
            ty,
            summaries,
            kind: ExprKind::Binding(binding),
        }
    }

    pub(in crate::hir) fn lower_map_pattern(
        &mut self,
        entries: &[(ast::Pattern, ast::Pattern)],
        root: &Expr,
        scope: &mut Scope,
        lowered: &mut Vec<LetBinding>,
    ) {
        let mut defaults = BTreeMap::<String, &ast::Pattern>::new();
        for (option, value) in entries {
            if pattern_keyword(option) != Some("or") {
                continue;
            }
            let PatternKind::Map(values) = &value.kind else {
                self.error(
                    "OSR-H0003",
                    "`:or` in map destructuring must be a map",
                    value.span,
                );
                continue;
            };
            for (name, default) in values {
                let Some(name) = pattern_name(name) else {
                    self.error(
                        "OSR-H0003",
                        "`:or` keys must be destructured names",
                        name.span,
                    );
                    continue;
                };
                defaults.insert(name.to_owned(), default);
            }
        }

        for (key_pattern, value_pattern) in entries {
            match pattern_keyword(key_pattern) {
                Some("keys" | "strs" | "syms") => {
                    let PatternKind::Vector(names) = &value_pattern.kind else {
                        self.error(
                            "OSR-H0003",
                            "`:keys`, `:strs`, and `:syms` require a vector",
                            value_pattern.span,
                        );
                        continue;
                    };
                    for source in names {
                        let PatternKind::Name(source_name) = &source.kind else {
                            self.error(
                                "OSR-H0003",
                                "map shorthand entries must be names",
                                source.span,
                            );
                            continue;
                        };
                        let local_name = destructured_local_name(source_name);
                        let default = defaults
                            .get(&local_name.canonical)
                            .or_else(|| defaults.get(&source_name.canonical))
                            .copied();
                        let value = self.map_pattern_access(
                            root,
                            &source_name.canonical,
                            default,
                            source.span,
                            scope,
                        );
                        let target = ast::Pattern {
                            span: source.span,
                            metadata: source.metadata.clone(),
                            kind: PatternKind::Name(local_name),
                        };
                        self.lower_pattern_bindings(
                            &target,
                            value,
                            &target.metadata,
                            scope,
                            lowered,
                        );
                    }
                }
                Some("as") => self.lower_pattern_bindings(
                    value_pattern,
                    root.clone(),
                    &value_pattern.metadata,
                    scope,
                    lowered,
                ),
                Some("or") => {}
                Some(other) => {
                    let value = self.map_pattern_access(
                        root,
                        other,
                        pattern_binding_name(value_pattern)
                            .and_then(|name| defaults.get(name).copied()),
                        value_pattern.span,
                        scope,
                    );
                    self.lower_pattern_bindings(
                        value_pattern,
                        value,
                        &value_pattern.metadata,
                        scope,
                        lowered,
                    );
                }
                None => {
                    let Some(key) = pattern_static_key(value_pattern) else {
                        self.error(
                            "OSR-H0003",
                            "explicit map destructuring requires a static key",
                            value_pattern.span,
                        );
                        continue;
                    };
                    let value = self.map_pattern_access(
                        root,
                        &key,
                        pattern_binding_name(key_pattern)
                            .and_then(|name| defaults.get(name).copied()),
                        key_pattern.span,
                        scope,
                    );
                    self.lower_pattern_bindings(
                        key_pattern,
                        value,
                        &key_pattern.metadata,
                        scope,
                        lowered,
                    );
                }
            }
        }
    }

    pub(in crate::hir) fn map_pattern_access(
        &mut self,
        root: &Expr,
        key: &str,
        default: Option<&ast::Pattern>,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        if let Some((attribute, ty)) = self.struct_field_type(&root.ty, key) {
            return Expr {
                span,
                ty,
                summaries: root.summaries.clone(),
                kind: ExprKind::Attribute {
                    value: Box::new(root.clone()),
                    attribute,
                },
            };
        }

        let key_expression = Expr::pure(span, Type::Str, ExprKind::String(key.to_owned()));
        let value_type = indexed_type(&root.ty);
        let Some(default) = default else {
            return Expr {
                span,
                ty: value_type,
                summaries: root.summaries.clone(),
                kind: ExprKind::Index {
                    value: Box::new(root.clone()),
                    index: Box::new(key_expression),
                },
            };
        };
        let default = self.lower_pattern_default(default, scope);
        let result_type = self.types.join(&value_type, &default.ty);
        let summaries = root.summaries.join(&default.summaries);
        let callee = Expr::pure(
            span,
            Type::Fn(FunctionType::new(
                vec![Type::Str, default.ty.clone()],
                result_type.clone(),
            )),
            ExprKind::Attribute {
                value: Box::new(root.clone()),
                attribute: "get".to_owned(),
            },
        );
        Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![
                    CallArgument::Positional(key_expression),
                    CallArgument::Positional(default),
                ],
            },
        }
    }

    pub(in crate::hir) fn lower_pattern_default(
        &mut self,
        pattern: &ast::Pattern,
        scope: &mut Scope,
    ) -> Expr {
        match &pattern.kind {
            PatternKind::Name(name) => self.lower_name(name, pattern.span, scope),
            PatternKind::Vector(values) => {
                let values = values
                    .iter()
                    .map(|value| self.lower_pattern_default(value, scope))
                    .collect::<Vec<_>>();
                let ty = self.types.join_all(values.iter().map(|value| &value.ty));
                let summaries = join_summaries(values.iter().map(|value| &value.summaries));
                Expr {
                    span: pattern.span,
                    ty: Type::Vector(Box::new(ty)),
                    summaries,
                    kind: ExprKind::Vector(values),
                }
            }
            PatternKind::Literal(form) => match &form.kind {
                FormKind::None => Expr::pure(form.span, Type::None, ExprKind::None),
                FormKind::Bool(value) => Expr::pure(form.span, Type::Bool, ExprKind::Bool(*value)),
                FormKind::Integer(value) => {
                    Expr::pure(form.span, Type::Int, ExprKind::Integer(value.clone()))
                }
                FormKind::Float(value) => {
                    Expr::pure(form.span, Type::Float, ExprKind::Float(value.clone()))
                }
                FormKind::String(value) => {
                    Expr::pure(form.span, Type::Str, ExprKind::String(value.clone()))
                }
                FormKind::Keyword(name) => Expr::pure(
                    form.span,
                    Type::Str,
                    ExprKind::String(name.canonical.trim_start_matches(':').to_owned()),
                ),
                _ => {
                    self.error(
                        "OSR-H0003",
                        "runtime destructuring defaults must be names or literal data in v0",
                        form.span,
                    );
                    Expr::error(form.span)
                }
            },
            _ => {
                self.error(
                    "OSR-H0003",
                    "invalid runtime destructuring default",
                    pattern.span,
                );
                Expr::error(pattern.span)
            }
        }
    }
}
