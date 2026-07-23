use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_lambda(
        &mut self,
        function: &ast::FnExpr,
        span: Span,
        outer: &mut Scope,
    ) -> Expr {
        self.lower_lambda_with_expected_parameters(function, span, outer, &[])
    }

    pub(in crate::hir) fn lower_lambda_with_expected_parameters(
        &mut self,
        function: &ast::FnExpr,
        span: Span,
        outer: &mut Scope,
        expected: &[Type],
    ) -> Expr {
        self.function_depth += 1;
        outer.push();
        let mut parameters = Vec::new();
        let mut parameter_bindings = Vec::new();
        for (index, parameter) in function.params.iter().enumerate() {
            let ty = if let Some(annotation) = &parameter.type_annotation {
                self.resolve_type_expr(annotation)
            } else {
                expected
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| self.types.fresh_var())
            };
            if parameter.type_annotation.is_some()
                && let Some(expected) = expected.get(index)
            {
                self.check_assignable(expected, &ty, parameter.span);
            }
            let default = parameter
                .default
                .as_ref()
                .map(|value| self.lower_expr(value, outer));
            let binding = self.declare_local(
                &parameter.name,
                BindingKind::Parameter,
                ty.clone(),
                parameter.metadata.clone(),
                parameter.span,
                outer,
            );
            parameters.push(Parameter {
                binding: binding.clone(),
                ty,
                default,
                variadic: parameter.variadic,
            });
            if let Some(pattern) = &parameter.pattern {
                let value = Expr::pure(
                    parameter.span,
                    self.binding_type(&binding),
                    ExprKind::Binding(binding),
                );
                self.lower_pattern_bindings(
                    pattern,
                    value,
                    &pattern.metadata,
                    outer,
                    &mut parameter_bindings,
                );
            }
        }
        let state_types = parameters
            .iter()
            .map(|parameter| parameter.ty.clone())
            .collect::<Vec<_>>();
        self.function_recur_contexts.push(FunctionRecurContext {
            depth: self.function_depth,
            state_types,
            used: false,
        });
        let body = self.lower_body(&function.body, outer, span);
        let function_recur = self
            .function_recur_contexts
            .pop()
            .expect("lambda recur context");
        let body = self.wrap_let_bindings(parameter_bindings, body, span);
        let body = if function_recur.used {
            self.validate_recur_tail(&body, true);
            self.wrap_function_recur(&parameters, body, span)
        } else {
            body
        };
        outer.pop();
        self.function_depth = self.function_depth.saturating_sub(1);
        for parameter in &mut parameters {
            parameter.ty = self.types.resolve(&parameter.ty);
        }
        let return_type = function.return_type.as_ref().map_or_else(
            || body.ty.clone(),
            |annotation| {
                let annotation = self.resolve_type_expr(annotation);
                self.check_assignable(&body.ty, &annotation, body.span);
                annotation
            },
        );
        let function_type = Type::Fn(
            FunctionType::new(
                parameters
                    .iter()
                    .map(|parameter| parameter.ty.clone())
                    .collect(),
                return_type,
            )
            .with_summaries(body.summaries.clone()),
        );
        Expr::pure(
            span,
            function_type,
            ExprKind::Lambda {
                parameters,
                body: Box::new(body),
            },
        )
    }
}
