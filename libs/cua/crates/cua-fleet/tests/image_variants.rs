#![allow(deprecated)] // exercises the deprecated `apply_pool` wrapper too
//! The runtime/image rule over the shared fixture manifests
//! (`tests/fixtures/image-variants.json`, also run by cua-sandbox's Python
//! tests), plus the rule's wiring into `FleetClient::apply_pool`.

use cua_fleet::{
    ImageEvidence, ImageVariant, PoolSpec, RuntimeKind, check_runtime,
    runtime::classify_manifest_json, testing::FakeFleet,
};
use serde_json::Value;

fn cases() -> Vec<Value> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/image-variants.json");
    let doc: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    doc["cases"].as_array().unwrap().clone()
}

fn runtime(name: &str) -> RuntimeKind {
    cua_fleet::parse_runtime(name).unwrap().unwrap()
}

#[test]
fn fixture_manifests_classify_and_pick_their_runtime() {
    let cases = cases();
    assert!(cases.len() >= 8);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let image = case["reference"].as_str().unwrap();
        let config = (!case["config"].is_null()).then(|| case["config"].to_string());
        let classified = classify_manifest_json(&case["manifest"].to_string(), config.as_deref());
        if let Some(refused) = case["refused"].as_str() {
            let e = classified.unwrap_err();
            assert!(matches!(e, cua_fleet::Error::Unsupported(_)), "{name}: {e}");
            assert!(e.to_string().contains(refused), "{name}: {e}");
            continue;
        }
        let variant = classified.unwrap();
        assert_eq!(
            variant,
            ImageVariant::parse(case["variant"].as_str().unwrap()).unwrap(),
            "{name}"
        );
        let evidence = ImageEvidence::Known(variant);
        let rts = &case["runtimes"];
        assert_eq!(
            check_runtime(None, image, &evidence).unwrap(),
            runtime(rts["default"].as_str().unwrap()),
            "{name}: default runtime"
        );
        for rt in ["kubevirt", "gvisor"] {
            let ok = check_runtime(Some(runtime(rt)), image, &evidence).is_ok();
            assert_eq!(ok, rts[rt].as_bool().unwrap(), "{name}: {rt}");
        }
    }
}

#[test]
fn an_index_is_classified_from_its_platforms() {
    let index = r#"{"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json",
        "manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json",
        "digest":"sha256:00","size":1,"platform":{"os":"darwin","architecture":"arm64"}}]}"#;
    let e = classify_manifest_json(index, None).unwrap_err();
    assert!(matches!(e, cua_fleet::Error::Unsupported(_)), "{e}");
    assert!(classify_manifest_json("not json", None).is_err());
}

#[tokio::test]
async fn apply_pool_refuses_a_runtime_the_manifest_rules_out() {
    let fake = FakeFleet::new();
    let client = fake.client();
    let disk = "registry.example/cua-e2e-variants:docker-disk";
    cua_fleet::testing::set_image_variant(disk, ImageVariant::ContainerDisk);
    let e = client
        .apply_pool(&PoolSpec::new("cua-e2e-variants-a", disk).runtime(RuntimeKind::Gvisor))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("containerDisk"), "{e}");
    assert!(!fake.exists("pool", "cua-e2e-variants-a", "cua-e2e-variants-a"));
    // The matching runtime goes through, whatever the tag says.
    client
        .apply_pool(&PoolSpec::new("cua-e2e-variants-b", disk).runtime(RuntimeKind::Kubevirt))
        .await
        .unwrap();
    // Unreadable manifests: the explicit runtime is sent unchecked.
    client
        .apply_pool(
            &PoolSpec::new("cua-e2e-variants-c", "registry.example/unregistered:latest")
                .runtime(RuntimeKind::Gvisor),
        )
        .await
        .unwrap();
}

#[test]
fn fallback_vectors_pick_the_runtime_from_the_reference() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/image-variants.json");
    let doc: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let fallbacks = doc["fallbacks"].as_array().unwrap();
    assert!(fallbacks.len() >= 8);
    let off = ImageEvidence::Unavailable("offline".into());
    for f in fallbacks {
        let image = f["reference"].as_str().unwrap();
        let want = runtime(f["runtime"].as_str().unwrap());
        assert_eq!(cua_fleet::runtime_from_reference(image), want, "{image}");
        assert_eq!(check_runtime(None, image, &off).unwrap(), want, "{image}");
    }
}

#[tokio::test]
async fn unset_runtime_on_an_unreadable_manifest_falls_back_to_the_reference() {
    let _fake = FakeFleet::new();
    // No fixture registered: the manifest is unreadable, which is not an error.
    assert_eq!(
        cua_fleet::resolve_runtime(None, "registry.example/nobody:docker-latest")
            .await
            .unwrap(),
        RuntimeKind::Gvisor
    );
    assert_eq!(
        cua_fleet::resolve_runtime(None, "registry.example/nobody:latest")
            .await
            .unwrap(),
        RuntimeKind::Kubevirt
    );
}
