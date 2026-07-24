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

const CORE_SOURCE: &str = include_str!("../../../stdlib/src/osiris/core.osr");
const CORE_KERNEL_SOURCE: &str = include_str!("../../../stdlib/src/osiris/core/kernel.osr");
const CORE_FUNCTION_SOURCE: &str = include_str!("../../../stdlib/src/osiris/core/function.osr");
const CORE_TRANSFORM_SOURCE: &str = include_str!("../../../stdlib/src/osiris/core/transform.osr");
const CORE_SEQUENCE_SOURCE: &str = include_str!("../../../stdlib/src/osiris/core/sequence.osr");
const CORE_COLLECTION_SOURCE: &str = include_str!("../../../stdlib/src/osiris/core/collection.osr");
const CORE_PREDICATE_SOURCE: &str = include_str!("../../../stdlib/src/osiris/core/predicate.osr");
const CORE_TYPES_SOURCE: &str = include_str!("../../../stdlib/src/osiris/core/types.osr");
const CORE_CONTROL_SOURCE: &str = include_str!("../../../stdlib/src/osiris/core/control.osr");
const CORE_COMPREHENSION_SOURCE: &str =
    include_str!("../../../stdlib/src/osiris/core/comprehension.osr");
const CORE_RECURSION_SOURCE: &str = include_str!("../../../stdlib/src/osiris/core/recursion.osr");
const CORE_CONCURRENT_SOURCE: &str = include_str!("../../../stdlib/src/osiris/core/concurrent.osr");
const COLLECTION_SOURCE: &str = include_str!("../../../stdlib/src/osiris/collection.osr");
const COLLECTION_KERNEL_SOURCE: &str =
    include_str!("../../../stdlib/src/osiris/collection/kernel.osr");
const SEQUENCE_SOURCE: &str = include_str!("../../../stdlib/src/osiris/sequence.osr");
const SEQUENCE_KERNEL_SOURCE: &str = include_str!("../../../stdlib/src/osiris/sequence/kernel.osr");
const STRING_SOURCE: &str = include_str!("../../../stdlib/src/osiris/string.osr");
const STRING_KERNEL_SOURCE: &str = include_str!("../../../stdlib/src/osiris/string/kernel.osr");
const MATH_SOURCE: &str = include_str!("../../../stdlib/src/osiris/math.osr");
const MATH_KERNEL_SOURCE: &str = include_str!("../../../stdlib/src/osiris/math/kernel.osr");
const CONCURRENT_SOURCE: &str = include_str!("../../../stdlib/src/osiris/concurrent.osr");
const CONCURRENT_KERNEL_SOURCE: &str =
    include_str!("../../../stdlib/src/osiris/concurrent/kernel.osr");
const PYTHON_SOURCE: &str = include_str!("../../../stdlib/src/osiris/python.osr");
const PYTHON_KERNEL_SOURCE: &str = include_str!("../../../stdlib/src/osiris/python/kernel.osr");

const CORE_FACADE_SOURCES: &[(&str, &str)] = &[
    ("osiris.core.function", CORE_FUNCTION_SOURCE),
    ("osiris.core.transform", CORE_TRANSFORM_SOURCE),
    ("osiris.core.sequence", CORE_SEQUENCE_SOURCE),
    ("osiris.core.collection", CORE_COLLECTION_SOURCE),
    ("osiris.core.predicate", CORE_PREDICATE_SOURCE),
    ("osiris.core.types", CORE_TYPES_SOURCE),
    ("osiris.core.control", CORE_CONTROL_SOURCE),
    ("osiris.core.comprehension", CORE_COMPREHENSION_SOURCE),
    ("osiris.core.recursion", CORE_RECURSION_SOURCE),
    ("osiris.core.concurrent", CORE_CONCURRENT_SOURCE),
];

#[derive(Clone)]
pub(super) struct StandardSource {
    pub(super) uri: String,
    pub(super) text: String,
    pub(super) lines: BTreeMap<String, u32>,
}

static SOURCES: OnceLock<BTreeMap<&'static str, StandardSource>> = OnceLock::new();

#[must_use]
pub fn source_artifact(namespace: &str) -> Option<String> {
    sources().get(namespace).map(|source| source.text.clone())
}

#[cfg(test)]
pub(crate) fn compilation_source_artifact(namespace: &str) -> Option<String> {
    compilation_sources()
        .get(namespace)
        .map(|source| source.text.clone())
}

#[must_use]
pub fn source_artifact_by_uri(uri: &str) -> Option<String> {
    sources()
        .values()
        .find(|source| source.uri == uri)
        .map(|source| source.text.clone())
}

pub(super) fn binding_source_location(
    binding: StandardBinding,
) -> super::super::StandardSourceLocation {
    let source = sources()
        .get(binding.namespace)
        .expect("standard namespace has embedded source");
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

pub(super) fn sources() -> &'static BTreeMap<&'static str, StandardSource> {
    SOURCES.get_or_init(|| {
        NAMESPACES
            .iter()
            .copied()
            .map(|namespace| (namespace, build_source(namespace)))
            .collect()
    })
}

pub(super) fn compilation_sources() -> BTreeMap<&'static str, StandardSource> {
    let mut result = sources().clone();
    for (namespace, source) in [
        ("osiris.core.kernel", CORE_KERNEL_SOURCE),
        ("osiris.core.function", CORE_FUNCTION_SOURCE),
        ("osiris.core.transform", CORE_TRANSFORM_SOURCE),
        ("osiris.core.sequence", CORE_SEQUENCE_SOURCE),
        ("osiris.core.collection", CORE_COLLECTION_SOURCE),
        ("osiris.core.predicate", CORE_PREDICATE_SOURCE),
        ("osiris.core.types", CORE_TYPES_SOURCE),
        ("osiris.core.control", CORE_CONTROL_SOURCE),
        ("osiris.core.comprehension", CORE_COMPREHENSION_SOURCE),
        ("osiris.core.recursion", CORE_RECURSION_SOURCE),
        ("osiris.core.concurrent", CORE_CONCURRENT_SOURCE),
        ("osiris.collection.kernel", COLLECTION_KERNEL_SOURCE),
        ("osiris.sequence.kernel", SEQUENCE_KERNEL_SOURCE),
        ("osiris.string.kernel", STRING_KERNEL_SOURCE),
        ("osiris.math.kernel", MATH_KERNEL_SOURCE),
        ("osiris.concurrent.kernel", CONCURRENT_KERNEL_SOURCE),
        ("osiris.python.kernel", PYTHON_KERNEL_SOURCE),
    ] {
        result.insert(namespace, build_source_from_text(namespace, source));
    }
    result.insert(CORE_NAMESPACE, core_facade_compilation_source());
    result
}

fn core_facade_compilation_source() -> StandardSource {
    let names = facade_macro_names().into_iter().collect::<Vec<_>>();
    let mut text = CORE_SOURCE.trim_end().to_owned();
    text.push_str("\n\n;; Facade implementations are authored in osiris.core.* modules.\n");
    text.push_str("(export [");
    text.push_str(&names.join(" "));
    text.push_str("])\n");
    let modules = facade_module_names();
    for (namespace, source) in CORE_FACADE_SOURCES {
        if modules.contains(*namespace) {
            if let Some(body) = implementation_body(source) {
                text.push('\n');
                text.push_str(body.trim());
                text.push('\n');
            }
        }
    }
    build_source_from_text(CORE_NAMESPACE, &text)
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
    let lowered = ast::lower_document(&crate::reader::read(CORE_SOURCE));
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

fn build_source(namespace: &'static str) -> StandardSource {
    if namespace == CORE_NAMESPACE {
        core_facade_compilation_source()
    } else {
        build_source_from_text(namespace, packaged_source(namespace))
    }
}

fn build_source_from_text(namespace: &'static str, source: &str) -> StandardSource {
    let text = source.to_owned();
    let lines = declaration_lines(namespace, &text);
    let uri = format!("osiris-stdlib:///{}.osr", namespace.replace('.', "/"));
    StandardSource { uri, text, lines }
}

fn packaged_source(namespace: &str) -> &'static str {
    match namespace {
        "osiris.core" => CORE_SOURCE,
        "osiris.collection" => COLLECTION_SOURCE,
        "osiris.sequence" => SEQUENCE_SOURCE,
        "osiris.string" => STRING_SOURCE,
        "osiris.math" => MATH_SOURCE,
        "osiris.concurrent" => CONCURRENT_SOURCE,
        "osiris.python" => PYTHON_SOURCE,
        _ => "",
    }
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
    let source = sources()
        .get(binding.namespace)
        .ok_or_else(|| format!("unknown standard namespace `{}`", binding.namespace))?;
    declaration_metadata(binding, &source.text)
        .ok_or_else(|| format!("standard source is missing `{}`", binding.id().as_str()))
        .and_then(|metadata| {
            interface::normalize_metadata(&metadata).map_err(|error| error.to_string())
        })
}
