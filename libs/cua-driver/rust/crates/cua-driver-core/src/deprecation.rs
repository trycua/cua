// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Deprecated MCP tools.
//!
//! A deprecated tool keeps working until its removal release. This table is
//! the single source of truth for what is deprecated, what replaces it and when
//! it goes. `tools/list` (and so `describe`, `list-tools` and the generated
//! docs) marks each entry, and every response from a deprecated tool carries a
//! plain-text notice. Removing a tool means deleting its row here along with
//! the tool itself.

use serde_json::{json, Value};

/// Tools stay available through this date. They are removed in the first Cua
/// Driver release after it.
pub const REMOVAL_AFTER: &str = "26 Nov 2026";
/// Machine-readable form of [`REMOVAL_AFTER`].
pub const REMOVAL_AFTER_ISO: &str = "2026-11-26";

/// A tool that still works but is scheduled for removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deprecation {
    pub tool: &'static str,
    /// What to use instead, written for an agent reading a tool result.
    pub replacement: &'static str,
}

pub const DEPRECATED_TOOLS: &[Deprecation] = &[
    Deprecation {
        tool: "replay_trajectory",
        replacement: "call the recorded steps yourself with run_steps (replay clicks blindly from old coordinates; keep start_recording for recording)",
    },
    Deprecation {
        tool: "install_ffmpeg",
        replacement: "install ffmpeg with your package manager (brew install ffmpeg, winget install Gyan.FFmpeg, or apt/dnf/pacman install ffmpeg)",
    },
    Deprecation {
        tool: "get_accessibility_tree",
        replacement: "list_windows and list_apps for what is open, and get_window_state for one window's accessibility tree",
    },
];

/// The deprecation entry for `tool`, if it is deprecated.
pub fn lookup(tool: &str) -> Option<&'static Deprecation> {
    DEPRECATED_TOOLS.iter().find(|entry| entry.tool == tool)
}

impl Deprecation {
    /// Plain-text notice appended to every response from the tool.
    pub fn notice(&self) -> String {
        format!(
            "Deprecated: `{}` still works but will be removed in the first Cua Driver release after {REMOVAL_AFTER}. Instead: {}.",
            self.tool, self.replacement
        )
    }

    /// Prefix for the tool description in `tools/list`.
    pub fn description_prefix(&self) -> String {
        format!(
            "DEPRECATED, removed in the first release after {REMOVAL_AFTER}. Instead: {}.",
            self.replacement
        )
    }

    /// Machine-readable marker added to the `tools/list` entry.
    pub fn list_marker(&self) -> Value {
        json!({
            "removal_after": REMOVAL_AFTER_ISO,
            "replacement": self.replacement,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_names_the_removal_date_and_a_replacement() {
        assert!(!DEPRECATED_TOOLS.is_empty());
        for entry in DEPRECATED_TOOLS {
            let notice = entry.notice();
            assert!(notice.contains(entry.tool), "{notice}");
            assert!(notice.contains(REMOVAL_AFTER), "{notice}");
            assert!(!entry.replacement.is_empty(), "{}", entry.tool);
            assert!(entry.description_prefix().contains(REMOVAL_AFTER));
            assert_eq!(entry.list_marker()["removal_after"], REMOVAL_AFTER_ISO);
        }
    }

    #[test]
    fn lookup_finds_only_deprecated_tools() {
        assert!(lookup("replay_trajectory").is_some());
        assert!(lookup("install_ffmpeg").is_some());
        assert!(lookup("get_accessibility_tree").is_some());
        assert!(lookup("click").is_none());
        assert!(lookup("page").is_none());
    }
}
