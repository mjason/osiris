//! Compiler-embedded standard-library identities and source artifacts.
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
    EmbeddedStandardArtifacts, StandardArtifactResource, embedded_artifacts, interface_artifact,
    source_artifact, source_artifact_by_uri, validate_embedded_artifacts,
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
    embedded_artifacts()
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
