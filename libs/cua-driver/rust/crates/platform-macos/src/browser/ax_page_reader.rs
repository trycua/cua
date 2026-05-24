//! AXPageReader: extract text and query elements from AX tree markdown strings.

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
    "AXWindow", "AXApplication", "AXGroup", "AXScrollArea",
    "AXSplitGroup", "AXSplitter", "AXMenuBar", "AXMenu",
    "AXMenuBarItem", "AXUnknown",
];

/// Roles whose text content is always extracted (without filtering).
const TEXT_ROLES: &[&str] = &["AXStaticText", "AXHeading", "AXWebArea"];

impl AXPageReader {
    /// Extract readable text from an AX tree markdown string.
    pub fn extract_text(tree_markdown: &str) -> String {
        let mut lines_out: Vec<String> = Vec::new();
        let mut last: Option<String> = None;

        for line in tree_markdown.lines() {
            let Some(el) = parse_ax_line(line) else { continue };

            let is_text_role = TEXT_ROLES.contains(&el.role.as_str());
            let is_skipped = SKIP_ROLES.contains(&el.role.as_str());

            if is_skipped && !is_text_role { continue; }

            // Pick the best text: title > value > description.
            let text = best_text(&el);
            if text.is_empty() { continue; }

            // Deduplicate consecutive identical lines.
            if last.as_deref() == Some(&text) { continue; }
            last = Some(text.clone());
            lines_out.push(text);
        }

        lines_out.join("\n")
    }

    /// Query elements from AX tree markdown by CSS selector (maps to AX roles).
    pub fn query(selector: &str, tree_markdown: &str) -> Vec<AXElement> {
        let roles = css_selector_to_ax_roles(selector);
        let match_all = roles.is_empty();

        let mut result = Vec::new();
        for line in tree_markdown.lines() {
            let Some(el) = parse_ax_line(line) else { continue };
            if match_all || roles.iter().any(|r| *r == el.role) {
                result.push(el);
            }
        }
        result
    }
}

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
    if pos.starts_with('"') {
        let (t, remaining) = parse_quoted_string(pos);
        title = t;
        pos = remaining.trim_start();
    }

    // Value: `= "..."`.
    if let Some(after_eq) = pos.strip_prefix("= ") {
        let p = after_eq.trim_start();
        if p.starts_with('"') {
            let (v, remaining) = parse_quoted_string(p);
            value = v;
            pos = remaining.trim_start();
        } else {
            pos = after_eq;
        }
    } else if let Some(after_eq) = pos.strip_prefix('=') {
        let p = after_eq.trim_start();
        if p.starts_with('"') {
            let (v, remaining) = parse_quoted_string(p);
            value = v;
            pos = remaining.trim_start();
        } else {
            pos = after_eq;
        }
    }

    // Description: `(...)`.
    if pos.starts_with('(') {
        if let Some(close) = pos.find(')') {
            description = pos[1..close].to_owned();
        }
    }

    Some(AXElement { role, title, value, description, index })
}

/// Parse a double-quoted string starting at the beginning of `s`.
/// Returns (content, remainder_after_closing_quote).
fn parse_quoted_string(s: &str) -> (String, &str) {
    if !s.starts_with('"') {
        return (String::new(), s);
    }
    let s = &s[1..]; // skip opening quote
    let mut result = String::new();
    let mut chars = s.char_indices();
    loop {
        match chars.next() {
            None => return (result, ""),
            Some((i, '\\')) => {
                match chars.next() {
                    Some((_, c)) => result.push(c),
                    None => return (result, &s[i + 1..]),
                }
            }
            Some((i, '"')) => {
                return (result, &s[i + 1..]);
            }
            Some((_, c)) => result.push(c),
        }
    }
}

fn split_first_word(s: &str) -> (&str, &str) {
    let pos = s.find(|c: char| c.is_whitespace()).unwrap_or(s.len());
    (&s[..pos], &s[pos..])
}

fn best_text(el: &AXElement) -> String {
    if !el.title.is_empty() { return el.title.clone(); }
    if !el.value.is_empty() { return el.value.clone(); }
    if !el.description.is_empty() { return el.description.clone(); }
    String::new()
}

fn css_selector_to_ax_roles(selector: &str) -> Vec<&'static str> {
    match selector.trim() {
        "a" | "link"                => vec!["AXLink"],
        "button"                    => vec!["AXButton"],
        "input"                     => vec!["AXTextField", "AXCheckBox", "AXRadioButton"],
        "h1" | "h2" | "h3"
        | "h4" | "h5" | "h6"       => vec!["AXHeading"],
        "p"                         => vec!["AXStaticText"],
        "img"                       => vec!["AXImage"],
        "select"                    => vec!["AXPopUpButton", "AXComboBox"],
        "li"                        => vec!["AXStaticText"],
        "*" | ""                    => vec![],
        _                           => vec![],
    }
}
