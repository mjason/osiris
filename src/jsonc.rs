//! Shared JSONC structural validation used by runtime config and build inputs.

use std::{collections::BTreeSet, fmt};

use serde::Deserialize;

struct CheckedValue;

impl<'de> Deserialize<'de> for CheckedValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(CheckedVisitor)
    }
}

struct CheckedVisitor;

impl<'de> serde::de::Visitor<'de> for CheckedVisitor {
    type Value = CheckedValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSONC data without duplicate object keys")
    }

    fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
        Ok(CheckedValue)
    }

    fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
        Ok(CheckedValue)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
        Ok(CheckedValue)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
        Ok(CheckedValue)
    }

    fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
        Ok(CheckedValue)
    }

    fn visit_string<E>(self, _: String) -> Result<Self::Value, E> {
        Ok(CheckedValue)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(CheckedValue)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(CheckedValue)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        while sequence.next_element::<CheckedValue>()?.is_some() {}
        Ok(CheckedValue)
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut seen = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(serde::de::Error::custom(format!(
                    "duplicate JSONC field `{key}`"
                )));
            }
            map.next_value::<CheckedValue>()?;
        }
        Ok(CheckedValue)
    }
}

pub(crate) fn validate_no_duplicate_keys(source: &str) -> Result<(), String> {
    json5::from_str::<CheckedValue>(source)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_nested_duplicate_jsonc_keys() {
        let error = validate_no_duplicate_keys("{outer: {policy: 1, policy: 2}}")
            .expect_err("duplicate nested key");
        assert!(error.contains("duplicate JSONC field `policy`"));
        validate_no_duplicate_keys("{/* comment */ outer: {policy: 1},}").unwrap();
    }
}
