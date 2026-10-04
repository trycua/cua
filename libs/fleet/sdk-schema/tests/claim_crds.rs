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

#[test]
fn warm_pool_idle_ttl_and_ttl_policy_are_optional_and_bounded() {
    let crd = serde_json::to_value(OSGymSandboxWarmPool::crd()).unwrap();
    let spec = crd
        .pointer("/spec/versions/0/schema/openAPIV3Schema/properties/spec")
        .unwrap();
    let required = spec.pointer("/required").and_then(Value::as_array).unwrap();
    for field in ["idleTtlSeconds", "ttlPolicy"] {
        assert!(
            !required.contains(&json!(field)),
            "{field} must be optional"
        );
    }

    let mut idle = spec.pointer("/properties/idleTtlSeconds").cloned().unwrap();
    idle.as_object_mut().unwrap().remove("description");
    normalize_numbers(&mut idle);
    assert_eq!(
        idle,
        json!({"type": "integer", "minimum": 0, "maximum": 4_294_967_295u64})
    );

    let mut policy = spec.pointer("/properties/ttlPolicy").cloned().unwrap();
    policy.as_object_mut().unwrap().remove("description");
    // No materialized default: absent keeps today's Retain behaviour without
    // the apiserver rewriting every stored pool.
    assert_eq!(
        policy,
        json!({"type": "string", "enum": ["Retain", "Cascade"]})
    );

    let status = crd
        .pointer("/spec/versions/0/schema/openAPIV3Schema/properties/status/properties")
        .unwrap();
    for field in ["lastClaimedAt", "lastActivityTime"] {
        let mut schema = status.pointer(&format!("/{field}")).cloned().unwrap();
        schema.as_object_mut().unwrap().remove("description");
        assert_eq!(schema, json!({"type": "string", "format": "date-time"}));
    }
}

#[test]
fn warm_pool_lifecycle_fields_round_trip_and_stay_absent_when_unset() {
    use cyclops_sdk_schema::{
        OSGymSandboxWarmPoolSpec, OSGymSandboxWarmPoolStatus, WarmPoolTtlPolicy,
    };

    let spec: OSGymSandboxWarmPoolSpec = serde_json::from_value(json!({
        "replicas": 0,
        "sandboxTemplateRef": {"name": "p-template"},
        "idleTtlSeconds": 900,
        "ttlPolicy": "Cascade",
    }))
    .unwrap();
    assert_eq!(spec.idle_ttl_seconds, Some(900));
    assert_eq!(spec.ttl_policy, Some(WarmPoolTtlPolicy::Cascade));
    assert_eq!(serde_json::to_value(&spec).unwrap()["ttlPolicy"], "Cascade");

    let legacy: OSGymSandboxWarmPoolSpec = serde_json::from_value(json!({
        "replicas": 1,
        "sandboxTemplateRef": {"name": "p-template"},
    }))
    .unwrap();
    let legacy_json = serde_json::to_value(&legacy).unwrap();
    assert!(legacy_json.get("idleTtlSeconds").is_none());
    assert!(legacy_json.get("ttlPolicy").is_none());

    assert!(
        serde_json::from_value::<OSGymSandboxWarmPoolSpec>(json!({
            "replicas": 1,
            "sandboxTemplateRef": {"name": "p-template"},
            "ttlPolicy": "Orphan",
        }))
        .is_err()
    );

    let status: OSGymSandboxWarmPoolStatus = serde_json::from_value(json!({
        "replicas": 1,
        "lastClaimedAt": "2026-09-22T10:00:00Z",
        "lastActivityTime": "2026-09-22T10:05:00Z",
    }))
    .unwrap();
    assert_eq!(
        status.last_claimed_at.as_deref(),
        Some("2026-09-22T10:00:00Z")
    );
    assert_eq!(
        status.last_activity_time.as_deref(),
        Some("2026-09-22T10:05:00Z")
    );
}

#[test]
fn claim_secret_ref_is_optional_and_pinned_to_the_gateway_prefix() {
    let claim = serde_json::to_value(OSGymSandboxClaim::crd()).unwrap();
    let spec = claim
        .pointer("/spec/versions/0/schema/openAPIV3Schema/properties/spec")
        .unwrap();
    let required = spec.get("required").and_then(Value::as_array).unwrap();
    assert!(!required.contains(&json!("secretRef")));

    let name = spec
        .pointer("/properties/secretRef/properties/name")
        .unwrap();
    let pattern = name["pattern"].as_str().unwrap();
    assert!(pattern.starts_with(&format!(
        "^{}",
        cyclops_sdk_schema::CLAIM_SECRET_NAME_PREFIX
    )));
    assert_eq!(name["maxLength"], json!(253));
    assert_eq!(
        spec.pointer("/properties/secretRef/required"),
        Some(&json!(["name"]))
    );
}

#[test]
fn claim_spec_round_trips_secret_ref_and_omits_it_when_unset() {
    use cyclops_sdk_schema::{ClaimSecretRef, ClaimSpec};

    let spec: ClaimSpec = serde_json::from_value(json!({
        "sandboxTemplateRef": { "name": "pool-template" },
        "secretRef": { "name": "cua-claim-claim-a" },
    }))
    .unwrap();
    assert_eq!(
        spec.secret_ref,
        Some(ClaimSecretRef {
            name: "cua-claim-claim-a".into()
        })
    );
    assert_eq!(
        serde_json::to_value(&spec).unwrap()["secretRef"],
        json!({ "name": "cua-claim-claim-a" })
    );

    let bare: ClaimSpec = serde_json::from_value(json!({
        "sandboxTemplateRef": { "name": "pool-template" },
    }))
    .unwrap();
    assert!(bare.secret_ref.is_none());
    assert!(
        serde_json::to_value(&bare)
            .unwrap()
            .get("secretRef")
            .is_none()
    );
}
