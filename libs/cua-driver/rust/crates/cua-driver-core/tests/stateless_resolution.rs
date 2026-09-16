use cua_driver_core::element_token::{self, ElementTarget, ResolvedElement};
use cua_driver_core::tool::with_runtime_scope;

fn target() -> ElementTarget {
    with_runtime_scope("stateless-resolution-test".into(), || {
        let snapshot = element_token::mint_snapshot_handle(42, 7);
        let token = element_token::token_for_identity(&snapshot, 3, b"button:save").unwrap();
        let resolved = element_token::resolve_element_args(
            42,
            None,
            Some(&token),
            None,
            None,
            "click",
            |_, target| Ok(Some(target.clone())),
        )
        .unwrap();
        match resolved {
            ResolvedElement::Element { element, .. } => element,
            ResolvedElement::None => panic!("token must identify a target"),
        }
    })
}

#[test]
fn identity_matching_survives_native_worker_handoff() {
    let target = target();
    std::thread::spawn(move || {
        assert!(target.matches_identity(b"button:save"));
        assert!(!target.matches_identity(b"button:cancel"));
    })
    .join()
    .unwrap();
}

#[test]
fn unique_matching_is_independent_of_ordinal() {
    let target = target();
    let candidates = [(3, b"button:cancel".to_vec()), (9, b"button:save".to_vec())];
    assert_eq!(target.resolve_unique(candidates, true).unwrap(), Some(9));
    assert_eq!(
        target
            .resolve_unique([(3, b"button:cancel".to_vec())], true)
            .unwrap(),
        None
    );
}

#[test]
fn duplicate_or_incomplete_matches_refuse() {
    let target = target();
    let candidates = [(3, b"button:save".to_vec()), (9, b"button:save".to_vec())];
    assert!(target.resolve_unique(candidates, true).is_err());
    assert!(target
        .resolve_unique([(3, b"button:save".to_vec())], false)
        .is_err());
    assert!(target
        .resolve_unique(std::iter::empty::<(usize, Vec<u8>)>(), false)
        .is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn native_resolution_does_not_block_dispatch_or_lose_identity() {
    let args = with_runtime_scope("stateless-resolution-test".into(), || {
        let snapshot = element_token::mint_snapshot_handle(42, 7);
        serde_json::json!({"element_token": element_token::token_for_identity(&snapshot, 3, b"button:save").unwrap()})
    });
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let worker = tokio::spawn(async move {
        let future = with_runtime_scope("stateless-resolution-test".into(), || {
            element_token::resolve_native(42, &args, "click", move |window, target| {
                assert_eq!(window, 7);
                let _ = entered.send(());
                blocked
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .map_err(|e| e.to_string())?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap();
                runtime
                    .block_on(async { target.resolve_unique([(9, b"button:save".to_vec())], true) })
            })
        });
        future.await
    });
    ready.await.unwrap();
    release.send(()).unwrap();
    assert!(matches!(
        worker.await.unwrap().unwrap(),
        ResolvedElement::Element { element: 9, .. }
    ));
}

#[test]
fn identity_free_addresses_refuse_before_native_lookup() {
    with_runtime_scope("stateless-resolution-test".into(), || {
        let snapshot = element_token::mint_snapshot_handle(42, 7);
        let token = element_token::token_for(&snapshot, 3);
        for (index, token, snapshot) in [
            (Some(3), None, Some(snapshot.as_str())),
            (None, Some(token.as_str()), None),
        ] {
            let mut looked_up = false;
            let result = element_token::resolve_element_args(
                42,
                index,
                token,
                snapshot,
                None,
                "click",
                |_, _| {
                    looked_up = true;
                    Ok(Some(()))
                },
            );
            assert!(result.is_err());
            assert!(!looked_up);
            assert_eq!(
                result.unwrap_err().structured_content.unwrap()["refusal"]["code"],
                "element_identity_required"
            );
        }
    });
}
