
#[derive(Clone, Debug, Default)]
pub(super) struct WorkspaceSymbolIndex {
    pub(super) source_uris: BTreeSet<String>,
    pub(super) sources: BTreeMap<String, String>,
    pub(super) definitions: BTreeMap<String, Location>,
    pub(super) ambiguous_definitions: BTreeSet<String>,
    pub(super) references: BTreeMap<String, Vec<Location>>,
    pub(super) rename_occurrences: BTreeMap<String, Vec<RenameOccurrence>>,
    pub(super) binding_kinds: BTreeMap<String, BindingKind>,
    pub(super) provider_names: BTreeMap<(String, String), String>,
    pub(super) ambiguous_provider_names: BTreeSet<(String, String)>,
    pub(super) pending_import_members: Vec<PendingImportMember>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RenameOccurrence {
    pub(super) uri: String,
    pub(super) span: Span,
    pub(super) spelling: String,
    pub(super) declaration: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PendingImportMember {
    pub(super) uri: String,
    pub(super) provider: String,
    pub(super) spelling: String,
    pub(super) span: Span,
}

pub(super) const fn span_contains(span: Span, offset: usize) -> bool {
    span.start <= offset && offset <= span.end
}

pub(super) fn occurrence_at(symbol: &SemanticSymbol, offset: usize) -> Option<Span> {
    symbol
        .occurrences
        .iter()
        .copied()
        .filter(|span| span.start <= offset && offset <= span.end)
        .min_by_key(|span| span.end.saturating_sub(span.start))
}

pub(super) fn rename_target<'index>(
    index: &'index WorkspaceSymbolIndex,
    uri: &str,
    offset: usize,
) -> Option<(&'index str, &'index RenameOccurrence)> {
    index
        .rename_occurrences
        .iter()
        .flat_map(|(binding_id, occurrences)| {
            occurrences
                .iter()
                .map(move |occurrence| (binding_id.as_str(), occurrence))
        })
        .filter(|(_, occurrence)| {
            occurrence.uri == uri
                && occurrence.span.start <= offset
                && offset <= occurrence.span.end
        })
        .min_by_key(|(_, occurrence)| occurrence.span.end.saturating_sub(occurrence.span.start))
}

pub(super) fn normalize_rename_name(new_name: &str) -> Result<String, LspStateError> {
    let normalized = new_name.nfc().collect::<String>();
    let parsed = reader::read(&normalized);
    let valid = parsed.diagnostics.is_empty()
        && parsed.forms.len() == 1
        && parsed.forms[0].metadata.is_empty()
        && parsed.forms[0].span == Span::new(0, normalized.len())
        && parsed.forms[0].datum_span == parsed.forms[0].span
        && matches!(parsed.forms[0].kind, FormKind::Symbol(_))
        && !normalized.contains(['/', '.']);
    if !valid {
        return Err(LspStateError::new(
            INVALID_PARAMS,
            "newName must be one non-qualified Osiris symbol",
        ));
    }
    Ok(normalized)
}

pub(super) fn is_reserved_rename_name(name: &str) -> bool {
    // Exhaustive parser heads from `ast::Lowerer::{lower_item,lower_list_expr,
    // lower_try_expr}`, plus the bootstrap macros declared in
    // `macro_expand::BOOTSTRAP_PRELUDE`. Keep this list aligned when either
    // grammar surface changes; unlike ordinary prelude functions these names
    // capture list-head syntax before runtime binding resolution.
    matches!(
        name,
        "module"
            | "import"
            | "import-for-syntax"
            | "py/import"
            | "py/decorate"
            | "export"
            | "alias"
            | "def"
            | "defn"
            | "defstruct"
            | "defstatic-schema"
            | "static-record"
            | "extern"
            | "defmacro"
            | "defn-for-syntax"
            | "fn"
            | "let"
            | "if"
            | "do"
            | "try"
            | "raise"
            | "catch"
            | "finally"
            | "->"
            | "->>"
            | "cond->"
            | "cond->>"
            | "as->"
            | "doto"
            | "defn-"
            | "and"
            | "or"
            | "when"
            | "if-not"
            | "when-not"
            | "cond"
            | "if-let"
            | "when-let"
            | "if-some"
            | "when-some"
            | "nil?"
            | "some?"
            | "some->"
            | "some->>"
            | "condp"
            | "case"
            | "for"
            | "doseq"
            | "when-first"
            | "loop"
            | "recur"
            | "dotimes"
            | "while"
            | "letfn"
            | "trampoline"
            | "lazy-seq"
            | "lazy-cat"
            | "delay"
            | "force"
            | "realized?"
            | "deref"
            | "future"
            | "future-call"
            | "future-done?"
            | "future-cancelled?"
            | "future-cancel"
            | "pmap"
            | "pvalues"
            | "pcalls"
            | "promise"
            | "deliver"
            | "lock"
            | "locking"
            | "time"
            | "binding"
            | "with-open"
            | "throw"
            | "assert"
            | "comment"
    )
}

pub(super) fn reject_rename_collision(
    index: &WorkspaceSymbolIndex,
    binding_id: &str,
    selected_spelling: &str,
    new_name: &str,
) -> Result<(), LspStateError> {
    if selected_spelling == new_name {
        return Ok(());
    }
    let declaration_uris = index
        .rename_occurrences
        .get(binding_id)
        .into_iter()
        .flatten()
        .filter(|occurrence| {
            occurrence.declaration
                && occurrence.spelling.nfc().collect::<String>() == selected_spelling
        })
        .map(|occurrence| occurrence.uri.as_str())
        .collect::<BTreeSet<_>>();
    if declaration_uris.is_empty() {
        return Err(LspStateError::new(
            INVALID_PARAMS,
            "selected spelling has no editable source declaration",
        ));
    }
    let collision = index
        .rename_occurrences
        .iter()
        .any(|(candidate_id, occurrences)| {
            occurrences.iter().any(|occurrence| {
                occurrence.declaration
                    && declaration_uris.contains(occurrence.uri.as_str())
                    && occurrence.spelling.nfc().collect::<String>() == new_name
                    && (candidate_id != binding_id
                        || occurrence.spelling.nfc().collect::<String>() != selected_spelling)
            })
        });
    if collision {
        return Err(LspStateError::new(
            INVALID_PARAMS,
            format!("newName `{new_name}` collides with an existing declaration"),
        ));
    }
    Ok(())
}
