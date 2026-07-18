//! Guard hand-maintained cua-driver guidance against resurrecting removed tools.

use std::path::{Path, PathBuf};

const FORBIDDEN: &[&str] = &[
    "cua-driver call screenshot",
    "cua-driver screenshot",
    "screenshot({",
    "changes only `screenshot`",
    "compatibility `screenshot`",
    "`screenshot` or the png",
    "tool named `screenshot`",
];

fn collect_docs(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_docs(&path, files);
        } else if matches!(path.extension().and_then(|ext| ext.to_str()), Some("md" | "mdx")) {
            files.push(path);
        }
    }
}

#[test]
fn maintained_driver_docs_do_not_invoke_removed_screenshot_tool() {
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

    let mut files = vec![driver_root.join("README.md"), rust_root.join("README.md")];
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
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
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
