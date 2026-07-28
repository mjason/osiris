use super::*;

impl Expander {
    /// Substitute a template's metadata before its datum.
    ///
    /// `with_kind` carries `original.metadata` through verbatim, so an unquote
    /// written inside `^{...}` would otherwise survive expansion unsubstituted
    /// and be rejected as non-serializable. Metadata is ordinary template
    /// content — `^{:doc {:default ~text}}` is the common case — so it is
    /// quoted first and the datum is then processed against the result.
    pub(in crate::macro_expand) fn syntax_quote(
        &mut self,
        form: &Form,
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
        context: &mut QuoteContext,
    ) -> Result<Form, EvalError> {
        if form.metadata.is_empty() {
            return self.syntax_quote_datum(form, environment, budget, depth, context);
        }
        let mut entries = Vec::with_capacity(form.metadata.len());
        for entry in &form.metadata {
            entries.push(MetadataEntry {
                key: self.syntax_quote(&entry.key, environment, budget, depth + 1, context)?,
                value: self.syntax_quote(&entry.value, environment, budget, depth + 1, context)?,
            });
        }
        let mut substituted = form.clone();
        substituted.metadata = entries;
        self.syntax_quote_datum(&substituted, environment, budget, depth, context)
    }

    fn syntax_quote_datum(
        &mut self,
        form: &Form,
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
        context: &mut QuoteContext,
    ) -> Result<Form, EvalError> {
        tick_budget(budget, depth, form.span)?;
        match &form.kind {
            FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::Unquote,
                form: expression,
            } => {
                let mut expanded = self
                    .eval(expression, environment, budget, depth + 1)?
                    .into_data(form.span)?;
                expanded.metadata = merge_call_metadata(&form.metadata, &expanded.metadata);
                Ok(expanded)
            }
            FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::UnquoteSplicing,
                ..
            } => Err(EvalError::evaluation(
                "unquote-splicing is only valid inside a syntax-quoted collection",
                form.span,
            )),
            FormKind::List(items) => {
                if let Some(quoted) = self.syntax_quote_binding_form(
                    form,
                    items,
                    environment,
                    budget,
                    depth,
                    context,
                )? {
                    return Ok(quoted);
                }
                self.syntax_quote_collection(items, environment, budget, depth + 1, context)
                    .map(|items| Self::with_kind(form, FormKind::List(items)))
            }
            FormKind::Vector(items) => self
                .syntax_quote_collection(items, environment, budget, depth + 1, context)
                .map(|items| Self::with_kind(form, FormKind::Vector(items))),
            FormKind::Map(items) => {
                let items =
                    self.syntax_quote_collection(items, environment, budget, depth + 1, context)?;
                if items.len() % 2 != 0 {
                    return Err(EvalError::evaluation(
                        "syntax-quoted map contains an odd number of forms after splicing",
                        form.span,
                    ));
                }
                Ok(Self::with_kind(form, FormKind::Map(items)))
            }
            FormKind::Set(items) => self
                .syntax_quote_collection(items, environment, budget, depth + 1, context)
                .map(|items| Self::with_kind(form, FormKind::Set(items))),
            FormKind::Symbol(name) if name.canonical.ends_with('#') => {
                if let Some(existing) = context.generated.get(&name.canonical) {
                    return Ok(Self::with_kind(form, existing.kind.clone()));
                }
                let hint = name.canonical.trim_end_matches('#');
                let generated_symbol = self.generated_symbol(hint, form.span);
                context
                    .generated
                    .insert(name.canonical.clone(), generated_symbol.clone());
                Ok(Self::with_kind(form, generated_symbol.kind))
            }
            FormKind::Symbol(name) => {
                // A name the template itself binds keeps the hygienic identity
                // created for that binding, so caller syntax spliced into the
                // same template can never be captured by it.
                if let Some(generated) = context.resolve(&name.canonical) {
                    return Ok(Self::with_kind(form, generated.kind.clone()));
                }
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
        context: &mut QuoteContext,
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
                quoted.push(self.syntax_quote(item, environment, budget, depth, context)?);
            }
        }
        Ok(quoted)
    }

    /// Quote a kernel `let` or `fn` whose binding names are authored inside the
    /// template. Those names become fresh hygienic identities, which is what
    /// makes macro-created bindings hygienic without an explicit `name#`.
    ///
    /// Returns `None` for every shape this pass cannot analyse statically —
    /// spliced binding vectors and map destructuring — so those keep the plain
    /// template behaviour instead of being renamed on a guess.
    fn syntax_quote_binding_form(
        &mut self,
        form: &Form,
        items: &[Form],
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
        context: &mut QuoteContext,
    ) -> Result<Option<Form>, EvalError> {
        let Some(head) = items.first().and_then(symbol_canonical) else {
            return Ok(None);
        };
        if !matches!(head, "let" | "fn") {
            return Ok(None);
        }
        // `let` and `fn` both carry their binders in the second position; the
        // head itself stays literal because it names a kernel form.
        const BINDING_INDEX: usize = 1;
        let Some(binder) = items.get(BINDING_INDEX) else {
            return Ok(None);
        };
        let FormKind::Vector(binders) = &binder.kind else {
            return Ok(None);
        };
        if binders.iter().any(is_unquote_splicing) {
            return Ok(None);
        }
        if head == "let" && binders.len() % 2 != 0 {
            return Ok(None);
        }

        context.scopes.push(BTreeMap::new());
        let quoted = (|| {
            let mut quoted_binders = Vec::with_capacity(binders.len());
            if head == "let" {
                // `let` is sequential: an initializer sees only the names bound
                // before it, so quote the initializer before declaring its name.
                for pair in binders.chunks(2) {
                    let value =
                        self.syntax_quote(&pair[1], environment, budget, depth + 1, context)?;
                    let pattern = self.syntax_quote_binder(
                        &pair[0],
                        environment,
                        budget,
                        depth + 1,
                        context,
                    )?;
                    quoted_binders.push(pattern);
                    quoted_binders.push(value);
                }
            } else {
                for parameter in binders {
                    quoted_binders.push(self.syntax_quote_binder(
                        parameter,
                        environment,
                        budget,
                        depth + 1,
                        context,
                    )?);
                }
            }

            let mut quoted = Vec::with_capacity(items.len());
            quoted.extend(items[..BINDING_INDEX].iter().cloned());
            quoted.push(Self::with_kind(binder, FormKind::Vector(quoted_binders)));
            quoted.extend(self.syntax_quote_collection(
                &items[BINDING_INDEX + 1..],
                environment,
                budget,
                depth + 1,
                context,
            )?);
            Ok(Self::with_kind(form, FormKind::List(quoted)))
        })();
        context.scopes.pop();
        quoted.map(Some)
    }

    /// Quote one binding position, declaring the hygienic identity of every
    /// plain symbol it introduces. Unquoted binders stay untouched: the macro
    /// author already chose that identity, and `~'name` is the explicit
    /// operation for reaching a call-site name.
    fn syntax_quote_binder(
        &mut self,
        form: &Form,
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
        context: &mut QuoteContext,
    ) -> Result<Form, EvalError> {
        tick_budget(budget, depth, form.span)?;
        match &form.kind {
            // `&` is the variadic marker, not a bound name.
            FormKind::Symbol(name) if name.canonical == "&" => Ok(form.clone()),
            // `name#` already carries a hygienic identity.
            FormKind::Symbol(name) if name.canonical.ends_with('#') => {
                self.syntax_quote(form, environment, budget, depth, context)
            }
            FormKind::Symbol(name) => {
                if let Some(generated) = context.resolve(&name.canonical) {
                    return Ok(Self::with_kind(form, generated.kind.clone()));
                }
                let generated = self.generated_symbol(&name.canonical, form.span);
                context.declare(name.canonical.clone(), generated.clone());
                Ok(Self::with_kind(form, generated.kind))
            }
            // Sequential destructuring binds every element, including the name
            // after `:as` and the rest name after `&`.
            FormKind::Vector(items) if !items.iter().any(is_unquote_splicing) => {
                let mut quoted = Vec::with_capacity(items.len());
                for item in items {
                    quoted.push(self.syntax_quote_binder(
                        item,
                        environment,
                        budget,
                        depth + 1,
                        context,
                    )?);
                }
                Ok(Self::with_kind(form, FormKind::Vector(quoted)))
            }
            _ => self.syntax_quote(form, environment, budget, depth, context),
        }
    }

    pub(in crate::macro_expand) fn generated_symbol(&mut self, hint: &str, span: Span) -> Form {
        let id = self.next_generated_name;
        self.next_generated_name += 1;
        let spelling = format!("{hint}__osr_g{id}");
        Form::new(
            FormKind::Symbol(Name {
                spelling,
                // The reader cannot produce a canonical name holding a control
                // character, and `symbol`/`keyword` reject one, so this
                // identity stays separate from every authored spelling.
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

fn is_unquote_splicing(form: &Form) -> bool {
    matches!(
        &form.kind,
        FormKind::ReaderMacro {
            macro_kind: ReaderMacroKind::UnquoteSplicing,
            ..
        }
    )
}
