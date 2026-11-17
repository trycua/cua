// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Shared `query` matching for accessibility snapshots.
//!
//! A query is a case-insensitive substring of a rendered tree row; `a|b`
//! matches rows containing either alternative. [`filter_tree`] keeps matching
//! rows plus their ancestor chain. Platform walkers keep their complete native
//! cache so original handles stay resolvable; [`project_elements_for_query`]
//! only projects the protocol response to the indexed rows the filtered tree
//! renders.

use cua_driver_contract::WindowObservation;
use serde_json::{json, Value};
use std::collections::HashSet;

/// Hard cap on `observations` rows in one response.
pub const MAX_OBSERVATIONS: usize = 500;

/// Publish a walk's display-only rows as `observations`.
///
/// A `query` read keeps the rows whose role, label or value contains the
/// query (case-insensitive, `a|b` for either), as its filtered Markdown does. The
/// list stops at [`MAX_OBSERVATIONS`] and then says so with
/// `observations_truncated: true`.
pub fn publish_observations(
    structured: &mut Value,
    rows: Vec<WindowObservation>,
    query: Option<&str>,
) {
    let query = query.map(Query::parse);
    let mut kept: Vec<WindowObservation> = rows
        .into_iter()
        .filter(|row| {
            query.as_ref().is_none_or(|query| {
                [
                    Some(row.role.as_str()),
                    row.label.as_deref(),
                    row.value.as_deref(),
                ]
                .into_iter()
                .flatten()
                .any(|text| query.matches(text))
            })
        })
        .collect();
    if kept.len() > MAX_OBSERVATIONS {
        kept.truncate(MAX_OBSERVATIONS);
        structured["observations_truncated"] = json!(true);
    }
    structured["observations"] = serde_json::to_value(kept).expect("WindowObservation serializes");
}

/// A parsed `query`: lowercase alternatives, any one of which must occur in a
/// row. Blank alternatives (`"a||b"`, a trailing `|`) are dropped; a query with
/// no non-blank alternative is matched literally.
struct Query(Vec<String>);

impl Query {
    fn parse(query: &str) -> Self {
        let lower = query.to_lowercase();
        let alternatives: Vec<String> = lower
            .split('|')
            .map(str::trim)
            .filter(|alt| !alt.is_empty())
            .map(str::to_owned)
            .collect();
        if alternatives.is_empty() {
            Self(vec![lower])
        } else {
            Self(alternatives)
        }
    }

    fn matches(&self, line: &str) -> bool {
        let line = line.to_lowercase();
        self.0.iter().any(|alt| line.contains(alt.as_str()))
    }
}

/// Lines of `markdown` matching `query`, each preceded by its ancestor chain
/// (by two-space indent), without repeating an ancestor already emitted.
pub fn filter_tree(markdown: &str, query: &str) -> String {
    let query = Query::parse(query);
    let mut ancestors: Vec<&str> = Vec::new();
    let mut last_emitted: Vec<Option<&str>> = Vec::new();
    let mut output: Vec<&str> = Vec::new();

    for line in markdown.lines() {
        let depth = line.chars().take_while(|c| *c == ' ').count() / 2;
        while ancestors.len() <= depth {
            ancestors.push("");
            last_emitted.push(None);
        }
        last_emitted[depth + 1..].fill(None);
        ancestors[depth] = line;

        if query.matches(line) {
            for d in 0..depth {
                if ancestors[d].is_empty() || last_emitted[d] == Some(ancestors[d]) {
                    continue;
                }
                last_emitted[d] = Some(ancestors[d]);
                output.push(ancestors[d]);
            }
            last_emitted[depth] = Some(line);
            output.push(line);
        }
    }

    if output.is_empty() {
        return String::new();
    }
    let mut result = output.join("\n");
    result.push('\n');
    result
}

/// Rows of a rendered tree that match `query` themselves, not counting the
/// ancestor rows [`filter_tree`] adds for context. A row runs from its `- `
/// line through its continuation lines; a blank line ends the tree, so a note
/// appended after it is not counted.
pub fn matching_rows(markdown: &str, query: &str) -> usize {
    let query = Query::parse(query);
    let mut count = 0;
    let mut row_matched = false;
    for line in markdown.lines() {
        if line.trim().is_empty() {
            break;
        }
        if line.trim_start().starts_with("- ") {
            row_matched = false;
        }
        if !row_matched && query.matches(line) {
            row_matched = true;
            count += 1;
        }
    }
    count
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

    const TREE: &str = "- [0] AXWindow \"Xcode\"\n  - [1] AXGroup \"Toolbar\"\n    - [2] AXButton \"Build\"\n    - [3] AXButton \"Run\"\n  - [4] AXGroup \"Editor\"\n    - [5] AXButton \"Reload plugin\"\n";

    #[test]
    fn a_query_keeps_matching_rows_and_their_ancestors() {
        assert_eq!(
            filter_tree(TREE, "build"),
            "- [0] AXWindow \"Xcode\"\n  - [1] AXGroup \"Toolbar\"\n    - [2] AXButton \"Build\"\n"
        );
        assert_eq!(matching_rows(&filter_tree(TREE, "build"), "build"), 1);
    }

    #[test]
    fn a_bar_separates_alternatives() {
        let filtered = filter_tree(TREE, "Build | plugin|");
        assert_eq!(
            filtered,
            "- [0] AXWindow \"Xcode\"\n  - [1] AXGroup \"Toolbar\"\n    - [2] AXButton \"Build\"\n  - [4] AXGroup \"Editor\"\n    - [5] AXButton \"Reload plugin\"\n"
        );
        assert_eq!(matching_rows(&filtered, "Build | plugin|"), 2);
    }

    #[test]
    fn a_query_that_matches_nothing_yields_an_empty_tree() {
        assert_eq!(filter_tree(TREE, "Settings|Preferences"), "");
        assert_eq!(matching_rows(TREE, "Settings|Preferences"), 0);
    }

    #[test]
    fn a_query_of_only_bars_is_matched_literally() {
        assert_eq!(filter_tree(TREE, "|"), "");
        let piped = "- [0] AXStaticText \"a | b\"\n";
        assert_eq!(filter_tree(piped, " | "), piped);
    }

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
