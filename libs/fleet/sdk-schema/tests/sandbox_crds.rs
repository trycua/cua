use cyclops_sdk_schema::{OSGymSandbox, OSGymSandboxTemplate};
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

fn normalize_kube_derive_representation(mut value: Value) -> Value {
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
    if let Some(status) = schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .and_then(|properties| properties.get_mut("status"))
        .and_then(Value::as_object_mut)
    {
        status.remove("nullable");
    }

    let version = value
        .pointer_mut("/spec/versions/0")
        .unwrap()
        .as_object_mut()
        .unwrap();
    if version.get("subresources") == Some(&json!({})) {
        version.remove("subresources");
    }

    normalize_numbers(&mut value);
    value
}

#[test]
fn raw_kube_derive_output_has_intentional_first_slice_shape_differences() {
    let sandbox = serde_json::to_value(OSGymSandbox::crd()).unwrap();
    let template = serde_json::to_value(OSGymSandboxTemplate::crd()).unwrap();

    for value in [&sandbox, &template] {
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
    }
    assert_eq!(
        sandbox.pointer("/spec/versions/0/schema/openAPIV3Schema/properties/status/nullable"),
        Some(&json!(true))
    );
    assert_eq!(
        template.pointer("/spec/versions/0/subresources"),
        Some(&json!({}))
    );
}

#[test]
fn sandbox_raw_crd_matches_the_authoritative_field_contract() {
    let generated =
        normalize_kube_derive_representation(serde_json::to_value(OSGymSandbox::crd()).unwrap());
    let authoritative =
        normalize_kube_derive_representation(authoritative_crd("osgymsandboxes.osgym.cua.ai"));

    assert_eq!(generated, authoritative);
}

#[test]
fn sandbox_template_raw_crd_matches_the_authoritative_field_contract() {
    let generated = normalize_kube_derive_representation(
        serde_json::to_value(OSGymSandboxTemplate::crd()).unwrap(),
    );
    let authoritative = normalize_kube_derive_representation(authoritative_crd(
        "osgymsandboxtemplates.osgym.cua.ai",
    ));

    assert_eq!(generated, authoritative);
}

#[test]
fn sandbox_crd_preserves_status_and_service_constraints() {
    let value = serde_json::to_value(OSGymSandbox::crd()).unwrap();

    assert_eq!(
        value.pointer("/spec/versions/0/subresources/status"),
        Some(&json!({}))
    );
    assert_eq!(
        value.pointer("/spec/versions/0/schema/openAPIV3Schema/properties/spec/properties/vmTemplate/properties/services/items/properties/targetPort/minimum"),
        Some(&json!(1.0))
    );
    assert_eq!(
        value.pointer("/spec/versions/0/schema/openAPIV3Schema/properties/spec/properties/vmTemplate/properties/services/items/properties/targetPort/maximum"),
        Some(&json!(65535.0))
    );
}

#[test]
fn sandbox_template_has_no_status_subresource() {
    let value = serde_json::to_value(OSGymSandboxTemplate::crd()).unwrap();

    assert_eq!(value.pointer("/spec/versions/0/subresources/status"), None);
}
