use cyclops_sdk_schema::generate::{render_crds, semantic_yaml_documents};
use serde_json::json;

const AUTHORITATIVE_CRDS: &str = include_str!("../../../clusters/base/osgym/crd.yaml");

#[test]
fn generated_bundle_matches_checked_in_contract() {
    let generated = semantic_yaml_documents(&render_crds().unwrap()).unwrap();
    let checked_in = semantic_yaml_documents(AUTHORITATIVE_CRDS).unwrap();

    assert_eq!(generated, checked_in);
}

#[test]
fn generated_bundle_has_stable_resource_order() {
    let documents = semantic_yaml_documents(&render_crds().unwrap()).unwrap();
    let names = documents
        .iter()
        .map(|value| value.pointer("/metadata/name").unwrap().as_str().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(
        names,
        vec![
            "osgymsandboxes.osgym.cua.ai",
            "osgymsandboxtemplates.osgym.cua.ai",
            "osgymsandboxwarmpools.osgym.cua.ai",
            "osgymsandboxclaims.osgym.cua.ai",
            "osgymworkspacepools.cua.ai",
        ]
    );
}

#[test]
fn workspace_pool_preserves_fields_constraints_and_kubernetes_extensions() {
    let workspace_pool = semantic_yaml_documents(&render_crds().unwrap())
        .unwrap()
        .into_iter()
        .find(|document| {
            document.pointer("/metadata/name") == Some(&json!("osgymworkspacepools.cua.ai"))
        })
        .unwrap();
    let schema = "/spec/versions/0/schema/openAPIV3Schema";

    assert_eq!(
        workspace_pool.pointer(&format!("{schema}/properties/spec/required")),
        Some(&json!(["replicas", "template"]))
    );
    assert_eq!(
        workspace_pool.pointer(&format!(
            "{schema}/properties/spec/properties/replicas/minimum"
        )),
        Some(&json!(0.0))
    );
    assert_eq!(
        workspace_pool.pointer(&format!(
            "{schema}/properties/spec/properties/template/required"
        )),
        Some(&json!(["containerDiskImage"]))
    );
    assert_eq!(
        workspace_pool.pointer(&format!("{schema}/properties/spec/properties/template/properties/tolerations/items/x-kubernetes-preserve-unknown-fields")),
        Some(&json!(true))
    );
    assert_eq!(
        workspace_pool.pointer(&format!("{schema}/properties/spec/properties/template/properties/probes/x-kubernetes-preserve-unknown-fields")),
        Some(&json!(true))
    );
    assert_eq!(
        workspace_pool.pointer(&format!(
            "{schema}/properties/spec/properties/autoscaling/properties/minPoolSize/default"
        )),
        Some(&json!(0))
    );
    assert_eq!(
        workspace_pool.pointer(&format!(
            "{schema}/properties/spec/properties/autoscaling/properties/maxPoolSize/minimum"
        )),
        Some(&json!(1.0))
    );
    assert_eq!(
        workspace_pool.pointer(&format!(
            "{schema}/properties/spec/properties/services/items/properties/targetPort/maximum"
        )),
        Some(&json!(65535.0))
    );
    assert_eq!(
        workspace_pool.pointer("/spec/versions/0/subresources/status"),
        Some(&json!({}))
    );
    assert_eq!(
        workspace_pool.pointer("/spec/versions/0/additionalPrinterColumns"),
        Some(&json!([
            {"name": "Replicas", "type": "integer", "jsonPath": ".spec.replicas"},
            {"name": "Available", "type": "integer", "jsonPath": ".status.availableCount"},
            {"name": "Phase", "type": "string", "jsonPath": ".status.phase"},
            {"name": "Age", "type": "date", "jsonPath": ".metadata.creationTimestamp"},
        ]))
    );
}

fn generated_documents() -> Vec<serde_json::Value> {
    semantic_yaml_documents(&render_crds().unwrap()).unwrap()
}

fn write_documents(path: &std::path::Path, documents: &[serde_json::Value]) {
    let yaml = documents
        .iter()
        .map(serde_yaml::to_string)
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join("---\n");
    std::fs::write(path, yaml).unwrap();
}

fn assert_drift_reports(documents: &[serde_json::Value], expected_names: &[&str]) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("crd.yaml");
    write_documents(&path, documents);

    let error = cyclops_sdk_schema::generate::check_crds(&path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("CRD bundle drift detected"));
    for name in expected_names {
        assert!(error.contains(name), "missing {name} from {error}");
    }
}

#[test]
fn workspace_pool_schema_allows_only_historical_template_paths() {
    let documents = generated_documents();
    let workspace_pool = documents
        .iter()
        .find(|document| {
            document.pointer("/metadata/name") == Some(&json!("osgymworkspacepools.cua.ai"))
        })
        .unwrap();
    let schema = "/spec/versions/0/schema/openAPIV3Schema/properties";
    let spec = workspace_pool
        .pointer(&format!("{schema}/spec"))
        .and_then(serde_json::Value::as_object)
        .unwrap();
    let template = workspace_pool
        .pointer(&format!("{schema}/spec/properties/template"))
        .and_then(serde_json::Value::as_object)
        .unwrap();

    assert_eq!(spec.get("required"), Some(&json!(["replicas", "template"])));
    assert_eq!(
        spec.get("properties")
            .and_then(serde_json::Value::as_object)
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["autoscaling", "replicas", "services", "template",])
    );
    assert_eq!(
        template.get("required"),
        Some(&json!(["containerDiskImage"]))
    );
    assert_eq!(
        template
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([
            "command",
            "containerDiskImage",
            "cpuCores",
            "firmware",
            "imagePullSecret",
            "memory",
            "nodeSelector",
            "oidc",
            "probes",
            "runtime",
            "runtimeClassName",
            "tolerations",
        ])
    );
    assert_eq!(
        workspace_pool.pointer(&format!(
            "{schema}/spec/properties/template/properties/imagePullPolicy"
        )),
        None
    );
    assert_eq!(
        workspace_pool.pointer(&format!(
            "{schema}/spec/properties/template/properties/services"
        )),
        None
    );
}

#[test]
fn check_crds_reports_modified_removed_added_reordered_and_duplicate_documents() {
    let documents = generated_documents();

    let mut modified = documents.clone();
    *modified[0].pointer_mut("/spec/group").unwrap() = json!("changed.example");
    assert_drift_reports(&modified, &["osgymsandboxes.osgym.cua.ai"]);

    let mut removed = documents.clone();
    removed.pop();
    assert_drift_reports(&removed, &["osgymworkspacepools.cua.ai"]);

    let mut added = documents.clone();
    let mut additional = documents[0].clone();
    *additional.pointer_mut("/metadata/name").unwrap() = json!("additional.cua.ai");
    added.push(additional);
    assert_drift_reports(&added, &["additional.cua.ai"]);

    let mut reordered = documents.clone();
    reordered.swap(0, 1);
    assert_drift_reports(
        &reordered,
        &[
            "osgymsandboxes.osgym.cua.ai",
            "osgymsandboxtemplates.osgym.cua.ai",
        ],
    );

    let mut duplicated = documents;
    duplicated.push(duplicated[0].clone());
    assert_drift_reports(&duplicated, &["osgymsandboxes.osgym.cua.ai"]);
}

#[test]
fn write_crds_creates_parent_directories_and_contextualizes_failures() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("nested/generated/crd.yaml");
    cyclops_sdk_schema::generate::write_crds(&output).unwrap();
    assert_eq!(
        semantic_yaml_documents(&std::fs::read_to_string(&output).unwrap()).unwrap(),
        generated_documents()
    );

    let blocked_parent = directory.path().join("not-a-directory");
    std::fs::write(&blocked_parent, "blocked").unwrap();
    let error = cyclops_sdk_schema::generate::write_crds(blocked_parent.join("crd.yaml"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("failed to create CRD output directory"));
    assert!(error.contains(&blocked_parent.display().to_string()));

    let directory_target = directory.path().join("directory-target");
    std::fs::create_dir(&directory_target).unwrap();
    let error = cyclops_sdk_schema::generate::write_crds(&directory_target)
        .unwrap_err()
        .to_string();
    assert!(error.contains("failed to write CRD bundle"));
    assert!(error.contains(&directory_target.display().to_string()));
}
