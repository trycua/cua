//! Guard current user guidance against resurrecting the removed screenshot tool.

use std::path::{Path, PathBuf};

const FORBIDDEN: &[&str] = &[
    "cua-driver call screenshot",
    "cua-driver screenshot",
    "gui tools (click, type_text, screenshot,",
    "screenshot({",
    "changes only `screenshot`",
    "changes only the screenshot tool",
    "compatibility `screenshot`",
    "`screenshot` or the png",
    "tool named `screenshot`",
];

fn collect_docs(dir: &Path, files: &mut Vec<PathBuf>) {
    let mut entries = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("failed to read docs directory {}: {error}", dir.display()))
        .map(|entry| entry.expect("failed to read docs directory entry").path())
        .collect::<Vec<_>>();
    entries.sort();

    for path in entries {
        if path.is_dir() {
            collect_docs(&path, files);
        } else if matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("md" | "mdx")
        ) {
            files.push(path);
        }
    }
}

#[test]
fn maintained_guidance_does_not_advertise_removed_screenshot_tool() {
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

    // Keep this list limited to current user guidance. Historical plans may
    // accurately discuss the old tool and are not instructions to users.
    let mut files = vec![
        driver_root.join("README.md"),
        driver_root.join("docs/tool-output-format.md"),
        rust_root.join("README.md"),
        driver_root.join("scripts/post-install-hints.txt"),
        driver_root.join("scripts/install.ps1"),
    ];
    collect_docs(&rust_root.join("Skills/cua-driver"), &mut files);
    collect_docs(
        &repo_root.join("docs/content/docs/reference/cua-driver"),
        &mut files,
    );
    collect_docs(
        &repo_root.join("docs/content/docs/how-to-guides/driver"),
        &mut files,
    );

    let mut failures = Vec::new();
    for path in files {
        let contents = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("failed to read guidance file {}: {error}", path.display())
        });
        let lower = contents.to_ascii_lowercase();
        for pattern in FORBIDDEN {
            if lower.contains(pattern) {
                failures.push(format!("{} contains {pattern:?}", path.display()));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "removed screenshot tool appears in maintained guidance:\n{}",
        failures.join("\n")
    );
}
