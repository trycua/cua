use camino::Utf8PathBuf;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use uniffi::{GenerateOptions, TargetLanguage};

#[test]
fn generated_bindings_hide_raw_credentials_and_claim_update() {
    build_sdk_cdylib();

    let output = temporary_output_directory();
    let library = Utf8PathBuf::from_path_buf(sdk_library_path()).unwrap();

    uniffi::generate(GenerateOptions {
        languages: vec![TargetLanguage::Python, TargetLanguage::Kotlin],
        source: library,
        out_dir: Utf8PathBuf::from_path_buf(output.clone()).unwrap(),
        config_override: None,
        format: false,
        crate_filter: Some("cyclops_sdk".into()),
        metadata_no_deps: false,
    })
    .unwrap();

    let python = generated_source(&output, "py");
    let kotlin = generated_source(&output, "kt");
    let python_configuration = record_block(
        &python,
        "class CyclopsConfiguration",
        "class _UniffiFfiConverterTypeCyclopsConfiguration",
    );
    let kotlin_configuration = record_block(
        &kotlin,
        "data class CyclopsConfiguration",
        "public object FfiConverterTypeCyclopsConfiguration",
    );

    assert!(python_configuration.contains("credentials"));
    assert!(!python_configuration.contains("client_secret"));
    assert!(!python_configuration.contains("clientSecret"));
    assert!(kotlin_configuration.contains("credentials"));
    assert!(!kotlin_configuration.contains("client_secret"));
    assert!(!kotlin_configuration.contains("clientSecret"));
    assert!(!python.contains("update_claim"));
    assert!(!python.contains("updateClaim"));
    assert!(!kotlin.contains("updateClaim"));
    assert!(!python.contains("PoolTimeout"));
    assert!(!python.contains("PoolFailed"));
    assert!(!kotlin.contains("PoolTimeout"));
    assert!(!kotlin.contains("PoolFailed"));

    fs::remove_dir_all(output).unwrap();
}

fn build_sdk_cdylib() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("Cargo.toml");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(manifest)
        .args(["-p", "cyclops-sdk"])
        .status()
        .unwrap();

    assert!(status.success(), "failed to build the cyclops-sdk cdylib");
}

fn sdk_library_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target")
        .join("debug")
        .join(format!("libcyclops_sdk{}", std::env::consts::DLL_SUFFIX))
}

fn temporary_output_directory() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "cyclops-sdk-bindings-{}-{timestamp}",
        std::process::id()
    ))
}

fn generated_source(directory: &Path, extension: &str) -> String {
    let mut stack = vec![directory.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|value| value == extension) {
                return fs::read_to_string(path).unwrap();
            }
        }
    }

    panic!("no generated .{extension} source found");
}

fn record_block<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start = source.find(start).unwrap();
    let source = &source[start..];
    &source[..source.find(end).unwrap()]
}
