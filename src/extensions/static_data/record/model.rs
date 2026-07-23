use super::datum::*;
use super::schema::*;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct SchemaIdentity {
    pub binding_id: String,
    pub schema_id: String,
    pub version: u64,
    pub body_hash: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct RecordOccurrenceId {
    pub distribution: String,
    pub version: String,
    pub interface_member_id: String,
    pub semantic_interface_hash: String,
    pub stable_record_id: String,
    pub record_body_hash: String,
}

impl RecordOccurrenceId {
    pub(in crate::records) fn json(&self) -> Json {
        Json::Object(vec![
            (
                "distribution".to_owned(),
                Json::String(self.distribution.clone()),
            ),
            ("version".to_owned(), Json::String(self.version.clone())),
            (
                "interface-member-id".to_owned(),
                Json::String(self.interface_member_id.clone()),
            ),
            (
                "semantic-interface-hash".to_owned(),
                Json::String(self.semantic_interface_hash.clone()),
            ),
            (
                "stable-record-id".to_owned(),
                Json::String(self.stable_record_id.clone()),
            ),
            (
                "record-body-hash".to_owned(),
                Json::String(self.record_body_hash.clone()),
            ),
        ])
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecordOrigin {
    pub module: String,
    pub span: Span,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub macro_origin: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IndexClaim {
    pub index_id: String,
    pub projection_field: String,
    pub projection_role: String,
    pub key: StaticDatum,
    pub normalized_key: String,
    pub raw_spelling: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ValidatedRecord {
    pub schema: SchemaIdentity,
    pub owner_binding_id: String,
    pub owner_name: String,
    pub module: String,
    pub public: bool,
    pub stable_record_id: String,
    pub record_body_hash: String,
    pub fields: Vec<(String, StaticDatum)>,
    pub index_claims: Vec<IndexClaim>,
    pub origin: RecordOrigin,
}

impl ValidatedRecord {
    #[must_use]
    pub fn occurrence(
        &self,
        distribution: impl Into<String>,
        version: impl Into<String>,
        interface_member_id: impl Into<String>,
        semantic_interface_hash: impl Into<String>,
    ) -> RecordOccurrenceId {
        RecordOccurrenceId {
            distribution: distribution.into(),
            version: version.into(),
            interface_member_id: interface_member_id.into(),
            semantic_interface_hash: semantic_interface_hash.into(),
            stable_record_id: self.stable_record_id.clone(),
            record_body_hash: self.record_body_hash.clone(),
        }
    }

    #[must_use]
    pub fn as_public(&self) -> Option<&Self> {
        self.public.then_some(self)
    }

    #[must_use]
    pub fn canonical_body_bytes(&self) -> Vec<u8> {
        self.body_json().bytes()
    }

    /// Verify identities and canonical ordering for a record reconstructed
    /// from an interface.  Provider occurrence data is intentionally absent:
    /// it is assembled only after the semantic interface hash exists.
    pub fn verify_integrity(&self) -> Result<(), RecordError> {
        if self.owner_binding_id.is_empty()
            || self.owner_name.is_empty()
            || self.module.is_empty()
            || self.origin.module != self.module
        {
            return Err(RecordError::new(
                RECORD_RECORD_SHAPE,
                "record owner, module, and origin must be present and consistent",
            ));
        }
        if self.schema.binding_id.is_empty()
            || !is_namespaced(&self.schema.schema_id)
            || self.schema.version == 0
        {
            return Err(RecordError::new(
                RECORD_RECORD_SHAPE,
                "record contains an invalid schema identity",
            ));
        }

        let expected_stable_id = sha256(
            &Json::Array(vec![
                Json::String(self.schema.binding_id.clone()),
                Json::String(self.schema.schema_id.clone()),
                Json::Number(self.schema.version.to_string()),
                Json::String(self.schema.body_hash.clone()),
                Json::String(self.owner_binding_id.clone()),
            ])
            .bytes(),
        );
        if self.stable_record_id != expected_stable_id {
            return Err(RecordError::new(
                RECORD_RECORD_SHAPE,
                "stable record id does not match schema and owner identities",
            ));
        }

        let mut previous_field = None;
        for (name, value) in &self.fields {
            if name.is_empty() || previous_field.is_some_and(|previous| previous >= name.as_str()) {
                return Err(RecordError::new(
                    RECORD_RECORD_SHAPE,
                    "record fields must have unique names in canonical order",
                ));
            }
            if value.clone().canonicalize()? != *value {
                return Err(RecordError::new(
                    RECORD_RECORD_SHAPE,
                    format!("record field `{name}` is not canonical"),
                ));
            }
            previous_field = Some(name.as_str());
        }

        let mut sorted_claims = self.index_claims.clone();
        sorted_claims.sort_by(index_claim_cmp);
        if sorted_claims != self.index_claims {
            return Err(RecordError::new(
                RECORD_RECORD_INDEX,
                "record index claims are not in canonical order",
            ));
        }
        let mut claim_keys = BTreeSet::new();
        for claim in &self.index_claims {
            if claim.index_id.is_empty()
                || claim.projection_field.is_empty()
                || claim.projection_role.is_empty()
                || !claim_keys.insert((claim.index_id.clone(), claim.normalized_key.clone()))
            {
                return Err(RecordError::new(
                    RECORD_RECORD_INDEX,
                    "record contains an empty or duplicate index claim",
                ));
            }
            if claim.key.clone().canonicalize()? != claim.key {
                return Err(RecordError::new(
                    RECORD_RECORD_INDEX,
                    "record index key is not canonical",
                ));
            }
            let (normalized_key, raw_spelling) = normalize_index_key(&claim.key)?;
            if normalized_key != claim.normalized_key || raw_spelling != claim.raw_spelling {
                return Err(RecordError::new(
                    RECORD_RECORD_INDEX,
                    "record index normalization does not match its key",
                ));
            }
        }

        let expected_body_hash = sha256(&self.canonical_body_bytes());
        if self.record_body_hash != expected_body_hash {
            return Err(RecordError::new(
                RECORD_RECORD_SHAPE,
                "record body hash does not match record payload",
            ));
        }
        Ok(())
    }

    pub(in crate::records) fn body_json(&self) -> Json {
        Json::Object(vec![
            (
                "stable-record-id".to_owned(),
                Json::String(self.stable_record_id.clone()),
            ),
            ("schema".to_owned(), schema_identity_json(&self.schema)),
            (
                "owner-binding-id".to_owned(),
                Json::String(self.owner_binding_id.clone()),
            ),
            (
                "owner-name".to_owned(),
                Json::String(self.owner_name.clone()),
            ),
            ("module".to_owned(), Json::String(self.module.clone())),
            (
                "fields".to_owned(),
                Json::Array(
                    self.fields
                        .iter()
                        .map(|(name, value)| {
                            Json::Array(vec![Json::String(name.clone()), value.to_json()])
                        })
                        .collect(),
                ),
            ),
            (
                "index-claims".to_owned(),
                Json::Array(self.index_claims.iter().map(index_claim_json).collect()),
            ),
        ])
    }
}
