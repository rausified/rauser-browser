use brauser_protocol::{AdoptedState, Request};

#[test]
fn adopted_state_and_its_forget_request_round_trip() {
    let request = r#"{"type":"forget_adopted_contracts","protocol_version":8,"request_id":"r","expected_revision":"sha256:abc"}"#;
    let parsed: Request = serde_json::from_str(request).unwrap();
    assert_eq!(serde_json::to_string(&parsed).unwrap(), request);
    // The revision is required, as for update_config.
    assert!(
        serde_json::from_str::<Request>(
            r#"{"type":"forget_adopted_contracts","protocol_version":8,"request_id":"r"}"#
        )
        .is_err()
    );

    let state = r#"{"issue":null,"revision":"sha256:abc","warnings":["full"],"full":true}"#;
    let parsed: AdoptedState = serde_json::from_str(state).unwrap();
    assert_eq!(serde_json::to_string(&parsed).unwrap(), state);
    // full is required, as are issue (even when null) and the rest.
    assert!(
        serde_json::from_str::<AdoptedState>(
            r#"{"issue":null,"revision":"missing","warnings":[]}"#
        )
        .is_err()
    );
    // issue is required even when null, and no other key is allowed.
    assert!(
        serde_json::from_str::<AdoptedState>(
            r#"{"revision":"missing","warnings":[],"full":false}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<AdoptedState>(
            r#"{"issue":null,"revision":"missing","warnings":[],"full":false,"extra":1}"#
        )
        .is_err()
    );
}
