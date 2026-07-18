//! Keep removed tools out of the live registry and maintained user guidance.

use std::path::{Path, PathBuf};
use std::process::Command;

use cua_driver_testkit::driver_binary;

const REMOVED_TOOLS: &[&str] = &["screenshot"];
const GUIDANCE_EXTENSIONS: &[&str] = &["md", "mdx"];

fn collect_guidance_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let mut entries = std::fs::read_dir(dir)
        .unwrap_or_else(|error| {
            panic!(
                "failed to read guidance directory {}: {error}",
                dir.display()
            )
        })
        .map(|entry| {
            entry
                .expect("failed to read guidance directory entry")
                .path()
        })
        .collect::<Vec<_>>();
    entries.sort();

    for path in entries {
        if path.is_dir() {
            collect_guidance_files(&path, files);
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| GUIDANCE_EXTENSIONS.contains(&extension))
        {
            files.push(path);
        }
    }
}

fn forbidden_guidance_patterns(tool: &str) -> Vec<String> {
    [
        format!("cua-driver call {tool}"),
        format!("cua-driver {tool}"),
        format!("{tool}({{"),
        format!("changes only `{tool}`"),
        format!("changes only the {tool} tool"),
        format!("compatibility `{tool}`"),
        format!("`{tool}` or the png"),
        format!("tool named `{tool}`"),
        format!("gui tools (click, type_text, {tool},"),
    ]
    .into_iter()
    .collect()
}

#[test]
fn removed_tools_are_absent_from_registry_and_guidance() {
    let rust_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("rust workspace root")
        .to_path_buf();
    let driver_root = rust_root.parent().expect("cua-driver root");
    let repo_root = driver_root
        .parent()
        .and_then(Path::parent)
        .expect("repository root");

    let output = Command::new(driver_binary())
        .args(["dump-docs", "--type", "mcp"])
        .env("CUA_DRIVER_EMBEDDED", "1")
        .output()
        .expect("run cua-driver dump-docs");
    assert!(
        output.status.success(),
        "cua-driver dump-docs failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let registry_docs: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse cua-driver dump-docs output");
    let registered = registry_docs["tools"]
        .as_array()
        .expect("dump-docs tools array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    for removed in REMOVED_TOOLS {
        assert!(
            !registered.contains(removed),
            "removed tool {removed:?} is still present in the live registry"
        );
    }

    let mut files = vec![
        driver_root.join("README.md"),
        rust_root.join("README.md"),
        driver_root.join("scripts/post-install-hints.txt"),
        driver_root.join("scripts/install.ps1"),
    ];
    collect_guidance_files(&driver_root.join("docs"), &mut files);
    collect_guidance_files(&rust_root.join("Skills/cua-driver"), &mut files);
    collect_guidance_files(
        &repo_root.join("docs/content/docs/reference/cua-driver"),
        &mut files,
    );
    collect_guidance_files(
        &repo_root.join("docs/content/docs/how-to-guides/driver"),
        &mut files,
    );

    let mut failures = Vec::new();
    for path in files {
        let contents = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("failed to read guidance file {}: {error}", path.display())
        });
        let lower = contents.to_ascii_lowercase();
        for removed in REMOVED_TOOLS {
            for pattern in forbidden_guidance_patterns(removed) {
                if lower.contains(&pattern) {
                    failures.push(format!("{} contains {pattern:?}", path.display()));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "removed tools appear in maintained guidance:\n{}",
        failures.join("\n")
    );
}
