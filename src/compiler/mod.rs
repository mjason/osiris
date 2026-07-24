//! End-to-end compiler orchestration.
//!
//! Individual passes remain usable by tooling, while this module owns pass
//! ordering and the rule that code generation only runs after all semantic
//! gates have succeeded.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use sha2::{Digest, Sha256};

use crate::{
    ast, backend, dependency,
    diagnostic::Diagnostic,
    hir, interface,
    interface_graph::{
        self, InterfaceBodyHashes, InterfaceGraphHashError, InterfaceHashEdge,
        PublishedInterfaceHashes,
    },
    macro_expand::{self, ExpansionOptions, ExpansionTrace, ImportedPhaseModule},
    module_graph::{ModuleGraph, ModuleGraphError},
    name::python_identifier,
    project::PythonVersion,
    reader, records, source_map,
    syntax::{Document, Name},
};

#[derive(Clone, Debug)]
pub struct CompileOptions {
    pub source_name: String,
    pub fallback_module_name: String,
    pub expected_module_name: Option<String>,
    pub distribution: String,
    pub distribution_version: String,
    pub target_python: PythonVersion,
    pub strict: bool,
    pub trust_policy: hir::ContractTrustPolicy,
}

impl CompileOptions {
    #[must_use]
    pub fn new(fallback_module_name: impl Into<String>, target_python: PythonVersion) -> Self {
        let fallback_module_name = fallback_module_name.into();
        Self {
            source_name: format!("{fallback_module_name}.osr"),
            distribution: fallback_module_name.clone(),
            distribution_version: "0".to_owned(),
            fallback_module_name,
            expected_module_name: None,
            target_python,
            strict: true,
            trust_policy: dependency::contract_trust_policy(&[], &[])
                .expect("empty trust policy is valid"),
        }
    }

    #[must_use]
    pub fn with_source_name(mut self, source_name: impl Into<String>) -> Self {
        self.source_name = source_name.into();
        self
    }

    /// Requires an explicit module declaration to match the source-root path.
    #[must_use]
    pub fn with_expected_module_name(mut self, module_name: impl Into<String>) -> Self {
        self.expected_module_name = Some(module_name.into());
        self
    }

    #[must_use]
    pub fn with_provider(
        mut self,
        distribution: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.distribution = distribution.into();
        self.distribution_version = version.into();
        self
    }

    #[must_use]
    pub const fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    #[must_use]
    pub fn with_trust_policy(mut self, trust_policy: hir::ContractTrustPolicy) -> Self {
        self.trust_policy = trust_policy;
        self
    }
}

#[derive(Clone, Debug)]
pub struct Analysis {
    pub document: Document,
    pub expanded_document: Document,
    pub expansion_traces: Vec<ExpansionTrace>,
    pub surface: ast::Module,
    pub hir: hir::Module,
    pub static_data: records::StaticModuleData,
    pub diagnostics: Vec<Diagnostic>,
    pub source_hash: String,
    pub cache_key: String,
}

impl Analysis {
    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.diagnostics.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct CompileResult {
    pub analysis: Analysis,
    pub python: Option<backend::GeneratedPython>,
    pub interface: Option<String>,
    pub source_map: Option<crate::artifact::SourceMap>,
    pub records: Option<records::EncodedSidecar>,
    pub build_hash: String,
}

impl CompileResult {
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.analysis.has_errors()
    }
}

/// One source buffer in a distribution-wide compilation.
#[derive(Clone, Copy, Debug)]
pub struct CompileInput<'source> {
    pub source: &'source str,
    pub options: &'source CompileOptions,
}

impl<'source> CompileInput<'source> {
    #[must_use]
    pub const fn new(source: &'source str, options: &'source CompileOptions) -> Self {
        Self { source, options }
    }
}

/// A diagnostic tied to the input position supplied to [`compile_workspace`].
#[derive(Clone, Debug)]
pub struct LocatedDiagnostic {
    pub input_index: usize,
    pub diagnostic: Diagnostic,
}

/// Result of compiling all source modules as one dependency graph.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceCompileResult {
    /// Successful units are returned in the same order as the inputs.
    pub units: Vec<CompileResult>,
    pub diagnostics: Vec<LocatedDiagnostic>,
}

impl WorkspaceCompileResult {
    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.diagnostics.is_empty()
    }
}

/// Runs every frontend pass and returns a recoverable semantic model.
#[must_use]
pub fn analyze(source: &str, options: &CompileOptions) -> Analysis {
    let document = reader::read(source);
    analyze_document(&document, options, &[], None)
}

fn analyze_document(
    document: &Document,
    options: &CompileOptions,
    imported_phase_modules: &[ImportedPhaseModule],
    interfaces: Option<&BTreeMap<String, interface::Interface>>,
) -> Analysis {
    let (source_hash, cache_key) = analysis_hashes(document, options);
    let expanded = macro_expand::expand_with_imported_phase_modules_for_module(
        document,
        imported_phase_modules,
        &options.fallback_module_name,
        ExpansionOptions::default(),
    );
    let mut surface_result = ast::lower_document(&expanded.document);
    install_module_identity(
        &mut surface_result.module,
        options,
        &mut surface_result.diagnostics,
    );
    if surface_result.module.name.as_ref().is_some_and(|name| {
        name.canonical
            .split('.')
            .any(|component| component == "__osiris_runtime__")
    }) {
        surface_result.diagnostics.push(Diagnostic::error(
            "OSR-C0004",
            "`__osiris_runtime__` is reserved for compiler-linked support",
            surface_result.module.span,
        ));
    }
    let static_data = interfaces.map_or_else(
        || records::analyze_module(&surface_result.module),
        |interfaces| records::analyze_module_with_interfaces(&surface_result.module, interfaces),
    );
    let hir_result = interfaces.map_or_else(
        || {
            hir::lower_module_with_compiler_policy(
                &surface_result.module,
                &options.fallback_module_name,
                &options.trust_policy,
                options.strict,
            )
        },
        |interfaces| {
            hir::lower_module_with_interfaces_and_compiler_policy(
                &surface_result.module,
                &options.fallback_module_name,
                interfaces,
                &options.trust_policy,
                options.strict,
            )
        },
    );

    // Surface lowering carries reader diagnostics forward so standalone AST
    // clients and the end-to-end compiler observe the same recovered errors.
    let mut diagnostics = surface_result.diagnostics;
    diagnostics.extend(static_data.diagnostics.iter().cloned());
    diagnostics.extend(hir_result.diagnostics);
    if diagnostics.is_empty()
        && let Err(error) = interface::build_with_static_data_for_target(
            &hir_result.module,
            &surface_result.module,
            &static_data,
            options.target_python,
        )
    {
        diagnostics.push(Diagnostic::error(
            error.code,
            error.message,
            hir_result.module.span,
        ));
    }
    sort_diagnostics(&mut diagnostics);

    Analysis {
        document: document.clone(),
        expanded_document: expanded.document,
        expansion_traces: expanded.traces,
        surface: surface_result.module,
        hir: hir_result.module,
        static_data,
        diagnostics,
        source_hash,
        cache_key,
    }
}

/// Compiles a source unit to Python after all frontend gates succeed.
#[must_use]
pub fn compile(source: &str, options: &CompileOptions) -> CompileResult {
    let analysis = analyze(source, options);
    finish_compile(analysis, options).0
}

fn finish_compile(
    mut analysis: Analysis,
    options: &CompileOptions,
) -> (CompileResult, Option<interface::Interface>) {
    let interface_model = build_interface_model(&mut analysis, options.target_python);
    finish_compile_with_model(analysis, options, interface_model)
}

fn build_interface_model(
    analysis: &mut Analysis,
    target_python: PythonVersion,
) -> Option<interface::Interface> {
    if analysis.has_errors() {
        return None;
    }
    match interface::build_with_static_data_for_target(
        &analysis.hir,
        &analysis.surface,
        &analysis.static_data,
        target_python,
    ) {
        Ok(interface) => Some(interface),
        Err(error) => {
            analysis.diagnostics.push(Diagnostic::error(
                error.code,
                error.message,
                analysis.hir.span,
            ));
            None
        }
    }
}

fn finish_compile_with_model(
    mut analysis: Analysis,
    options: &CompileOptions,
    interface_model: Option<interface::Interface>,
) -> (CompileResult, Option<interface::Interface>) {
    let build_hash = build_hash(&analysis, options, interface_model.as_ref());
    let interface = interface_model
        .as_ref()
        .and_then(|model| match interface::render(model) {
            Ok(interface) => Some(interface),
            Err(error) => {
                analysis.diagnostics.push(Diagnostic::error(
                    error.code,
                    error.message,
                    analysis.hir.span,
                ));
                None
            }
        });
    let records = interface_model.as_ref().and_then(|model| {
        let indexed = model
            .owned_records
            .iter()
            .map(|record| records::IndexedRecord {
                occurrence: record.occurrence(
                    &options.distribution,
                    &options.distribution_version,
                    &analysis.hir.name,
                    model.semantic_interface_hash(),
                ),
                record: record.clone(),
                dependency_path: vec![analysis.hir.name.clone()],
            })
            .collect::<Vec<_>>();
        if let Err(diagnostics) = records::merge_unique_indexes(indexed.clone()) {
            analysis.diagnostics.extend(diagnostics);
            return None;
        }
        match records::encode_sidecar([model.semantic_interface_hash().to_owned()], indexed) {
            Ok(sidecar) => Some(sidecar),
            Err(error) => {
                analysis.diagnostics.push(Diagnostic::error(
                    error.code,
                    error.message,
                    error.span.unwrap_or(analysis.hir.span),
                ));
                None
            }
        }
    });
    sort_diagnostics(&mut analysis.diagnostics);
    let (python, generated_map) =
        if analysis.has_errors() || interface.is_none() || records.is_none() {
            (None, None)
        } else {
            match backend::compile_module(&analysis.hir, options.target_python) {
                Ok(generated) => {
                    let generated_name = python_module_path(&analysis.hir.name)
                        .to_string_lossy()
                        .into_owned();
                    let map = source_map::generate(source_map::GenerateInput {
                        source_name: &options.source_name,
                        generated_name: &generated_name,
                        generated_source: &generated.source,
                        module: &analysis.hir,
                        traces: &analysis.expansion_traces,
                        python_target: options.target_python,
                        source_hash: &analysis.source_hash,
                        build_hash: &build_hash,
                    });
                    (Some(generated), Some(map))
                }
                Err(error) => {
                    let span = error.span.unwrap_or(analysis.hir.span);
                    analysis
                        .diagnostics
                        .push(Diagnostic::error("OSR-B0001", error.message, span));
                    sort_diagnostics(&mut analysis.diagnostics);
                    (None, None)
                }
            }
        };

    (
        CompileResult {
            analysis,
            python,
            interface,
            source_map: generated_map,
            records,
            build_hash,
        },
        interface_model,
    )
}

mod support;
mod workspace;

pub use support::python_module_path;
use support::*;
use workspace::PreparedInput;
pub use workspace::{analyze_workspace_recovering, compile_workspace};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
