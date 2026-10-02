// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua.env.v1.AccessibilityService` over the backend's accessibility tree
//! (AT-SPI on Linux through cua-driver `platform-linux`).

use std::sync::Arc;
use std::time::Instant;

use cua_proto::env::v1::{
    accessibility_service_server::AccessibilityService, AccessibilityAction, AccessibilityNode,
    ActRequest, ActResponse, Delivery, DeliveryReport, ErrorReason, FindRequest, FindResponse,
    GetTreeRequest, GetTreeResponse, Rect, WindowFilter, WindowRef,
};
use tonic::{Code, Request, Response, Status};

use super::backend::{A11yActionKind, A11yNode, WindowAction, WindowRecord};
use super::status::{self, invalid, join_error, provider};
use super::{random_id, A11ySnapshotRecord, DesktopState, A11Y_HISTORY};

pub(crate) struct Accessibility(pub Arc<DesktopState>);

fn action_name(action: &str) -> Option<AccessibilityAction> {
    Some(match action.to_lowercase().as_str() {
        "click" | "press" | "activate" | "invoke" | "push" => AccessibilityAction::Press,
        "focus" | "grab_focus" => AccessibilityAction::Focus,
        "expand" | "expand or contract" => AccessibilityAction::Expand,
        "collapse" => AccessibilityAction::Collapse,
        "select" => AccessibilityAction::Select,
        "showmenu" | "show_menu" | "menu" => AccessibilityAction::ShowMenu,
        "increment" => AccessibilityAction::Increment,
        "decrement" => AccessibilityAction::Decrement,
        _ => return None,
    })
}

fn proto_node(node: &A11yNode) -> AccessibilityNode {
    AccessibilityNode {
        element_id: node.element_id.clone(),
        parent_id: node.parent_id.clone(),
        depth: node.depth,
        role: node.role.clone(),
        native_role: node.native_role.clone(),
        name: node.name.clone(),
        value: node.value.clone(),
        description: node.description.clone(),
        bounds: node.bounds.map(|(x, y, width, height)| Rect {
            x,
            y,
            width,
            height,
        }),
        states: node.states.clone(),
        actions: node
            .actions
            .iter()
            .filter_map(|action| action_name(action))
            .map(|action| action as i32)
            .collect(),
        attributes: Default::default(),
    }
}

impl Accessibility {
    async fn resolve_window(&self, reference: Option<WindowRef>) -> Result<WindowRecord, Status> {
        if let Some(reference) = reference {
            return self.0.window(&reference);
        }
        let state = self.0.clone();
        let windows = tokio::task::spawn_blocking(move || state.backend.windows())
            .await
            .map_err(join_error)?
            .map_err(provider)?;
        windows
            .into_iter()
            .filter(|window| super::windows::matches(&WindowFilter::default(), window))
            .find(|window| window.focused)
            .ok_or_else(|| {
                status::status(
                    Code::NotFound,
                    ErrorReason::TargetUnavailable,
                    "no focused window",
                )
            })
    }

    async fn snapshot(
        &self,
        window: WindowRecord,
        max_depth: u32,
        max_nodes: u32,
    ) -> Result<(String, Vec<A11yNode>, bool), Status> {
        let state = self.0.clone();
        let target = window.clone();
        let tree = tokio::task::spawn_blocking(move || {
            state.backend.a11y_tree(&target, max_depth, max_nodes)
        })
        .await
        .map_err(join_error)?
        .map_err(|error| match error.code {
            cua_spacesd_provider_api::ProviderErrorCode::Unsupported => {
                status::unsupported("a11y", error.message)
            }
            _ => provider(error),
        })?;
        let id = random_id("ax");
        let mut snapshots = self.0.a11y.lock().unwrap();
        if snapshots.len() >= A11Y_HISTORY {
            if let Some(oldest) = snapshots
                .iter()
                .min_by_key(|(_, record)| record.created)
                .map(|(id, _)| id.clone())
            {
                snapshots.remove(&oldest);
            }
        }
        snapshots.insert(
            id.clone(),
            A11ySnapshotRecord {
                window,
                backend_snapshot: tree.snapshot,
                created: Instant::now(),
            },
        );
        Ok((id, tree.nodes, tree.truncated))
    }
}

#[tonic::async_trait]
impl AccessibilityService for Accessibility {
    async fn get_tree(
        &self,
        request: Request<GetTreeRequest>,
    ) -> Result<Response<GetTreeResponse>, Status> {
        let request = request.into_inner();
        let window = self.resolve_window(request.window).await?;
        let reference = WindowRef {
            id: window.handle.0.clone(),
            epoch: window.epoch.0,
        };
        let (snapshot_id, nodes, truncated) = self
            .snapshot(window, request.max_depth, request.max_nodes)
            .await?;
        Ok(Response::new(GetTreeResponse {
            snapshot_id,
            window: Some(reference),
            nodes: nodes.iter().map(proto_node).collect(),
            truncated,
        }))
    }

    async fn find(&self, request: Request<FindRequest>) -> Result<Response<FindResponse>, Status> {
        let request = request.into_inner();
        let query = request.query.unwrap_or_default();
        let window = self.resolve_window(request.window).await?;
        let (snapshot_id, nodes, _) = self.snapshot(window, 0, 0).await?;
        let limit = if request.max_results == 0 {
            50
        } else {
            request.max_results as usize
        };
        let lower = |value: &str| value.to_lowercase();
        let matches = nodes
            .iter()
            .filter(|node| {
                query.role.is_empty() || node.role == query.role || node.native_role == query.role
            })
            .filter(|node| query.name.is_empty() || node.name == query.name)
            .filter(|node| {
                query.name_contains.is_empty()
                    || lower(&node.name).contains(&lower(&query.name_contains))
            })
            .filter(|node| {
                query.value_contains.is_empty()
                    || lower(&node.value).contains(&lower(&query.value_contains))
            })
            .filter(|node| query.states.iter().all(|state| node.states.contains(state)))
            .take(limit)
            .map(proto_node)
            .collect();
        Ok(Response::new(FindResponse {
            snapshot_id,
            nodes: matches,
        }))
    }

    async fn act(&self, request: Request<ActRequest>) -> Result<Response<ActResponse>, Status> {
        let request = request.into_inner();
        let element = request
            .element
            .ok_or_else(|| invalid("element is required"))?;
        let action = match AccessibilityAction::try_from(request.action)
            .unwrap_or(AccessibilityAction::Unspecified)
        {
            AccessibilityAction::Unspecified => return Err(invalid("action is required")),
            AccessibilityAction::Press => A11yActionKind::Press,
            AccessibilityAction::Focus => A11yActionKind::Focus,
            AccessibilityAction::SetValue => A11yActionKind::SetValue,
            AccessibilityAction::Increment => A11yActionKind::Increment,
            AccessibilityAction::Decrement => A11yActionKind::Decrement,
            AccessibilityAction::ShowMenu => A11yActionKind::ShowMenu,
            AccessibilityAction::Expand => A11yActionKind::Expand,
            AccessibilityAction::Collapse => A11yActionKind::Collapse,
            AccessibilityAction::Select => A11yActionKind::Select,
            AccessibilityAction::ScrollIntoView => A11yActionKind::ScrollIntoView,
            AccessibilityAction::Custom => A11yActionKind::Custom,
        };
        let (window, backend_snapshot) = {
            let snapshots = self.0.a11y.lock().unwrap();
            let record = snapshots.get(&element.snapshot_id).ok_or_else(|| {
                status::status(
                    Code::FailedPrecondition,
                    ErrorReason::StaleSnapshot,
                    "unknown or expired snapshot_id",
                )
            })?;
            // Element ids are only valid in the newest snapshot of a window.
            let newer = snapshots.values().any(|other| {
                other.window.handle == record.window.handle && other.created > record.created
            });
            if newer {
                return Err(status::status(
                    Code::FailedPrecondition,
                    ErrorReason::StaleSnapshot,
                    "a newer accessibility snapshot of this window exists",
                ));
            }
            (record.window.clone(), record.backend_snapshot)
        };
        let foreground = Delivery::try_from(request.delivery).ok() == Some(Delivery::Foreground);
        let state = self.0.clone();
        let element_id = element.element_id.clone();
        let value = request.value.clone();
        tokio::task::spawn_blocking(move || {
            if foreground {
                state
                    .backend
                    .window_action(&window, WindowAction::Activate)?;
            }
            state
                .backend
                .a11y_act(&window, backend_snapshot, &element_id, action, &value)
        })
        .await
        .map_err(join_error)?
        .map_err(|error| match error.code {
            cua_spacesd_provider_api::ProviderErrorCode::StaleTarget => status::status(
                Code::FailedPrecondition,
                ErrorReason::StaleSnapshot,
                error.message,
            ),
            _ => provider(error),
        })?;
        Ok(Response::new(ActResponse {
            report: Some(DeliveryReport {
                delivery: if foreground {
                    Delivery::Foreground
                } else {
                    Delivery::Background
                } as i32,
                focus_changed: foreground,
                pointer_moved: false,
                detail: "accessibility action".into(),
            }),
        }))
    }
}
