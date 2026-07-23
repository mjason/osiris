use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_extern_functions(&mut self, external: &ast::Extern) {
        for declaration in &external.items {
            let AstItemKind::Defn(function) = &declaration.kind else {
                continue;
            };
            let Some(name) = function.name.as_ref() else {
                continue;
            };
            let Some(binding) = self.global_id(name) else {
                continue;
            };
            let signature = match self.binding_type(&binding) {
                Type::Fn(signature) => signature,
                _ => continue,
            };
            let mut scope = Scope::default();
            scope.push();
            let mut parameters = Vec::new();
            for (index, parameter) in function.params.iter().enumerate() {
                let ty = signature
                    .parameters
                    .get(index)
                    .cloned()
                    .unwrap_or(Type::Error);
                let default = parameter
                    .default
                    .as_ref()
                    .map(|default| self.lower_expr(default, &mut scope));
                if let Some(default) = &default {
                    self.check_assignable(&default.ty, &ty, default.span);
                    self.require_pure(default, "extern parameter default");
                }
                let local = self.declare_local(
                    &parameter.name,
                    BindingKind::Parameter,
                    ty.clone(),
                    parameter.metadata.clone(),
                    parameter.span,
                    &mut scope,
                );
                parameters.push(Parameter {
                    binding: local,
                    ty,
                    default,
                    variadic: parameter.variadic,
                });
            }
            for parameter in &mut parameters {
                parameter.ty = self.types.resolve(&parameter.ty);
            }
            let return_type = self.types.resolve(&signature.return_type);
            let summaries = function
                .contract
                .as_ref()
                .map_or_else(CallSummaries::unknown, |contract| {
                    contract.summaries.clone()
                });
            let contract_evidence = self
                .callables
                .get(&binding)
                .map_or_else(ContractEvidence::default, |callable| {
                    callable.contract_evidence.clone()
                });
            let final_signature = FunctionType::new(
                parameters
                    .iter()
                    .map(|parameter| parameter.ty.clone())
                    .collect(),
                return_type.clone(),
            )
            .with_summaries(summaries.clone());
            self.set_binding_type(&binding, Type::Fn(final_signature.clone()));
            if let Some(callable) = self.callables.get_mut(&binding) {
                callable.signature = final_signature;
                for (shape, parameter) in callable.parameters.iter_mut().zip(&parameters) {
                    shape.ty = parameter.ty.clone();
                }
            }
            self.extern_functions.push(ExternFunction {
                binding,
                parameters,
                return_type,
                contract_id: function
                    .contract
                    .as_ref()
                    .map(|contract| contract.id.clone()),
                summaries,
                contract_evidence,
            });
        }
    }

    pub(in crate::hir) fn lower_function(&mut self, function: &ast::Function) -> Option<Function> {
        let binding = self.global_id(function.name.as_ref()?)?;
        let signature = match self.binding_type(&binding) {
            Type::Fn(signature) => signature,
            _ => return None,
        };
        let mut scope = Scope::default();
        scope.push();
        self.contract_evidence_stack
            .push(ContractEvidence::default());
        let mut parameters = Vec::new();
        let mut parameter_bindings = Vec::new();
        for (index, parameter) in function.params.iter().enumerate() {
            let ty = signature
                .parameters
                .get(index)
                .cloned()
                .unwrap_or(Type::Error);
            let default = parameter
                .default
                .as_ref()
                .map(|default| self.lower_expr(default, &mut scope));
            if let Some(default) = &default {
                self.check_assignable(&default.ty, &ty, default.span);
            }
            let local = self.declare_local(
                &parameter.name,
                BindingKind::Parameter,
                ty.clone(),
                parameter.metadata.clone(),
                parameter.span,
                &mut scope,
            );
            parameters.push(Parameter {
                binding: local.clone(),
                ty,
                default,
                variadic: parameter.variadic,
            });
            if let Some(pattern) = &parameter.pattern {
                let value = Expr::pure(
                    parameter.span,
                    self.binding_type(&local),
                    ExprKind::Binding(local),
                );
                self.lower_pattern_bindings(
                    pattern,
                    value,
                    &pattern.metadata,
                    &mut scope,
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
        let body = self.lower_body(&function.body, &mut scope, function.span);
        let function_recur = self
            .function_recur_contexts
            .pop()
            .expect("function recur context");
        let body = self.wrap_let_bindings(parameter_bindings, body, function.span);
        let body = if function_recur.used {
            self.validate_recur_tail(&body, true);
            self.wrap_function_recur(&parameters, body, function.span)
        } else {
            body
        };
        let contract_evidence = self
            .contract_evidence_stack
            .pop()
            .expect("function contract evidence scope");
        for parameter in &mut parameters {
            parameter.ty = self.types.resolve(&parameter.ty);
        }
        let declared_return = self.types.resolve(&signature.return_type);
        let return_type = if function.return_type.is_some() {
            self.check_assignable(&body.ty, &declared_return, body.span);
            declared_return
        } else {
            body.ty.clone()
        };
        let summaries = parameters
            .iter()
            .fold(body.summaries.clone(), |summary, parameter| {
                parameter
                    .default
                    .as_ref()
                    .map_or(summary.clone(), |default| summary.join(&default.summaries))
            });
        let final_signature = FunctionType::new(
            parameters
                .iter()
                .map(|parameter| parameter.ty.clone())
                .collect(),
            return_type.clone(),
        )
        .with_summaries(summaries.clone());
        self.set_binding_type(&binding, Type::Fn(final_signature.clone()));
        if let Some(callable) = self.callables.get_mut(&binding) {
            callable.signature = final_signature;
            callable.contract_evidence = contract_evidence.clone();
            for (shape, parameter) in callable.parameters.iter_mut().zip(&parameters) {
                shape.ty = parameter.ty.clone();
            }
        }
        let causal = match causal_requirement(&function.metadata) {
            Ok(requirement) => requirement,
            Err(message) => {
                self.error("OSR-C0004", message, function.span);
                None
            }
        };
        if let Some(requirement) = &causal {
            self.validate_causal_function(
                function
                    .name
                    .as_ref()
                    .map_or("<anonymous>", |name| name.spelling.as_str()),
                &summaries,
                &contract_evidence,
                requirement,
                function.span,
            );
        }
        Some(Function {
            binding,
            decorators: Vec::new(),
            parameters,
            return_type,
            body,
            summaries,
            contract_evidence,
            causal,
        })
    }

    pub(in crate::hir) fn lower_struct(&mut self, structure: &ast::Defstruct) -> Option<Struct> {
        let binding = self.global_id(&structure.name)?;
        let generic_parameters = self
            .struct_type_parameters
            .get(&binding)
            .cloned()
            .unwrap_or_default();
        let mut scope = Scope::default();
        scope.push();
        let mut fields = Vec::new();
        for field in &structure.fields {
            let mut ty = field
                .type_annotation
                .as_ref()
                .map_or(Type::Unknown, |expression| {
                    self.resolve_type_expr_with_generics(expression, &generic_parameters)
                });
            if field.type_annotation.is_none() {
                self.error(
                    "OSR-T0002",
                    format!("struct field `{}` requires a type", field.name.spelling),
                    field.span,
                );
            }
            let default = field
                .default
                .as_ref()
                .map(|value| self.lower_expr(value, &mut Scope::default()));
            if let Some(default) = &default {
                if !contains_type_variable(&ty) {
                    self.check_assignable(&default.ty, &ty, default.span);
                }
                self.require_pure(default, "struct field default");
            }
            ty = self.types.resolve(&ty);
            let field_binding = self.declare_local(
                &field.name,
                BindingKind::Field,
                ty.clone(),
                field.metadata.clone(),
                field.span,
                &mut scope,
            );
            fields.push(StructField {
                binding: field_binding,
                ty,
                default,
            });
        }
        let checks = structure
            .checks
            .iter()
            .map(|check| {
                let condition = self.lower_expr(&check.condition, &mut scope);
                self.check_assignable(&condition.ty, &Type::Bool, condition.span);
                self.require_pure(&condition, "struct check");
                let message = check.message.as_ref().map(|message| {
                    let message = self.lower_expr(message, &mut scope);
                    self.check_assignable(&message.ty, &Type::Str, message.span);
                    self.require_pure(&message, "struct check message");
                    message
                });
                StructCheck {
                    span: check.span,
                    condition,
                    message,
                }
            })
            .collect::<Vec<_>>();
        let mut constructor_summaries =
            fields
                .iter()
                .fold(CallSummaries::pure_scalar(), |summary, field| {
                    field
                        .default
                        .as_ref()
                        .map_or(summary.clone(), |default| summary.join(&default.summaries))
                });
        constructor_summaries = checks.iter().fold(constructor_summaries, |summary, check| {
            let summary = summary.join(&check.condition.summaries);
            check
                .message
                .as_ref()
                .map_or(summary.clone(), |message| summary.join(&message.summaries))
        });
        if !checks.is_empty() {
            constructor_summaries.effects = constructor_summaries
                .effects
                .union(&EffectRow::singleton(Effect::Throw));
        }
        let constructor_signature = FunctionType::new(
            fields.iter().map(|field| field.ty.clone()).collect(),
            Type::Nominal {
                binding: binding.as_str().to_owned(),
                args: structure
                    .type_params
                    .iter()
                    .filter_map(|name| generic_parameters.get(&name.canonical).cloned())
                    .collect(),
            },
        )
        .with_summaries(constructor_summaries);
        if let Some(callable) = self.callables.get_mut(&binding) {
            callable.signature = constructor_signature;
            for (shape, field) in callable.parameters.iter_mut().zip(&fields) {
                shape.ty = field.ty.clone();
            }
        }
        Some(Struct {
            binding,
            decorators: Vec::new(),
            type_parameters: structure
                .type_params
                .iter()
                .map(|name| name.canonical.clone())
                .collect(),
            fields,
            checks,
            doc: structure.doc.clone(),
        })
    }
}
