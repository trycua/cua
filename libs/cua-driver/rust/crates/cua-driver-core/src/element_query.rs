//! Shared projection for query-filtered accessibility snapshots.
//!
//! Platform walkers keep their complete native cache so original handles stay
//! resolvable. This module only projects the protocol response to the indexed
//! rows rendered by the filtered Markdown tree.

use serde_json::Value;
use std::collections::HashSet;

pub fn project_elements_for_query(
    mut elements: Vec<Value>,
    query: Option<&str>,
    filtered_markdown: &str,
) -> Vec<Value> {
    if query.is_none() {
        return elements;
    }

    let visible_indices: HashSet<u64> = filtered_markdown
        .lines()
        .filter_map(|line| {
            let rendered = line.trim_start().strip_prefix("- [")?;
            let (index, _) = rendered.split_once(']')?;
            index.parse().ok()
        })
        .collect();
    elements.retain(|entry| {
        entry
            .get("element_index")
            .and_then(Value::as_u64)
            .is_some_and(|index| visible_indices.contains(&index))
    });
    elements
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn projection_preserves_original_indices_for_matches_and_ancestors() {
        let elements = (0..5)
            .map(|element_index| json!({ "element_index": element_index }))
            .collect();
        let markdown = "- [0] Window\n  - [1] Window\n    - [3] Left\n";
        let projected = project_elements_for_query(elements, Some("Left"), markdown);
        let indices: Vec<_> = projected
            .iter()
            .map(|entry| entry["element_index"].as_u64().unwrap())
            .collect();
        assert_eq!(indices, vec![0, 1, 3]);
    }

    #[test]
    fn absent_query_returns_every_element() {
        let elements = vec![json!({ "element_index": 7 })];
        assert_eq!(
            project_elements_for_query(elements.clone(), None, ""),
            elements
        );
    }
}
