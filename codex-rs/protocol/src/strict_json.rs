//! Duplicate-aware JSON decoding for security-sensitive protocol inputs.

use std::fmt;

use serde::Deserializer;
use serde::de::DeserializeOwned;
use serde::de::DeserializeSeed;
use serde::de::MapAccess;
use serde::de::SeqAccess;
use serde::de::Visitor;
use serde_json::Map;
use serde_json::Number;
use serde_json::Value;

/// Deserialize one JSON value while rejecting duplicate object keys at every
/// nesting level. This is intended for `deserialize_with` on fields that would
/// otherwise be decoded directly into [`Value`].
pub fn deserialize_value_no_duplicates<'de, D>(deserializer: D) -> Result<Value, D::Error>
where
    D: Deserializer<'de>,
{
    NoDuplicateValueSeed.deserialize(deserializer)
}

/// Decode a complete JSON document, rejecting duplicate keys recursively and
/// trailing bytes before converting it to the requested type.
pub fn from_slice_no_duplicates<T>(bytes: &[u8]) -> Result<T, serde_json::Error>
where
    T: DeserializeOwned,
{
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = NoDuplicateValueSeed.deserialize(&mut deserializer)?;
    deserializer.end()?;
    serde_json::from_value(value)
}

/// Validate a complete JSON document without retaining its decoded value.
pub fn validate_slice_no_duplicates(bytes: &[u8]) -> Result<(), serde_json::Error> {
    from_slice_no_duplicates::<Value>(bytes).map(|_| ())
}

struct NoDuplicateValueSeed;

impl<'de> DeserializeSeed<'de> for NoDuplicateValueSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(NoDuplicateValueVisitor)
    }
}

struct NoDuplicateValueVisitor;

impl<'de> Visitor<'de> for NoDuplicateValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Value::String(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        NoDuplicateValueSeed.deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(NoDuplicateValueSeed)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(serde::de::Error::custom(format!(
                    "duplicate object key {key:?}"
                )));
            }
            let value = object.next_value_seed(NoDuplicateValueSeed)?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_json_rejects_nested_duplicate_keys() {
        let error = from_slice_no_duplicates::<Value>(
            br#"{"schema":{"properties":{"id":{"type":"string","type":"number"}}}}"#,
        )
        .expect_err("nested duplicate must fail");

        assert!(error.to_string().contains("duplicate object key \"type\""));
    }

    #[test]
    fn strict_json_rejects_trailing_documents() {
        from_slice_no_duplicates::<Value>(br#"{} {}"#)
            .expect_err("a second JSON document must fail");
    }
}
