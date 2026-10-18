// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua.env.v1.WindowsService`: enumeration, a snapshot-first watch stream,
//! window management and app launching.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use cua_proto::env::v1::{
    app_spec, open_request, watch_windows_response, windows_service_server::WindowsService,
    ActivateWindowRequest, ActivateWindowResponse, AppInfo, AppSpec, CloseWindowRequest,
    CloseWindowResponse, Delivery, GetWindowRequest, GetWindowResponse, KeepAlive,
    LaunchAppRequest, LaunchAppResponse, ListWindowsRequest, ListWindowsResponse,
    MaximizeWindowRequest, MaximizeWindowResponse, MinimizeWindowRequest, MinimizeWindowResponse,
    OpenRequest, OpenResponse, Rect, RestoreWindowRequest, RestoreWindowResponse,
    SetWindowBoundsRequest, SetWindowBoundsResponse, WatchWindowsRequest, WatchWindowsResponse,
    WindowClosed, WindowFilter, WindowInfo, WindowKind as ProtoKind, WindowRef, WindowSnapshot,
    WindowState as ProtoState,
};
use tokio_stream::Stream;
use tonic::{Request, Response, Status};

use super::backend::{
    AppSpecData, LaunchRequest, OpenTarget, WindowAction, WindowKind, WindowRecord, WindowStateKind,
};
use super::status::{invalid, join_error, provider};
use super::DesktopState;

pub(crate) struct Windows(pub Arc<DesktopState>);

pub(crate) fn window_info(window: &WindowRecord) -> WindowInfo {
    let (x, y, width, height) = window.bounds;
    WindowInfo {
        r#ref: Some(WindowRef {
            id: window.handle.0.clone(),
            epoch: window.epoch.0,
        }),
        title: window.title.clone(),
        app: Some(AppInfo {
            name: window.app_name.clone(),
            app_id: window.app_id.clone(),
            pid: window.pid,
        }),
        bounds: Some(Rect {
            x,
            y,
            width,
            height,
        }),
        display_id: window.display_id.clone(),
        state: match window.state {
            WindowStateKind::Normal => ProtoState::Normal,
            WindowStateKind::Minimized => ProtoState::Minimized,
            WindowStateKind::Maximized => ProtoState::Maximized,
            WindowStateKind::Fullscreen => ProtoState::Fullscreen,
            WindowStateKind::Hidden => ProtoState::Hidden,
        } as i32,
        kind: match window.kind {
            WindowKind::Standard => ProtoKind::Standard,
            WindowKind::Dialog => ProtoKind::Dialog,
            WindowKind::Panel => ProtoKind::Panel,
            WindowKind::Menu => ProtoKind::Menu,
            WindowKind::Tooltip => ProtoKind::Tooltip,
            WindowKind::System => ProtoKind::System,
            WindowKind::Phantom => ProtoKind::Phantom,
        } as i32,
        focused: window.focused,
        on_screen: window.on_screen,
        z_order: window.z_order,
    }
}

pub(crate) fn matches(filter: &WindowFilter, window: &WindowRecord) -> bool {
    if !filter.include_system && matches!(window.kind, WindowKind::System | WindowKind::Phantom) {
        return false;
    }
    let contains = |haystack: &str, needle: &str| {
        needle.is_empty() || haystack.to_lowercase().contains(&needle.to_lowercase())
    };
    contains(&window.title, &filter.title_contains)
        && contains(&window.app_name, &filter.app_name_contains)
        && (filter.app_id.is_empty() || window.app_id == filter.app_id)
        && (filter.pid == 0 || window.pid == filter.pid)
        && (filter.display_id.is_empty() || window.display_id == filter.display_id)
        && (!filter.on_screen_only || window.on_screen)
}

fn app_spec(spec: Option<AppSpec>) -> Result<AppSpecData, Status> {
    match spec.and_then(|spec| spec.app) {
        Some(app_spec::App::AppId(id)) if !id.is_empty() => Ok(AppSpecData::AppId(id)),
        Some(app_spec::App::Executable(path)) if !path.is_empty() => {
            Ok(AppSpecData::Executable(path))
        }
        Some(app_spec::App::Name(name)) if !name.is_empty() => Ok(AppSpecData::Name(name)),
        _ => Err(invalid(
            "exactly one of app_id, executable or name is required",
        )),
    }
}

impl Windows {
    async fn list(&self, filter: WindowFilter) -> Result<Vec<WindowRecord>, Status> {
        let state = self.0.clone();
        let windows = tokio::task::spawn_blocking(move || state.backend.windows())
            .await
            .map_err(join_error)?
            .map_err(provider)?;
        Ok(windows
            .into_iter()
            .filter(|window| matches(&filter, window))
            .collect())
    }

    async fn act(
        &self,
        reference: Option<WindowRef>,
        action: WindowAction,
    ) -> Result<WindowInfo, Status> {
        let reference = reference.ok_or_else(|| invalid("window is required"))?;
        let window = self.0.window(&reference)?;
        let state = self.0.clone();
        let target = window.clone();
        tokio::task::spawn_blocking(move || state.backend.window_action(&target, action))
            .await
            .map_err(join_error)?
            .map_err(provider)?;
        self.settled(&window).await
    }

    /// Re-read a window after a change the window manager applies
    /// asynchronously.
    async fn settled(&self, window: &WindowRecord) -> Result<WindowInfo, Status> {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let current = self
            .list(WindowFilter {
                include_system: true,
                ..WindowFilter::default()
            })
            .await?
            .into_iter()
            .find(|candidate| candidate.handle == window.handle)
            .unwrap_or_else(|| window.clone());
        Ok(window_info(&current))
    }
}

type WatchStream = Pin<Box<dyn Stream<Item = Result<WatchWindowsResponse, Status>> + Send>>;

#[tonic::async_trait]
impl WindowsService for Windows {
    async fn list_windows(
        &self,
        request: Request<ListWindowsRequest>,
    ) -> Result<Response<ListWindowsResponse>, Status> {
        let filter = request.into_inner().filter.unwrap_or_default();
        let windows = self.list(filter).await?;
        Ok(Response::new(ListWindowsResponse {
            windows: windows.iter().map(window_info).collect(),
        }))
    }

    type WatchWindowsStream = WatchStream;

    async fn watch_windows(
        &self,
        request: Request<WatchWindowsRequest>,
    ) -> Result<Response<Self::WatchWindowsStream>, Status> {
        let request = request.into_inner();
        let filter = request.filter.unwrap_or_default();
        let keepalive = request
            .keepalive_interval
            .and_then(|duration| Duration::try_from(duration).ok())
            .filter(|duration| !duration.is_zero())
            .unwrap_or(Duration::from_secs(30));
        let (sender, receiver) = tokio::sync::mpsc::channel(64);
        let state = self.0.clone();
        tokio::spawn(async move {
            let read = |state: Arc<DesktopState>| async move {
                tokio::task::spawn_blocking(move || state.backend.windows())
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or_default()
            };
            let mut known: HashMap<String, WindowInfo> = HashMap::new();
            let first: Vec<WindowRecord> = read(state.clone())
                .await
                .into_iter()
                .filter(|w| matches(&filter, w))
                .collect();
            let snapshot: Vec<WindowInfo> = first.iter().map(window_info).collect();
            for info in &snapshot {
                known.insert(
                    info.r#ref
                        .as_ref()
                        .map(|r| r.id.clone())
                        .unwrap_or_default(),
                    info.clone(),
                );
            }
            if sender
                .send(Ok(WatchWindowsResponse {
                    event: Some(watch_windows_response::Event::Snapshot(WindowSnapshot {
                        windows: snapshot,
                    })),
                }))
                .await
                .is_err()
            {
                return;
            }
            let mut last_send = tokio::time::Instant::now();
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if sender.is_closed() {
                    return;
                }
                let current: HashMap<String, WindowInfo> = read(state.clone())
                    .await
                    .into_iter()
                    .filter(|w| matches(&filter, w))
                    .map(|w| (w.handle.0.clone(), window_info(&w)))
                    .collect();
                let mut events = Vec::new();
                for (id, info) in &current {
                    match known.get(id) {
                        None => events.push(watch_windows_response::Event::Created(info.clone())),
                        Some(previous) if previous != info => {
                            events.push(watch_windows_response::Event::Updated(info.clone()))
                        }
                        _ => {}
                    }
                }
                for (id, info) in &known {
                    if !current.contains_key(id) {
                        events.push(watch_windows_response::Event::Closed(WindowClosed {
                            r#ref: info.r#ref.clone(),
                        }));
                    }
                }
                known = current;
                if events.is_empty() && last_send.elapsed() >= keepalive {
                    events.push(watch_windows_response::Event::Keepalive(KeepAlive {}));
                }
                for event in events {
                    last_send = tokio::time::Instant::now();
                    if sender
                        .send(Ok(WatchWindowsResponse { event: Some(event) }))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(receiver),
        )))
    }

    async fn get_window(
        &self,
        request: Request<GetWindowRequest>,
    ) -> Result<Response<GetWindowResponse>, Status> {
        let reference = request
            .into_inner()
            .window
            .ok_or_else(|| invalid("window is required"))?;
        let window = self.0.window(&reference)?;
        Ok(Response::new(GetWindowResponse {
            window: Some(window_info(&window)),
        }))
    }

    async fn activate_window(
        &self,
        request: Request<ActivateWindowRequest>,
    ) -> Result<Response<ActivateWindowResponse>, Status> {
        let window = self
            .act(request.into_inner().window, WindowAction::Activate)
            .await?;
        Ok(Response::new(ActivateWindowResponse {
            window: Some(window),
        }))
    }

    async fn set_window_bounds(
        &self,
        request: Request<SetWindowBoundsRequest>,
    ) -> Result<Response<SetWindowBoundsResponse>, Status> {
        let request = request.into_inner();
        let reference = request
            .window
            .ok_or_else(|| invalid("window is required"))?;
        let window = self.0.window(&reference)?;
        let position = request.position.map(|point| (point.x, point.y));
        let size = match (request.width, request.height) {
            (None, None) => None,
            (width, height) => Some((
                width.unwrap_or(window.bounds.2),
                height.unwrap_or(window.bounds.3),
            )),
        };
        if size.is_some_and(|(width, height)| width <= 0.0 || height <= 0.0) {
            return Err(invalid("width and height must be positive"));
        }
        let state = self.0.clone();
        let target = window.clone();
        tokio::task::spawn_blocking(move || {
            state.backend.set_window_bounds(&target, position, size)
        })
        .await
        .map_err(join_error)?
        .map_err(provider)?;
        Ok(Response::new(SetWindowBoundsResponse {
            window: Some(self.settled(&window).await?),
        }))
    }

    async fn minimize_window(
        &self,
        request: Request<MinimizeWindowRequest>,
    ) -> Result<Response<MinimizeWindowResponse>, Status> {
        let window = self
            .act(request.into_inner().window, WindowAction::Minimize)
            .await?;
        Ok(Response::new(MinimizeWindowResponse {
            window: Some(window),
        }))
    }

    async fn maximize_window(
        &self,
        request: Request<MaximizeWindowRequest>,
    ) -> Result<Response<MaximizeWindowResponse>, Status> {
        let window = self
            .act(request.into_inner().window, WindowAction::Maximize)
            .await?;
        Ok(Response::new(MaximizeWindowResponse {
            window: Some(window),
        }))
    }

    async fn restore_window(
        &self,
        request: Request<RestoreWindowRequest>,
    ) -> Result<Response<RestoreWindowResponse>, Status> {
        let window = self
            .act(request.into_inner().window, WindowAction::Restore)
            .await?;
        Ok(Response::new(RestoreWindowResponse {
            window: Some(window),
        }))
    }

    async fn close_window(
        &self,
        request: Request<CloseWindowRequest>,
    ) -> Result<Response<CloseWindowResponse>, Status> {
        let request = request.into_inner();
        let reference = request
            .window
            .ok_or_else(|| invalid("window is required"))?;
        let window = self.0.window(&reference)?;
        let state = self.0.clone();
        let target = window.clone();
        let force = request.force;
        tokio::task::spawn_blocking(move || {
            state
                .backend
                .window_action(&target, WindowAction::Close { force })
        })
        .await
        .map_err(join_error)?
        .map_err(provider)?;
        // Give the app a moment to close (or to show a save prompt).
        let mut closed = false;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let still_there = self
                .list(WindowFilter {
                    include_system: true,
                    ..WindowFilter::default()
                })
                .await?
                .iter()
                .any(|candidate| candidate.handle == window.handle);
            if !still_there {
                closed = true;
                break;
            }
        }
        Ok(Response::new(CloseWindowResponse { closed }))
    }

    async fn launch_app(
        &self,
        request: Request<LaunchAppRequest>,
    ) -> Result<Response<LaunchAppResponse>, Status> {
        let request = request.into_inner();
        let launch = LaunchRequest {
            app: app_spec(request.app)?,
            args: request.args,
            env: request.env.into_iter().collect(),
            cwd: (!request.cwd.is_empty()).then_some(request.cwd),
            background: Delivery::try_from(request.delivery).ok() != Some(Delivery::Foreground),
        };
        let state = self.0.clone();
        let pid = tokio::task::spawn_blocking(move || state.backend.launch(&launch))
            .await
            .map_err(join_error)?
            .map_err(provider)?;
        let wait = request
            .wait_for_window
            .and_then(|duration| Duration::try_from(duration).ok())
            .unwrap_or_default()
            .min(Duration::from_secs(60));
        let mut windows = Vec::new();
        if !wait.is_zero() && pid != 0 {
            let deadline = tokio::time::Instant::now() + wait;
            while tokio::time::Instant::now() < deadline {
                windows = self
                    .list(WindowFilter {
                        pid,
                        ..WindowFilter::default()
                    })
                    .await?
                    .iter()
                    .map(window_info)
                    .collect();
                if !windows.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        Ok(Response::new(LaunchAppResponse { pid, windows }))
    }

    async fn open(&self, request: Request<OpenRequest>) -> Result<Response<OpenResponse>, Status> {
        let request = request.into_inner();
        let target = match request.target {
            Some(open_request::Target::Url(url)) if !url.is_empty() => OpenTarget::Url(url),
            Some(open_request::Target::Path(path)) if !path.is_empty() => OpenTarget::Path(path),
            _ => return Err(invalid("url or path is required")),
        };
        let with = match request.with_app {
            Some(spec) if spec.app.is_some() => Some(app_spec(Some(spec))?),
            _ => None,
        };
        let background = Delivery::try_from(request.delivery).ok() != Some(Delivery::Foreground);
        let state = self.0.clone();
        let pid = tokio::task::spawn_blocking(move || {
            state.backend.open(&target, with.as_ref(), background)
        })
        .await
        .map_err(join_error)?
        .map_err(provider)?;
        Ok(Response::new(OpenResponse { pid }))
    }
}
