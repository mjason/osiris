use super::*;

impl Expander {
    pub(in crate::macro_expand) fn syntax_quote(
        &mut self,
        form: &Form,
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
        generated: &mut BTreeMap<String, Form>,
    ) -> Result<Form, EvalError> {
        tick_budget(budget, depth, form.span)?;
        match &form.kind {
            FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::Unquote,
                form: expression,
            } => self
                .eval(expression, environment, budget, depth + 1)?
                .into_data(form.span),
            FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::UnquoteSplicing,
                ..
            } => Err(EvalError::evaluation(
                "unquote-splicing is only valid inside a syntax-quoted collection",
                form.span,
            )),
            FormKind::List(items) => self
                .syntax_quote_collection(items, environment, budget, depth + 1, generated)
                .map(|items| Self::with_kind(form, FormKind::List(items))),
            FormKind::Vector(items) => self
                .syntax_quote_collection(items, environment, budget, depth + 1, generated)
                .map(|items| Self::with_kind(form, FormKind::Vector(items))),
            FormKind::Map(items) => {
                let items =
                    self.syntax_quote_collection(items, environment, budget, depth + 1, generated)?;
                if items.len() % 2 != 0 {
                    return Err(EvalError::evaluation(
                        "syntax-quoted map contains an odd number of forms after splicing",
                        form.span,
                    ));
                }
                Ok(Self::with_kind(form, FormKind::Map(items)))
            }
            FormKind::Set(items) => self
                .syntax_quote_collection(items, environment, budget, depth + 1, generated)
                .map(|items| Self::with_kind(form, FormKind::Set(items))),
            FormKind::Symbol(name) if name.canonical.ends_with('#') => {
                if let Some(existing) = generated.get(&name.canonical) {
                    return Ok(existing.clone());
                }
                let hint = name.canonical.trim_end_matches('#');
                let generated_symbol = self.generated_symbol(hint, form.span);
                generated.insert(name.canonical.clone(), generated_symbol.clone());
                Ok(generated_symbol)
            }
            FormKind::Symbol(name) => {
                let Some(namespace) = &self.active_phase_namespace else {
                    return Ok(form.clone());
                };
                let Some(canonical) = self
                    .definition_names
                    .get(namespace)
                    .and_then(|names| names.get(&name.canonical))
                else {
                    return Ok(form.clone());
                };
                Ok(Self::with_kind(
                    form,
                    FormKind::Symbol(Name {
                        spelling: format!("{namespace}/{canonical}"),
                        canonical: format!("{namespace}/{canonical}"),
                    }),
                ))
            }
            // Quote and nested syntax quote introduce their own unquote boundary.
            FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::Quote | ReaderMacroKind::SyntaxQuote,
                ..
            } => Ok(form.clone()),
            _ => Ok(form.clone()),
        }
    }

    pub(in crate::macro_expand) fn syntax_quote_collection(
        &mut self,
        items: &[Form],
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
        generated: &mut BTreeMap<String, Form>,
    ) -> Result<Vec<Form>, EvalError> {
        let mut quoted = Vec::new();
        for item in items {
            if let FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::UnquoteSplicing,
                form: expression,
            } = &item.kind
            {
                let value = self
                    .eval(expression, environment, budget, depth + 1)?
                    .into_data(item.span)?;
                quoted.extend(sequence_items(&value, item.span)?);
            } else {
                quoted.push(self.syntax_quote(item, environment, budget, depth, generated)?);
            }
        }
        Ok(quoted)
    }

    pub(in crate::macro_expand) fn generated_symbol(&mut self, hint: &str, span: Span) -> Form {
        let id = self.next_generated_name;
        self.next_generated_name += 1;
        let spelling = format!("{hint}__osr_g{id}");
        Form::new(
            FormKind::Symbol(Name {
                spelling,
                // Reader-created names always canonicalize their spelling.  A
                // separate NUL-prefixed identity therefore cannot collide with
                // a caller binding that merely has the same visible spelling.
                canonical: format!("\0osr-gensym:{id}:{hint}"),
            }),
            span,
        )
    }

    pub(in crate::macro_expand) fn with_kind(original: &Form, kind: FormKind) -> Form {
        Form {
            span: original.span,
            datum_span: original.datum_span,
            metadata: original.metadata.clone(),
            kind,
        }
    }
}
