use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_call(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        if let Some(name) = call.callee.name().map(|name| name.canonical.as_str()) {
            if let Some(lowered) = self.lower_known_call(name, call, span, scope) {
                return lowered;
            }
            let standard = self
                .resolve_named_call_binding(name, scope)
                .and_then(|id| crate::stdlib::find_by_id(&id).map(|binding| (id, binding)));
            if standard.as_ref().is_some_and(|(_, binding)| {
                binding.namespace == "osiris.math" && binding.canonical == "abs"
            }) {
                if !call.keywords.is_empty() || call.positional.len() != 1 {
                    for argument in &call.keywords {
                        let _ = self.lower_expr(&argument.value, scope);
                    }
                    self.error("OSR-T0006", "abs expects one positional operand", span);
                    return Expr::error(span);
                }
                let mut lowered = self.lower_abs(&call.positional[0], span, scope);
                self.replace_direct_call_binding(&mut lowered, standard.expect("checked").0);
                return lowered;
            }
            if let Some(mut operator) = operator_from_name(name) {
                if !call.keywords.is_empty() {
                    for argument in &call.keywords {
                        let _ = self.lower_expr(&argument.value, scope);
                    }
                    self.error(
                        "OSR-T0008",
                        "operators do not accept keyword arguments",
                        span,
                    );
                    return Expr::error(span);
                }
                operator = match (operator, call.positional.len()) {
                    (Operator::Subtract, 1) => Operator::Negate,
                    (Operator::Add, 1) => Operator::Positive,
                    (operator, _) => operator,
                };
                return self.lower_operator(operator, &call.positional, span, scope);
            }
            if name == "index"
                || standard.as_ref().is_some_and(|(_, binding)| {
                    binding.namespace == crate::stdlib::CORE_NAMESPACE && binding.canonical == "get"
                })
            {
                return self.lower_index_call(call, span, scope);
            }
        }

        self.lower_indirect_call(call, span, scope)
    }

    fn lower_known_call(
        &mut self,
        canonical: &str,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
    ) -> Option<Expr> {
        let internal = canonical.strip_prefix("osiris.kernel/");
        let standard = self
            .resolve_named_call_binding(canonical, scope)
            .and_then(|id| crate::stdlib::find_by_id(&id).map(|binding| (id, binding)));
        let source_name =
            internal.or_else(|| standard.as_ref().map(|(_, binding)| binding.canonical))?;

        if let Some(operation) = SequenceOperation::from_source_name(source_name) {
            let mut lowered = self.lower_sequence_call(call, span, scope, operation);
            if let Some((id, _)) = &standard {
                self.replace_direct_call_binding(&mut lowered, id.clone());
            }
            return Some(lowered);
        }
        if source_name == "mapv" {
            let mut lowered = self.lower_mapv(call, span, scope);
            if let Some((id, _)) = &standard {
                self.replace_direct_call_binding(&mut lowered, id.clone());
            }
            return Some(lowered);
        }
        if let Some((id, binding)) = &standard
            && binding.namespace == "osiris.concurrent"
            && binding.canonical == "pmap"
        {
            let mut lowered = self.lower_mapv(call, span, scope);
            lowered.summaries = lowered.summaries.join(&CallSummaries::unknown());
            self.replace_direct_call_binding(&mut lowered, id.clone());
            return Some(lowered);
        }
        if let Some(operation) = CollectionOperation::from_source_name(source_name) {
            let mut lowered = self.lower_map_like(call, span, scope, operation);
            if let Some((id, _)) = &standard {
                self.replace_direct_call_binding(&mut lowered, id.clone());
            }
            return Some(lowered);
        }
        let mut lowered = match source_name {
            "reduce" => Some(self.lower_reduce(call, span, scope, false)),
            "fold" => Some(self.lower_reduce(call, span, scope, true)),
            "reduced" => {
                Some(self.lower_reduced_operation(call, span, scope, ReducedOperation::Wrap))
            }
            "reduced?" => {
                Some(self.lower_reduced_operation(call, span, scope, ReducedOperation::Predicate))
            }
            "unreduced" => {
                Some(self.lower_reduced_operation(call, span, scope, ReducedOperation::Unwrap))
            }
            _ => None,
        };
        if let Some(expression) = &mut lowered {
            if let Some((id, _)) = &standard {
                self.replace_direct_call_binding(expression, id.clone());
            }
            return lowered;
        }

        if let Some((id, binding)) = &standard {
            let mut expression = match binding.canonical {
                "future-call" => self.lower_future_call(call, span, scope),
                "future-done?" => self.lower_future_predicate(call, span, scope, "future_done"),
                "future-cancelled?" => {
                    self.lower_future_predicate(call, span, scope, "future_cancelled")
                }
                "future-cancel" => self.lower_future_predicate(call, span, scope, "future_cancel"),
                "promise" => self.lower_promise(call, span, scope),
                "deliver" => self.lower_deliver(call, span, scope),
                "deref" => self.lower_deref(call, span, scope),
                "lock" => self.lower_lock(call, span, scope),
                _ => return None,
            };
            self.replace_direct_call_binding(&mut expression, id.clone());
            return Some(expression);
        }

        let intrinsic = internal?;
        Some(match intrinsic {
            "assert*" => self.lower_assert(call, span, scope),
            "truthy*" => self.lower_control_intrinsic(call, span, scope, ControlIntrinsic::Truthy),
            "nil*" => self.lower_control_intrinsic(call, span, scope, ControlIntrinsic::Nil),
            "present*" => {
                self.lower_control_intrinsic(call, span, scope, ControlIntrinsic::Present)
            }
            "nonempty*" => {
                self.lower_control_intrinsic(call, span, scope, ControlIntrinsic::Nonempty)
            }
            "doseq*" => self.lower_doseq(call, span, scope),
            "for-stop*" => self.lower_for_stop(call, span, scope),
            "loop*" => self.lower_loop(call, span, scope),
            "recur*" => self.lower_recur(call, span, scope),
            "trampoline*" => self.lower_trampoline(call, span, scope),
            "lazy-seq*" => self.lower_lazy_seq(call, span, scope),
            "delay*" => self.lower_delay(call, span, scope),
            "force*" => self.lower_force(call, span, scope),
            "deref*" => self.lower_deref(call, span, scope),
            "realized*" => self.lower_realized(call, span, scope),
            "future-call*" => self.lower_future_call(call, span, scope),
            "future-done*" => self.lower_future_predicate(call, span, scope, "future_done"),
            "future-cancelled*" => {
                self.lower_future_predicate(call, span, scope, "future_cancelled")
            }
            "future-cancel*" => self.lower_future_predicate(call, span, scope, "future_cancel"),
            "promise*" => self.lower_promise(call, span, scope),
            "deliver*" => self.lower_deliver(call, span, scope),
            "lock*" => self.lower_lock(call, span, scope),
            "locking*" => self.lower_locking(call, span, scope),
            "time*" => self.lower_time(call, span, scope),
            "binding*" => self.lower_dynamic_binding(call, span, scope),
            "close*" => self.lower_close(call, span, scope),
            "letfn*" => self.lower_letfn(call, span, scope),
            _ => return None,
        })
    }

    fn resolve_named_call_binding(&self, name: &str, scope: &Scope) -> Option<BindingId> {
        scope
            .resolve(name)
            .cloned()
            .or_else(|| self.resolve_global_name(name))
            .or_else(|| self.qualified_imports.get(name).cloned())
    }

    fn replace_direct_call_binding(&self, expression: &mut Expr, binding: BindingId) {
        let ExprKind::Call { callee, .. } = &mut expression.kind else {
            return;
        };
        callee.kind = ExprKind::Binding(binding);
    }

    fn lower_index_call(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if call.positional.len() != 2 || !call.keywords.is_empty() {
            self.error("OSR-T0003", "index expects two positional arguments", span);
            return Expr::error(span);
        }
        let value = self.lower_expr(&call.positional[0], scope);
        let index = self.lower_expr(&call.positional[1], scope);
        let mut summaries = value.summaries.join(&index.summaries);
        if matches!(value.ty, Type::Any | Type::Unknown) {
            summaries = summaries.join(&CallSummaries::unknown());
        }
        Expr {
            span,
            ty: indexed_type(&value.ty),
            summaries,
            kind: ExprKind::Index {
                value: Box::new(value),
                index: Box::new(index),
            },
        }
    }

    fn lower_indirect_call(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        let callee = self.lower_expr(&call.callee, scope);
        let callable = self.callable_for_expr(&callee);
        let mut arguments = Vec::new();
        let mut summaries = callee.summaries.clone();
        for argument in &call.args {
            match argument {
                AstCallArg::Positional(argument) => {
                    let argument = self.lower_expr(argument, scope);
                    summaries = summaries.join(&argument.summaries);
                    arguments.push(CallArgument::Positional(argument));
                }
                AstCallArg::Keyword(argument) => {
                    let value = self.lower_expr(&argument.value, scope);
                    summaries = summaries.join(&value.summaries);
                    let source_name = argument.key.canonical.trim_start_matches(':');
                    let emitted_name = callable
                        .as_ref()
                        .and_then(|info| {
                            info.parameters
                                .iter()
                                .find(|parameter| parameter.accepted_names.contains(source_name))
                        })
                        .map_or_else(
                            || python_identifier(source_name),
                            |parameter| python_identifier(&parameter.canonical),
                        );
                    arguments.push(CallArgument::Keyword {
                        name: emitted_name,
                        value,
                    });
                }
            }
        }

        if let Some(info) = &callable {
            self.record_contract_evidence(&info.contract_evidence);
        }
        let callback_summaries = arguments
            .iter()
            .filter_map(|argument| match argument {
                CallArgument::Positional(value) | CallArgument::Keyword { value, .. } => {
                    match &value.ty {
                        Type::Fn(function) => Some(function.summaries.clone()),
                        _ => None,
                    }
                }
            })
            .collect::<Vec<_>>();

        let (ty, latent) = match callable {
            Some(info) => {
                let info = self.instantiate_callable(&info);
                self.validate_call(&info, call, &arguments, span);
                let mut summaries = info.signature.summaries.clone();
                summaries.temporal =
                    self.specialize_temporal_summary(&summaries.temporal, &info, call, &arguments);
                ((*info.signature.return_type).clone(), summaries)
            }
            None => self.infer_indirect_call(&callee, call, &arguments, span),
        };
        let ty = self.types.resolve(&ty);
        let temporal = summaries.temporal.compose(&latent.temporal);
        summaries = summaries.join(&latent);
        summaries.temporal = temporal;
        for callback in callback_summaries {
            summaries = summaries.join(&callback);
        }
        Expr {
            span,
            ty,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments,
            },
        }
    }

    fn infer_indirect_call(
        &mut self,
        callee: &Expr,
        call: &ast::CallExpr,
        arguments: &[CallArgument],
        span: Span,
    ) -> (Type, CallSummaries) {
        match &callee.ty {
            Type::Fn(function) => {
                let positional_count = call.positional.len();
                if function.parameters.len() != call.args.len() {
                    self.error(
                        "OSR-T0004",
                        format!(
                            "call expects {} arguments, found {}",
                            function.parameters.len(),
                            call.args.len()
                        ),
                        span,
                    );
                }
                for (actual, expected) in arguments
                    .iter()
                    .filter_map(|argument| match argument {
                        CallArgument::Positional(value) => Some(&value.ty),
                        CallArgument::Keyword { .. } => None,
                    })
                    .zip(&function.parameters[..positional_count.min(function.parameters.len())])
                {
                    self.check_assignable(actual, expected, span);
                }
                ((*function.return_type).clone(), function.summaries.clone())
            }
            Type::Any => (Type::Any, CallSummaries::unknown()),
            Type::Error | Type::Unknown | Type::TypeVar(_) => {
                (Type::Error, CallSummaries::unknown())
            }
            other => {
                self.error(
                    "OSR-T0005",
                    format!("value of type `{other}` is not callable"),
                    span,
                );
                (Type::Error, CallSummaries::unknown())
            }
        }
    }
}
