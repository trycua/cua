//! Accessibility-snapshot text helpers.
//!
//! `get_window_state` renders the AX/UIA tree as indented text lines like
//! `  [12] AXButton id=btn-ok "OK"`. The harness tests parse that text to find
//! an element's index or assert a control is present — logic that was
//! copy-pasted across the per-app harness files.

/// True when the snapshot text is empty or signals a permission wall (TCC on
/// macOS) rather than a real tree — the caller should skip element assertions.
pub fn looks_empty(text: &str) -> bool {
    let line_count = text.lines().count();
    text.is_empty()
        || line_count <= 2
        || text.contains("Accessibility permission required")
        || text.contains("TCC permission")
}

/// True when an element with `id=<id>` appears in the snapshot.
pub fn has_id(text: &str, id: &str) -> bool {
    text.contains(&format!("id={id}"))
}

/// The `[index]` of the first element line carrying `id=<id>`.
pub fn element_index_by_id(text: &str, id: &str) -> Option<u64> {
    index_on_line_matching(text, &format!("id={id}"))
}

/// The `[index]` of the first element line containing `needle` (case-insensitive).
pub fn element_index_containing(text: &str, needle: &str) -> Option<u64> {
    let needle = needle.to_lowercase();
    for line in text.lines() {
        if line.to_lowercase().contains(&needle) {
            if let Some(idx) = bracket_index(line) {
                return Some(idx);
            }
        }
    }
    None
}

fn index_on_line_matching(text: &str, needle: &str) -> Option<u64> {
    text.lines()
        .find(|line| line.contains(needle))
        .and_then(bracket_index)
}

/// Parse the leading `[N]` index token from an element line.
fn bracket_index(line: &str) -> Option<u64> {
    let start = line.find('[')? + 1;
    let end = line[start..].find(']')? + start;
    line[start..end].trim().parse().ok()
}
