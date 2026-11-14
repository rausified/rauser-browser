//! An order-preserving configuration value.
//!
//! Validation reports the first error in document order, so tables keep their
//! keys in the order the source wrote them. Any serde format (TOML, JSON) can
//! deserialize into this type; the engine never repairs what it is given.

use std::collections::HashSet;
use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<Value>),
    /// Keys in source order.
    Table(Vec<(String, Value)>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_table(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Table(t) => Some(t),
            _ => None,
        }
    }

    /// The first value stored under `key` in a table.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_table()?
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }
}

impl Value {
    /// The value's content as canonical JSON: table keys sorted by their
    /// UTF-8 bytes at every level, no white space, strings and numbers in
    /// `serde_json`'s encoding. Two values with the same content give the
    /// same text whatever order their source wrote the keys in, and the text
    /// never depends on how a Rust type prints itself (`Debug`), so it is a
    /// stable input for a content hash (`Document::canonical_json`).
    pub fn canonical_json(&self) -> String {
        let mut out = String::new();
        self.write_canonical(&mut out);
        out
    }

    fn write_canonical(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Int(n) => out.push_str(&n.to_string()),
            // A non-finite float encodes as `null`, as serde_json does.
            Value::Float(f) => out.push_str(&serde_json::Value::from(*f).to_string()),
            Value::String(s) => out.push_str(&serde_json::Value::from(s.as_str()).to_string()),
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_canonical(out);
                }
                out.push(']');
            }
            Value::Table(entries) => {
                // Keys are unique (deserialization refuses duplicates), so
                // the sorted order is total.
                let mut sorted: Vec<&(String, Value)> = entries.iter().collect();
                sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
                out.push('{');
                for (i, (key, value)) in sorted.into_iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::Value::from(key.as_str()).to_string());
                    out.push(':');
                    value.write_canonical(out);
                }
                out.push('}');
            }
        }
    }
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueVisitor)
    }
}

struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a configuration value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Int(v))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        // Values past i64 are kept as numbers; the format check reads them by
        // value, so an integer this large is still an unsupported format.
        Ok(i64::try_from(v).map_or(Value::Float(v as f64), Value::Int))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
        Ok(Value::Float(v))
    }

    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }

    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }

    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        Value::deserialize(d)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut out = Vec::new();
        while let Some(v) = seq.next_element()? {
            out.push(v);
        }
        Ok(Value::Array(out))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut out: Vec<(String, Value)> = Vec::new();
        // A set beside the ordered list keeps the duplicate check linear.
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(de::Error::custom(format!("duplicate key {key:?}")));
            }
            let value = map.next_value()?;
            out.push((key, value));
        }
        Ok(Value::Table(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_keep_source_order() {
        let v: Value = serde_json::from_str(r#"{"b": 1, "a": [true, null, "x", 1.5]}"#).unwrap();
        let keys: Vec<&str> = v
            .as_table()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(keys, ["b", "a"]);
        assert_eq!(
            v.get("a").unwrap().as_array().unwrap(),
            [
                Value::Bool(true),
                Value::Null,
                Value::String("x".into()),
                Value::Float(1.5)
            ]
        );
    }

    #[test]
    fn canonical_json_sorts_keys_at_every_level() {
        let a: Value =
            serde_json::from_str(r#"{"b": {"z": 1, "y": [2.5, "q\""]}, "a": null, "c": true}"#)
                .unwrap();
        let b: Value =
            serde_json::from_str(r#"{"c": true, "a": null, "b": {"y": [2.5, "q\""], "z": 1}}"#)
                .unwrap();
        assert_eq!(
            a.canonical_json(),
            r#"{"a":null,"b":{"y":[2.5,"q\""],"z":1},"c":true}"#
        );
        assert_eq!(a.canonical_json(), b.canonical_json());
    }

    #[test]
    fn canonical_json_tells_content_apart() {
        let text = |json: &str| {
            serde_json::from_str::<Value>(json)
                .unwrap()
                .canonical_json()
        };
        assert_ne!(text(r#"{"a": [1, 2]}"#), text(r#"{"a": [2, 1]}"#));
        assert_ne!(text(r#"{"a": "1"}"#), text(r#"{"a": 1}"#));
        assert_ne!(text(r#"{"a": 1}"#), text(r#"{"a": 1, "b": null}"#));
        assert_eq!(text(r#"{"k": "é\u0001"}"#), "{\"k\":\"é\\u0001\"}");
    }

    #[test]
    fn duplicate_keys_are_refused() {
        assert!(serde_json::from_str::<Value>(r#"{"a": 1, "a": 2}"#).is_err());
    }

    #[test]
    fn large_tables_deserialize_in_linear_time() {
        // A pairwise duplicate check does about 5 * 10^9 comparisons here.
        let body: Vec<String> = (0..100_000).map(|i| format!("\"k{i}\": {i}")).collect();
        let json = format!("{{{}}}", body.join(","));
        let start = std::time::Instant::now();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v.as_table().unwrap().len(), 100_000);
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
    }
}
