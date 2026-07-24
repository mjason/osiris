//! Validated compiler-owned standard source, interfaces, and link metadata.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::OnceLock,
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    compiler::{self, CompileInput, CompileOptions},
    interface::{self, Interface},
};

use super::{CORE_NAMESPACE, NAMESPACES, StandardBinding};

mod source;

#[cfg(test)]
pub(super) use source::compilation_source_artifact;
pub(crate) use source::{binding_metadata, facade_macro_names};
use source::{compilation_sources, sources, standard_resource_hash, validate_standard_resources};
pub use source::{source_artifact, source_artifact_by_uri};

pub fn validate_resources() -> Result<(), String> {
    validate_standard_resources()
}

#[derive(Clone, Debug)]
pub struct StandardArtifactResource {
    pub path: String,
    pub kind: &'static str,
    pub content_hash: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct StandardArtifacts {
    pub schema: &'static str,
    pub compiler_version: &'static str,
    pub language_version: &'static str,
    pub standard_library_abi: u32,
    pub source_tree_hash: &'static str,
    pub semantic_hash: String,
    pub manifest_hash: String,
    pub resources: Vec<StandardArtifactResource>,
}

static ARTIFACTS: OnceLock<Result<StandardArtifacts, String>> = OnceLock::new();
static INTERFACES: OnceLock<Result<BTreeMap<String, Interface>, String>> = OnceLock::new();
static CORE_INTERFACE: OnceLock<Result<Interface, String>> = OnceLock::new();
static COMPILED_SOURCES: OnceLock<Result<Vec<crate::compiler::CompileResult>, String>> =
    OnceLock::new();

const CORE_MACRO_PROVIDERS: &[&str] = &[
    "osiris.core.control",
    "osiris.core.comprehension",
    "osiris.core.recursion",
    "osiris.core.concurrent",
];

#[derive(Clone, Debug, Default)]
pub(crate) struct LinkedStandardSupport {
    pub files: Vec<(PathBuf, String)>,
    pub helpers: BTreeSet<String>,
    pub binding_ids: BTreeSet<String>,
    pub source_maps: Vec<crate::artifact::SourceMap>,
}

pub(super) fn binding_source_location(binding: StandardBinding) -> super::StandardSourceLocation {
    source::binding_source_location(binding)
}

pub fn standard_artifacts() -> Result<&'static StandardArtifacts, String> {
    ARTIFACTS
        .get_or_init(build_artifacts)
        .as_ref()
        .map_err(Clone::clone)
}

pub fn validate_standard_artifacts() -> Result<(), String> {
    validate_standard_resources()?;
    let artifacts = standard_artifacts()?;
    if artifacts.schema != "osiris-standard-artifacts/v1"
        || artifacts.language_version != crate::LANGUAGE_VERSION
        || artifacts.standard_library_abi != crate::STANDARD_LIBRARY_ABI
        || artifacts.source_tree_hash != standard_resource_hash()
    {
        return Err("standard artifact manifest has incompatible identity".to_owned());
    }
    let mut interfaces = BTreeMap::new();
    for resource in &artifacts.resources {
        if digest(&resource.bytes) != resource.content_hash {
            return Err(format!(
                "standard resource `{}` failed its hash",
                resource.path
            ));
        }
        if resource.kind == "interface" {
            let source = std::str::from_utf8(&resource.bytes).map_err(|error| error.to_string())?;
            let interface = interface::read(source).map_err(|error| {
                format!("invalid standard interface `{}`: {error}", resource.path)
            })?;
            interfaces.insert(interface.module.clone(), interface);
        }
    }
    if artifacts.semantic_hash != semantic_hash_for_interfaces(&interfaces) {
        return Err("standard semantic hash is stale".to_owned());
    }
    let expected = manifest_hash(&artifacts.resources);
    if expected != artifacts.manifest_hash {
        return Err("standard artifact manifest hash is stale".to_owned());
    }
    Ok(())
}

pub fn interface_artifact(namespace: &str) -> Result<Interface, String> {
    if namespace == CORE_NAMESPACE {
        return CORE_INTERFACE.get_or_init(compile_core_interface).clone();
    }
    let interfaces = INTERFACES.get_or_init(|| {
        validate_standard_artifacts()?;
        let artifacts = standard_artifacts()?;
        artifacts
            .resources
            .iter()
            .filter(|resource| resource.kind == "interface")
            .map(|resource| {
                let source = std::str::from_utf8(&resource.bytes)
                    .map_err(|error| format!("{}: {error}", resource.path))?;
                let interface = interface::read(source)
                    .map_err(|error| format!("{}: {error}", resource.path))?;
                Ok((interface.module.clone(), interface))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()
    });
    interfaces
        .as_ref()
        .map_err(Clone::clone)?
        .get(namespace)
        .cloned()
        .ok_or_else(|| format!("standard interface `{namespace}` is missing"))
}

fn compile_core_interface() -> Result<Interface, String> {
    let compilation_sources = compilation_sources()?;
    let namespaces = ["osiris.core.kernel", CORE_NAMESPACE];
    let options = namespaces
        .iter()
        .map(|namespace| {
            CompileOptions::new(*namespace, crate::types::PythonVersion::DEFAULT_TARGET)
                .with_source_name(format!("stdlib/src/{}.osr", namespace.replace('.', "/")))
                .with_expected_module_name(*namespace)
                .with_provider("osiris-stdlib", crate::version())
        })
        .collect::<Vec<_>>();
    let inputs = namespaces
        .iter()
        .zip(&options)
        .map(|(namespace, options)| {
            CompileInput::new(&compilation_sources[namespace].text, options)
        })
        .collect::<Vec<_>>();
    let compiled = compiler::compile_workspace(&inputs, &BTreeMap::new());
    if compiled.has_errors() {
        let diagnostics = compiled
            .diagnostics
            .iter()
            .map(|located| {
                format!(
                    "error[{}]: {}",
                    located.diagnostic.code, located.diagnostic.message
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "could not compile embedded core interface:\n{diagnostics}"
        ));
    }
    let encoded = compiled
        .units
        .into_iter()
        .find(|result| result.analysis.hir.name == CORE_NAMESPACE)
        .and_then(|result| result.interface)
        .ok_or_else(|| "core compilation produced no interface".to_owned())?;
    interface::read(&encoded).map_err(|error| format!("invalid embedded core interface: {error}"))
}

fn build_artifacts() -> Result<StandardArtifacts, String> {
    validate_standard_resources()?;
    let compiled = compiled_sources()?;
    let sources = sources()?;
    let mut resources = Vec::new();
    let mut interfaces = BTreeMap::new();
    for (namespace, result) in NAMESPACES.iter().copied().zip(compiled) {
        let source = &sources[namespace];
        add_resource(
            &mut resources,
            source_path(namespace, "osr"),
            "source",
            source.text.as_bytes(),
        );
        let encoded = result
            .interface
            .as_ref()
            .ok_or_else(|| format!("{namespace} compilation produced no interface"))?;
        let interface = interface::read(encoded)
            .map_err(|error| format!("{namespace} encoded interface: {error}"))?;
        add_resource(
            &mut resources,
            source_path(namespace, "osri"),
            "interface",
            encoded.as_bytes(),
        );
        let index = serde_json::to_vec(&SourceIndex {
            schema: "osiris-standard-source-index/v1",
            namespace,
            uri: &source.uri,
            bindings: &source.lines,
        })
        .map_err(|error| error.to_string())?;
        add_resource(
            &mut resources,
            source_path(namespace, "source-index.json"),
            "source-index",
            &index,
        );
        if !interface.macros.is_empty() {
            let macro_ir = serde_json::to_vec(
                &interface
                    .macros
                    .iter()
                    .map(|item| &item.phase_ir)
                    .collect::<Vec<_>>(),
            )
            .map_err(|error| error.to_string())?;
            add_resource(
                &mut resources,
                source_path(namespace, "macro-ir.json"),
                "macro-ir",
                &macro_ir,
            );
            let helpers = serde_json::to_vec(
                &interface
                    .phase_helpers
                    .iter()
                    .map(|item| &item.phase_ir)
                    .collect::<Vec<_>>(),
            )
            .map_err(|error| error.to_string())?;
            add_resource(
                &mut resources,
                source_path(namespace, "phase-helpers.json"),
                "phase-helper-closure",
                &helpers,
            );
        }
        interfaces.insert(namespace.to_owned(), interface);
    }
    finish_artifacts(resources, interfaces)
}

fn compiled_sources() -> Result<&'static Vec<crate::compiler::CompileResult>, String> {
    COMPILED_SOURCES
        .get_or_init(compile_sources)
        .as_ref()
        .map_err(Clone::clone)
}

fn compile_sources() -> Result<Vec<crate::compiler::CompileResult>, String> {
    let compilation_sources = compilation_sources()?;
    let namespaces = compilation_sources
        .keys()
        .copied()
        .filter(|namespace| !CORE_MACRO_PROVIDERS.contains(namespace))
        .collect::<Vec<_>>();
    let options = namespaces
        .iter()
        .map(|namespace| {
            CompileOptions::new(*namespace, crate::types::PythonVersion::DEFAULT_TARGET)
                .with_source_name(format!("stdlib/src/{}.osr", namespace.replace('.', "/")))
                .with_expected_module_name(*namespace)
                .with_provider("osiris-stdlib", crate::version())
        })
        .collect::<Vec<_>>();
    let inputs = namespaces
        .iter()
        .zip(&options)
        .map(|(namespace, options)| {
            CompileInput::new(&compilation_sources[namespace].text, options)
        })
        .collect::<Vec<_>>();
    let compiled = compiler::compile_workspace(&inputs, &BTreeMap::new());
    if compiled.has_errors() {
        let diagnostics = compiled
            .diagnostics
            .iter()
            .map(|located| {
                let namespace = namespaces
                    .get(located.input_index)
                    .copied()
                    .unwrap_or("<unknown>");
                format!(
                    "{namespace}: error[{}]: {}",
                    located.diagnostic.code, located.diagnostic.message
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "could not compile source-distributed standard library:\n{diagnostics}"
        ));
    }
    let mut units = compiled
        .units
        .into_iter()
        .map(|unit| (unit.analysis.hir.name.clone(), unit))
        .collect::<BTreeMap<_, _>>();
    for namespace in CORE_MACRO_PROVIDERS {
        let options = CompileOptions::new(*namespace, crate::types::PythonVersion::DEFAULT_TARGET)
            .with_source_name(format!("stdlib/src/{}.osr", namespace.replace('.', "/")))
            .with_expected_module_name(*namespace)
            .with_provider("osiris-stdlib", crate::version());
        let unit = compiler::compile(&compilation_sources[namespace].text, &options);
        if unit.has_errors() {
            let diagnostics = unit
                .analysis
                .diagnostics
                .iter()
                .map(|diagnostic| {
                    format!(
                        "{namespace}: error[{}]: {}",
                        diagnostic.code, diagnostic.message
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            return Err(format!(
                "could not compile standard macro implementation:\n{diagnostics}"
            ));
        }
        units.insert((*namespace).to_owned(), unit);
    }
    NAMESPACES
        .iter()
        .map(|namespace| {
            units
                .remove(*namespace)
                .ok_or_else(|| format!("compiled standard module `{namespace}` is missing"))
        })
        .collect()
}

fn finish_artifacts(
    mut resources: Vec<StandardArtifactResource>,
    interfaces: BTreeMap<String, Interface>,
) -> Result<StandardArtifacts, String> {
    let core_interface = interfaces
        .get(CORE_NAMESPACE)
        .ok_or_else(|| "compiled osiris.core interface is missing".to_owned())?;
    let core = serde_json::to_vec(
        &core_interface
            .bindings
            .iter()
            .map(|binding| binding.id.clone())
            .chain(core_interface.macros.iter().map(|macro_| macro_.id.clone()))
            .collect::<Vec<_>>(),
    )
    .map_err(|error| error.to_string())?;
    add_resource(
        &mut resources,
        "core-exports.json".to_owned(),
        "core-export-manifest",
        &core,
    );
    let helpers = crate::backend::linkable_helper_ir_bytes().map_err(|error| error.to_string())?;
    add_resource(
        &mut resources,
        "linkable-helpers.hir.json".to_owned(),
        "linkable-helper-hir",
        &helpers,
    );
    resources.sort_by(|left, right| left.path.cmp(&right.path));
    let manifest_hash = manifest_hash(&resources);
    let semantic_hash = semantic_hash_for_interfaces(&interfaces);
    Ok(StandardArtifacts {
        schema: "osiris-standard-artifacts/v1",
        compiler_version: crate::version(),
        language_version: crate::LANGUAGE_VERSION,
        standard_library_abi: crate::STANDARD_LIBRARY_ABI,
        source_tree_hash: standard_resource_hash(),
        semantic_hash,
        manifest_hash,
        resources,
    })
}

pub(crate) fn linked_standard_support(
    package: &str,
    roots: &BTreeSet<String>,
    target: crate::types::PythonVersion,
) -> Result<LinkedStandardSupport, String> {
    let compiled = compiled_sources()?;
    let mut namespaces = roots
        .iter()
        .filter_map(|id| {
            NAMESPACES.iter().copied().find(|namespace| {
                super::exports(namespace).any(|binding| {
                    binding.kind != crate::name::BindingKind::Type && binding.id().as_str() == id
                })
            })
        })
        .collect::<BTreeSet<_>>();
    let mut support = LinkedStandardSupport {
        binding_ids: roots.clone(),
        ..LinkedStandardSupport::default()
    };
    let mut generated = BTreeMap::new();
    loop {
        let pending = namespaces
            .iter()
            .filter(|namespace| !generated.contains_key(**namespace))
            .copied()
            .collect::<Vec<_>>();
        if pending.is_empty() {
            break;
        }
        for namespace in pending {
            let position = NAMESPACES
                .iter()
                .position(|candidate| *candidate == namespace)
                .ok_or_else(|| format!("unknown standard namespace `{namespace}`"))?;
            let selected = support
                .binding_ids
                .iter()
                .filter(|id| super::exports(namespace).any(|binding| binding.id().as_str() == *id))
                .cloned()
                .collect::<BTreeSet<_>>();
            let mut module = compiled[position].analysis.hir.clone();
            module.items.retain(|item| {
                let binding = match &item.kind {
                    crate::hir::ItemKind::Value(value) => Some(&value.binding),
                    crate::hir::ItemKind::Function(function) => Some(&function.binding),
                    crate::hir::ItemKind::Struct(structure) => Some(&structure.binding),
                    _ => None,
                };
                binding.is_some_and(|binding| selected.contains(binding.as_str()))
            });
            let output = crate::backend::compile_module_with_runtime(&module, target, package)
                .map_err(|error| format!("could not link `{namespace}`: {error}"))?;
            if let Some(runtime) = &output.runtime_support {
                support.helpers.extend(runtime.helpers.iter().cloned());
                support
                    .binding_ids
                    .extend(runtime.binding_ids.iter().cloned());
                for binding_id in &runtime.binding_ids {
                    if let Some(owner) = NAMESPACES.iter().copied().find(|candidate| {
                        super::exports(candidate).any(|binding| {
                            binding.kind != crate::name::BindingKind::Type
                                && binding.id().as_str() == binding_id
                        })
                    }) {
                        namespaces.insert(owner);
                    }
                }
            }
            let generated_name = format!(
                "{}/stdlib/{}.py",
                package.replace('.', "/"),
                namespace.rsplit('.').next().unwrap_or("standard")
            );
            support.source_maps.push(crate::source_map::generate(
                crate::source_map::GenerateInput {
                    source_name: &sources()?[namespace].uri,
                    generated_name: &generated_name,
                    generated_source: &output.source,
                    module: &module,
                    traces: &compiled[position].analysis.expansion_traces,
                    python_target: target,
                    source_hash: &compiled[position].analysis.source_hash,
                    build_hash: &digest(output.source.as_bytes()),
                },
            ));
            generated.insert(namespace, output.source);
        }
    }

    let root = PathBuf::from(package.replace('.', "/")).join("stdlib");
    support.files.push((
        root.join("__init__.py"),
        "\"\"\"Source-compiled private Osiris standard modules.\"\"\"\n".to_owned(),
    ));
    support
        .files
        .extend(generated.into_iter().map(|(namespace, source)| {
            (
                root.join(format!(
                    "{}.py",
                    namespace.rsplit('.').next().unwrap_or("standard")
                )),
                source,
            )
        }));
    for source_map in &support.source_maps {
        let mut encoded = serde_json::to_string_pretty(source_map)
            .map_err(|error| format!("could not encode linked source map: {error}"))?;
        encoded.push('\n');
        support.files.push((
            PathBuf::from(&source_map.generated).with_extension("py.map"),
            encoded,
        ));
    }
    Ok(support)
}

fn semantic_hash_for_interfaces(interfaces: &BTreeMap<String, Interface>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(crate::STANDARD_LIBRARY_ABI.to_be_bytes());
    for (namespace, interface) in interfaces {
        hasher.update(namespace.as_bytes());
        hasher.update([0]);
        hasher.update(interface.semantic_interface_hash().as_bytes());
        hasher.update([0xff]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn source_path(namespace: &str, extension: &str) -> String {
    format!(
        "{}/{}.{}",
        namespace.replace('.', "/"),
        namespace.rsplit('.').next().unwrap_or("standard"),
        extension
    )
}

fn add_resource(
    resources: &mut Vec<StandardArtifactResource>,
    path: String,
    kind: &'static str,
    bytes: &[u8],
) {
    resources.push(StandardArtifactResource {
        path,
        kind,
        content_hash: digest(bytes),
        bytes: bytes.to_vec(),
    });
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn manifest_hash(resources: &[StandardArtifactResource]) -> String {
    let mut hasher = Sha256::new();
    for resource in resources {
        hasher.update((resource.path.len() as u64).to_be_bytes());
        hasher.update(resource.path.as_bytes());
        hasher.update(resource.kind.as_bytes());
        hasher.update(resource.content_hash.as_bytes());
    }
    format!("sha256:{:x}", hasher.finalize())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceIndex<'a> {
    schema: &'static str,
    namespace: &'a str,
    uri: &'a str,
    bindings: &'a BTreeMap<String, u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdlib::exports;

    #[test]
    fn standard_resources_are_complete_and_validated() {
        validate_standard_artifacts().expect("standard artifacts");
        let artifacts = standard_artifacts().unwrap();
        assert_eq!(artifacts.language_version, crate::LANGUAGE_VERSION);
        for namespace in NAMESPACES {
            let prefix = namespace.replace('.', "/");
            assert!(artifacts.resources.iter().any(|resource| resource.path.starts_with(&prefix) && resource.kind == "source"));
            assert!(
                artifacts
                    .resources
                    .iter()
                    .any(|resource| resource.path.starts_with(&prefix)
                        && resource.kind == "interface")
            );
            assert!(
                artifacts
                    .resources
                    .iter()
                    .any(|resource| resource.path.starts_with(&prefix)
                        && resource.kind == "source-index")
            );
        }
        assert!(
            artifacts
                .resources
                .iter()
                .any(|resource| resource.kind == "macro-ir")
        );
        assert!(
            artifacts
                .resources
                .iter()
                .any(|resource| resource.kind == "phase-helper-closure")
        );
        assert!(
            artifacts
                .resources
                .iter()
                .any(|resource| resource.kind == "linkable-helper-hir")
        );
    }

    #[test]
    fn source_locations_resolve_to_real_declarations() {
        for binding in NAMESPACES.iter().flat_map(|namespace| exports(namespace)) {
            let location = binding_source_location(binding);
            let source = source_artifact_by_uri(&location.uri).expect("standard source URI");
            let line = source.lines().nth(location.line as usize - 1).unwrap_or("");
            assert!(
                line.contains(binding.canonical),
                "{} at {}:{}",
                binding.id().as_str(),
                location.uri,
                location.line
            );
        }
    }
}
