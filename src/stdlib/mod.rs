//! Compiler-owned Kernel identities and source-distributed standard artifacts.
//!
//! This module is the single source of truth for public facade names. Private
//! Python helper layout and Phase-1 implementation names are deliberately not
//! part of these identities.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

use crate::{
    ast,
    name::{BindingId, BindingKind},
};
mod api;
mod artifacts;

pub use api::{
    StandardApiRecord, StandardApiSelection, StandardSourceLocation, api_catalog, api_record,
    query_api,
};
pub(crate) use artifacts::linked_standard_support;
pub use artifacts::{
    StandardArtifactResource, StandardArtifacts, interface_artifact, source_artifact,
    source_artifact_by_uri, standard_artifacts, validate_resources, validate_standard_artifacts,
};

pub const CORE_NAMESPACE: &str = "osiris.core";

pub const NAMESPACES: &[&str] = &[
    CORE_NAMESPACE,
    "osiris.collection",
    "osiris.sequence",
    "osiris.string",
    "osiris.math",
    "osiris.concurrent",
    "osiris.python",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StandardBinding {
    pub namespace: &'static str,
    pub canonical: &'static str,
    pub kind: BindingKind,
}

impl StandardBinding {
    #[must_use]
    pub fn id(self) -> BindingId {
        BindingId::new(self.namespace, self.canonical, self.kind)
    }

    #[must_use]
    pub fn runtime_name(self) -> String {
        crate::name::python_identifier(self.canonical)
    }
}

#[must_use]
pub fn is_standard_namespace(namespace: &str) -> bool {
    NAMESPACES.contains(&namespace)
}

/// Whether an ordinary source module receives the default `osiris.core`
/// referral. Standard implementation modules opt out through authored
/// `:osiris/internal true` metadata, and the core facade cannot refer itself.
#[must_use]
pub(crate) fn uses_implicit_core(module: &ast::Module) -> bool {
    let is_standard_bootstrap = module.name.as_ref().is_some_and(|name| {
        NAMESPACES.iter().any(|namespace| {
            name.canonical == *namespace
                || name
                    .canonical
                    .strip_prefix(namespace)
                    .is_some_and(|suffix| suffix.starts_with('.'))
        })
    });
    let is_internal = module.metadata.iter().any(|entry| {
        matches!(&entry.key.kind, crate::syntax::FormKind::Keyword(name)
            if name.canonical.trim_start_matches(':') == "osiris/internal")
            && matches!(entry.value.kind, crate::syntax::FormKind::Bool(true))
    });
    let has_explicit_core = module.items.iter().any(|item| {
        matches!(&item.kind, ast::ItemKind::Import(import)
            if import.module.canonical == CORE_NAMESPACE)
    });
    !is_standard_bootstrap && !is_internal && !has_explicit_core
}

/// Avoid initializing the typed core interface for modules that cannot use an
/// implicitly referred name. This keeps no-core CLI builds fast while name
/// resolution remains exact for every authored or macro-generated reference.
#[must_use]
pub(crate) fn needs_implicit_core(module: &ast::Module) -> bool {
    if !uses_implicit_core(module) {
        return false;
    }
    let names = exports(CORE_NAMESPACE)
        .map(|binding| binding.canonical)
        .collect::<BTreeSet<_>>();
    let Ok(value) = serde_json::to_value(module) else {
        return true;
    };
    contains_core_name(&value, &names)
}

fn contains_core_name(value: &serde_json::Value, names: &BTreeSet<&str>) -> bool {
    match value {
        serde_json::Value::Object(object) => {
            if object
                .get("canonical")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| {
                    names.contains(name)
                        || name
                            .strip_prefix("osiris.core/")
                            .or_else(|| name.strip_prefix("osiris.core."))
                            .is_some_and(|name| names.contains(name))
                })
            {
                return true;
            }
            object
                .values()
                .any(|value| contains_core_name(value, names))
        }
        serde_json::Value::Array(values) => {
            values.iter().any(|value| contains_core_name(value, names))
        }
        _ => false,
    }
}

#[must_use]
pub(crate) fn document_may_use_implicit_core_macro(document: &crate::syntax::Document) -> bool {
    let names = artifacts::facade_macro_names();
    document
        .forms
        .iter()
        .any(|form| form_may_call_core_macro(form, &names))
}

fn form_may_call_core_macro(form: &crate::syntax::Form, names: &BTreeSet<String>) -> bool {
    use crate::syntax::FormKind;

    let children = match &form.kind {
        FormKind::List(items) => {
            if items
                .first()
                .and_then(|head| match &head.kind {
                    FormKind::Symbol(name) => Some(name.canonical.as_str()),
                    _ => None,
                })
                .is_some_and(|name| {
                    names.contains(name)
                        || name
                            .strip_prefix("osiris.core/")
                            .or_else(|| name.strip_prefix("osiris.core."))
                            .is_some_and(|name| names.contains(name))
                })
            {
                return true;
            }
            items.as_slice()
        }
        FormKind::Vector(items) | FormKind::Map(items) | FormKind::Set(items) => items.as_slice(),
        FormKind::ReaderMacro { form, .. } => {
            return form_may_call_core_macro(form, names);
        }
        _ => return false,
    };
    children
        .iter()
        .any(|child| form_may_call_core_macro(child, names))
}

pub fn exports(namespace: &str) -> impl Iterator<Item = StandardBinding> {
    source_catalog()
        .get(namespace)
        .into_iter()
        .flat_map(|bindings| bindings.iter().copied())
}

#[must_use]
pub fn find(namespace: &str, canonical: &str) -> Option<StandardBinding> {
    exports(namespace).find(|binding| binding.canonical == canonical)
}

#[must_use]
pub fn find_by_id(id: &BindingId) -> Option<StandardBinding> {
    NAMESPACES
        .iter()
        .flat_map(|namespace| exports(namespace))
        .find(|binding| binding.id() == *id)
}

/// Hash the semantic facade contract independently from private helper layout.
#[must_use]
pub fn semantic_hash() -> String {
    standard_artifacts()
        .map(|artifacts| artifacts.semantic_hash.clone())
        .unwrap_or_default()
}

fn source_catalog() -> &'static BTreeMap<&'static str, Vec<StandardBinding>> {
    static CATALOG: OnceLock<BTreeMap<&'static str, Vec<StandardBinding>>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        NAMESPACES
            .iter()
            .copied()
            .map(|namespace| (namespace, source_bindings(namespace)))
            .collect()
    })
}

fn source_bindings(namespace: &'static str) -> Vec<StandardBinding> {
    let source = source_artifact(namespace).expect("standard namespace has packaged source");
    let lowered = ast::lower_document(&crate::reader::read(&source));
    assert!(
        lowered.diagnostics.is_empty(),
        "invalid packaged standard source `{namespace}`: {:?}",
        lowered.diagnostics
    );
    let exported = lowered
        .module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ast::ItemKind::Export(export) => Some(export.names.iter()),
            _ => None,
        })
        .flatten()
        .map(|name| name.canonical.clone())
        .collect::<Vec<_>>();
    let mut declarations = BTreeMap::<String, BTreeSet<BindingKind>>::new();
    for item in &lowered.module.items {
        collect_binding_kind(&item.kind, &mut declarations);
        if let ast::ItemKind::Extern(external) = &item.kind {
            for nested in &external.items {
                collect_binding_kind(&nested.kind, &mut declarations);
            }
        }
    }
    exported
        .into_iter()
        .flat_map(|canonical| {
            declarations
                .get(&canonical)
                .into_iter()
                .flatten()
                .copied()
                .map(move |kind| StandardBinding {
                    namespace,
                    canonical: Box::leak(canonical.clone().into_boxed_str()),
                    kind,
                })
        })
        .collect()
}

fn collect_binding_kind(
    item: &ast::ItemKind,
    declarations: &mut BTreeMap<String, BTreeSet<BindingKind>>,
) {
    let declaration = match item {
        ast::ItemKind::Def(definition) => Some((&definition.name.canonical, BindingKind::Value)),
        ast::ItemKind::Defn(function) => function
            .name
            .as_ref()
            .map(|name| (&name.canonical, BindingKind::Function)),
        ast::ItemKind::Defstruct(structure) => Some((&structure.name.canonical, BindingKind::Type)),
        ast::ItemKind::Defmacro(macro_) => Some((&macro_.name.canonical, BindingKind::Macro)),
        _ => None,
    };
    if let Some((canonical, kind)) = declaration {
        declarations
            .entry(canonical.clone())
            .or_default()
            .insert(kind);
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
