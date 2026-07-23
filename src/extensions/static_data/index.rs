use super::datum::*;
use super::record::*;
use super::*;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IndexedRecord {
    pub occurrence: RecordOccurrenceId,
    pub record: ValidatedRecord,
    #[serde(default)]
    pub dependency_path: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MergedIndexClaim {
    pub index_id: String,
    pub normalized_key: String,
    pub raw_spelling: Option<String>,
    pub projection_field: String,
    pub projection_role: String,
    pub occurrence: RecordOccurrenceId,
    pub dependency_path: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MergedIndexes {
    pub claims: Vec<MergedIndexClaim>,
    pub effective_record_index_hash: String,
}

/// Merge all public index claims.  Exact occurrence+claim repeats are the only
/// idempotent case, which is what makes diamond dependency traversal harmless.
pub fn merge_unique_indexes<I>(records: I) -> Result<MergedIndexes, Vec<Diagnostic>>
where
    I: IntoIterator<Item = IndexedRecord>,
{
    let mut records = records
        .into_iter()
        .filter(|record| record.record.public)
        .collect::<Vec<_>>();
    records.sort_by(|left, right| {
        left.occurrence
            .cmp(&right.occurrence)
            .then_with(|| left.dependency_path.cmp(&right.dependency_path))
    });
    let mut by_key = BTreeMap::<(String, String), Vec<MergedIndexClaim>>::new();
    for indexed in records {
        for claim in &indexed.record.index_claims {
            let merged = MergedIndexClaim {
                index_id: claim.index_id.clone(),
                normalized_key: claim.normalized_key.clone(),
                raw_spelling: claim.raw_spelling.clone(),
                projection_field: claim.projection_field.clone(),
                projection_role: claim.projection_role.clone(),
                occurrence: indexed.occurrence.clone(),
                dependency_path: indexed.dependency_path.clone(),
            };
            by_key
                .entry((merged.index_id.clone(), merged.normalized_key.clone()))
                .or_default()
                .push(merged);
        }
    }
    let mut claims = Vec::new();
    let mut diagnostics = Vec::new();
    for ((index_id, normalized_key), mut candidates) in by_key {
        candidates.sort_by(merged_claim_cmp);
        let mut unique = Vec::new();
        for candidate in candidates {
            if unique.iter().any(|existing: &MergedIndexClaim| {
                existing.occurrence == candidate.occurrence
                    && existing.projection_field == candidate.projection_field
                    && existing.projection_role == candidate.projection_role
                    && existing.normalized_key == candidate.normalized_key
            }) {
                continue;
            }
            if let Some(existing) = unique.first() {
                diagnostics.push(Diagnostic::error(
                    RECORD_INDEX_CONFLICT,
                    format!(
                        "unique index `{index_id}` key `{normalized_key}` is claimed by `{}` ({}) and `{}` ({})",
                        occurrence_display(&existing.occurrence),
                        existing.projection_role,
                        occurrence_display(&candidate.occurrence),
                        candidate.projection_role,
                    ),
                    candidate_span(&candidate),
                ));
            } else {
                unique.push(candidate);
            }
        }
        if diagnostics.is_empty() {
            claims.extend(unique);
        }
    }
    if !diagnostics.is_empty() {
        diagnostics.sort_by(|left, right| {
            left.message
                .cmp(&right.message)
                .then_with(|| left.span.start.cmp(&right.span.start))
        });
        return Err(diagnostics);
    }
    claims.sort_by(merged_claim_cmp);
    let payload = Json::Array(claims.iter().map(merged_claim_json).collect());
    Ok(MergedIndexes {
        effective_record_index_hash: sha256(&payload.bytes()),
        claims,
    })
}

pub(in crate::records) fn merged_claim_cmp(
    left: &MergedIndexClaim,
    right: &MergedIndexClaim,
) -> Ordering {
    left.index_id
        .cmp(&right.index_id)
        .then_with(|| left.normalized_key.cmp(&right.normalized_key))
        .then_with(|| left.occurrence.cmp(&right.occurrence))
        .then_with(|| left.projection_field.cmp(&right.projection_field))
        .then_with(|| left.projection_role.cmp(&right.projection_role))
}

pub(in crate::records) fn occurrence_display(occurrence: &RecordOccurrenceId) -> String {
    format!(
        "{}:{}:{}:{}",
        occurrence.distribution,
        occurrence.version,
        occurrence.interface_member_id,
        occurrence.stable_record_id
    )
}

pub(in crate::records) fn candidate_span(candidate: &MergedIndexClaim) -> Span {
    // Dependency records carry their source span in `ValidatedRecord`; this
    // compact merged claim intentionally remains transport-friendly.
    let _ = candidate;
    Span::default()
}

pub(in crate::records) fn merged_claim_json(claim: &MergedIndexClaim) -> Json {
    Json::Object(vec![
        ("index-id".to_owned(), Json::String(claim.index_id.clone())),
        (
            "normalized-key".to_owned(),
            Json::String(claim.normalized_key.clone()),
        ),
        (
            "raw-spelling".to_owned(),
            claim
                .raw_spelling
                .as_ref()
                .map_or(Json::Null, |value| Json::String(value.clone())),
        ),
        (
            "projection-field".to_owned(),
            Json::String(claim.projection_field.clone()),
        ),
        (
            "projection-role".to_owned(),
            Json::String(claim.projection_role.clone()),
        ),
        ("occurrence".to_owned(), claim.occurrence.json()),
        (
            "dependency-path".to_owned(),
            Json::Array(
                claim
                    .dependency_path
                    .iter()
                    .map(|value| Json::String(value.clone()))
                    .collect(),
            ),
        ),
    ])
}
