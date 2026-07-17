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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OmissionCounts {
    pub(crate) css_hidden: usize,
    pub(crate) offscreen: usize,
    pub(crate) page_occluded: usize,
    pub(crate) no_layout: usize,
    pub(crate) unknown: usize,
    pub(crate) budget: usize,
    pub(crate) unprovable_frame: usize,
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
    pub(crate) css_hidden_dom_count: usize,
    pub(crate) unprovable_frame_count: usize,
    pub(crate) complete: bool,
}

impl SemanticDocument {
    pub(crate) fn extend(&mut self, mut other: Self) {
        let was_empty = self.nodes.is_empty();
        let offset = self.nodes.len();
        for node in &mut other.nodes {
            node.document_order += offset;
        }
        self.nodes.extend(other.nodes);
        self.css_hidden_dom_count += other.css_hidden_dom_count;
        self.unprovable_frame_count += other.unprovable_frame_count;
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
        scope_backend_node_id: Option<i64>,
    ) -> SemanticPage {
        let mut candidates = scoped_indices(&self.nodes, query, scope_backend_node_id);
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
        let selected = with_ancestors(&self.nodes, page_slice);
        let outline = render_outline(&self.nodes, &selected);
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
    apply_page_occlusion(&mut nodes, layout);
    remove_redundant_static_text(&mut nodes);
    SemanticDocument {
        nodes,
        css_hidden_dom_count: dom.css_hidden_count,
        unprovable_frame_count: 0,
        complete: true,
    }
}

fn apply_page_occlusion(nodes: &mut [SemanticNode], layout: &LayoutIndex) {
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
    let style = attrs
        .get("style")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    style.contains("display:none")
        || style.contains("display: none")
        || style.contains("visibility:hidden")
        || style.contains("visibility: hidden")
        || style.contains("opacity:0")
        || style.contains("opacity: 0")
}

fn layout_hidden(layout: &LayoutMeta) -> bool {
    layout
        .styles
        .get("display")
        .is_some_and(|value| value.eq_ignore_ascii_case("none"))
        || layout
            .styles
            .get("visibility")
            .is_some_and(|value| value.eq_ignore_ascii_case("hidden"))
        || layout
            .styles
            .get("opacity")
            .and_then(|value| value.parse::<f64>().ok())
            .is_some_and(|opacity| opacity <= 0.0)
}

fn classify_visibility(
    dom: Option<&DomMeta>,
    layout: Option<&LayoutMeta>,
    viewport: &Viewport,
) -> BrowserVisibility {
    if dom.is_some_and(|meta| meta.css_hidden) || layout.is_some_and(layout_hidden) {
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
    if matches!(role, "textbox" | "searchbox") || matches!(tag, "input" | "textarea") || editable {
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
    actions
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
        !node
            .parent_ax_id
            .as_ref()
            .and_then(|parent| names.get(parent))
            .is_some_and(|parent_name| parent_name == name)
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

fn scoped_indices(
    nodes: &[SemanticNode],
    query: Option<&str>,
    scope_backend_node_id: Option<i64>,
) -> Vec<usize> {
    let by_ax_id: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(idx, node)| (node.ax_id.as_str(), idx))
        .collect();
    let mut allowed = HashSet::new();
    if let Some(scope_backend) = scope_backend_node_id {
        if let Some(scope) = nodes
            .iter()
            .find(|node| node.backend_node_id == Some(scope_backend))
        {
            let mut stack = vec![scope.ax_id.as_str()];
            while let Some(ax_id) = stack.pop() {
                if !allowed.insert(ax_id.to_owned()) {
                    continue;
                }
                if let Some(idx) = by_ax_id.get(ax_id) {
                    stack.extend(nodes[*idx].child_ax_ids.iter().map(String::as_str));
                }
            }
        }
    }
    let query = query.map(|value| value.trim().to_ascii_lowercase());
    let in_scope =
        |node: &SemanticNode| scope_backend_node_id.is_none() || allowed.contains(&node.ax_id);
    let exact_match_exists = query.as_ref().is_some_and(|query| {
        !query.is_empty()
            && nodes
                .iter()
                .any(|node| in_scope(node) && node_contains_query(node, query))
    });
    nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            in_scope(node)
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

fn with_ancestors(nodes: &[SemanticNode], selected: &[usize]) -> HashSet<usize> {
    let by_ax_id: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(idx, node)| (node.ax_id.as_str(), idx))
        .collect();
    let mut keep: HashSet<usize> = selected.iter().copied().collect();
    for idx in selected {
        let mut parent = nodes[*idx].parent_ax_id.as_deref();
        while let Some(parent_id) = parent {
            let Some(parent_idx) = by_ax_id.get(parent_id).copied() else {
                break;
            };
            if !keep.insert(parent_idx) {
                break;
            }
            parent = nodes[parent_idx].parent_ax_id.as_deref();
        }
    }
    keep
}

fn render_outline(nodes: &[SemanticNode], selected: &HashSet<usize>) -> String {
    let by_ax_id: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(idx, node)| (node.ax_id.as_str(), idx))
        .collect();
    let mut ordered: Vec<usize> = selected.iter().copied().collect();
    ordered.sort_by_key(|idx| nodes[*idx].document_order);
    let mut lines = Vec::new();
    for idx in ordered {
        let node = &nodes[idx];
        if matches!(node.role.as_str(), "rootwebarea" | "webarea") {
            continue;
        }
        let mut depth = 0;
        let mut parent = node.parent_ax_id.as_deref();
        while let Some(parent_id) = parent {
            let Some(parent_idx) = by_ax_id.get(parent_id).copied() else {
                break;
            };
            if selected.contains(&parent_idx)
                && !matches!(nodes[parent_idx].role.as_str(), "rootwebarea" | "webarea")
            {
                depth += 1;
            }
            parent = nodes[parent_idx].parent_ax_id.as_deref();
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
    use crate::browser::store::FrameRef;

    fn frame() -> FrameRef {
        FrameRef::main_unproven()
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
                && node.actions == vec![BrowserActionKind::Click]
        }));
        assert!(page
            .selected
            .iter()
            .all(|node| node.name.as_deref() != Some("Static panel")));
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
    fn semantic_text_removes_icon_glyphs_and_nonbreaking_spaces() {
        assert_eq!(
            clean_semantic_text("\u{e001} Reply\u{00a0}now \u{f8ff}".to_owned()).as_deref(),
            Some("Reply now")
        );
    }
}
