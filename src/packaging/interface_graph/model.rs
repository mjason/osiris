use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use serde::Serialize;

use crate::{
    hash::{push_field, sha256},
    interface::{COMPILER_ABI, Interface, LANGUAGE_ABI},
    module_graph::{EdgeKind, ModuleEdge},
};

const SEMANTIC_GROUP_HASH_VERSION: &str = "osiris-semantic-interface-group-v1";
const TOOLING_GROUP_HASH_VERSION: &str = "osiris-tooling-metadata-group-v1";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct InterfaceBodyHashes {
    pub semantic_body: String,
    pub tooling_body: String,
}

impl InterfaceBodyHashes {
    #[must_use]
    pub fn from_interface(interface: &Interface) -> Self {
        Self {
            semantic_body: interface.hashes.semantic_body.clone(),
            tooling_body: interface.hashes.tooling_body.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct PublishedInterfaceHashes {
    pub semantic_interface: String,
    pub tooling_metadata: String,
}

impl PublishedInterfaceHashes {
    #[must_use]
    pub fn legacy_body_hashes(interface: &Interface) -> Self {
        Self {
            semantic_interface: interface.hashes.semantic_body.clone(),
            tooling_metadata: interface.hashes.tooling_body.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct InterfaceHashEdge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

impl From<&ModuleEdge> for InterfaceHashEdge {
    fn from(edge: &ModuleEdge) -> Self {
        Self {
            from: edge.from.clone(),
            to: edge.to.clone(),
            kind: edge.kind,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedHashDependency {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    pub semantic_interface_hash: String,
    pub tooling_metadata_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InterfaceHashMember {
    pub module: String,
    pub semantic_body_hash: String,
    pub tooling_body_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InterfaceHashGroup {
    pub id: String,
    pub members: Vec<InterfaceHashMember>,
    pub internal_edges: Vec<InterfaceHashEdge>,
    pub external_dependencies: Vec<ResolvedHashDependency>,
    pub semantic_interface_hash: String,
    pub tooling_metadata_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemberInterfaceHashes {
    pub group: String,
    pub semantic_interface_hash: String,
    pub tooling_metadata_hash: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct InterfaceGraphHashes {
    pub groups: Vec<InterfaceHashGroup>,
    pub members: BTreeMap<String, MemberInterfaceHashes>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InterfaceGraphHashError {
    EmptyModule,
    DuplicateProvider(String),
    UnknownImporter(String),
    MissingDependency { from: String, to: String },
    InvalidHash { owner: String, value: String },
    InvalidGroup(String),
    ComponentCycle(String),
}

impl fmt::Display for InterfaceGraphHashError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyModule => formatter.write_str("interface graph contains an empty module id"),
            Self::DuplicateProvider(module) => write!(
                formatter,
                "interface `{module}` is present as both a local and external provider"
            ),
            Self::UnknownImporter(module) => {
                write!(formatter, "interface edge has unknown importer `{module}`")
            }
            Self::MissingDependency { from, to } => {
                write!(formatter, "interface `{from}` depends on missing `{to}`")
            }
            Self::InvalidHash { owner, value } => {
                write!(formatter, "interface `{owner}` has invalid hash `{value}`")
            }
            Self::InvalidGroup(message) => {
                write!(formatter, "invalid interface hash group: {message}")
            }
            Self::ComponentCycle(component) => write!(
                formatter,
                "condensed interface graph unexpectedly cycles at `{component}`"
            ),
        }
    }
}

impl Error for InterfaceGraphHashError {}

pub fn verify_interface_hash_group(
    group: &InterfaceHashGroup,
) -> Result<(), InterfaceGraphHashError> {
    if group.members.is_empty() {
        return Err(InterfaceGraphHashError::InvalidGroup(
            "group has no members".to_owned(),
        ));
    }
    let mut members = group.members.clone();
    members.sort_by(|left, right| left.module.cmp(&right.module));
    if members != group.members
        || members
            .windows(2)
            .any(|pair| pair[0].module == pair[1].module)
    {
        return Err(InterfaceGraphHashError::InvalidGroup(
            "members must be unique and sorted by module".to_owned(),
        ));
    }
    if group.id != members[0].module {
        return Err(InterfaceGraphHashError::InvalidGroup(
            "group id must equal its first module".to_owned(),
        ));
    }
    let member_names = members
        .iter()
        .map(|member| member.module.as_str())
        .collect::<BTreeSet<_>>();
    for member in &members {
        validate_hash(&member.module, &member.semantic_body_hash)?;
        validate_hash(&member.module, &member.tooling_body_hash)?;
    }

    let mut internal_edges = group.internal_edges.clone();
    internal_edges.sort();
    internal_edges.dedup();
    if internal_edges != group.internal_edges
        || internal_edges.iter().any(|edge| {
            !member_names.contains(edge.from.as_str()) || !member_names.contains(edge.to.as_str())
        })
    {
        return Err(InterfaceGraphHashError::InvalidGroup(
            "internal edges must be unique, sorted, and remain within the group".to_owned(),
        ));
    }

    let mut dependencies = group.external_dependencies.clone();
    dependencies.sort_by(|left, right| {
        (&left.from, left.kind, &left.to).cmp(&(&right.from, right.kind, &right.to))
    });
    dependencies.dedup();
    if dependencies != group.external_dependencies
        || dependencies.iter().any(|dependency| {
            !member_names.contains(dependency.from.as_str())
                || member_names.contains(dependency.to.as_str())
        })
    {
        return Err(InterfaceGraphHashError::InvalidGroup(
            "external dependencies must be unique, sorted, and leave the group".to_owned(),
        ));
    }
    let mut targets = BTreeMap::<&str, (&str, &str)>::new();
    for dependency in &dependencies {
        validate_hash(&dependency.to, &dependency.semantic_interface_hash)?;
        validate_hash(&dependency.to, &dependency.tooling_metadata_hash)?;
        let hashes = (
            dependency.semantic_interface_hash.as_str(),
            dependency.tooling_metadata_hash.as_str(),
        );
        if targets
            .insert(dependency.to.as_str(), hashes)
            .is_some_and(|previous| previous != hashes)
        {
            return Err(InterfaceGraphHashError::InvalidGroup(format!(
                "dependency `{}` resolves to inconsistent hashes",
                dependency.to
            )));
        }
    }

    validate_hash(&group.id, &group.semantic_interface_hash)?;
    validate_hash(&group.id, &group.tooling_metadata_hash)?;
    let semantic = semantic_group_hash(&members, &internal_edges, &dependencies);
    if semantic != group.semantic_interface_hash {
        return Err(InterfaceGraphHashError::InvalidGroup(
            "semantic interface hash does not match the group body".to_owned(),
        ));
    }
    let tooling = tooling_group_hash(&members, &semantic, &dependencies);
    if tooling != group.tooling_metadata_hash {
        return Err(InterfaceGraphHashError::InvalidGroup(
            "tooling metadata hash does not match the group body".to_owned(),
        ));
    }
    Ok(())
}
