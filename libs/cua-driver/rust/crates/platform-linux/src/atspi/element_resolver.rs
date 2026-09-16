use super::*;
use cua_driver_core::element_token::{self, ElementTarget, ResolvedElement};
use cua_driver_core::protocol::ToolResult;
use std::sync::Arc;

pub fn identity_for_node(node: &AtspiNode) -> Vec<u8> {
    serde_json::to_vec(&(
        &node.role,
        &node.name,
        &node.description,
        &node.actions,
        node.depth,
        node.in_web_content,
    ))
    .expect("AT-SPI identity tuple")
}

pub struct RetainedElement {
    pid: u32,
    window: u64,
    frame: usize,
    position: usize,
    observed: AtspiNode,
    identity: ElementTarget,
    visited: Arc<Vec<Visited<'static>>>,
}

pub enum ElementRef {
    Index(usize),
    Resolved(Arc<RetainedElement>),
}

impl From<usize> for ElementRef {
    fn from(index: usize) -> Self {
        Self::Index(index)
    }
}

impl From<Arc<RetainedElement>> for ElementRef {
    fn from(element: Arc<RetainedElement>) -> Self {
        Self::Resolved(element)
    }
}

impl ElementRef {
    pub fn index(&self) -> usize {
        match self {
            Self::Index(index) => *index,
            Self::Resolved(element) => element.index(),
        }
    }

    pub(super) async fn resolve(&self, pid: u32) -> Result<(Arc<Vec<Visited<'static>>>, usize)> {
        match self {
            Self::Resolved(element) => {
                element.checked_window(pid, None)?;
                element.verify_live().await?;
                Ok((element.visited.clone(), element.position))
            }
            Self::Index(index) => {
                let visited = collect_visited(shared_connection().await?, pid)
                    .await?
                    .context("current accessibility state is unavailable")?;
                let position = visited
                    .iter()
                    .enumerate()
                    .filter(|(_, node)| is_indexable(node))
                    .nth(*index)
                    .map(|(position, _)| position)
                    .context("native element is unavailable")?;
                Ok((Arc::new(visited), position))
            }
        }
    }
}

impl std::fmt::Display for RetainedElement {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.index().fmt(formatter)
    }
}

impl RetainedElement {
    pub fn checked_window(&self, pid: u32, window: Option<u64>) -> Result<u64> {
        if self.pid != pid || window.is_some_and(|window| window != self.window) {
            anyhow::bail!("target process or window ownership changed");
        }
        Ok(self.window)
    }

    pub fn index(&self) -> usize {
        self.observed.element_index.unwrap()
    }

    pub fn can_activate(&self) -> bool {
        !self.visited[self.position].has_editable
            && self.observed.role != "table cell"
            && activation_index(&self.observed.role, &self.observed.actions).is_some()
    }

    async fn verify_live(&self) -> Result<()> {
        cua_driver_core::tool::check_native_dispatch()?;
        if !crate::wayland::is_wayland()
            && !crate::x11::window_belongs_to_pid(self.window, self.pid)
        {
            anyhow::bail!("target window ownership changed");
        }
        let conn = shared_connection().await?;
        let root = self
            .visited
            .iter()
            .find(|node| node.depth == 0 && node.frame_ordinal == self.frame)
            .context("target window root is unavailable")?;
        let app = app_for_pid(conn, self.pid)
            .await?
            .context("target application disappeared")?;
        let seeds = app
            .get_children()
            .await?
            .iter()
            .filter_map(RawObjectRef::from_atspi)
            .collect::<Vec<_>>();
        let frame = resolve_window_frame(conn, self.pid, self.window, &seeds)
            .await
            .and_then(|index| seeds.get(index))
            .context("target window scope is unproven")?;
        if frame.name != root.acc.inner().destination().as_str()
            || frame.path != root.acc.inner().path().as_str()
        {
            anyhow::bail!("target window root changed");
        }
        let element = &self.visited[self.position].acc;
        let state = element.get_state().await?;
        if state.contains(State::Defunct) || !is_enabled_state(&state) {
            anyhow::bail!("target is defunct or disabled");
        }
        let mut current = self.observed.clone();
        current.role = element.get_role_name().await?;
        let mut name = element.name().await?;
        let proxies = element.proxies().await?;
        let interfaces = element.get_interfaces().await?;
        if name.trim().is_empty() && interfaces.contains(Interface::Text) {
            let value = read_text_content(&proxies).await?;
            if !value.trim().is_empty() {
                name = value;
            }
        }
        current.name = (!name.is_empty()).then_some(name);
        current.actions.clear();
        if interfaces.contains(Interface::Action) {
            let action = proxies.action().await?;
            let count = action.n_actions().await?;
            if count < 0 {
                anyhow::bail!("native action count is invalid");
            }
            for index in 0..count {
                current.actions.push(action.get_name(index).await?);
            }
        }
        let mut ancestor = element.clone();
        current.in_web_content = is_web_process_bus(ancestor.inner().destination().as_str());
        for depth in 0..element_token::MAX_NATIVE_ANCESTORS {
            if ancestor.inner().destination().as_str() == frame.name
                && ancestor.inner().path().as_str() == frame.path
            {
                current.depth = depth;
                if self.identity.matches_identity(&identity_for_node(&current)) {
                    return Ok(());
                }
                anyhow::bail!("target description changed before delivery");
            }
            let parent = RawObjectRef::from_atspi(&ancestor.parent().await?)
                .context("target ancestry is unavailable")?;
            ancestor = accessible_for(conn, &parent).await?;
            current.in_web_content |= is_web_process_bus(&parent.name)
                || is_document_role(&ancestor.get_role_name().await?);
        }
        anyhow::bail!("target no longer belongs to the requested window")
    }

    pub fn screen_bounds(&self) -> Result<(i32, i32, u32, u32)> {
        bounded(
            async {
                self.verify_live().await?;
                element_bounds_for_visited(&self.visited, self.pid, self.window, Some(self.frame))
                    .await
                    .into_iter()
                    .find(|(index, ..)| *index == self.index())
                    .map(|(_, x, y, width, height)| (x, y, width, height))
                    .context("resolved target has no usable bounds")
            },
            || Err(anyhow!("resolved target bounds timed out")),
        )
    }

    pub fn perform_action(self: &Arc<Self>) -> Result<(String, bool)> {
        super::perform_action(self.pid, self.clone())
    }
}

pub async fn resolve_element_args(
    pid: i32,
    args: &serde_json::Value,
    tool_name: &str,
) -> Result<ResolvedElement<Arc<RetainedElement>>, ToolResult> {
    match element_token::resolve_native(pid, args, tool_name, move |window, target| {
        resolve_fresh(pid, window, target)
    })
    .await?
    {
        ResolvedElement::Element {
            window_id,
            via_token,
            element,
            ..
        } => Ok(ResolvedElement::Element {
            window_id,
            element_index: element.index(),
            via_token,
            element,
        }),
        ResolvedElement::None => Ok(ResolvedElement::None),
    }
}

pub(crate) fn resolve_fresh(
    pid: i32,
    window: u64,
    identity: &ElementTarget,
) -> Result<Option<Arc<RetainedElement>>, String> {
    bounded(
        async {
            let pid = u32::try_from(pid)?;
            let conn = shared_connection().await?;
            let (visited, frame, complete) = collect_visited_bounded(conn, pid, window, None, None)
                .await?
                .context("current accessibility state is unavailable")?;
            let frame = frame.context("current accessibility window scope is unproven")?;
            let (_, nodes) = render(&visited, Some(frame));
            let matched = identity
                .resolve_unique(
                    nodes.into_iter().filter_map(|node| {
                        node.element_index?;
                        let identity = identity_for_node(&node);
                        Some((node, identity))
                    }),
                    complete,
                )
                .map_err(anyhow::Error::msg)?;
            let Some(observed) = matched else {
                return Ok(None);
            };
            let position = visited
                .iter()
                .enumerate()
                .filter(|(_, node)| is_indexable(node))
                .nth(observed.element_index.unwrap())
                .map(|(position, _)| position)
                .context("resolved native target is unavailable")?;
            Ok(Some(Arc::new(RetainedElement {
                pid,
                window,
                frame,
                position,
                observed,
                identity: identity.clone(),
                visited: Arc::new(visited),
            })))
        },
        || Err(anyhow!("current accessibility resolution timed out")),
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(i: usize, name: &str) -> AtspiNode {
        AtspiNode {
            element_index: Some(i),
            role: "button".into(),
            name: Some(name.into()),
            value: None,
            checked: None,
            enabled: Some(true),
            selected: None,
            description: None,
            actions: vec!["click".into()],
            element_key: i as u64,
            depth: 0,
            parent_element_index: None,
            in_web_content: false,
        }
    }
    #[test]
    fn semantic_identity_ignores_traversal_index() {
        assert_eq!(
            identity_for_node(&node(1, "Save")),
            identity_for_node(&node(9, "Save"))
        );
        assert_ne!(
            identity_for_node(&node(1, "Save")),
            identity_for_node(&node(1, "Cancel"))
        );
    }
}
