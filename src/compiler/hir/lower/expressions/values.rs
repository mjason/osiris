use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_expr(&mut self, expression: &ast::Expr, scope: &mut Scope) -> Expr {
        match &expression.kind {
            AstExprKind::None => Expr::pure(expression.span, Type::None, ExprKind::None),
            AstExprKind::Bool(value) => {
                Expr::pure(expression.span, Type::Bool, ExprKind::Bool(*value))
            }
            AstExprKind::Integer(value) => {
                Expr::pure(expression.span, Type::Int, ExprKind::Integer(value.clone()))
            }
            AstExprKind::Float(value) => {
                Expr::pure(expression.span, Type::Float, ExprKind::Float(value.clone()))
            }
            AstExprKind::String(value) => {
                Expr::pure(expression.span, Type::Str, ExprKind::String(value.clone()))
            }
            AstExprKind::Keyword(name) => Expr::pure(
                expression.span,
                Type::Str,
                ExprKind::String(name.canonical.trim_start_matches(':').to_owned()),
            ),
            AstExprKind::Name(name) => self.lower_name(name, expression.span, scope),
            AstExprKind::List(items) => self.lower_sequence(items, expression.span, scope, true),
            AstExprKind::Vector(items) => self.lower_sequence(items, expression.span, scope, false),
            AstExprKind::Map(entries) => {
                let entries = entries
                    .iter()
                    .map(|(key, value)| {
                        (self.lower_expr(key, scope), self.lower_expr(value, scope))
                    })
                    .collect::<Vec<_>>();
                let key_type = self.types.join_all(entries.iter().map(|(key, _)| &key.ty));
                let value_type = self
                    .types
                    .join_all(entries.iter().map(|(_, value)| &value.ty));
                let summaries = join_summaries(
                    entries
                        .iter()
                        .flat_map(|(key, value)| [&key.summaries, &value.summaries]),
                );
                Expr {
                    span: expression.span,
                    ty: Type::Map(Box::new(key_type), Box::new(value_type)),
                    summaries,
                    kind: ExprKind::Map(entries),
                }
            }
            AstExprKind::Set(items) => {
                let items = items
                    .iter()
                    .map(|item| self.lower_expr(item, scope))
                    .collect::<Vec<_>>();
                let ty = self.types.join_all(items.iter().map(|item| &item.ty));
                let summaries = join_summaries(items.iter().map(|item| &item.summaries));
                Expr {
                    span: expression.span,
                    ty: Type::Set(Box::new(ty)),
                    summaries,
                    kind: ExprKind::Set(items),
                }
            }
            AstExprKind::Call(call) => self.lower_call(call, expression.span, scope),
            AstExprKind::Let { bindings, body } => {
                scope.push();
                let mut lowered = Vec::new();
                for binding in bindings {
                    let mut value = self.lower_expr(&binding.value, scope);
                    if let Some(annotation) = &binding.type_annotation {
                        let annotated = self.resolve_type_expr(annotation);
                        self.check_assignable(&value.ty, &annotated, binding.span);
                        value.ty = annotated;
                    }
                    self.lower_pattern_bindings(
                        &binding.pattern,
                        value,
                        &binding.metadata,
                        scope,
                        &mut lowered,
                    );
                }
                let body = self.lower_body(body, scope, expression.span);
                scope.pop();
                let summaries = lowered
                    .iter()
                    .fold(body.summaries.clone(), |summary, binding| {
                        summary.join(&binding.value.summaries)
                    });
                Expr {
                    span: expression.span,
                    ty: body.ty.clone(),
                    summaries,
                    kind: ExprKind::Let {
                        bindings: lowered,
                        body: Box::new(body),
                    },
                }
            }
            AstExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition = self.lower_expr(condition, scope);
                self.check_assignable(&condition.ty, &Type::Bool, condition.span);
                let then_branch = self.lower_expr(then_branch, scope);
                let else_branch = else_branch.as_ref().map_or_else(
                    || Expr::pure(expression.span, Type::None, ExprKind::None),
                    |branch| self.lower_expr(branch, scope),
                );
                let ty = self.types.join(&then_branch.ty, &else_branch.ty);
                let summaries = condition
                    .summaries
                    .join(&then_branch.summaries)
                    .join(&else_branch.summaries);
                Expr {
                    span: expression.span,
                    ty,
                    summaries,
                    kind: ExprKind::If {
                        condition: Box::new(condition),
                        then_branch: Box::new(then_branch),
                        else_branch: Box::new(else_branch),
                    },
                }
            }
            AstExprKind::Do(body) => self.lower_body(body, scope, expression.span),
            AstExprKind::Fn(function) => self.lower_lambda(function, expression.span, scope),
            AstExprKind::Raise(value) => {
                let value = value
                    .as_ref()
                    .map(|value| Box::new(self.lower_expr(value, scope)));
                if let Some(value) = &value
                    && !self.is_raiseable_type(&value.ty)
                {
                    self.error(
                        "OSR-T0033",
                        format!("raise expects an exception value, found `{}`", value.ty),
                        value.span,
                    );
                }
                let summaries = value
                    .as_ref()
                    .map_or_else(CallSummaries::pure_scalar, |value| value.summaries.clone());
                let summaries = CallSummaries {
                    effects: summaries
                        .effects
                        .union(&EffectRow::singleton(Effect::Throw)),
                    ..summaries
                };
                Expr {
                    span: expression.span,
                    ty: Type::Never,
                    summaries,
                    kind: ExprKind::Raise(value),
                }
            }
            AstExprKind::Try(try_expression) => {
                self.lower_try(try_expression, expression.span, scope)
            }
            AstExprKind::Quote(_)
            | AstExprKind::SyntaxQuote(_)
            | AstExprKind::Unquote(_)
            | AstExprKind::UnquoteSplicing(_) => {
                self.error(
                    "OSR-H0004",
                    "quoted syntax is only valid during macro expansion",
                    expression.span,
                );
                Expr::error(expression.span)
            }
            AstExprKind::Error(_) => Expr::error(expression.span),
        }
    }

    pub(in crate::hir) fn is_raiseable_type(&self, ty: &Type) -> bool {
        match self.types.resolve(ty) {
            Type::Any | Type::Unknown => true,
            Type::Nominal { binding, .. } => {
                python_builtin_exception_from_binding(&binding).is_some()
            }
            Type::Union(members) => members.iter().all(|member| self.is_raiseable_type(member)),
            Type::Never => true,
            _ => false,
        }
    }

    pub(in crate::hir) fn lower_sequence(
        &mut self,
        items: &[ast::Expr],
        span: Span,
        scope: &mut Scope,
        list: bool,
    ) -> Expr {
        let items = items
            .iter()
            .map(|item| self.lower_expr(item, scope))
            .collect::<Vec<_>>();
        let item_type = self.types.join_all(items.iter().map(|item| &item.ty));
        let summaries = join_summaries(items.iter().map(|item| &item.summaries));
        Expr {
            span,
            ty: if list {
                Type::List(Box::new(item_type))
            } else {
                Type::Vector(Box::new(item_type))
            },
            summaries,
            kind: if list {
                ExprKind::List(items)
            } else {
                ExprKind::Vector(items)
            },
        }
    }
}
