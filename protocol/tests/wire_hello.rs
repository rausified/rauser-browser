//! The hello handshake's wire shape, including the optional
//! `extension_build` (the build Chrome has loaded, recorded by the host for
//! scripts/dev/deploy-live; additive within protocol version 8).

use brauser_protocol::Request;
use serde_json::{Value, json};

mod common;
use common::{assert_request_round_trips, schema_subset};

fn hello() -> Value {
    json!({"type": "hello", "protocol_version": 8, "request_id": "h"})
}

#[test]
fn a_hello_without_a_build_still_round_trips() {
    let Request::Hello(value) = assert_request_round_trips("HelloRequest", &hello()) else {
        panic!("expected hello");
    };
    assert_eq!(value.extension_build, None);
}

#[test]
fn a_hello_carries_an_optional_extension_build() {
    let mut value = hello();
    value["extension_build"] = json!("0.1.0+abcdef012345");
    let Request::Hello(parsed) = assert_request_round_trips("HelloRequest", &value) else {
        panic!("expected hello");
    };
    assert_eq!(
        parsed.extension_build.as_deref(),
        Some("0.1.0+abcdef012345")
    );
}

#[test]
fn the_schema_bounds_the_extension_build() {
    for bad in [json!(""), json!("x".repeat(129)), json!(1), json!(null)] {
        let mut value = hello();
        value["extension_build"] = bad.clone();
        assert!(!schema_subset::valid("HelloRequest", &value), "{bad}");
    }
    let mut value = hello();
    value["extension_build"] = json!("x".repeat(128));
    assert!(schema_subset::valid("HelloRequest", &value));
}

#[test]
fn hello_still_refuses_other_fields() {
    let mut value = hello();
    value["extension_version"] = json!("1");
    assert!(!schema_subset::valid("HelloRequest", &value));
    assert!(serde_json::from_value::<Request>(value).is_err());
}
