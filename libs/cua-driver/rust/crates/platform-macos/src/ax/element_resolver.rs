use super::bindings::*;
use super::tree::AXNode;
use core_foundation::base::{CFEqual, CFRelease, CFRetain, CFTypeRef};
use cua_driver_core::element_token::{self, ElementTarget, ResolvedElement};
use cua_driver_core::protocol::ToolResult;
#[derive(Clone)]
struct Binding {
    pid: i32,
    window: u32,
    target: ElementTarget,
    web: bool,
    root: std::sync::Arc<RetainedElement>,
}

pub struct RetainedElement(usize, Option<Binding>);
impl RetainedElement {
    pub fn as_ptr(&self) -> usize {
        self.0
    }
    pub fn screen_center(&self) -> anyhow::Result<(f64, f64)> {
        unsafe { element_screen_center(self.checked_ptr()? as AXUIElementRef) }
            .ok_or_else(|| anyhow::anyhow!("retained target geometry is unavailable"))
    }

    pub fn in_web_content(&self) -> bool {
        self.1.as_ref().is_none_or(|binding| binding.web)
    }

    pub fn checked_ptr(&self) -> anyhow::Result<usize> {
        cua_driver_core::tool::check_native_dispatch()?;
        if self.0 == 0 {
            anyhow::bail!("native element is unavailable");
        }
        if let Some(binding) = &self.1 {
            let (identity, root) = self.live_identity()?;
            if !binding.target.matches_identity(&identity)
                || unsafe { CFEqual(root.0 as CFTypeRef, binding.root.0 as CFTypeRef) } == 0
            {
                anyhow::bail!("element description or ancestry changed before dispatch");
            }
            root.verify_scope(binding.pid, binding.window)?;
        }
        Ok(self.0)
    }

    fn verify_scope(&self, pid: i32, window: u32) -> anyhow::Result<()> {
        if !matches!(
            crate::windows::resolve_window_owner(pid, window),
            crate::windows::WindowOwner::SamePid
        ) {
            anyhow::bail!("target window ownership changed before dispatch");
        }
        let root = self.0 as AXUIElementRef;
        if unsafe { ax_get_window_id(root) } != Some(window) {
            anyhow::bail!("element is not bound to the requested native window");
        }
        let app = Self(unsafe { AXUIElementCreateApplication(pid) } as usize, None);
        if app.0 == 0 {
            anyhow::bail!("application accessibility root is unavailable");
        }
        let mut current = FreshAxElements {
            elements: Vec::new(),
        };
        for attribute in ["AXChildren", "AXWindows"] {
            current.elements.extend(
                unsafe { copy_element_array(app.0 as AXUIElementRef, attribute) }
                    .map_err(|code| anyhow::anyhow!("AX root read failed: {code}"))?
                    .into_iter()
                    .map(|element| element as usize),
            );
        }
        if !current
            .elements
            .iter()
            .any(|&element| unsafe { CFEqual(element as CFTypeRef, self.0 as CFTypeRef) } != 0)
        {
            anyhow::bail!("element root left the application's current tree");
        }
        Ok(())
    }

    fn live_identity(&self) -> anyhow::Result<(Vec<u8>, Self)> {
        let element = self.0 as AXUIElementRef;
        if element.is_null() {
            anyhow::bail!("native element is unavailable");
        }
        let enabled = unsafe { copy_bool_attr_checked(element, "AXEnabled") }
            .map_err(|code| anyhow::anyhow!("AX enabled read failed: {code}"))?;
        if enabled == Some(false) {
            anyhow::bail!("native element is disabled");
        }
        let mut current = unsafe { Self::retain(self.0) };
        let mut depth = 0;
        let mut web = false;
        for _ in 0..element_token::MAX_NATIVE_ANCESTORS {
            let role = read_string(current.0 as AXUIElementRef, "AXRole")?
                .unwrap_or_else(|| "AXUnknown".into());
            web |= super::tree::is_web_content_role(&role);
            let parent = Self(
                unsafe { copy_element_attr(current.0 as AXUIElementRef, "AXParent") }
                    .ok_or_else(|| anyhow::anyhow!("native ancestry is incomplete"))?
                    as usize,
                None,
            );
            let parent_role = read_string(parent.0 as AXUIElementRef, "AXRole")?
                .unwrap_or_else(|| "AXUnknown".into());
            if parent_role == "AXApplication" {
                let actions = unsafe { copy_action_names_checked(element) }
                    .map_err(|code| anyhow::anyhow!("AX action read failed: {code}"))?;
                return Ok((
                    identity_for_properties(
                        &read_string(element, "AXRole")?.unwrap_or_else(|| "AXUnknown".into()),
                        &read_string(element, "AXTitle")?,
                        &read_string(element, "AXDescription")?,
                        &read_string(element, "AXIdentifier")?,
                        &actions,
                        depth,
                        web,
                    ),
                    current,
                ));
            }
            depth += usize::from(!super::tree::collapses_depth(&parent_role));
            current = parent;
        }
        anyhow::bail!("native ancestry limit reached")
    }

    pub unsafe fn retain(ptr: usize) -> Self {
        if ptr != 0 {
            unsafe { CFRetain(ptr as AXUIElementRef as CFTypeRef) };
        }
        Self(ptr, None)
    }
}
impl Clone for RetainedElement {
    fn clone(&self) -> Self {
        let mut retained = unsafe { Self::retain(self.0) };
        retained.1 = self.1.clone();
        retained
    }
}

fn read_string(element: AXUIElementRef, attribute: &str) -> anyhow::Result<Option<String>> {
    unsafe { copy_string_attr_checked(element, attribute) }
        .map_err(|code| anyhow::anyhow!("AX {attribute} read failed: {code}"))
}
impl Drop for RetainedElement {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe { CFRelease(self.0 as AXUIElementRef as CFTypeRef) };
        }
    }
}
pub struct FreshAxElements {
    pub elements: Vec<usize>,
}
impl FreshAxElements {
    pub fn from_nodes(nodes: &[AXNode]) -> Self {
        Self {
            elements: nodes
                .iter()
                .filter(|n| n.element_index.is_some())
                .map(|n| n.element_ptr)
                .collect(),
        }
    }
    fn retain_element(&self, index: usize) -> Option<RetainedElement> {
        self.elements
            .get(index)
            .filter(|p| **p != 0)
            .map(|p| unsafe { RetainedElement::retain(*p) })
    }
}
impl Drop for FreshAxElements {
    fn drop(&mut self) {
        for ptr in &self.elements {
            if *ptr != 0 {
                unsafe { CFRelease(*ptr as AXUIElementRef as CFTypeRef) };
            }
        }
    }
}
pub fn identity_for_node(n: &AXNode) -> Vec<u8> {
    identity_for_properties(
        &n.role,
        &n.title,
        &n.description,
        &n.identifier,
        &n.actions,
        n.depth,
        n.in_web_content,
    )
}
fn identity_for_properties(
    role: &str,
    title: &Option<String>,
    description: &Option<String>,
    identifier: &Option<String>,
    actions: &[String],
    depth: usize,
    web: bool,
) -> Vec<u8> {
    serde_json::to_vec(&(role, title, description, identifier, actions, depth, web))
        .expect("AX identity tuple")
}

pub async fn resolve_element_args(
    pid: i32,
    args: &serde_json::Value,
    tool: &str,
) -> Result<ResolvedElement<RetainedElement>, ToolResult> {
    element_token::resolve_native(pid, args, tool, move |w, t| resolve_fresh(pid, w, t)).await
}
pub(crate) fn resolve_fresh(
    pid: i32,
    w: u64,
    t: &ElementTarget,
) -> Result<Option<RetainedElement>, String> {
    let w =
        u32::try_from(w).map_err(|_| format!("window_id {w} is not a valid macOS window id"))?;
    let tree = super::tree::walk_tree(pid, Some(w), None);
    let payload = FreshAxElements::from_nodes(&tree.nodes);
    let matched = t.resolve_unique(
        tree.nodes
            .iter()
            .filter_map(|n| Some((n.element_index?, identity_for_node(n)))),
        !tree.truncated
            && tree
                .window_scope
                .as_ref()
                .is_some_and(|scope| scope.is_matched()),
    )?;
    let web = matched
        .and_then(|index| {
            tree.nodes
                .iter()
                .find(|node| node.element_index == Some(index))
        })
        .is_some_and(|node| node.in_web_content);
    let Some(mut element) = matched.and_then(|i| payload.retain_element(i)) else {
        return Ok(None);
    };
    let (identity, root) = element.live_identity().map_err(|error| error.to_string())?;
    if !t.matches_identity(&identity) {
        return Err("element changed during resolution".into());
    }
    root.verify_scope(pid, w)
        .map_err(|error| error.to_string())?;
    element.1 = Some(Binding {
        pid,
        window: w,
        target: t.clone(),
        web,
        root: std::sync::Arc::new(root),
    });
    Ok(Some(element))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_native_target_refuses_without_accessibility_lookup() {
        let element = unsafe { RetainedElement::retain(0) };
        assert!(element.checked_ptr().is_err());
    }

    #[test]
    fn empty_projection_has_no_element() {
        assert!(FreshAxElements::from_nodes(&[]).retain_element(0).is_none());
    }
}
