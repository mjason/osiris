use super::*;

impl<'hir> Backend<'hir> {
    pub(super) fn lower_function(
        &mut self,
        function: &hir::Function,
    ) -> Result<py::Stmt, BackendError> {
        let binding_name = self.python_name(&function.binding).to_owned();
        let decorators = self.lower_decorators(&function.decorators)?;
        let mut parameters = py::Parameters::default();
        let keyword_only_from =
            function
                .parameters
                .iter()
                .enumerate()
                .find_map(|(index, parameter)| {
                    (parameter.default.is_some()
                        && function.parameters[index + 1..]
                            .iter()
                            .any(|later| later.default.is_none() && !later.variadic))
                    .then_some(index)
                });
        for (parameter_index, parameter) in function.parameters.iter().enumerate() {
            let name = self.python_name(&parameter.binding).to_owned();
            let annotation = Some(self.annotation(&parameter.ty, Some(function.body.span))?);
            let default = match &parameter.default {
                Some(default) => {
                    let lowered = self.lower_value(default)?;
                    if !lowered.prefix.is_empty() {
                        return Err(self.error(
                            "function parameter defaults must be expression-only",
                            Some(default.span),
                        ));
                    }
                    Some(lowered.value.ok_or_else(|| {
                        self.error(
                            "parameter default does not produce a value",
                            Some(default.span),
                        )
                    })?)
                }
                None => None,
            };
            let parameter = py::Parameter {
                name,
                annotation,
                default,
            };
            if function.parameters[parameter_index].variadic {
                parameters.vararg = Some(py::Parameter {
                    name: parameter.name,
                    annotation: parameter.annotation,
                    default: None,
                });
            } else if keyword_only_from.is_some_and(|start| parameter_index >= start) {
                parameters.keyword_only.push(parameter);
            } else {
                parameters.positional.push(parameter);
            }
        }
        let returns = Some(self.annotation(&function.return_type, Some(function.body.span))?);
        let mut body = self.lower_tail(&function.body)?;
        if body.is_empty() {
            body.push(py::Stmt::Pass);
        }
        Ok(py::Stmt::FunctionDef(Box::new(py::FunctionDef {
            name: binding_name,
            parameters,
            returns,
            decorators,
            body,
            is_async: false,
        })))
    }

    pub(super) fn lower_struct(
        &mut self,
        structure: &hir::Struct,
    ) -> Result<Vec<py::Stmt>, BackendError> {
        self.need_dataclass = true;
        let mut decorators = self.lower_decorators(&structure.decorators)?;
        let struct_name = self.python_name(&structure.binding).to_owned();
        let previous = std::mem::take(&mut self.active_type_parameters);
        if let Ok(binding) = self.binding(&structure.binding) {
            if let Type::Nominal { args, .. } = &binding.ty {
                for (parameter, argument) in structure.type_parameters.iter().zip(args) {
                    if let Type::TypeVar(variable) = argument {
                        self.typevar_names.insert(*variable, parameter.clone());
                    }
                }
            }
        }
        for parameter in &structure.type_parameters {
            let python = self
                .typevars
                .entry(parameter.clone())
                .or_insert_with(|| parameter.clone())
                .clone();
            self.active_type_parameters
                .insert(parameter.clone(), python);
        }
        let mut body = Vec::new();
        if let Some(doc) = &structure.doc {
            body.push(py::Stmt::Expr(py::Expr::string(doc.clone())));
        }
        for field in &structure.fields {
            let target = py::Expr::name(self.python_name(&field.binding).to_owned());
            let annotation = self.annotation(&field.ty, Some(Span::default()))?;
            let value = match &field.default {
                None => None,
                Some(default) => {
                    let lowered = self.lower_value(default)?;
                    let value = lowered.value.ok_or_else(|| {
                        self.error(
                            "struct field default does not produce a value",
                            Some(default.span),
                        )
                    })?;
                    if lowered.prefix.is_empty() && is_safe_dataclass_default(&value) {
                        Some(value)
                    } else if lowered.prefix.is_empty() {
                        self.need_dataclass_field = true;
                        Some(py::Expr::call(
                            py::Expr::name("field"),
                            vec![py::CallArgument::Keyword(py::KeywordArgument::Named {
                                name: "default_factory".to_owned(),
                                value: py::Expr::Lambda {
                                    parameters: Box::new(py::Parameters::default()),
                                    body: Box::new(value),
                                },
                            })],
                        ))
                    } else {
                        // A helper function preserves both complex control
                        // flow and per-instance default evaluation.
                        self.need_dataclass_field = true;
                        let helper = self.fresh_helper("_osr_default");
                        let mut helper_body = lowered.prefix;
                        helper_body.push(py::Stmt::Return(Some(value)));
                        body.push(py::Stmt::FunctionDef(Box::new(py::FunctionDef {
                            name: helper.clone(),
                            parameters: py::Parameters::default(),
                            returns: None,
                            decorators: Vec::new(),
                            body: helper_body,
                            is_async: false,
                        })));
                        Some(py::Expr::call(
                            py::Expr::name("field"),
                            vec![py::CallArgument::Keyword(py::KeywordArgument::Named {
                                name: "default_factory".to_owned(),
                                value: py::Expr::name(helper),
                            })],
                        ))
                    }
                }
            };
            body.push(py::Stmt::AnnAssign(py::AnnAssign {
                target,
                annotation,
                value,
            }));
        }
        if !structure.checks.is_empty() {
            let self_name = py::Expr::name("self");
            let mut overrides = BTreeMap::new();
            for field in &structure.fields {
                overrides.insert(
                    field.binding.clone(),
                    py::Expr::Attribute {
                        value: Box::new(self_name.clone()),
                        attr: self.python_name(&field.binding).to_owned(),
                    },
                );
            }
            self.binding_overrides.push(overrides);
            let mut checks = Vec::new();
            for check in &structure.checks {
                let lowered = self.lower_value(&check.condition)?;
                let condition = lowered.value.ok_or_else(|| {
                    self.error(
                        "struct check does not produce a boolean value",
                        Some(check.condition.span),
                    )
                })?;
                let message = check
                    .message
                    .as_ref()
                    .map(|message| self.lower_value(message))
                    .transpose()?;
                checks.push((lowered.prefix, condition, message));
            }
            self.binding_overrides.pop();
            let mut check_body = Vec::new();
            for (prefix, condition, message) in checks {
                check_body.extend(prefix);
                let mut failure_body = Vec::new();
                let message = if let Some(message) = message {
                    failure_body.extend(message.prefix);
                    message.value.ok_or_else(|| {
                        self.error("struct check message does not produce a value", None)
                    })?
                } else {
                    py::Expr::string(format!("invariant failed for {struct_name}"))
                };
                failure_body.push(py::Stmt::Raise(py::Raise {
                    exception: Some(py::Expr::call(
                        py::Expr::name("ValueError"),
                        vec![py::CallArgument::Positional(message)],
                    )),
                    cause: None,
                }));
                check_body.push(py::Stmt::If(py::IfStmt {
                    test: py::Expr::UnaryOp {
                        op: py::UnaryOp::Not,
                        operand: Box::new(condition),
                    },
                    body: failure_body,
                    orelse: Vec::new(),
                }));
            }
            body.push(py::Stmt::FunctionDef(Box::new(py::FunctionDef {
                name: "__post_init__".to_owned(),
                parameters: py::Parameters {
                    positional: vec![py::Parameter::new("self")],
                    ..py::Parameters::default()
                },
                returns: Some(py::Expr::name("None")),
                decorators: Vec::new(),
                body: check_body,
                is_async: false,
            })));
        }
        let mut bases = Vec::new();
        if !structure.type_parameters.is_empty() {
            // Generic and TypeVar are runtime names on Python 3.11. A
            // type parameter need not occur in a field annotation (for
            // example an extension marker struct can intentionally leave its
            // payload as Any), so register both imports from the declaration
            // itself rather than relying on annotation traversal.
            self.typing.insert("Generic".to_owned());
            self.typing.insert("TypeVar".to_owned());
            bases.push(py::Expr::Subscript {
                value: Box::new(py::Expr::name("Generic")),
                slice: Box::new(if structure.type_parameters.len() == 1 {
                    py::Expr::name(
                        self.active_type_parameters
                            .get(&structure.type_parameters[0])
                            .cloned()
                            .unwrap_or_else(|| structure.type_parameters[0].clone()),
                    )
                } else {
                    py::Expr::Tuple(
                        structure
                            .type_parameters
                            .iter()
                            .map(|parameter| {
                                py::Expr::name(
                                    self.active_type_parameters
                                        .get(parameter)
                                        .cloned()
                                        .unwrap_or_else(|| parameter.clone()),
                                )
                            })
                            .collect(),
                    )
                }),
            });
        }
        self.active_type_parameters = previous;
        decorators.push(py::Expr::call(
            py::Expr::name("dataclass"),
            vec![py::CallArgument::Keyword(py::KeywordArgument::Named {
                name: "frozen".to_owned(),
                value: py::Expr::Literal(py::Literal::Bool(true)),
            })],
        ));
        Ok(vec![py::Stmt::ClassDef(py::ClassDef {
            name: struct_name,
            bases,
            keywords: Vec::new(),
            decorators,
            body,
        })])
    }

    fn lower_decorators(
        &mut self,
        decorators: &[hir::Expr],
    ) -> Result<Vec<py::Expr>, BackendError> {
        decorators
            .iter()
            .map(|decorator| {
                let lowered = self.lower_value(decorator)?;
                if !lowered.prefix.is_empty() {
                    return Err(self.error(
                        "Python decorator must lower to a single expression",
                        Some(decorator.span),
                    ));
                }
                lowered.value.ok_or_else(|| {
                    self.error(
                        "Python decorator does not produce a value",
                        Some(decorator.span),
                    )
                })
            })
            .collect()
    }
}
