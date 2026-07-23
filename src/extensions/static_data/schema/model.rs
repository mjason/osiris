use super::datum::*;

/// The closed v0 schema type language. `OneOf` contains static datums rather
/// than names so that a schema remains independent of a runtime registry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum StaticType {
    Any,
    None,
    Bool,
    Int,
    Float,
    Str,
    Keyword,
    Symbol,
    List(Box<StaticType>),
    Vector(Box<StaticType>),
    Map(Box<StaticType>, Box<StaticType>),
    Set(Box<StaticType>),
    Optional(Box<StaticType>),
    OneOf(Vec<StaticDatum>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SchemaField {
    pub name: String,
    #[serde(rename = "type")]
    pub datum_type: StaticType,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<StaticDatum>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum ProjectionKind {
    Field,
    Each,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IndexProjection {
    pub kind: ProjectionKind,
    pub field: String,
    pub role: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SchemaIndex {
    pub id: String,
    pub scope: String,
    pub projections: Vec<IndexProjection>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StaticSchema {
    pub name: String,
    pub schema_id: String,
    pub version: u64,
    pub fields: Vec<SchemaField>,
    pub indexes: Vec<SchemaIndex>,
    /// Hash of the canonical schema body, excluding source spans/metadata.
    pub body_hash: String,
}

impl StaticSchema {
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&SchemaField> {
        self.fields.iter().find(|field| field.name == name)
    }

    #[must_use]
    pub fn identity(&self, binding_id: impl Into<String>) -> SchemaIdentity {
        SchemaIdentity {
            binding_id: binding_id.into(),
            schema_id: self.schema_id.clone(),
            version: self.version,
            body_hash: self.body_hash.clone(),
        }
    }

    pub(in crate::records) fn body_json(&self) -> Json {
        Json::Object(vec![
            ("schema-id".to_owned(), Json::String(self.schema_id.clone())),
            ("version".to_owned(), Json::Number(self.version.to_string())),
            (
                "fields".to_owned(),
                Json::Array(self.fields.iter().map(schema_field_json).collect()),
            ),
            (
                "indexes".to_owned(),
                Json::Array(self.indexes.iter().map(schema_index_json).collect()),
            ),
        ])
    }

    #[must_use]
    pub fn canonical_body_bytes(&self) -> Vec<u8> {
        self.body_json().bytes()
    }

    /// Verify a schema reconstructed from a data-only compilation interface.
    /// This repeats the structural invariants normally established while
    /// lowering `defstatic-schema` and authenticates its embedded body hash.
    pub fn verify_integrity(&self) -> Result<(), RecordError> {
        if self.name.is_empty() {
            return Err(RecordError::new(RECORD_SCHEMA_SHAPE, "empty schema name"));
        }
        if !is_namespaced(&self.schema_id) {
            return Err(RecordError::new(
                RECORD_SCHEMA_SHAPE,
                "schema-id must contain a namespace separator `/`",
            ));
        }
        if self.version == 0 {
            return Err(RecordError::new(
                RECORD_SCHEMA_SHAPE,
                "schema version must be positive",
            ));
        }

        let mut field_names = BTreeSet::new();
        for field in &self.fields {
            if field.name.is_empty() || !field_names.insert(field.name.clone()) {
                return Err(RecordError::new(
                    RECORD_SCHEMA_FIELD,
                    format!("empty or duplicate schema field `{}`", field.name),
                ));
            }
            verify_static_type(&field.datum_type)?;
            if let Some(default) = &field.default {
                if default.clone().canonicalize()? != *default {
                    return Err(RecordError::new(
                        RECORD_SCHEMA_FIELD,
                        format!("default for field `{}` is not canonical", field.name),
                    ));
                }
                if !field.datum_type.accepts(default) {
                    return Err(RecordError::new(
                        RECORD_SCHEMA_TYPE,
                        format!("default for field `{}` does not match its type", field.name),
                    ));
                }
                if field.required {
                    return Err(RecordError::new(
                        RECORD_SCHEMA_FIELD,
                        format!("required field `{}` cannot also have a default", field.name),
                    ));
                }
            }
        }

        let field_types = self
            .fields
            .iter()
            .map(|field| (field.name.as_str(), &field.datum_type))
            .collect::<BTreeMap<_, _>>();
        let mut index_ids = BTreeSet::new();
        for index in &self.indexes {
            if !is_namespaced(&index.id) || !index_ids.insert(index.id.clone()) {
                return Err(RecordError::new(
                    RECORD_SCHEMA_INDEX,
                    format!("invalid or duplicate schema index `{}`", index.id),
                ));
            }
            if index.scope.is_empty() || index.projections.is_empty() {
                return Err(RecordError::new(
                    RECORD_SCHEMA_INDEX,
                    format!(
                        "schema index `{}` has an empty scope or projection list",
                        index.id
                    ),
                ));
            }
            for projection in &index.projections {
                let field_type = field_types.get(projection.field.as_str()).ok_or_else(|| {
                    RecordError::new(
                        RECORD_SCHEMA_INDEX,
                        format!(
                            "index `{}` references unknown field `{}`",
                            index.id, projection.field
                        ),
                    )
                })?;
                if projection.role.is_empty() {
                    return Err(RecordError::new(
                        RECORD_SCHEMA_INDEX,
                        format!("index `{}` has an empty projection role", index.id),
                    ));
                }
                if matches!(projection.kind, ProjectionKind::Each)
                    && !matches!(
                        field_type,
                        StaticType::List(_) | StaticType::Vector(_) | StaticType::Set(_)
                    )
                {
                    return Err(RecordError::new(
                        RECORD_SCHEMA_INDEX,
                        format!(
                            "`:each` projection `{}` must target a collection field",
                            projection.field
                        ),
                    ));
                }
            }
        }

        let expected = sha256(&self.canonical_body_bytes());
        if self.body_hash != expected {
            return Err(RecordError::new(
                RECORD_SCHEMA_SHAPE,
                "schema body hash does not match schema payload",
            ));
        }
        Ok(())
    }
}

pub(in crate::records) fn verify_static_type(datum_type: &StaticType) -> Result<(), RecordError> {
    match datum_type {
        StaticType::List(inner)
        | StaticType::Vector(inner)
        | StaticType::Set(inner)
        | StaticType::Optional(inner) => verify_static_type(inner),
        StaticType::Map(key, value) => {
            verify_static_type(key)?;
            verify_static_type(value)
        }
        StaticType::OneOf(values) => {
            if values.is_empty() {
                return Err(RecordError::new(
                    RECORD_SCHEMA_TYPE,
                    "OneOf requires at least one static value",
                ));
            }
            let mut seen = BTreeSet::new();
            for value in values {
                if value.clone().canonicalize()? != *value || !seen.insert(value.canonical_bytes())
                {
                    return Err(RecordError::new(
                        RECORD_SCHEMA_TYPE,
                        "OneOf contains a non-canonical or duplicate value",
                    ));
                }
            }
            Ok(())
        }
        StaticType::Any
        | StaticType::None
        | StaticType::Bool
        | StaticType::Int
        | StaticType::Float
        | StaticType::Str
        | StaticType::Keyword
        | StaticType::Symbol => Ok(()),
    }
}
