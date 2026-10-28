//! Helpers shared by the `wire_*` protocol test crates.
//!
//! Each `tests/wire_<feature>.rs` file is its own crate that uses only some of
//! these, so every item allows dead code.

use brauser_protocol::{Request, Response};
use serde_json::Value;

// Every agent, vocabulary and adapter key is required on the wire, like `storage`.
#[allow(dead_code)]
pub const AGENT_AND_ADAPTERS: &str = concat!(
    r#""agent":null,"#,
    r#""summary_vocabulary":{"document_types":["report"],"categories":["news"]},"#,
    r#""adapters":{"format":1,"artifacts":[]}"#,
);

#[allow(dead_code)]
pub fn config(storage: &str) -> String {
    format!(
        r#"{{{storage}"capture_enabled":false,"strip_params":["utm_*","fbclid","gclid"],{AGENT_AND_ADAPTERS}}}"#
    )
}

#[allow(dead_code)]
pub fn update(config: &str) -> String {
    format!(
        r#"{{"type":"update_config","protocol_version":8,"request_id":"r1","expected_revision":"missing","picker_token":null,"knowledge_base_token":null,"config":{config}}}"#
    )
}

/// A request that serde reads and the schema accepts, written back unchanged.
#[allow(dead_code)]
pub fn assert_request_round_trips(definition: &str, value: &Value) -> Request {
    assert!(
        schema_subset::valid(definition, value),
        "schema refused {value}"
    );
    let parsed: Request = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(&serde_json::to_value(&parsed).unwrap(), value);
    parsed
}

#[allow(dead_code)]
pub fn assert_response_round_trips(definition: &str, value: &Value) -> Response {
    assert!(
        schema_subset::valid(definition, value),
        "schema refused {value}"
    );
    let parsed: Response = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(&serde_json::to_value(&parsed).unwrap(), value);
    parsed
}

#[allow(dead_code)]
pub mod schema_subset {
    use serde_json::Value;

    const KEYWORDS: &[&str] = &[
        "$ref",
        "oneOf",
        "const",
        "enum",
        "type",
        "properties",
        "required",
        "additionalProperties",
        "items",
        "minItems",
        "maxItems",
        "minLength",
        "maxLength",
        "minimum",
        "maximum",
        "description",
        // Not checked here (no regex in this subset); the host checks a
        // requested_at's form and window at runtime.
        "pattern",
    ];

    pub fn schema() -> Value {
        serde_json::from_str(include_str!("../../schema.json")).unwrap()
    }

    /// Whether `value` satisfies `#/$defs/<definition>`.
    pub fn valid(definition: &str, value: &Value) -> bool {
        let root = schema();
        let shape = &root["$defs"][definition];
        assert!(shape.is_object(), "no definition {definition}");
        matches(&root, shape, value)
    }

    fn number(shape: &serde_json::Map<String, Value>, key: &str) -> Option<u64> {
        shape.get(key).map(|bound| bound.as_u64().unwrap())
    }

    fn matches(root: &Value, shape: &Value, value: &Value) -> bool {
        let shape = shape.as_object().unwrap();
        for key in shape.keys() {
            assert!(
                KEYWORDS.contains(&key.as_str()),
                "unsupported keyword {key}"
            );
        }
        if let Some(reference) = shape.get("$ref") {
            let name = reference
                .as_str()
                .unwrap()
                .strip_prefix("#/$defs/")
                .unwrap();
            return matches(root, &root["$defs"][name], value);
        }
        if let Some(alternatives) = shape.get("oneOf") {
            let passing = alternatives
                .as_array()
                .unwrap()
                .iter()
                .filter(|alternative| matches(root, alternative, value))
                .count();
            return passing == 1;
        }
        if shape.get("const").is_some_and(|constant| value != constant) {
            return false;
        }
        if shape
            .get("enum")
            .is_some_and(|values| !values.as_array().unwrap().contains(value))
        {
            return false;
        }
        match shape.get("type").and_then(Value::as_str) {
            None => true,
            Some("null") => value.is_null(),
            Some("boolean") => value.is_boolean(),
            Some("integer") => value.as_u64().is_some_and(|n| {
                number(shape, "minimum").is_none_or(|min| n >= min)
                    && number(shape, "maximum").is_none_or(|max| n <= max)
            }),
            Some("string") => value.as_str().is_some_and(|text| {
                let length = text.chars().count() as u64;
                number(shape, "minLength").is_none_or(|min| length >= min)
                    && number(shape, "maxLength").is_none_or(|max| length <= max)
            }),
            Some("array") => value.as_array().is_some_and(|items| {
                let length = items.len() as u64;
                number(shape, "minItems").is_none_or(|min| length >= min)
                    && number(shape, "maxItems").is_none_or(|max| length <= max)
                    && shape
                        .get("items")
                        .is_none_or(|item| items.iter().all(|entry| matches(root, item, entry)))
            }),
            Some("object") => value.as_object().is_some_and(|object| {
                let properties = shape.get("properties").and_then(Value::as_object);
                let required = shape
                    .get("required")
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice);
                let closed = shape.get("additionalProperties") == Some(&Value::Bool(false));
                required
                    .iter()
                    .all(|key| object.contains_key(key.as_str().unwrap()))
                    && object.iter().all(|(key, entry)| {
                        match properties.and_then(|known| known.get(key)) {
                            Some(property) => matches(root, property, entry),
                            None => !closed,
                        }
                    })
            }),
            Some(other) => panic!("unsupported type {other}"),
        }
    }
}
