use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn callable_for_expr(&self, expression: &Expr) -> Option<CallableInfo> {
        if let ExprKind::Binding(binding) = &expression.kind {
            if let Some(callable) = self.callables.get(binding) {
                return Some(callable.clone());
            }
        }
        let ExprKind::Lambda { parameters, .. } = &expression.kind else {
            return None;
        };
        let Type::Fn(signature) = &expression.ty else {
            return None;
        };
        let parameters = parameters
            .iter()
            .map(|parameter| {
                let binding = self.bindings.get(&parameter.binding)?;
                Some(CallableParameter {
                    canonical: binding.name.canonical.clone(),
                    accepted_names: parameter_names(
                        &Name {
                            spelling: binding.source_spelling.clone(),
                            canonical: binding.name.canonical.clone(),
                        },
                        &binding.metadata,
                    ),
                    ty: parameter.ty.clone(),
                    required: parameter.default.is_none() && !parameter.variadic,
                    variadic: parameter.variadic,
                    span: binding.name.span,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(CallableInfo {
            signature: signature.clone(),
            parameters,
            generic_variables: Vec::new(),
            contract_evidence: ContractEvidence::default(),
        })
    }

    pub(in crate::hir) fn instantiate_callable(&mut self, callable: &CallableInfo) -> CallableInfo {
        if callable.generic_variables.is_empty() {
            return callable.clone();
        }
        let substitutions = callable
            .generic_variables
            .iter()
            .copied()
            .map(|variable| (variable, self.types.fresh_var()))
            .collect::<BTreeMap<_, _>>();
        let mut instantiated = callable.clone();
        instantiated.signature.parameters = instantiated
            .signature
            .parameters
            .iter()
            .map(|ty| replace_type_variables(ty, &substitutions))
            .collect();
        instantiated.signature.return_type = Box::new(replace_type_variables(
            &instantiated.signature.return_type,
            &substitutions,
        ));
        for parameter in &mut instantiated.parameters {
            parameter.ty = replace_type_variables(&parameter.ty, &substitutions);
        }
        instantiated.generic_variables.clear();
        instantiated
    }

    pub(in crate::hir) fn validate_call(
        &mut self,
        callable: &CallableInfo,
        source_call: &ast::CallExpr,
        arguments: &[CallArgument],
        span: Span,
    ) {
        let mut assigned = vec![false; callable.parameters.len()];
        let mut positional_index = 0_usize;
        let mut saw_keyword = false;
        for (source, lowered) in source_call.args.iter().zip(arguments) {
            match (source, lowered) {
                (AstCallArg::Positional(_), CallArgument::Positional(value)) => {
                    if saw_keyword {
                        self.error(
                            "OSR-T0012",
                            "a positional argument cannot follow a keyword argument",
                            value.span,
                        );
                    }
                    let parameter_index = positional_index;
                    positional_index += 1;
                    let Some(last_parameter) = callable.parameters.last() else {
                        self.error("OSR-T0004", "too many positional arguments", value.span);
                        continue;
                    };
                    if parameter_index >= callable.parameters.len() && !last_parameter.variadic {
                        self.error("OSR-T0004", "too many positional arguments", value.span);
                        continue;
                    }
                    let actual_parameter_index = if parameter_index >= callable.parameters.len()
                        || callable.parameters[parameter_index].variadic
                    {
                        callable.parameters.len() - 1
                    } else {
                        parameter_index
                    };
                    let parameter = &callable.parameters[actual_parameter_index];
                    if !parameter.variadic {
                        if assigned[actual_parameter_index] {
                            self.error(
                                "OSR-T0009",
                                format!(
                                    "argument for `{}` was supplied more than once",
                                    parameter.canonical
                                ),
                                value.span,
                            );
                        }
                        assigned[actual_parameter_index] = true;
                    }
                    self.check_assignable(&value.ty, &parameter.ty, value.span);
                }
                (AstCallArg::Keyword(source_keyword), CallArgument::Keyword { value, .. }) => {
                    saw_keyword = true;
                    let source_name = source_keyword.key.canonical.trim_start_matches(':');
                    let Some((parameter_index, parameter)) = callable
                        .parameters
                        .iter()
                        .enumerate()
                        .find(|(_, parameter)| parameter.accepted_names.contains(source_name))
                    else {
                        self.error(
                            "OSR-T0008",
                            format!("unknown keyword argument `:{source_name}`"),
                            source_keyword.span,
                        );
                        continue;
                    };
                    if parameter.variadic {
                        self.error(
                            "OSR-T0008",
                            format!(
                                "variadic parameter `{}` cannot be passed by keyword",
                                parameter.canonical
                            ),
                            source_keyword.span,
                        );
                        continue;
                    }
                    if assigned[parameter_index] {
                        self.error(
                            "OSR-T0009",
                            format!(
                                "argument for `{}` was supplied more than once",
                                parameter.canonical
                            ),
                            source_keyword.span,
                        );
                    }
                    assigned[parameter_index] = true;
                    self.check_assignable(&value.ty, &parameter.ty, value.span);
                }
                _ => {
                    self.error("OSR-H0005", "internal call lowering mismatch", span);
                }
            }
        }
        for (index, parameter) in callable.parameters.iter().enumerate() {
            if parameter.required && !assigned[index] {
                self.error(
                    "OSR-T0010",
                    format!("missing required argument `{}`", parameter.canonical),
                    parameter.span,
                );
            }
        }
    }

    pub(in crate::hir) fn require_pure(&mut self, expression: &Expr, context: &str) {
        if !expression.summaries.effects.effects.is_empty() || expression.summaries.effects.open {
            self.error(
                "OSR-T0013",
                format!("{context} must be pure"),
                expression.span,
            );
        }
    }

    pub(in crate::hir) fn record_contract_evidence(&mut self, evidence: &ContractEvidence) {
        if let Some(current) = self.contract_evidence_stack.last_mut() {
            *current = current.join(evidence);
        }
    }

    pub(in crate::hir) fn validate_causal_function(
        &mut self,
        name: &str,
        summaries: &CallSummaries,
        evidence: &ContractEvidence,
        requirement: &CausalRequirement,
        span: Span,
    ) {
        for fact in evidence.unverified() {
            let identity = fact.contract_id.as_deref().unwrap_or(&fact.binding);
            self.error(
                "OSR-C0001",
                format!(
                    "causal function `{name}` depends on untrusted declared contract `{identity}` from `{}`",
                    fact.provider_module
                ),
                span,
            );
        }
        match summaries.temporal.future {
            TemporalBound::Finite(0) => {}
            TemporalBound::Finite(value) => self.error(
                "OSR-C0002",
                format!("causal function `{name}` reads {value} step(s) into the future"),
                span,
            ),
            _ => self.error(
                "OSR-C0002",
                format!("causal function `{name}` has an unproved future bound"),
                span,
            ),
        }
        match &summaries.temporal.availability {
            Availability::Immediate => {}
            Availability::Named(actual)
                if requirement.decision_point.as_deref() == Some(actual.as_str()) => {}
            Availability::Named(actual) => self.error(
                "OSR-C0003",
                format!(
                    "causal function `{name}` cannot prove availability `{actual}` at its decision point"
                ),
                span,
            ),
            Availability::Unknown => self.error(
                "OSR-C0003",
                format!("causal function `{name}` has unknown data availability"),
                span,
            ),
        }
    }
}
