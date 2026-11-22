// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Deterministic semantic browser snapshots.
//!
//! The collector joins Chrome's accessibility tree with pierced DOM metadata
//! and layout snapshot evidence. Accessibility supplies readable semantics,
//! DOM backend ids preserve exact mutation capabilities, and layout evidence
//! keeps hidden retained application state from displacing the active view.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

use super::store::{BrowserActionKind, BrowserVisibility, FrameRef, RefEntry};

pub(crate) const SEMANTIC_COMPUTED_STYLES: &[&str] = &[
    "display",
    "visibility",
    "opacity",
    "pointer-events",
    "cursor",
    "position",
    "z-index",
    "overflow-x",
    "overflow-y",
];
pub(crate) const DEFAULT_SEMANTIC_NODE_BUDGET: usize = 300;
const NEAR_VIEWPORT_MARGIN: f64 = 1_000.0;
const MAX_SEMANTIC_TEXT_CHARS: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    fn from_value(value: &Value) -> Option<Self> {
        let values = value.as_array()?;
        Some(Self {
            x: values.first()?.as_f64()?,
            y: values.get(1)?.as_f64()?,
            width: values.get(2)?.as_f64()?,
            height: values.get(3)?.as_f64()?,
        })
    }

    fn has_area(self) -> bool {
        self.width > 0.0 && self.height > 0.0
    }

    fn intersects(self, other: Self) -> bool {
        self.x < other.x + other.width
            && self.x + self.width > other.x
            && self.y < other.y + other.height
            && self.y + self.height > other.y
    }

    fn expanded(self, margin: f64) -> Self {
        Self {
            x: self.x - margin,
            y: self.y - margin,
            width: self.width + margin * 2.0,
            height: self.height + margin * 2.0,
        }
    }

    fn covers(self, other: Self) -> bool {
        self.x <= other.x
            && self.y <= other.y
            && self.x + self.width >= other.x + other.width
            && self.y + self.height >= other.y + other.height
    }
}

#[derive(Debug, Clone, Default)]
struct DomMeta {
    tag: String,
    attrs: HashMap<String, String>,
    order: usize,
    css_hidden: bool,
    transparent: bool,
    parent_backend_node_id: Option<i64>,
    frame_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DomIndex {
    nodes: HashMap<i64, DomMeta>,
    pub(crate) css_hidden_count: usize,
}

#[derive(Debug, Clone, Default)]
struct LayoutMeta {
    bounds: Option<Rect>,
    client_rect: Option<Rect>,
    scroll_rect: Option<Rect>,
    styles: HashMap<String, String>,
    paint_order: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LayoutIndex {
    nodes: HashMap<i64, LayoutMeta>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Viewport {
    rect: Option<Rect>,
}

#[derive(Debug, Clone)]
pub(crate) struct SemanticNode {
    pub(crate) ax_id: String,
    pub(crate) parent_ax_id: Option<String>,
    pub(crate) child_ax_ids: Vec<String>,
    pub(crate) backend_node_id: Option<i64>,
    pub(crate) role: String,
    pub(crate) name: Option<String>,
    pub(crate) value: Option<String>,
    pub(crate) states: BTreeMap<String, Value>,
    pub(crate) frame: FrameRef,
    pub(crate) visibility: BrowserVisibility,
    pub(crate) actions: Vec<BrowserActionKind>,
    pub(crate) document_order: usize,
}

impl SemanticNode {
    pub(crate) fn to_ref_entry(&self) -> Option<RefEntry> {
        let backend_node_id = self.backend_node_id?;
        Some(RefEntry {
            backend_node_id,
            node_name: self.role.clone(),
            label: self.name.clone(),
            actions: self.actions.clone(),
            visibility: Some(self.visibility),
            semantic: true,
            frame: self.frame.clone(),
        })
    }
}

/// One semantic node's identity inside a composed snapshot. AX node ids and
/// backend node ids are only unique within their owning frame/document, so a
/// scope key must retain the complete frame proof.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SemanticNodeIdentity {
    pub(crate) ax_id: String,
    pub(crate) frame: FrameRef,
}

/// Auditable evidence for the scope root selected from a caller-owned ref.
/// Internal node/frame ids remain private; callers receive the source ref,
/// resolved semantics, frame kind, and exact ancestor distance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SemanticScopeAnchor {
    pub(crate) source_ref: String,
    pub(crate) identity: SemanticNodeIdentity,
    pub(crate) role: String,
    pub(crate) name: Option<String>,
    pub(crate) distance: usize,
}

/// The subtree a semantic page is restricted to.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SemanticScope<'a> {
    /// Direct `scope_ref`: the subtree of the node with this backend id.
    Backend(i64),
    /// `scope_ancestor_role`: a root already proven by
    /// [`SemanticDocument::validate_ancestor_subtree`].
    Ancestor(&'a SemanticNodeIdentity),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SemanticScopeFailure {
    SourceMissing,
    SourceNotSemantic,
    SourceChanged,
    AncestorMissing,
    UnprovenFrame,
    BrokenParent,
    Cycle,
    Ambiguous,
    CrossFrame,
}

impl SemanticScopeFailure {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SourceMissing => "source_missing",
            Self::SourceNotSemantic => "source_not_semantic",
            Self::SourceChanged => "source_changed",
            Self::AncestorMissing => "ancestor_missing",
            Self::UnprovenFrame => "unproven_frame",
            Self::BrokenParent => "broken_parent",
            Self::Cycle => "cycle",
            Self::Ambiguous => "ambiguous",
            Self::CrossFrame => "cross_frame",
        }
    }
}

/// Raw AX ancestry retained before ignored/inline/static nodes are removed
/// from the rendered semantic working set. Chromium can place retained
/// controls beneath ignored wrappers, so scoped membership cannot safely be
/// inferred from the filtered nodes alone.
#[derive(Debug, Clone)]
struct SemanticAncestryNode {
    ax_id: String,
    parent_ax_id: Option<String>,
    child_ax_ids: Vec<String>,
    frame: FrameRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SemanticBackendIdentity {
    backend_node_id: i64,
    frame: FrameRef,
}

struct SemanticTreeIndex {
    by_identity: HashMap<SemanticNodeIdentity, Vec<usize>>,
    by_backend: HashMap<SemanticBackendIdentity, Vec<usize>>,
}

impl SemanticTreeIndex {
    fn new(nodes: &[SemanticNode]) -> Self {
        let mut by_identity = HashMap::<SemanticNodeIdentity, Vec<usize>>::new();
        let mut by_backend = HashMap::<SemanticBackendIdentity, Vec<usize>>::new();
        for (idx, node) in nodes.iter().enumerate() {
            by_identity
                .entry(SemanticNodeIdentity {
                    ax_id: node.ax_id.clone(),
                    frame: node.frame.clone(),
                })
                .or_default()
                .push(idx);
            if let Some(backend_node_id) = node.backend_node_id {
                by_backend
                    .entry(SemanticBackendIdentity {
                        backend_node_id,
                        frame: node.frame.clone(),
                    })
                    .or_default()
                    .push(idx);
            }
        }
        Self {
            by_identity,
            by_backend,
        }
    }

    fn nodes_for(&self, identity: &SemanticNodeIdentity) -> &[usize] {
        self.by_identity
            .get(identity)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn nodes_for_backend(&self, frame: &FrameRef, backend_node_id: i64) -> &[usize] {
        self.by_backend
            .get(&SemanticBackendIdentity {
                backend_node_id,
                frame: frame.clone(),
            })
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

struct SemanticAncestryIndex {
    by_identity: HashMap<SemanticNodeIdentity, Vec<usize>>,
    by_parent: HashMap<SemanticNodeIdentity, Vec<usize>>,
    by_ax_id: HashMap<String, Vec<usize>>,
}

impl SemanticAncestryIndex {
    fn new(nodes: &[SemanticAncestryNode]) -> Self {
        let mut by_identity = HashMap::<SemanticNodeIdentity, Vec<usize>>::new();
        let mut by_parent = HashMap::<SemanticNodeIdentity, Vec<usize>>::new();
        let mut by_ax_id = HashMap::<String, Vec<usize>>::new();
        for (idx, node) in nodes.iter().enumerate() {
            by_identity
                .entry(SemanticNodeIdentity {
                    ax_id: node.ax_id.clone(),
                    frame: node.frame.clone(),
                })
                .or_default()
                .push(idx);
            if let Some(parent_ax_id) = &node.parent_ax_id {
                by_parent
                    .entry(SemanticNodeIdentity {
                        ax_id: parent_ax_id.clone(),
                        frame: node.frame.clone(),
                    })
                    .or_default()
                    .push(idx);
            }
            by_ax_id.entry(node.ax_id.clone()).or_default().push(idx);
        }
        Self {
            by_identity,
            by_parent,
            by_ax_id,
        }
    }

    fn nodes_for(&self, identity: &SemanticNodeIdentity) -> &[usize] {
        self.by_identity
            .get(identity)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn children_of(&self, identity: &SemanticNodeIdentity) -> &[usize] {
        self.by_parent
            .get(identity)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn exists_in_other_frame(
        &self,
        nodes: &[SemanticAncestryNode],
        frame: &FrameRef,
        ax_id: &str,
    ) -> bool {
        self.by_ax_id
            .get(ax_id)
            .is_some_and(|indices| indices.iter().any(|idx| nodes[*idx].frame != *frame))
    }

    /// The root and every descendant reached through raw `childIds` in the
    /// root's frame. Callers prove the subtree with
    /// [`SemanticDocument::validate_ancestor_subtree`] first.
    fn descendant_identities(
        &self,
        nodes: &[SemanticAncestryNode],
        root: &SemanticNodeIdentity,
    ) -> HashSet<SemanticNodeIdentity> {
        let mut allowed = HashSet::new();
        let mut stack = vec![root.clone()];
        while let Some(identity) = stack.pop() {
            let [idx] = self.nodes_for(&identity) else {
                continue;
            };
            let current = &nodes[*idx];
            if !allowed.insert(identity) {
                continue;
            }
            stack.extend(
                current
                    .child_ax_ids
                    .iter()
                    .map(|child_ax_id| SemanticNodeIdentity {
                        ax_id: child_ax_id.clone(),
                        frame: current.frame.clone(),
                    }),
            );
        }
        allowed
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OmissionCounts {
    pub(crate) css_hidden: usize,
    pub(crate) offscreen: usize,
    pub(crate) page_occluded: usize,
    pub(crate) no_layout: usize,
    pub(crate) unknown: usize,
    pub(crate) budget: usize,
    pub(crate) unprovable_frame: usize,
    pub(crate) not_rendered: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct SemanticPage {
    pub(crate) outline: String,
    pub(crate) selected: Vec<SemanticNode>,
    pub(crate) selected_nodes: usize,
    pub(crate) total_nodes: usize,
    pub(crate) next_offset: Option<usize>,
    pub(crate) omissions: OmissionCounts,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SemanticDocument {
    pub(crate) nodes: Vec<SemanticNode>,
    ancestry: Vec<SemanticAncestryNode>,
    pub(crate) css_hidden_dom_count: usize,
    pub(crate) unprovable_frame_count: usize,
    pub(crate) not_rendered_count: usize,
    pub(crate) complete: bool,
}

impl SemanticDocument {
    pub(crate) fn document_title(&self) -> Option<&str> {
        self.nodes
            .iter()
            .find(|node| matches!(node.role.as_str(), "rootwebarea" | "webarea"))
            .and_then(|node| node.name.as_deref())
    }

    pub(crate) fn extend(&mut self, mut other: Self) {
        let was_empty = self.nodes.is_empty();
        let offset = self.nodes.len();
        for node in &mut other.nodes {
            node.document_order += offset;
        }
        self.nodes.extend(other.nodes);
        self.ancestry.extend(other.ancestry);
        self.css_hidden_dom_count += other.css_hidden_dom_count;
        self.unprovable_frame_count += other.unprovable_frame_count;
        self.not_rendered_count += other.not_rendered_count;
        self.complete = if was_empty {
            other.complete
        } else {
            self.complete && other.complete
        };
    }

    pub(crate) fn page(
        &self,
        offset: usize,
        budget: usize,
        query: Option<&str>,
        scope: Option<SemanticScope<'_>>,
    ) -> SemanticPage {
        let tree = SemanticTreeIndex::new(&self.nodes);
        let allowed = match scope {
            None => None,
            Some(SemanticScope::Backend(backend_node_id)) => {
                Some(backend_subtree(&self.nodes, backend_node_id))
            }
            Some(SemanticScope::Ancestor(root)) => Some(self.ancestor_subtree(&tree, root)),
        };
        let mut candidates = scoped_indices(&self.nodes, query, allowed.as_ref());
        candidates.retain(|idx| {
            !matches!(
                self.nodes[*idx].visibility,
                BrowserVisibility::CssHidden | BrowserVisibility::PageOccluded
            )
        });
        candidates.sort_by_key(|idx| {
            (
                Reverse(query.map_or(0, |query| query_score(&self.nodes[*idx], query))),
                rank(&self.nodes[*idx]),
                self.nodes[*idx].document_order,
            )
        });

        let start = offset.min(candidates.len());
        let end = (start + budget.max(1)).min(candidates.len());
        let page_slice = &candidates[start..end];
        let outline = match scope {
            Some(SemanticScope::Ancestor(root)) => {
                let parent_of = |idx: usize| framed_parent(&self.nodes, &tree, idx);
                let anchor = tree.nodes_for(root).first().copied();
                let selected = with_ancestors(page_slice, parent_of, anchor);
                render_outline(&self.nodes, &selected, parent_of)
            }
            _ => {
                let by_ax_id: HashMap<&str, usize> = self
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(idx, node)| (node.ax_id.as_str(), idx))
                    .collect();
                let parent_of = |idx: usize| {
                    let parent = self.nodes[idx].parent_ax_id.as_deref()?;
                    by_ax_id.get(parent).copied()
                };
                let selected = with_ancestors(page_slice, parent_of, None);
                render_outline(&self.nodes, &selected, parent_of)
            }
        };
        let selected_nodes = page_slice
            .iter()
            .map(|idx| self.nodes[*idx].clone())
            .collect::<Vec<_>>();
        let semantic_css_hidden = self
            .nodes
            .iter()
            .filter(|node| node.visibility == BrowserVisibility::CssHidden)
            .count();
        let mut omissions = OmissionCounts {
            css_hidden: self.css_hidden_dom_count.max(semantic_css_hidden),
            unprovable_frame: self.unprovable_frame_count,
            not_rendered: self.not_rendered_count,
            ..Default::default()
        };
        for node in &self.nodes {
            match node.visibility {
                BrowserVisibility::CssHidden => {}
                BrowserVisibility::Offscreen => omissions.offscreen += 1,
                BrowserVisibility::PageOccluded => omissions.page_occluded += 1,
                BrowserVisibility::NoLayout => omissions.no_layout += 1,
                BrowserVisibility::Unknown => omissions.unknown += 1,
                BrowserVisibility::InViewport | BrowserVisibility::NearViewport => {}
            }
        }
        omissions.budget = candidates.len().saturating_sub(end);

        SemanticPage {
            outline,
            selected: selected_nodes,
            selected_nodes: page_slice.len(),
            total_nodes: candidates.len(),
            next_offset: (end < candidates.len()).then_some(end),
            omissions,
        }
    }

    /// Resolve a live semantic ref to the nearest strict ancestor with the
    /// requested lowercase role. Every step stays in the source ref's exact
    /// frame; malformed or ambiguous ancestry refuses instead of widening the
    /// search.
    pub(crate) fn resolve_ancestor_scope(
        &self,
        source_ref: &str,
        source: &RefEntry,
        requested_role: &str,
    ) -> Result<SemanticScopeAnchor, SemanticScopeFailure> {
        let tree = SemanticTreeIndex::new(&self.nodes);
        let source_idx = match tree.nodes_for_backend(&source.frame, source.backend_node_id) {
            [] => return Err(SemanticScopeFailure::SourceMissing),
            [idx] => *idx,
            _ => return Err(SemanticScopeFailure::Ambiguous),
        };
        let source_frame = self.nodes[source_idx].frame.clone();
        let source_node = &self.nodes[source_idx];
        if !source.semantic {
            return Err(SemanticScopeFailure::SourceNotSemantic);
        }
        if source_node.role != source.node_name || source_node.name != source.label {
            return Err(SemanticScopeFailure::SourceChanged);
        }
        if source_frame.identity.is_none() {
            return Err(SemanticScopeFailure::UnprovenFrame);
        }
        let ancestry = SemanticAncestryIndex::new(&self.ancestry);
        let source_identity = SemanticNodeIdentity {
            ax_id: source_node.ax_id.clone(),
            frame: source_frame.clone(),
        };
        // A DOM-supplemented source has no raw AX node, so its first edge
        // would be a DOM parent, not a proven AX one.
        let mut parent_ax_id = match ancestry.nodes_for(&source_identity) {
            [] => return Err(SemanticScopeFailure::BrokenParent),
            [idx] => self.ancestry[*idx].parent_ax_id.clone(),
            _ => return Err(SemanticScopeFailure::Ambiguous),
        };
        let mut visited = HashSet::from([source_identity]);
        let mut distance = 0;
        loop {
            let Some(parent_id) = parent_ax_id.take() else {
                return Err(SemanticScopeFailure::AncestorMissing);
            };
            let parent_identity = SemanticNodeIdentity {
                ax_id: parent_id.clone(),
                frame: source_frame.clone(),
            };
            if !visited.insert(parent_identity.clone()) {
                return Err(SemanticScopeFailure::Cycle);
            }
            let parent_idx = match ancestry.nodes_for(&parent_identity) {
                [] => {
                    if ancestry.exists_in_other_frame(&self.ancestry, &source_frame, &parent_id) {
                        return Err(SemanticScopeFailure::CrossFrame);
                    }
                    return Err(SemanticScopeFailure::BrokenParent);
                }
                [idx] => *idx,
                _ => return Err(SemanticScopeFailure::Ambiguous),
            };
            distance += 1;
            match tree.nodes_for(&parent_identity) {
                [idx] => {
                    let node = &self.nodes[*idx];
                    if node.role == requested_role {
                        return Ok(SemanticScopeAnchor {
                            source_ref: source_ref.to_owned(),
                            identity: parent_identity,
                            role: node.role.clone(),
                            name: node.name.clone(),
                            distance,
                        });
                    }
                }
                [] => {}
                _ => return Err(SemanticScopeFailure::Ambiguous),
            }
            parent_ax_id = self.ancestry[parent_idx].parent_ax_id.clone();
        }
    }

    /// Indices inside a proven ancestor scope: the raw AX descendants of
    /// `root` in its frame, plus DOM-supplemented actions whose same-frame
    /// parent is already inside.
    fn ancestor_subtree(
        &self,
        tree: &SemanticTreeIndex,
        root: &SemanticNodeIdentity,
    ) -> HashSet<usize> {
        let ancestry = SemanticAncestryIndex::new(&self.ancestry);
        let mut identities = ancestry.descendant_identities(&self.ancestry, root);
        let mut allowed: HashSet<usize> = identities
            .iter()
            .filter_map(|identity| match tree.nodes_for(identity) {
                [idx] => Some(*idx),
                _ => None,
            })
            .collect();
        // A supplemented action has no raw AX node. Admit it only when its
        // identity is unique and its parent is already inside; repeat so a
        // supplement under a supplement is reached.
        loop {
            let additions = self
                .nodes
                .iter()
                .enumerate()
                .filter_map(|(idx, node)| {
                    if allowed.contains(&idx) {
                        return None;
                    }
                    let identity = SemanticNodeIdentity {
                        ax_id: node.ax_id.clone(),
                        frame: node.frame.clone(),
                    };
                    if !ancestry.nodes_for(&identity).is_empty()
                        || tree.nodes_for(&identity).len() != 1
                    {
                        return None;
                    }
                    let parent = SemanticNodeIdentity {
                        ax_id: node.parent_ax_id.clone()?,
                        frame: node.frame.clone(),
                    };
                    identities.contains(&parent).then_some((idx, identity))
                })
                .collect::<Vec<_>>();
            if additions.is_empty() {
                break;
            }
            for (idx, identity) in additions {
                allowed.insert(idx);
                identities.insert(identity);
            }
        }
        allowed
    }

    /// Prove that an ancestor-scoped subtree is one unambiguous same-frame AX
    /// tree before a query can mint refs from it. Legacy direct `scope_ref`
    /// reads keep their existing best-effort traversal; the new ancestor
    /// contract is deliberately fail closed.
    pub(crate) fn validate_ancestor_subtree(
        &self,
        root: &SemanticNodeIdentity,
    ) -> Result<(), SemanticScopeFailure> {
        let tree = SemanticTreeIndex::new(&self.nodes);
        match tree.nodes_for(root) {
            [] => return Err(SemanticScopeFailure::SourceMissing),
            [_] => {}
            _ => return Err(SemanticScopeFailure::Ambiguous),
        }
        let ancestry = SemanticAncestryIndex::new(&self.ancestry);
        match ancestry.nodes_for(root) {
            [] => return Err(SemanticScopeFailure::SourceMissing),
            [_] => {}
            _ => return Err(SemanticScopeFailure::Ambiguous),
        }

        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        let mut stack = vec![(root.clone(), true)];
        while let Some((identity, entering)) = stack.pop() {
            if !entering {
                visiting.remove(&identity);
                visited.insert(identity);
                continue;
            }
            if visiting.contains(&identity) {
                return Err(SemanticScopeFailure::Cycle);
            }
            if visited.contains(&identity) {
                return Err(SemanticScopeFailure::Ambiguous);
            }
            visiting.insert(identity.clone());
            stack.push((identity.clone(), false));

            if tree.nodes_for(&identity).len() > 1 {
                return Err(SemanticScopeFailure::Ambiguous);
            }
            let node_idx = match ancestry.nodes_for(&identity) {
                [idx] => *idx,
                [] => return Err(SemanticScopeFailure::BrokenParent),
                _ => return Err(SemanticScopeFailure::Ambiguous),
            };
            let node = &self.ancestry[node_idx];
            let mut declared_children = HashSet::new();
            for child_ax_id in &node.child_ax_ids {
                if !declared_children.insert(child_ax_id.as_str()) {
                    return Err(SemanticScopeFailure::Ambiguous);
                }
                let child_identity = SemanticNodeIdentity {
                    ax_id: child_ax_id.clone(),
                    frame: identity.frame.clone(),
                };
                let child_idx = match ancestry.nodes_for(&child_identity) {
                    [] => {
                        if ancestry.exists_in_other_frame(
                            &self.ancestry,
                            &identity.frame,
                            child_ax_id,
                        ) {
                            return Err(SemanticScopeFailure::CrossFrame);
                        }
                        return Err(SemanticScopeFailure::BrokenParent);
                    }
                    [child_idx] => *child_idx,
                    _ => return Err(SemanticScopeFailure::Ambiguous),
                };
                if self.ancestry[child_idx].parent_ax_id.as_deref() != Some(node.ax_id.as_str()) {
                    return Err(SemanticScopeFailure::BrokenParent);
                }
                stack.push((child_identity, true));
            }

            // A raw node that claims this parent but is absent from the
            // parent's childIds would otherwise enter a by-parent subtree and
            // widen the scope. DOM-supplemented actions are intentionally not
            // present in the raw graph and are handled separately by page().
            for child_idx in ancestry.children_of(&identity) {
                let child = &self.ancestry[*child_idx];
                let child_identity = SemanticNodeIdentity {
                    ax_id: child.ax_id.clone(),
                    frame: child.frame.clone(),
                };
                if ancestry.nodes_for(&child_identity).len() > 1 {
                    return Err(SemanticScopeFailure::Ambiguous);
                }
                if !declared_children.contains(child.ax_id.as_str()) {
                    return Err(SemanticScopeFailure::BrokenParent);
                }
            }
        }
        Ok(())
    }
}

impl DomIndex {
    fn is_ancestor_of(&self, ancestor: i64, mut descendant: i64) -> bool {
        let mut visited = HashSet::new();
        while visited.insert(descendant) {
            let Some(parent) = self
                .nodes
                .get(&descendant)
                .and_then(|node| node.parent_backend_node_id)
            else {
                return false;
            };
            if parent == ancestor {
                return true;
            }
            descendant = parent;
        }
        false
    }

    fn shares_dom_branch(&self, left: i64, right: i64) -> bool {
        self.is_ancestor_of(left, right) || self.is_ancestor_of(right, left)
    }
}

pub(crate) fn build_dom_index(root: &Value) -> DomIndex {
    fn walk(
        node: &Value,
        inherited_hidden: bool,
        parent_backend_node_id: Option<i64>,
        inherited_frame_id: Option<&str>,
        order: &mut usize,
        index: &mut DomIndex,
    ) {
        let node_type = node.get("nodeType").and_then(Value::as_i64).unwrap_or(0);
        let attrs = attributes(node);
        let hidden = inherited_hidden || statically_hidden(&attrs);
        let transparent = statically_transparent(&attrs)
            || parent_backend_node_id
                .and_then(|parent| index.nodes.get(&parent))
                .is_some_and(|parent| parent.transparent);
        let backend_node_id = node.get("backendNodeId").and_then(Value::as_i64);
        let frame_id = if node_type == 9 {
            node.get("frameId").and_then(Value::as_str)
        } else {
            inherited_frame_id
        };
        if let Some(backend) = backend_node_id {
            let tag = node
                .get("nodeName")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            if node_type == 1 && hidden && !inherited_hidden {
                index.css_hidden_count += 1;
            }
            index.nodes.insert(
                backend,
                DomMeta {
                    tag,
                    attrs,
                    order: *order,
                    css_hidden: hidden,
                    transparent,
                    parent_backend_node_id,
                    frame_id: frame_id.map(str::to_owned),
                },
            );
            *order += 1;
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for child in children {
                walk(
                    child,
                    hidden,
                    backend_node_id.or(parent_backend_node_id),
                    frame_id,
                    order,
                    index,
                );
            }
        }
        if let Some(shadow_roots) = node.get("shadowRoots").and_then(Value::as_array) {
            for shadow_root in shadow_roots {
                if shadow_root.get("shadowRootType").and_then(Value::as_str) == Some("user-agent") {
                    continue;
                }
                walk(
                    shadow_root,
                    hidden,
                    backend_node_id.or(parent_backend_node_id),
                    frame_id,
                    order,
                    index,
                );
            }
        }
        if let Some(content_document) = node.get("contentDocument") {
            walk(
                content_document,
                hidden,
                backend_node_id.or(parent_backend_node_id),
                content_document.get("frameId").and_then(Value::as_str),
                order,
                index,
            );
        }
    }

    let mut index = DomIndex::default();
    let mut order = 0;
    walk(
        root,
        false,
        None,
        root.get("frameId").and_then(Value::as_str),
        &mut order,
        &mut index,
    );
    index
}

pub(crate) fn build_layout_index(snapshot: &Value) -> LayoutIndex {
    let Some(strings) = snapshot.get("strings").and_then(Value::as_array) else {
        return LayoutIndex::default();
    };
    let string_at = |idx: i64| -> Option<String> {
        usize::try_from(idx)
            .ok()
            .and_then(|idx| strings.get(idx))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };

    let mut out = LayoutIndex::default();
    let Some(documents) = snapshot.get("documents").and_then(Value::as_array) else {
        return out;
    };
    for document in documents {
        let Some(backend_ids) = document
            .pointer("/nodes/backendNodeId")
            .and_then(Value::as_array)
        else {
            continue;
        };
        let layout = document.get("layout").unwrap_or(&Value::Null);
        let node_indices = layout
            .get("nodeIndex")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let bounds = layout
            .get("bounds")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let styles = layout
            .get("styles")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let paint_orders = layout
            .get("paintOrders")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let client_rects = layout
            .get("clientRects")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let scroll_rects = layout
            .get("scrollRects")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for (layout_idx, node_index) in node_indices.iter().enumerate() {
            let Some(node_index) = node_index.as_u64().and_then(|v| usize::try_from(v).ok()) else {
                continue;
            };
            let Some(backend) = backend_ids.get(node_index).and_then(Value::as_i64) else {
                continue;
            };
            let mut computed = HashMap::new();
            if let Some(style_indices) = styles.get(layout_idx).and_then(Value::as_array) {
                for (name, value_idx) in SEMANTIC_COMPUTED_STYLES
                    .iter()
                    .zip(style_indices.iter().filter_map(Value::as_i64))
                {
                    if let Some(value) = string_at(value_idx) {
                        computed.insert((*name).to_owned(), value);
                    }
                }
            }
            out.nodes.insert(
                backend,
                LayoutMeta {
                    bounds: bounds.get(layout_idx).and_then(Rect::from_value),
                    client_rect: client_rects.get(layout_idx).and_then(Rect::from_value),
                    scroll_rect: scroll_rects.get(layout_idx).and_then(Rect::from_value),
                    styles: computed,
                    paint_order: paint_orders.get(layout_idx).and_then(Value::as_i64),
                },
            );
        }
    }
    out
}

pub(crate) fn parse_viewport(metrics: &Value) -> Viewport {
    let viewport = metrics
        .get("cssVisualViewport")
        .or_else(|| metrics.get("visualViewport"));
    let Some(viewport) = viewport else {
        return Viewport::default();
    };
    let x = viewport
        .get("pageX")
        .or_else(|| viewport.get("offsetX"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let y = viewport
        .get("pageY")
        .or_else(|| viewport.get("offsetY"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let width = viewport.get("clientWidth").and_then(Value::as_f64);
    let height = viewport.get("clientHeight").and_then(Value::as_f64);
    Viewport {
        rect: width.zip(height).map(|(width, height)| Rect {
            x,
            y,
            width,
            height,
        }),
    }
}

pub(crate) fn compose_accessibility_tree(
    ax_tree: &Value,
    dom: &DomIndex,
    layout: &LayoutIndex,
    viewport: &Viewport,
    frame: FrameRef,
) -> SemanticDocument {
    let Some(ax_nodes) = ax_tree.get("nodes").and_then(Value::as_array) else {
        return SemanticDocument {
            complete: false,
            css_hidden_dom_count: dom.css_hidden_count,
            ..Default::default()
        };
    };

    // Preserve the complete raw AX ancestry before presentation-oriented
    // filtering removes ignored wrappers, inline text boxes, or redundant
    // static text. Chromium reports parent/child edges through those wrappers.
    let ancestry = ax_nodes
        .iter()
        .filter_map(|ax| {
            let ax_id = ax.get("nodeId").and_then(Value::as_str)?.to_owned();
            if ax_id.is_empty() {
                return None;
            }
            Some(SemanticAncestryNode {
                ax_id,
                parent_ax_id: ax
                    .get("parentId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                child_ax_ids: ax
                    .get("childIds")
                    .and_then(Value::as_array)
                    .map(|ids| {
                        ids.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                frame: frame.clone(),
            })
        })
        .collect();

    let mut nodes = Vec::new();
    for (fallback_order, ax) in ax_nodes.iter().enumerate() {
        if ax.get("ignored").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let ax_id = ax
            .get("nodeId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if ax_id.is_empty() {
            continue;
        }
        let role = ax_value_string(ax.get("role"))
            .unwrap_or_else(|| "unknown".to_owned())
            .to_ascii_lowercase();
        if role == "inlinetextbox" {
            continue;
        }
        let backend_node_id = ax.get("backendDOMNodeId").and_then(Value::as_i64);
        let dom_meta = backend_node_id.and_then(|backend| dom.nodes.get(&backend));
        let layout_meta = backend_node_id.and_then(|backend| layout.nodes.get(&backend));
        let states = ax_states(ax);
        let visibility = classify_visibility(dom_meta, layout_meta, viewport);
        let actions = action_kinds(&role, dom_meta, &states, layout_meta);
        let name = ax_value_string(ax.get("name")).and_then(clean_semantic_text);
        let value = ax_value_string(ax.get("value")).and_then(clean_semantic_text);
        let document_order = dom_meta.map_or(fallback_order, |meta| meta.order);
        nodes.push(SemanticNode {
            ax_id,
            parent_ax_id: ax
                .get("parentId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            child_ax_ids: ax
                .get("childIds")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            backend_node_id,
            role,
            name,
            value,
            states,
            frame: frame.clone(),
            visibility,
            actions,
            document_order,
        });
    }

    supplement_dom_actions(&mut nodes, dom, layout, viewport, &frame);
    apply_page_occlusion(&mut nodes, dom, layout);
    remove_redundant_static_text(&mut nodes);
    SemanticDocument {
        nodes,
        ancestry,
        css_hidden_dom_count: dom.css_hidden_count,
        unprovable_frame_count: 0,
        not_rendered_count: 0,
        complete: true,
    }
}

fn apply_page_occlusion(nodes: &mut [SemanticNode], dom: &DomIndex, layout: &LayoutIndex) {
    for node in nodes {
        if node.visibility != BrowserVisibility::InViewport {
            continue;
        }
        let Some(target_backend) = node.backend_node_id else {
            continue;
        };
        let Some(target) = layout.nodes.get(&target_backend) else {
            continue;
        };
        let (Some(target_bounds), Some(target_paint)) = (target.bounds, target.paint_order) else {
            continue;
        };
        let covered = layout.nodes.iter().any(|(&backend, overlay)| {
            if backend == target_backend
                || dom.shares_dom_branch(backend, target_backend)
                || overlay
                    .paint_order
                    .is_none_or(|paint| paint <= target_paint)
                || !overlay
                    .styles
                    .get("position")
                    .is_some_and(|position| matches!(position.as_str(), "fixed" | "absolute"))
                || overlay
                    .styles
                    .get("pointer-events")
                    .is_some_and(|value| value == "none")
                || layout_hidden(overlay)
            {
                return false;
            }
            overlay
                .bounds
                .is_some_and(|bounds| bounds.has_area() && bounds.covers(target_bounds))
        });
        if covered {
            node.visibility = BrowserVisibility::PageOccluded;
        }
    }
}

fn supplement_dom_actions(
    nodes: &mut Vec<SemanticNode>,
    dom: &DomIndex,
    layout: &LayoutIndex,
    viewport: &Viewport,
    frame: &FrameRef,
) {
    let existing = nodes
        .iter()
        .filter_map(|node| node.backend_node_id)
        .collect::<HashSet<_>>();
    let expected_frame = frame
        .identity
        .as_ref()
        .map(|identity| identity.frame_id.as_str());
    let by_backend = nodes
        .iter()
        .filter_map(|node| Some((node.backend_node_id?, node.ax_id.clone())))
        .collect::<HashMap<_, _>>();
    let mut candidates = dom.nodes.iter().collect::<Vec<_>>();
    candidates.sort_by_key(|(_, meta)| meta.order);
    for (&backend_node_id, meta) in candidates {
        if existing.contains(&backend_node_id)
            || expected_frame.is_some_and(|expected| meta.frame_id.as_deref() != Some(expected))
        {
            continue;
        }
        let layout_meta = layout.nodes.get(&backend_node_id);
        let visibility = classify_visibility(Some(meta), layout_meta, viewport);
        if matches!(
            visibility,
            BrowserVisibility::CssHidden | BrowserVisibility::PageOccluded
        ) {
            continue;
        }
        let role = meta
            .attrs
            .get("role")
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_else(|| match meta.tag.as_str() {
                "a" => "link".to_owned(),
                "button" => "button".to_owned(),
                "input" => match meta.attrs.get("type").map(String::as_str) {
                    Some("checkbox") => "checkbox".to_owned(),
                    Some("radio") => "radio".to_owned(),
                    Some("button" | "submit" | "reset" | "image") => "button".to_owned(),
                    Some("range") => "slider".to_owned(),
                    _ => "textbox".to_owned(),
                },
                "textarea" => "textbox".to_owned(),
                "select" => "combobox".to_owned(),
                "option" => "option".to_owned(),
                "summary" => "summary".to_owned(),
                _ => "generic".to_owned(),
            });
        let mut states = BTreeMap::new();
        if meta.attrs.contains_key("disabled") {
            states.insert("disabled".to_owned(), Value::Bool(true));
        }
        let actions = action_kinds(&role, Some(meta), &states, layout_meta);
        if actions.is_empty() {
            continue;
        }
        let name = ["aria-label", "placeholder", "title", "name", "id"]
            .iter()
            .find_map(|key| meta.attrs.get(*key).cloned())
            .and_then(clean_semantic_text);
        nodes.push(SemanticNode {
            ax_id: format!("dom-{backend_node_id}"),
            parent_ax_id: meta
                .parent_backend_node_id
                .and_then(|parent| by_backend.get(&parent).cloned()),
            child_ax_ids: Vec::new(),
            backend_node_id: Some(backend_node_id),
            role,
            name,
            value: meta
                .attrs
                .get("value")
                .cloned()
                .and_then(clean_semantic_text),
            states,
            frame: frame.clone(),
            visibility,
            actions,
            document_order: meta.order,
        });
    }
}

fn attributes(node: &Value) -> HashMap<String, String> {
    node.get("attributes")
        .and_then(Value::as_array)
        .map(|attrs| {
            attrs
                .chunks_exact(2)
                .filter_map(|pair| {
                    Some((
                        pair[0].as_str()?.to_ascii_lowercase(),
                        pair[1].as_str()?.to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn statically_hidden(attrs: &HashMap<String, String>) -> bool {
    if attrs.contains_key("hidden") || attrs.get("aria-hidden").is_some_and(|v| v == "true") {
        return true;
    }
    inline_style(attrs).any(|(property, value)| hiding_style(property, value))
}

fn statically_transparent(attrs: &HashMap<String, String>) -> bool {
    inline_style(attrs).any(|(property, value)| {
        property.trim().eq_ignore_ascii_case("opacity") && hiding_style(property, value)
    })
}

fn inline_style(attrs: &HashMap<String, String>) -> impl Iterator<Item = (&str, &str)> {
    attrs
        .get("style")
        .into_iter()
        .flat_map(|style| style.split(';'))
        .filter_map(|declaration| declaration.split_once(':'))
}

fn hiding_style(property: &str, value: &str) -> bool {
    let value = value.trim().trim_end_matches("!important").trim_end();
    match property.trim().to_ascii_lowercase().as_str() {
        "display" => value.eq_ignore_ascii_case("none"),
        "visibility" => value.eq_ignore_ascii_case("hidden"),
        "opacity" => value.parse::<f64>().is_ok_and(|opacity| opacity <= 0.0),
        _ => false,
    }
}

fn layout_hidden(layout: &LayoutMeta) -> bool {
    layout
        .styles
        .iter()
        .any(|(property, value)| hiding_style(property, value))
}

fn classify_visibility(
    dom: Option<&DomMeta>,
    layout: Option<&LayoutMeta>,
    viewport: &Viewport,
) -> BrowserVisibility {
    let rendered = layout.is_some_and(|layout| layout.bounds.is_some_and(Rect::has_area));
    if dom.is_some_and(|meta| meta.transparent || (meta.css_hidden && !rendered))
        || layout.is_some_and(layout_hidden)
    {
        return BrowserVisibility::CssHidden;
    }
    let Some(layout) = layout else {
        return BrowserVisibility::Unknown;
    };
    let Some(bounds) = layout.bounds else {
        return BrowserVisibility::NoLayout;
    };
    if !bounds.has_area() {
        return BrowserVisibility::NoLayout;
    }
    let Some(viewport) = viewport.rect else {
        return BrowserVisibility::Unknown;
    };
    if bounds.intersects(viewport) {
        BrowserVisibility::InViewport
    } else if bounds.intersects(viewport.expanded(NEAR_VIEWPORT_MARGIN)) {
        BrowserVisibility::NearViewport
    } else {
        BrowserVisibility::Offscreen
    }
}

fn action_kinds(
    role: &str,
    dom: Option<&DomMeta>,
    states: &BTreeMap<String, Value>,
    layout: Option<&LayoutMeta>,
) -> Vec<BrowserActionKind> {
    if states.get("disabled").and_then(Value::as_bool) == Some(true) {
        return Vec::new();
    }
    let mut actions = Vec::new();
    if matches!(
        role,
        "button"
            | "link"
            | "checkbox"
            | "radio"
            | "switch"
            | "tab"
            | "menuitem"
            | "menuitemcheckbox"
            | "menuitemradio"
            | "option"
            | "treeitem"
            | "slider"
            | "spinbutton"
            | "combobox"
            | "listbox"
            | "summary"
    ) {
        actions.push(BrowserActionKind::Click);
    }
    let tag = dom.map(|meta| meta.tag.as_str()).unwrap_or("");
    let editable = states.get("editable").is_some_and(|value| {
        value.as_bool() == Some(true) || value.as_str().is_some_and(|value| value != "false")
    }) || dom.is_some_and(|meta| {
        meta.attrs
            .get("contenteditable")
            .is_some_and(|value| value.is_empty() || value.eq_ignore_ascii_case("true"))
    });
    let file_input = tag == "input"
        && dom.is_some_and(|meta| {
            meta.attrs
                .get("type")
                .is_some_and(|value| value.eq_ignore_ascii_case("file"))
        });
    if file_input {
        actions.push(BrowserActionKind::Upload);
    } else if matches!(role, "textbox" | "searchbox")
        || matches!(tag, "input" | "textarea")
        || editable
    {
        actions.push(BrowserActionKind::Type);
    }
    if actions.is_empty()
        && dom.is_some_and(|meta| {
            meta.attrs.contains_key("onclick")
                || meta
                    .attrs
                    .get("tabindex")
                    .is_some_and(|value| value != "-1")
                || (!matches!(meta.tag.as_str(), "html" | "body")
                    && layout.is_some_and(|layout| {
                        layout
                            .styles
                            .get("cursor")
                            .is_some_and(|cursor| cursor.eq_ignore_ascii_case("pointer"))
                            && !layout
                                .styles
                                .get("pointer-events")
                                .is_some_and(|value| value.eq_ignore_ascii_case("none"))
                    }))
        })
        && layout.is_some_and(|meta| meta.bounds.is_some_and(Rect::has_area))
    {
        actions.push(BrowserActionKind::Click);
    }
    if let Some(layout) = layout.filter(|meta| {
        meta.bounds.is_some_and(Rect::has_area)
            && !meta
                .styles
                .get("pointer-events")
                .is_some_and(|value| value.eq_ignore_ascii_case("none"))
    }) {
        if !actions.is_empty() {
            actions.push(BrowserActionKind::Pointer);
        }
        if layout_is_scrollable(layout) {
            actions.push(BrowserActionKind::Scroll);
        }
    }
    actions
}

fn layout_is_scrollable(layout: &LayoutMeta) -> bool {
    let (Some(client), Some(scroll)) = (layout.client_rect, layout.scroll_rect) else {
        return false;
    };
    let scrollable_axis = |overflow: Option<&String>, scroll_extent: f64, client_extent: f64| {
        overflow.is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "auto" | "scroll" | "overlay"
            )
        }) && scroll_extent > client_extent + 0.5
    };
    scrollable_axis(layout.styles.get("overflow-x"), scroll.width, client.width)
        || scrollable_axis(
            layout.styles.get("overflow-y"),
            scroll.height,
            client.height,
        )
}

fn ax_states(node: &Value) -> BTreeMap<String, Value> {
    const KEPT: &[&str] = &[
        "checked",
        "disabled",
        "editable",
        "expanded",
        "focused",
        "focusable",
        "pressed",
        "required",
        "selected",
    ];
    node.get("properties")
        .and_then(Value::as_array)
        .map(|properties| {
            properties
                .iter()
                .filter_map(|property| {
                    let name = property.get("name").and_then(Value::as_str)?;
                    KEPT.contains(&name).then(|| {
                        (
                            name.to_owned(),
                            property
                                .pointer("/value/value")
                                .cloned()
                                .unwrap_or(Value::Null),
                        )
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn ax_value_string(value: Option<&Value>) -> Option<String> {
    let value = value?.get("value")?;
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn clean_semantic_text(value: String) -> Option<String> {
    let normalized = value
        .chars()
        .map(|ch| match ch {
            '\u{feff}' | '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}' | '\u{00a0}'
            | '\u{2007}' | '\u{202f}' => ' ',
            '\u{e000}'..='\u{f8ff}' => ' ',
            _ => ch,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        return None;
    }
    Some(normalized.chars().take(MAX_SEMANTIC_TEXT_CHARS).collect())
}

fn remove_redundant_static_text(nodes: &mut Vec<SemanticNode>) {
    let names: HashMap<String, String> = nodes
        .iter()
        .filter_map(|node| node.name.clone().map(|name| (node.ax_id.clone(), name)))
        .collect();
    nodes.retain(|node| {
        if node.role != "statictext" && node.role != "text" {
            return true;
        }
        let Some(name) = node.name.as_deref() else {
            return false;
        };
        node.parent_ax_id
            .as_ref()
            .and_then(|parent| names.get(parent))
            .is_none_or(|parent_name| parent_name != name)
    });
}

fn rank(node: &SemanticNode) -> u8 {
    let priority_context = node.states.get("focused").and_then(Value::as_bool) == Some(true)
        || matches!(node.role.as_str(), "dialog" | "alertdialog");
    if priority_context {
        return 0;
    }
    match (node.visibility, node.actions.is_empty()) {
        (BrowserVisibility::InViewport, false) => 1,
        (BrowserVisibility::InViewport, true) => 2,
        (BrowserVisibility::NearViewport, false) => 3,
        (BrowserVisibility::NearViewport, true) => 4,
        (BrowserVisibility::Unknown | BrowserVisibility::NoLayout, false) => 5,
        (BrowserVisibility::Unknown | BrowserVisibility::NoLayout, true) => 6,
        (BrowserVisibility::Offscreen, false) => 7,
        (BrowserVisibility::Offscreen, true) => 8,
        (BrowserVisibility::CssHidden | BrowserVisibility::PageOccluded, _) => 9,
    }
}

/// Nodes under the first node with `scope_backend`, following filtered
/// `childIds` by AX id. This is the direct `scope_ref` contract.
fn backend_subtree(nodes: &[SemanticNode], scope_backend: i64) -> HashSet<usize> {
    let by_ax_id: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(idx, node)| (node.ax_id.as_str(), idx))
        .collect();
    let mut allowed = HashSet::new();
    if let Some(scope) = nodes
        .iter()
        .find(|node| node.backend_node_id == Some(scope_backend))
    {
        let mut stack = vec![scope.ax_id.as_str()];
        while let Some(ax_id) = stack.pop() {
            if !allowed.insert(ax_id) {
                continue;
            }
            if let Some(idx) = by_ax_id.get(ax_id) {
                stack.extend(nodes[*idx].child_ax_ids.iter().map(String::as_str));
            }
        }
    }
    nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| allowed.contains(node.ax_id.as_str()))
        .map(|(idx, _)| idx)
        .collect()
}

fn scoped_indices(
    nodes: &[SemanticNode],
    query: Option<&str>,
    allowed: Option<&HashSet<usize>>,
) -> Vec<usize> {
    let query = query.map(|value| value.trim().to_ascii_lowercase());
    let in_scope = |idx: usize| allowed.is_none_or(|allowed| allowed.contains(&idx));
    let exact_match_exists = query.as_ref().is_some_and(|query| {
        !query.is_empty()
            && nodes
                .iter()
                .enumerate()
                .any(|(idx, node)| in_scope(idx) && node_contains_query(node, query))
    });
    nodes
        .iter()
        .enumerate()
        .filter(|(idx, node)| {
            in_scope(*idx)
                && query.as_ref().is_none_or(|query| {
                    query.is_empty()
                        || if exact_match_exists {
                            node_contains_query(node, query)
                        } else {
                            query_score(node, query) > 0
                        }
                })
        })
        .map(|(idx, _)| idx)
        .collect()
}

fn node_contains_query(node: &SemanticNode, query: &str) -> bool {
    node.role.to_ascii_lowercase().contains(query)
        || node
            .name
            .as_ref()
            .is_some_and(|name| name.to_ascii_lowercase().contains(query))
        || node
            .value
            .as_ref()
            .is_some_and(|value| value.to_ascii_lowercase().contains(query))
}

fn query_score(node: &SemanticNode, query: &str) -> usize {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return 0;
    }
    if node_contains_query(node, &query) {
        return usize::MAX;
    }
    let fields = [
        Some(node.role.as_str()),
        node.name.as_deref(),
        node.value.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::to_ascii_lowercase)
    .collect::<Vec<_>>();
    query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .filter(|term| fields.iter().any(|field| field.contains(term)))
        .count()
}

/// The parent of `idx` in its own frame, or `None` when that parent is
/// absent or ambiguous.
fn framed_parent(nodes: &[SemanticNode], tree: &SemanticTreeIndex, idx: usize) -> Option<usize> {
    let identity = SemanticNodeIdentity {
        ax_id: nodes[idx].parent_ax_id.clone()?,
        frame: nodes[idx].frame.clone(),
    };
    match tree.nodes_for(&identity) {
        [parent] => Some(*parent),
        _ => None,
    }
}

/// Add each selected node's ancestors for outline context, stopping at
/// `anchor` when one is given.
fn with_ancestors(
    selected: &[usize],
    parent_of: impl Fn(usize) -> Option<usize>,
    anchor: Option<usize>,
) -> HashSet<usize> {
    let mut keep: HashSet<usize> = selected.iter().copied().collect();
    for idx in selected {
        if Some(*idx) == anchor {
            continue;
        }
        let mut parent = parent_of(*idx);
        while let Some(parent_idx) = parent {
            if !keep.insert(parent_idx) || Some(parent_idx) == anchor {
                break;
            }
            parent = parent_of(parent_idx);
        }
    }
    keep
}

fn render_outline(
    nodes: &[SemanticNode],
    selected: &HashSet<usize>,
    parent_of: impl Fn(usize) -> Option<usize>,
) -> String {
    let mut ordered: Vec<usize> = selected.iter().copied().collect();
    ordered.sort_by_key(|idx| nodes[*idx].document_order);
    let mut lines = Vec::new();
    for idx in ordered {
        let node = &nodes[idx];
        if matches!(node.role.as_str(), "rootwebarea" | "webarea") {
            continue;
        }
        let mut depth = 0;
        let mut visited = HashSet::new();
        let mut parent = parent_of(idx);
        while let Some(parent_idx) = parent {
            if !visited.insert(parent_idx) {
                break;
            }
            if selected.contains(&parent_idx)
                && !matches!(nodes[parent_idx].role.as_str(), "rootwebarea" | "webarea")
            {
                depth += 1;
            }
            parent = parent_of(parent_idx);
        }
        let mut line = format!("{}- {}", "  ".repeat(depth), node.role);
        if let Some(name) = &node.name {
            line.push(' ');
            line.push_str(&serde_json::to_string(name).unwrap_or_else(|_| "\"\"".to_owned()));
        }
        if let Some(value) = &node.value {
            if node.name.as_deref() != Some(value) {
                line.push_str(": ");
                line.push_str(value);
            }
        }
        if !node.states.is_empty() {
            let states = node
                .states
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join(",");
            line.push_str(&format!(" [{states}]"));
        }
        lines.push(line);
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::browser::store::{FrameIdentity, FrameKind};

    #[test]
    fn file_inputs_expose_upload_instead_of_text_typing() {
        let dom = DomMeta {
            tag: "input".into(),
            attrs: HashMap::from([("type".into(), "file".into())]),
            order: 0,
            css_hidden: false,
            transparent: false,
            parent_backend_node_id: None,
            frame_id: None,
        };
        assert_eq!(
            action_kinds("textbox", Some(&dom), &BTreeMap::new(), None),
            vec![BrowserActionKind::Upload]
        );
    }
    use crate::browser::store::FrameRef;

    fn frame() -> FrameRef {
        FrameRef::main_unproven()
    }

    fn proven_frame(kind: FrameKind, frame_id: &str) -> FrameRef {
        FrameRef {
            kind,
            oopif_target_id: (kind == FrameKind::Oopif).then(|| format!("target-{frame_id}")),
            identity: Some(FrameIdentity {
                frame_id: frame_id.to_owned(),
                loader_id: format!("loader-{frame_id}"),
            }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn scope_node(
        ax_id: &str,
        parent_ax_id: Option<&str>,
        child_ax_ids: &[&str],
        backend_node_id: Option<i64>,
        role: &str,
        name: Option<&str>,
        frame: FrameRef,
        document_order: usize,
    ) -> SemanticNode {
        SemanticNode {
            ax_id: ax_id.to_owned(),
            parent_ax_id: parent_ax_id.map(str::to_owned),
            child_ax_ids: child_ax_ids.iter().map(|id| (*id).to_owned()).collect(),
            backend_node_id,
            role: role.to_owned(),
            name: name.map(str::to_owned),
            value: None,
            states: BTreeMap::new(),
            frame,
            visibility: BrowserVisibility::InViewport,
            actions: matches!(role, "button" | "link")
                .then_some(vec![BrowserActionKind::Click])
                .unwrap_or_default(),
            document_order,
        }
    }

    fn scope_document(nodes: Vec<SemanticNode>) -> SemanticDocument {
        let ancestry = nodes
            .iter()
            .map(|node| SemanticAncestryNode {
                ax_id: node.ax_id.clone(),
                parent_ax_id: node.parent_ax_id.clone(),
                child_ax_ids: node.child_ax_ids.clone(),
                frame: node.frame.clone(),
            })
            .collect();
        SemanticDocument {
            nodes,
            ancestry,
            complete: true,
            ..Default::default()
        }
    }

    #[test]
    fn ancestor_scope_selects_only_the_source_row_when_actions_repeat() {
        let main = proven_frame(FrameKind::Main, "main");
        let document = scope_document(vec![
            scope_node(
                "root",
                None,
                &["row-1", "row-2"],
                Some(1),
                "rootwebarea",
                None,
                main.clone(),
                0,
            ),
            scope_node(
                "row-1",
                Some("root"),
                &["version-1", "action-1"],
                Some(10),
                "row",
                Some("Release 0.19.89"),
                main.clone(),
                1,
            ),
            scope_node(
                "version-1",
                Some("row-1"),
                &[],
                Some(11),
                "link",
                Some("0.19.89"),
                main.clone(),
                2,
            ),
            scope_node(
                "action-1",
                Some("row-1"),
                &[],
                Some(12),
                "button",
                Some("View release options"),
                main.clone(),
                3,
            ),
            scope_node(
                "row-2",
                Some("root"),
                &["version-2", "action-2"],
                Some(20),
                "row",
                Some("Release 0.19.88"),
                main.clone(),
                4,
            ),
            scope_node(
                "version-2",
                Some("row-2"),
                &[],
                Some(21),
                "link",
                Some("0.19.88"),
                main.clone(),
                5,
            ),
            scope_node(
                "action-2",
                Some("row-2"),
                &[],
                Some(22),
                "button",
                Some("View release options"),
                main.clone(),
                6,
            ),
        ]);
        let source = document.nodes[2].to_ref_entry().unwrap();
        let anchor = document
            .resolve_ancestor_scope("p1:2", &source, "row")
            .expect("nearest row scope");
        assert_eq!(anchor.role, "row");
        assert_eq!(anchor.name.as_deref(), Some("Release 0.19.89"));
        assert_eq!(anchor.distance, 1);

        let page = document.page(
            0,
            300,
            Some("View release options"),
            Some(SemanticScope::Ancestor(&anchor.identity)),
        );
        let backends = page
            .selected
            .iter()
            .filter_map(|node| node.backend_node_id)
            .collect::<Vec<_>>();
        assert_eq!(backends, vec![12]);
    }

    #[test]
    fn ancestor_scope_uses_the_nearest_nested_role_not_the_outer_match() {
        let main = proven_frame(FrameKind::Main, "main");
        let document = scope_document(vec![
            scope_node(
                "root",
                None,
                &["outer"],
                Some(1),
                "rootwebarea",
                None,
                main.clone(),
                0,
            ),
            scope_node(
                "outer",
                Some("root"),
                &["outer-action", "inner"],
                Some(10),
                "row",
                Some("Outer"),
                main.clone(),
                1,
            ),
            scope_node(
                "outer-action",
                Some("outer"),
                &[],
                Some(11),
                "button",
                Some("View release options"),
                main.clone(),
                2,
            ),
            scope_node(
                "inner",
                Some("outer"),
                &["source", "inner-action"],
                Some(20),
                "row",
                Some("Inner"),
                main.clone(),
                3,
            ),
            scope_node(
                "source",
                Some("inner"),
                &[],
                Some(21),
                "link",
                Some("0.19.89"),
                main.clone(),
                4,
            ),
            scope_node(
                "inner-action",
                Some("inner"),
                &[],
                Some(22),
                "button",
                Some("View release options"),
                main.clone(),
                5,
            ),
        ]);
        let source = document.nodes[4].to_ref_entry().unwrap();
        let anchor = document
            .resolve_ancestor_scope("p1:4", &source, "row")
            .expect("nearest nested row");
        assert_eq!(anchor.name.as_deref(), Some("Inner"));
        assert_eq!(anchor.distance, 1);
        let page = document.page(
            0,
            300,
            Some("View release options"),
            Some(SemanticScope::Ancestor(&anchor.identity)),
        );
        assert_eq!(page.selected.len(), 1);
        assert_eq!(page.selected[0].backend_node_id, Some(22));
        assert!(!page.outline.contains("Outer"), "{}", page.outline);
    }

    #[test]
    fn ancestor_scope_refuses_missing_source_and_missing_role() {
        let main = proven_frame(FrameKind::Main, "main");
        let document = scope_document(vec![
            scope_node(
                "root",
                None,
                &["source"],
                Some(1),
                "rootwebarea",
                None,
                main.clone(),
                0,
            ),
            scope_node(
                "source",
                Some("root"),
                &[],
                Some(2),
                "link",
                Some("version"),
                main.clone(),
                1,
            ),
        ]);
        let source = document.nodes[1].to_ref_entry().unwrap();
        let mut missing = source.clone();
        missing.backend_node_id = 999;
        assert_eq!(
            document.resolve_ancestor_scope("p1:9", &missing, "row"),
            Err(SemanticScopeFailure::SourceMissing)
        );
        assert_eq!(
            document.resolve_ancestor_scope("p1:1", &source, "row"),
            Err(SemanticScopeFailure::AncestorMissing)
        );

        let mut changed = source.clone();
        changed.label = Some("previous virtualized version".to_owned());
        assert_eq!(
            document.resolve_ancestor_scope("p1:1", &changed, "row"),
            Err(SemanticScopeFailure::SourceChanged)
        );
        let mut non_semantic = source.clone();
        non_semantic.semantic = false;
        assert_eq!(
            document.resolve_ancestor_scope("p1:1", &non_semantic, "row"),
            Err(SemanticScopeFailure::SourceNotSemantic)
        );

        let mut unproven = document.clone();
        for node in &mut unproven.nodes {
            node.frame = FrameRef::main_unproven();
        }
        let source = unproven.nodes[1].to_ref_entry().unwrap();
        assert_eq!(
            unproven.resolve_ancestor_scope("p1:1", &source, "row"),
            Err(SemanticScopeFailure::UnprovenFrame)
        );
    }

    #[test]
    fn ancestor_scope_refuses_broken_and_cyclic_parent_chains() {
        let main = proven_frame(FrameKind::Main, "main");
        let broken = scope_document(vec![scope_node(
            "source",
            Some("missing-parent"),
            &[],
            Some(2),
            "link",
            Some("version"),
            main.clone(),
            0,
        )]);
        let source = broken.nodes[0].to_ref_entry().unwrap();
        assert_eq!(
            broken.resolve_ancestor_scope("p1:0", &source, "row"),
            Err(SemanticScopeFailure::BrokenParent)
        );

        let cyclic = scope_document(vec![
            scope_node(
                "source",
                Some("parent"),
                &[],
                Some(2),
                "link",
                Some("version"),
                main.clone(),
                0,
            ),
            scope_node(
                "parent",
                Some("source"),
                &["source"],
                Some(3),
                "group",
                None,
                main,
                1,
            ),
        ]);
        let source = cyclic.nodes[0].to_ref_entry().unwrap();
        assert_eq!(
            cyclic.resolve_ancestor_scope("p1:0", &source, "row"),
            Err(SemanticScopeFailure::Cycle)
        );
    }

    #[test]
    fn ancestor_scope_refuses_ambiguous_and_cross_frame_ancestry() {
        let main = proven_frame(FrameKind::Main, "main");
        let iframe = proven_frame(FrameKind::Iframe, "child");
        let ambiguous = scope_document(vec![
            scope_node(
                "source-a",
                None,
                &[],
                Some(2),
                "link",
                Some("version"),
                main.clone(),
                0,
            ),
            scope_node(
                "source-b",
                None,
                &[],
                Some(2),
                "link",
                Some("version"),
                main.clone(),
                1,
            ),
        ]);
        let source = ambiguous.nodes[0].to_ref_entry().unwrap();
        assert_eq!(
            ambiguous.resolve_ancestor_scope("p1:0", &source, "row"),
            Err(SemanticScopeFailure::Ambiguous)
        );

        let cross_frame = scope_document(vec![
            scope_node(
                "source",
                Some("row"),
                &[],
                Some(2),
                "link",
                Some("version"),
                main.clone(),
                0,
            ),
            scope_node(
                "row",
                None,
                &["source"],
                Some(3),
                "row",
                Some("Wrong frame"),
                iframe,
                1,
            ),
        ]);
        let source = cross_frame.nodes[0].to_ref_entry().unwrap();
        assert_eq!(
            cross_frame.resolve_ancestor_scope("p1:0", &source, "row"),
            Err(SemanticScopeFailure::CrossFrame)
        );
    }

    #[test]
    fn ancestor_scope_refuses_duplicate_children_but_keeps_dom_supplements_bounded() {
        let main = proven_frame(FrameKind::Main, "main");
        let duplicate = scope_document(vec![
            scope_node(
                "row",
                None,
                &["action"],
                Some(1),
                "row",
                None,
                main.clone(),
                0,
            ),
            scope_node(
                "action",
                Some("row"),
                &[],
                Some(2),
                "button",
                Some("Options"),
                main.clone(),
                1,
            ),
            scope_node(
                "action",
                Some("row"),
                &[],
                Some(3),
                "button",
                Some("Duplicate options"),
                main.clone(),
                2,
            ),
        ]);
        let root = SemanticNodeIdentity {
            ax_id: "row".to_owned(),
            frame: main.clone(),
        };
        assert_eq!(
            duplicate.validate_ancestor_subtree(&root),
            Err(SemanticScopeFailure::Ambiguous)
        );

        let mut supplemented = scope_document(vec![
            scope_node("row", None, &[], Some(1), "row", None, main.clone(), 0),
            scope_node(
                "dom-action",
                Some("row"),
                &[],
                Some(2),
                "button",
                Some("DOM supplement"),
                main.clone(),
                1,
            ),
            scope_node(
                "outside",
                None,
                &[],
                Some(3),
                "group",
                None,
                main.clone(),
                2,
            ),
            scope_node(
                "dom-outside",
                Some("outside"),
                &[],
                Some(4),
                "button",
                Some("DOM supplement"),
                main,
                3,
            ),
        ]);
        supplemented
            .ancestry
            .retain(|node| !node.ax_id.starts_with("dom-"));
        assert_eq!(supplemented.validate_ancestor_subtree(&root), Ok(()));
        let page = supplemented.page(
            0,
            300,
            Some("DOM supplement"),
            Some(SemanticScope::Ancestor(&root)),
        );
        assert_eq!(page.selected.len(), 1);
        assert_eq!(page.selected[0].backend_node_id, Some(2));

        // A supplemented source has no raw AX edge to walk from.
        let source = supplemented.nodes[1].to_ref_entry().unwrap();
        assert_eq!(
            supplemented.resolve_ancestor_scope("p1:1", &source, "row"),
            Err(SemanticScopeFailure::BrokenParent)
        );
    }

    #[test]
    fn ancestor_scope_crosses_ignored_wrappers_in_both_directions() {
        let main = proven_frame(FrameKind::Main, "main");
        let ax = json!({"nodes": [
            {"nodeId": "root", "ignored": false, "role": {"value": "RootWebArea"},
             "childIds": ["row", "other-row"]},
            {"nodeId": "row", "parentId": "root", "ignored": false,
             "backendDOMNodeId": 10, "role": {"value": "row"},
             "name": {"value": "Release 0.19.89"}, "childIds": ["ignored-a"]},
            {"nodeId": "ignored-a", "parentId": "row", "ignored": true,
             "childIds": ["ignored-b"]},
            {"nodeId": "ignored-b", "parentId": "ignored-a", "ignored": true,
             "childIds": ["source", "action"]},
            {"nodeId": "source", "parentId": "ignored-b", "ignored": false,
             "backendDOMNodeId": 11, "role": {"value": "link"},
             "name": {"value": "0.19.89"}},
            {"nodeId": "action", "parentId": "ignored-b", "ignored": false,
             "backendDOMNodeId": 12, "role": {"value": "button"},
             "name": {"value": "View release options"}},
            {"nodeId": "other-row", "parentId": "root", "ignored": false,
             "backendDOMNodeId": 20, "role": {"value": "row"},
             "name": {"value": "Release 0.19.88"}, "childIds": ["other-action"]},
            {"nodeId": "other-action", "parentId": "other-row", "ignored": false,
             "backendDOMNodeId": 21, "role": {"value": "button"},
             "name": {"value": "View release options"}}
        ]});
        let document = compose_accessibility_tree(
            &ax,
            &DomIndex::default(),
            &LayoutIndex::default(),
            &Viewport::default(),
            main,
        );
        assert!(document
            .nodes
            .iter()
            .all(|node| !node.ax_id.starts_with("ignored-")));

        let source = document
            .nodes
            .iter()
            .find(|node| node.backend_node_id == Some(11))
            .and_then(SemanticNode::to_ref_entry)
            .expect("retained semantic source");
        let anchor = document
            .resolve_ancestor_scope("p1:source", &source, "row")
            .expect("row through ignored wrappers");
        assert_eq!(anchor.name.as_deref(), Some("Release 0.19.89"));
        assert_eq!(anchor.distance, 3);
        assert_eq!(document.validate_ancestor_subtree(&anchor.identity), Ok(()));

        let page = document.page(
            0,
            300,
            Some("View release options"),
            Some(SemanticScope::Ancestor(&anchor.identity)),
        );
        assert_eq!(page.total_nodes, 1);
        assert_eq!(page.selected.len(), 1);
        assert_eq!(page.selected[0].backend_node_id, Some(12));
        assert!(!page.outline.contains("0.19.88"), "{}", page.outline);
    }

    #[test]
    fn ancestor_scope_refuses_ignored_wrapper_cycles_and_ambiguity() {
        let main = proven_frame(FrameKind::Main, "main");
        let mut cyclic = scope_document(vec![scope_node(
            "source",
            Some("ignored-a"),
            &[],
            Some(2),
            "link",
            Some("version"),
            main.clone(),
            0,
        )]);
        cyclic.ancestry = vec![
            SemanticAncestryNode {
                ax_id: "source".to_owned(),
                parent_ax_id: Some("ignored-a".to_owned()),
                child_ax_ids: Vec::new(),
                frame: main.clone(),
            },
            SemanticAncestryNode {
                ax_id: "ignored-a".to_owned(),
                parent_ax_id: Some("ignored-b".to_owned()),
                child_ax_ids: vec!["source".to_owned()],
                frame: main.clone(),
            },
            SemanticAncestryNode {
                ax_id: "ignored-b".to_owned(),
                parent_ax_id: Some("ignored-a".to_owned()),
                child_ax_ids: vec!["ignored-a".to_owned()],
                frame: main.clone(),
            },
        ];
        let source = cyclic.nodes[0].to_ref_entry().unwrap();
        assert_eq!(
            cyclic.resolve_ancestor_scope("p1:source", &source, "row"),
            Err(SemanticScopeFailure::Cycle)
        );

        let mut ambiguous = scope_document(vec![
            scope_node(
                "row",
                None,
                &["ignored"],
                Some(1),
                "row",
                None,
                main.clone(),
                0,
            ),
            scope_node(
                "source",
                Some("ignored"),
                &[],
                Some(2),
                "link",
                Some("version"),
                main.clone(),
                1,
            ),
        ]);
        ambiguous.ancestry.extend([
            SemanticAncestryNode {
                ax_id: "ignored".to_owned(),
                parent_ax_id: Some("row".to_owned()),
                child_ax_ids: vec!["source".to_owned()],
                frame: main.clone(),
            },
            SemanticAncestryNode {
                ax_id: "ignored".to_owned(),
                parent_ax_id: Some("row".to_owned()),
                child_ax_ids: vec!["source".to_owned()],
                frame: main.clone(),
            },
        ]);
        let source = ambiguous.nodes[1].to_ref_entry().unwrap();
        assert_eq!(
            ambiguous.resolve_ancestor_scope("p1:source", &source, "row"),
            Err(SemanticScopeFailure::Ambiguous)
        );
        let row = SemanticNodeIdentity {
            ax_id: "row".to_owned(),
            frame: main,
        };
        assert_eq!(
            ambiguous.validate_ancestor_subtree(&row),
            Err(SemanticScopeFailure::Ambiguous)
        );
    }

    #[test]
    fn ancestor_scope_skips_a_source_that_has_the_requested_role() {
        let main = proven_frame(FrameKind::Main, "main");
        let nested = scope_document(vec![
            scope_node(
                "outer",
                None,
                &["source"],
                Some(1),
                "row",
                Some("Outer"),
                main.clone(),
                0,
            ),
            scope_node(
                "source",
                Some("outer"),
                &["action"],
                Some(2),
                "row",
                Some("Inner"),
                main.clone(),
                1,
            ),
            scope_node(
                "action",
                Some("source"),
                &[],
                Some(3),
                "button",
                Some("Options"),
                main.clone(),
                2,
            ),
        ]);
        let source = nested.nodes[1].to_ref_entry().unwrap();
        let anchor = nested
            .resolve_ancestor_scope("p1:1", &source, "row")
            .expect("the strict ancestor row");
        assert_eq!(anchor.name.as_deref(), Some("Outer"));
        assert_eq!(anchor.distance, 1);

        let alone = scope_document(vec![scope_node(
            "source",
            None,
            &[],
            Some(2),
            "row",
            Some("Only"),
            main,
            0,
        )]);
        let source = alone.nodes[0].to_ref_entry().unwrap();
        assert_eq!(
            alone.resolve_ancestor_scope("p1:0", &source, "row"),
            Err(SemanticScopeFailure::AncestorMissing)
        );
    }

    #[test]
    fn ancestor_subtree_with_a_child_frame_edge_refuses() {
        let main = proven_frame(FrameKind::Main, "main");
        let iframe = proven_frame(FrameKind::Iframe, "child");
        let document = scope_document(vec![
            scope_node(
                "row",
                None,
                &["frame-root"],
                Some(1),
                "row",
                None,
                main.clone(),
                0,
            ),
            scope_node(
                "frame-root",
                Some("row"),
                &[],
                Some(2),
                "button",
                Some("Options"),
                iframe,
                1,
            ),
        ]);
        let root = SemanticNodeIdentity {
            ax_id: "row".to_owned(),
            frame: main,
        };
        assert_eq!(
            document.validate_ancestor_subtree(&root),
            Err(SemanticScopeFailure::CrossFrame)
        );
    }

    #[test]
    fn direct_scope_still_follows_child_ids_into_same_process_iframes() {
        let main = proven_frame(FrameKind::Main, "main");
        let iframe = proven_frame(FrameKind::Iframe, "child");
        let document = scope_document(vec![
            scope_node(
                "region",
                None,
                &["frame-root"],
                Some(1),
                "region",
                None,
                main.clone(),
                0,
            ),
            scope_node(
                "frame-root",
                Some("region"),
                &["frame-action"],
                Some(2),
                "rootwebarea",
                None,
                iframe.clone(),
                1,
            ),
            scope_node(
                "frame-action",
                Some("frame-root"),
                &[],
                Some(3),
                "button",
                Some("Options"),
                iframe,
                2,
            ),
            scope_node(
                "outside",
                None,
                &[],
                Some(4),
                "button",
                Some("Options"),
                main,
                3,
            ),
        ]);
        let page = document.page(0, 300, Some("Options"), Some(SemanticScope::Backend(1)));
        let backends = page
            .selected
            .iter()
            .filter_map(|node| node.backend_node_id)
            .collect::<Vec<_>>();
        assert_eq!(backends, vec![3]);
    }

    #[test]
    fn not_rendered_count_propagates_through_extend_and_page() {
        let mut document = SemanticDocument {
            not_rendered_count: 1,
            ..Default::default()
        };
        document.extend(SemanticDocument {
            complete: true,
            ..Default::default()
        });
        assert_eq!(document.page(0, 300, None, None).omissions.not_rendered, 1);
    }

    #[test]
    fn hidden_dom_nodes_do_not_enter_the_visible_working_set() {
        let dom = build_dom_index(&json!({
            "nodeType": 9,
            "children": [
                {"nodeType": 1, "nodeName": "BUTTON", "backendNodeId": 1,
                 "attributes": ["aria-hidden", "true"]},
                {"nodeType": 1, "nodeName": "BUTTON", "backendNodeId": 2,
                 "attributes": ["aria-label", "Reply"]}
            ]
        }));
        let ax = json!({"nodes": [
            {"nodeId": "root", "ignored": false, "role": {"value": "RootWebArea"},
             "childIds": ["retained", "reply"]},
            {"nodeId": "retained", "parentId": "root", "ignored": false,
             "backendDOMNodeId": 1, "role": {"value": "button"},
             "name": {"value": "Retained"}},
            {"nodeId": "reply", "parentId": "root", "ignored": false,
             "backendDOMNodeId": 2, "role": {"value": "button"},
             "name": {"value": "Reply"}}
        ]});
        let document = compose_accessibility_tree(
            &ax,
            &dom,
            &LayoutIndex::default(),
            &Viewport::default(),
            frame(),
        );
        let page = document.page(0, 300, None, None);
        assert_eq!(page.omissions.css_hidden, 1);
        let actionable = page
            .selected
            .iter()
            .filter(|node| !node.actions.is_empty())
            .collect::<Vec<_>>();
        assert_eq!(actionable.len(), 1);
        assert_eq!(actionable[0].name.as_deref(), Some("Reply"));
    }

    #[test]
    fn inline_style_hides_only_on_exact_hiding_declarations() {
        let dom = build_dom_index(&json!({
            "nodeType": 9,
            "children": [
                {"nodeType": 1, "nodeName": "DIV", "backendNodeId": 1,
                 "attributes": ["style", "opacity: 0.98; transition: opacity 0.2s"]},
                {"nodeType": 1, "nodeName": "DIV", "backendNodeId": 2,
                 "attributes": ["style", "--overlay-display:none;opacity:0.5"]},
                {"nodeType": 1, "nodeName": "DIV", "backendNodeId": 3,
                 "attributes": ["style", "color: red; display: none !important"]},
                {"nodeType": 1, "nodeName": "DIV", "backendNodeId": 4,
                 "attributes": ["style", "OPACITY:0"]}
            ]
        }));
        let hidden = |backend| dom.nodes[&backend].css_hidden;
        assert!(!hidden(1));
        assert!(!hidden(2));
        assert!(hidden(3));
        assert!(hidden(4));
        assert_eq!(dom.css_hidden_count, 2);
    }

    #[test]
    fn rendered_layout_overrides_inherited_static_hiding() {
        let dom = build_dom_index(&json!({
            "nodeType": 9,
            "frameId": "F_MAIN",
            "children": [
                {"nodeType": 1, "nodeName": "FORM", "backendNodeId": 1,
                 "attributes": ["style", "visibility: hidden"],
                 "children": [
                    {"nodeType": 1, "nodeName": "INPUT", "backendNodeId": 2,
                     "attributes": ["aria-label", "Business name",
                                    "style", "visibility: visible"]},
                    {"nodeType": 1, "nodeName": "INPUT", "backendNodeId": 3,
                     "attributes": ["aria-label", "Unrendered"]}
                 ]},
                {"nodeType": 1, "nodeName": "DIV", "backendNodeId": 4,
                 "attributes": ["style", "opacity: 0"],
                 "children": [
                    {"nodeType": 1, "nodeName": "SPAN", "children": [
                        {"nodeType": 1, "nodeName": "INPUT", "backendNodeId": 5,
                         "attributes": ["aria-label", "Transparent"]}
                    ]}
                 ]}
            ]
        }));
        let layout = build_layout_index(&json!({
            "strings": ["block", "visible", "1", "auto"],
            "documents": [{
                "nodes": {"backendNodeId": [2, 5]},
                "layout": {
                    "nodeIndex": [0, 1],
                    "bounds": [[10, 10, 200, 30], [10, 50, 200, 30]],
                    "styles": [[0, 1, 2, 3, 3, 3, 3], [0, 1, 2, 3, 3, 3, 3]],
                    "paintOrders": [1, 2]
                }
            }]
        }));
        let viewport = parse_viewport(&json!({
            "cssVisualViewport": {"pageX": 0.0, "pageY": 0.0,
                                  "clientWidth": 800.0, "clientHeight": 600.0}
        }));
        let document = compose_accessibility_tree(
            &json!({"nodes": [{"nodeId": "root", "ignored": false,
                "role": {"value": "RootWebArea"}, "childIds": []}]}),
            &dom,
            &layout,
            &viewport,
            frame(),
        );
        let page = document.page(0, 300, None, None);
        let names = page
            .selected
            .iter()
            .filter_map(|node| Some((node.name.as_deref()?, node.visibility)))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![("Business name", BrowserVisibility::InViewport)]
        );
    }

    #[test]
    fn dom_supplement_adds_only_explicit_visible_custom_actions() {
        let dom = build_dom_index(&json!({
            "nodeType": 9,
            "frameId": "F_MAIN",
            "children": [
                {"nodeType": 1, "nodeName": "DIV", "backendNodeId": 1,
                 "attributes": ["aria-label", "Custom action", "onclick", "run()"]},
                {"nodeType": 1, "nodeName": "DIV", "backendNodeId": 2,
                 "attributes": ["aria-label", "Static panel"]}
            ]
        }));
        let layout = build_layout_index(&json!({
            "strings": ["block", "visible", "1", "auto"],
            "documents": [{
                "nodes": {"backendNodeId": [1, 2]},
                "layout": {
                    "nodeIndex": [0, 1],
                    "bounds": [[10, 10, 100, 30], [10, 50, 100, 30]],
                    "styles": [[0, 1, 2, 3, 3, 3, 3], [0, 1, 2, 3, 3, 3, 3]],
                    "paintOrders": [1, 2]
                }
            }]
        }));
        let viewport = parse_viewport(&json!({
            "cssVisualViewport": {"pageX": 0.0, "pageY": 0.0,
                                  "clientWidth": 800.0, "clientHeight": 600.0}
        }));
        let document = compose_accessibility_tree(
            &json!({"nodes": [{"nodeId": "root", "ignored": false,
                "role": {"value": "RootWebArea"}, "childIds": []}]}),
            &dom,
            &layout,
            &viewport,
            frame(),
        );
        let page = document.page(0, 300, None, None);
        assert!(page.selected.iter().any(|node| {
            node.name.as_deref() == Some("Custom action")
                && node.actions == vec![BrowserActionKind::Click, BrowserActionKind::Pointer]
        }));
        assert!(page
            .selected
            .iter()
            .all(|node| node.name.as_deref() != Some("Static panel")));
    }

    #[test]
    fn dom_supplement_exposes_scrollable_containers_without_click_authority() {
        let dom = build_dom_index(&json!({
            "nodeType": 9,
            "frameId": "F_MAIN",
            "children": [
                {"nodeType": 1, "nodeName": "DIV", "backendNodeId": 1,
                 "attributes": ["aria-label", "Scrollable archive"]},
                {"nodeType": 1, "nodeName": "DIV", "backendNodeId": 2,
                 "attributes": ["aria-label", "Overflow-visible panel"]}
            ]
        }));
        let layout = build_layout_index(&json!({
            "strings": ["block", "visible", "1", "auto", "default", "static", "0", "scroll"],
            "documents": [{
                "nodes": {"backendNodeId": [1, 2]},
                "layout": {
                    "nodeIndex": [0, 1],
                    "bounds": [[10, 10, 100, 50], [10, 80, 100, 50]],
                    "clientRects": [[10, 10, 100, 50], [10, 80, 100, 50]],
                    "scrollRects": [[0, 0, 100, 250], [0, 0, 100, 250]],
                    "styles": [
                        [0, 1, 2, 3, 4, 5, 6, 3, 7],
                        [0, 1, 2, 3, 4, 5, 6, 1, 1]
                    ],
                    "paintOrders": [1, 2]
                }
            }]
        }));
        let viewport = parse_viewport(&json!({
            "cssVisualViewport": {"pageX": 0.0, "pageY": 0.0,
                                  "clientWidth": 800.0, "clientHeight": 600.0}
        }));
        let document = compose_accessibility_tree(
            &json!({"nodes": [{"nodeId": "root", "ignored": false,
                "role": {"value": "RootWebArea"}, "childIds": []}]}),
            &dom,
            &layout,
            &viewport,
            frame(),
        );
        let page = document.page(0, 300, None, None);
        let scrollable = page
            .selected
            .iter()
            .find(|node| node.name.as_deref() == Some("Scrollable archive"))
            .expect("scrollable DOM supplement");
        assert_eq!(scrollable.actions, vec![BrowserActionKind::Scroll]);
        assert!(page
            .selected
            .iter()
            .all(|node| node.name.as_deref() != Some("Overflow-visible panel")));
    }

    #[test]
    fn layout_ranks_in_viewport_actions_before_offscreen_content() {
        let dom = build_dom_index(&json!({
            "nodeType": 9,
            "children": [
                {"nodeType": 1, "nodeName": "P", "backendNodeId": 1},
                {"nodeType": 1, "nodeName": "BUTTON", "backendNodeId": 2}
            ]
        }));
        let layout = build_layout_index(&json!({
            "strings": ["block", "visible", "1", "auto", "pointer"],
            "documents": [{
                "nodes": {"backendNodeId": [1, 2]},
                "layout": {
                    "nodeIndex": [0, 1],
                    "bounds": [[0, 5000, 100, 20], [10, 10, 100, 30]],
                    "styles": [[0, 1, 2, 3, 4], [0, 1, 2, 3, 4]],
                    "paintOrders": [1, 2]
                }
            }]
        }));
        let viewport = parse_viewport(&json!({
            "cssVisualViewport": {"pageX": 0.0, "pageY": 0.0,
                                  "clientWidth": 800.0, "clientHeight": 600.0}
        }));
        let ax = json!({"nodes": [
            {"nodeId": "root", "ignored": false, "role": {"value": "RootWebArea"},
             "childIds": ["text", "reply"]},
            {"nodeId": "text", "parentId": "root", "ignored": false,
             "backendDOMNodeId": 1, "role": {"value": "StaticText"},
             "name": {"value": "Old content"}},
            {"nodeId": "reply", "parentId": "root", "ignored": false,
             "backendDOMNodeId": 2, "role": {"value": "button"},
             "name": {"value": "Reply"}}
        ]});
        let document = compose_accessibility_tree(&ax, &dom, &layout, &viewport, frame());
        let page = document.page(0, 1, None, None);
        assert_eq!(page.selected[0].name.as_deref(), Some("Reply"));
        assert_eq!(page.next_offset, Some(1));
    }

    #[test]
    fn fixed_painted_overlay_marks_covered_action_as_page_occluded() {
        let dom = build_dom_index(&json!({
            "nodeType": 9,
            "frameId": "F_MAIN",
            "children": [
                {"nodeType": 1, "nodeName": "BUTTON", "backendNodeId": 1},
                {"nodeType": 1, "nodeName": "DIV", "backendNodeId": 2}
            ]
        }));
        let layout = build_layout_index(&json!({
            "strings": ["block", "visible", "1", "auto", "fixed", "100"],
            "documents": [{
                "nodes": {"backendNodeId": [1, 2]},
                "layout": {
                    "nodeIndex": [0, 1],
                    "bounds": [[10, 10, 100, 30], [0, 0, 800, 600]],
                    "styles": [[0, 1, 2, 3, 3, 3, 3], [0, 1, 2, 3, 3, 4, 5]],
                    "paintOrders": [1, 2]
                }
            }]
        }));
        let viewport = parse_viewport(&json!({
            "cssVisualViewport": {"pageX": 0.0, "pageY": 0.0,
                                  "clientWidth": 800.0, "clientHeight": 600.0}
        }));
        let ax = json!({"nodes": [
            {"nodeId": "root", "ignored": false, "role": {"value": "RootWebArea"},
             "childIds": ["button"]},
            {"nodeId": "button", "parentId": "root", "ignored": false,
             "backendDOMNodeId": 1, "role": {"value": "button"},
             "name": {"value": "Covered"}}
        ]});
        let document = compose_accessibility_tree(&ax, &dom, &layout, &viewport, frame());
        let page = document.page(0, 300, None, None);
        assert!(page
            .selected
            .iter()
            .all(|node| node.name.as_deref() != Some("Covered")));
        assert_eq!(page.omissions.page_occluded, 1);
    }

    #[test]
    fn dialog_container_does_not_occlude_its_own_actions() {
        let dom = build_dom_index(&json!({
            "nodeType": 9,
            "frameId": "F_MAIN",
            "children": [{
                "nodeType": 1,
                "nodeName": "DIV",
                "backendNodeId": 10,
                "attributes": ["role", "alertdialog", "aria-label", "Remove app"],
                "children": [
                    {"nodeType": 1, "nodeName": "BUTTON", "backendNodeId": 11,
                     "attributes": ["aria-label", "Cancel"]},
                    {"nodeType": 1, "nodeName": "BUTTON", "backendNodeId": 12,
                     "attributes": ["aria-label", "Remove"]}
                ]
            }]
        }));
        let layout = build_layout_index(&json!({
            "strings": ["block", "visible", "1", "auto", "default", "static", "0", "fixed", "100"],
            "documents": [{
                "nodes": {"backendNodeId": [10, 11, 12]},
                "layout": {
                    "nodeIndex": [0, 1, 2],
                    "bounds": [[0, 0, 800, 600], [250, 400, 120, 36], [390, 400, 120, 36]],
                    "styles": [
                        [0, 1, 2, 3, 4, 7, 8, 3, 3],
                        [0, 1, 2, 3, 4, 5, 6, 3, 3],
                        [0, 1, 2, 3, 4, 5, 6, 3, 3]
                    ],
                    "paintOrders": [100, 10, 11]
                }
            }]
        }));
        let viewport = parse_viewport(&json!({
            "cssVisualViewport": {"pageX": 0.0, "pageY": 0.0,
                                  "clientWidth": 800.0, "clientHeight": 600.0}
        }));
        let ax = json!({"nodes": [
            {"nodeId": "root", "ignored": false, "role": {"value": "RootWebArea"},
             "childIds": ["dialog"]},
            {"nodeId": "dialog", "parentId": "root", "ignored": false,
             "backendDOMNodeId": 10, "role": {"value": "alertdialog"},
             "name": {"value": "Remove app"}, "childIds": ["cancel", "remove"]},
            {"nodeId": "cancel", "parentId": "dialog", "ignored": false,
             "backendDOMNodeId": 11, "role": {"value": "button"},
             "name": {"value": "Cancel"}, "childIds": []},
            {"nodeId": "remove", "parentId": "dialog", "ignored": false,
             "backendDOMNodeId": 12, "role": {"value": "button"},
             "name": {"value": "Remove"}, "childIds": []}
        ]});
        let document = compose_accessibility_tree(&ax, &dom, &layout, &viewport, frame());
        let page = document.page(0, 300, None, None);
        let actions = page
            .selected
            .iter()
            .filter(|node| !node.actions.is_empty())
            .filter_map(|node| node.name.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(actions, vec!["Cancel", "Remove"]);
        assert_eq!(page.omissions.page_occluded, 0);
    }

    #[test]
    fn semantic_text_removes_icon_glyphs_and_nonbreaking_spaces() {
        assert_eq!(
            clean_semantic_text("\u{e001} Reply\u{00a0}now \u{f8ff}".to_owned()).as_deref(),
            Some("Reply now")
        );
    }
}
