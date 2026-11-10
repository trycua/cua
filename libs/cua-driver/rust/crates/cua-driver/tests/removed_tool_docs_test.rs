// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

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

fn advertises_removed_tool(text: &str, tool: &str, live_tools: &HashSet<String>) -> bool {
    let text = text.to_ascii_lowercase();
    let invocation = [
        format!("cua-driver call {tool}"),
        format!("cua-driver {tool}"),
        format!("{tool}("),
        format!("__{tool}`"),
        format!("\"tool\": \"{tool}\""),
    ]
    .iter()
    .any(|pattern| text.contains(pattern));

    let tool_reference = [
        format!("{tool} tool"),
        format!("tool `{tool}`"),
        format!("tool named `{tool}`"),
    ]
    .iter()
    .any(|pattern| text.contains(pattern))
        || (about_tools(&text, live_tools)
            && (names_tool_in_backticks(&text, tool)
                || lists_tool_with_live_tools(&text, tool, live_tools)));
    let explicitly_absent = [
        "no standalone",
        "does not register",
        "does not add",
        "removed",
        "obsolete",
    ]
    .iter()
    .any(|marker| text.contains(marker));

    invocation || (tool_reference && !explicitly_absent)
}

/// The paragraph says "tool" or backticks a live tool name, so a bare name
/// in it is read as a tool rather than an action or a value.
fn about_tools(text: &str, live_tools: &HashSet<String>) -> bool {
    text.contains("tool")
        || live_tools
            .iter()
            .any(|live| text.contains(&format!("`{live}`")))
}

/// `` `screenshot` `` names the tool unless a noun such as "object" or
/// "field" marks it as a result or parameter key of the same name.
fn names_tool_in_backticks(text: &str, tool: &str) -> bool {
    let quoted = format!("`{tool}`");
    text.match_indices(&quoted).any(|(at, _)| {
        let next = text[at + quoted.len()..]
            .split(|c: char| !c.is_ascii_alphanumeric())
            .find(|word| !word.is_empty());
        !matches!(
            next,
            Some("object" | "field" | "key" | "property" | "parameter" | "argument")
        )
    })
}

/// `[screenshot, scroll]`, `(click, screenshot)` or `{"screenshot", "click"}`:
/// a bracketed list of bare or quoted names holding the tool and a tool the
/// live registry still serves.
fn lists_tool_with_live_tools(text: &str, tool: &str, live_tools: &HashSet<String>) -> bool {
    text.split(['[', '(', '{'])
        .skip(1)
        .filter_map(|after_open| after_open.split([']', ')', '}']).next())
        .any(|inside| {
            let items: Option<Vec<&str>> = inside
                .split(',')
                .map(|item| {
                    let item = item.trim().trim_matches(['"', '\'', '`']);
                    let is_name = !item.is_empty()
                        && item
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
                    is_name.then_some(item)
                })
                .collect();
            items.is_some_and(|items| {
                items.contains(&tool) && items.iter().any(|item| live_tools.contains(*item))
            })
        })
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
    collect_docs(&repo_root.join("docs/content/docs/cua-driver"), &mut files);

    let mut failures = Vec::new();
    for path in files.into_iter().filter(|path| path.is_file()) {
        let contents = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("failed to read guidance file {}: {error}", path.display())
        });
        let mut line_number = 1;
        for paragraph in contents.split_inclusive("\n\n") {
            for tool in REMOVED_TOOLS {
                if advertises_removed_tool(paragraph, tool, &registered) {
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
fn removed_tool_reference_detection_covers_invocations_lists_and_prose() {
    let live: HashSet<String> = [
        "scroll",
        "wait",
        "click",
        "get_window_state",
        "zoom",
        "launch_app",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for reference in [
        "cua-driver call screenshot '{}'",
        "screenshot({ window_id: 42 })",
        "use the `screenshot` tool",
        "use the screenshot tool",
        "  tools: [screenshot, scroll, wait]",
        "GUI tools (click, type_text, screenshot, get_window_state)",
        r#"safe_tools := {"screenshot", "get_window_state", "click"}"#,
        "keep `screenshot` and `zoom` out of the allowlist",
        "`launch_app` start_minimized: only `screenshot` and `delivery_mode` need it restored",
        "keeps the normal MCP tools and changes only `screenshot`, which requires `pid`",
        "do not expose the `mcp__cua-computer-use__screenshot` tool name",
        r#"{"tool": "screenshot", "args": {}}"#,
    ] {
        assert!(
            advertises_removed_tool(reference, "screenshot", &live),
            "{reference:?}"
        );
    }

    for notice in [
        "there is no standalone screenshot tool",
        "the `screenshot` tool was removed",
        "this does not register a standalone `screenshot` tool",
        "the old compatibility flag does not add a screenshot tool",
        "plus a structured `screenshot` object",
        r#"("som"/"screenshot" still decode as deprecated aliases.)"#,
        "use get_window_state for a window tree plus screenshot, then zoom",
        "take a screenshot, then click",
        "`shot` saves a screenshot, `click` clicks",
        "(screenshot, zoom, and click are separate steps)",
        "save the image as `screenshot` before continuing",
        "the workflow has three steps: (screenshot, click, zoom)",
        r#"{"tool": "click", "args": {"name": "screenshot"}}"#,
    ] {
        assert!(
            !advertises_removed_tool(notice, "screenshot", &live),
            "{notice:?}"
        );
    }
}
