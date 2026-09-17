use cua_driver_core::{element_token, tool::with_runtime_scope};
use std::cell::Cell;

#[test]
fn incomplete_walk_cannot_prove_a_unique_identity() {
    with_runtime_scope("partial-walk".into(), || {
        let snapshot = element_token::mint_snapshot_handle(42, 7);
        let token = element_token::token_for_identity(&snapshot, 0, b"Save").unwrap();
        let lookup = |complete, names: Vec<&str>| {
            element_token::resolve_element_args(
                42,
                None,
                Some(&token),
                None,
                Some(7),
                "click",
                |_, target| {
                    target.resolve_unique(
                        names
                            .into_iter()
                            .enumerate()
                            .map(|(index, name)| (name.as_bytes().to_vec(), index)),
                        complete,
                    )
                },
            )
        };
        assert!(
            lookup(false, vec!["Save"]).is_err(),
            "another match may be hidden beyond the cut"
        );
        assert!(lookup(true, vec!["Save", "Save"]).is_err());
        assert!(lookup(true, vec!["Delete"]).is_err());
        assert_eq!(
            lookup(true, vec!["Delete", "Save"])
                .unwrap()
                .into_parts(None)
                .2,
            Some(1)
        );
    });
}

#[test]
fn missing_native_identity_still_counts_as_an_ambiguous_match() {
    let snapshot = element_token::mint_snapshot_handle(42, 7);
    let token = element_token::token_for_identity(&snapshot, 0, b"Save").unwrap();
    let result = element_token::resolve_element_args(
        42,
        None,
        Some(&token),
        None,
        Some(7),
        "click",
        |_, target| {
            target.resolve_unique(
                vec![
                    (b"Save".to_vec(), None),
                    (b"Save".to_vec(), Some(String::from("native identity"))),
                ],
                true,
            )
        },
    );
    assert_eq!(
        result.unwrap_err().structured_content.unwrap()["refusal"]["code"],
        "invalid_element_token",
        "missing native identity must not hide a duplicate"
    );
}

#[test]
fn native_resolution_allows_blocking_rpc_and_preserves_token_scope() {
    with_runtime_scope("native-lookup-owner".into(), || {
        let snapshot = element_token::mint_snapshot_handle(42, 7);
        let token = element_token::token_for_identity(&snapshot, 0, b"button:Save").unwrap();
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
                |_, target| {
                    // AT-SPI's synchronous facade runs its D-Bus future this way.
                    let rpc_runtime = tokio::runtime::Builder::new_current_thread()
                        .build()
                        .unwrap();
                    let matches =
                        rpc_runtime.block_on(async { target.matches_identity(b"button:Save") });
                    assert!(
                        matches,
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
                |_, target| {
                    lookup_called.set(true);
                    let changed_controls = ["Delete", "Save"];
                    Ok(changed_controls.get(target.element_index).copied())
                },
            );
            assert!(
                result.is_err(),
                "an ordinal cannot identify the observed control"
            );
            assert!(!lookup_called.get(), "refuse before native lookup");
        }
    });
}

#[test]
fn observed_identity_still_resolves_after_another_observation() {
    with_runtime_scope("stateless-address-positive".into(), || {
        let snapshot = element_token::mint_snapshot_handle(42, 7);
        let token = element_token::token_for_identity(&snapshot, 0, b"button:Save").unwrap();
        let _later_observation = element_token::mint_snapshot_handle(42, 7);
        let resolved = element_token::resolve_element_args(
            42,
            None,
            Some(&token),
            None,
            Some(7),
            "click",
            |_, target| {
                Ok([b"button:Delete".as_slice(), b"button:Save".as_slice()]
                    .into_iter()
                    .find(|description| target.matches_identity(description)))
            },
        )
        .unwrap();
        assert_eq!(resolved.into_parts(None).2, Some(b"button:Save".as_slice()));
    });
}
