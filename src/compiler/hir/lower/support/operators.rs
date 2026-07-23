use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_operator(
        &mut self,
        operator: Operator,
        operands: &[ast::Expr],
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        let operands = operands
            .iter()
            .map(|operand| self.lower_expr(operand, scope))
            .collect::<Vec<_>>();
        let summaries = join_summaries(operands.iter().map(|operand| &operand.summaries));
        match operator {
            Operator::And | Operator::Or => {
                if operands.len() < 2 {
                    self.error(
                        "OSR-T0006",
                        "boolean operator expects at least two operands",
                        span,
                    );
                }
                for operand in &operands {
                    self.check_assignable(&operand.ty, &Type::Bool, operand.span);
                }
                Expr {
                    span,
                    ty: Type::Bool,
                    summaries,
                    kind: ExprKind::Operator { operator, operands },
                }
            }
            Operator::Not => {
                if operands.len() != 1 {
                    self.error("OSR-T0006", "not expects one operand", span);
                }
                if let Some(operand) = operands.first() {
                    self.check_assignable(&operand.ty, &Type::Bool, operand.span);
                }
                Expr {
                    span,
                    ty: Type::Bool,
                    summaries,
                    kind: ExprKind::Operator { operator, operands },
                }
            }
            _ => self.lower_scalar_operator(operator, operands, summaries, span),
        }
    }

    pub(in crate::hir) fn lower_scalar_operator(
        &mut self,
        operator: Operator,
        operands: Vec<Expr>,
        summaries: CallSummaries,
        span: Span,
    ) -> Expr {
        let Some(scalar) = operator.scalar() else {
            return Expr::error(span);
        };
        let unary = matches!(operator, Operator::Negate | Operator::Positive);
        let comparison = matches!(
            operator,
            Operator::Equal
                | Operator::NotEqual
                | Operator::Less
                | Operator::LessEqual
                | Operator::Greater
                | Operator::GreaterEqual
        );
        if (unary && operands.len() != 1) || (!unary && operands.len() < 2) {
            self.error("OSR-T0006", "operator has invalid arity", span);
            return Expr::error(span);
        }

        if unary {
            let selection = self.select_operator(scalar, std::slice::from_ref(&operands[0].ty));
            return match selection {
                OperatorSelection::Selected(choice) => {
                    self.apply_operator_choice(operator, operands, span, *choice)
                }
                OperatorSelection::Ambiguous => {
                    self.error(
                        "OSR-T0008",
                        format!(
                            "operator `{}` has multiple static implementations",
                            scalar.stable_name()
                        ),
                        span,
                    );
                    Expr::error(span)
                }
                OperatorSelection::None => {
                    self.error(
                        "OSR-T0007",
                        format!("operator is not defined for `{}`", operands[0].ty),
                        span,
                    );
                    Expr::error(span)
                }
            };
        }

        let mut current_type = operands[0].ty.clone();
        let mut choices = Vec::with_capacity(operands.len() - 1);
        for operand in &operands[1..] {
            let pair = [current_type.clone(), operand.ty.clone()];
            let selection = self.select_operator(scalar, &pair);
            let choice = match selection {
                OperatorSelection::Selected(choice) => *choice,
                OperatorSelection::Ambiguous => {
                    self.error(
                        "OSR-T0008",
                        format!(
                            "operator `{}` has multiple static implementations",
                            scalar.stable_name()
                        ),
                        span,
                    );
                    return Expr::error(span);
                }
                OperatorSelection::None => {
                    self.error(
                        "OSR-T0007",
                        format!(
                            "operator is not defined for `{}` and `{}`",
                            current_type, operand.ty
                        ),
                        span,
                    );
                    return Expr::error(span);
                }
            };
            current_type = if comparison {
                operand.ty.clone()
            } else {
                choice.result.clone()
            };
            choices.push(choice);
        }

        // Keep the compact n-ary core operator representation when no
        // extension capability was selected.  Imported instances are calls to
        // their declared binding, lowered left-to-right so every operand is
        // evaluated once and each selected summary is retained.
        if choices.iter().all(|choice| choice.binding.is_none()) {
            return Expr {
                span,
                ty: if comparison { Type::Bool } else { current_type },
                summaries,
                kind: ExprKind::Operator { operator, operands },
            };
        }

        let mut operand_iter = operands.into_iter();
        let mut current = operand_iter.next().unwrap_or_else(|| Expr::error(span));
        for (choice, operand) in choices.into_iter().zip(operand_iter) {
            current = self.apply_operator_choice(operator, vec![current, operand], span, choice);
        }
        current
    }

    pub(in crate::hir) fn select_operator(
        &self,
        operator: ScalarOperator,
        operands: &[Type],
    ) -> OperatorSelection {
        let mut imported = self
            .operator_instances
            .values()
            .filter(|instance| instance.operator == operator)
            .filter_map(|instance| {
                if instance.operands.len() != operands.len()
                    || operands.iter().any(is_dynamic_operator_type)
                {
                    return None;
                }
                let mut variables = BTreeMap::new();
                if !operands
                    .iter()
                    .zip(&instance.operands)
                    .all(|(actual, expected)| {
                        operator_type_matches(&self.types, actual, expected, &mut variables)
                    })
                {
                    return None;
                }
                if contains_unresolved_operator_variable(&instance.result, &variables) {
                    return None;
                }
                let result = replace_type_variables(&instance.result, &variables);
                Some(OperatorChoice {
                    result,
                    summaries: instance.summaries.clone(),
                    binding: Some(BindingId::from_interface(instance.binding.clone())),
                    contract_evidence: self
                        .operator_contract_evidence
                        .get(&instance.id)
                        .cloned()
                        .unwrap_or_default(),
                })
            })
            .collect::<Vec<_>>();
        if imported.len() > 1 {
            return OperatorSelection::Ambiguous;
        }
        if let Some(choice) = imported.pop() {
            return OperatorSelection::Selected(Box::new(choice));
        }

        let signatures = scalar_operator_signatures(operator);
        select_operator_signature(&self.types, &signatures, operands).map_or(
            OperatorSelection::None,
            |signature| {
                OperatorSelection::Selected(Box::new(OperatorChoice {
                    result: signature.result.clone(),
                    summaries: signature.summaries.clone(),
                    binding: None,
                    contract_evidence: ContractEvidence::default(),
                }))
            },
        )
    }

    pub(in crate::hir) fn apply_operator_choice(
        &mut self,
        operator: Operator,
        operands: Vec<Expr>,
        span: Span,
        choice: OperatorChoice,
    ) -> Expr {
        self.record_contract_evidence(&choice.contract_evidence);
        let summaries = join_summaries(operands.iter().map(|operand| &operand.summaries))
            .join(&choice.summaries);
        if let Some(binding) = choice.binding {
            let callee_type = Type::Fn(
                FunctionType::new(
                    operands.iter().map(|operand| operand.ty.clone()).collect(),
                    choice.result.clone(),
                )
                .with_summaries(choice.summaries.clone()),
            );
            let callee = Expr::pure(span, callee_type, ExprKind::Binding(binding));
            return Expr {
                span,
                ty: choice.result,
                summaries,
                kind: ExprKind::Call {
                    callee: Box::new(callee),
                    arguments: operands.into_iter().map(CallArgument::Positional).collect(),
                },
            };
        }
        Expr {
            span,
            ty: choice.result,
            summaries,
            kind: ExprKind::Operator { operator, operands },
        }
    }
}
