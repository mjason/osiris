/// Ordered JSON tree used to reject duplicate names before lookup.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::records) enum Json {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub(in crate::records) fn bytes(&self) -> Vec<u8> {
        let mut output = String::new();
        self.write(&mut output);
        output.into_bytes()
    }

    pub(in crate::records) fn write(&self, output: &mut String) {
        match self {
            Self::Null => output.push_str("null"),
            Self::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
            Self::Number(value) => output.push_str(value),
            Self::String(value) => output.push_str(&json_quote(value)),
            Self::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        output.push(',');
                    }
                    value.write(output);
                }
                output.push(']');
            }
            Self::Object(fields) => {
                let mut fields = fields.iter().collect::<Vec<_>>();
                fields.sort_by(|left, right| utf16_cmp(&left.0, &right.0));
                output.push('{');
                for (index, (key, value)) in fields.iter().enumerate() {
                    if index != 0 {
                        output.push(',');
                    }
                    output.push_str(&json_quote(key));
                    output.push(':');
                    value.write(output);
                }
                output.push('}');
            }
        }
    }
}

impl Serialize for StaticDatum {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_json(&self.to_json(), serializer)
    }
}

pub(in crate::records) fn serialize_json<S>(value: &Json, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Json::Null => serializer.serialize_unit(),
        Json::Bool(value) => serializer.serialize_bool(*value),
        Json::Number(value) => {
            if let Ok(number) = value.parse::<u64>() {
                serializer.serialize_u64(number)
            } else if let Ok(number) = value.parse::<i64>() {
                serializer.serialize_i64(number)
            } else if let Ok(number) = value.parse::<f64>() {
                serializer.serialize_f64(number)
            } else {
                Err(ser::Error::custom("invalid canonical JSON number"))
            }
        }
        Json::String(value) => serializer.serialize_str(value),
        Json::Array(values) => {
            let mut sequence = serializer.serialize_seq(Some(values.len()))?;
            for value in values {
                sequence.serialize_element(&JsonSerializable(value))?;
            }
            sequence.end()
        }
        Json::Object(fields) => {
            let mut map = serializer.serialize_map(Some(fields.len()))?;
            for (key, value) in fields {
                map.serialize_entry(key, &JsonSerializable(value))?;
            }
            map.end()
        }
    }
}

struct JsonSerializable<'a>(&'a Json);

impl Serialize for JsonSerializable<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_json(self.0, serializer)
    }
}

pub(in crate::records) fn utf16_cmp(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

pub(in crate::records) fn json_quote(value: &str) -> String {
    // serde_json's string encoder follows JSON's required escaping rules and
    // leaves non-control Unicode scalars intact, which is also JCS's form.
    serde_json::to_string(value).expect("Rust strings are always JSON strings")
}

pub(in crate::records) fn tagged(tag: &str, mut fields: Vec<(&str, Json)>) -> Json {
    let mut object = vec![("$osiris".to_owned(), Json::String(tag.to_owned()))];
    object.extend(fields.drain(..).map(|(key, value)| (key.to_owned(), value)));
    Json::Object(object)
}

pub(in crate::records) fn object_field<'a>(
    fields: &'a [(String, Json)],
    key: &str,
) -> Result<&'a Json, RecordError> {
    fields
        .iter()
        .find_map(|(name, value)| (name == key).then_some(value))
        .ok_or_else(|| RecordError::new(RECORD_SIDECAR, format!("missing JSON member `{key}`")))
}

pub(in crate::records) fn object_string(
    fields: &[(String, Json)],
    key: &str,
) -> Result<String, RecordError> {
    match object_field(fields, key)? {
        Json::String(value) => Ok(value.clone()),
        _ => Err(RecordError::new(
            RECORD_SIDECAR,
            format!("JSON member `{key}` must be a string"),
        )),
    }
}

pub(in crate::records) fn object_optional_string(
    fields: &[(String, Json)],
    key: &str,
) -> Result<Option<String>, RecordError> {
    match fields
        .iter()
        .find_map(|(name, value)| (name == key).then_some(value))
    {
        None => Ok(None),
        Some(Json::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(RecordError::new(
            RECORD_SIDECAR,
            format!("JSON member `{key}` must be a string"),
        )),
    }
}

pub(in crate::records) fn object_array<'a>(
    fields: &'a [(String, Json)],
    key: &str,
) -> Result<&'a [Json], RecordError> {
    match object_field(fields, key)? {
        Json::Array(values) => Ok(values),
        _ => Err(RecordError::new(
            RECORD_SIDECAR,
            format!("JSON member `{key}` must be an array"),
        )),
    }
}
