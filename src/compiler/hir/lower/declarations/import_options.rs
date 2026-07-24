use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn validate_interface_exclusions(
        &mut self,
        import: &ast::Import,
        exports: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let mut excluded = BTreeSet::new();
        for name in &import.excluded {
            if !exports.contains(&name.canonical) {
                self.error(
                    "OSR-H0011",
                    format!(
                        "module `{}` does not export excluded member `{}`",
                        import.module.canonical, name.spelling
                    ),
                    member_span(name, import.span),
                );
            } else if !excluded.insert(name.canonical.clone()) {
                self.error(
                    "OSR-H0013",
                    format!("duplicate excluded import member `{}`", name.spelling),
                    import.span,
                );
            }
        }
        excluded
    }

    pub(super) fn validate_interface_renames(
        &mut self,
        import: &ast::Import,
        exports: &BTreeSet<String>,
        excluded: &BTreeSet<String>,
    ) -> BTreeMap<String, String> {
        let mut renamed = BTreeMap::new();
        let mut local_names = BTreeSet::new();
        for rename in &import.renamed {
            let canonical = &rename.canonical.canonical;
            if !exports.contains(canonical) {
                self.error(
                    "OSR-H0011",
                    format!(
                        "module `{}` does not export renamed member `{}`",
                        import.module.canonical, rename.canonical.spelling
                    ),
                    import.span,
                );
                continue;
            }
            if excluded.contains(canonical) {
                self.error(
                    "OSR-H0014",
                    format!(
                        "excluded import member `{}` cannot also be renamed",
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
                    "OSR-H0013",
                    format!("duplicate rename for `{}`", rename.canonical.spelling),
                    import.span,
                );
            }
            if !local_names.insert(rename.local.canonical.clone()) {
                self.error(
                    "OSR-H0015",
                    format!("duplicate import local name `{}`", rename.local.spelling),
                    import.span,
                );
            }
        }
        renamed
    }

    pub(super) fn validate_interface_refers(
        &mut self,
        import: &ast::Import,
        exports: &BTreeSet<String>,
        excluded: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let mut referred = if import.refer_all {
            exports.clone()
        } else {
            BTreeSet::new()
        };
        let mut explicit = BTreeSet::new();
        for member in &import.members {
            if !exports.contains(&member.canonical) {
                self.error(
                    "OSR-H0011",
                    format!(
                        "module `{}` does not export imported member `{}`",
                        import.module.canonical, member.spelling
                    ),
                    member_span(member, import.span),
                );
                continue;
            }
            if !explicit.insert(member.canonical.clone()) {
                self.error(
                    "OSR-H0013",
                    format!("duplicate referred import member `{}`", member.spelling),
                    import.span,
                );
            }
            referred.insert(member.canonical.clone());
        }
        referred.retain(|name| !excluded.contains(name));
        referred
    }
}
