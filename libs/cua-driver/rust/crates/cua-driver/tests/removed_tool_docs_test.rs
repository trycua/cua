//! Keep removed tools out of the live registry and maintained user guidance.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

const REMOVED_TOOLS: &[&str] = &["screenshot"];

#[cfg(target_os = "macos")]
fn live_tool_names() -> HashSet<String> {
    platform_macos::register_tools()
        .tool_names()
        .map(str::to_owned)
        .collect()
}

#[cfg(target_os = "windows")]
fn live_tool_names() -> HashSet<String> {
    platform_windows::register_tools()
        .tool_names()
        .map(str::to_owned)
        .collect()
}

#[cfg(target_os = "linux")]
fn live_tool_names() -> HashSet<String> {
    platform_linux::register_tools()
        .tool_names()
        .map(str::to_owned)
        .collect()
}

fn collect_docs(dir: &Path, files: &mut Vec<PathBuf>) {
    // Nix tests the filtered Rust source, which intentionally excludes the
    // repository-level docs tree. Scan every maintained-doc root available in
    // the current source distribution instead of requiring the full checkout.
    if !dir.is_dir() {
        return;
    }

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

fn advertises_removed_tool(line: &str, tool: &str) -> bool {
    let line = line.to_ascii_lowercase();
    let invocation = [
        format!("cua-driver call {tool}"),
        format!("cua-driver {tool}"),
        format!("{tool}("),
    ]
    .iter()
    .any(|pattern| line.contains(pattern));

    let tool_reference = [
        format!("`{tool}` tool"),
        format!("{tool} tool"),
        format!("tool `{tool}`"),
        format!("tool named `{tool}`"),
    ]
    .iter()
    .any(|pattern| line.contains(pattern));
    let explicitly_absent = ["no standalone", "does not register", "removed", "obsolete"]
        .iter()
        .any(|marker| line.contains(marker));

    invocation || (tool_reference && !explicitly_absent)
}

#[test]
fn maintained_guidance_does_not_advertise_removed_tools() {
    let registered = live_tool_names();
    for tool in REMOVED_TOOLS {
        assert!(
            !registered.contains(*tool),
            "removed tool {tool:?} is still present in the live registry"
        );
    }

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
    // accurately discuss old tools and are not instructions to users.
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
    for path in files.into_iter().filter(|path| path.is_file()) {
        let contents = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("failed to read guidance file {}: {error}", path.display())
        });
        let mut line_number = 1;
        for paragraph in contents.split_inclusive("\n\n") {
            for tool in REMOVED_TOOLS {
                if advertises_removed_tool(paragraph, tool) {
                    failures.push(format!(
                        "{}:{} advertises removed tool {tool:?}: {}",
                        path.display(),
                        line_number,
                        paragraph.split_whitespace().collect::<Vec<_>>().join(" ")
                    ));
                }
            }
            line_number += paragraph.lines().count();
        }
    }

    assert!(
        failures.is_empty(),
        "removed tools appear in maintained guidance:\n{}",
        failures.join("\n")
    );
}

#[test]
fn removed_tool_reference_detection_covers_invocations_and_prose() {
    for reference in [
        "cua-driver call screenshot '{}'",
        "screenshot({ window_id: 42 })",
        "use the `screenshot` tool",
        "use the screenshot tool",
    ] {
        assert!(
            advertises_removed_tool(reference, "screenshot"),
            "{reference:?}"
        );
    }

    for notice in [
        "there is no standalone screenshot tool",
        "the `screenshot` tool was removed",
        "this does not register a standalone `screenshot` tool",
    ] {
        assert!(!advertises_removed_tool(notice, "screenshot"), "{notice:?}");
    }
}
