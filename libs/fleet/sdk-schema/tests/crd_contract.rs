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
        ]
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
fn check_crds_reports_modified_removed_added_reordered_and_duplicate_documents() {
    let documents = generated_documents();

    let mut modified = documents.clone();
    *modified[0].pointer_mut("/spec/group").unwrap() = json!("changed.example");
    assert_drift_reports(&modified, &["osgymsandboxes.osgym.cua.ai"]);

    let mut removed = documents.clone();
    removed.pop();
    assert_drift_reports(&removed, &["osgymsandboxclaims.osgym.cua.ai"]);

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
