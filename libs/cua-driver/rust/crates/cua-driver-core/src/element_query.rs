// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Shared projection for query-filtered accessibility snapshots.
//!
//! Platform walkers keep their complete native cache so original handles stay
//! resolvable. This module only projects the protocol response to the indexed
//! rows rendered by the filtered Markdown tree.

use cua_driver_contract::WindowObservation;
use serde_json::{json, Value};
use std::collections::HashSet;

/// Hard cap on `observations` rows in one response.
pub const MAX_OBSERVATIONS: usize = 500;

/// Publish a walk's display-only rows as `observations`.
///
/// A `query` read keeps the rows whose role, label or value contains the
/// query (case-insensitive), the same rows its filtered Markdown shows. The
/// list stops at [`MAX_OBSERVATIONS`] and then says so with
/// `observations_truncated: true`.
pub fn publish_observations(
    structured: &mut Value,
    rows: Vec<WindowObservation>,
    query: Option<&str>,
) {
    let needle = query.map(str::to_lowercase);
    let mut kept: Vec<WindowObservation> = rows
        .into_iter()
        .filter(|row| {
            needle.as_deref().is_none_or(|needle| {
                [
                    Some(row.role.as_str()),
                    row.label.as_deref(),
                    row.value.as_deref(),
                ]
                .into_iter()
                .flatten()
                .any(|text| text.to_lowercase().contains(needle))
            })
        })
        .collect();
    if kept.len() > MAX_OBSERVATIONS {
        kept.truncate(MAX_OBSERVATIONS);
        structured["observations_truncated"] = json!(true);
    }
    structured["observations"] = serde_json::to_value(kept).expect("WindowObservation serializes");
}

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

    fn text(role: &str, value: &str) -> WindowObservation {
        WindowObservation {
            role: role.into(),
            depth: 1,
            label: Some(value.into()),
            value: Some(value.into()),
            frame: None,
            parent_index: Some(0),
            in_web_content: None,
        }
    }

    #[test]
    fn observations_carry_no_action_handle() {
        let mut structured = json!({});
        publish_observations(&mut structured, vec![text("AXStaticText", "6")], None);
        let row = &structured["observations"][0];
        assert_eq!(row["role"], "AXStaticText");
        assert_eq!(row["value"], "6");
        assert_eq!(row["parent_index"], 0);
        assert!(row.get("element_index").is_none());
        assert!(row.get("element_token").is_none());
        assert!(structured.get("observations_truncated").is_none());
    }

    #[test]
    fn a_query_keeps_matching_observations_only() {
        let mut structured = json!({});
        publish_observations(
            &mut structured,
            vec![
                text("AXStaticText", "4+2"),
                text("AXStaticText", "Result 6"),
            ],
            Some("result"),
        );
        let rows = structured["observations"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["value"], "Result 6");

        let mut by_role = json!({});
        publish_observations(
            &mut by_role,
            vec![text("AXStaticText", "6")],
            Some("statictext"),
        );
        assert_eq!(by_role["observations"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn observations_stop_at_the_cap_and_say_so() {
        let mut structured = json!({});
        let rows = (0..MAX_OBSERVATIONS + 3)
            .map(|n| text("AXStaticText", &n.to_string()))
            .collect();
        publish_observations(&mut structured, rows, None);
        assert_eq!(
            structured["observations"].as_array().unwrap().len(),
            MAX_OBSERVATIONS
        );
        assert_eq!(structured["observations_truncated"], true);
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
