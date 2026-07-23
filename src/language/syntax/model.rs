/// A document-local identity stable across unchanged incremental reads.
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
