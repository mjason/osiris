use super::*;

#[test]
fn facade_ids_are_unique_and_every_interface_matches_normative_source() {
    let catalog = NAMESPACES
        .iter()
        .flat_map(|namespace| exports(namespace))
        .collect::<Vec<_>>();
    let ids = catalog
        .iter()
        .map(|binding| binding.id())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), catalog.len());
    let artifacts = embedded_artifacts().expect("compiled standard artifacts");
    for namespace in NAMESPACES {
        let source_ids = exports(namespace)
            .map(|binding| binding.id().as_str().to_owned())
            .collect::<std::collections::BTreeSet<_>>();
        let interface = interface_artifact(namespace).expect("compiled standard interface");
        let interface_ids = interface
            .bindings
            .iter()
            .map(|binding| binding.id.clone())
            .chain(interface.macros.iter().map(|macro_| macro_.id.clone()))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(source_ids, interface_ids, "{namespace}");

        let source = source_artifact(namespace).expect("packaged standard source");
        let resource_path = format!(
            "{}/{}.osr",
            namespace.replace('.', "/"),
            namespace.rsplit('.').next().unwrap_or("standard")
        );
        let resource = artifacts
            .resources
            .iter()
            .find(|resource| resource.kind == "source" && resource.path == resource_path)
            .unwrap_or_else(|| panic!("missing source artifact for {namespace}"));
        assert_eq!(resource.bytes, source.as_bytes(), "{namespace}");
    }

    let core_ids = exports(CORE_NAMESPACE)
        .map(|binding| binding.id().as_str().to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    let manifest = artifacts
        .resources
        .iter()
        .find(|resource| resource.kind == "core-export-manifest")
        .expect("core export manifest");
    let artifact_ids = serde_json::from_slice::<Vec<String>>(&manifest.bytes)
        .expect("core export manifest is JSON")
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(core_ids, artifact_ids);
    assert!(semantic_hash().starts_with("sha256:"));
}

#[test]
fn every_ordinary_standard_binding_has_a_typed_signature() {
    for binding in NAMESPACES.iter().flat_map(|namespace| exports(namespace)) {
        if binding.kind != BindingKind::Function {
            continue;
        }
        let interface = interface_artifact(binding.namespace).expect("standard interface");
        let public = interface
            .bindings
            .iter()
            .find(|public| public.id == binding.id().as_str())
            .unwrap_or_else(|| panic!("missing standard binding for {}", binding.id().as_str()));
        assert!(
            matches!(&public.ty, crate::types::Type::Fn(signature) if !matches!(signature.return_type.as_ref(), crate::types::Type::Unknown)),
            "missing or unknown standard function type for {}",
            binding.id().as_str()
        );
    }
}

#[test]
fn every_standard_binding_has_complete_queryable_metadata() {
    let catalog = api_catalog();
    let export_count = NAMESPACES
        .iter()
        .flat_map(|namespace| exports(namespace))
        .count();
    assert_eq!(catalog.len(), export_count);
    for record in catalog {
        assert!(!record.call_shapes.is_empty(), "{}", record.binding_id);
        assert!(
            record
                .documentation
                .default
                .as_ref()
                .is_some_and(|value| !value.is_empty()),
            "{}",
            record.binding_id
        );
        assert!(record.documentation.translations.contains_key("zh-CN"));
        let default = record.documentation.default.as_deref().unwrap_or_default();
        assert!(default.len() >= 20, "{}", record.binding_id);
        assert!(
            !default.contains("standard function in")
                && !default.contains("standard macro in")
                && !default.contains("follows the")
                && !default.contains("contract in OEP"),
            "template documentation for {}: {default}",
            record.binding_id
        );
        assert!(!record.source.uri.is_empty());
        assert!(!record.semantic_hash.is_empty());
    }
}

#[test]
fn source_dispatched_functions_publish_their_exact_call_shapes() {
    let reduce = api_record(find(CORE_NAMESPACE, "reduce").expect("core reduce"));
    assert_eq!(
        reduce.call_shapes,
        [
            "(reduce function collection)",
            "(reduce function initial collection)",
        ]
    );
    let partition = api_record(find("osiris.sequence", "partition").expect("sequence partition"));
    assert_eq!(partition.call_shapes.len(), 3);
    assert_eq!(partition.call_shapes[0], "(partition size collection)");
}

#[test]
fn kernel_leaves_are_minimal_contracts_behind_authored_osiris_facades() {
    for namespace in NAMESPACES {
        let source = source_artifact(namespace).expect("packaged standard source");
        let lowered = ast::lower_document(&crate::reader::read(&source));
        assert!(lowered.diagnostics.is_empty(), "{namespace}");
        let public_functions = lowered
            .module
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ast::ItemKind::Defn(function) => {
                    function.name.as_ref().map(|name| name.canonical.as_str())
                }
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        for binding in exports(namespace).filter(|binding| binding.kind == BindingKind::Function) {
            assert!(
                public_functions.contains(binding.canonical),
                "{} must be an authored public defn",
                binding.id().as_str()
            );
        }
    }

    for namespace in NAMESPACES {
        let kernel_namespace = format!("{namespace}.kernel");
        let facade = ast::lower_document(&crate::reader::read(
            &source_artifact(namespace).expect("packaged facade source"),
        ));
        assert!(facade.module.items.iter().any(|item| matches!(
            &item.kind,
            ast::ItemKind::Import(import)
                if import.module.canonical == kernel_namespace && import.refer_all
        )));

        let source = artifacts::compilation_source_artifact(&kernel_namespace)
            .unwrap_or_else(|| panic!("embedded Kernel source for {namespace}"));
        let kernel = ast::lower_document(&crate::reader::read(&source));
        assert!(kernel.diagnostics.is_empty(), "{:?}", kernel.diagnostics);
        assert_eq!(
            kernel
                .module
                .name
                .as_ref()
                .map(|name| name.canonical.as_str()),
            Some(kernel_namespace.as_str())
        );
        let external = kernel
            .module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ast::ItemKind::Extern(external) => Some(external),
                _ => None,
            })
            .expect("implementation namespace has a typed Kernel boundary");
        for declaration in &external.items {
            let (canonical, metadata) = match &declaration.kind {
                ast::ItemKind::Defn(function) => (
                    function
                        .name
                        .as_ref()
                        .expect("Kernel function has a name")
                        .canonical
                        .as_str(),
                    function.metadata.as_slice(),
                ),
                ast::ItemKind::Def(definition) => (
                    definition.name.canonical.as_str(),
                    definition.metadata.as_slice(),
                ),
                _ => continue,
            };
            assert!(canonical.starts_with("kernel-leaf-"), "{canonical}");
            assert!(metadata.iter().all(|entry| {
                !matches!(
                    &entry.key.kind,
                    crate::syntax::FormKind::Keyword(name)
                        if matches!(
                            name.canonical.trim_start_matches(':'),
                            "doc" | "osiris/names" | "category" | "since" | "deprecated"
                        )
                )
            }));
        }
    }
}

#[test]
fn standard_linker_compiles_only_reachable_source_items() {
    let roots = std::collections::BTreeSet::from([
        BindingId::new(CORE_NAMESPACE, "mapv", BindingKind::Function)
            .as_str()
            .to_owned(),
        BindingId::new(CORE_NAMESPACE, "reduce", BindingKind::Function)
            .as_str()
            .to_owned(),
    ]);
    let support = linked_standard_support(
        "demo.__osiris_runtime__",
        &roots,
        crate::types::PythonVersion::DEFAULT_TARGET,
    )
    .expect("link selected standard source");
    let core = support
        .files
        .iter()
        .find(|(path, _)| path.ends_with("stdlib/core.py"))
        .map(|(_, source)| source)
        .expect("linked core module");
    assert!(core.contains("def mapv("));
    assert!(core.contains("def reduce("));
    assert!(!core.contains("def identity("));
    assert!(!core.contains("from osiris"));
    assert!(support.helpers.contains("apply"));
    assert!(support.helpers.contains("mapv"));
    assert!(support.helpers.contains("reduce"));
    assert!(!support.helpers.contains("identity"));
}

#[test]
fn linked_standard_types_resolve_to_the_private_kernel_package() {
    let roots = std::collections::BTreeSet::from([BindingId::new(
        "osiris.concurrent",
        "lock",
        BindingKind::Function,
    )
    .as_str()
    .to_owned()]);
    let support = linked_standard_support(
        "demo.__osiris_runtime__",
        &roots,
        crate::types::PythonVersion::DEFAULT_TARGET,
    )
    .expect("link concurrent source");
    let concurrent = support
        .files
        .iter()
        .find(|(path, _)| path.ends_with("stdlib/concurrent.py"))
        .map(|(_, source)| source)
        .expect("linked concurrent module");
    assert!(
        concurrent.contains("from demo.__osiris_runtime__ import Lock"),
        "{concurrent}"
    );
    assert!(!concurrent.contains("from osiris."), "{concurrent}");
    assert!(support.helpers.contains("Lock"));
}
