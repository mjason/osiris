use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SupportModule {
    Control,
    Logical,
    SequenceCore,
    SequenceEager,
    SequenceTransforms,
    SequencePartitions,
    SequenceConsumers,
    Standard,
}

impl SupportModule {
    const fn path(self) -> &'static str {
        match self {
            Self::Control => "control.py",
            Self::Logical => "_logical.py",
            Self::SequenceCore => "_sequence_core.py",
            Self::SequenceEager => "_sequence_eager.py",
            Self::SequenceTransforms => "_sequence_transforms.py",
            Self::SequencePartitions => "_sequence_partitions.py",
            Self::SequenceConsumers => "_sequence_consumers.py",
            Self::Standard => "standard.py",
        }
    }

    const fn import_name(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Logical => "_logical",
            Self::SequenceCore => "_sequence_core",
            Self::SequenceEager => "_sequence_eager",
            Self::SequenceTransforms => "_sequence_transforms",
            Self::SequencePartitions => "_sequence_partitions",
            Self::SequenceConsumers => "_sequence_consumers",
            Self::Standard => "standard",
        }
    }

    const fn source(self) -> &'static str {
        match self {
            Self::Control => include_str!("runtime_templates/control.py"),
            Self::Logical => include_str!("runtime_templates/_logical.py"),
            Self::SequenceCore => include_str!("runtime_templates/_sequence_core.py"),
            Self::SequenceEager => include_str!("runtime_templates/_sequence_eager.py"),
            Self::SequenceTransforms => {
                include_str!("runtime_templates/_sequence_transforms.py")
            }
            Self::SequencePartitions => {
                include_str!("runtime_templates/_sequence_partitions.py")
            }
            Self::SequenceConsumers => {
                include_str!("runtime_templates/_sequence_consumers.py")
            }
            Self::Standard => include_str!("runtime_templates/standard.py"),
        }
    }

    fn dependencies(self) -> &'static [Self] {
        match self {
            Self::Control | Self::Logical | Self::SequenceCore => &[],
            Self::SequenceEager => &[Self::Control, Self::SequenceCore],
            Self::SequenceTransforms => &[Self::Control, Self::Logical, Self::SequenceCore],
            Self::SequencePartitions => &[
                Self::Control,
                Self::SequenceCore,
                Self::SequenceEager,
                Self::SequenceTransforms,
            ],
            Self::SequenceConsumers => &[Self::Control, Self::SequenceCore],
            Self::Standard => &[
                Self::Control,
                Self::Logical,
                Self::SequenceCore,
                Self::SequenceConsumers,
            ],
        }
    }
}

const CONTROL_HELPERS: &[&str] = &[
    "Delay",
    "Future",
    "Lock",
    "Promise",
    "assert_value",
    "binding_values",
    "close",
    "delay",
    "deliver",
    "deref",
    "dynamic_get",
    "force",
    "future_call",
    "future_cancel",
    "future_cancelled",
    "future_done",
    "is_nil",
    "lock",
    "locking",
    "present",
    "promise",
    "realized",
    "time_value",
    "truthy",
];

const LOGICAL_HELPERS: &[&str] = &["logical_map", "logical_set"];

const SEQUENCE_CORE_HELPERS: &[&str] = &[
    "coll_q",
    "concat",
    "cons",
    "count",
    "empty",
    "empty_q",
    "first",
    "lazy_seq",
    "next",
    "nth",
    "rest",
    "seq",
    "seq_q",
    "sequential_q",
];

const SEQUENCE_EAGER_HELPERS: &[&str] = &[
    "Reduced",
    "doseq",
    "filter",
    "filterv",
    "fold",
    "for_stop",
    "loop",
    "map",
    "mapcat",
    "mapcatv",
    "mapv",
    "nonempty",
    "recur",
    "reduce",
    "reduced",
    "reduced_p",
    "remove",
    "removev",
    "trampoline",
    "unreduced",
];

const SEQUENCE_TRANSFORM_HELPERS: &[&str] = &[
    "cycle",
    "dedupe",
    "distinct",
    "drop",
    "drop_while",
    "iterate",
    "keep",
    "keep_indexed",
    "map_indexed",
    "repeat",
    "repeatedly",
    "sequence",
    "take",
    "take_while",
];

const SEQUENCE_PARTITION_HELPERS: &[&str] = &[
    "drop_last",
    "interleave",
    "interpose",
    "partition",
    "partition_all",
    "partition_by",
    "reductions",
    "take_last",
];

const SEQUENCE_CONSUMER_HELPERS: &[&str] = &[
    "doall",
    "dorun",
    "every_q",
    "not_any_q",
    "not_every_q",
    "run_bang",
    "some",
];

const STANDARD_INTERNAL_HELPERS: &[&str] = &["apply"];

fn helper_module(helper: &str) -> SupportModule {
    for (module, names) in [
        (SupportModule::Control, CONTROL_HELPERS),
        (SupportModule::Logical, LOGICAL_HELPERS),
        (SupportModule::SequenceCore, SEQUENCE_CORE_HELPERS),
        (SupportModule::SequenceEager, SEQUENCE_EAGER_HELPERS),
        (
            SupportModule::SequenceTransforms,
            SEQUENCE_TRANSFORM_HELPERS,
        ),
        (
            SupportModule::SequencePartitions,
            SEQUENCE_PARTITION_HELPERS,
        ),
        (SupportModule::SequenceConsumers, SEQUENCE_CONSUMER_HELPERS),
    ] {
        if names.contains(&helper) {
            return module;
        }
    }
    SupportModule::Standard
}

fn add_closure(module: SupportModule, modules: &mut BTreeSet<SupportModule>) {
    if !modules.insert(module) {
        return;
    }
    for dependency in module.dependencies() {
        add_closure(*dependency, modules);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkableHelperCatalog {
    schema: &'static str,
    format: u32,
    nodes: Vec<LinkableHelperNode>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkableHelperNode {
    id: String,
    export: String,
    unit: &'static str,
    unit_dependencies: Vec<&'static str>,
    facade_bindings: Vec<String>,
    body: Vec<LinkableInstruction>,
    content_hash: String,
}

#[derive(Serialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
enum LinkableInstruction {
    LinkExport { unit: &'static str, name: String },
}

/// Serialize the target-neutral linker DAG embedded beside standard
/// interfaces. Python source is selected only after these stable nodes have
/// been resolved to their transitive support-unit closure.
pub(crate) fn linkable_helper_ir_bytes() -> Result<Vec<u8>, serde_json::Error> {
    let mut helpers = [
        CONTROL_HELPERS,
        LOGICAL_HELPERS,
        SEQUENCE_CORE_HELPERS,
        SEQUENCE_EAGER_HELPERS,
        SEQUENCE_TRANSFORM_HELPERS,
        SEQUENCE_PARTITION_HELPERS,
        SEQUENCE_CONSUMER_HELPERS,
        STANDARD_INTERNAL_HELPERS,
    ]
    .into_iter()
    .flatten()
    .map(|helper| (*helper).to_owned())
    .collect::<BTreeSet<_>>();
    helpers.extend(
        crate::stdlib::NAMESPACES
            .iter()
            .flat_map(|namespace| crate::stdlib::exports(namespace))
            .filter(|binding| binding.kind != crate::name::BindingKind::Macro)
            .map(crate::stdlib::StandardBinding::runtime_name),
    );
    let nodes = helpers
        .into_iter()
        .map(|helper| {
            let module = helper_module(&helper);
            let facade_bindings = crate::stdlib::NAMESPACES
                .iter()
                .flat_map(|namespace| crate::stdlib::exports(namespace))
                .filter(|binding| {
                    binding.kind != crate::name::BindingKind::Macro
                        && binding.runtime_name() == helper
                })
                .map(|binding| binding.id().as_str().to_owned())
                .collect();
            LinkableHelperNode {
                id: format!("osiris.linkable/{helper}"),
                export: helper.clone(),
                unit: module.import_name(),
                unit_dependencies: module
                    .dependencies()
                    .iter()
                    .map(|dependency| dependency.import_name())
                    .collect(),
                facade_bindings,
                body: vec![LinkableInstruction::LinkExport {
                    unit: module.import_name(),
                    name: helper,
                }],
                content_hash: format!("sha256:{:x}", Sha256::digest(module.source().as_bytes())),
            }
        })
        .collect();
    serde_json::to_vec(&LinkableHelperCatalog {
        schema: "osiris-linkable-helper-hir/v1",
        format: crate::LINKABLE_HELPER_FORMAT,
        nodes,
    })
}

/// Generate the deterministic module closure and exact root exports for one
/// distribution-private support package.
#[must_use]
pub fn runtime_support_files(package: &str, helpers: &BTreeSet<String>) -> Vec<(PathBuf, String)> {
    let root = PathBuf::from(package.replace('.', "/"));
    let mut modules = BTreeSet::new();
    for helper in helpers {
        add_closure(helper_module(helper), &mut modules);
    }

    let mut init = String::from("\"\"\"Compiler-linked private Osiris support.\"\"\"\n\n");
    for module in &modules {
        let selected = helpers
            .iter()
            .filter(|helper| helper_module(helper) == *module)
            .cloned()
            .collect::<Vec<_>>();
        if !selected.is_empty() {
            init.push_str(&format!(
                "from .{} import {}\n",
                module.import_name(),
                selected.join(", ")
            ));
        }
    }
    init.push_str("\n__all__ = [\n");
    for helper in helpers {
        init.push_str(&format!("    {helper:?},\n"));
    }
    init.push_str("]\n");

    let mut files = vec![(root.join("__init__.py"), init)];
    files.extend(
        modules
            .into_iter()
            .map(|module| (root.join(module.path()), module.source().to_owned())),
    );
    files
}

/// Materialize every private runtime file requested by one generated module.
///
/// Unlike [`runtime_support_files`], which renders only compiler-owned Kernel
/// helpers, this also compiles the reachable standard-library facades from
/// their packaged Osiris source. Library embedders should use this function
/// when writing a standalone generated distribution.
pub fn runtime_distribution_files(
    support: &super::RuntimeSupport,
    target: crate::types::PythonVersion,
) -> Result<Vec<(PathBuf, String)>, String> {
    let linked =
        crate::stdlib::linked_standard_support(&support.package, &support.binding_ids, target)?;
    let mut helpers = support.helpers.clone();
    helpers.extend(linked.helpers);
    let mut files = runtime_support_files(&support.package, &helpers);
    files.extend(linked.files);
    Ok(files)
}

/// Hash each requested helper's versioned implementation module.
#[must_use]
pub fn runtime_helper_hashes(helpers: &BTreeSet<String>) -> BTreeMap<String, String> {
    helpers
        .iter()
        .map(|helper| {
            let source = helper_module(helper).source();
            let digest = Sha256::digest(source.as_bytes());
            (helper.clone(), format!("sha256:{digest:x}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_linking_uses_the_transitive_module_closure() {
        let files = runtime_support_files(
            "demo.__osiris_runtime__",
            &BTreeSet::from(["mapv".to_owned()]),
        );
        let paths = files
            .iter()
            .map(|(path, _)| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("/_sequence_eager.py"))
        );
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("/_sequence_core.py"))
        );
        assert!(paths.iter().any(|path| path.ends_with("/control.py")));
        assert!(!paths.iter().any(|path| path.ends_with("/standard.py")));
        assert!(files[0].1.contains("from ._sequence_eager import mapv"));
    }

    #[test]
    fn logical_collection_helpers_link_their_private_module() {
        let files = runtime_support_files(
            "demo.__osiris_runtime__",
            &BTreeSet::from(["logical_map".to_owned(), "logical_set".to_owned()]),
        );
        let paths = files
            .iter()
            .map(|(path, _)| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(paths.iter().any(|path| path.ends_with("/_logical.py")));
        assert!(
            files[0]
                .1
                .contains("from ._logical import logical_map, logical_set")
        );
    }

    #[test]
    fn linkable_helper_ir_is_a_versioned_linker_dag() {
        let bytes = linkable_helper_ir_bytes().expect("linkable helper IR");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON helper IR");
        assert_eq!(value["schema"], "osiris-linkable-helper-hir/v1");
        assert_eq!(value["format"], crate::LINKABLE_HELPER_FORMAT);
        let nodes = value["nodes"].as_array().expect("helper nodes");
        assert!(nodes.iter().any(|node| {
            node["export"] == "mapv"
                && node["unit"] == "_sequence_eager"
                && node["body"][0]["op"] == "link-export"
        }));
    }
}
