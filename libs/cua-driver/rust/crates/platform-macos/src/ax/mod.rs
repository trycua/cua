//! macOS Accessibility (AX) API bindings and tree walker.
//!
//! # AX element indexing
//! Every *actionable* element in the tree is assigned an `element_index` (0-based,
//! depth-first). The index is stable within a single `get_window_state` snapshot
//! and is passed to `click`, `type_text`, etc. to identify the target element
//! without requiring pixel coordinates.
//!
//! # Tree format (treeMarkdown)
//! ```text
//!   - [0] AXButton "OK" [actions=[press]]
//!     - AXStaticText = "OK"
//!   - [1] AXTextField "Name" [value="John"]
//! ```
//! - Indexed elements: `- [N] AXRole "Title" [key=val ...]`
//! - Non-indexed (no actions, not interesting): `- AXRole = "value"`
//! - 2-space indent per depth level

pub mod bindings;
pub mod tree;
pub mod cache;

pub use tree::{walk_tree, AXNode, TreeWalkResult};
pub use cache::ElementCache;
