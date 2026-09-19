use cua_driver_core::{element_token, tool::with_runtime_scope};
use std::cell::Cell;

#[test]
fn native_resolution_allows_blocking_rpc_and_preserves_token_scope() {
    with_runtime_scope("native-lookup-owner".into(), || {
        let snapshot = element_token::mint_snapshot_handle(42, 7);
        let token = element_token::token_for_reference(&snapshot, 0, b"button:Save").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let resolved = runtime
            .block_on(element_token::resolve_native(
                42,
                None,
                Some(&token),
                None,
                Some(7),
                "click",
                |_, reference| {
                    // AT-SPI's synchronous facade runs its D-Bus future this way.
                    let rpc_runtime = tokio::runtime::Builder::new_current_thread()
                        .build()
                        .unwrap();
                    let matches = rpc_runtime.block_on(async { reference == b"button:Save" });
                    assert!(matches);
                    assert_eq!(
                        cua_driver_core::tool::current_dispatch_runtime_scope().as_deref(),
                        Some("native-lookup-owner"),
                        "the worker must retain the authenticated caller's scope"
                    );
                    Ok(Some("Save"))
                },
            ))
            .unwrap();
        assert_eq!(resolved.into_parts(None).2, Some("Save"));
    });
}

#[test]
fn identity_free_addresses_refuse_before_current_ordinal_lookup() {
    with_runtime_scope("stateless-address-safety".into(), || {
        let snapshot = element_token::mint_snapshot_handle(42, 7);
        let ordinal_token = element_token::token_for(&snapshot, 0);
        for (token, index, snapshot_id) in [
            (Some(ordinal_token.as_str()), None, None),
            (None, Some(0), Some(snapshot.as_str())),
        ] {
            let lookup_called = Cell::new(false);
            let result = element_token::resolve_element_args(
                42,
                index,
                token,
                snapshot_id,
                Some(7),
                "click",
                |_, _| {
                    lookup_called.set(true);
                    Ok(Some("Delete"))
                },
            );
            assert_eq!(
                result.unwrap_err().structured_content.unwrap()["refusal"]["code"],
                "element_identity_required"
            );
            assert!(!lookup_called.get(), "refuse before native lookup");
        }
    });
}

#[test]
fn observed_reference_still_resolves_after_another_observation() {
    with_runtime_scope("stateless-address-positive".into(), || {
        let snapshot = element_token::mint_snapshot_handle(42, 7);
        let token = element_token::token_for_reference(&snapshot, 0, b"button:Save").unwrap();
        let _later_observation = element_token::mint_snapshot_handle(42, 7);
        let resolved = element_token::resolve_element_args(
            42,
            None,
            Some(&token),
            None,
            Some(7),
            "click",
            |_, reference| {
                Ok([b"button:Delete".as_slice(), b"button:Save".as_slice()]
                    .into_iter()
                    .find(|candidate| *candidate == reference))
            },
        )
        .unwrap();
        assert_eq!(resolved.into_parts(None).2, Some(b"button:Save".as_slice()));
    });
}
