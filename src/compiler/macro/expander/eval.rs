use super::*;

impl Expander {
    pub(in crate::macro_expand) fn evaluate_macro(
        &mut self,
        definition: &FunctionDef,
        call: &Form,
        arguments: &[Form],
    ) -> Result<Form, EvalError> {
        let values = arguments
            .iter()
            .cloned()
            .map(Value::Data)
            .collect::<Vec<_>>();
        let mut budget = EvalBudget::default();
        let mut environment = Environment::new();
        let previous_namespace = std::mem::replace(
            &mut self.active_phase_namespace,
            definition.namespace.clone(),
        );
        let result = (|| {
            bind_parameters(
                &mut BindContext {
                    expander: self,
                    environment: &mut environment,
                    budget: &mut budget,
                    span: call.span,
                    depth: 0,
                },
                &definition.params,
                &values,
                true,
            )?;
            environment.insert("&form".to_owned(), Value::Data(call.clone()));
            self.eval_body(
                &definition.body,
                &mut environment,
                &mut budget,
                0,
                definition.span,
            )
        })();
        self.active_phase_namespace = previous_namespace;
        result?.into_data(call.span)
    }

    pub(in crate::macro_expand) fn eval(
        &mut self,
        form: &Form,
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
    ) -> Result<Value, EvalError> {
        tick_budget(budget, depth, form.span)?;
        match &form.kind {
            FormKind::Symbol(name) => {
                if let Some(value) = environment.get(&name.canonical) {
                    return Ok(value.clone());
                }
                if let Some(namespace) = &self.active_phase_namespace {
                    let scoped = scoped_phase_name(namespace, &name.canonical);
                    if self.phase_functions.contains_key(&scoped) {
                        return Ok(Value::Callable(Callable::User(scoped)));
                    }
                }
                if self.phase_functions.contains_key(&name.canonical) {
                    return Ok(Value::Callable(Callable::User(name.canonical.clone())));
                }
                if let Some(name) = builtin_name(&name.canonical) {
                    return Ok(Value::Callable(Callable::Builtin(name)));
                }
                Err(EvalError::evaluation(
                    format!("unbound phase-1 name `{}`", name.spelling),
                    form.span,
                ))
            }
            FormKind::List(items) => self.eval_list(form, items, environment, budget, depth + 1),
            FormKind::Vector(items) => {
                let items = self.eval_collection(items, environment, budget, depth + 1)?;
                Ok(Value::Data(Self::with_kind(form, FormKind::Vector(items))))
            }
            FormKind::Map(items) => {
                let items = self.eval_collection(items, environment, budget, depth + 1)?;
                Ok(Value::Data(Self::with_kind(form, FormKind::Map(items))))
            }
            FormKind::Set(items) => {
                let items = self.eval_collection(items, environment, budget, depth + 1)?;
                Ok(Value::Data(Self::with_kind(form, FormKind::Set(items))))
            }
            FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::Quote,
                form: quoted,
            } => Ok(Value::Data((**quoted).clone())),
            FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::SyntaxQuote,
                form: template,
            } => {
                let mut generated = BTreeMap::new();
                self.syntax_quote(template, environment, budget, depth + 1, &mut generated)
                    .map(Value::Data)
            }
            FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::Unquote | ReaderMacroKind::UnquoteSplicing,
                ..
            } => Err(EvalError::evaluation(
                "unquote is only valid inside syntax quote",
                form.span,
            )),
            FormKind::Error(message) => Err(EvalError::evaluation(
                format!("cannot evaluate recovered syntax error: {message}"),
                form.span,
            )),
            _ => Ok(Value::Data(form.clone())),
        }
    }

    pub(in crate::macro_expand) fn eval_collection(
        &mut self,
        items: &[Form],
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
    ) -> Result<Vec<Form>, EvalError> {
        let mut evaluated = Vec::with_capacity(items.len());
        for item in items {
            evaluated.push(
                self.eval(item, environment, budget, depth)?
                    .into_data(item.span)?,
            );
        }
        Ok(evaluated)
    }

    pub(in crate::macro_expand) fn eval_list(
        &mut self,
        form: &Form,
        items: &[Form],
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
    ) -> Result<Value, EvalError> {
        let Some(head) = items.first() else {
            return Ok(Value::Data(form.clone()));
        };
        match symbol_canonical(head) {
            Some("quote") => {
                require_form_arity(items, 2, "quote", form.span)?;
                return Ok(Value::Data(items[1].clone()));
            }
            Some("if") => {
                if !(3..=4).contains(&items.len()) {
                    return Err(EvalError::evaluation(
                        "`if` expects a condition, then branch, and optional else branch",
                        form.span,
                    ));
                }
                let condition = self.eval(&items[1], environment, budget, depth)?;
                if is_truthy(&condition) {
                    return self.eval(&items[2], environment, budget, depth);
                }
                return match items.get(3) {
                    Some(alternative) => self.eval(alternative, environment, budget, depth),
                    None => Ok(Value::Data(none(form.span))),
                };
            }
            Some("do") => {
                return self.eval_body(&items[1..], environment, budget, depth, form.span);
            }
            Some("let") => {
                return self.eval_let(form, items, environment, budget, depth);
            }
            Some("fn") => {
                if items.len() < 3 {
                    return Err(EvalError::evaluation(
                        "`fn` expects a parameter vector and body",
                        form.span,
                    ));
                }
                let params = parse_parameters(&items[1])
                    .map_err(|message| EvalError::evaluation(message, items[1].span))?;
                return Ok(Value::Callable(Callable::Lambda(Rc::new(Lambda {
                    params,
                    body: items[2..].to_vec(),
                    closure: environment.clone(),
                    namespace: self.active_phase_namespace.clone(),
                }))));
            }
            Some("and") => {
                let mut value = Value::Data(boolean(true, form.span));
                for expression in &items[1..] {
                    value = self.eval(expression, environment, budget, depth)?;
                    if !is_truthy(&value) {
                        break;
                    }
                }
                return Ok(value);
            }
            Some("or") => {
                for expression in &items[1..] {
                    let value = self.eval(expression, environment, budget, depth)?;
                    if is_truthy(&value) {
                        return Ok(value);
                    }
                }
                return Ok(Value::Data(none(form.span)));
            }
            Some("cond") => {
                if items.len() % 2 == 0 {
                    return Err(EvalError::evaluation(
                        "`cond` expects condition/result pairs",
                        form.span,
                    ));
                }
                for clause in items[1..].chunks_exact(2) {
                    let matches =
                        matches!(
                            &clause[0].kind,
                            FormKind::Keyword(name) if name.canonical == ":else"
                        ) || is_truthy(&self.eval(&clause[0], environment, budget, depth)?);
                    if matches {
                        return self.eval(&clause[1], environment, budget, depth);
                    }
                }
                return Ok(Value::Data(none(form.span)));
            }
            _ => {}
        }

        let callable = match self.eval(head, environment, budget, depth)? {
            Value::Callable(callable) => callable,
            Value::Data(_) | Value::Reduced(_) => {
                return Err(EvalError::evaluation(
                    "the first item in a phase-1 call must be a function",
                    head.span,
                ));
            }
        };
        let mut arguments = Vec::with_capacity(items.len().saturating_sub(1));
        for argument in &items[1..] {
            arguments.push(self.eval(argument, environment, budget, depth)?);
        }
        self.invoke_callable(callable, arguments, form.span, budget, depth)
    }

    pub(in crate::macro_expand) fn eval_let(
        &mut self,
        form: &Form,
        items: &[Form],
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
    ) -> Result<Value, EvalError> {
        if items.len() < 3 {
            return Err(EvalError::evaluation(
                "`let` expects a binding vector and body",
                form.span,
            ));
        }
        let FormKind::Vector(bindings) = &items[1].kind else {
            return Err(EvalError::evaluation(
                "`let` bindings must be a vector",
                items[1].span,
            ));
        };
        if bindings.len() % 2 != 0 {
            return Err(EvalError::evaluation(
                "`let` bindings must contain pattern/value pairs",
                items[1].span,
            ));
        }
        let mut local = environment.clone();
        for binding in bindings.chunks_exact(2) {
            let pattern = parse_pattern(&binding[0])
                .map_err(|message| EvalError::evaluation(message, binding[0].span))?;
            let value = self.eval(&binding[1], &mut local, budget, depth)?;
            bind_pattern(
                &mut BindContext {
                    expander: self,
                    environment: &mut local,
                    budget,
                    span: binding[0].span,
                    depth: depth + 1,
                },
                &pattern,
                value,
            )?;
        }
        self.eval_body(&items[2..], &mut local, budget, depth, form.span)
    }

    pub(in crate::macro_expand) fn eval_body(
        &mut self,
        body: &[Form],
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
        fallback_span: Span,
    ) -> Result<Value, EvalError> {
        let mut result = Value::Data(none(fallback_span));
        for expression in body {
            result = self.eval(expression, environment, budget, depth)?;
        }
        Ok(result)
    }

    pub(in crate::macro_expand) fn invoke_callable(
        &mut self,
        callable: Callable,
        arguments: Vec<Value>,
        span: Span,
        budget: &mut EvalBudget,
        depth: usize,
    ) -> Result<Value, EvalError> {
        tick_budget(budget, depth, span)?;
        match callable {
            Callable::Builtin(name) => {
                self.invoke_builtin(name, arguments, span, budget, depth + 1)
            }
            Callable::User(name) => {
                let definition = self.phase_functions.get(&name).cloned().ok_or_else(|| {
                    EvalError::evaluation(format!("unknown phase-1 function `{name}`"), span)
                })?;
                let mut environment = Environment::new();
                let previous_namespace = std::mem::replace(
                    &mut self.active_phase_namespace,
                    definition.namespace.clone(),
                );
                let result = (|| {
                    bind_parameters(
                        &mut BindContext {
                            expander: self,
                            environment: &mut environment,
                            budget,
                            span,
                            depth: depth + 1,
                        },
                        &definition.params,
                        &arguments,
                        false,
                    )?;
                    self.eval_body(
                        &definition.body,
                        &mut environment,
                        budget,
                        depth + 1,
                        definition.span,
                    )
                })();
                self.active_phase_namespace = previous_namespace;
                result
            }
            Callable::Lambda(lambda) => {
                let mut environment = lambda.closure.clone();
                let previous_namespace =
                    std::mem::replace(&mut self.active_phase_namespace, lambda.namespace.clone());
                let result = (|| {
                    bind_parameters(
                        &mut BindContext {
                            expander: self,
                            environment: &mut environment,
                            budget,
                            span,
                            depth: depth + 1,
                        },
                        &lambda.params,
                        &arguments,
                        false,
                    )?;
                    self.eval_body(&lambda.body, &mut environment, budget, depth + 1, span)
                })();
                self.active_phase_namespace = previous_namespace;
                result
            }
        }
    }
}
