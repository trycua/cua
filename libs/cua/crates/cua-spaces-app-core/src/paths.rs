// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Paths as native macOS apps show them: under the home folder as `~/...`.

/// `path` relative to `home` (`~`, `~/x`), else unchanged.
pub fn display_path(path: &str, home: Option<&str>) -> String {
    let Some(h) = home.map(|h| h.trim_end_matches('/')) else {
        return path.to_string();
    };
    if h.is_empty() {
        return path.to_string();
    }
    if path == h {
        return "~".into();
    }
    match path.strip_prefix(h).filter(|rest| rest.starts_with('/')) {
        Some(rest) => format!("~{rest}"),
        None => path.to_string(),
    }
}

/// Replaces every occurrence of the home folder in free text.
pub fn display_paths(text: &str, home: Option<&str>) -> String {
    let Some(h) = home.map(|h| h.trim_end_matches('/')) else {
        return text.to_string();
    };
    if h.is_empty() {
        return text.to_string();
    }
    let replaced = text.replace(&format!("{h}/"), "~/");
    // A bare home folder, followed by the end, whitespace, a comma or `)`.
    let mut out = String::with_capacity(replaced.len());
    let mut rest = replaced.as_str();
    while let Some(at) = rest.find(h) {
        let after = &rest[at + h.len()..];
        let boundary = after
            .chars()
            .next()
            .is_none_or(|c| c.is_whitespace() || c == ',' || c == ')');
        out.push_str(&rest[..at]);
        out.push_str(if boundary { "~" } else { h });
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_relative() {
        let home = Some("/Users/ada");
        assert_eq!(display_path("/Users/ada/Projects/x", home), "~/Projects/x");
        assert_eq!(display_path("/Users/ada", home), "~");
        assert_eq!(display_path("/Users/adam/x", home), "/Users/adam/x");
        assert_eq!(display_path("/tmp", None), "/tmp");
        assert_eq!(
            display_paths("sends /Users/ada/a and /Users/ada, done", home),
            "sends ~/a and ~, done"
        );
    }
}
