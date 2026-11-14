// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Spreadsheet grids seen in a window's accessibility tree (see
//! [`cua_driver_core::sheet_grid`]): the last grid each window showed, and a
//! fresh read of it for an action's result.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use cua_driver_core::sheet_grid::{self, Grid, NodeFacts};

use crate::ax::tree::{AXNode, TreeWalkResult};

/// Longest a read of the grid after an action may walk.
const READ_TIMEOUT_MS: u64 = 1500;
/// Most nodes a read of the grid after an action may walk (a screen of a
/// sheet is a few hundred cells; the toolbars follow them).
const READ_MAX_ELEMENTS: usize = 4000;

fn facts(node: &AXNode) -> NodeFacts<'_> {
    NodeFacts {
        depth: node.depth,
        role: &node.role,
        title: node.title.as_deref(),
        description: node.description.as_deref(),
        value: node.value.as_deref(),
        index: node.element_index,
        selected: node.selected,
    }
}

/// The spreadsheet grid in a walked tree, if it has one.
pub fn grid_of(tree: &TreeWalkResult) -> Option<Grid> {
    let nodes: Vec<NodeFacts<'_>> = tree.nodes.iter().map(facts).collect();
    sheet_grid::find_grid(&nodes, !tree.truncated)
}

fn grids() -> &'static Mutex<HashMap<(i32, u32), Grid>> {
    static GRIDS: OnceLock<Mutex<HashMap<(i32, u32), Grid>>> = OnceLock::new();
    GRIDS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remember `grid` as what `window_id` shows now.
pub fn remember(pid: i32, window_id: u32, grid: Grid) {
    let mut map = grids().lock().unwrap_or_else(|e| e.into_inner());
    if map.len() > 64 {
        map.clear();
    }
    map.insert((pid, window_id), grid);
}

/// Forget `window_id`'s grid (a complete read found none).
pub fn forget(pid: i32, window_id: u32) {
    grids()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&(pid, window_id));
}

/// The grid `window_id` showed at its last read or action.
pub fn last_grid(pid: i32, window_id: u32) -> Option<Grid> {
    grids()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(pid, window_id))
        .cloned()
}

/// Read `window_id`'s grid now (blocking; run off the async runtime).
pub fn read_grid(pid: i32, window_id: u32) -> Option<Grid> {
    let tree = crate::ax::tree::with_menus_collapsed(true, || {
        crate::ax::tree::walk_tree_budgeted(
            pid,
            Some(window_id),
            None,
            crate::ax::tree::DEFAULT_MAX_DEPTH,
            cua_driver_core::walk_budget::WalkBudget::new(READ_TIMEOUT_MS, READ_MAX_ELEMENTS),
        )
    });
    if !tree.window_scope.as_ref().is_none_or(|s| s.is_matched()) {
        return None;
    }
    grid_of(&tree)
}
