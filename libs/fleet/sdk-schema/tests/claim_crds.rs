use cyclops_sdk_schema::{OSGymSandboxClaim, OSGymSandboxWarmPool};
use kube::CustomResourceExt;
use serde::Deserialize;
use serde_json::{Value, json};

const AUTHORITATIVE_CRDS: &str = include_str!("../../../clusters/base/osgym/crd.yaml");

fn authoritative_crd(name: &str) -> Value {
    serde_yaml::Deserializer::from_str(AUTHORITATIVE_CRDS)
        .map(|document| Value::deserialize(document).unwrap())
        .find(|document| document.pointer("/metadata/name") == Some(&json!(name)))
        .unwrap()
}

fn normalize_numbers(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(normalize_numbers),
        Value::Object(values) => values.values_mut().for_each(normalize_numbers),
        Value::Number(number) => {
            if let Some(number) = number.as_f64().filter(|number| number.fract() == 0.0) {
                *value = json!(number as i64);
            }
        }
        _ => {}
    }
}

fn normalize_known_kube_derive_artifacts(mut value: Value) -> Value {
    let names = value
        .pointer_mut("/spec/names")
        .unwrap()
        .as_object_mut()
        .unwrap();
    if names.get("categories") == Some(&json!([])) {
        names.remove("categories");
    }

    let schema = value
        .pointer_mut("/spec/versions/0/schema/openAPIV3Schema")
        .unwrap()
        .as_object_mut()
        .unwrap();
    schema.remove("title");
    if schema.get("required") == Some(&json!(["spec"])) {
        schema.remove("required");
    }
    let properties = schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .unwrap();
    if let Some(status) = properties.get_mut("status").and_then(Value::as_object_mut) {
        status.remove("nullable");
    }
    if let Some(autoscaling) = properties
        .get_mut("spec")
        .and_then(Value::as_object_mut)
        .and_then(|spec| spec.get_mut("properties"))
        .and_then(Value::as_object_mut)
        .and_then(|spec| spec.get_mut("autoscaling"))
        .and_then(Value::as_object_mut)
    {
        autoscaling.remove("nullable");
    }

    normalize_numbers(&mut value);
    value
}

fn ttl_schema(crd: Value) -> Value {
    let mut schema = crd
        .pointer(
            "/spec/versions/0/schema/openAPIV3Schema/properties/spec/properties/ttlSecondsAfterCreated",
        )
        .cloned()
        .unwrap();
    schema.as_object_mut().unwrap().remove("description");
    normalize_numbers(&mut schema);
    schema
}

#[test]
fn pool_and_claim_ttl_are_optional_bounded_u32_integers() {
    for crd in [
        serde_json::to_value(OSGymSandboxWarmPool::crd()).unwrap(),
        serde_json::to_value(OSGymSandboxClaim::crd()).unwrap(),
    ] {
        let required = crd
            .pointer("/spec/versions/0/schema/openAPIV3Schema/properties/spec/required")
            .and_then(Value::as_array)
            .unwrap();
        assert!(
            !required.contains(&json!("ttlSecondsAfterCreated")),
            "ttlSecondsAfterCreated must be optional",
        );
        assert_eq!(
            ttl_schema(crd),
            json!({"type": "integer", "minimum": 0, "maximum": 4_294_967_295u64}),
        );
    }
}

#[test]
fn raw_kube_derive_output_has_intentional_warm_pool_and_claim_shape_differences() {
    let warm_pool = serde_json::to_value(OSGymSandboxWarmPool::crd()).unwrap();
    let claim = serde_json::to_value(OSGymSandboxClaim::crd()).unwrap();

    for value in [&warm_pool, &claim] {
        assert_eq!(
            value.pointer("/spec/versions/0/schema/openAPIV3Schema/required"),
            Some(&json!(["spec"]))
        );
        assert!(
            value
                .pointer("/spec/versions/0/schema/openAPIV3Schema/title")
                .is_some()
        );
        assert_eq!(value.pointer("/spec/names/categories"), Some(&json!([])));
        assert_eq!(
            value.pointer("/spec/versions/0/schema/openAPIV3Schema/properties/status/nullable"),
            Some(&json!(true))
        );
    }
    assert_eq!(
        warm_pool.pointer(
            "/spec/versions/0/schema/openAPIV3Schema/properties/spec/properties/autoscaling/nullable"
        ),
        Some(&json!(true))
    );
}

#[test]
fn warm_pool_raw_crd_matches_the_authoritative_field_contract() {
    let generated = normalize_known_kube_derive_artifacts(
        serde_json::to_value(OSGymSandboxWarmPool::crd()).unwrap(),
    );
    let authoritative = normalize_known_kube_derive_artifacts(authoritative_crd(
        "osgymsandboxwarmpools.osgym.cua.ai",
    ));

    assert_eq!(generated, authoritative);
}

#[test]
fn claim_bind_deadline_schema_documents_900_second_default_without_materializing_it() {
    let claim = serde_json::to_value(OSGymSandboxClaim::crd()).unwrap();
    let bind_deadline = claim
        .pointer("/spec/versions/0/schema/openAPIV3Schema/properties/spec/properties/bindDeadline")
        .unwrap();

    assert!(bind_deadline.get("default").is_none());
    assert!(
        bind_deadline["description"]
            .as_str()
            .unwrap()
            .contains("default 900")
    );
}

#[test]
fn claim_raw_crd_matches_the_authoritative_field_contract() {
    let generated = normalize_known_kube_derive_artifacts(
        serde_json::to_value(OSGymSandboxClaim::crd()).unwrap(),
    );
    let authoritative =
        normalize_known_kube_derive_artifacts(authoritative_crd("osgymsandboxclaims.osgym.cua.ai"));

    assert_eq!(generated, authoritative);
}
