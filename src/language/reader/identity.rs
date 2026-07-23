use super::*;

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

pub(super) fn build_node_identities(
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
