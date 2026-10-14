// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Drive keys and the layout.
//!
//! A key is a relative, `/`-separated path: no leading slash, no `.` or `..`
//! component, no empty component, no backslash, no control character. A
//! key ending in `/` names a folder (a prefix). The three top-level areas
//! are `public/`, `agents/<agent>/` and `spaces/<space>/`.

use crate::{Error, Result};

/// Longest key accepted (S3's limit is 1024 bytes).
pub const MAX_KEY: usize = 1024;

/// The public area.
pub const PUBLIC: &str = "public/";
/// The agents area.
pub const AGENTS: &str = "agents/";
/// The Spaces area.
pub const SPACES: &str = "spaces/";

/// Normalizes a user-supplied path to a key. `""` and `"/"` are the root.
/// A trailing `/` is kept (it names a folder).
pub fn normalize(p: &str) -> Result<String> {
    let trimmed = p.trim();
    let folder = trimmed.ends_with('/') && trimmed.len() > 1;
    let body = trimmed.trim_start_matches('/').trim_end_matches('/');
    if body.is_empty() {
        return Ok(String::new());
    }
    if body.contains('\\') {
        return Err(Error::Invalid(format!("{p:?}: use / as the separator")));
    }
    let mut parts = vec![];
    for c in body.split('/') {
        if c.is_empty() || c == "." || c == ".." {
            return Err(Error::Invalid(format!(
                "{p:?}: empty, `.` or `..` components are not allowed"
            )));
        }
        if c.chars().any(|ch| ch.is_control()) {
            return Err(Error::Invalid(format!("{p:?}: control characters")));
        }
        parts.push(c);
    }
    let mut key = parts.join("/");
    if folder {
        key.push('/');
    }
    if key.len() > MAX_KEY {
        return Err(Error::Invalid(format!("path longer than {MAX_KEY} bytes")));
    }
    Ok(key)
}

/// Normalizes to a folder key (always ending in `/`, or `""` for the root).
pub fn folder(p: &str) -> Result<String> {
    let k = normalize(p)?;
    Ok(if k.is_empty() || k.ends_with('/') {
        k
    } else {
        format!("{k}/")
    })
}

/// Whether `name` is a valid agent name: 1 to 63 of `[a-z0-9._-]`, starting
/// with a letter or digit.
pub fn valid_agent_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')
        })
}

/// An agent's home folder: `agents/<name>/`.
pub fn agent_home(name: &str) -> Result<String> {
    if !valid_agent_name(name) {
        return Err(Error::Invalid(format!(
            "agent name {name:?}: use 1-63 of a-z, 0-9, `.`, `_`, `-`"
        )));
    }
    Ok(format!("{AGENTS}{name}/"))
}

/// The folder name for a Space id: `local:work` is `local-work`,
/// `relay:0123` is `relay-0123`. Anything outside `[A-Za-z0-9._-]` becomes
/// `-`.
pub fn space_folder_name(space_id: &str) -> String {
    let s: String = space_id
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "space".into() } else { s }
}

/// A Space's folder: `spaces/<folder name>/`.
pub fn space_folder(space_id: &str) -> String {
    format!("{SPACES}{}/", space_folder_name(space_id))
}

/// The area a key is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Area {
    /// The root or one of the area folders themselves.
    Root,
    Public,
    /// `agents/<name>/...`
    Agent(String),
    /// `spaces/<folder>/...`
    Space(String),
    /// Anything else at the top level (only the user reaches it).
    Other,
}

/// The area of a normalized key.
pub fn area(key: &str) -> Area {
    if key.is_empty() || key == AGENTS || key == SPACES {
        return Area::Root;
    }
    if key == PUBLIC || key.starts_with(PUBLIC) {
        return Area::Public;
    }
    let second = |rest: &str| rest.split('/').next().unwrap_or("").to_string();
    if let Some(rest) = key.strip_prefix(AGENTS) {
        return Area::Agent(second(rest));
    }
    if let Some(rest) = key.strip_prefix(SPACES) {
        return Area::Space(second(rest));
    }
    Area::Other
}

/// The parent folder of a key (`a/b/c` -> `a/b/`; `a` -> `""`).
pub fn parent(key: &str) -> String {
    let k = key.trim_end_matches('/');
    match k.rfind('/') {
        Some(i) => k[..=i].to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_refuses_escapes() {
        assert_eq!(normalize("/public/a.md").unwrap(), "public/a.md");
        assert_eq!(normalize("public/docs/").unwrap(), "public/docs/");
        assert_eq!(normalize("").unwrap(), "");
        assert_eq!(normalize("/").unwrap(), "");
        for bad in ["a/../b", "a//b", "./a", "a\\b", "a/\u{7}/b"] {
            assert_eq!(
                normalize(bad).unwrap_err().tag(),
                "invalid_argument",
                "{bad}"
            );
        }
        assert!(normalize(&"a".repeat(MAX_KEY + 1)).is_err());
        assert_eq!(folder("agents/ada").unwrap(), "agents/ada/");
    }

    #[test]
    fn areas_and_names() {
        assert_eq!(area("public/x"), Area::Public);
        assert_eq!(
            area("agents/ada/memory/MEMORY.md"),
            Area::Agent("ada".into())
        );
        assert_eq!(area("agents/ada/"), Area::Agent("ada".into()));
        assert_eq!(
            area("spaces/local-work/out.txt"),
            Area::Space("local-work".into())
        );
        assert_eq!(area("agents/"), Area::Root);
        assert_eq!(area("elsewhere/x"), Area::Other);
        assert!(valid_agent_name("ada"));
        assert!(valid_agent_name("research-bot.2"));
        assert!(!valid_agent_name("Ada"));
        assert!(!valid_agent_name("-x"));
        assert!(!valid_agent_name("a/b"));
        assert!(agent_home("../x").is_err());
        assert_eq!(space_folder("local:work"), "spaces/local-work/");
        assert_eq!(space_folder("relay:0123abcd"), "spaces/relay-0123abcd/");
        assert_eq!(
            space_folder("direct:10.0.0.5:3211"),
            "spaces/direct-10.0.0.5-3211/"
        );
        assert_eq!(parent("a/b/c"), "a/b/");
        assert_eq!(parent("a"), "");
    }
}
