use super::*;

impl Expander {
    pub(in crate::macro_expand) fn expand_form(&mut self, form: &Form, depth: usize) -> Form {
        match &form.kind {
            FormKind::List(items) => self.expand_list(form, items, depth),
            FormKind::Vector(items) => Self::with_kind(
                form,
                FormKind::Vector(
                    items
                        .iter()
                        .map(|item| self.expand_form(item, depth))
                        .collect(),
                ),
            ),
            FormKind::Map(items) => Self::with_kind(
                form,
                FormKind::Map(
                    items
                        .iter()
                        .map(|item| self.expand_form(item, depth))
                        .collect(),
                ),
            ),
            FormKind::Set(items) => Self::with_kind(
                form,
                FormKind::Set(
                    items
                        .iter()
                        .map(|item| self.expand_form(item, depth))
                        .collect(),
                ),
            ),
            // Quoted data and phase-1 templates are not runtime macro calls.
            FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::Quote | ReaderMacroKind::SyntaxQuote,
                ..
            } => form.clone(),
            FormKind::ReaderMacro {
                macro_kind,
                form: body,
            } => Self::with_kind(
                form,
                FormKind::ReaderMacro {
                    macro_kind: *macro_kind,
                    form: Box::new(self.expand_form(body, depth)),
                },
            ),
            _ => form.clone(),
        }
    }

    /// Expand one module-level form while preserving the dependency graph
    /// boundary. Header and phase declarations are authored compiler inputs;
    /// allowing a macro to create one after graph construction would make
    /// imports and phase-1 bindings differ between passes.
    pub(in crate::macro_expand) fn expand_top_level_forms(&mut self, form: &Form) -> Vec<Form> {
        if top_level_boundary_head(form).is_some() {
            return vec![form.clone()];
        }

        let trace_start = self.traces.len();
        let expanded = self.expand_form(form, 0);
        if self.traces.len() > trace_start {
            if let Some(head) = generated_top_level_boundary_head(&expanded) {
                self.diagnostics.push(Diagnostic::error(
                    "OSR-M0008",
                    format!("macro expansion cannot generate top-level declaration `{head}`"),
                    form.span,
                ));
                return vec![error_form(
                    "macro-generated top-level declaration",
                    form.span,
                )];
            }
            if let Some(declarations) = generated_declaration_sequence(&expanded) {
                return declarations;
            }
        }
        vec![expanded]
    }

    pub(in crate::macro_expand) fn expand_list(
        &mut self,
        form: &Form,
        items: &[Form],
        depth: usize,
    ) -> Form {
        let Some(head) = items.first().and_then(symbol_canonical) else {
            return self.expand_list_children(form, items, depth);
        };

        if is_phase_one_declaration(head) {
            return form.clone();
        }

        let short_name = head.rsplit('/').next().unwrap_or(head);
        let user_macro = self
            .macros
            .get(head)
            .filter(|definition| !definition.imported)
            .or_else(|| {
                self.macros
                    .get(short_name)
                    .filter(|definition| !definition.imported)
            })
            .or_else(|| {
                self.macro_exports
                    .get(head)
                    .and_then(|target| self.macros.get(target))
            })
            .cloned();
        if user_macro.is_none() {
            return self.expand_list_children(form, items, depth);
        }
        if self.expansions >= self.options.max_expansions {
            self.diagnostics.push(Diagnostic::error(
                "OSR-M0002",
                format!(
                    "macro expansion exceeded the limit of {} calls",
                    self.options.max_expansions
                ),
                form.span,
            ));
            return error_form("macro expansion limit", form.span);
        }
        self.expansions += 1;
        let expanded = match self.evaluate_macro(
            user_macro.as_ref().expect("macro presence checked"),
            form,
            &items[1..],
        ) {
            Ok(expanded) => Some(expanded),
            Err(error) => {
                self.diagnostics
                    .push(Diagnostic::error(error.code, error.message, error.span));
                Some(error_form("phase-1 evaluation failed", form.span))
            }
        };
        let Some(mut expanded) = expanded else {
            return self.expand_list_children(form, items, depth);
        };
        if form_node_count(&expanded) > DEFAULT_MAX_RESULT_NODES {
            self.diagnostics.push(Diagnostic::error(
                "OSR-M0006",
                format!(
                    "macro expansion result exceeded the limit of {DEFAULT_MAX_RESULT_NODES} forms"
                ),
                form.span,
            ));
            return error_form("macro expansion result limit", form.span);
        }
        expanded.metadata = merge_call_metadata(&form.metadata, &expanded.metadata);
        if let Err(exceeded) = check_metadata_resources(&expanded.metadata, METADATA_TARGET_LIMITS)
        {
            self.diagnostics.push(Diagnostic::error(
                "OSR-M0009",
                format!(
                    "metadata for one syntax target exceeds the {} limit of {} (found {})",
                    exceeded.resource, exceeded.limit, exceeded.actual
                ),
                form.span,
            ));
            return error_form("macro expansion metadata limit", form.span);
        }
        expanded.span = form.span;
        expanded.datum_span = form.datum_span;
        let mut origin = self.active_origins.clone();
        origin.push(form.span);
        self.traces.push(ExpansionTrace {
            macro_name: short_name.to_owned(),
            macro_binding_id: user_macro
                .as_ref()
                .and_then(|definition| definition.macro_binding_id.clone())
                .expect("every collected macro has a stable binding id"),
            call_span: form.span,
            expansion_span: expanded.span,
            depth,
            origin,
        });

        if self.options.once {
            expanded
        } else {
            self.active_origins.push(form.span);
            let recursively_expanded = self.expand_form(&expanded, depth + 1);
            self.active_origins.pop();
            recursively_expanded
        }
    }

    pub(in crate::macro_expand) fn expand_list_children(
        &mut self,
        form: &Form,
        items: &[Form],
        depth: usize,
    ) -> Form {
        Self::with_kind(
            form,
            FormKind::List(
                items
                    .iter()
                    .map(|item| self.expand_form(item, depth))
                    .collect(),
            ),
        )
    }
}
