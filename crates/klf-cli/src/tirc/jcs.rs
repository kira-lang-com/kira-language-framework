use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Number, Value};

pub(in crate::tirc) const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

pub(in crate::tirc) fn parse_json(bytes: &[u8], label: &str) -> Result<Value, String> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = StrictValue::deserialize(&mut deserializer)
        .map_err(|error| format!("{label} is invalid JSON: {error}"))?;
    deserializer
        .end()
        .map_err(|error| format!("{label} is invalid JSON: {error}"))?;
    Ok(value.0)
}

pub(in crate::tirc) fn canonical_json(value: &Value) -> Result<String, String> {
    validate_numbers(value)?;
    serde_jcs::to_string(value)
        .map_err(|error| format!("RFC 8785 JSON canonicalization failed: {error}"))
}

fn validate_numbers(value: &Value) -> Result<(), String> {
    match value {
        Value::Number(number) if number.is_i64() => {
            let integer = number
                .as_i64()
                .ok_or_else(|| "JSON signed integer is out of range".to_owned())?;
            if !exactly_representable(integer.unsigned_abs()) {
                return Err(format!(
                    "JSON integer {integer} is not exactly representable as an IEEE 754 double; encode it as a string"
                ));
            }
        }
        Value::Number(number) if number.is_u64() => {
            let integer = number
                .as_u64()
                .ok_or_else(|| "JSON unsigned integer is out of range".to_owned())?;
            if !exactly_representable(integer) {
                return Err(format!(
                    "JSON integer {integer} is not exactly representable as an IEEE 754 double; encode it as a string"
                ));
            }
        }
        Value::Number(number) => {
            let float = number.as_f64().ok_or_else(|| {
                "JSON number is not representable as an IEEE 754 double".to_owned()
            })?;
            if !float.is_finite() {
                return Err("JSON number must be finite".to_owned());
            }
        }
        Value::Array(values) => {
            for value in values {
                validate_numbers(value)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                validate_numbers(value)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::String(_) => {}
    }
    Ok(())
}

fn exactly_representable(integer: u64) -> bool {
    let significant_bits = u64::BITS - integer.leading_zeros();
    let discarded_bits = significant_bits.saturating_sub(53);
    discarded_bits == 0 || integer.trailing_zeros() >= discarded_bits
}

struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictValueVisitor)
    }
}

struct StrictValueVisitor;

impl<'de> Visitor<'de> for StrictValueVisitor {
    type Value = StrictValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value without duplicate object properties")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(Number::from(value))))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(Number::from(value))))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(|number| StrictValue(Value::Number(number)))
            .ok_or_else(|| E::custom("JSON number must be finite"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictValue>()? {
            values.push(value.0);
        }
        Ok(StrictValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom(format!(
                    "duplicate object property `{key}`"
                )));
            }
            let value = object.next_value::<StrictValue>()?;
            values.insert(key, value.0);
        }
        Ok(StrictValue(Value::Object(values)))
    }
}
