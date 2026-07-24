use super::*;

impl Expander {
    pub(in crate::macro_expand) fn new(
        options: ExpansionOptions,
        local_module_name: impl Into<String>,
    ) -> Self {
        Self {
            options,
            local_module_name: local_module_name.into(),
            expansions: 0,
            next_generated_name: 0,
            macros: BTreeMap::new(),
            macro_exports: BTreeMap::new(),
            phase_functions: BTreeMap::new(),
            definition_names: BTreeMap::new(),
            active_phase_namespace: None,
            active_origins: Vec::new(),
            diagnostics: Vec::new(),
            traces: Vec::new(),
        }
    }

    pub(in crate::macro_expand) fn collect_phase_one_declarations(&mut self, forms: &[Form]) {
        let module_name = self.local_module_name.clone();
        self.collect_phase_one_declarations_scoped(forms, None, &module_name);
    }

    pub(in crate::macro_expand) fn collect_imported_phase_modules(
        &mut self,
        modules: &[ImportedPhaseModule],
    ) {
        let mut grouped = BTreeMap::<&str, Vec<&ImportedPhaseModule>>::new();
        for module in modules {
            if module.namespace.trim().is_empty() {
                self.diagnostics.push(Diagnostic::error(
                    "OSR-M0003",
                    "imported phase-1 module requires a non-empty namespace",
                    module
                        .forms
                        .first()
                        .map_or(Span::default(), |form| form.span),
                ));
                continue;
            }
            grouped
                .entry(module.namespace.as_str())
                .or_default()
                .push(module);
        }

        for (namespace, group) in grouped {
            let span = group
                .iter()
                .flat_map(|module| module.forms.iter())
                .map(|form| form.span)
                .min_by_key(|span| (span.start, span.end))
                .unwrap_or_default();
            let forms = group[0].forms.as_slice();
            if group
                .iter()
                .skip(1)
                .any(|module| module.forms.as_slice() != forms)
            {
                self.diagnostics.push(Diagnostic::error(
                    "OSR-M0003",
                    format!(
                        "imported phase-1 namespace `{namespace}` was loaded with inconsistent declarations"
                    ),
                    span,
                ));
                continue;
            }
            let definition_names = group[0].definition_names.clone();
            if group
                .iter()
                .skip(1)
                .any(|module| module.definition_names != definition_names)
            {
                self.diagnostics.push(Diagnostic::error(
                    "OSR-M0003",
                    format!(
                        "imported phase-1 namespace `{namespace}` was loaded with inconsistent definition names"
                    ),
                    span,
                ));
                continue;
            }
            self.definition_names
                .insert(namespace.to_owned(), definition_names);

            let mut macro_names = BTreeMap::<String, String>::new();
            let mut conflicting_names = BTreeSet::new();
            for module in group {
                for (visible, target) in &module.macro_names {
                    match macro_names.get(visible) {
                        Some(existing) if existing != target => {
                            conflicting_names.insert(visible.clone());
                        }
                        Some(_) => {}
                        None => {
                            macro_names.insert(visible.clone(), target.clone());
                        }
                    }
                }
            }
            for name in conflicting_names {
                macro_names.remove(&name);
                self.diagnostics.push(Diagnostic::error(
                    "OSR-M0003",
                    format!(
                        "imported phase-1 namespace `{namespace}` exposes `{name}` with conflicting targets"
                    ),
                    span,
                ));
            }

            let definitions =
                self.collect_phase_one_declarations_scoped(forms, Some(namespace), namespace);
            for (visible, target) in macro_names {
                let internal = definitions.get(&target).cloned().or_else(|| {
                    let target_short = target.rsplit('/').next().unwrap_or(&target);
                    let mut matches = definitions
                        .iter()
                        .filter(|(source, _)| {
                            source.rsplit('/').next().unwrap_or(source) == target_short
                        })
                        .map(|(_, internal)| internal.clone());
                    let first = matches.next()?;
                    matches.next().is_none().then_some(first)
                });
                let Some(internal) = internal else {
                    self.diagnostics.push(Diagnostic::error(
                        "OSR-M0003",
                        format!(
                            "imported phase-1 namespace `{namespace}` exposes unknown or ambiguous macro `{target}`"
                        ),
                        span,
                    ));
                    continue;
                };
                if let Some(existing) = self.macro_exports.get(&visible) {
                    if existing != &internal {
                        self.diagnostics.push(Diagnostic::error(
                            "OSR-M0003",
                            format!("imported macro name `{visible}` has conflicting definitions"),
                            span,
                        ));
                    }
                    continue;
                }
                if self
                    .macros
                    .get(&visible)
                    .is_some_and(|definition| !definition.imported)
                {
                    self.diagnostics.push(Diagnostic::error(
                        "OSR-M0003",
                        format!("imported macro name `{visible}` conflicts with a local macro"),
                        span,
                    ));
                    continue;
                }
                self.macro_exports.insert(visible, internal);
            }
        }
    }

    pub(in crate::macro_expand) fn collect_phase_one_declarations_scoped(
        &mut self,
        forms: &[Form],
        namespace: Option<&str>,
        binding_module: &str,
    ) -> BTreeMap<String, String> {
        let mut imported_macros = BTreeMap::new();
        for form in forms {
            let FormKind::List(items) = &form.kind else {
                continue;
            };
            let Some(head) = items.first().and_then(symbol_canonical) else {
                continue;
            };
            let kind = match head {
                "defmacro" => PhaseDeclarationKind::Macro,
                "defn-for-syntax" => PhaseDeclarationKind::Function,
                _ => continue,
            };
            let mut definition = match parse_phase_declaration(form, kind) {
                Ok(definition) => definition,
                Err(message) => {
                    self.diagnostics
                        .push(Diagnostic::error("OSR-M0003", message, form.span));
                    continue;
                }
            };
            if let Some(namespace) = namespace {
                definition.name = scoped_phase_name(namespace, &definition.source_name);
                definition.namespace = Some(namespace.to_owned());
                definition.imported = true;
            }
            if matches!(kind, PhaseDeclarationKind::Macro) {
                definition.macro_binding_id = Some(
                    BindingId::new(binding_module, &definition.source_name, BindingKind::Macro)
                        .as_str()
                        .to_owned(),
                );
            }
            if self.macros.contains_key(&definition.name)
                || self.phase_functions.contains_key(&definition.name)
            {
                self.diagnostics.push(Diagnostic::error(
                    "OSR-M0003",
                    format!("duplicate phase-1 declaration `{}`", definition.name),
                    definition.span,
                ));
                continue;
            }
            match kind {
                PhaseDeclarationKind::Macro => {
                    imported_macros.insert(definition.source_name.clone(), definition.name.clone());
                    self.macros.insert(definition.name.clone(), definition);
                }
                PhaseDeclarationKind::Function => {
                    self.phase_functions
                        .insert(definition.name.clone(), definition);
                }
            }
        }
        imported_macros
    }
}
