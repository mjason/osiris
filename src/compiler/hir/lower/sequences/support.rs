use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn specialize_temporal_summary(
        &self,
        summary: &crate::types::TemporalSummary,
        callable: &CallableInfo,
        source_call: &ast::CallExpr,
        arguments: &[CallArgument],
    ) -> crate::types::TemporalSummary {
        let mut substitutions = BTreeMap::new();
        let mut positional_index = 0_usize;
        for (source, lowered) in source_call.args.iter().zip(arguments) {
            let parameter = match (source, lowered) {
                (AstCallArg::Positional(_), CallArgument::Positional(_)) => {
                    let parameter = callable.parameters.get(positional_index).or_else(|| {
                        callable
                            .parameters
                            .last()
                            .filter(|parameter| parameter.variadic)
                    });
                    if parameter.is_some_and(|parameter| !parameter.variadic) {
                        positional_index += 1;
                    }
                    parameter
                }
                (AstCallArg::Keyword(keyword), CallArgument::Keyword { .. }) => {
                    let source_name = keyword.key.canonical.trim_start_matches(':');
                    callable
                        .parameters
                        .iter()
                        .find(|parameter| parameter.accepted_names.contains(source_name))
                }
                _ => None,
            };
            let value = match lowered {
                CallArgument::Positional(value) | CallArgument::Keyword { value, .. } => value,
            };
            if let (Some(parameter), Some(value)) =
                (parameter, self.symbolic_argument_expression(value))
            {
                substitutions.insert(parameter.canonical.clone(), value);
            }
        }
        summary.substitute(&substitutions)
    }

    pub(in crate::hir) fn lower_sequence_callback(
        &mut self,
        expression: &ast::Expr,
        _span: Span,
        scope: &mut Scope,
        expected: &[Type],
    ) -> Expr {
        let value = match &expression.kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                expression.span,
                scope,
                expected,
            ),
            _ => self.lower_expr(expression, scope),
        };
        if let Type::Fn(signature) = &value.ty {
            if signature.parameters.len() != expected.len() {
                self.error(
                    "OSR-T0041",
                    format!(
                        "sequence callback expects {} argument(s), found {}",
                        expected.len(),
                        signature.parameters.len()
                    ),
                    value.span,
                );
            }
            for (actual, parameter) in expected.iter().zip(&signature.parameters) {
                self.check_assignable(actual, parameter, value.span);
            }
        } else if !matches!(value.ty, Type::Any | Type::Unknown | Type::Error) {
            self.error(
                "OSR-T0041",
                "sequence callback must be a function",
                value.span,
            );
        }
        value
    }

    pub(in crate::hir) fn symbolic_argument_expression(&self, expression: &Expr) -> Option<String> {
        match &expression.kind {
            ExprKind::Integer(value) => Some(value.clone()),
            ExprKind::Binding(binding) => self
                .bindings
                .get(binding)
                .map(|binding| binding.name.canonical.clone()),
            _ => None,
        }
    }
}
