use super::datum::*;
use super::record::*;

pub const RECORD_SIDECAR_FORMAT_VERSION: u64 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SidecarRecord {
    pub occurrence: RecordOccurrenceId,
    pub record: ValidatedRecord,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecordSidecar {
    pub format_version: u64,
    pub interface_semantic_hashes: Vec<String>,
    pub records: Vec<SidecarRecord>,
    pub record_set_hash: String,
}

impl RecordSidecar {
    #[must_use]
    pub fn records_payload_bytes(&self) -> Vec<u8> {
        Json::Array(self.records.iter().map(sidecar_record_json).collect()).bytes()
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        sidecar_json(self).bytes()
    }

    #[must_use]
    pub fn records_hash(&self) -> String {
        sha256(&self.canonical_bytes())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedSidecar {
    pub sidecar: RecordSidecar,
    pub bytes: Vec<u8>,
    pub record_set_hash: String,
    pub records_hash: String,
}

/// Build the distribution-owned sidecar from public records only.  The caller
/// should pass records reconstructed from `.osri`; this function never imports
/// or executes an extension module.
pub fn encode_sidecar(
    interface_semantic_hashes: impl IntoIterator<Item = String>,
    records: impl IntoIterator<Item = IndexedRecord>,
) -> Result<EncodedSidecar, RecordError> {
    let mut records = records.into_iter().collect::<Vec<_>>();
    if records.iter().any(|record| !record.record.public) {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "private static records cannot be emitted in a runtime sidecar",
        ));
    }
    records.sort_by(|left, right| left.occurrence.cmp(&right.occurrence));
    let mut sidecar_records = Vec::new();
    let mut seen = BTreeSet::new();
    for indexed in records {
        if indexed.occurrence.stable_record_id != indexed.record.stable_record_id
            || indexed.occurrence.record_body_hash != indexed.record.record_body_hash
        {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "record occurrence does not match its validated record",
            ));
        }
        if !seen.insert(indexed.occurrence.clone()) {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "duplicate record occurrence in sidecar",
            ));
        }
        sidecar_records.push(SidecarRecord {
            occurrence: indexed.occurrence,
            record: indexed.record,
        });
    }
    let mut interface_semantic_hashes = interface_semantic_hashes.into_iter().collect::<Vec<_>>();
    interface_semantic_hashes.sort();
    interface_semantic_hashes.dedup();
    let records_payload = Json::Array(sidecar_records.iter().map(sidecar_record_json).collect());
    let record_set_hash = sha256(&records_payload.bytes());
    let sidecar = RecordSidecar {
        format_version: RECORD_SIDECAR_FORMAT_VERSION,
        interface_semantic_hashes,
        records: sidecar_records,
        record_set_hash,
    };
    let bytes = sidecar.canonical_bytes();
    let records_hash = sha256(&bytes);
    Ok(EncodedSidecar {
        record_set_hash: sidecar.record_set_hash.clone(),
        records_hash,
        bytes,
        sidecar,
    })
}

/// Decode and fully verify a sidecar.  `expected_records_hash` is the marker's
/// hash of the complete sidecar bytes; passing it is strongly recommended and
/// makes tampering fail before the decoded data is exposed.
pub fn decode_sidecar(
    bytes: &[u8],
    expected_records_hash: Option<&str>,
) -> Result<RecordSidecar, RecordError> {
    if let Some(expected) = expected_records_hash {
        let actual = sha256(bytes);
        if actual != expected {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                format!("sidecar byte hash mismatch: expected `{expected}`, got `{actual}`"),
            ));
        }
    }
    let root = serde_json::from_slice::<Json>(bytes).map_err(|error| {
        RecordError::new(RECORD_SIDECAR, format!("invalid sidecar JSON: {error}"))
    })?;
    let sidecar = sidecar_from_json(&root)?;
    if sidecar.canonical_bytes() != bytes {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "sidecar bytes are not canonical RFC 8785 JSON",
        ));
    }
    let actual_set_hash = sha256(&sidecar.records_payload_bytes());
    if actual_set_hash != sidecar.record_set_hash {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            format!(
                "record-set-hash mismatch: expected `{}`, got `{actual_set_hash}`",
                sidecar.record_set_hash
            ),
        ));
    }
    Ok(sidecar)
}

/// Compare a sidecar generated from a fresh interface with an existing byte
/// artifact.  This is the fail-closed check used by build backends.
pub fn verify_sidecar_against_records(
    bytes: &[u8],
    expected_records_hash: Option<&str>,
    expected_interface_hashes: &[String],
    expected_records: &[IndexedRecord],
) -> Result<(), RecordError> {
    let decoded = decode_sidecar(bytes, expected_records_hash)?;
    let encoded = encode_sidecar(
        expected_interface_hashes.iter().cloned(),
        expected_records.iter().cloned(),
    )?;
    if decoded != encoded.sidecar || bytes != encoded.bytes {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "sidecar records differ from the canonical interface records",
        ));
    }
    Ok(())
}

pub(in crate::records) fn sidecar_json(sidecar: &RecordSidecar) -> Json {
    Json::Object(vec![
        (
            "format-version".to_owned(),
            Json::Number(sidecar.format_version.to_string()),
        ),
        (
            "interface-semantic-hashes".to_owned(),
            Json::Array(
                sidecar
                    .interface_semantic_hashes
                    .iter()
                    .map(|value| Json::String(value.clone()))
                    .collect(),
            ),
        ),
        (
            "record-identities".to_owned(),
            Json::Array(
                sidecar
                    .records
                    .iter()
                    .map(|record| record.occurrence.json())
                    .collect(),
            ),
        ),
        (
            "record-set-hash".to_owned(),
            Json::String(sidecar.record_set_hash.clone()),
        ),
        (
            "records".to_owned(),
            Json::Array(sidecar.records.iter().map(sidecar_record_json).collect()),
        ),
    ])
}

pub(in crate::records) fn sidecar_record_json(record: &SidecarRecord) -> Json {
    Json::Object(vec![
        ("occurrence".to_owned(), record.occurrence.json()),
        ("record".to_owned(), record_json(&record.record)),
    ])
}

pub(in crate::records) fn record_json(record: &ValidatedRecord) -> Json {
    let mut fields = vec![
        ("schema".to_owned(), schema_identity_json(&record.schema)),
        (
            "owner-binding-id".to_owned(),
            Json::String(record.owner_binding_id.clone()),
        ),
        (
            "owner-name".to_owned(),
            Json::String(record.owner_name.clone()),
        ),
        ("module".to_owned(), Json::String(record.module.clone())),
        ("public".to_owned(), Json::Bool(record.public)),
        (
            "stable-record-id".to_owned(),
            Json::String(record.stable_record_id.clone()),
        ),
        (
            "record-body-hash".to_owned(),
            Json::String(record.record_body_hash.clone()),
        ),
        (
            "fields".to_owned(),
            Json::Array(
                record
                    .fields
                    .iter()
                    .map(|(name, value)| {
                        Json::Array(vec![Json::String(name.clone()), value.to_json()])
                    })
                    .collect(),
            ),
        ),
        (
            "index-claims".to_owned(),
            Json::Array(record.index_claims.iter().map(index_claim_json).collect()),
        ),
        ("origin".to_owned(), origin_json(&record.origin)),
    ];
    // `record_json` is a sidecar payload, so all members are mandatory.  The
    // vector is kept explicit to make additions visible in code review.
    fields.shrink_to_fit();
    Json::Object(fields)
}

pub(in crate::records) fn origin_json(origin: &RecordOrigin) -> Json {
    let mut fields = vec![
        ("module".to_owned(), Json::String(origin.module.clone())),
        (
            "span".to_owned(),
            Json::Array(vec![
                Json::Number(origin.span.start.to_string()),
                Json::Number(origin.span.end.to_string()),
            ]),
        ),
    ];
    if let Some(value) = &origin.macro_origin {
        fields.push(("macro-origin".to_owned(), Json::String(value.clone())));
    }
    Json::Object(fields)
}
