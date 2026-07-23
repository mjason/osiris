use super::super::super::*;

impl<'a> Lowerer<'a> {
    /// Lower the macro-generated `loop*` primitive. Runtime iteration uses a
    /// Python `while` loop and therefore does not grow the Python stack.
    pub(in crate::hir) fn lower_loop(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        if !call.keywords.is_empty() || call.positional.is_empty() {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0022",
                "osiris.prelude/loop* expects a callback and zero or more initial values",
                span,
            );
            return Expr::error(span);
        }

        let initials = call.positional[1..]
            .iter()
            .map(|initial| self.lower_expr(initial, scope))
            .collect::<Vec<_>>();
        let expected = initials
            .iter()
            .map(|initial| initial.ty.clone())
            .collect::<Vec<_>>();

        // `recur*` is only meaningful while lowering this callback.  Keep the
        // arity stack lexical so nested loops validate against their nearest
        // state vector.
        let callback_is_inline = matches!(&call.positional[0].kind, AstExprKind::Fn(_));
        self.loop_arities.push(expected.len());
        self.loop_state_types.push(expected.clone());
        self.loop_callback_depths
            .push(self.function_depth + usize::from(callback_is_inline));
        let function = match &call.positional[0].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[0].span,
                scope,
                &expected,
            ),
            _ => self.lower_expr(&call.positional[0], scope),
        };
        if let ExprKind::Lambda { body, .. } = &function.kind {
            self.validate_recur_tail(body, true);
        }
        self.loop_arities.pop();
        self.loop_state_types.pop();
        self.loop_callback_depths.pop();

        let Type::Fn(signature) = &function.ty else {
            self.error(
                "OSR-T0022",
                "osiris.prelude/loop* callback must be a function",
                function.span,
            );
            return Expr::error(span);
        };
        if signature.parameters.len() != expected.len() {
            self.error(
                "OSR-T0022",
                format!(
                    "loop callback expects {} state value(s), found {}",
                    expected.len(),
                    signature.parameters.len()
                ),
                function.span,
            );
            return Expr::error(span);
        }
        for (actual, expected) in expected.iter().zip(&signature.parameters) {
            self.check_assignable(actual, expected, function.span);
        }
        let callback_summaries = signature.summaries.clone();
        let result_type = self.types.resolve(&signature.return_type);
        let mut summaries = initials
            .iter()
            .fold(function.summaries.clone(), |summary, initial| {
                summary.join(&initial.summaries)
            })
            .join(&callback_summaries);
        summaries.data = DataProperties::scalar();

        let binding = self.ensure_core_loop_binding(span);
        let mut parameter_types = vec![function.ty.clone()];
        parameter_types.extend(initials.iter().map(|initial| initial.ty.clone()));
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(parameter_types, result_type.clone())
                    .with_summaries(callback_summaries),
            ),
            ExprKind::Binding(binding),
        );
        let mut arguments = vec![CallArgument::Positional(function)];
        arguments.extend(initials.into_iter().map(CallArgument::Positional));
        Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments,
            },
        }
    }

    /// Turn a function-local `recur` body into the same state-machine shape as
    /// an explicit `loop`.  The callback deliberately reuses the parameter
    /// binding ids from the surrounding function: the body was lowered against
    /// those ids already, and the backend will emit a readable local helper
    /// whose parameters shadow the outer function parameters for each step.
    pub(in crate::hir) fn wrap_function_recur(
        &mut self,
        parameters: &[Parameter],
        body: Expr,
        span: Span,
    ) -> Expr {
        let state_types = parameters
            .iter()
            .map(|parameter| self.types.resolve(&parameter.ty))
            .collect::<Vec<_>>();
        let callback_parameters = parameters
            .iter()
            .zip(&state_types)
            .map(|(parameter, ty)| Parameter {
                binding: parameter.binding.clone(),
                ty: ty.clone(),
                default: None,
                // A variadic outer parameter is represented by one tuple/list
                // state value in the loop.  The callback therefore receives a
                // fixed arity state vector and keeps the original binding's
                // value unchanged.
                variadic: false,
            })
            .collect::<Vec<_>>();
        let body_type = body.ty.clone();
        let body_summaries = body.summaries.clone();
        let callback_type = Type::Fn(
            FunctionType::new(state_types.clone(), body_type.clone())
                .with_summaries(body_summaries.clone()),
        );
        let callback = Expr {
            span,
            ty: callback_type.clone(),
            summaries: body_summaries.clone(),
            kind: ExprKind::Lambda {
                parameters: callback_parameters,
                body: Box::new(body),
            },
        };
        let initials = parameters
            .iter()
            .zip(&state_types)
            .map(|(parameter, ty)| {
                Expr::pure(
                    span,
                    ty.clone(),
                    ExprKind::Binding(parameter.binding.clone()),
                )
            })
            .collect::<Vec<_>>();
        let binding = self.ensure_core_loop_binding(span);
        let mut callee_parameters = vec![callback_type];
        callee_parameters.extend(state_types);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(callee_parameters, body_type.clone())
                    .with_summaries(body_summaries.clone()),
            ),
            ExprKind::Binding(binding),
        );
        let mut arguments = vec![CallArgument::Positional(callback)];
        arguments.extend(initials.into_iter().map(CallArgument::Positional));
        Expr {
            span,
            ty: body_type,
            summaries: body_summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments,
            },
        }
    }

    /// `recur` is a tail-only transfer to the owning loop.  The ordinary
    /// expression lowerer intentionally keeps the core AST small, so perform
    /// this structural check on the already typed callback body. Nested
    /// lambdas are skipped here; their function-depth check in `lower_recur`
    /// diagnoses attempts to capture an outer loop, while nested loops validate
    /// their own callbacks when they are lowered.
    pub(in crate::hir) fn validate_recur_tail(&mut self, expression: &Expr, tail: bool) {
        match &expression.kind {
            ExprKind::Call { callee, arguments } => {
                let is_recur = self
                    .core_recur_binding
                    .as_ref()
                    .is_some_and(|binding| {
                        matches!(&callee.kind, ExprKind::Binding(candidate) if candidate == binding)
                    });
                if is_recur && !tail {
                    self.error(
                        "OSR-T0023",
                        "recur must appear in tail position",
                        expression.span,
                    );
                }
                self.validate_recur_tail(callee, false);
                for argument in arguments {
                    let value = match argument {
                        CallArgument::Positional(value) | CallArgument::Keyword { value, .. } => {
                            value
                        }
                    };
                    self.validate_recur_tail(value, false);
                }
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.validate_recur_tail(condition, false);
                self.validate_recur_tail(then_branch, tail);
                self.validate_recur_tail(else_branch, tail);
            }
            ExprKind::Let { bindings, body } => {
                for binding in bindings {
                    self.validate_recur_tail(&binding.value, false);
                }
                self.validate_recur_tail(body, tail);
            }
            ExprKind::Do(expressions) => {
                for expression in expressions.iter().take(expressions.len().saturating_sub(1)) {
                    self.validate_recur_tail(expression, false);
                }
                if let Some(last) = expressions.last() {
                    self.validate_recur_tail(last, tail);
                }
            }
            ExprKind::Try {
                body,
                catches,
                finally_body,
            } => {
                let branch_tail = tail && finally_body.is_none();
                self.validate_recur_tail(body, branch_tail);
                for catch in catches {
                    self.validate_recur_tail(&catch.body, branch_tail);
                }
                if let Some(finally_body) = finally_body {
                    self.validate_recur_tail(finally_body, false);
                }
            }
            ExprKind::Lambda { .. } => {}
            ExprKind::Operator { operands, .. } => {
                for operand in operands {
                    self.validate_recur_tail(operand, false);
                }
            }
            ExprKind::Attribute { value, .. } | ExprKind::Raise(Some(value)) => {
                self.validate_recur_tail(value, false)
            }
            ExprKind::Index { value, index } => {
                self.validate_recur_tail(value, false);
                self.validate_recur_tail(index, false);
            }
            ExprKind::List(items) | ExprKind::Vector(items) | ExprKind::Set(items) => {
                for item in items {
                    self.validate_recur_tail(item, false);
                }
            }
            ExprKind::Map(entries) => {
                for (key, value) in entries {
                    self.validate_recur_tail(key, false);
                    self.validate_recur_tail(value, false);
                }
            }
            ExprKind::Raise(None)
            | ExprKind::None
            | ExprKind::Bool(_)
            | ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::Binding(_)
            | ExprKind::Error => {}
        }
    }

    /// Lower a `recur*` token as a non-returning call.  Treating it as
    /// `Never` lets ordinary `if`/`do` inference keep the branch's value type,
    /// while the runtime implementation returns a private state token that
    /// `loop` consumes.
    pub(in crate::hir) fn lower_recur(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        let values = call
            .positional
            .iter()
            .map(|value| self.lower_expr(value, scope))
            .collect::<Vec<_>>();
        if !call.keywords.is_empty() {
            self.error("OSR-T0023", "recur does not accept keyword arguments", span);
        }
        let loop_arity = self.loop_arities.last().copied();
        let owns_loop = loop_arity.is_some()
            && self.loop_callback_depths.last().copied() == Some(self.function_depth);
        let function_context = (!owns_loop)
            .then(|| {
                self.function_recur_contexts
                    .iter()
                    .rposition(|context| context.depth == self.function_depth)
            })
            .flatten();
        let (expected_arity, expected_types, function_context) = if owns_loop {
            (
                loop_arity.expect("loop arity exists when callback is owned"),
                self.loop_state_types.last().cloned().unwrap_or_default(),
                None,
            )
        } else if let Some(index) = function_context {
            let context = &self.function_recur_contexts[index];
            (
                context.state_types.len(),
                context.state_types.clone(),
                Some(index),
            )
        } else {
            self.error(
                "OSR-T0023",
                if loop_arity.is_some() {
                    "recur may only appear in the owning loop callback"
                } else {
                    "recur may only appear inside a loop or function body"
                },
                span,
            );
            return Expr::error(span);
        };
        if let Some(index) = function_context {
            self.function_recur_contexts[index].used = true;
        }
        if values.len() != expected_arity {
            self.error(
                "OSR-T0023",
                format!(
                    "recur expects {} value(s), found {}",
                    expected_arity,
                    values.len()
                ),
                span,
            );
            return Expr::error(span);
        }
        for (value, expected) in values.iter().zip(&expected_types) {
            self.check_assignable(&value.ty, expected, value.span);
        }
        let summaries = join_summaries(values.iter().map(|value| &value.summaries));
        let binding = self.ensure_core_recur_binding(span, expected_arity);
        let callee_type = Type::Fn(
            FunctionType::new(
                values.iter().map(|value| value.ty.clone()).collect(),
                Type::Never,
            )
            .with_summaries(CallSummaries::pure_scalar()),
        );
        Expr {
            span,
            ty: Type::Never,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(Expr::pure(span, callee_type, ExprKind::Binding(binding))),
                arguments: values.into_iter().map(CallArgument::Positional).collect(),
            },
        }
    }
}
