use super::*;

impl Lowerer {
    pub(super) fn lower_expr(&mut self, form: &Form) -> Expr {
        let kind = match &form.kind {
            FormKind::None => ExprKind::None,
            FormKind::Bool(value) => ExprKind::Bool(*value),
            FormKind::Integer(value) => ExprKind::Integer(value.clone()),
            FormKind::Float(value) => ExprKind::Float(value.clone()),
            FormKind::String(value) => ExprKind::String(value.clone()),
            FormKind::Keyword(name) => ExprKind::Keyword(name.clone()),
            FormKind::Symbol(name) => ExprKind::Name(name.clone()),
            FormKind::List(parts) => return self.lower_list_expr(form, parts),
            FormKind::Vector(parts) => {
                ExprKind::Vector(parts.iter().map(|part| self.lower_expr(part)).collect())
            }
            FormKind::Map(parts) => self.lower_map_expr(form, parts),
            FormKind::Set(parts) => {
                ExprKind::Set(parts.iter().map(|part| self.lower_expr(part)).collect())
            }
            FormKind::ReaderMacro {
                macro_kind,
                form: inner,
            } => {
                let expression = Box::new(match macro_kind {
                    crate::syntax::ReaderMacroKind::Quote
                    | crate::syntax::ReaderMacroKind::SyntaxQuote => {
                        self.lower_quoted_template(inner)
                    }
                    crate::syntax::ReaderMacroKind::Unquote
                    | crate::syntax::ReaderMacroKind::UnquoteSplicing => self.lower_expr(inner),
                });
                match macro_kind {
                    crate::syntax::ReaderMacroKind::Quote => ExprKind::Quote(expression),
                    crate::syntax::ReaderMacroKind::SyntaxQuote => {
                        ExprKind::SyntaxQuote(expression)
                    }
                    crate::syntax::ReaderMacroKind::Unquote => ExprKind::Unquote(expression),
                    crate::syntax::ReaderMacroKind::UnquoteSplicing => {
                        ExprKind::UnquoteSplicing(expression)
                    }
                }
            }
            FormKind::Error(message) => ExprKind::Error(message.clone()),
        };
        Expr::from_form(form, kind)
    }

    pub(super) fn lower_quoted_template(&mut self, form: &Form) -> Expr {
        let kind = match &form.kind {
            FormKind::None => ExprKind::None,
            FormKind::Bool(value) => ExprKind::Bool(*value),
            FormKind::Integer(value) => ExprKind::Integer(value.clone()),
            FormKind::Float(value) => ExprKind::Float(value.clone()),
            FormKind::String(value) => ExprKind::String(value.clone()),
            FormKind::Keyword(name) => ExprKind::Keyword(name.clone()),
            FormKind::Symbol(name) => ExprKind::Name(name.clone()),
            FormKind::List(parts) => ExprKind::List(
                parts
                    .iter()
                    .map(|part| self.lower_quoted_template(part))
                    .collect(),
            ),
            FormKind::Vector(parts) => ExprKind::Vector(
                parts
                    .iter()
                    .map(|part| self.lower_quoted_template(part))
                    .collect(),
            ),
            FormKind::Map(parts) => ExprKind::Map(
                parts
                    .chunks_exact(2)
                    .map(|pair| {
                        (
                            self.lower_quoted_template(&pair[0]),
                            self.lower_quoted_template(&pair[1]),
                        )
                    })
                    .collect(),
            ),
            FormKind::Set(parts) => ExprKind::Set(
                parts
                    .iter()
                    .map(|part| self.lower_quoted_template(part))
                    .collect(),
            ),
            FormKind::ReaderMacro {
                macro_kind,
                form: inner,
            } => {
                let expression = Box::new(match macro_kind {
                    crate::syntax::ReaderMacroKind::Unquote
                    | crate::syntax::ReaderMacroKind::UnquoteSplicing => self.lower_expr(inner),
                    crate::syntax::ReaderMacroKind::Quote
                    | crate::syntax::ReaderMacroKind::SyntaxQuote => {
                        self.lower_quoted_template(inner)
                    }
                });
                match macro_kind {
                    crate::syntax::ReaderMacroKind::Quote => ExprKind::Quote(expression),
                    crate::syntax::ReaderMacroKind::SyntaxQuote => {
                        ExprKind::SyntaxQuote(expression)
                    }
                    crate::syntax::ReaderMacroKind::Unquote => ExprKind::Unquote(expression),
                    crate::syntax::ReaderMacroKind::UnquoteSplicing => {
                        ExprKind::UnquoteSplicing(expression)
                    }
                }
            }
            FormKind::Error(message) => ExprKind::Error(message.clone()),
        };
        Expr::from_form(form, kind)
    }

    pub(super) fn lower_list_expr(&mut self, form: &Form, parts: &[Form]) -> Expr {
        if parts.is_empty() {
            return Expr::from_form(
                form,
                ExprKind::List(parts.iter().map(|part| self.lower_expr(part)).collect()),
            );
        }
        let Some(head) = parts.first().and_then(symbol_name) else {
            return self.lower_call_expr(form, parts);
        };
        match ExpressionForm::from_name(&head.canonical) {
            Some(ExpressionForm::Fn) => self.lower_fn_expr(form, parts),
            Some(ExpressionForm::Let) => self.lower_let_expr(form, parts),
            Some(ExpressionForm::If) => self.lower_if_expr(form, parts),
            Some(ExpressionForm::Do) => Expr::from_form(
                form,
                ExprKind::Do(
                    parts[1..]
                        .iter()
                        .map(|part| self.lower_expr(part))
                        .collect(),
                ),
            ),
            Some(ExpressionForm::Try) => self.lower_try_expr(form, parts),
            Some(ExpressionForm::Raise) => self.lower_raise_expr(form, parts),
            None => self.lower_call_expr(form, parts),
        }
    }

    pub(super) fn lower_call_expr(&mut self, form: &Form, parts: &[Form]) -> Expr {
        let callee_form = parts.first().unwrap_or(form);
        let callee = Box::new(self.lower_expr(callee_form));
        let mut args = Vec::new();
        let mut positional = Vec::new();
        let mut keywords = Vec::new();
        let mut index = 1;
        while index < parts.len() {
            let part = &parts[index];
            if let Some(key) = keyword_name(part) {
                let Some(value_form) = parts.get(index + 1) else {
                    self.error(
                        AST_EXPECTED_PAIR,
                        format!("keyword argument `{}` requires a value", key.spelling),
                        part.span,
                    );
                    break;
                };
                let value = self.lower_expr(value_form);
                let argument = KeywordArg {
                    span: part.span.cover(value_form.span),
                    metadata: part.metadata.clone(),
                    key,
                    value,
                };
                keywords.push(argument.clone());
                args.push(CallArg::Keyword(argument));
                index += 2;
            } else {
                let value = self.lower_expr(part);
                positional.push(value.clone());
                args.push(CallArg::Positional(value));
                index += 1;
            }
        }
        let info = NodeInfo::from_form(form);
        Expr::from_form(
            form,
            ExprKind::Call(CallExpr {
                span: info.span,
                metadata: info.metadata,
                callee,
                args,
                positional,
                keywords,
            }),
        )
    }

    pub(super) fn lower_fn_expr(&mut self, form: &Form, parts: &[Form]) -> Expr {
        let info = NodeInfo::from_form(form);
        let params_form = parts.get(1);
        let params = params_form
            .map(|part| self.lower_params(part, FunctionPhase::Runtime))
            .unwrap_or_else(|| {
                self.error(
                    AST_EXPECTED_VECTOR,
                    "fn expects a parameter vector",
                    form.span,
                );
                Vec::new()
            });
        let index = if params_form.is_some() { 2 } else { 1 };
        let return_type = params_form
            .map(|params| self.lower_metadata_type(&params.metadata, "function return"))
            .unwrap_or_default()
            .annotation;
        let body = parts[index..]
            .iter()
            .map(|part| self.lower_expr(part))
            .collect::<Vec<_>>();
        if body.is_empty() {
            self.error(AST_WRONG_SHAPE, "fn body cannot be empty", form.span);
        }
        Expr::from_form(
            form,
            ExprKind::Fn(FnExpr {
                span: info.span,
                metadata: info.metadata,
                params,
                return_type,
                body,
            }),
        )
    }

    pub(super) fn lower_let_expr(&mut self, form: &Form, parts: &[Form]) -> Expr {
        let bindings = match parts.get(1) {
            Some(Form {
                kind: FormKind::Vector(bindings),
                ..
            }) => self.lower_bindings(bindings),
            Some(binding_form) => {
                self.error(
                    AST_EXPECTED_VECTOR,
                    "let expects a binding vector",
                    binding_form.span,
                );
                Vec::new()
            }
            None => {
                self.error(
                    AST_EXPECTED_VECTOR,
                    "let expects a binding vector",
                    form.span,
                );
                Vec::new()
            }
        };
        let body_start = usize::from(parts.get(1).is_some()) + 1;
        let body = parts
            .get(body_start..)
            .unwrap_or_default()
            .iter()
            .map(|part| self.lower_expr(part))
            .collect::<Vec<_>>();
        if body.is_empty() {
            self.error(AST_WRONG_SHAPE, "let body cannot be empty", form.span);
        }
        Expr::from_form(form, ExprKind::Let { bindings, body })
    }

    pub(super) fn lower_bindings(&mut self, forms: &[Form]) -> Vec<Binding> {
        if forms.len() % 2 != 0 {
            self.error(
                AST_EXPECTED_PAIR,
                "let bindings require pattern/value pairs",
                forms.last().map_or(Span::default(), |form| form.span),
            );
        }
        forms
            .chunks(2)
            .filter_map(|pair| {
                let pattern_form = pair.first()?;
                let value_form = pair.get(1)?;
                let value = self.lower_expr(value_form);
                let metadata_type =
                    self.lower_metadata_type(&pattern_form.metadata, "local binding");
                Some(Binding {
                    span: pattern_form.span.cover(value_form.span),
                    metadata: pattern_form.metadata.clone(),
                    pattern: self.lower_pattern(pattern_form),
                    type_annotation: metadata_type.annotation,
                    value,
                })
            })
            .collect()
    }

    pub(super) fn lower_if_expr(&mut self, form: &Form, parts: &[Form]) -> Expr {
        if !(3..=4).contains(&parts.len()) {
            self.error(
                AST_WRONG_SHAPE,
                "if expects condition, then branch, and optional else branch",
                form.span,
            );
        }
        let condition = parts
            .get(1)
            .map(|part| self.lower_expr(part))
            .unwrap_or_else(|| self.error_expr(form.span, "missing if condition"));
        let then_branch = parts
            .get(2)
            .map(|part| self.lower_expr(part))
            .unwrap_or_else(|| self.error_expr(form.span, "missing if then branch"));
        let else_branch = parts.get(3).map(|part| Box::new(self.lower_expr(part)));
        Expr::from_form(
            form,
            ExprKind::If {
                condition: Box::new(condition),
                then_branch: Box::new(then_branch),
                else_branch,
            },
        )
    }

    pub(super) fn lower_try_expr(&mut self, form: &Form, parts: &[Form]) -> Expr {
        let info = NodeInfo::from_form(form);
        let mut body = Vec::new();
        let mut catches = Vec::new();
        let mut finally_body = None;
        let mut saw_finally = false;
        for part in parts.iter().skip(1) {
            let Some(clause) = list_parts(part) else {
                body.push(self.lower_expr(part));
                continue;
            };
            let Some(kind) = clause.first().and_then(symbol_name) else {
                body.push(self.lower_expr(part));
                continue;
            };
            match kind.canonical.as_str() {
                "catch" => {
                    if saw_finally {
                        self.error(
                            AST_WRONG_SHAPE,
                            "catch clauses must appear before finally",
                            part.span,
                        );
                    }
                    catches.push(self.lower_catch(part, clause));
                }
                "finally" => {
                    if saw_finally {
                        self.error(
                            AST_WRONG_SHAPE,
                            "try accepts at most one finally clause",
                            part.span,
                        );
                        continue;
                    }
                    saw_finally = true;
                    let body = clause[1..]
                        .iter()
                        .map(|form| self.lower_expr(form))
                        .collect::<Vec<_>>();
                    if body.is_empty() {
                        self.error(AST_WRONG_SHAPE, "finally body cannot be empty", part.span);
                    }
                    finally_body = Some(body);
                }
                _ => body.push(self.lower_expr(part)),
            }
        }
        if body.is_empty() {
            self.error(AST_WRONG_SHAPE, "try body cannot be empty", form.span);
        }
        Expr::from_form(
            form,
            ExprKind::Try(TryExpr {
                span: info.span,
                metadata: info.metadata,
                body,
                catches,
                finally_body,
            }),
        )
    }

    pub(super) fn lower_catch(&mut self, form: &Form, clause: &[Form]) -> CatchClause {
        let info = NodeInfo::from_form(form);
        if clause.len() < 3 {
            self.error(
                AST_WRONG_SHAPE,
                "catch expects exception type, binding, and body",
                form.span,
            );
        }
        let exception_type = clause.get(1).map(|part| self.lower_type(part));
        let binding = clause.get(2).and_then(|part| {
            self.require_name(part, "catch binding").map(|name| Param {
                span: part.span,
                metadata: part.metadata.clone(),
                name,
                pattern: None,
                type_annotation: None,
                default: None,
                variadic: false,
            })
        });
        if clause.get(2).is_some() && binding.is_none() {
            self.error(
                AST_INVALID_NAME,
                "catch binding must be a symbol",
                clause[2].span,
            );
        }
        // Keep malformed catch clauses recoverable.  The shape diagnostic
        // above is sufficient; never panic while slicing a short clause.
        let body = clause
            .get(3..)
            .unwrap_or(&[])
            .iter()
            .map(|part| self.lower_expr(part))
            .collect::<Vec<_>>();
        if body.is_empty() {
            self.error(AST_WRONG_SHAPE, "catch body cannot be empty", form.span);
        }
        CatchClause {
            span: info.span,
            metadata: info.metadata,
            exception_type,
            binding,
            body,
        }
    }

    pub(super) fn lower_raise_expr(&mut self, form: &Form, parts: &[Form]) -> Expr {
        if parts.len() > 2 {
            self.error(
                AST_WRONG_SHAPE,
                "raise accepts at most one expression",
                form.span,
            );
        }
        Expr::from_form(
            form,
            ExprKind::Raise(parts.get(1).map(|part| Box::new(self.lower_expr(part)))),
        )
    }
}
