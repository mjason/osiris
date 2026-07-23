use super::*;

impl Expander {
    pub(in crate::macro_expand) fn invoke_builtin(
        &mut self,
        name: &'static str,
        mut arguments: Vec<Value>,
        span: Span,
        budget: &mut EvalBudget,
        depth: usize,
    ) -> Result<Value, EvalError> {
        match name {
            "identity" => {
                require_value_arity(&arguments, 1, name, span)?;
                Ok(arguments.remove(0))
            }
            "reduced" => {
                require_value_arity(&arguments, 1, name, span)?;
                Ok(Value::Reduced(Box::new(arguments.remove(0))))
            }
            "reduced?" => {
                require_value_arity(&arguments, 1, name, span)?;
                Ok(Value::Data(boolean(
                    matches!(arguments.first(), Some(Value::Reduced(_))),
                    span,
                )))
            }
            "unreduced" => {
                require_value_arity(&arguments, 1, name, span)?;
                Ok(match arguments.remove(0) {
                    Value::Reduced(value) => *value,
                    value => value,
                })
            }
            "list" => Ok(Value::Data(list(values_into_forms(arguments, span)?, span))),
            "vector" => Ok(Value::Data(vector(
                values_into_forms(arguments, span)?,
                span,
            ))),
            "hash-map" => {
                let items = values_into_forms(arguments, span)?;
                if items.len() % 2 != 0 {
                    return Err(EvalError::evaluation(
                        "`hash-map` expects key/value pairs",
                        span,
                    ));
                }
                Ok(Value::Data(Form::new(FormKind::Map(items), span)))
            }
            "hash-set" => Ok(Value::Data(Form::new(
                FormKind::Set(unique_forms(values_into_forms(arguments, span)?)),
                span,
            ))),
            "not" => {
                require_value_arity(&arguments, 1, name, span)?;
                Ok(Value::Data(boolean(!is_truthy(&arguments[0]), span)))
            }
            "=" | "not=" => {
                let forms = values_into_forms(arguments, span)?;
                let equal = forms
                    .windows(2)
                    .all(|pair| crate::syntax::datum_eq(&pair[0], &pair[1]));
                Ok(Value::Data(boolean(
                    if name == "=" { equal } else { !equal },
                    span,
                )))
            }
            "+" | "-" | "*" | "/" | "inc" | "dec" => {
                let forms = values_into_forms(arguments, span)?;
                numeric_builtin(name, &forms, span).map(Value::Data)
            }
            "<" | "<=" | ">" | ">=" => {
                let forms = values_into_forms(arguments, span)?;
                compare_builtin(name, &forms, span).map(Value::Data)
            }
            "cons" => {
                require_value_arity(&arguments, 2, name, span)?;
                let mut forms = sequence_items(
                    &arguments.pop().expect("arity checked").into_data(span)?,
                    span,
                )?;
                forms.insert(0, arguments.pop().expect("arity checked").into_data(span)?);
                Ok(Value::Data(list(forms, span)))
            }
            "concat" => {
                let mut result = Vec::new();
                for argument in arguments {
                    result.extend(sequence_items(&argument.into_data(span)?, span)?);
                }
                Ok(Value::Data(list(result, span)))
            }
            "conj" => {
                if arguments.is_empty() {
                    return Err(EvalError::evaluation(
                        "`conj` expects a collection and zero or more values",
                        span,
                    ));
                }
                let collection = arguments.remove(0).into_data(span)?;
                let values = values_into_forms(arguments, span)?;
                conj(collection, values, span).map(Value::Data)
            }
            "first" | "rest" | "next" => {
                require_value_arity(&arguments, 1, name, span)?;
                let collection = arguments.remove(0).into_data(span)?;
                let items = sequence_items(&collection, span)?;
                let result = match name {
                    "first" => items.first().cloned().unwrap_or_else(|| none(span)),
                    "rest" => list(items.into_iter().skip(1).collect(), span),
                    "next" if items.len() <= 1 => none(span),
                    "next" => list(items.into_iter().skip(1).collect(), span),
                    _ => unreachable!(),
                };
                Ok(Value::Data(result))
            }
            "nth" => {
                if !(2..=3).contains(&arguments.len()) {
                    return Err(EvalError::evaluation(
                        "`nth` expects a collection, index, and optional default",
                        span,
                    ));
                }
                let collection = arguments.remove(0).into_data(span)?;
                let index = form_to_usize(&arguments.remove(0).into_data(span)?, span)?;
                let default = arguments
                    .pop()
                    .map(|value| value.into_data(span))
                    .transpose()?;
                sequence_items(&collection, span)?
                    .get(index)
                    .cloned()
                    .or(default)
                    .map(Value::Data)
                    .ok_or_else(|| EvalError::evaluation("`nth` index is out of bounds", span))
            }
            "count" => {
                require_value_arity(&arguments, 1, name, span)?;
                let form = arguments.remove(0).into_data(span)?;
                let count = collection_count(&form, span)?;
                Ok(Value::Data(integer(count, span)))
            }
            "empty?" => {
                require_value_arity(&arguments, 1, name, span)?;
                let form = arguments.remove(0).into_data(span)?;
                Ok(Value::Data(boolean(
                    collection_count(&form, span)? == 0,
                    span,
                )))
            }
            "seq" => {
                require_value_arity(&arguments, 1, name, span)?;
                let form = arguments.remove(0).into_data(span)?;
                let items = sequence_items(&form, span)?;
                Ok(Value::Data(if items.is_empty() {
                    none(span)
                } else {
                    list(items, span)
                }))
            }
            "get" => {
                if !(2..=3).contains(&arguments.len()) {
                    return Err(EvalError::evaluation(
                        "`get` expects a collection, key, and optional default",
                        span,
                    ));
                }
                let collection = arguments.remove(0).into_data(span)?;
                let key = arguments.remove(0).into_data(span)?;
                let default = arguments
                    .pop()
                    .map(|value| value.into_data(span))
                    .transpose()?
                    .unwrap_or_else(|| none(span));
                Ok(Value::Data(
                    get_from_collection(&collection, &key).unwrap_or(default),
                ))
            }
            "contains?" => {
                require_value_arity(&arguments, 2, name, span)?;
                let collection = arguments.remove(0).into_data(span)?;
                let key = arguments.remove(0).into_data(span)?;
                Ok(Value::Data(boolean(
                    collection_contains(&collection, &key),
                    span,
                )))
            }
            "assoc" => {
                if arguments.len() < 3 || arguments.len() % 2 == 0 {
                    return Err(EvalError::evaluation(
                        "`assoc` expects a map and key/value pairs",
                        span,
                    ));
                }
                let map = arguments.remove(0).into_data(span)?;
                assoc_form(map, values_into_forms(arguments, span)?, span).map(Value::Data)
            }
            "dissoc" => {
                if arguments.is_empty() {
                    return Err(EvalError::evaluation(
                        "`dissoc` expects a map and zero or more keys",
                        span,
                    ));
                }
                let map = arguments.remove(0).into_data(span)?;
                dissoc_form(map, &values_into_forms(arguments, span)?, span).map(Value::Data)
            }
            "keys" | "vals" => {
                require_value_arity(&arguments, 1, name, span)?;
                let map = arguments.remove(0).into_data(span)?;
                let FormKind::Map(items) = map.kind else {
                    return Err(EvalError::evaluation(
                        format!("`{name}` expects a map"),
                        span,
                    ));
                };
                let offset = usize::from(name == "vals");
                Ok(Value::Data(list(
                    items.into_iter().skip(offset).step_by(2).collect(),
                    span,
                )))
            }
            "meta" => {
                require_value_arity(&arguments, 1, name, span)?;
                let target = arguments.remove(0).into_data(span)?;
                Ok(Value::Data(metadata_map(&target)))
            }
            "with-meta" => {
                require_value_arity(&arguments, 2, name, span)?;
                let target = arguments.remove(0).into_data(span)?;
                let metadata = arguments.remove(0).into_data(span)?;
                with_metadata(target, &metadata, span).map(Value::Data)
            }
            "vary-meta" => {
                if arguments.len() < 2 {
                    return Err(EvalError::evaluation(
                        "`vary-meta` expects syntax, a function, and optional arguments",
                        span,
                    ));
                }
                let target = arguments.remove(0).into_data(span)?;
                let callable = value_callable(arguments.remove(0), span)?;
                let mut call_arguments = vec![Value::Data(metadata_map(&target))];
                call_arguments.extend(arguments);
                let metadata = self
                    .invoke_callable(callable, call_arguments, span, budget, depth + 1)?
                    .into_data(span)?;
                with_metadata(target, &metadata, span).map(Value::Data)
            }
            "gensym" => {
                if arguments.len() > 1 {
                    return Err(EvalError::evaluation(
                        "`gensym` accepts at most one prefix",
                        span,
                    ));
                }
                let prefix = arguments
                    .pop()
                    .map(|value| value.into_data(span))
                    .transpose()?
                    .map(|form| form_name_or_string(&form, span))
                    .transpose()?
                    .unwrap_or_else(|| "G__".to_owned());
                Ok(Value::Data(self.generated_symbol(&prefix, span)))
            }
            "syntax-error" => {
                if arguments.is_empty() || arguments.len() > 2 {
                    return Err(EvalError::evaluation(
                        "`syntax-error` expects a message, optionally preceded by syntax",
                        span,
                    ));
                }
                let (error_span, message_value) = if arguments.len() == 2 {
                    let target = arguments.remove(0).into_data(span)?;
                    (target.span, arguments.remove(0).into_data(span)?)
                } else {
                    (span, arguments.remove(0).into_data(span)?)
                };
                let message = form_to_string(&message_value, span)?;
                Err(EvalError::new("OSR-M0007", message, error_span))
            }
            "symbol" | "keyword" => {
                require_value_arity(&arguments, 1, name, span)?;
                let spelling = form_to_string(&arguments.remove(0).into_data(span)?, span)?;
                let spelling = if name == "keyword" && !spelling.starts_with(':') {
                    format!(":{spelling}")
                } else {
                    spelling
                };
                Ok(Value::Data(named_form(name == "keyword", &spelling, span)))
            }
            "name" | "namespace" => {
                require_value_arity(&arguments, 1, name, span)?;
                let form = arguments.remove(0).into_data(span)?;
                let full = form_name_or_string(&form, span)?;
                let trimmed = full.strip_prefix(':').unwrap_or(&full);
                let value = if name == "name" {
                    trimmed.rsplit('/').next().unwrap_or(trimmed).to_owned()
                } else {
                    trimmed
                        .rsplit_once('/')
                        .map(|(namespace, _)| namespace.to_owned())
                        .unwrap_or_default()
                };
                Ok(Value::Data(string(&value, span)))
            }
            "str" => {
                let forms = values_into_forms(arguments, span)?;
                let mut value = String::new();
                for form in forms {
                    value.push_str(&display_form(&form));
                }
                Ok(Value::Data(string(&value, span)))
            }
            "nil?" | "some?" | "symbol?" | "keyword?" | "list?" | "vector?" | "map?" | "set?"
            | "sequential?" => {
                require_value_arity(&arguments, 1, name, span)?;
                let form = arguments.remove(0).into_data(span)?;
                let matches = match name {
                    "nil?" => matches!(form.kind, FormKind::None),
                    "some?" => !matches!(form.kind, FormKind::None),
                    "symbol?" => matches!(form.kind, FormKind::Symbol(_)),
                    "keyword?" => matches!(form.kind, FormKind::Keyword(_)),
                    "list?" => matches!(form.kind, FormKind::List(_)),
                    "vector?" => matches!(form.kind, FormKind::Vector(_)),
                    "map?" => matches!(form.kind, FormKind::Map(_)),
                    "set?" => matches!(form.kind, FormKind::Set(_)),
                    "sequential?" => {
                        matches!(form.kind, FormKind::List(_) | FormKind::Vector(_))
                    }
                    _ => unreachable!(),
                };
                Ok(Value::Data(boolean(matches, span)))
            }
            "apply" => {
                if arguments.len() < 2 {
                    return Err(EvalError::evaluation(
                        "`apply` expects a function and an argument sequence",
                        span,
                    ));
                }
                let callable = value_callable(arguments.remove(0), span)?;
                let tail = arguments.pop().expect("length checked").into_data(span)?;
                arguments.extend(sequence_items(&tail, span)?.into_iter().map(Value::Data));
                self.invoke_callable(callable, arguments, span, budget, depth + 1)
            }
            "map" | "mapv" => {
                if arguments.len() < 2 {
                    return Err(EvalError::evaluation(
                        format!("`{name}` expects a function and collections"),
                        span,
                    ));
                }
                let callable = value_callable(arguments.remove(0), span)?;
                let collections = arguments
                    .into_iter()
                    .map(|value| value.into_data(span))
                    .map(|result| result.and_then(|form| sequence_items(&form, span)))
                    .collect::<Result<Vec<_>, _>>()?;
                let length = collections.iter().map(Vec::len).min().unwrap_or(0);
                let mut mapped = Vec::with_capacity(length);
                for index in 0..length {
                    let call_arguments = collections
                        .iter()
                        .map(|collection| Value::Data(collection[index].clone()))
                        .collect();
                    mapped.push(
                        self.invoke_callable(
                            callable.clone(),
                            call_arguments,
                            span,
                            budget,
                            depth + 1,
                        )?
                        .into_data(span)?,
                    );
                }
                Ok(Value::Data(if name == "mapv" {
                    vector(mapped, span)
                } else {
                    list(mapped, span)
                }))
            }
            "mapcat" | "mapcatv" => {
                if arguments.len() != 2 {
                    return Err(EvalError::evaluation(
                        format!("`{name}` expects a function and one collection"),
                        span,
                    ));
                }
                let callable = value_callable(arguments.remove(0), span)?;
                let collection = arguments.remove(0).into_data(span)?;
                let mut flattened = Vec::new();
                for item in sequence_items(&collection, span)? {
                    let result = self
                        .invoke_callable(
                            callable.clone(),
                            vec![Value::Data(item)],
                            span,
                            budget,
                            depth + 1,
                        )?
                        .into_data(span)?;
                    flattened.extend(sequence_items(&result, span)?);
                }
                Ok(Value::Data(if name == "mapcatv" {
                    vector(flattened, span)
                } else {
                    list(flattened, span)
                }))
            }
            "filter" | "filterv" => {
                if arguments.len() != 2 {
                    return Err(EvalError::evaluation(
                        format!("`{name}` expects a predicate and one collection"),
                        span,
                    ));
                }
                let callable = value_callable(arguments.remove(0), span)?;
                let collection = arguments.remove(0).into_data(span)?;
                let mut selected = Vec::new();
                for item in sequence_items(&collection, span)? {
                    let result = self.invoke_callable(
                        callable.clone(),
                        vec![Value::Data(item.clone())],
                        span,
                        budget,
                        depth + 1,
                    )?;
                    let predicate = result.into_data(span)?;
                    if !matches!(predicate.kind, FormKind::None | FormKind::Bool(false)) {
                        selected.push(item);
                    }
                }
                Ok(Value::Data(if name == "filterv" {
                    vector(selected, span)
                } else {
                    list(selected, span)
                }))
            }
            "reduce" | "fold" => {
                let valid = if name == "fold" {
                    arguments.len() == 3
                } else {
                    (2..=3).contains(&arguments.len())
                };
                if !valid {
                    return Err(EvalError::evaluation(
                        if name == "fold" {
                            "`fold` expects a function, initial value, and collection"
                        } else {
                            "`reduce` expects a function, optional initial value, and collection"
                        },
                        span,
                    ));
                }
                let callable = value_callable(arguments.remove(0), span)?;
                let collection = arguments.pop().expect("length checked").into_data(span)?;
                let mut items = sequence_items(&collection, span)?.into_iter();
                let mut accumulator = match arguments.pop() {
                    Some(initial) => initial,
                    None => Value::Data(items.next().ok_or_else(|| {
                        EvalError::evaluation("`reduce` without an initial value needs data", span)
                    })?),
                };
                for item in items {
                    let next = self.invoke_callable(
                        callable.clone(),
                        vec![accumulator, Value::Data(item)],
                        span,
                        budget,
                        depth + 1,
                    )?;
                    match next {
                        Value::Reduced(value) => {
                            accumulator = *value;
                            break;
                        }
                        value => accumulator = value,
                    }
                }
                Ok(accumulator)
            }
            _ => Err(EvalError::evaluation(
                format!("unsupported phase-1 builtin `{name}`"),
                span,
            )),
        }
    }
}
