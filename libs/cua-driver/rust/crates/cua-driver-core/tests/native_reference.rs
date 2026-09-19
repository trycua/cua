use cua_driver_core::{element_token, tool::with_runtime_scope};

#[test]
fn reference_payload_round_trips_without_shared_matching_policy() {
    with_runtime_scope("opaque-reference-roundtrip".into(), || {
        let observed = element_token::mint_snapshot_handle(17, 29);
        // The shared layer must not prescribe JSON, fields, or a fingerprint size.
        let payload = b"native\0reference.with/separators\xff";
        let token = element_token::token_for_reference(&observed, 4, payload).unwrap();
        let _later = element_token::mint_snapshot_handle(17, 29);
        let resolved = element_token::resolve_element_args(
            17,
            None,
            Some(&token),
            None,
            Some(29),
            "click",
            |window, reference| {
                assert_eq!(window, 29);
                assert_eq!(reference, payload);
                Ok(Some("native target"))
            },
        )
        .unwrap();
        assert_eq!(
            resolved.into_parts(None),
            (Some(4), Some(29), Some("native target"))
        );
    });
}

#[test]
fn backend_refusals_are_preserved_without_shared_tree_policy() {
    with_runtime_scope("opaque-reference-refusal".into(), || {
        let observed = element_token::mint_snapshot_handle(17, 29);
        let token = element_token::token_for_reference(&observed, 4, b"native target").unwrap();
        let error = element_token::resolve_element_args::<(), _>(
            17,
            None,
            Some(&token),
            None,
            Some(29),
            "click",
            |_, reference| {
                assert_eq!(reference, b"native target");
                Err("native reference is unavailable".into())
            },
        )
        .unwrap_err();
        let refusal = &error.structured_content.unwrap()["refusal"];
        assert_eq!(refusal["code"], "element_resolution_failed");
        assert_eq!(refusal["message"], "native reference is unavailable");
    });
}

#[test]
fn fingerprint_version_is_not_reinterpreted_as_a_reference() {
    with_runtime_scope("opaque-reference-version".into(), || {
        let observed = element_token::mint_snapshot_handle(17, 29);
        let token = element_token::token_for_reference(&observed, 4, b"native target").unwrap();
        assert!(token.starts_with("et2."));
        let old_version = token.replacen("et2.", "et1.", 1);
        let error = element_token::resolve_element_args::<(), _>(
            17,
            None,
            Some(&old_version),
            None,
            Some(29),
            "click",
            |_, _| panic!("old envelope must be rejected before native lookup"),
        )
        .unwrap_err();
        assert_eq!(
            error.structured_content.unwrap()["refusal"]["code"],
            "invalid_element_token"
        );
    });
}

#[test]
fn modified_reference_is_rejected_before_native_resolution() {
    with_runtime_scope("opaque-reference-tamper".into(), || {
        let observed = element_token::mint_snapshot_handle(17, 29);
        let token = element_token::token_for_reference(&observed, 4, b"native target").unwrap();
        let mut fields: Vec<_> = token.split('.').map(str::to_owned).collect();
        let reference_index = fields.len() - 2;
        fields[reference_index] = "YW5vdGhlciB0YXJnZXQ".into();
        let modified = fields.join(".");
        let result = element_token::resolve_element_args::<(), _>(
            17,
            None,
            Some(&modified),
            None,
            Some(29),
            "click",
            |_, _| panic!("tampering must refuse before native resolution"),
        )
        .unwrap_err();
        assert_eq!(
            result.structured_content.as_ref().unwrap()["refusal"]["code"],
            "invalid_element_token"
        );
    });
}
