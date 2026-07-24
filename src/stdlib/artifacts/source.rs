use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

use crate::{
    ast, interface,
    name::{BindingId, BindingKind},
    source::Span,
    syntax::MetadataEntry,
};

use super::super::{CORE_NAMESPACE, NAMESPACES, StandardBinding};

mod provider;

const CORE_KERNEL_SOURCE: &str = include_str!("../../../stdlib/src/osiris/core/kernel.osr");
const COLLECTION_KERNEL_SOURCE: &str =
    include_str!("../../../stdlib/src/osiris/collection/kernel.osr");
const SEQUENCE_KERNEL_SOURCE: &str = include_str!("../../../stdlib/src/osiris/sequence/kernel.osr");
const STRING_KERNEL_SOURCE: &str = include_str!("../../../stdlib/src/osiris/string/kernel.osr");
const MATH_KERNEL_SOURCE: &str = include_str!("../../../stdlib/src/osiris/math/kernel.osr");
const CONCURRENT_KERNEL_SOURCE: &str =
    include_str!("../../../stdlib/src/osiris/concurrent/kernel.osr");
const PYTHON_KERNEL_SOURCE: &str = include_str!("../../../stdlib/src/osiris/python/kernel.osr");

const CORE_FACADE_NAMESPACES: &[&str] = &[
    "osiris.core.function",
    "osiris.core.transform",
    "osiris.core.sequence",
    "osiris.core.collection",
    "osiris.core.predicate",
    "osiris.core.types",
    "osiris.core.control",
    "osiris.core.comprehension",
    "osiris.core.recursion",
    "osiris.core.concurrent",
];

#[derive(Clone)]
pub(super) struct StandardSource {
    pub(super) uri: String,
    pub(super) text: String,
    pub(super) lines: BTreeMap<String, u32>,
}

static SOURCES: OnceLock<Result<BTreeMap<&'static str, StandardSource>, String>> = OnceLock::new();

pub(crate) fn validate_standard_resources() -> Result<(), String> {
    provider::validate()?;
    sources().map(|_| ())
}

pub(super) const fn standard_resource_hash() -> &'static str {
    provider::expected_hash()
}

#[must_use]
pub fn source_artifact(namespace: &str) -> Option<String> {
    sources()
        .ok()?
        .get(namespace)
        .map(|source| source.text.clone())
}

#[cfg(test)]
pub(crate) fn compilation_source_artifact(namespace: &str) -> Option<String> {
    compilation_sources()
        .ok()?
        .get(namespace)
        .map(|source| source.text.clone())
}

#[must_use]
pub fn source_artifact_by_uri(uri: &str) -> Option<String> {
    compilation_sources()
        .ok()?
        .values()
        .find(|source| source.uri == uri)
        .map(|source| source.text.clone())
}

pub(super) fn binding_source_location(
    binding: StandardBinding,
) -> super::super::StandardSourceLocation {
    if binding.namespace == CORE_NAMESPACE {
        for namespace in CORE_FACADE_NAMESPACES {
            let text = packaged_source(namespace).expect("validated standard facade source");
            let source = StandardSource {
                uri: format!("osiris-stdlib:///{}.osr", namespace.replace('.', "/")),
                lines: declaration_lines(CORE_NAMESPACE, &text),
                text,
            };
            if let Some(line) = source.lines.get(binding.id().as_str()) {
                return super::super::StandardSourceLocation {
                    uri: source.uri,
                    line: *line,
                    column: 1,
                };
            }
        }
    }
    let source = sources()
        .expect("validated standard resources")
        .get(binding.namespace)
        .expect("standard namespace has packaged source");
    super::super::StandardSourceLocation {
        uri: source.uri.clone(),
        line: source
            .lines
            .get(binding.id().as_str())
            .copied()
            .unwrap_or(1),
        column: 1,
    }
}

pub(super) fn sources() -> Result<&'static BTreeMap<&'static str, StandardSource>, String> {
    SOURCES
        .get_or_init(|| {
            NAMESPACES
                .iter()
                .copied()
                .map(|namespace| build_source(namespace).map(|source| (namespace, source)))
                .collect()
        })
        .as_ref()
        .map_err(Clone::clone)
}

pub(super) fn compilation_sources() -> Result<BTreeMap<&'static str, StandardSource>, String> {
    let mut result = sources()?.clone();
    for (namespace, source) in [
        ("osiris.core.kernel", CORE_KERNEL_SOURCE),
        ("osiris.collection.kernel", COLLECTION_KERNEL_SOURCE),
        ("osiris.sequence.kernel", SEQUENCE_KERNEL_SOURCE),
        ("osiris.string.kernel", STRING_KERNEL_SOURCE),
        ("osiris.math.kernel", MATH_KERNEL_SOURCE),
        ("osiris.concurrent.kernel", CONCURRENT_KERNEL_SOURCE),
        ("osiris.python.kernel", PYTHON_KERNEL_SOURCE),
    ] {
        result.insert(namespace, build_source_from_text(namespace, source));
    }
    for namespace in CORE_FACADE_NAMESPACES {
        let source = packaged_source(namespace)?;
        result.insert(namespace, build_source_from_text(namespace, &source));
    }
    result.insert(CORE_NAMESPACE, core_facade_compilation_source()?);
    Ok(result)
}

fn core_facade_compilation_source() -> Result<StandardSource, String> {
    let names = facade_macro_names().into_iter().collect::<Vec<_>>();
    let mut text = packaged_source(CORE_NAMESPACE)?.trim_end().to_owned();
    text.push_str("\n\n;; Facade implementations are authored in osiris.core.* modules.\n");
    text.push_str("(export [");
    text.push_str(&names.join(" "));
    text.push_str("])\n");
    let modules = facade_module_names();
    for namespace in CORE_FACADE_NAMESPACES {
        if modules.contains(*namespace) {
            let source = packaged_source(namespace)?;
            if let Some(body) = implementation_body(&source) {
                text.push('\n');
                text.push_str(body.trim());
                text.push('\n');
            }
        }
    }
    Ok(build_source_from_text(CORE_NAMESPACE, &text))
}

fn implementation_body(source: &str) -> Option<&str> {
    let lowered = ast::lower_document(&crate::reader::read(source));
    let start = lowered.module.items.iter().find_map(|item| {
        matches!(
            item.kind,
            ast::ItemKind::Def(_)
                | ast::ItemKind::Defn(_)
                | ast::ItemKind::Defstruct(_)
                | ast::ItemKind::Defmacro(_)
                | ast::ItemKind::DefnForSyntax(_)
        )
        .then_some(item.span.start)
    })?;
    source.get(start..)
}

fn module_metadata_symbols(key: &str) -> BTreeSet<String> {
    let source = packaged_source(CORE_NAMESPACE).expect("validated standard core source");
    let lowered = ast::lower_document(&crate::reader::read(&source));
    lowered
        .module
        .metadata
        .iter()
        .find_map(|entry| match (&entry.key.kind, &entry.value.kind) {
            (crate::syntax::FormKind::Keyword(found), crate::syntax::FormKind::Vector(names))
                if found.canonical.trim_start_matches(':') == key =>
            {
                Some(names)
            }
            _ => None,
        })
        .into_iter()
        .flatten()
        .filter_map(|form| match &form.kind {
            crate::syntax::FormKind::Symbol(name) => Some(name.canonical.clone()),
            _ => None,
        })
        .collect()
}

fn facade_module_names() -> BTreeSet<String> {
    module_metadata_symbols("osiris/facade-modules")
}

pub(crate) fn facade_macro_names() -> BTreeSet<String> {
    module_metadata_symbols("osiris/facade-macros")
}

fn build_source(namespace: &'static str) -> Result<StandardSource, String> {
    if namespace == CORE_NAMESPACE {
        core_facade_compilation_source()
    } else {
        packaged_source(namespace).map(|source| build_source_from_text(namespace, &source))
    }
}

fn build_source_from_text(namespace: &'static str, source: &str) -> StandardSource {
    let text = source.to_owned();
    let lines = declaration_lines(namespace, &text);
    let uri = format!("osiris-stdlib:///{}.osr", namespace.replace('.', "/"));
    StandardSource { uri, text, lines }
}

fn packaged_source(namespace: &str) -> Result<String, String> {
    provider::read(&format!("src/{}.osr", namespace.replace('.', "/")))
}

fn declaration_lines(namespace: &str, source: &str) -> BTreeMap<String, u32> {
    let lowered = ast::lower_document(&crate::reader::read(source));
    let exported = lowered
        .module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ast::ItemKind::Export(export) => Some(export.names.iter()),
            _ => None,
        })
        .flatten()
        .map(|name| name.canonical.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut lines = BTreeMap::new();
    for_each_declaration(&lowered.module, |name, kind, _, span| {
        if exported.contains(name) {
            lines.insert(
                BindingId::new(namespace, name, kind).as_str().to_owned(),
                declaration_line_number(source, span, name),
            );
        }
    });
    lines
}

fn declaration_metadata(binding: StandardBinding, source: &str) -> Option<Vec<MetadataEntry>> {
    let lowered = ast::lower_document(&crate::reader::read(source));
    let mut result = None;
    for_each_declaration(&lowered.module, |name, kind, metadata, _| {
        if result.is_none() && name == binding.canonical && kind == binding.kind {
            result = Some(metadata.to_vec());
        }
    });
    result
}

fn for_each_declaration(
    module: &ast::Module,
    mut visitor: impl FnMut(&str, BindingKind, &[MetadataEntry], Span),
) {
    for item in &module.items {
        visit_item(item, &mut visitor);
        if let ast::ItemKind::Extern(external) = &item.kind {
            for nested in &external.items {
                visit_item(nested, &mut visitor);
            }
        }
    }
}

fn visit_item(
    item: &ast::Item,
    visitor: &mut impl FnMut(&str, BindingKind, &[MetadataEntry], Span),
) {
    match &item.kind {
        ast::ItemKind::Def(definition) => visitor(
            &definition.name.canonical,
            BindingKind::Value,
            &definition.metadata,
            definition.span,
        ),
        ast::ItemKind::Defn(function) => {
            if let Some(name) = &function.name {
                visitor(
                    &name.canonical,
                    BindingKind::Function,
                    &function.metadata,
                    function.span,
                );
            }
        }
        ast::ItemKind::Defstruct(structure) => visitor(
            &structure.name.canonical,
            BindingKind::Type,
            &structure.metadata,
            structure.span,
        ),
        ast::ItemKind::Defmacro(macro_) => visitor(
            &macro_.name.canonical,
            BindingKind::Macro,
            &macro_.metadata,
            macro_.span,
        ),
        _ => {}
    }
}

fn line_number(source: &str, span: Span) -> u32 {
    source
        .as_bytes()
        .get(..span.start)
        .unwrap_or_default()
        .iter()
        .filter(|byte| **byte == b'\n')
        .count() as u32
        + 1
}

fn declaration_line_number(source: &str, span: Span, name: &str) -> u32 {
    let base = line_number(source, span);
    let declaration = source.get(span.start..span.end).unwrap_or_default();
    declaration
        .lines()
        .position(|line| {
            line.trim_start().starts_with("(def")
                && line
                    .split(|character: char| {
                        character.is_whitespace() || "()[]{}".contains(character)
                    })
                    .any(|token| token == name)
        })
        .map_or(base, |offset| base + offset as u32)
}

pub(crate) fn binding_metadata(binding: StandardBinding) -> Result<Vec<MetadataEntry>, String> {
    let source = sources()?
        .get(binding.namespace)
        .ok_or_else(|| format!("unknown standard namespace `{}`", binding.namespace))?;
    declaration_metadata(binding, &source.text)
        .ok_or_else(|| format!("standard source is missing `{}`", binding.id().as_str()))
        .and_then(|metadata| {
            interface::normalize_metadata(&metadata).map_err(|error| error.to_string())
        })
}
