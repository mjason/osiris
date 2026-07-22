//! Recoverable S-expression grammar built over the lossless token stream.
//!
//! `nom` provides the parser contract and composition layer. Recovery is kept
//! inside individual grammar productions so an incomplete editor buffer still
//! produces forms, metadata, and stable diagnostics.

use std::collections::{BTreeMap, VecDeque};

use nom::{
    IResult, Parser,
    branch::alt,
    error::{Error, ErrorKind},
};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::{
    diagnostic::Diagnostic,
    lexer::lex,
    source::Span,
    syntax::{
        Document, Form, FormKind, METADATA_TARGET_LIMITS, MetadataEntry, Name, NodeId,
        NodeIdentity, NodePath, NodePathSegment, ReaderMacroKind, SyntaxNodeKind, Token, TokenKind,
        check_metadata_resources, datum_eq, metadata_datum_is_serializable, source_form_eq,
    },
};

// Keep recursive reader productions below the default Rust thread stack while
// still allowing realistic generated forms.  Inputs beyond this bound become
// recoverable error forms instead of risking a process-level stack overflow.
const MAX_DEPTH: usize = 96;
const MAX_DIAGNOSTICS: usize = 100;

type TokenInput<'source> = &'source [&'source Token];
type ParseResult<'source> = IResult<TokenInput<'source>, ParsedForm>;

#[derive(Debug)]
struct ParsedForm {
    form: Form,
    diagnostics: Vec<Diagnostic>,
}

impl ParsedForm {
    fn new(form: Form) -> Self {
        Self {
            form,
            diagnostics: Vec::new(),
        }
    }

    fn error(form: Form, diagnostic: Diagnostic) -> Self {
        Self {
            form,
            diagnostics: vec![diagnostic],
        }
    }
}

/// Reads a complete source file into a lossless token stream and recoverable form tree.
#[must_use]
pub fn read(source: &str) -> Document {
    read_snapshot(source, None)
}

/// Reads a new source snapshot while retaining identities for unchanged forms
/// from `previous`. Parsing remains fully recoverable; this API never mutates
/// or borrows the previous snapshot in the returned document.
#[must_use]
pub fn read_incremental(source: &str, previous: &Document) -> Document {
    read_snapshot(source, Some(previous))
}

fn read_snapshot(source: &str, previous: Option<&Document>) -> Document {
    let lexed = lex(source);
    let significant = lexed
        .tokens
        .iter()
        .filter(|token| !token.kind.is_trivia())
        .collect::<Vec<_>>();
    let mut input = significant.as_slice();
    let mut forms = Vec::new();
    let mut diagnostics = lexed.diagnostics;

    while !input.is_empty() {
        match parse_form(input, 0, source.len()) {
            Ok((rest, parsed)) if rest.len() < input.len() => {
                input = rest;
                forms.push(parsed.form);
                diagnostics.extend(parsed.diagnostics);
            }
            Ok(_) | Err(_) => {
                let token = input[0];
                diagnostics.push(Diagnostic::error(
                    "OSR-R0013",
                    "reader could not make progress at this token",
                    token.span,
                ));
                forms.push(Form::new(
                    FormKind::Error("unreadable token".to_owned()),
                    token.span,
                ));
                input = &input[1..];
            }
        }
    }

    diagnostics
        .sort_by_key(|diagnostic| (diagnostic.span.start, diagnostic.span.end, diagnostic.code));
    diagnostics.truncate(MAX_DIAGNOSTICS);

    drop(significant);
    let nodes = build_node_identities(source, &forms, previous);
    Document {
        format_version: 1,
        source_len: source.len(),
        tokens: lexed.tokens,
        forms,
        nodes,
        diagnostics,
    }
}

#[derive(Clone)]
struct LocatedForm<'form> {
    form: &'form Form,
    path: NodePath,
}

struct IdentityMatcher<'previous> {
    previous_ids: BTreeMap<&'previous NodePath, NodeId>,
    assigned: BTreeMap<NodePath, NodeId>,
    edit_map: EditMap,
}

#[derive(Clone, Copy)]
struct EditMap {
    previous_len: usize,
    current_len: usize,
    unchanged_prefix: usize,
    unchanged_suffix: usize,
}

impl EditMap {
    fn between(previous: &str, current: &str) -> Self {
        let mut unchanged_prefix = previous
            .bytes()
            .zip(current.bytes())
            .take_while(|(previous, current)| previous == current)
            .count();
        while unchanged_prefix > 0
            && (!previous.is_char_boundary(unchanged_prefix)
                || !current.is_char_boundary(unchanged_prefix))
        {
            unchanged_prefix -= 1;
        }
        let maximum_suffix = previous
            .len()
            .min(current.len())
            .saturating_sub(unchanged_prefix);
        let mut unchanged_suffix = previous
            .bytes()
            .rev()
            .zip(current.bytes().rev())
            .take(maximum_suffix)
            .take_while(|(previous, current)| previous == current)
            .count();
        while unchanged_suffix > 0
            && (!previous.is_char_boundary(previous.len() - unchanged_suffix)
                || !current.is_char_boundary(current.len() - unchanged_suffix))
        {
            unchanged_suffix -= 1;
        }
        Self {
            previous_len: previous.len(),
            current_len: current.len(),
            unchanged_prefix,
            unchanged_suffix,
        }
    }

    fn map_span(self, span: Span) -> Option<Span> {
        if span.end <= self.unchanged_prefix {
            return Some(span);
        }
        if span.start < self.previous_len.saturating_sub(self.unchanged_suffix) {
            return None;
        }
        let start_from_end = self.previous_len.saturating_sub(span.start);
        let end_from_end = self.previous_len.saturating_sub(span.end);
        Some(Span::new(
            self.current_len.checked_sub(start_from_end)?,
            self.current_len.checked_sub(end_from_end)?,
        ))
    }
}

fn build_node_identities(
    source: &str,
    forms: &[Form],
    previous: Option<&Document>,
) -> Vec<NodeIdentity> {
    let mut assigned = BTreeMap::new();
    let mut next_id = 1;
    if let Some(previous) = previous {
        next_id = previous
            .nodes
            .iter()
            .map(|node| node.id.get())
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let mut matcher = IdentityMatcher {
            previous_ids: previous
                .nodes
                .iter()
                .map(|node| (&node.path, node.id))
                .collect(),
            assigned: BTreeMap::new(),
            edit_map: EditMap::between(&document_source(previous), source),
        };
        matcher.reconcile_sequence(locate_top_level(&previous.forms), locate_top_level(forms));
        assigned = matcher.assigned;
    }

    let mut identities = Vec::new();
    for (index, form) in forms.iter().enumerate() {
        collect_node_identities(
            form,
            NodePath::top_level(index),
            &assigned,
            &mut next_id,
            &mut identities,
        );
    }
    identities
}

fn document_source(document: &Document) -> String {
    document
        .tokens
        .iter()
        .map(|token| token.text.as_str())
        .collect()
}

impl IdentityMatcher<'_> {
    fn reconcile_sequence(
        &mut self,
        previous: Vec<LocatedForm<'_>>,
        current: Vec<LocatedForm<'_>>,
    ) {
        let mut anchors = Vec::new();
        let mut previous_cursor = 0;
        for (current_index, current) in current.iter().enumerate() {
            let Some(relative_index) = previous[previous_cursor..].iter().position(|previous| {
                self.edit_map.map_span(previous.form.span) == Some(current.form.span)
                    && source_form_eq(previous.form, current.form)
            }) else {
                continue;
            };
            let previous_index = previous_cursor + relative_index;
            previous_cursor = previous_index + 1;
            anchors.push((previous_index, current_index));
        }

        let mut previous_start = 0;
        let mut current_start = 0;
        for (previous_index, current_index) in anchors {
            self.reconcile_structural_sequence(
                &previous[previous_start..previous_index],
                &current[current_start..current_index],
            );
            self.assign_exact_tree(&previous[previous_index], &current[current_index]);
            previous_start = previous_index + 1;
            current_start = current_index + 1;
        }
        self.reconcile_structural_sequence(&previous[previous_start..], &current[current_start..]);
    }

    fn reconcile_structural_sequence(
        &mut self,
        previous: &[LocatedForm<'_>],
        current: &[LocatedForm<'_>],
    ) {
        let mut candidates = BTreeMap::<[u8; 32], VecDeque<usize>>::new();
        for (index, located) in previous.iter().enumerate() {
            candidates
                .entry(form_fingerprint(located.form))
                .or_default()
                .push_back(index);
        }

        let mut anchors = Vec::new();
        let mut previous_cursor = 0;
        for (current_index, located) in current.iter().enumerate() {
            let fingerprint = form_fingerprint(located.form);
            let Some(indices) = candidates.get_mut(&fingerprint) else {
                continue;
            };
            while indices
                .front()
                .is_some_and(|index| *index < previous_cursor)
            {
                indices.pop_front();
            }
            let Some(position) = indices
                .iter()
                .position(|index| source_form_eq(previous[*index].form, located.form))
            else {
                continue;
            };
            let previous_index = indices
                .remove(position)
                .expect("the matching candidate position exists");
            previous_cursor = previous_index + 1;
            anchors.push((previous_index, current_index));
        }

        let mut previous_start = 0;
        let mut current_start = 0;
        for (previous_index, current_index) in anchors {
            self.reconcile_modified_range(
                &previous[previous_start..previous_index],
                &current[current_start..current_index],
            );
            self.assign_exact_tree(&previous[previous_index], &current[current_index]);
            previous_start = previous_index + 1;
            current_start = current_index + 1;
        }
        self.reconcile_modified_range(&previous[previous_start..], &current[current_start..]);
    }

    fn reconcile_modified_range(
        &mut self,
        previous: &[LocatedForm<'_>],
        current: &[LocatedForm<'_>],
    ) {
        let mut previous_cursor = 0;
        for current in current {
            let Some(relative_index) = previous[previous_cursor..]
                .iter()
                .position(|candidate| same_form_shape(candidate.form, current.form))
            else {
                continue;
            };
            let previous = &previous[previous_cursor + relative_index];
            previous_cursor += relative_index + 1;
            self.reconcile_children(previous, current);
        }
    }

    fn assign_exact_tree(&mut self, previous: &LocatedForm<'_>, current: &LocatedForm<'_>) {
        if let Some(id) = self.previous_ids.get(&previous.path).copied() {
            self.assigned.insert(current.path.clone(), id);
        }
        for (previous, current) in corresponding_children(previous, current) {
            self.assign_exact_tree(&previous, &current);
        }
    }

    fn reconcile_children(&mut self, previous: &LocatedForm<'_>, current: &LocatedForm<'_>) {
        self.reconcile_sequence(
            locate_metadata_keys(previous.form, &previous.path),
            locate_metadata_keys(current.form, &current.path),
        );
        self.reconcile_sequence(
            locate_metadata_values(previous.form, &previous.path),
            locate_metadata_values(current.form, &current.path),
        );
        self.reconcile_sequence(
            locate_kind_children(previous.form, &previous.path),
            locate_kind_children(current.form, &current.path),
        );
    }
}

fn locate_top_level(forms: &[Form]) -> Vec<LocatedForm<'_>> {
    forms
        .iter()
        .enumerate()
        .map(|(index, form)| LocatedForm {
            form,
            path: NodePath::top_level(index),
        })
        .collect()
}

fn locate_metadata_keys<'form>(form: &'form Form, path: &NodePath) -> Vec<LocatedForm<'form>> {
    form.metadata
        .iter()
        .enumerate()
        .map(|(index, entry)| LocatedForm {
            form: &entry.key,
            path: path.child(NodePathSegment::MetadataKey { index }),
        })
        .collect()
}

fn locate_metadata_values<'form>(form: &'form Form, path: &NodePath) -> Vec<LocatedForm<'form>> {
    form.metadata
        .iter()
        .enumerate()
        .map(|(index, entry)| LocatedForm {
            form: &entry.value,
            path: path.child(NodePathSegment::MetadataValue { index }),
        })
        .collect()
}

fn locate_kind_children<'form>(form: &'form Form, path: &NodePath) -> Vec<LocatedForm<'form>> {
    match &form.kind {
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => items
            .iter()
            .enumerate()
            .map(|(index, form)| LocatedForm {
                form,
                path: path.child(NodePathSegment::CollectionItem { index }),
            })
            .collect(),
        FormKind::ReaderMacro { form, .. } => vec![LocatedForm {
            form,
            path: path.child(NodePathSegment::ReaderOperand),
        }],
        _ => Vec::new(),
    }
}

fn corresponding_children<'previous, 'current>(
    previous: &LocatedForm<'previous>,
    current: &LocatedForm<'current>,
) -> Vec<(LocatedForm<'previous>, LocatedForm<'current>)> {
    let previous_metadata_keys = locate_metadata_keys(previous.form, &previous.path);
    let current_metadata_keys = locate_metadata_keys(current.form, &current.path);
    let previous_metadata_values = locate_metadata_values(previous.form, &previous.path);
    let current_metadata_values = locate_metadata_values(current.form, &current.path);
    let previous_children = locate_kind_children(previous.form, &previous.path);
    let current_children = locate_kind_children(current.form, &current.path);
    previous_metadata_keys
        .into_iter()
        .zip(current_metadata_keys)
        .chain(
            previous_metadata_values
                .into_iter()
                .zip(current_metadata_values),
        )
        .chain(previous_children.into_iter().zip(current_children))
        .collect()
}

fn collect_node_identities(
    form: &Form,
    path: NodePath,
    assigned: &BTreeMap<NodePath, NodeId>,
    next_id: &mut u64,
    identities: &mut Vec<NodeIdentity>,
) {
    let id = assigned.get(&path).copied().unwrap_or_else(|| {
        let id = NodeId::new(*next_id);
        *next_id = next_id.saturating_add(1);
        id
    });
    identities.push(NodeIdentity {
        id,
        path: path.clone(),
        kind: SyntaxNodeKind::from(&form.kind),
        span: form.span,
        datum_span: form.datum_span,
    });
    for child in locate_metadata_keys(form, &path)
        .into_iter()
        .chain(locate_metadata_values(form, &path))
        .chain(locate_kind_children(form, &path))
    {
        collect_node_identities(child.form, child.path, assigned, next_id, identities);
    }
}

fn same_form_shape(previous: &Form, current: &Form) -> bool {
    std::mem::discriminant(&previous.kind) == std::mem::discriminant(&current.kind)
}

fn form_fingerprint(form: &Form) -> [u8; 32] {
    let mut hasher = Sha256::new();
    update_form_fingerprint(&mut hasher, form);
    hasher.finalize().into()
}

fn update_form_fingerprint(hasher: &mut Sha256, form: &Form) {
    update_usize(hasher, form.metadata.len());
    for entry in &form.metadata {
        update_form_fingerprint(hasher, &entry.key);
        update_form_fingerprint(hasher, &entry.value);
    }
    match &form.kind {
        FormKind::None => hasher.update([0]),
        FormKind::Bool(value) => hasher.update([1, u8::from(*value)]),
        FormKind::Integer(value) => update_tagged_text(hasher, 2, value),
        FormKind::Float(value) => update_tagged_text(hasher, 3, value),
        FormKind::String(value) => update_tagged_text(hasher, 4, value),
        FormKind::Keyword(name) => {
            hasher.update([5]);
            update_text(hasher, &name.spelling);
            update_text(hasher, &name.canonical);
        }
        FormKind::Symbol(name) => {
            hasher.update([6]);
            update_text(hasher, &name.spelling);
            update_text(hasher, &name.canonical);
        }
        FormKind::List(items) => update_collection_fingerprint(hasher, 7, items),
        FormKind::Vector(items) => update_collection_fingerprint(hasher, 8, items),
        FormKind::Map(items) => update_collection_fingerprint(hasher, 9, items),
        FormKind::Set(items) => update_collection_fingerprint(hasher, 10, items),
        FormKind::ReaderMacro { macro_kind, form } => {
            hasher.update([
                11,
                match macro_kind {
                    ReaderMacroKind::Quote => 0,
                    ReaderMacroKind::SyntaxQuote => 1,
                    ReaderMacroKind::Unquote => 2,
                    ReaderMacroKind::UnquoteSplicing => 3,
                },
            ]);
            update_form_fingerprint(hasher, form);
        }
        FormKind::Error(message) => update_tagged_text(hasher, 12, message),
    }
}

fn update_collection_fingerprint(hasher: &mut Sha256, tag: u8, items: &[Form]) {
    hasher.update([tag]);
    update_usize(hasher, items.len());
    for item in items {
        update_form_fingerprint(hasher, item);
    }
}

fn update_tagged_text(hasher: &mut Sha256, tag: u8, text: &str) {
    hasher.update([tag]);
    update_text(hasher, text);
}

fn update_text(hasher: &mut Sha256, text: &str) {
    update_usize(hasher, text.len());
    hasher.update(text.as_bytes());
}

fn update_usize(hasher: &mut Sha256, value: usize) {
    hasher.update(u64::try_from(value).unwrap_or(u64::MAX).to_le_bytes());
}

fn parse_form(input: TokenInput<'_>, depth: usize, eof_offset: usize) -> ParseResult<'_> {
    let Some(first) = input.first().copied() else {
        return Err(nom::Err::Error(Error::new(input, ErrorKind::Eof)));
    };
    if depth >= MAX_DEPTH {
        return Ok((
            &input[1..],
            ParsedForm::error(
                Form::new(FormKind::Error("reader depth limit".to_owned()), first.span),
                Diagnostic::error(
                    "OSR-R0010",
                    format!("reader nesting exceeds the limit of {MAX_DEPTH}"),
                    first.span,
                ),
            ),
        ));
    }

    alt((
        |input| {
            parse_collection(
                input,
                depth,
                eof_offset,
                TokenKind::LeftParen,
                TokenKind::RightParen,
                ")",
                CollectionKind::List,
            )
        },
        |input| {
            parse_collection(
                input,
                depth,
                eof_offset,
                TokenKind::LeftBracket,
                TokenKind::RightBracket,
                "]",
                CollectionKind::Vector,
            )
        },
        |input| {
            parse_collection(
                input,
                depth,
                eof_offset,
                TokenKind::LeftBrace,
                TokenKind::RightBrace,
                "}",
                CollectionKind::Map,
            )
        },
        |input| {
            parse_collection(
                input,
                depth,
                eof_offset,
                TokenKind::SetStart,
                TokenKind::RightBrace,
                "}",
                CollectionKind::Set,
            )
        },
        |input| {
            parse_prefix(
                input,
                depth,
                eof_offset,
                TokenKind::Quote,
                ReaderMacroKind::Quote,
            )
        },
        |input| {
            parse_prefix(
                input,
                depth,
                eof_offset,
                TokenKind::SyntaxQuote,
                ReaderMacroKind::SyntaxQuote,
            )
        },
        |input| {
            parse_prefix(
                input,
                depth,
                eof_offset,
                TokenKind::Unquote,
                ReaderMacroKind::Unquote,
            )
        },
        |input| {
            parse_prefix(
                input,
                depth,
                eof_offset,
                TokenKind::UnquoteSplicing,
                ReaderMacroKind::UnquoteSplicing,
            )
        },
        |input| parse_metadata(input, depth, eof_offset),
        parse_string,
        parse_atom,
        parse_lexical_error,
        parse_unexpected_closing,
    ))
    .parse(input)
}

#[allow(clippy::too_many_arguments)]
fn parse_collection<'source>(
    input: TokenInput<'source>,
    depth: usize,
    eof_offset: usize,
    opener_kind: TokenKind,
    closer_kind: TokenKind,
    closer_spelling: &'static str,
    collection_kind: CollectionKind,
) -> ParseResult<'source> {
    let (mut rest, opener) = exact_token(input, opener_kind)?;
    let mut items = Vec::new();
    let mut diagnostics = Vec::new();

    let end = loop {
        let Some(next) = rest.first().copied() else {
            diagnostics.push(Diagnostic::error(
                "OSR-R0002",
                format!("missing closing delimiter `{closer_spelling}`"),
                Span::empty(eof_offset),
            ));
            break eof_offset;
        };

        if next.kind == closer_kind {
            rest = &rest[1..];
            break next.span.end;
        }
        if is_closing(next.kind) {
            diagnostics.push(Diagnostic::error(
                "OSR-R0003",
                format!(
                    "expected closing delimiter `{closer_spelling}` before `{}`",
                    next.text
                ),
                next.span,
            ));
            break next.span.start;
        }

        match parse_form(rest, depth + 1, eof_offset) {
            Ok((next_rest, parsed)) if next_rest.len() < rest.len() => {
                rest = next_rest;
                items.push(parsed.form);
                diagnostics.extend(parsed.diagnostics);
            }
            _ => {
                diagnostics.push(Diagnostic::error(
                    "OSR-R0013",
                    "reader could not make progress inside a collection",
                    next.span,
                ));
                items.push(Form::new(
                    FormKind::Error("unreadable collection item".to_owned()),
                    next.span,
                ));
                rest = &rest[1..];
            }
        }
    };

    let span = Span::new(opener.span.start, end);
    validate_collection(collection_kind, &items, span, &mut diagnostics);
    let kind = match collection_kind {
        CollectionKind::List => FormKind::List(items),
        CollectionKind::Vector => FormKind::Vector(items),
        CollectionKind::Map => FormKind::Map(items),
        CollectionKind::Set => FormKind::Set(items),
    };
    Ok((
        rest,
        ParsedForm {
            form: Form::new(kind, span),
            diagnostics,
        },
    ))
}

fn parse_prefix<'source>(
    input: TokenInput<'source>,
    depth: usize,
    eof_offset: usize,
    prefix_kind: TokenKind,
    macro_kind: ReaderMacroKind,
) -> ParseResult<'source> {
    let (rest, prefix) = exact_token(input, prefix_kind)?;
    let Some(next) = rest.first().copied() else {
        return Ok((
            rest,
            ParsedForm::error(
                Form::new(
                    FormKind::Error("missing reader prefix operand".to_owned()),
                    prefix.span,
                ),
                Diagnostic::error(
                    "OSR-R0004",
                    format!("reader prefix `{}` requires a form", prefix.text),
                    prefix.span,
                ),
            ),
        ));
    };
    if is_closing(next.kind) {
        return Ok((
            rest,
            ParsedForm::error(
                Form::new(
                    FormKind::Error("missing reader prefix operand".to_owned()),
                    prefix.span,
                ),
                Diagnostic::error(
                    "OSR-R0004",
                    format!("reader prefix `{}` requires a form", prefix.text),
                    prefix.span,
                ),
            ),
        ));
    }

    let (rest, parsed) = parse_form(rest, depth + 1, eof_offset)?;
    let span = prefix.span.cover(parsed.form.span);
    Ok((
        rest,
        ParsedForm {
            form: Form::new(
                FormKind::ReaderMacro {
                    macro_kind,
                    form: Box::new(parsed.form),
                },
                span,
            ),
            diagnostics: parsed.diagnostics,
        },
    ))
}

fn parse_metadata(input: TokenInput<'_>, depth: usize, eof_offset: usize) -> ParseResult<'_> {
    let (mut rest, first_caret) = exact_token(input, TokenKind::Metadata)?;
    let start = first_caret.span.start;
    let mut current_caret = first_caret;
    let mut last_end = current_caret.span.end;
    let mut layers = Vec::new();
    let mut authored_entries = 0_usize;
    let mut resource_exceeded = false;
    let mut diagnostics = Vec::new();

    loop {
        let Some(next) = rest.first().copied() else {
            diagnostics.push(Diagnostic::error(
                "OSR-R0004",
                "metadata prefix `^` requires a descriptor and target",
                current_caret.span,
            ));
            return Ok((
                rest,
                ParsedForm {
                    form: Form::new(
                        FormKind::Error("missing metadata descriptor".to_owned()),
                        Span::new(start, last_end),
                    ),
                    diagnostics,
                },
            ));
        };
        if is_closing(next.kind) || next.kind == TokenKind::Metadata {
            diagnostics.push(Diagnostic::error(
                "OSR-R0004",
                "metadata prefix `^` requires a descriptor",
                current_caret.span,
            ));
            return Ok((
                rest,
                ParsedForm {
                    form: Form::new(
                        FormKind::Error("missing metadata descriptor".to_owned()),
                        Span::new(start, last_end),
                    ),
                    diagnostics,
                },
            ));
        }

        let (next_rest, descriptor) = parse_form(rest, depth + 1, eof_offset)?;
        last_end = descriptor.form.span.end;
        diagnostics.extend(descriptor.diagnostics);
        let descriptor_entries = match &descriptor.form.kind {
            FormKind::Map(items) if items.len() / 2 > METADATA_TARGET_LIMITS.max_entries => {
                resource_exceeded = true;
                diagnostics.push(Diagnostic::error(
                    "OSR-R0014",
                    format!(
                        "metadata for one syntax target exceeds the entry count limit of {} (found {})",
                        METADATA_TARGET_LIMITS.max_entries,
                        items.len() / 2
                    ),
                    descriptor.form.span,
                ));
                Vec::new()
            }
            _ => normalize_metadata_descriptor(&descriptor.form, &mut diagnostics),
        };
        let next_entry_count = authored_entries.saturating_add(descriptor_entries.len());
        if next_entry_count > METADATA_TARGET_LIMITS.max_entries {
            if !resource_exceeded {
                diagnostics.push(Diagnostic::error(
                    "OSR-R0014",
                    format!(
                        "metadata for one syntax target exceeds the entry count limit of {} (found {})",
                        METADATA_TARGET_LIMITS.max_entries, next_entry_count
                    ),
                    Span::new(start, descriptor.form.span.end),
                ));
            }
            resource_exceeded = true;
        } else if !resource_exceeded {
            authored_entries = next_entry_count;
            layers.push(descriptor_entries);
        }
        rest = next_rest;

        if rest
            .first()
            .is_some_and(|token| token.kind == TokenKind::Metadata)
        {
            current_caret = rest[0];
            last_end = current_caret.span.end;
            rest = &rest[1..];
        } else {
            break;
        }
    }

    let Some(next) = rest.first().copied() else {
        diagnostics.push(Diagnostic::error(
            "OSR-R0004",
            "metadata descriptor requires a target form",
            Span::empty(last_end),
        ));
        return Ok((
            rest,
            ParsedForm {
                form: Form::new(
                    FormKind::Error("missing metadata target".to_owned()),
                    Span::new(start, last_end),
                ),
                diagnostics,
            },
        ));
    };
    if is_closing(next.kind) {
        diagnostics.push(Diagnostic::error(
            "OSR-R0004",
            "metadata descriptor requires a target form",
            Span::empty(last_end),
        ));
        return Ok((
            rest,
            ParsedForm {
                form: Form::new(
                    FormKind::Error("missing metadata target".to_owned()),
                    Span::new(start, last_end),
                ),
                diagnostics,
            },
        ));
    }

    let (rest, mut target) = parse_form(rest, depth + 1, eof_offset)?;
    if !target.form.supports_metadata() {
        diagnostics.push(Diagnostic::error(
            "OSR-R0009",
            "metadata can only be attached to a symbol or collection form",
            target.form.datum_span,
        ));
    } else {
        let metadata = merge_metadata_layers(layers);
        match (!resource_exceeded)
            .then(|| check_metadata_resources(&metadata, METADATA_TARGET_LIMITS))
        {
            None => {}
            Some(Ok(_)) => target.form.metadata = metadata,
            Some(Err(exceeded)) => diagnostics.push(Diagnostic::error(
                "OSR-R0014",
                format!(
                    "metadata for one syntax target exceeds the {} limit of {} (found {})",
                    exceeded.resource, exceeded.limit, exceeded.actual
                ),
                Span::new(start, target.form.datum_span.start),
            )),
        }
    }
    target.form.span = Span::new(start, target.form.span.end);
    diagnostics.extend(target.diagnostics);

    Ok((
        rest,
        ParsedForm {
            form: target.form,
            diagnostics,
        },
    ))
}

fn parse_string(input: TokenInput<'_>) -> ParseResult<'_> {
    let (rest, token) = exact_token(input, TokenKind::String)?;
    match decode_string(&token.text) {
        Ok(value) => Ok((
            rest,
            ParsedForm::new(Form::new(FormKind::String(value), token.span)),
        )),
        Err(message) => Ok((
            rest,
            ParsedForm::error(
                Form::new(
                    FormKind::Error("invalid string literal".to_owned()),
                    token.span,
                ),
                Diagnostic::error("OSR-R0012", message, token.span),
            ),
        )),
    }
}

fn parse_atom(input: TokenInput<'_>) -> ParseResult<'_> {
    let (rest, token) = exact_token(input, TokenKind::Atom)?;
    Ok((rest, ParsedForm::new(read_atom(token))))
}

fn parse_lexical_error(input: TokenInput<'_>) -> ParseResult<'_> {
    let (rest, token) = exact_token(input, TokenKind::Error)?;
    Ok((
        rest,
        ParsedForm::new(Form::new(FormKind::Error(token.text.clone()), token.span)),
    ))
}

fn parse_unexpected_closing(input: TokenInput<'_>) -> ParseResult<'_> {
    let Some(token) = input
        .first()
        .copied()
        .filter(|token| is_closing(token.kind))
    else {
        return Err(nom::Err::Error(Error::new(input, ErrorKind::Tag)));
    };
    Ok((
        &input[1..],
        ParsedForm::error(
            Form::new(
                FormKind::Error("unexpected closing delimiter".to_owned()),
                token.span,
            ),
            Diagnostic::error(
                "OSR-R0001",
                format!("unexpected closing delimiter `{}`", token.text),
                token.span,
            ),
        ),
    ))
}

fn exact_token<'source>(
    input: TokenInput<'source>,
    expected: TokenKind,
) -> IResult<TokenInput<'source>, &'source Token> {
    let Some(token) = input
        .first()
        .copied()
        .filter(|token| token.kind == expected)
    else {
        return Err(nom::Err::Error(Error::new(input, ErrorKind::Tag)));
    };
    Ok((&input[1..], token))
}

fn validate_collection(
    kind: CollectionKind,
    items: &[Form],
    span: Span,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match kind {
        CollectionKind::Map => {
            if items.len() % 2 != 0 {
                diagnostics.push(Diagnostic::error(
                    "OSR-R0006",
                    "map literal requires an even number of forms",
                    items.last().map_or(span, |item| item.span),
                ));
            }

            let entries = items.chunks_exact(2).collect::<Vec<_>>();
            for (index, entry) in entries.iter().enumerate() {
                if entries[..index]
                    .iter()
                    .any(|previous| datum_eq(&previous[0], &entry[0]))
                {
                    diagnostics.push(Diagnostic::error(
                        "OSR-R0007",
                        "duplicate map key",
                        entry[0].span,
                    ));
                }
            }
        }
        CollectionKind::Set => {
            for (index, item) in items.iter().enumerate() {
                if items[..index]
                    .iter()
                    .any(|previous| datum_eq(previous, item))
                {
                    diagnostics.push(Diagnostic::error(
                        "OSR-R0008",
                        "duplicate set item",
                        item.span,
                    ));
                }
            }
        }
        CollectionKind::List | CollectionKind::Vector => {}
    }
}

#[derive(Clone, Copy)]
enum CollectionKind {
    List,
    Vector,
    Map,
    Set,
}

fn is_closing(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::RightParen | TokenKind::RightBracket | TokenKind::RightBrace
    )
}

fn read_atom(token: &Token) -> Form {
    let kind = match token.text.as_str() {
        "none" => FormKind::None,
        "true" => FormKind::Bool(true),
        "false" => FormKind::Bool(false),
        spelling if spelling.starts_with(':') && spelling.len() > 1 => {
            FormKind::Keyword(name(spelling))
        }
        spelling => match canonical_integer(spelling) {
            Some(integer) => FormKind::Integer(integer),
            None if is_float(spelling) => {
                FormKind::Float(spelling.replace('_', "").to_ascii_lowercase())
            }
            None => FormKind::Symbol(name(spelling)),
        },
    };
    Form::new(kind, token.span)
}

fn name(spelling: &str) -> Name {
    Name {
        spelling: spelling.to_owned(),
        canonical: spelling.nfc().collect(),
    }
}

fn synthetic_keyword(spelling: &str, span: Span) -> Form {
    Form::new(FormKind::Keyword(name(spelling)), span)
}

fn canonical_integer(spelling: &str) -> Option<String> {
    let cleaned = spelling.replace('_', "");
    let (sign, digits) = match cleaned.as_bytes().first() {
        Some(b'+') => ("", &cleaned[1..]),
        Some(b'-') => ("-", &cleaned[1..]),
        _ => ("", cleaned.as_str()),
    };
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || !valid_numeric_underscores(spelling)
    {
        return None;
    }
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        Some("0".to_owned())
    } else {
        Some(format!("{sign}{digits}"))
    }
}

fn is_float(spelling: &str) -> bool {
    if !spelling
        .bytes()
        .any(|byte| matches!(byte, b'.' | b'e' | b'E'))
        || !valid_numeric_underscores(spelling)
    {
        return false;
    }
    let cleaned = spelling.replace('_', "");
    cleaned
        .parse::<f64>()
        .is_ok_and(|number| number.is_finite())
}

fn valid_numeric_underscores(spelling: &str) -> bool {
    !spelling.starts_with('_')
        && !spelling.ends_with('_')
        && !spelling.contains("__")
        && spelling.bytes().enumerate().all(|(index, byte)| {
            byte != b'_'
                || (index > 0
                    && spelling.as_bytes()[index - 1].is_ascii_digit()
                    && spelling
                        .as_bytes()
                        .get(index + 1)
                        .is_some_and(u8::is_ascii_digit))
        })
}

fn decode_string(spelling: &str) -> Result<String, String> {
    let body = spelling
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .ok_or_else(|| "unterminated string literal".to_owned())?;
    let mut result = String::new();
    let mut characters = body.chars().peekable();

    while let Some(character) = characters.next() {
        if character != '\\' {
            result.push(character);
            continue;
        }

        let escaped = characters
            .next()
            .ok_or_else(|| "unterminated string escape".to_owned())?;
        match escaped {
            '"' => result.push('"'),
            '\'' => result.push('\''),
            '\\' => result.push('\\'),
            '/' => result.push('/'),
            'a' => result.push('\u{0007}'),
            'b' => result.push('\u{0008}'),
            'f' => result.push('\u{000c}'),
            'n' => result.push('\n'),
            'r' => result.push('\r'),
            't' => result.push('\t'),
            'v' => result.push('\u{000b}'),
            '\n' => {}
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
            }
            'u' => result.push(decode_hex_escape(&mut characters, 4)?),
            'U' => result.push(decode_hex_escape(&mut characters, 8)?),
            'x' => result.push(decode_hex_escape(&mut characters, 2)?),
            digit @ '0'..='7' => {
                let mut digits = String::from(digit);
                while digits.len() < 3
                    && characters
                        .peek()
                        .is_some_and(|next| ('0'..='7').contains(next))
                {
                    digits.push(characters.next().expect("peeked above"));
                }
                let value = u32::from_str_radix(&digits, 8).map_err(|error| error.to_string())?;
                result.push(
                    char::from_u32(value)
                        .ok_or_else(|| "octal escape is not a Unicode scalar".to_owned())?,
                );
            }
            other => return Err(format!("unsupported string escape `\\{other}`")),
        }
    }
    Ok(result)
}

fn decode_hex_escape(
    characters: &mut impl Iterator<Item = char>,
    width: usize,
) -> Result<char, String> {
    let digits = characters.take(width).collect::<String>();
    if digits.chars().count() != width || !digits.chars().all(|digit| digit.is_ascii_hexdigit()) {
        return Err(format!("invalid {width}-digit hexadecimal escape"));
    }
    let value = u32::from_str_radix(&digits, 16).map_err(|error| error.to_string())?;
    char::from_u32(value).ok_or_else(|| "escape is not a Unicode scalar value".to_owned())
}

fn normalize_metadata_descriptor(
    descriptor: &Form,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<MetadataEntry> {
    let entries = match &descriptor.kind {
        FormKind::Map(items) if items.len() % 2 == 0 => items
            .chunks_exact(2)
            .map(|pair| MetadataEntry {
                key: pair[0].clone(),
                value: pair[1].clone(),
            })
            .collect(),
        FormKind::Keyword(_) => vec![MetadataEntry {
            key: descriptor.clone(),
            value: Form::new(FormKind::Bool(true), descriptor.span),
        }],
        FormKind::Symbol(_) | FormKind::String(_) => vec![MetadataEntry {
            key: synthetic_keyword(":tag", descriptor.span),
            value: descriptor.clone(),
        }],
        FormKind::Vector(_) => vec![MetadataEntry {
            key: synthetic_keyword(":param-tags", descriptor.span),
            value: descriptor.clone(),
        }],
        _ => {
            diagnostics.push(Diagnostic::error(
                "OSR-R0005",
                "metadata descriptor must be a map, keyword, symbol, string, or vector",
                descriptor.span,
            ));
            Vec::new()
        }
    };

    for entry in &entries {
        if !metadata_datum_is_serializable(&entry.key)
            || !metadata_datum_is_serializable(&entry.value)
        {
            diagnostics.push(Diagnostic::error(
                "OSR-R0011",
                "metadata must contain only serializable phase-1 data",
                descriptor.span,
            ));
            return Vec::new();
        }
    }
    entries
}

fn merge_metadata_layers(layers: Vec<Vec<MetadataEntry>>) -> Vec<MetadataEntry> {
    let mut merged: Vec<MetadataEntry> = Vec::new();
    for layer in layers.into_iter().rev() {
        for entry in layer {
            if let Some(existing) = merged
                .iter_mut()
                .find(|existing| datum_eq(&existing.key, &entry.key))
            {
                *existing = entry;
            } else {
                merged.push(entry);
            }
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{MAX_DEPTH, parse_form, read, read_incremental};
    use crate::{
        lexer::lex,
        syntax::{
            FormKind, METADATA_TARGET_LIMITS, NodePath, NodePathSegment, SyntaxNodeKind, TokenKind,
        },
    };

    #[test]
    fn nom_form_parser_consumes_one_form_and_leaves_the_rest() {
        let source = "(alpha beta) gamma";
        let lexed = lex(source);
        assert!(lexed.diagnostics.is_empty());
        let significant = lexed
            .tokens
            .iter()
            .filter(|token| !token.kind.is_trivia())
            .collect::<Vec<_>>();

        let (rest, parsed) = parse_form(significant.as_slice(), 0, source.len())
            .expect("nom reader parser should parse one complete form");
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].text, "gamma");
        assert!(parsed.diagnostics.is_empty());
        assert!(matches!(
            parsed.form.kind,
            FormKind::List(items) if items.len() == 2
        ));
    }

    #[test]
    fn every_nom_production_preserves_the_following_form() {
        let cases = [
            ("(alpha) tail", "tail"),
            ("[alpha] tail", "tail"),
            ("{:alpha 1} tail", "tail"),
            ("#{alpha} tail", "tail"),
            ("'alpha tail", "tail"),
            ("`alpha tail", "tail"),
            ("~alpha tail", "tail"),
            ("~@alpha tail", "tail"),
            ("^:private alpha tail", "tail"),
            ("\"alpha\" tail", "tail"),
            ("alpha tail", "tail"),
            ("# tag", "tag"),
            (") tail", "tail"),
        ];

        for (source, expected) in cases {
            let lexed = lex(source);
            let significant = lexed
                .tokens
                .iter()
                .filter(|token| !token.kind.is_trivia())
                .collect::<Vec<_>>();

            let (rest, _) = parse_form(significant.as_slice(), 0, source.len())
                .unwrap_or_else(|error| panic!("failed to parse `{source}`: {error:?}"));
            assert_eq!(rest.len(), 1, "unexpected remainder for `{source}`");
            assert_eq!(
                rest.first().map(|token| token.text.as_str()),
                Some(expected),
                "production consumed tokens from the following form in `{source}`"
            );
        }
    }

    #[test]
    fn nom_form_parser_reports_eof_without_consuming_input() {
        let input: &[&crate::syntax::Token] = &[];
        let result = parse_form(input, 0, 0);
        assert!(matches!(result, Err(nom::Err::Error(_))));
    }

    #[test]
    fn reads_unicode_and_preserves_trivia() {
        let source = "; 数据\n(归一化 values lower upper)\n";
        let document = read(source);

        assert!(!document.has_errors());
        assert_eq!(
            document
                .tokens
                .iter()
                .map(|token| token.text.as_str())
                .collect::<String>(),
            source
        );
        assert!(
            document
                .tokens
                .iter()
                .any(|token| token.kind == TokenKind::Comment)
        );
    }

    #[test]
    fn node_identities_are_serialized_unique_and_queryable() {
        let document = read("(same same) same");
        let ids = document
            .nodes
            .iter()
            .map(|node| node.id)
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), document.nodes.len());
        assert_ne!(
            document.node_id(&NodePath::top_level(0)),
            document.node_id(&NodePath::top_level(1)),
            "repeated forms need distinct identities"
        );
        let top_level = document
            .node_identity(&NodePath::top_level(1))
            .expect("top-level identity");
        assert!(matches!(
            document.form_for_id(top_level.id).map(|form| &form.kind),
            Some(FormKind::Symbol(name)) if name.spelling == "same"
        ));
        let nested = NodePath::top_level(0).child(NodePathSegment::CollectionItem { index: 1 });
        assert!(matches!(
            document.form_at_path(&nested).map(|form| &form.kind),
            Some(FormKind::Symbol(name)) if name.spelling == "same"
        ));
        assert!(document.node_id(&nested).is_some());
        let encoded = serde_json::to_value(&document).expect("document should serialize");
        assert_eq!(
            encoded["nodes"].as_array().map(Vec::len),
            Some(document.nodes.len())
        );
    }

    #[test]
    fn incremental_read_preserves_ids_across_preceding_trivia_and_form_edits() {
        let original = read("(def first 1)\n(def second 2)\n");
        let trivia = read_incremental(
            "; inserted comment\n\n(def first 1)\n(def second 2)\n",
            &original,
        );
        assert_eq!(
            original.node_id(&NodePath::top_level(0)),
            trivia.node_id(&NodePath::top_level(0))
        );
        assert_eq!(
            original.node_id(&NodePath::top_level(1)),
            trivia.node_id(&NodePath::top_level(1))
        );

        let changed = read_incremental("(def first 100)\n(def second 2)\n", &original);
        assert_ne!(
            original.node_id(&NodePath::top_level(0)),
            changed.node_id(&NodePath::top_level(0)),
            "the edited enclosing form needs a new identity"
        );
        assert_eq!(
            original.node_id(&NodePath::top_level(1)),
            changed.node_id(&NodePath::top_level(1)),
            "an unchanged following form retains its identity"
        );
    }

    #[test]
    fn incremental_read_tracks_unchanged_forms_after_an_insertion() {
        let original = read("(def retained 2)\n");
        let inserted = read_incremental("(def added 1)\n(def retained 2)\n", &original);
        assert_eq!(
            original.node_id(&NodePath::top_level(0)),
            inserted.node_id(&NodePath::top_level(1))
        );
    }

    #[test]
    fn duplicate_and_error_nodes_keep_distinct_stable_identities() {
        let duplicate = read("same same");
        let duplicate_after_trivia = read_incremental("; note\nsame same", &duplicate);
        assert_eq!(
            duplicate.node_id(&NodePath::top_level(0)),
            duplicate_after_trivia.node_id(&NodePath::top_level(0))
        );
        assert_eq!(
            duplicate.node_id(&NodePath::top_level(1)),
            duplicate_after_trivia.node_id(&NodePath::top_level(1))
        );
        assert_ne!(
            duplicate_after_trivia.node_id(&NodePath::top_level(0)),
            duplicate_after_trivia.node_id(&NodePath::top_level(1))
        );

        let anchored_duplicates = read("anchor same same");
        let inserted_duplicate = read_incremental("same anchor same same", &anchored_duplicates);
        assert_eq!(
            anchored_duplicates.node_id(&NodePath::top_level(1)),
            inserted_duplicate.node_id(&NodePath::top_level(2)),
            "an inserted duplicate must not steal the retained node identity"
        );
        assert_eq!(
            anchored_duplicates.node_id(&NodePath::top_level(2)),
            inserted_duplicate.node_id(&NodePath::top_level(3))
        );
        assert_ne!(
            inserted_duplicate.node_id(&NodePath::top_level(0)),
            inserted_duplicate.node_id(&NodePath::top_level(2))
        );

        let broken = read("' ) tail");
        let broken_after_trivia = read_incremental("; note\n' ) tail", &broken);
        let error_ids = broken
            .nodes
            .iter()
            .filter(|node| node.kind == SyntaxNodeKind::Error)
            .map(|node| node.id)
            .collect::<Vec<_>>();
        let shifted_error_ids = broken_after_trivia
            .nodes
            .iter()
            .filter(|node| node.kind == SyntaxNodeKind::Error)
            .map(|node| node.id)
            .collect::<Vec<_>>();
        assert!(!error_ids.is_empty());
        assert_eq!(error_ids, shifted_error_ids);
    }

    #[test]
    fn preserves_original_unicode_spelling_while_normalizing_names() {
        let source = "(e\u{301})";
        let document = read(source);
        assert!(!document.has_errors(), "{:?}", document.diagnostics);

        let FormKind::List(items) = &document.forms[0].kind else {
            panic!("expected a list form");
        };
        let FormKind::Symbol(name) = &items[0].kind else {
            panic!("expected a symbol form");
        };
        assert_eq!(name.spelling, "e\u{301}");
        assert_eq!(name.canonical, "é");
    }

    #[test]
    fn metadata_descriptors_are_normalized_and_leftmost_wins() {
        let document = read("^:static ^:awesome ^{:static false :bar :baz} sym");
        assert!(!document.has_errors(), "{:?}", document.diagnostics);
        let metadata = &document.forms[0].metadata;

        assert_eq!(metadata.len(), 3);
        let static_entry = metadata
            .iter()
            .find(|entry| {
                matches!(&entry.key.kind, FormKind::Keyword(name) if name.canonical == ":static")
            })
            .expect("static metadata should exist");
        assert!(matches!(static_entry.value.kind, FormKind::Bool(true)));
    }

    #[test]
    fn supports_all_five_metadata_descriptors() {
        let document = read("^{:a 1} ^:flag ^Tag ^\"doc\" ^[A B _] target");
        assert!(!document.has_errors(), "{:?}", document.diagnostics);
        assert_eq!(document.forms[0].metadata.len(), 4);
    }

    #[test]
    fn metadata_resource_boundaries_are_accepted_and_overflow_recovers() {
        let depth_boundary = metadata_depth_source(METADATA_TARGET_LIMITS.max_depth);
        let depth_overflow = metadata_depth_source(METADATA_TARGET_LIMITS.max_depth + 1);
        let entries_boundary = metadata_entries_source(METADATA_TARGET_LIMITS.max_entries);
        let entries_overflow = metadata_entries_source(METADATA_TARGET_LIMITS.max_entries + 1);
        let nodes_boundary = metadata_nodes_source(METADATA_TARGET_LIMITS.max_nodes);
        let nodes_overflow = metadata_nodes_source(METADATA_TARGET_LIMITS.max_nodes + 1);
        let bytes_boundary = metadata_bytes_source(METADATA_TARGET_LIMITS.max_normalized_bytes);
        let bytes_overflow = metadata_bytes_source(METADATA_TARGET_LIMITS.max_normalized_bytes + 1);

        for (source, label) in [
            (depth_boundary, "nesting depth"),
            (entries_boundary, "entry count"),
            (nodes_boundary, "node count"),
            (bytes_boundary, "normalized byte size"),
        ] {
            let document = read(&source);
            assert!(
                !document.has_errors(),
                "metadata {label} boundary should be accepted: {:?}",
                document.diagnostics
            );
            assert_eq!(document.forms.len(), 2);
            assert!(!document.forms[0].metadata.is_empty());
            assert_tail_is_read(&document);
        }

        for (source, label) in [
            (depth_overflow, "nesting depth"),
            (entries_overflow, "entry count"),
            (nodes_overflow, "node count"),
            (bytes_overflow, "normalized byte size"),
        ] {
            let document = read(&source);
            let diagnostic = document
                .diagnostics
                .iter()
                .find(|diagnostic| diagnostic.code == "OSR-R0014")
                .unwrap_or_else(|| {
                    panic!(
                        "metadata {label} overflow needs OSR-R0014: {:?}",
                        document.diagnostics
                    )
                });
            assert!(diagnostic.message.contains(label), "{diagnostic:?}");
            assert_eq!(document.forms.len(), 2);
            assert!(document.forms[0].metadata.is_empty());
            assert_tail_is_read(&document);
        }
    }

    #[test]
    fn metadata_limits_do_not_apply_to_ordinary_business_data() {
        let vector = std::iter::repeat_n("value", METADATA_TARGET_LIMITS.max_nodes + 1)
            .collect::<Vec<_>>()
            .join(" ");
        let large_string = "x".repeat(METADATA_TARGET_LIMITS.max_normalized_bytes + 1);
        let source = format!("[{vector}] \"{large_string}\" tail");
        let document = read(&source);

        assert!(!document.has_errors(), "{:?}", document.diagnostics);
        assert_eq!(document.forms.len(), 3);
        assert!(
            document
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code != "OSR-R0014")
        );
        assert!(matches!(
            &document.forms[2].kind,
            FormKind::Symbol(name) if name.canonical == "tail"
        ));
    }

    fn metadata_depth_source(maximum_depth: usize) -> String {
        let collection_count = maximum_depth.saturating_sub(1);
        format!(
            "^{{:x {}value{}}} target tail",
            "[".repeat(collection_count),
            "]".repeat(collection_count)
        )
    }

    fn metadata_entries_source(entries: usize) -> String {
        let values = (0..entries)
            .map(|index| format!(":k{index} {index}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!("^{{{values}}} target tail")
    }

    fn metadata_nodes_source(nodes: usize) -> String {
        let leaves = std::iter::repeat_n("x", nodes.saturating_sub(2))
            .collect::<Vec<_>>()
            .join(" ");
        format!("^{{:x [{leaves}]}} target tail")
    }

    fn metadata_bytes_source(bytes: usize) -> String {
        // Normalized `{:x "..."}` contributes seven bytes beyond the UTF-8
        // payload: two braces, one separator, `:x`, and two quotes.
        let payload = "x".repeat(bytes.saturating_sub(7));
        format!("^{{:x \"{payload}\"}} target tail")
    }

    fn assert_tail_is_read(document: &crate::syntax::Document) {
        assert!(matches!(
            &document.forms[1].kind,
            FormKind::Symbol(name) if name.canonical == "tail"
        ));
    }

    #[test]
    fn metadata_does_not_attach_to_scalars() {
        let document = read("^:private 42");
        assert!(
            document
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-R0009")
        );
        assert!(document.forms[0].metadata.is_empty());
    }

    #[test]
    fn recovers_at_an_outer_closing_delimiter() {
        let document = read("([)] tail");
        let codes = document
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"OSR-R0003"));
        assert!(codes.contains(&"OSR-R0001"));
        assert_eq!(document.forms.len(), 3);
        assert!(matches!(
            &document.forms[2].kind,
            FormKind::Symbol(name) if name.canonical == "tail"
        ));
    }

    #[test]
    fn recovers_missing_prefix_operand_without_swallowing_following_forms() {
        let document = read("' ) tail");
        let codes = document
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"OSR-R0004"));
        assert!(codes.contains(&"OSR-R0001"));
        assert!(matches!(
            document.forms.last().map(|form| &form.kind),
            Some(FormKind::Symbol(name)) if name.canonical == "tail"
        ));
    }

    #[test]
    fn recovers_an_unclosed_string_without_swallowing_the_next_line() {
        let document = read("\"unterminated\n(tail)");

        assert_eq!(document.forms.len(), 2);
        assert!(matches!(document.forms[0].kind, FormKind::Error(_)));
        assert!(matches!(
            &document.forms[1].kind,
            FormKind::List(items)
                if matches!(
                    items.as_slice(),
                    [crate::syntax::Form {
                        kind: FormKind::Symbol(name),
                        ..
                    }] if name.canonical == "tail"
                )
        ));
    }

    #[test]
    fn caps_nesting_with_a_recoverable_nom_form() {
        let nesting = MAX_DEPTH + 8;
        let source = format!("{}value{}", "(".repeat(nesting), ")".repeat(nesting));
        let document = read(&source);
        assert!(
            document
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-R0010"),
            "deep input should report the reader depth limit"
        );
        assert!(!document.forms.is_empty());
    }

    #[test]
    fn diagnoses_collection_invariants() {
        let document = read("{:a 1 :a 2 :odd} #{1 1}");
        let codes = document
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"OSR-R0006"));
        assert!(codes.contains(&"OSR-R0007"));
        assert!(codes.contains(&"OSR-R0008"));
    }

    #[test]
    fn decodes_common_string_escapes() {
        let document = read(r#""line\n\u4e2d\x41""#);
        assert!(!document.has_errors(), "{:?}", document.diagnostics);
        assert!(matches!(
            &document.forms[0].kind,
            FormKind::String(value) if value == "line\n中A"
        ));
    }

    #[test]
    fn decodes_wide_octal_and_line_continuation_escapes() {
        let document = read(
            r#""\141\U0001f600first\
second""#,
        );
        assert!(!document.has_errors(), "{:?}", document.diagnostics);
        assert!(matches!(
            &document.forms[0].kind,
            FormKind::String(value) if value == "a😀firstsecond"
        ));
    }
}
