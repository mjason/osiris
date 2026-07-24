use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_sequence_call(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
        operation: SequenceOperation,
    ) -> Expr {
        if !call.keywords.is_empty() || !operation.accepts_arity(call.positional.len()) {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0041",
                format!(
                    "osiris.kernel/{} received an invalid argument list",
                    operation.runtime_name()
                ),
                span,
            );
            return Expr::error(span);
        }

        let mut arguments = Vec::with_capacity(call.positional.len());
        let mut parameter_types = Vec::with_capacity(call.positional.len());
        let mut summaries = CallSummaries::unknown();
        let result_type;
        let list_of = |item: Type| Type::List(Box::new(item));
        let item_type = |value: &Type| indexed_type(value);

        match operation {
            SequenceOperation::Cons => {
                let value = self.lower_expr(&call.positional[0], scope);
                let collection = self.lower_expr(&call.positional[1], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                result_type = list_of(self.types.join(&value.ty, &item));
                summaries = value.summaries.join(&collection.summaries);
                parameter_types.extend([value.ty.clone(), collection.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(value),
                    CallArgument::Positional(collection),
                ]);
            }
            SequenceOperation::Concat => {
                let mut item = Type::Never;
                for expression in &call.positional {
                    let value = self.lower_expr(expression, scope);
                    let value_item = item_type(&self.types.resolve(&value.ty));
                    item = self.types.join(&item, &value_item);
                    summaries = summaries.join(&value.summaries);
                    parameter_types.push(value.ty.clone());
                    arguments.push(CallArgument::Positional(value));
                }
                result_type = list_of(if item == Type::Never { Type::Any } else { item });
            }
            SequenceOperation::Count => {
                let value = self.lower_expr(&call.positional[0], scope);
                summaries = summaries.join(&value.summaries);
                parameter_types.push(value.ty.clone());
                arguments.push(CallArgument::Positional(value));
                result_type = Type::Int;
            }
            SequenceOperation::EmptyQ => {
                let value = self.lower_expr(&call.positional[0], scope);
                summaries = summaries.join(&value.summaries);
                parameter_types.push(value.ty.clone());
                arguments.push(CallArgument::Positional(value));
                result_type = Type::Bool;
            }
            SequenceOperation::SeqQ | SequenceOperation::CollQ | SequenceOperation::SequentialQ => {
                let value = self.lower_expr(&call.positional[0], scope);
                summaries = summaries.join(&value.summaries);
                parameter_types.push(value.ty.clone());
                arguments.push(CallArgument::Positional(value));
                result_type = Type::Bool;
            }
            SequenceOperation::First | SequenceOperation::Rest | SequenceOperation::Next => {
                let value = self.lower_expr(&call.positional[0], scope);
                let item = item_type(&self.types.resolve(&value.ty));
                summaries = summaries.join(&value.summaries);
                parameter_types.push(value.ty.clone());
                arguments.push(CallArgument::Positional(value));
                result_type = match operation {
                    SequenceOperation::First => Type::option(item),
                    SequenceOperation::Rest => list_of(item),
                    SequenceOperation::Next => Type::option(list_of(item)),
                    _ => unreachable!(),
                };
            }
            SequenceOperation::Nth => {
                let collection = self.lower_expr(&call.positional[0], scope);
                let index = self.lower_expr(&call.positional[1], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                summaries = summaries.join(&collection.summaries).join(&index.summaries);
                parameter_types.extend([collection.ty.clone(), index.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(collection),
                    CallArgument::Positional(index),
                ]);
                let mut nth_result = item;
                if let Some(default) = call.positional.get(2) {
                    let default = self.lower_expr(default, scope);
                    nth_result = self.types.join(&nth_result, &default.ty);
                    summaries = summaries.join(&default.summaries);
                    parameter_types.push(default.ty.clone());
                    arguments.push(CallArgument::Positional(default));
                }
                result_type = nth_result;
            }
            SequenceOperation::Seq | SequenceOperation::Empty | SequenceOperation::Sequence => {
                let value = self.lower_expr(&call.positional[0], scope);
                let item = item_type(&self.types.resolve(&value.ty));
                let value_type = self.types.resolve(&value.ty);
                let empty_type = value_type.clone();
                summaries = summaries.join(&value.summaries);
                parameter_types.push(value.ty.clone());
                arguments.push(CallArgument::Positional(value));
                result_type = match operation {
                    SequenceOperation::Seq => Type::option(list_of(item)),
                    SequenceOperation::Empty => match &empty_type {
                        Type::List(_) | Type::Vector(_) | Type::Set(_) | Type::Map(_, _) => {
                            empty_type.clone()
                        }
                        Type::Str => Type::Str,
                        Type::Bytes => Type::Bytes,
                        _ => list_of(item),
                    },
                    SequenceOperation::Sequence => list_of(item),
                    _ => unreachable!(),
                };
            }
            SequenceOperation::Take | SequenceOperation::Drop | SequenceOperation::TakeLast => {
                let amount = self.lower_expr(&call.positional[0], scope);
                let collection = self.lower_expr(&call.positional[1], scope);
                self.check_assignable(&amount.ty, &Type::Int, amount.span);
                let item = item_type(&self.types.resolve(&collection.ty));
                summaries = summaries
                    .join(&amount.summaries)
                    .join(&collection.summaries);
                parameter_types.extend([amount.ty.clone(), collection.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(amount),
                    CallArgument::Positional(collection),
                ]);
                result_type = list_of(item);
            }
            SequenceOperation::TakeWhile
            | SequenceOperation::DropWhile
            | SequenceOperation::Keep
            | SequenceOperation::Remove
            | SequenceOperation::Removev
            | SequenceOperation::Some
            | SequenceOperation::Every
            | SequenceOperation::NotEvery
            | SequenceOperation::NotAny => {
                let collection = self.lower_expr(&call.positional[1], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                let callback = self.lower_sequence_callback(
                    &call.positional[0],
                    call.positional[0].span,
                    scope,
                    std::slice::from_ref(&item),
                );
                let callback_result = match &callback.ty {
                    Type::Fn(signature) => (*signature.return_type).clone(),
                    _ => Type::Any,
                };
                summaries = summaries
                    .join(&callback.summaries)
                    .join(&collection.summaries);
                parameter_types.extend([callback.ty.clone(), collection.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(callback),
                    CallArgument::Positional(collection),
                ]);
                result_type = match operation {
                    SequenceOperation::TakeWhile | SequenceOperation::DropWhile => list_of(item),
                    SequenceOperation::Keep | SequenceOperation::Remove => {
                        list_of(if operation == SequenceOperation::Keep {
                            non_nil_type(&callback_result)
                        } else {
                            item.clone()
                        })
                    }
                    SequenceOperation::Removev => Type::Vector(Box::new(item)),
                    SequenceOperation::Some => Type::option(callback_result),
                    SequenceOperation::Every
                    | SequenceOperation::NotEvery
                    | SequenceOperation::NotAny => Type::Bool,
                    _ => unreachable!(),
                };
            }
            SequenceOperation::Distinct | SequenceOperation::Dedupe => {
                let collection = self.lower_expr(&call.positional[0], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                summaries = summaries.join(&collection.summaries);
                parameter_types.push(collection.ty.clone());
                arguments.push(CallArgument::Positional(collection));
                result_type = list_of(item);
            }
            SequenceOperation::Partition | SequenceOperation::PartitionAll => {
                let values = call
                    .positional
                    .iter()
                    .map(|expression| self.lower_expr(expression, scope))
                    .collect::<Vec<_>>();
                self.check_assignable(&values[0].ty, &Type::Int, values[0].span);
                if values.len() >= 3 {
                    self.check_assignable(&values[1].ty, &Type::Int, values[1].span);
                }
                let collection = values.last().expect("partition arity validated");
                let mut item = item_type(&self.types.resolve(&collection.ty));
                if operation == SequenceOperation::Partition && values.len() == 4 {
                    let padding_item = item_type(&self.types.resolve(&values[2].ty));
                    item = self.types.join(&item, &padding_item);
                }
                for value in values {
                    summaries = summaries.join(&value.summaries);
                    parameter_types.push(value.ty.clone());
                    arguments.push(CallArgument::Positional(value));
                }
                result_type = list_of(list_of(item));
            }
            SequenceOperation::PartitionBy => {
                let collection = self.lower_expr(&call.positional[1], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                let callback = self.lower_sequence_callback(
                    &call.positional[0],
                    call.positional[0].span,
                    scope,
                    std::slice::from_ref(&item),
                );
                summaries = summaries
                    .join(&callback.summaries)
                    .join(&collection.summaries);
                parameter_types.extend([callback.ty.clone(), collection.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(callback),
                    CallArgument::Positional(collection),
                ]);
                result_type = list_of(list_of(item));
            }
            SequenceOperation::Interleave => {
                let mut item = Type::Never;
                for expression in &call.positional {
                    let collection = self.lower_expr(expression, scope);
                    let collection_item = item_type(&self.types.resolve(&collection.ty));
                    item = self.types.join(&item, &collection_item);
                    summaries = summaries.join(&collection.summaries);
                    parameter_types.push(collection.ty.clone());
                    arguments.push(CallArgument::Positional(collection));
                }
                result_type = list_of(if item == Type::Never { Type::Any } else { item });
            }
            SequenceOperation::Interpose => {
                let separator = self.lower_expr(&call.positional[0], scope);
                let collection = self.lower_expr(&call.positional[1], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                result_type = list_of(self.types.join(&separator.ty, &item));
                summaries = separator.summaries.join(&collection.summaries);
                parameter_types.extend([separator.ty.clone(), collection.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(separator),
                    CallArgument::Positional(collection),
                ]);
            }
            SequenceOperation::DropLast => {
                let values = call
                    .positional
                    .iter()
                    .map(|expression| self.lower_expr(expression, scope))
                    .collect::<Vec<_>>();
                if values.len() == 2 {
                    self.check_assignable(&values[0].ty, &Type::Int, values[0].span);
                }
                let collection = values.last().expect("drop-last arity validated");
                let item = item_type(&self.types.resolve(&collection.ty));
                for value in values {
                    summaries = summaries.join(&value.summaries);
                    parameter_types.push(value.ty.clone());
                    arguments.push(CallArgument::Positional(value));
                }
                result_type = list_of(item);
            }
            SequenceOperation::KeepIndexed | SequenceOperation::MapIndexed => {
                let collection = self.lower_expr(&call.positional[1], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                let callback = self.lower_sequence_callback(
                    &call.positional[0],
                    call.positional[0].span,
                    scope,
                    &[Type::Int, item.clone()],
                );
                let callback_result = match &callback.ty {
                    Type::Fn(signature) => (*signature.return_type).clone(),
                    _ => Type::Any,
                };
                summaries = summaries
                    .join(&callback.summaries)
                    .join(&collection.summaries);
                parameter_types.extend([callback.ty.clone(), collection.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(callback),
                    CallArgument::Positional(collection),
                ]);
                result_type = if operation == SequenceOperation::KeepIndexed {
                    list_of(non_nil_type(&callback_result))
                } else {
                    list_of(callback_result)
                };
            }
            SequenceOperation::Iterate => {
                let initial = self.lower_expr(&call.positional[1], scope);
                let callback = self.lower_sequence_callback(
                    &call.positional[0],
                    call.positional[0].span,
                    scope,
                    std::slice::from_ref(&initial.ty),
                );
                if let Type::Fn(signature) = &callback.ty {
                    self.check_assignable(&signature.return_type, &initial.ty, callback.span);
                }
                summaries = summaries.join(&callback.summaries).join(&initial.summaries);
                parameter_types.extend([callback.ty.clone(), initial.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(callback),
                    CallArgument::Positional(initial.clone()),
                ]);
                result_type = list_of(initial.ty);
            }
            SequenceOperation::Repeat => {
                let values = call
                    .positional
                    .iter()
                    .map(|expression| self.lower_expr(expression, scope))
                    .collect::<Vec<_>>();
                let value_type = values
                    .last()
                    .map(|value| value.ty.clone())
                    .expect("repeat arity validated");
                if values.len() == 2 {
                    self.check_assignable(&values[0].ty, &Type::Int, values[0].span);
                }
                for value in values {
                    summaries = summaries.join(&value.summaries);
                    parameter_types.push(value.ty.clone());
                    arguments.push(CallArgument::Positional(value));
                }
                result_type = list_of(value_type);
            }
            SequenceOperation::Repeatedly => {
                let callback_index = if call.positional.len() == 1 { 0 } else { 1 };
                let callback = self.lower_sequence_callback(
                    &call.positional[callback_index],
                    call.positional[callback_index].span,
                    scope,
                    &[],
                );
                if call.positional.len() == 2 {
                    let amount = self.lower_expr(&call.positional[0], scope);
                    self.check_assignable(&amount.ty, &Type::Int, amount.span);
                    summaries = summaries.join(&amount.summaries);
                    parameter_types.push(amount.ty.clone());
                    arguments.push(CallArgument::Positional(amount));
                }
                summaries = summaries.join(&callback.summaries);
                parameter_types.push(callback.ty.clone());
                arguments.push(CallArgument::Positional(callback.clone()));
                let callback_result = match callback.ty {
                    Type::Fn(signature) => (*signature.return_type).clone(),
                    _ => Type::Any,
                };
                result_type = list_of(callback_result);
            }
            SequenceOperation::Cycle => {
                let collection = self.lower_expr(&call.positional[0], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                summaries = summaries.join(&collection.summaries);
                parameter_types.push(collection.ty.clone());
                arguments.push(CallArgument::Positional(collection));
                result_type = list_of(item);
            }
            SequenceOperation::Reductions => {
                let collection_index = call.positional.len() - 1;
                let collection = self.lower_expr(&call.positional[collection_index], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                let (accumulator, initial_expr) = if call.positional.len() == 3 {
                    let initial = self.lower_expr(&call.positional[1], scope);
                    (initial.ty.clone(), Some(initial))
                } else {
                    (item.clone(), None)
                };
                let callback = self.lower_sequence_callback(
                    &call.positional[0],
                    call.positional[0].span,
                    scope,
                    &[accumulator.clone(), item],
                );
                if let Type::Fn(signature) = &callback.ty {
                    let callback_accumulator = unreduced_type(&signature.return_type);
                    self.check_assignable(&callback_accumulator, &accumulator, callback.span);
                }
                summaries = summaries
                    .join(&callback.summaries)
                    .join(&collection.summaries);
                parameter_types.push(callback.ty.clone());
                arguments.push(CallArgument::Positional(callback));
                if let Some(initial) = initial_expr {
                    summaries = summaries.join(&initial.summaries);
                    parameter_types.push(initial.ty.clone());
                    arguments.push(CallArgument::Positional(initial));
                }
                parameter_types.push(collection.ty.clone());
                arguments.push(CallArgument::Positional(collection));
                result_type = list_of(accumulator);
            }
            SequenceOperation::RunBang => {
                let collection = self.lower_expr(&call.positional[1], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                let callback = self.lower_sequence_callback(
                    &call.positional[0],
                    call.positional[0].span,
                    scope,
                    std::slice::from_ref(&item),
                );
                summaries = summaries
                    .join(&callback.summaries)
                    .join(&collection.summaries);
                parameter_types.extend([callback.ty.clone(), collection.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(callback),
                    CallArgument::Positional(collection),
                ]);
                result_type = Type::None;
            }
            SequenceOperation::Doall | SequenceOperation::Dorun => {
                let (limit, collection) = if call.positional.len() == 2 {
                    let limit = self.lower_expr(&call.positional[0], scope);
                    self.check_assignable(&limit.ty, &Type::Int, limit.span);
                    let collection = self.lower_expr(&call.positional[1], scope);
                    (Some(limit), collection)
                } else {
                    (None, self.lower_expr(&call.positional[0], scope))
                };
                summaries = summaries.join(&collection.summaries);
                if let Some(limit) = limit {
                    summaries = summaries.join(&limit.summaries);
                    parameter_types.push(limit.ty.clone());
                    arguments.push(CallArgument::Positional(limit));
                }
                parameter_types.push(collection.ty.clone());
                arguments.push(CallArgument::Positional(collection.clone()));
                result_type = if operation == SequenceOperation::Doall {
                    collection.ty
                } else {
                    Type::None
                };
            }
        }

        let binding = self.ensure_core_collection_binding(operation.runtime_name(), span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(parameter_types, result_type.clone())
                    .with_summaries(CallSummaries::unknown()),
            ),
            ExprKind::Binding(binding),
        );
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
}
