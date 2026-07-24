use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn predeclare_standard_import(&mut self, import: &ast::Import) {
        let namespace = import.module.canonical.as_str();
        let interface = match crate::stdlib::interface_artifact(namespace) {
            Ok(interface) => interface,
            Err(message) => {
                self.error(
                    "OSR-H0022",
                    format!("cannot load standard namespace `{namespace}`: {message}"),
                    import.span,
                );
                return;
            }
        };
        let exports = interface
            .bindings
            .iter()
            .map(|binding| (binding.canonical.clone(), binding.kind))
            .chain(
                interface
                    .macros
                    .iter()
                    .map(|macro_| (macro_.canonical.clone(), BindingKind::Macro)),
            )
            .collect::<BTreeMap<_, _>>();

        let excluded = self.validate_standard_exclusions(import, &exports);
        let renamed = self.validate_standard_renames(import, &exports, &excluded);
        let referred = self.validate_standard_refers(import, &exports, &excluded);
        let qualifier = import
            .alias
            .as_ref()
            .map_or(namespace, |alias| alias.canonical.as_str());

        for public in &interface.bindings {
            if excluded.contains(&public.canonical) {
                continue;
            }
            let Some(id) = self.install_standard_interface_binding(public, &interface, import.span)
            else {
                continue;
            };
            for qualified in [
                format!("{qualifier}/{}", public.canonical),
                format!("{qualifier}.{}", public.canonical),
                format!("{namespace}/{}", public.canonical),
                format!("{namespace}.{}", public.canonical),
            ] {
                self.qualified_imports.insert(qualified, id.clone());
            }
        }

        for canonical in referred {
            if exports.get(&canonical) == Some(&BindingKind::Macro) {
                continue;
            }
            let Some(public) = interface
                .bindings
                .iter()
                .find(|binding| binding.canonical == canonical)
            else {
                continue;
            };
            let local = renamed.get(&canonical).unwrap_or(&canonical).clone();
            let Some(id) = self.install_standard_interface_binding(public, &interface, import.span)
            else {
                continue;
            };
            if let Some(existing) = self.globals.get(&local) {
                self.error(
                    "OSR-N0003",
                    format!(
                        "standard import name `{local}` conflicts with binding `{}`",
                        existing.as_str()
                    ),
                    import.span,
                );
                continue;
            }
            self.globals.insert(local.clone(), id.clone());
            if local != canonical {
                self.aliases.push(Alias {
                    spelling: local.clone(),
                    canonical: local,
                    target: id,
                    span: import.span,
                    public: false,
                });
            }
        }
    }

    fn install_standard_interface_binding(
        &mut self,
        public: &PublicBinding,
        interface: &Interface,
        span: Span,
    ) -> Option<BindingId> {
        let id = self.install_imported_binding(public, interface, None, &[], span)?;
        if public.kind == BindingKind::Type && interface.module == crate::stdlib::CORE_NAMESPACE {
            if let Some(binding) = self.bindings.get_mut(&id) {
                binding.runtime = Some(RuntimeBinding {
                    module: "osiris.kernel".to_owned(),
                    name: public.python.clone(),
                    python_module: false,
                });
            }
        }
        Some(id)
    }

    fn validate_standard_exclusions(
        &mut self,
        import: &ast::Import,
        exports: &BTreeMap<String, BindingKind>,
    ) -> BTreeSet<String> {
        let mut excluded = BTreeSet::new();
        for name in &import.excluded {
            if !exports.contains_key(&name.canonical) {
                self.error(
                    "OSR-H0023",
                    format!(
                        "standard namespace `{}` does not export excluded name `{}`",
                        import.module.canonical, name.spelling
                    ),
                    import.span,
                );
            } else if !excluded.insert(name.canonical.clone()) {
                self.error(
                    "OSR-H0024",
                    format!("duplicate excluded standard name `{}`", name.spelling),
                    import.span,
                );
            }
        }
        excluded
    }

    fn validate_standard_renames(
        &mut self,
        import: &ast::Import,
        exports: &BTreeMap<String, BindingKind>,
        excluded: &BTreeSet<String>,
    ) -> BTreeMap<String, String> {
        let mut renamed = BTreeMap::new();
        let mut local_names = BTreeSet::new();
        for rename in &import.renamed {
            let canonical = &rename.canonical.canonical;
            if !exports.contains_key(canonical) {
                self.error(
                    "OSR-H0023",
                    format!(
                        "standard namespace `{}` does not export renamed name `{}`",
                        import.module.canonical, rename.canonical.spelling
                    ),
                    import.span,
                );
                continue;
            }
            if excluded.contains(canonical) {
                self.error(
                    "OSR-H0025",
                    format!(
                        "excluded standard name `{}` cannot also be renamed",
                        rename.canonical.spelling
                    ),
                    import.span,
                );
                continue;
            }
            if renamed
                .insert(canonical.clone(), rename.local.canonical.clone())
                .is_some()
            {
                self.error(
                    "OSR-H0024",
                    format!("duplicate rename for `{}`", rename.canonical.spelling),
                    import.span,
                );
            }
            if !local_names.insert(rename.local.canonical.clone()) {
                self.error(
                    "OSR-H0026",
                    format!(
                        "duplicate standard import local name `{}`",
                        rename.local.spelling
                    ),
                    import.span,
                );
            }
        }
        renamed
    }

    fn validate_standard_refers(
        &mut self,
        import: &ast::Import,
        exports: &BTreeMap<String, BindingKind>,
        excluded: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let mut referred = if import.refer_all {
            exports.keys().cloned().collect()
        } else {
            BTreeSet::new()
        };
        let mut explicit = BTreeSet::new();
        for name in &import.members {
            if !exports.contains_key(&name.canonical) {
                self.error(
                    "OSR-H0023",
                    format!(
                        "standard namespace `{}` does not export referred name `{}`",
                        import.module.canonical, name.spelling
                    ),
                    import.span,
                );
                continue;
            }
            if !explicit.insert(name.canonical.clone()) {
                self.error(
                    "OSR-H0024",
                    format!("duplicate referred standard name `{}`", name.spelling),
                    import.span,
                );
            }
            referred.insert(name.canonical.clone());
        }
        referred.retain(|name| !excluded.contains(name));
        referred
    }
}
