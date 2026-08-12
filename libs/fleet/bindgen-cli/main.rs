use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
    process,
};

use anyhow::{Context, Result, bail};
use serde_json::Value;

fn main() {
    let command = env::args().nth(1);
    let result = match command.as_deref() {
        Some("resolve-cdylib") => resolve_cdylib_command(),
        Some("resolve-build-messages") => resolve_build_messages_command(),
        _ => {
            uniffi::uniffi_bindgen_main();
            return;
        }
    };

    if let Err(error) = result {
        eprintln!("error: {error:#}");
        process::exit(1);
    }
}

fn resolve_cdylib_command() -> Result<()> {
    let arguments = ResolverArguments::parse(true)?;
    let sdk_manifest = fs::canonicalize(&arguments.sdk_manifest).with_context(|| {
        format!(
            "could not canonicalize SDK manifest {}",
            arguments.sdk_manifest.display()
        )
    })?;
    let metadata = fs::read_to_string(&arguments.metadata).with_context(|| {
        format!(
            "could not read Cargo metadata {}",
            arguments.metadata.display()
        )
    })?;
    let build_messages = fs::read_to_string(&arguments.build_messages).with_context(|| {
        format!(
            "could not read Cargo build messages {}",
            arguments.build_messages.display()
        )
    })?;

    let package_id = resolve_package_id(&metadata, &sdk_manifest)?;
    let library = resolve_cdylib_from_messages(&build_messages, &package_id)?;
    println!("{}", library.display());
    Ok(())
}

fn resolve_build_messages_command() -> Result<()> {
    let arguments = ResolverArguments::parse(false)?;
    let package_id = arguments
        .package_id
        .as_deref()
        .expect("required by ResolverArguments::parse");
    let build_messages = fs::read_to_string(&arguments.build_messages).with_context(|| {
        format!(
            "could not read Cargo build messages {}",
            arguments.build_messages.display()
        )
    })?;

    let library = resolve_cdylib_from_messages(&build_messages, package_id)?;
    println!("{}", library.display());
    Ok(())
}

struct ResolverArguments {
    sdk_manifest: PathBuf,
    metadata: PathBuf,
    build_messages: PathBuf,
    package_id: Option<String>,
}

impl ResolverArguments {
    fn parse(require_metadata: bool) -> Result<Self> {
        let mut sdk_manifest = None;
        let mut metadata = None;
        let mut build_messages = None;
        let mut package_id = None;
        let mut arguments = env::args().skip(2);

        while let Some(argument) = arguments.next() {
            let value = arguments
                .next()
                .with_context(|| format!("missing value for {argument}"))?;
            match argument.as_str() {
                "--sdk-manifest" => sdk_manifest = Some(PathBuf::from(value)),
                "--metadata" => metadata = Some(PathBuf::from(value)),
                "--build-messages" => build_messages = Some(PathBuf::from(value)),
                "--package-id" => package_id = Some(value),
                _ => bail!("unknown resolver option {argument}"),
            }
        }

        let build_messages = build_messages.context("missing required --build-messages")?;
        if require_metadata {
            Ok(Self {
                sdk_manifest: sdk_manifest.context("missing required --sdk-manifest")?,
                metadata: metadata.context("missing required --metadata")?,
                build_messages,
                package_id,
            })
        } else {
            Ok(Self {
                sdk_manifest: PathBuf::new(),
                metadata: PathBuf::new(),
                build_messages,
                package_id: Some(package_id.context("missing required --package-id")?),
            })
        }
    }
}

fn resolve_package_id(metadata_json: &str, sdk_manifest: &Path) -> Result<String> {
    let metadata: Value =
        serde_json::from_str(metadata_json).context("could not parse Cargo metadata JSON")?;
    let workspace_members = metadata
        .get("workspace_members")
        .and_then(Value::as_array)
        .context("Cargo metadata did not contain workspace_members")?
        .iter()
        .filter_map(Value::as_str)
        .collect::<HashSet<_>>();
    let packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .context("Cargo metadata did not contain packages")?;

    let canonical_packages = packages
        .iter()
        .filter_map(|package| {
            let id = package.get("id")?.as_str()?;
            let manifest_path = package.get("manifest_path")?.as_str()?;
            (Path::new(manifest_path) == sdk_manifest).then_some(id)
        })
        .collect::<Vec<_>>();
    if canonical_packages.is_empty() {
        bail!(
            "Cargo metadata did not contain the canonical SDK package manifest {}",
            sdk_manifest.display()
        );
    }

    let workspace_candidates = canonical_packages
        .iter()
        .copied()
        .filter(|package_id| workspace_members.contains(package_id))
        .collect::<Vec<_>>();
    match workspace_candidates.as_slice() {
        [package_id] => Ok((*package_id).to_owned()),
        [] => bail!(
            "Cargo package with canonical SDK manifest {} is not a workspace member",
            sdk_manifest.display()
        ),
        _ => bail!(
            "Cargo metadata identified multiple workspace packages with canonical SDK manifest {}",
            sdk_manifest.display()
        ),
    }
}

fn resolve_cdylib_from_messages(messages_jsonl: &str, package_id: &str) -> Result<PathBuf> {
    let artifacts = messages_jsonl
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(line_number, line)| {
            serde_json::from_str::<Value>(line).with_context(|| {
                format!(
                    "could not parse Cargo build JSON at line {}",
                    line_number + 1
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let matching_artifacts = artifacts
        .iter()
        .filter(|artifact| is_matching_cdylib_artifact(artifact, package_id))
        .collect::<Vec<_>>();

    let artifact = match matching_artifacts.as_slice() {
        [] => bail!("no matching compiler-artifact record for cyclops-sdk package {package_id}"),
        [artifact] => *artifact,
        _ => bail!(
            "multiple matching compiler-artifact records for cyclops-sdk package {package_id}"
        ),
    };
    let filenames = artifact
        .get("filenames")
        .and_then(Value::as_array)
        .context("matching compiler-artifact record did not contain filenames")?;
    let libraries = filenames
        .iter()
        .filter_map(Value::as_str)
        .filter(|filename| is_supported_cdylib_filename(filename))
        .collect::<Vec<_>>();

    match libraries.as_slice() {
        [] => bail!(
            "matching compiler-artifact record has no host library filename (.so, .dylib, or .dll)"
        ),
        [library] => Ok(PathBuf::from(library)),
        _ => bail!(
            "matching compiler-artifact record has multiple host library filenames (.so, .dylib, or .dll)"
        ),
    }
}

fn is_matching_cdylib_artifact(artifact: &Value, package_id: &str) -> bool {
    artifact.get("reason").and_then(Value::as_str) == Some("compiler-artifact")
        && artifact.get("package_id").and_then(Value::as_str) == Some(package_id)
        && artifact
            .get("target")
            .and_then(|target| target.get("name"))
            .and_then(Value::as_str)
            == Some("cyclops_sdk")
        && artifact
            .get("target")
            .and_then(|target| target.get("crate_types"))
            .and_then(Value::as_array)
            .is_some_and(|crate_types| {
                crate_types
                    .iter()
                    .any(|kind| kind.as_str() == Some("cdylib"))
            })
}

fn is_supported_cdylib_filename(filename: &str) -> bool {
    matches!(
        Path::new(filename)
            .extension()
            .and_then(|extension| extension.to_str()),
        Some("so" | "dylib" | "dll")
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{resolve_cdylib_from_messages, resolve_package_id};

    const SDK_MANIFEST: &str = "/workspace/cyclops-cs/sdk/Cargo.toml";

    #[test]
    fn requires_the_canonical_manifest_to_be_a_workspace_member() {
        let exact_member = include_str!("tests/fixtures/metadata-exact-workspace-member.json");
        assert_eq!(
            resolve_package_id(exact_member, Path::new(SDK_MANIFEST)).unwrap(),
            "exact 0.1.0"
        );

        let canonical_absent = include_str!("tests/fixtures/metadata-canonical-absent.json");
        let error = resolve_package_id(canonical_absent, Path::new(SDK_MANIFEST))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("did not contain the canonical SDK package"),
            "{error}"
        );

        let canonical_not_member =
            include_str!("tests/fixtures/metadata-canonical-not-member.json");
        let error = resolve_package_id(canonical_not_member, Path::new(SDK_MANIFEST))
            .unwrap_err()
            .to_string();
        assert!(error.contains("not a workspace member"), "{error}");

        let ambiguous = include_str!("tests/fixtures/metadata-ambiguous-canonical-members.json");
        let error = resolve_package_id(ambiguous, Path::new(SDK_MANIFEST))
            .unwrap_err()
            .to_string();
        assert!(error.contains("multiple workspace packages"), "{error}");
    }

    #[test]
    fn resolves_the_exact_package_after_unrelated_artifacts_and_diagnostics() {
        let metadata = r#"{
          "workspace_members": ["wrong 0.1.0", "exact 0.1.0"],
          "packages": [
            {"id":"wrong 0.1.0","name":"cyclops-sdk","manifest_path":"/workspace/other/Cargo.toml"},
            {"manifest_path":"/workspace/cyclops-cs/sdk/Cargo.toml","name":"cyclops-sdk","id":"exact 0.1.0"}
          ]
        }"#;
        let messages = r#"
{"reason":"compiler-message","package_id":"dependency 0.1.0"}
{"target":{"name":"cyclops_sdk","crate_types":["cdylib"]},"filenames":["/tmp/wrong/libcyclops_sdk.so"],"package_id":"wrong 0.1.0","reason":"compiler-artifact"}
{"reason":"build-script-executed","package_id":"dependency 0.1.0"}
{"filenames":["/tmp/exact/libcyclops_sdk.rlib","/tmp/exact/libcyclops_sdk\u002eso"],"reason":"compiler-artifact","package_id":"exact 0.1.0","target":{"crate_types":["lib","cdylib"],"name":"cyclops_sdk"}}
"#;

        let package_id = resolve_package_id(metadata, Path::new(SDK_MANIFEST)).unwrap();
        let library = resolve_cdylib_from_messages(messages, &package_id).unwrap();

        assert_eq!(package_id, "exact 0.1.0");
        assert_eq!(library, Path::new("/tmp/exact/libcyclops_sdk.so"));
    }

    #[test]
    fn rejects_ambiguous_artifacts_and_library_filenames() {
        let multiple_artifacts = r#"
{"reason":"compiler-artifact","package_id":"exact","target":{"name":"cyclops_sdk","crate_types":["cdylib"]},"filenames":["/tmp/one/libcyclops_sdk.so"]}
{"reason":"compiler-artifact","package_id":"exact","target":{"name":"cyclops_sdk","crate_types":["cdylib"]},"filenames":["/tmp/two/libcyclops_sdk.so"]}
"#;
        assert!(
            resolve_cdylib_from_messages(multiple_artifacts, "exact")
                .unwrap_err()
                .to_string()
                .contains("multiple matching compiler-artifact records")
        );

        let multiple_filenames = r#"
{"reason":"compiler-artifact","package_id":"exact","target":{"name":"cyclops_sdk","crate_types":["cdylib"]},"filenames":["/tmp/exact/libcyclops_sdk.so","/tmp/exact/cyclops_sdk.dll"]}
"#;
        assert!(
            resolve_cdylib_from_messages(multiple_filenames, "exact")
                .unwrap_err()
                .to_string()
                .contains("multiple host library filenames")
        );
    }

    #[test]
    fn rejects_missing_artifacts_and_accepts_cross_abi_extensions() {
        let no_match = r#"{"reason":"compiler-message","package_id":"exact"}"#;
        assert!(
            resolve_cdylib_from_messages(no_match, "exact")
                .unwrap_err()
                .to_string()
                .contains("no matching compiler-artifact record")
        );

        let windows_cdylib = r#"{"reason":"compiler-artifact","package_id":"exact","target":{"name":"cyclops_sdk","crate_types":["cdylib"]},"filenames":["C:\\target\\release\\cyclops_sdk.dll"]}"#;
        assert_eq!(
            resolve_cdylib_from_messages(windows_cdylib, "exact").unwrap(),
            Path::new(r"C:\target\release\cyclops_sdk.dll")
        );
    }
}
