// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! AXPageReader: extract text and query elements from AX tree markdown strings.

use cua_driver_core::tree_text;

pub struct AXPageReader;

#[derive(Debug, Clone)]
pub struct AXElement {
    pub role: String,
    pub title: String,
    pub value: String,
    pub description: String,
    pub index: Option<usize>,
}

/// Roles to skip when extracting plain text.
const SKIP_ROLES: &[&str] = &[
    "AXWindow",
    "AXApplication",
    "AXGroup",
    "AXScrollArea",
    "AXSplitGroup",
    "AXSplitter",
    "AXMenuBar",
    "AXMenu",
    "AXMenuBarItem",
    "AXUnknown",
];

/// Roles whose text content is always extracted (without filtering).
const TEXT_ROLES: &[&str] = &["AXStaticText", "AXHeading", "AXWebArea"];

impl AXPageReader {
    /// The part of a window-scoped AX tree that is page content: every
    /// `AXWebArea` row and the rows nested under it. A window-scoped walk also
    /// carries the app's menu bar and the browser's own toolbar, which are not
    /// the page; reading them as page text answers a `get_text` or `query_dom`
    /// with "Apple, About This Mac, ...". `None` when the tree has no web area
    /// (the page's accessibility tree is not built, or the window is not a
    /// page).
    pub fn web_area_markdown(tree_markdown: &str) -> Option<String> {
        let mut out: Vec<&str> = Vec::new();
        let mut area_indent: Option<usize> = None;
        for line in tree_markdown.lines() {
            let indent = line.len() - line.trim_start().len();
            if let Some(base) = area_indent {
                if indent > base {
                    out.push(line);
                    continue;
                }
                area_indent = None;
            }
            if parse_ax_line(line).is_some_and(|el| el.role == "AXWebArea") {
                area_indent = Some(indent);
                out.push(line);
            }
        }
        (!out.is_empty()).then(|| out.join("\n"))
    }

    /// Extract readable text from an AX tree markdown string.
    pub fn extract_text(tree_markdown: &str) -> String {
        let mut lines_out: Vec<String> = Vec::new();
        let mut last: Option<String> = None;

        for line in tree_markdown.lines() {
            let Some(el) = parse_ax_line(line) else {
                continue;
            };

            let is_text_role = TEXT_ROLES.contains(&el.role.as_str());
            let is_skipped = SKIP_ROLES.contains(&el.role.as_str());

            if is_skipped && !is_text_role {
                continue;
            }

            // Pick the best text: title > value > description.
            let text = best_text(&el);
            if text.is_empty() {
                continue;
            }

            // Deduplicate consecutive identical lines.
            if last.as_deref() == Some(&text) {
                continue;
            }
            last = Some(text.clone());
            lines_out.push(text);
        }

        lines_out.join("\n")
    }

    /// Query elements from AX tree markdown with a role filter from
    /// [`AxRoleFilter::from_css`].
    pub fn query(filter: AxRoleFilter, tree_markdown: &str) -> Vec<AXElement> {
        tree_markdown
            .lines()
            .filter_map(parse_ax_line)
            .filter(|el| match filter {
                AxRoleFilter::Any => true,
                AxRoleFilter::Roles(roles) => roles.contains(&el.role.as_str()),
            })
            .collect()
    }
}

/// The AX roles a CSS selector selects when the page is read through the
/// accessibility tree instead of the DOM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxRoleFilter {
    /// `*` or an empty selector: every element.
    Any,
    Roles(&'static [&'static str]),
}

impl AxRoleFilter {
    /// The AX tree carries no ids, classes or attributes, so only a bare tag
    /// selector with a role mapping (or `*`) can be applied. `None` for every
    /// other selector: matching it by role alone would answer a different
    /// query.
    pub fn from_css(selector: &str) -> Option<Self> {
        let roles: &'static [&'static str] = match selector.trim() {
            "*" | "" => return Some(Self::Any),
            "a" | "link" => &["AXLink"],
            "button" => &["AXButton"],
            "input" => &["AXTextField", "AXCheckBox", "AXRadioButton"],
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => &["AXHeading"],
            "p" | "li" => &["AXStaticText"],
            "img" => &["AXImage"],
            "select" => &["AXPopUpButton", "AXComboBox"],
            _ => return None,
        };
        Some(Self::Roles(roles))
    }
}

/// The selectors [`AxRoleFilter::from_css`] accepts, for refusal messages.
pub const AX_SELECTORS: &str = "a, link, button, input, h1-h6, p, li, img, select, *";

/// Parse a single AX tree markdown line into an AXElement.
///
/// Formats handled:
///   `  - [N] AXRole "title" = "value" (description)`
///   `  - AXRole "title" = "value" (description)`
///   `  - AXRole "title"`
///   `  - AXRole = "value"`
fn parse_ax_line(line: &str) -> Option<AXElement> {
    // Strip leading whitespace and "- ".
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("- ")?;

    // Optional index: `[N] `.
    let (index, rest) = if rest.starts_with('[') {
        if let Some(bracket_end) = rest.find(']') {
            let idx_str = &rest[1..bracket_end];
            let idx = idx_str.parse::<usize>().ok();
            (idx, rest[bracket_end + 1..].trim_start())
        } else {
            (None, rest)
        }
    } else {
        (None, rest)
    };

    // Role is the first whitespace-separated token.
    let (role, after_role) = split_first_word(rest);
    if role.is_empty() || !role.starts_with("AX") {
        return None;
    }
    let role = role.to_owned();
    let after_role = after_role.trim_start();

    // Parse: optional `"title"`, optional `= "value"`, optional `(description)`.
    let mut title = String::new();
    let mut value = String::new();
    let mut description = String::new();
    let mut pos = after_role;

    // Title: starts with `"`.
    if let Some((t, remaining)) = tree_text::unquote(pos) {
        title = t;
        pos = remaining.trim_start();
    }

    // Value: `= "..."`.
    if let Some(after_eq) = pos.strip_prefix('=') {
        match tree_text::unquote(after_eq.trim_start()) {
            Some((v, remaining)) => {
                value = v;
                pos = remaining.trim_start();
            }
            None => pos = after_eq.trim_start(),
        }
    }

    // Description: `(...)`.
    if pos.starts_with('(') {
        if let Some(close) = pos.find(')') {
            description = tree_text::unescape(&pos[1..close]);
        }
    }

    Some(AXElement {
        role,
        title,
        value,
        description,
        index,
    })
}

fn split_first_word(s: &str) -> (&str, &str) {
    let pos = s.find(|c: char| c.is_whitespace()).unwrap_or(s.len());
    (&s[..pos], &s[pos..])
}

fn best_text(el: &AXElement) -> String {
    if !el.title.is_empty() {
        return el.title.clone();
    }
    if !el.value.is_empty() {
        return el.value.clone();
    }
    if !el.description.is_empty() {
        return el.description.clone();
    }
    String::new()
}

#[cfg(test)]
mod web_area_tests {
    use super::{AXPageReader, AxRoleFilter};

    const TREE: &str = "\
- [0] AXMenuBar
  - [1] AXMenuBarItem \"Apple\"
    - [2] AXMenuItem \"About This Mac\"
- [3] AXWindow \"Orders - Chrome\"
  - [4] AXToolbar
    - [5] AXTextField \"Address and search bar\"
  - [6] AXWebArea \"Orders\"
    - [7] AXHeading \"Your loans\"
    - [8] AXStaticText \"Hedge trimmer, due 3 Nov\"
  - [9] AXButton \"Back\"
";

    #[test]
    fn web_area_markdown_keeps_only_page_rows() {
        let page = AXPageReader::web_area_markdown(TREE).expect("web area");
        assert!(page.contains("Hedge trimmer"));
        assert!(page.contains("Your loans"));
        assert!(!page.contains("About This Mac"));
        assert!(!page.contains("Address and search bar"));
        assert!(!page.contains("\"Back\""));
        let text = AXPageReader::extract_text(&page);
        assert!(text.contains("Hedge trimmer, due 3 Nov"));
        assert!(!text.contains("Apple"));
    }

    #[test]
    fn a_tree_without_a_web_area_has_no_page_content() {
        let tree = "- [0] AXMenuBar\n  - [1] AXMenuBarItem \"Apple\"\n- [2] AXWindow \"x\"\n";
        assert_eq!(AXPageReader::web_area_markdown(tree), None);
    }

    #[test]
    fn a_selector_the_ax_tree_cannot_express_has_no_role_filter() {
        for selector in [
            "select#model",
            "nav",
            "div.menu",
            "input[type=text]",
            "a, button",
        ] {
            assert_eq!(AxRoleFilter::from_css(selector), None, "{selector}");
        }
    }

    #[test]
    fn a_tag_selector_returns_only_its_roles() {
        let page = AXPageReader::web_area_markdown(TREE).expect("web area");
        let filter = AxRoleFilter::from_css("h1").expect("h1 maps to a role");
        let found = AXPageReader::query(filter, &page);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Your loans");
    }

    #[test]
    fn the_wildcard_returns_every_row() {
        let page = AXPageReader::web_area_markdown(TREE).expect("web area");
        let filter = AxRoleFilter::from_css(" * ").expect("wildcard");
        assert_eq!(
            AXPageReader::query(filter, &page).len(),
            page.lines().count()
        );
    }
}
