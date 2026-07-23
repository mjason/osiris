use super::*;

pub(super) fn locate_interface_graph_error(
    error: &InterfaceGraphHashError,
    prepared: &[PreparedInput],
) -> LocatedDiagnostic {
    let module = match error {
        InterfaceGraphHashError::DuplicateProvider(module)
        | InterfaceGraphHashError::UnknownImporter(module)
        | InterfaceGraphHashError::InvalidHash { owner: module, .. }
        | InterfaceGraphHashError::ComponentCycle(module) => Some(module.as_str()),
        InterfaceGraphHashError::MissingDependency { from, .. } => Some(from.as_str()),
        InterfaceGraphHashError::EmptyModule | InterfaceGraphHashError::InvalidGroup(_) => None,
    };
    let unit = module
        .and_then(|module| prepared.iter().find(|unit| unit.module_name == module))
        .or_else(|| prepared.first());
    LocatedDiagnostic {
        input_index: unit.map_or(0, |unit| unit.input_index),
        diagnostic: Diagnostic::error(
            "OSR-G0012",
            error.to_string(),
            unit.map_or_else(|| crate::source::Span::empty(0), |unit| unit.header.span),
        ),
    }
}

pub(super) fn analysis_hashes(document: &Document, options: &CompileOptions) -> (String, String) {
    let source = document
        .tokens
        .iter()
        .map(|token| token.text.as_str())
        .collect::<String>();
    let source_hash = hash_fields([source.as_str()]);
    let target_python = options.target_python.to_string();
    let cache_key = hash_fields([
        "osiris-analysis-cache-v1",
        interface::COMPILER_ABI,
        interface::LANGUAGE_ABI,
        &source_hash,
        &options.fallback_module_name,
        &target_python,
        &options.trust_policy.hash,
    ]);
    (source_hash, cache_key)
}

pub(super) fn build_hash(
    analysis: &Analysis,
    options: &CompileOptions,
    interface: Option<&interface::Interface>,
) -> String {
    let target_python = options.target_python.to_string();
    hash_fields([
        "osiris-build-v1",
        interface::COMPILER_ABI,
        interface::LANGUAGE_ABI,
        &analysis.cache_key,
        &analysis.source_hash,
        &analysis.hir.trust_policy_hash,
        &target_python,
        interface.map_or("none", interface::Interface::semantic_interface_hash),
    ])
}

pub(super) fn hash_fields<'a>(fields: impl IntoIterator<Item = &'a str>) -> String {
    let mut hasher = Sha256::new();
    for field in fields {
        hasher.update(field.len().to_be_bytes());
        hasher.update(field.as_bytes());
    }
    format!("sha256:{:x}", hasher.finalize())
}

pub(super) fn install_module_identity(
    module: &mut ast::Module,
    options: &CompileOptions,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(name) = &module.name else {
        module.name = Some(Name {
            spelling: options.fallback_module_name.clone(),
            canonical: options.fallback_module_name.clone(),
        });
        return;
    };
    let Some(expected) = &options.expected_module_name else {
        return;
    };
    if name.canonical != *expected {
        diagnostics.push(Diagnostic::error(
            "OSR-G0011",
            format!(
                "module declaration `{}` does not match source path module `{expected}`",
                name.spelling
            ),
            module.span,
        ));
    }
}

pub(super) fn imported_phase_modules(
    module: &ast::Module,
    interfaces: &BTreeMap<String, interface::Interface>,
) -> Vec<ImportedPhaseModule> {
    let mut modules = BTreeMap::<String, ImportedPhaseModule>::new();
    for item in &module.items {
        let import = match &item.kind {
            ast::ItemKind::Import(import) | ast::ItemKind::ImportForSyntax(import) => import,
            _ => continue,
        };
        let Some(interface) = interfaces.get(&import.module.canonical) else {
            continue;
        };
        if interface.macros.is_empty() {
            continue;
        }
        let descriptor = modules.entry(interface.module.clone()).or_insert_with(|| {
            ImportedPhaseModule::new(
                interface.module.clone(),
                interface.imported_phase_forms(),
                BTreeMap::new(),
            )
            .with_definition_names(
                interface
                    .bindings
                    .iter()
                    .map(|binding| (binding.canonical.clone(), binding.canonical.clone()))
                    .chain(
                        interface
                            .macros
                            .iter()
                            .map(|macro_| (macro_.canonical.clone(), macro_.canonical.clone())),
                    )
                    .collect(),
            )
        });
        let qualifier = import
            .alias
            .as_ref()
            .map_or(import.module.canonical.as_str(), |alias| {
                alias.canonical.as_str()
            });
        for imported_macro in &interface.macros {
            descriptor.macro_names.insert(
                format!("{qualifier}/{}", imported_macro.canonical),
                imported_macro.canonical.clone(),
            );
            descriptor.macro_names.insert(
                format!("{qualifier}.{}", imported_macro.canonical),
                imported_macro.canonical.clone(),
            );
        }
        for member in &import.members {
            if let Some(imported_macro) = interface
                .macros
                .iter()
                .find(|imported_macro| imported_macro.canonical == member.canonical)
            {
                descriptor
                    .macro_names
                    .insert(member.canonical.clone(), imported_macro.canonical.clone());
                descriptor
                    .macro_names
                    .insert(member.spelling.clone(), imported_macro.canonical.clone());
            }
        }
    }
    modules.into_values().collect()
}

pub(super) fn locate_graph_error(
    error: &ModuleGraphError,
    prepared: &[PreparedInput],
) -> LocatedDiagnostic {
    let (code, module, explicit_span) = match error {
        ModuleGraphError::UnnamedModule { span } => ("OSR-G0001", None, Some(*span)),
        ModuleGraphError::DuplicateModule { module, second, .. } => {
            ("OSR-G0002", Some(module.as_str()), *second)
        }
        ModuleGraphError::MissingModule { from, span, .. } => {
            ("OSR-G0003", Some(from.as_str()), Some(*span))
        }
        ModuleGraphError::InterfaceIo { .. } => ("OSR-G0004", None, None),
        ModuleGraphError::InterfaceParse { .. } => ("OSR-G0005", None, None),
        ModuleGraphError::InterfaceModuleMismatch { requested, .. } => {
            ("OSR-G0006", Some(requested.as_str()), None)
        }
        ModuleGraphError::DuplicateInterface { module, .. } => {
            ("OSR-G0007", Some(module.as_str()), None)
        }
        ModuleGraphError::Phase1Cycle { modules } => {
            ("OSR-G0008", modules.first().map(String::as_str), None)
        }
    };
    let unit = module
        .and_then(|module| {
            prepared
                .iter()
                .rev()
                .find(|unit| unit.module_name == module)
        })
        .or_else(|| {
            explicit_span.and_then(|span| {
                prepared.iter().find(|unit| {
                    unit.header.span.start <= span.start && span.end <= unit.header.span.end
                })
            })
        })
        .or_else(|| prepared.first());
    LocatedDiagnostic {
        input_index: unit.map_or(0, |unit| unit.input_index),
        diagnostic: Diagnostic::error(
            code,
            error.to_string(),
            explicit_span
                .or_else(|| unit.map(|unit| unit.header.span))
                .unwrap_or_else(|| crate::source::Span::empty(0)),
        ),
    }
}

pub(super) fn sort_located_diagnostics(diagnostics: &mut [LocatedDiagnostic]) {
    diagnostics.sort_by(|left, right| {
        (
            left.input_index,
            left.diagnostic.span.start,
            left.diagnostic.span.end,
            left.diagnostic.code,
            &left.diagnostic.message,
        )
            .cmp(&(
                right.input_index,
                right.diagnostic.span.start,
                right.diagnostic.span.end,
                right.diagnostic.code,
                &right.diagnostic.message,
            ))
    });
}

#[must_use]
pub fn python_module_path(module_name: &str) -> PathBuf {
    let mut path = PathBuf::new();
    for component in module_name.split(['/', '.']) {
        path.push(python_identifier(component));
    }
    path.set_extension("py");
    path
}

pub(super) fn sort_diagnostics(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by(|left, right| {
        (left.span.start, left.span.end, left.code, &left.message).cmp(&(
            right.span.start,
            right.span.end,
            right.code,
            &right.message,
        ))
    });
}
