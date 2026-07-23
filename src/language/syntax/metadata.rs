use std::collections::BTreeSet;

use serde::Serialize;

use crate::{diagnostic::Diagnostic, source::Span};

/// Rich Metadata is intentionally much smaller than ordinary program data.
/// These limits apply to one syntax target. Reader entry accounting includes
/// all authored layers; depth, nodes, and bytes are checked on retained data.
/// Collection forms which are not metadata remain unrestricted by this policy.
pub(crate) const METADATA_TARGET_LIMITS: MetadataResourceLimits = MetadataResourceLimits {
    max_depth: 32,
    max_entries: 128,
    max_nodes: 2_048,
    max_normalized_bytes: 64 * 1024,
};

/// Aggregate metadata retained for one public declaration, including its
/// parameter or field metadata.
pub(crate) const METADATA_DECLARATION_LIMITS: MetadataResourceLimits = MetadataResourceLimits {
    max_depth: METADATA_TARGET_LIMITS.max_depth,
    max_entries: 512,
    max_nodes: 8_192,
    max_normalized_bytes: 256 * 1024,
};

/// Aggregate metadata retained by one `.osri` interface.
pub(crate) const METADATA_INTERFACE_LIMITS: MetadataResourceLimits = MetadataResourceLimits {
    max_depth: METADATA_TARGET_LIMITS.max_depth,
    max_entries: 4_096,
    max_nodes: 65_536,
    max_normalized_bytes: 2 * 1024 * 1024,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MetadataResourceLimits {
    pub max_depth: usize,
    pub max_entries: usize,
    pub max_nodes: usize,
    pub max_normalized_bytes: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct MetadataResourceUsage {
    pub depth: usize,
    pub entries: usize,
    pub nodes: usize,
    pub normalized_bytes: usize,
}

impl MetadataResourceUsage {
    #[must_use]
    pub fn saturating_add(self, other: Self) -> Self {
        Self {
            depth: self.depth.max(other.depth),
            entries: self.entries.saturating_add(other.entries),
            nodes: self.nodes.saturating_add(other.nodes),
            normalized_bytes: self.normalized_bytes.saturating_add(other.normalized_bytes),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MetadataLimitExceeded {
    pub resource: &'static str,
    pub actual: usize,
    pub limit: usize,
}

/// Measures one metadata map without recursion and stops as soon as a limit
/// is exceeded. Byte accounting matches the normalized reader-compatible
/// spelling; map/set sorting does not affect its size.
pub(crate) fn check_metadata_resources(
    metadata: &[MetadataEntry],
    limits: MetadataResourceLimits,
) -> Result<MetadataResourceUsage, MetadataLimitExceeded> {
    let mut usage = MetadataResourceUsage {
        entries: metadata.len(),
        normalized_bytes: collection_overhead(2, metadata.len().saturating_mul(2)),
        ..MetadataResourceUsage::default()
    };
    check_metadata_usage(usage, limits)?;

    let mut pending = Vec::with_capacity(metadata.len().saturating_mul(2));
    for entry in metadata {
        pending.push((&entry.key, 1_usize));
        pending.push((&entry.value, 1_usize));
    }

    while let Some((form, depth)) = pending.pop() {
        usage.depth = usage.depth.max(depth);
        usage.nodes = usage.nodes.saturating_add(1);
        usage.normalized_bytes = usage
            .normalized_bytes
            .saturating_add(normalized_form_local_bytes(form));

        if !form.metadata.is_empty() {
            usage.entries = usage.entries.saturating_add(form.metadata.len());
            usage.normalized_bytes = usage.normalized_bytes.saturating_add(collection_overhead(
                4,
                form.metadata.len().saturating_mul(2),
            ));
            for entry in &form.metadata {
                pending.push((&entry.key, depth.saturating_add(1)));
                pending.push((&entry.value, depth.saturating_add(1)));
            }
        }

        match &form.kind {
            FormKind::List(items)
            | FormKind::Vector(items)
            | FormKind::Map(items)
            | FormKind::Set(items) => {
                for item in items {
                    pending.push((item, depth.saturating_add(1)));
                }
            }
            FormKind::ReaderMacro { form, .. } => {
                pending.push((form, depth.saturating_add(1)));
            }
            _ => {}
        }
        check_metadata_usage(usage, limits)?;
    }
    Ok(usage)
}

pub(crate) fn check_metadata_usage(
    usage: MetadataResourceUsage,
    limits: MetadataResourceLimits,
) -> Result<(), MetadataLimitExceeded> {
    for (resource, actual, limit) in [
        ("nesting depth", usage.depth, limits.max_depth),
        ("entry count", usage.entries, limits.max_entries),
        ("node count", usage.nodes, limits.max_nodes),
        (
            "normalized byte size",
            usage.normalized_bytes,
            limits.max_normalized_bytes,
        ),
    ] {
        if actual > limit {
            return Err(MetadataLimitExceeded {
                resource,
                actual,
                limit,
            });
        }
    }
    Ok(())
}

/// Metadata data cannot contain reader forms, errors, non-finite floats, odd
/// maps, or another metadata attachment. The latter keeps metadata a plain
/// immutable datum rather than a recursively annotated syntax graph.
pub(crate) fn metadata_datum_is_serializable(form: &Form) -> bool {
    let mut pending = vec![form];
    while let Some(form) = pending.pop() {
        if !form.metadata.is_empty() {
            return false;
        }
        match &form.kind {
            FormKind::None
            | FormKind::Bool(_)
            | FormKind::Integer(_)
            | FormKind::String(_)
            | FormKind::Keyword(_)
            | FormKind::Symbol(_) => {}
            FormKind::Float(value) => {
                if !value.parse::<f64>().is_ok_and(f64::is_finite) {
                    return false;
                }
            }
            FormKind::List(items) | FormKind::Vector(items) | FormKind::Set(items) => {
                pending.extend(items);
            }
            FormKind::Map(items) => {
                if items.len() % 2 != 0 {
                    return false;
                }
                pending.extend(items);
            }
            FormKind::ReaderMacro { .. } | FormKind::Error(_) => return false,
        }
    }
    true
}

/// Return normalized aliases declared by Clojure-style rich metadata.
pub(crate) fn metadata_aliases(metadata: &[MetadataEntry], canonical: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    for entry in metadata {
        if metadata_name(&entry.key)
            .is_some_and(|name| name.trim_start_matches(':') == "osiris/names")
        {
            collect_metadata_names(&entry.value, &mut names);
        }
    }
    names.remove(canonical);
    names.into_iter().collect()
}

fn collect_metadata_names(form: &Form, names: &mut BTreeSet<String>) {
    let FormKind::Map(entries) = &form.kind else {
        return;
    };
    for pair in entries.chunks_exact(2) {
        match metadata_name(&pair[0])
            .unwrap_or_default()
            .trim_start_matches(':')
        {
            "preferred" => {
                if let FormKind::Symbol(name) = &pair[1].kind {
                    names.insert(name.canonical.clone());
                }
            }
            "aliases" => {
                if let FormKind::Vector(values) = &pair[1].kind {
                    names.extend(values.iter().filter_map(|value| {
                        let FormKind::Symbol(name) = &value.kind else {
                            return None;
                        };
                        Some(name.canonical.clone())
                    }));
                }
            }
            _ => collect_metadata_names(&pair[1], names),
        }
    }
}

fn metadata_name(form: &Form) -> Option<&str> {
    match &form.kind {
        FormKind::Keyword(name) | FormKind::Symbol(name) => Some(&name.canonical),
        _ => None,
    }
}

fn collection_overhead(delimiters: usize, item_count: usize) -> usize {
    delimiters.saturating_add(item_count.saturating_sub(1))
}

fn normalized_form_local_bytes(form: &Form) -> usize {
    match &form.kind {
        FormKind::None => 4,
        FormKind::Bool(true) => 4,
        FormKind::Bool(false) => 5,
        FormKind::Integer(value) | FormKind::Float(value) => value.len(),
        FormKind::String(value) => normalized_string_bytes(value),
        FormKind::Keyword(name) | FormKind::Symbol(name) => name.canonical.len(),
        FormKind::List(items) | FormKind::Vector(items) | FormKind::Map(items) => {
            collection_overhead(2, items.len())
        }
        FormKind::Set(items) => collection_overhead(3, items.len()),
        FormKind::ReaderMacro { macro_kind, .. } => match macro_kind {
            ReaderMacroKind::UnquoteSplicing => 2,
            _ => 1,
        },
        FormKind::Error(message) => 9_usize.saturating_add(message.len()),
    }
}

fn normalized_string_bytes(value: &str) -> usize {
    value.chars().fold(2_usize, |size, character| {
        size.saturating_add(match character {
            '"' | '\\' | '\u{08}' | '\u{0c}' | '\n' | '\r' | '\t' => 2,
            character if character <= '\u{1f}' => 6,
            character => character.len_utf8(),
        })
    })
}
