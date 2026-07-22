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

/// A document-local identity that remains stable across incremental reads when
/// the corresponding source-derived form is unchanged.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct NodeId(u64);

impl NodeId {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One edge in a path from a [`Document`] to a retained source form.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "relation", rename_all = "kebab-case")]
pub enum NodePathSegment {
    TopLevel { index: usize },
    MetadataKey { index: usize },
    MetadataValue { index: usize },
    CollectionItem { index: usize },
    ReaderOperand,
}

/// The location of a node in one particular document snapshot.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct NodePath(Vec<NodePathSegment>);

impl NodePath {
    #[must_use]
    pub fn top_level(index: usize) -> Self {
        Self(vec![NodePathSegment::TopLevel { index }])
    }

    #[must_use]
    pub fn segments(&self) -> &[NodePathSegment] {
        &self.0
    }

    /// Returns a path extended by one tree relation.
    #[must_use]
    pub fn child(&self, segment: NodePathSegment) -> Self {
        let mut segments = self.0.clone();
        segments.push(segment);
        Self(segments)
    }
}

/// A compact kind tag for tooling that wants to index nodes without decoding
/// the complete recovered form tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyntaxNodeKind {
    None,
    Bool,
    Integer,
    Float,
    String,
    Keyword,
    Symbol,
    List,
    Vector,
    Map,
    Set,
    ReaderMacro,
    Error,
}

/// Serializable identity side-table entry for one retained [`Form`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NodeIdentity {
    pub id: NodeId,
    pub path: NodePath,
    pub kind: SyntaxNodeKind,
    pub span: Span,
    pub datum_span: Span,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TokenKind {
    Whitespace,
    Comment,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    LeftBrace,
    RightBrace,
    SetStart,
    Quote,
    SyntaxQuote,
    Unquote,
    UnquoteSplicing,
    Metadata,
    String,
    Atom,
    Error,
}

impl TokenKind {
    #[must_use]
    pub const fn is_trivia(self) -> bool {
        matches!(self, Self::Whitespace | Self::Comment)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Token {
    pub kind: TokenKind,
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReaderMacroKind {
    Quote,
    SyntaxQuote,
    Unquote,
    UnquoteSplicing,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Name {
    pub spelling: String,
    pub canonical: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MetadataEntry {
    pub key: Form,
    pub value: Form,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Form {
    /// Span of the complete form, including metadata prefixes.
    pub span: Span,
    /// Span of the datum itself, excluding metadata prefixes.
    pub datum_span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<MetadataEntry>,
    #[serde(flatten)]
    pub kind: FormKind,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum FormKind {
    None,
    Bool(bool),
    Integer(String),
    Float(String),
    String(String),
    Keyword(Name),
    Symbol(Name),
    List(Vec<Form>),
    Vector(Vec<Form>),
    Map(Vec<Form>),
    Set(Vec<Form>),
    ReaderMacro {
        macro_kind: ReaderMacroKind,
        form: Box<Form>,
    },
    Error(String),
}

impl Form {
    #[must_use]
    pub fn new(kind: FormKind, span: Span) -> Self {
        Self {
            span,
            datum_span: span,
            metadata: Vec::new(),
            kind,
        }
    }

    #[must_use]
    pub const fn supports_metadata(&self) -> bool {
        matches!(
            self.kind,
            FormKind::Symbol(_)
                | FormKind::List(_)
                | FormKind::Vector(_)
                | FormKind::Map(_)
                | FormKind::Set(_)
                | FormKind::ReaderMacro { .. }
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Document {
    #[serde(rename = "version")]
    pub format_version: u32,
    pub source_len: usize,
    pub tokens: Vec<Token>,
    pub forms: Vec<Form>,
    /// Identities cover every retained source-derived form, including error
    /// forms and normalized metadata key/value forms. The Reader currently
    /// lowers a metadata descriptor container into entries, so that discarded
    /// descriptor container itself has no independently addressable node.
    pub nodes: Vec<NodeIdentity>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Document {
    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.diagnostics.is_empty()
    }

    #[must_use]
    pub fn node_identity(&self, path: &NodePath) -> Option<&NodeIdentity> {
        self.nodes.iter().find(|node| &node.path == path)
    }

    #[must_use]
    pub fn node_id(&self, path: &NodePath) -> Option<NodeId> {
        self.node_identity(path).map(|node| node.id)
    }

    #[must_use]
    pub fn form_at_path(&self, path: &NodePath) -> Option<&Form> {
        let (first, rest) = path.segments().split_first()?;
        let NodePathSegment::TopLevel { index } = first else {
            return None;
        };
        descend_form(self.forms.get(*index)?, rest)
    }

    #[must_use]
    pub fn form_for_id(&self, id: NodeId) -> Option<&Form> {
        let path = &self.nodes.iter().find(|node| node.id == id)?.path;
        self.form_at_path(path)
    }
}

fn descend_form<'form>(form: &'form Form, path: &[NodePathSegment]) -> Option<&'form Form> {
    let Some((segment, rest)) = path.split_first() else {
        return Some(form);
    };
    let child = match segment {
        NodePathSegment::TopLevel { .. } => return None,
        NodePathSegment::MetadataKey { index } => &form.metadata.get(*index)?.key,
        NodePathSegment::MetadataValue { index } => &form.metadata.get(*index)?.value,
        NodePathSegment::CollectionItem { index } => match &form.kind {
            FormKind::List(items)
            | FormKind::Vector(items)
            | FormKind::Map(items)
            | FormKind::Set(items) => items.get(*index)?,
            _ => return None,
        },
        NodePathSegment::ReaderOperand => match &form.kind {
            FormKind::ReaderMacro { form, .. } => form,
            _ => return None,
        },
    };
    descend_form(child, rest)
}

impl From<&FormKind> for SyntaxNodeKind {
    fn from(kind: &FormKind) -> Self {
        match kind {
            FormKind::None => Self::None,
            FormKind::Bool(_) => Self::Bool,
            FormKind::Integer(_) => Self::Integer,
            FormKind::Float(_) => Self::Float,
            FormKind::String(_) => Self::String,
            FormKind::Keyword(_) => Self::Keyword,
            FormKind::Symbol(_) => Self::Symbol,
            FormKind::List(_) => Self::List,
            FormKind::Vector(_) => Self::Vector,
            FormKind::Map(_) => Self::Map,
            FormKind::Set(_) => Self::Set,
            FormKind::ReaderMacro { .. } => Self::ReaderMacro,
            FormKind::Error(_) => Self::Error,
        }
    }
}

pub(crate) fn source_form_eq(left: &Form, right: &Form) -> bool {
    left.metadata.len() == right.metadata.len()
        && left
            .metadata
            .iter()
            .zip(&right.metadata)
            .all(|(left, right)| {
                source_form_eq(&left.key, &right.key) && source_form_eq(&left.value, &right.value)
            })
        && match (&left.kind, &right.kind) {
            (FormKind::None, FormKind::None) => true,
            (FormKind::Bool(left), FormKind::Bool(right)) => left == right,
            (FormKind::Integer(left), FormKind::Integer(right))
            | (FormKind::Float(left), FormKind::Float(right))
            | (FormKind::String(left), FormKind::String(right))
            | (FormKind::Error(left), FormKind::Error(right)) => left == right,
            (FormKind::Keyword(left), FormKind::Keyword(right))
            | (FormKind::Symbol(left), FormKind::Symbol(right)) => left == right,
            (FormKind::List(left), FormKind::List(right))
            | (FormKind::Vector(left), FormKind::Vector(right))
            | (FormKind::Map(left), FormKind::Map(right))
            | (FormKind::Set(left), FormKind::Set(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| source_form_eq(left, right))
            }
            (
                FormKind::ReaderMacro {
                    macro_kind: left_kind,
                    form: left,
                },
                FormKind::ReaderMacro {
                    macro_kind: right_kind,
                    form: right,
                },
            ) => left_kind == right_kind && source_form_eq(left, right),
            _ => false,
        }
}

pub(crate) fn datum_eq(left: &Form, right: &Form) -> bool {
    match (&left.kind, &right.kind) {
        (FormKind::None, FormKind::None) => true,
        (FormKind::Bool(left), FormKind::Bool(right)) => left == right,
        (FormKind::Integer(left), FormKind::Integer(right))
        | (FormKind::Float(left), FormKind::Float(right))
        | (FormKind::String(left), FormKind::String(right))
        | (FormKind::Error(left), FormKind::Error(right)) => left == right,
        (FormKind::Keyword(left), FormKind::Keyword(right))
        | (FormKind::Symbol(left), FormKind::Symbol(right)) => left.canonical == right.canonical,
        (FormKind::List(left), FormKind::List(right))
        | (FormKind::Vector(left), FormKind::Vector(right)) => sequence_eq(left, right),
        (FormKind::Map(left), FormKind::Map(right)) => map_eq(left, right),
        (FormKind::Set(left), FormKind::Set(right)) => unordered_eq(left, right),
        (
            FormKind::ReaderMacro {
                macro_kind: left_kind,
                form: left,
            },
            FormKind::ReaderMacro {
                macro_kind: right_kind,
                form: right,
            },
        ) => left_kind == right_kind && datum_eq(left, right),
        _ => false,
    }
}

fn sequence_eq(left: &[Form], right: &[Form]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| datum_eq(left, right))
}

fn unordered_eq(left: &[Form], right: &[Form]) -> bool {
    left.len() == right.len() && match_unordered(left, right, datum_eq)
}

fn map_eq(left: &[Form], right: &[Form]) -> bool {
    if left.len() != right.len() || left.len() % 2 != 0 {
        return false;
    }

    let left_entries = left.chunks_exact(2).collect::<Vec<_>>();
    let right_entries = right.chunks_exact(2).collect::<Vec<_>>();
    match_unordered(&left_entries, &right_entries, |left, right| {
        datum_eq(&left[0], &right[0]) && datum_eq(&left[1], &right[1])
    })
}

fn match_unordered<T>(left: &[T], right: &[T], equals: impl Fn(&T, &T) -> bool) -> bool {
    let mut matched = vec![false; right.len()];
    left.iter().all(|item| {
        right
            .iter()
            .enumerate()
            .find(|(index, candidate)| !matched[*index] && equals(item, candidate))
            .is_some_and(|(index, _)| {
                matched[index] = true;
                true
            })
    })
}

#[cfg(test)]
mod tests {
    use super::{Form, FormKind, datum_eq};
    use crate::source::Span;

    fn integer(value: &str) -> Form {
        Form::new(FormKind::Integer(value.to_owned()), Span::default())
    }

    #[test]
    fn map_equality_preserves_key_value_pairs() {
        let left = Form::new(
            FormKind::Map(vec![integer("1"), integer("2")]),
            Span::default(),
        );
        let swapped = Form::new(
            FormKind::Map(vec![integer("2"), integer("1")]),
            Span::default(),
        );

        assert!(!datum_eq(&left, &swapped));
    }

    #[test]
    fn unordered_equality_does_not_reuse_a_match() {
        let repeated = Form::new(
            FormKind::Set(vec![integer("1"), integer("1")]),
            Span::default(),
        );
        let distinct = Form::new(
            FormKind::Set(vec![integer("1"), integer("2")]),
            Span::default(),
        );

        assert!(!datum_eq(&repeated, &distinct));
    }
}
