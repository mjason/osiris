use std::collections::{BTreeMap, BTreeSet};

use ruff_python_ast::{
    Expr, Mod, Stmt,
    visitor::{Visitor, walk_expr, walk_stmt},
};
use ruff_python_parser::{Mode, ParseOptions, parse};
use ruff_text_size::Ranged;

use super::*;

pub(super) fn resolve_provider_names(
    module: &mut ast::Module,
    options: &CompileOptions,
    diagnostics: &mut Vec<Diagnostic>,
) {
    resolve_embedded_sources(module, options, diagnostics);
    let module_name = module
        .name
        .as_ref()
        .map_or(options.fallback_module_name.as_str(), |name| {
            name.canonical.as_str()
        });
    let provider_id = provider_id(options);
    let runtime_root = runtime_root(module_name);
    let mut providers = BTreeMap::new();
    let mut lowered_handles = BTreeMap::<String, String>::new();

    for item in &mut module.items {
        let ast::ItemKind::EmbeddedPython(provider) = &mut item.kind else {
            continue;
        };
        let lowered = crate::name::python_identifier(&provider.handle.canonical);
        if let Some(previous) =
            lowered_handles.insert(lowered.clone(), provider.handle.canonical.clone())
            && previous != provider.handle.canonical
        {
            diagnostics.push(Diagnostic::error(
                "OSR-C0005",
                format!(
                    "embedded Python handles `{previous}` and `{}` lower to the same Python module name `{lowered}`",
                    provider.handle.spelling,
                ),
                provider.span,
            ));
        }
        let logical = format!(
            "{runtime_root}.packages.{provider_id}.{}.{}",
            crate::name::python_module_identifier(module_name),
            lowered
        );
        provider.logical_module = Some(logical.clone());
        if providers
            .insert(provider.handle.canonical.clone(), logical)
            .is_some()
        {
            diagnostics.push(Diagnostic::error(
                "OSR-C0005",
                format!(
                    "duplicate embedded Python handle `{}`",
                    provider.handle.spelling
                ),
                provider.span,
            ));
        }
    }

    for item in &mut module.items {
        let ast::ItemKind::Extern(external) = &mut item.kind else {
            continue;
        };
        let Some(handle) = &external.provider_handle else {
            continue;
        };
        if external.backend.canonical != "python" {
            diagnostics.push(Diagnostic::error(
                "OSR-C0006",
                "embedded provider handles are supported only by `extern python`",
                external.span,
            ));
            continue;
        }
        let Some(logical) = providers.get(&handle.canonical) else {
            diagnostics.push(Diagnostic::error(
                "OSR-C0007",
                format!("unknown embedded Python provider `{}`", handle.spelling),
                external.span,
            ));
            continue;
        };
        external.module.clone_from(logical);
    }
}

pub(super) fn compile_python_modules(
    module: &ast::Module,
    target: crate::types::PythonVersion,
) -> (Vec<EmbeddedPythonArtifact>, Vec<Diagnostic>) {
    let providers = module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ast::ItemKind::EmbeddedPython(provider) => {
                Some((provider.handle.canonical.clone(), provider))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let authored_modules = providers
        .keys()
        .map(|handle| (crate::name::python_identifier(handle), handle.clone()))
        .collect::<BTreeMap<_, _>>();
    let roots = module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ast::ItemKind::Extern(external) => external
                .provider_handle
                .as_ref()
                .map(|handle| handle.canonical.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();

    let mut compiled = BTreeMap::<String, (EmbeddedPythonArtifact, BTreeSet<String>)>::new();
    let mut diagnostics = Vec::new();
    for (handle, provider) in &providers {
        let Some(logical_module) = &provider.logical_module else {
            continue;
        };
        match parse_and_relocate(provider, target, &authored_modules, &providers) {
            Ok((source, dependencies)) => {
                compiled.insert(
                    handle.clone(),
                    (
                        EmbeddedPythonArtifact {
                            handle: handle.clone(),
                            logical_module: logical_module.clone(),
                            source,
                            source_span: provider.body_span,
                        },
                        dependencies,
                    ),
                );
            }
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }

    let mut reachable = roots;
    let mut pending = reachable.iter().cloned().collect::<Vec<_>>();
    while let Some(handle) = pending.pop() {
        let Some((_, dependencies)) = compiled.get(&handle) else {
            continue;
        };
        for dependency in dependencies {
            if reachable.insert(dependency.clone()) {
                pending.push(dependency.clone());
            }
        }
    }
    let artifacts = compiled
        .into_iter()
        .filter_map(|(handle, (artifact, _))| reachable.contains(&handle).then_some(artifact))
        .collect();
    (artifacts, diagnostics)
}

fn parse_and_relocate(
    provider: &ast::EmbeddedPython,
    target: crate::types::PythonVersion,
    authored_modules: &BTreeMap<String, String>,
    providers: &BTreeMap<String, &ast::EmbeddedPython>,
) -> Result<(String, BTreeSet<String>), Diagnostic> {
    let options =
        ParseOptions::from(Mode::Module).with_target_version(ruff_python_ast::PythonVersion {
            major: target.major,
            minor: target.minor,
        });
    let parsed = parse(&provider.body, options).map_err(|error| {
        let range = error.range();
        let start = embedded_host_offset(provider, usize::from(range.start()));
        let end = embedded_host_offset(provider, usize::from(range.end()));
        Diagnostic::error(
            "OSR-B0002",
            format!("invalid embedded Python: {error}"),
            crate::source::Span::new(start, end.max(start + 1)),
        )
    })?;
    let Mod::Module(module) = parsed.into_syntax() else {
        unreachable!("module parse options return a module");
    };
    let logical_by_authored = authored_modules
        .iter()
        .filter_map(|(authored, handle)| {
            providers
                .get(handle)
                .and_then(|provider| provider.logical_module.as_ref())
                .map(|logical| (authored.clone(), (handle.clone(), logical.clone())))
        })
        .collect::<BTreeMap<_, _>>();
    let mut collector = ImportCollector {
        logical_by_authored: &logical_by_authored,
        replacements: Vec::new(),
        dependencies: BTreeSet::new(),
        forbidden: None,
    };
    for statement in &module.body {
        collector.visit_stmt(statement);
    }
    if let Some((range, message)) = collector.forbidden {
        return Err(Diagnostic::error(
            "OSR-B0003",
            message,
            crate::source::Span::new(
                embedded_host_offset(provider, usize::from(range.start())),
                embedded_host_offset(provider, usize::from(range.end())),
            ),
        ));
    }
    collector
        .replacements
        .sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    let mut relocated = provider.body.clone();
    for (start, end, replacement) in collector.replacements {
        relocated.replace_range(start..end, &replacement);
    }
    crate::backend::format_embedded_module(&relocated)
        .map(|source| (source, collector.dependencies))
        .map_err(|error| Diagnostic::error("OSR-B0002", error.message, provider.body_span))
}

fn embedded_host_offset(provider: &ast::EmbeddedPython, offset: usize) -> usize {
    crate::reader::embedded_source_offset(
        &provider.raw_body,
        &provider.body,
        provider.body_span,
        offset,
    )
}

struct ImportCollector<'a> {
    logical_by_authored: &'a BTreeMap<String, (String, String)>,
    replacements: Vec<(usize, usize, String)>,
    dependencies: BTreeSet<String>,
    forbidden: Option<(ruff_text_size::TextRange, &'static str)>,
}

impl<'a> ImportCollector<'a> {
    fn replace_identifier(
        &mut self,
        identifier: &ruff_python_ast::Identifier,
        preserve_binding: bool,
    ) {
        let authored = identifier.id.as_str();
        let Some((handle, logical)) = self.logical_by_authored.get(authored) else {
            return;
        };
        self.dependencies.insert(handle.clone());
        let replacement = if preserve_binding {
            format!("{logical} as {authored}")
        } else {
            logical.clone()
        };
        self.replacements.push((
            usize::from(identifier.range.start()),
            usize::from(identifier.range.end()),
            replacement,
        ));
    }
}

impl<'a> Visitor<'a> for ImportCollector<'_> {
    fn visit_stmt(&mut self, statement: &'a Stmt) {
        match statement {
            Stmt::Import(import) => {
                for alias in &import.names {
                    self.replace_identifier(&alias.name, alias.asname.is_none());
                }
            }
            Stmt::ImportFrom(import) if import.level == 0 => {
                if let Some(module) = &import.module {
                    self.replace_identifier(module, false);
                }
            }
            Stmt::ImportFrom(import) => self.reject(
                import.range,
                "embedded Python modules must not use relative imports",
            ),
            Stmt::Assign(assign) if assign.targets.iter().any(is_sys_path_target) => {
                self.reject(
                    assign.range,
                    "embedded Python modules must not mutate `sys.path`",
                );
            }
            Stmt::AnnAssign(assign) if is_sys_path_target(&assign.target) => {
                self.reject(
                    assign.range,
                    "embedded Python modules must not mutate `sys.path`",
                );
            }
            Stmt::AugAssign(assign) if is_sys_path_target(&assign.target) => {
                self.reject(
                    assign.range,
                    "embedded Python modules must not mutate `sys.path`",
                );
            }
            _ => {}
        }
        walk_stmt(self, statement);
    }

    fn visit_expr(&mut self, expression: &'a Expr) {
        if let Expr::Call(call) = expression {
            match expression_path(&call.func).as_deref() {
                Some(["__import__"]) | Some(["importlib", "import_module"]) => self.reject(
                    call.range,
                    "embedded Python module discovery must use static import statements",
                ),
                Some(["sys", "path", _]) => self.reject(
                    call.range,
                    "embedded Python modules must not mutate `sys.path`",
                ),
                _ => {}
            }
        }
        walk_expr(self, expression);
    }
}

impl ImportCollector<'_> {
    fn reject(&mut self, range: ruff_text_size::TextRange, message: &'static str) {
        if self.forbidden.is_none() {
            self.forbidden = Some((range, message));
        }
    }
}

fn expression_path(expression: &Expr) -> Option<Vec<&str>> {
    match expression {
        Expr::Name(name) => Some(vec![name.id.as_str()]),
        Expr::Attribute(attribute) => {
            let mut path = expression_path(&attribute.value)?;
            path.push(attribute.attr.as_str());
            Some(path)
        }
        _ => None,
    }
}

fn is_sys_path_target(expression: &Expr) -> bool {
    match expression {
        Expr::Subscript(subscript) => is_sys_path_target(&subscript.value),
        _ => expression_path(expression).as_deref() == Some(&["sys", "path"]),
    }
}

fn provider_id(options: &CompileOptions) -> String {
    let identity = format!("{}\0{}", options.distribution, options.distribution_version);
    let hash = crate::hash::sha256(identity.as_bytes());
    format!(
        "{}_{}",
        crate::name::python_identifier(&options.distribution),
        &hash.trim_start_matches("sha256:")[..12]
    )
}

fn runtime_root(module: &str) -> String {
    module.split_once('.').map_or_else(
        || "__osiris_runtime__".to_owned(),
        |(package, _)| {
            format!(
                "{}.__osiris_runtime__",
                crate::name::python_identifier(package)
            )
        },
    )
}

/// Fill in the body of every `py/embed` provider from the content the caller
/// resolved.
///
/// A reference the caller did not resolve is an error rather than an empty
/// provider: silently compiling an empty Python module would turn a missing file
/// into an `ImportError` far from its cause. One file may back only one
/// provider, so a repeated path is an error too (OEP-0001-R006CB).
fn resolve_embedded_sources(
    module: &mut ast::Module,
    options: &CompileOptions,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut claimed = BTreeMap::<String, String>::new();
    for item in &mut module.items {
        let ast::ItemKind::EmbeddedPython(provider) = &mut item.kind else {
            continue;
        };
        let Some(path) = provider.source_path.clone() else {
            continue;
        };
        if let Some(previous) = claimed.insert(path.clone(), provider.handle.spelling.clone()) {
            diagnostics.push(Diagnostic::error(
                "OSR-C0009",
                format!(
                    "`{path}` already backs embedded provider `{previous}`; one file has one owner"
                ),
                provider.span,
            ));
            continue;
        }
        let Some(content) = options.embedded_sources.get(&path) else {
            diagnostics.push(Diagnostic::error(
                "OSR-C0009",
                format!("embedded provider source `{path}` was not found"),
                provider.span,
            ));
            continue;
        };
        provider.raw_body = content.clone();
        provider.body = content.clone();
    }
}
