use super::*;

impl Lowerer {
    pub(super) fn lower_params(&mut self, form: &Form, phase: FunctionPhase) -> Vec<Param> {
        let parts = match &form.kind {
            FormKind::Vector(parts) => parts.as_slice(),
            _ => {
                self.error(
                    AST_EXPECTED_VECTOR,
                    "parameters must be a vector",
                    form.span,
                );
                return Vec::new();
            }
        };
        let mut params = Vec::new();
        let mut index = 0;
        let mut next_is_variadic = false;
        let mut saw_variadic = false;
        while let Some(part) = parts.get(index) {
            if is_ampersand_symbol(part) {
                if saw_variadic || next_is_variadic {
                    self.error(
                        AST_WRONG_SHAPE,
                        "parameter vector contains duplicate &",
                        part.span,
                    );
                }
                saw_variadic = true;
                next_is_variadic = true;
                index += 1;
                if parts.get(index).is_none() {
                    self.error(
                        AST_WRONG_SHAPE,
                        "& requires a variadic parameter",
                        part.span,
                    );
                }
                continue;
            }
            let param = self.lower_param(part, next_is_variadic, phase);
            let consumed = 1;
            params.push(param);
            if next_is_variadic && index + consumed < parts.len() {
                self.error(
                    AST_WRONG_SHAPE,
                    "variadic parameter must be the final parameter",
                    part.span,
                );
            }
            next_is_variadic = false;
            index += consumed;
        }
        params
    }

    pub(super) fn lower_param(
        &mut self,
        form: &Form,
        variadic: bool,
        phase: FunctionPhase,
    ) -> Param {
        let info = NodeInfo::from_form(form);
        // Syntax-quoted macro templates may splice parameter names into a
        // generated runtime function, e.g. ``(fn [~time ~previous] ...)``.
        // The reader macro wrapper is intentional syntax data, not a runtime
        // value; retain the spliced symbol as the provisional parameter name
        // so AST validation does not reject an otherwise valid template.
        if phase == FunctionPhase::Runtime
            && let FormKind::ReaderMacro {
                macro_kind: crate::syntax::ReaderMacroKind::Unquote,
                form: inner,
            } = &form.kind
            && let Some(name) = symbol_name(inner)
        {
            let type_annotation = self
                .lower_metadata_type(&info.metadata, "parameter")
                .annotation;
            return Param {
                span: info.span,
                metadata: info.metadata,
                name,
                pattern: None,
                type_annotation,
                default: None,
                variadic,
            };
        }
        if let Some((pattern_form, declaration_tail)) = destructured_parameter_parts(form, phase) {
            return self.lower_destructured_param(
                form,
                &info,
                pattern_form,
                declaration_tail,
                variadic,
                phase,
            );
        }
        let parts = match &form.kind {
            FormKind::Vector(parts) => parts.as_slice(),
            _ => std::slice::from_ref(form),
        };
        if parts.is_empty() || (parts.len() != 1 && parts.len() != 3) {
            self.error(
                AST_WRONG_SHAPE,
                "a parameter is a name or `[name = default]`; attach types with Rich Metadata",
                form.span,
            );
        }
        let name = parts
            .first()
            .and_then(|part| self.require_name(part, "parameter name"))
            .unwrap_or_else(error_name);
        let target_form = parts.first().unwrap_or(form);
        let mut metadata_type = self.lower_metadata_type(&target_form.metadata, "parameter");
        if !std::ptr::eq(target_form, form) {
            let wrapper_type = self.lower_metadata_type(&form.metadata, "parameter");
            if metadata_type.present && wrapper_type.present {
                self.error(
                    AST_CONFLICTING_TYPE_ANNOTATION,
                    "parameter type metadata appears on both its declaration and name; name metadata takes precedence",
                    target_form.span,
                );
            } else if wrapper_type.present {
                metadata_type = wrapper_type;
            }
        }
        let type_annotation = metadata_type.annotation;
        let mut default = None;
        let mut index = 1;
        if parts.get(index).is_some_and(is_equal_symbol) {
            index += 1;
            if let Some(value) = parts.get(index) {
                default = Some(self.lower_expr(value));
                index += 1;
            } else {
                self.error(
                    AST_WRONG_SHAPE,
                    "parameter = requires a default expression",
                    form.span,
                );
            }
        }
        if index < parts.len() {
            self.error(
                AST_WRONG_SHAPE,
                "unexpected forms after parameter declaration; attach types with `^Type` or `^{:type Type}`",
                form.span,
            );
        }
        Param {
            span: info.span,
            metadata: merge_declaration_metadata(info.metadata, &target_form.metadata),
            name,
            pattern: None,
            type_annotation,
            default,
            variadic,
        }
    }

    pub(super) fn lower_destructured_param(
        &mut self,
        form: &Form,
        info: &NodeInfo,
        pattern_form: &Form,
        parts: &[Form],
        variadic: bool,
        phase: FunctionPhase,
    ) -> Param {
        let mut metadata_type = self.lower_metadata_type(&pattern_form.metadata, "parameter");
        if !std::ptr::eq(pattern_form, form) {
            let wrapper_type = self.lower_metadata_type(&form.metadata, "parameter");
            if metadata_type.present && wrapper_type.present {
                self.error(
                    AST_CONFLICTING_TYPE_ANNOTATION,
                    "parameter type metadata appears on both its declaration and pattern; pattern metadata takes precedence",
                    pattern_form.span,
                );
            } else if wrapper_type.present {
                metadata_type = wrapper_type;
            }
        }
        let type_annotation = metadata_type.annotation;
        let mut default = None;
        let mut index = 0;
        if parts.get(index).is_some_and(is_equal_symbol) {
            index += 1;
            if let Some(value) = parts.get(index) {
                default = Some(self.lower_expr(value));
                index += 1;
            } else {
                self.error(
                    AST_WRONG_SHAPE,
                    "destructured parameter = requires a default expression",
                    form.span,
                );
            }
        }
        if index < parts.len() {
            self.error(
                AST_WRONG_SHAPE,
                "unexpected forms after destructured parameter declaration",
                form.span,
            );
        }
        if phase == FunctionPhase::Runtime
            && matches!(pattern_form.kind, FormKind::Vector(_))
            && type_annotation.is_none()
        {
            self.error(
                AST_WRONG_SHAPE,
                "runtime vector destructuring requires an explicit Rich Metadata type; use `^{:type (Vector T)} [...]` or `^Any [...]`",
                form.span,
            );
        }
        let parameter_index = self.next_pattern_parameter;
        self.next_pattern_parameter += 1;
        let spelling = format!("\0arg{parameter_index}");
        Param {
            span: info.span,
            metadata: info.metadata.clone(),
            name: Name {
                spelling: spelling.clone(),
                canonical: spelling,
            },
            pattern: Some(self.lower_pattern(pattern_form)),
            type_annotation,
            default,
            variadic,
        }
    }

    pub(super) fn lower_type(&mut self, form: &Form) -> TypeExpr {
        let kind = match &form.kind {
            FormKind::Symbol(name) => TypeExprKind::Name(name.clone()),
            FormKind::List(parts) if parts.is_empty() => TypeExprKind::Literal(form.clone()),
            FormKind::List(parts) => {
                let constructor = Box::new(self.lower_type(&parts[0]));
                let args = parts[1..]
                    .iter()
                    .map(|part| self.lower_type(part))
                    .collect::<Vec<_>>();
                if matches!(
                    &constructor.kind,
                    TypeExprKind::Name(name) if name.canonical == "Fn"
                ) {
                    let parameter_form = parts.get(1);
                    let return_form = match parts.get(2..) {
                        Some([arrow, result])
                            if symbol_name(arrow).is_some_and(|name| name.canonical == "->") =>
                        {
                            Some(result)
                        }
                        Some([result]) => Some(result),
                        _ => None,
                    };
                    if let (
                        Some(Form {
                            kind: FormKind::Vector(parameters),
                            ..
                        }),
                        Some(return_form),
                    ) = (parameter_form, return_form)
                    {
                        return TypeExpr::from_form(
                            form,
                            TypeExprKind::Function {
                                parameters: parameters
                                    .iter()
                                    .map(|parameter| self.lower_type(parameter))
                                    .collect(),
                                return_type: Box::new(self.lower_type(return_form)),
                            },
                        );
                    }
                }
                match constructor.kind {
                    TypeExprKind::Name(ref name) if name.canonical == "Union" => {
                        TypeExprKind::Union(args)
                    }
                    TypeExprKind::Name(ref name) if name.canonical == "Tuple" => {
                        TypeExprKind::Tuple(args)
                    }
                    _ => TypeExprKind::Apply { constructor, args },
                }
            }
            FormKind::Vector(_) | FormKind::Map(_) | FormKind::Set(_) => {
                TypeExprKind::Literal(form.clone())
            }
            _ => TypeExprKind::Literal(form.clone()),
        };
        TypeExpr::from_form(form, kind)
    }

    pub(super) fn lower_pattern(&mut self, form: &Form) -> Pattern {
        let kind = match &form.kind {
            FormKind::Symbol(name) if name.canonical == "_" => PatternKind::Ignore,
            FormKind::Symbol(name) => PatternKind::Name(name.clone()),
            FormKind::Vector(parts) => {
                PatternKind::Vector(parts.iter().map(|part| self.lower_pattern(part)).collect())
            }
            FormKind::Map(parts) => {
                if parts.len() % 2 != 0 {
                    self.error(
                        AST_EXPECTED_PAIR,
                        "map pattern requires key/value pairs",
                        form.span,
                    );
                }
                PatternKind::Map(
                    parts
                        .chunks(2)
                        .filter_map(|pair| {
                            Some((
                                self.lower_pattern(pair.first()?),
                                self.lower_pattern(pair.get(1)?),
                            ))
                        })
                        .collect(),
                )
            }
            _ => PatternKind::Literal(form.clone()),
        };
        Pattern {
            span: form.span,
            metadata: form.metadata.clone(),
            kind,
        }
    }

    pub(super) fn lower_map_expr(&mut self, form: &Form, parts: &[Form]) -> ExprKind {
        if parts.len() % 2 != 0 {
            self.error(
                AST_EXPECTED_PAIR,
                "map expression requires key/value pairs",
                form.span,
            );
        }
        ExprKind::Map(
            parts
                .chunks(2)
                .filter_map(|pair| {
                    Some((
                        self.lower_expr(pair.first()?),
                        self.lower_expr(pair.get(1)?),
                    ))
                })
                .collect(),
        )
    }

    pub(super) fn lower_name_collection(&mut self, form: &Form, what: &str) -> Vec<Name> {
        let parts = match &form.kind {
            FormKind::Vector(parts) | FormKind::List(parts) | FormKind::Set(parts) => parts,
            _ => {
                self.error(
                    AST_EXPECTED_VECTOR,
                    format!("{what} collection must be a vector or list"),
                    form.span,
                );
                return Vec::new();
            }
        };
        parts
            .iter()
            .filter_map(|part| self.require_name(part, what))
            .collect()
    }

    pub(super) fn require_name(&mut self, form: &Form, what: &str) -> Option<Name> {
        if let Some(name) = template_symbol_name(form) {
            return Some(name);
        }
        self.error(
            AST_INVALID_NAME,
            format!("{what} must be a symbol"),
            form.span,
        );
        None
    }

    pub(super) fn error_expr(&mut self, span: Span, message: &str) -> Expr {
        self.error(AST_WRONG_SHAPE, message, span);
        Expr {
            span,
            metadata: Vec::new(),
            kind: ExprKind::Error(message.to_owned()),
        }
    }
}
