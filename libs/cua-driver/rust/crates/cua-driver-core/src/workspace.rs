//! Platform-neutral workspace lifecycle and MCP tools.
//!
//! A workspace is an ownership/routing boundary, not a security sandbox.
//! Backends such as an OS-native desktop/Space or a nested compositor expose
//! their real capabilities; callers must not infer process, filesystem, or
//! network isolation from either backend.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::protocol::{Content, ToolResult};
use crate::tool::{Tool, ToolDef, ToolRegistry};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceKind {
    Auto,
    NativeSpace,
    NestedCompositor,
}

impl WorkspaceKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "native_space" => Some(Self::NativeSpace),
            "nested_compositor" => Some(Self::NestedCompositor),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::NativeSpace => "native_space",
            Self::NestedCompositor => "nested_compositor",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCapabilities {
    pub create: bool,
    pub attach: bool,
    pub close: bool,
    pub visual_separation: bool,
    pub separate_display_server: bool,
    pub host_focus_isolation: bool,
    pub launch: bool,
    pub move_existing_window: bool,
    pub capture: bool,
    pub input: bool,
    pub filesystem_isolation: bool,
    pub network_isolation: bool,
    pub process_isolation: bool,
    /// Always false for native spaces and nested compositors. Reserved for a
    /// future VM/sandbox backend so clients cannot confuse visual separation
    /// with a security boundary.
    pub security_isolation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceBackendDescriptor {
    pub kind: WorkspaceKind,
    pub available: bool,
    pub experimental: bool,
    pub capabilities: WorkspaceCapabilities,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CreateWorkspaceRequest {
    pub kind: WorkspaceKind,
    pub name: Option<String>,
    /// Backend-native identifier used to attach to an existing native space.
    pub native_id: Option<String>,
    pub options: Value,
}

#[derive(Debug, Clone)]
pub struct CreatedWorkspace {
    pub kind: WorkspaceKind,
    pub adopted: bool,
    pub native_id: Option<String>,
    pub capabilities: WorkspaceCapabilities,
    pub detail: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowTarget {
    pub window_id: i64,
    pub pid: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceRecord {
    pub workspace_id: String,
    pub name: Option<String>,
    pub kind: WorkspaceKind,
    pub adopted: bool,
    pub platform: String,
    pub native_id: Option<String>,
    pub status: WorkspaceStatus,
    pub created_at_ms: u128,
    pub capabilities: WorkspaceCapabilities,
    pub launched_pids: Vec<u32>,
    pub detail: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    Active,
    Closing,
    Closed,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("workspace backend is unavailable: {0}")]
    Unavailable(String),
    #[error("workspace request is unsupported: {0}")]
    Unsupported(String),
    #[error("{kind} cannot adopt an existing window; relaunch_required={relaunch_required}")]
    MoveUnsupported {
        kind: &'static str,
        relaunch_required: bool,
    },
    #[error("workspace '{0}' was not found")]
    NotFound(String),
    #[error("workspace '{0}' is already closed")]
    Closed(String),
    #[error("workspace '{0}' is closing")]
    Closing(String),
    #[error("workspace backend failed: {0}")]
    Backend(String),
    #[error("workspace '{0}' has an operation in progress")]
    Busy(String),
    #[error("workspace '{0}' is still bound to one or more sessions")]
    InUse(String),
    #[error(
        "{tool} requires workspace_ref from get_workspace_state for workspace '{workspace_id}'"
    )]
    ReferenceRequired { tool: String, workspace_id: String },
    #[error("workspace_ref '{0}' is unknown or stale")]
    InvalidReference(String),
    #[error("workspace_ref belongs to workspace '{reference_workspace}', not '{requested}'")]
    ReferenceConflict {
        reference_workspace: String,
        requested: String,
    },
    #[error("workspace_ref no longer belongs to workspace '{0}'")]
    TargetMoved(String),
    #[error("session '{session}' is already bound to workspace '{existing}', not '{requested}'")]
    BindingConflict {
        session: String,
        existing: String,
        requested: String,
    },
    #[error("session '{0}' has ended and cannot be rebound")]
    SessionEnded(String),
}

impl WorkspaceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unavailable(_) => "workspace_backend_unavailable",
            Self::Unsupported(_) => "workspace_operation_unsupported",
            Self::MoveUnsupported { .. } => "workspace_move_unsupported",
            Self::NotFound(_) => "workspace_not_found",
            Self::Closed(_) => "workspace_closed",
            Self::Closing(_) => "workspace_closing",
            Self::Backend(_) => "workspace_backend_error",
            Self::Busy(_) => "workspace_busy",
            Self::InUse(_) => "workspace_in_use",
            Self::ReferenceRequired { .. } => "workspace_ref_required",
            Self::InvalidReference(_) => "workspace_ref_stale",
            Self::ReferenceConflict { .. } => "workspace_ref_conflict",
            Self::TargetMoved(_) => "workspace_target_moved",
            Self::BindingConflict { .. } => "workspace_binding_conflict",
            Self::SessionEnded(_) => "session_ended",
        }
    }
}

/// Platform adapter. Implementations own any child processes or native
/// handles and key them by the core-generated workspace id.
#[async_trait]
pub trait WorkspaceBackend: Send + Sync {
    fn platform(&self) -> &'static str;
    fn descriptors(&self) -> Vec<WorkspaceBackendDescriptor>;
    async fn create(
        &self,
        workspace_id: &str,
        request: &CreateWorkspaceRequest,
    ) -> Result<CreatedWorkspace, WorkspaceError>;
    async fn close(&self, workspace_id: &str, launched_pids: &[u32]) -> Result<(), WorkspaceError>;
    async fn move_window(
        &self,
        workspace_id: &str,
        target: &WindowTarget,
    ) -> Result<Value, WorkspaceError>;

    /// Return a bounded, backend-native inventory of windows that currently
    /// belong to this workspace. Pixel and accessibility capture remain
    /// window-targeted; callers use the returned window ids with the existing
    /// state tools.
    async fn get_state(&self, workspace_id: &str) -> Result<Value, WorkspaceError>;

    /// Apply workspace-specific connection variables to a child command.
    /// The daemon's own process environment must never be mutated.
    fn configure_command(
        &self,
        workspace_id: &str,
        command: &mut std::process::Command,
    ) -> Result<(), WorkspaceError>;
}

pub struct WorkspaceManager {
    backend: Arc<dyn WorkspaceBackend>,
    records: RwLock<HashMap<String, WorkspaceRecord>>,
    in_flight: Mutex<HashSet<String>>,
    session_bindings: Mutex<HashMap<String, String>>,
    window_references: Mutex<HashMap<String, WorkspaceWindowReference>>,
}

#[derive(Clone)]
struct WorkspaceWindowReference {
    workspace_id: String,
    window_id: i64,
    pid: Option<i64>,
}

impl WorkspaceManager {
    pub fn new(backend: Arc<dyn WorkspaceBackend>) -> Self {
        Self {
            backend,
            records: RwLock::new(HashMap::new()),
            in_flight: Mutex::new(HashSet::new()),
            session_bindings: Mutex::new(HashMap::new()),
            window_references: Mutex::new(HashMap::new()),
        }
    }

    pub fn descriptors(&self) -> Vec<WorkspaceBackendDescriptor> {
        self.backend.descriptors()
    }

    pub async fn create(
        &self,
        request: CreateWorkspaceRequest,
    ) -> Result<WorkspaceRecord, WorkspaceError> {
        let workspace_id = format!("ws_{}", uuid::Uuid::new_v4().simple());
        let created = self.backend.create(&workspace_id, &request).await?;
        let record = WorkspaceRecord {
            workspace_id: workspace_id.clone(),
            name: request.name,
            kind: created.kind,
            adopted: created.adopted,
            platform: self.backend.platform().to_owned(),
            native_id: created.native_id,
            status: WorkspaceStatus::Active,
            created_at_ms: now_ms(),
            capabilities: created.capabilities,
            launched_pids: Vec::new(),
            detail: created.detail,
        };
        self.records
            .write()
            .unwrap()
            .insert(workspace_id, record.clone());
        Ok(record)
    }

    pub fn list(&self) -> Vec<WorkspaceRecord> {
        let mut records: Vec<_> = self.records.read().unwrap().values().cloned().collect();
        records.sort_by_key(|record| record.created_at_ms);
        records
    }

    pub fn get(&self, workspace_id: &str) -> Result<WorkspaceRecord, WorkspaceError> {
        self.records
            .read()
            .unwrap()
            .get(workspace_id)
            .cloned()
            .ok_or_else(|| WorkspaceError::NotFound(workspace_id.to_owned()))
    }

    pub async fn close(&self, workspace_id: &str, force: bool) -> Result<(), WorkspaceError> {
        let launched_pids = {
            // Lock bindings before records, matching bind_session. This makes
            // the in-use check and Active→Closing transition atomic with
            // respect to a concurrent session bind. Snapshot launched PIDs
            // only after taking the records write lock, so a launch recorded
            // immediately before Closing cannot be omitted from cleanup.
            let bindings = self.session_bindings.lock().unwrap();
            if !force && bindings.values().any(|bound| bound == workspace_id) {
                return Err(WorkspaceError::InUse(workspace_id.to_owned()));
            }
            let mut records = self.records.write().unwrap();
            let record = records
                .get_mut(workspace_id)
                .ok_or_else(|| WorkspaceError::NotFound(workspace_id.to_owned()))?;
            match record.status {
                WorkspaceStatus::Active => {
                    if self.in_flight.lock().unwrap().contains(workspace_id) {
                        return Err(WorkspaceError::Busy(workspace_id.to_owned()));
                    }
                    record.status = WorkspaceStatus::Closing;
                    record.launched_pids.clone()
                }
                WorkspaceStatus::Closed => return Ok(()),
                WorkspaceStatus::Closing => {
                    return Err(WorkspaceError::Closing(workspace_id.to_owned()))
                }
            }
        };
        if let Err(error) = self.backend.close(workspace_id, &launched_pids).await {
            if let Some(record) = self.records.write().unwrap().get_mut(workspace_id) {
                record.status = WorkspaceStatus::Active;
            }
            return Err(error);
        }
        if let Some(record) = self.records.write().unwrap().get_mut(workspace_id) {
            record.status = WorkspaceStatus::Closed;
            record.detail = json!({});
        }
        if force {
            self.session_bindings
                .lock()
                .unwrap()
                .retain(|_, bound| bound != workspace_id);
        }
        self.window_references
            .lock()
            .unwrap()
            .retain(|_, target| target.workspace_id != workspace_id);
        Ok(())
    }

    pub async fn move_window(
        &self,
        workspace_id: &str,
        target: WindowTarget,
    ) -> Result<Value, WorkspaceError> {
        let lease = self.begin_operation(workspace_id)?;
        let record = &lease.record;
        if !record.capabilities.move_existing_window {
            return Err(WorkspaceError::MoveUnsupported {
                kind: record.kind.as_str(),
                relaunch_required: record.kind == WorkspaceKind::NestedCompositor,
            });
        }
        self.backend.move_window(workspace_id, &target).await
    }

    pub async fn get_state(&self, workspace_id: &str) -> Result<Value, WorkspaceError> {
        let mut state = self.backend_state(workspace_id).await?;
        self.bind_window_references(workspace_id, &mut state)?;
        Ok(state)
    }

    async fn backend_state(&self, workspace_id: &str) -> Result<Value, WorkspaceError> {
        let _lease = self.begin_operation(workspace_id)?;
        self.backend_state_while_leased(workspace_id).await
    }

    async fn backend_state_while_leased(
        &self,
        workspace_id: &str,
    ) -> Result<Value, WorkspaceError> {
        let record = self.get_active(workspace_id)?;
        if !record.capabilities.capture {
            return Err(WorkspaceError::Unsupported(format!(
                "{} does not expose workspace state",
                record.kind.as_str()
            )));
        }
        self.backend.get_state(workspace_id).await
    }

    fn bind_window_references(
        &self,
        workspace_id: &str,
        state: &mut Value,
    ) -> Result<(), WorkspaceError> {
        let windows = state
            .get_mut("windows")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| WorkspaceError::Backend("workspace state omitted windows".into()))?;
        let mut references = self.window_references.lock().unwrap();
        references.retain(|_, target| target.workspace_id != workspace_id);
        for window in windows {
            let Some(object) = window.as_object_mut() else {
                continue;
            };
            let Some(window_id) = object.get("window_id").and_then(Value::as_i64) else {
                continue;
            };
            let reference = format!("w{}", uuid::Uuid::new_v4().simple());
            references.insert(
                reference.clone(),
                WorkspaceWindowReference {
                    workspace_id: workspace_id.to_owned(),
                    window_id,
                    pid: object.get("pid").and_then(Value::as_i64),
                },
            );
            object.insert("workspace_ref".into(), Value::String(reference));
        }
        Ok(())
    }

    pub fn resolve_workspace_id(&self, args: &Value) -> Result<Option<String>, WorkspaceError> {
        let explicit = args.get("workspace_id").and_then(Value::as_str);
        let session = args.get("session").and_then(Value::as_str);
        if let Some(explicit) = explicit {
            self.get_active(explicit)?;
            if let Some(session) = session {
                if let Some(bound) = self.workspace_for_session(session) {
                    if bound != explicit {
                        return Err(WorkspaceError::BindingConflict {
                            session: session.to_owned(),
                            existing: bound,
                            requested: explicit.to_owned(),
                        });
                    }
                }
            }
            return Ok(Some(explicit.to_owned()));
        }
        let resolved = session.and_then(|id| self.workspace_for_session(id));
        if let Some(workspace_id) = resolved.as_deref() {
            self.get_active(workspace_id)?;
        }
        Ok(resolved)
    }

    pub fn configure_command(
        &self,
        workspace_id: &str,
        command: &mut std::process::Command,
    ) -> Result<(), WorkspaceError> {
        let record = self.get_active(workspace_id)?;
        if !record.capabilities.launch {
            return Err(WorkspaceError::Unsupported(format!(
                "{} cannot launch applications",
                record.kind.as_str()
            )));
        }
        self.backend.configure_command(workspace_id, command)
    }

    pub fn note_launch(&self, workspace_id: &str, pid: u32) -> Result<(), WorkspaceError> {
        let mut records = self.records.write().unwrap();
        let record = records
            .get_mut(workspace_id)
            .ok_or_else(|| WorkspaceError::NotFound(workspace_id.to_owned()))?;
        match record.status {
            WorkspaceStatus::Active => {
                if !record.launched_pids.contains(&pid) {
                    record.launched_pids.push(pid);
                }
                Ok(())
            }
            WorkspaceStatus::Closing => Err(WorkspaceError::Closing(workspace_id.to_owned())),
            WorkspaceStatus::Closed => Err(WorkspaceError::Closed(workspace_id.to_owned())),
        }
    }

    fn get_active(&self, workspace_id: &str) -> Result<WorkspaceRecord, WorkspaceError> {
        let record = self.get(workspace_id)?;
        match record.status {
            WorkspaceStatus::Active => Ok(record),
            WorkspaceStatus::Closing => Err(WorkspaceError::Closing(workspace_id.to_owned())),
            WorkspaceStatus::Closed => Err(WorkspaceError::Closed(workspace_id.to_owned())),
        }
    }

    fn begin_operation(&self, workspace_id: &str) -> Result<WorkspaceLease<'_>, WorkspaceError> {
        let records = self.records.read().unwrap();
        let record = records
            .get(workspace_id)
            .cloned()
            .ok_or_else(|| WorkspaceError::NotFound(workspace_id.to_owned()))?;
        match record.status {
            WorkspaceStatus::Active => {}
            WorkspaceStatus::Closing => {
                return Err(WorkspaceError::Closing(workspace_id.to_owned()))
            }
            WorkspaceStatus::Closed => return Err(WorkspaceError::Closed(workspace_id.to_owned())),
        }
        let mut in_flight = self.in_flight.lock().unwrap();
        if !in_flight.insert(workspace_id.to_owned()) {
            return Err(WorkspaceError::Busy(workspace_id.to_owned()));
        }
        drop(in_flight);
        drop(records);
        Ok(WorkspaceLease {
            manager: self,
            workspace_id: workspace_id.to_owned(),
            record,
        })
    }

    /// Hold a workspace active across a multi-step platform operation such as
    /// configure → spawn → move → ownership recording. `close_workspace`
    /// returns `workspace_busy` until the lease is dropped.
    pub fn lease_operation(
        self: &Arc<Self>,
        workspace_id: &str,
    ) -> Result<WorkspaceOperationLease, WorkspaceError> {
        let records = self.records.read().unwrap();
        let record = records
            .get(workspace_id)
            .ok_or_else(|| WorkspaceError::NotFound(workspace_id.to_owned()))?;
        match record.status {
            WorkspaceStatus::Active => {}
            WorkspaceStatus::Closing => {
                return Err(WorkspaceError::Closing(workspace_id.to_owned()))
            }
            WorkspaceStatus::Closed => return Err(WorkspaceError::Closed(workspace_id.to_owned())),
        }
        let mut in_flight = self.in_flight.lock().unwrap();
        if !in_flight.insert(workspace_id.to_owned()) {
            return Err(WorkspaceError::Busy(workspace_id.to_owned()));
        }
        drop(in_flight);
        drop(records);
        Ok(WorkspaceOperationLease {
            manager: self.clone(),
            workspace_id: workspace_id.to_owned(),
        })
    }

    pub fn bind_session(
        &self,
        session: &str,
        requested: Option<&str>,
    ) -> Result<Option<String>, WorkspaceError> {
        let mut bindings = self.session_bindings.lock().unwrap();
        if crate::session::is_session_ended(session) {
            return Err(WorkspaceError::SessionEnded(session.to_owned()));
        }
        if let Some(existing) = bindings.get(session) {
            if requested.is_some_and(|value| value != existing) {
                return Err(WorkspaceError::BindingConflict {
                    session: session.to_owned(),
                    existing: existing.clone(),
                    requested: requested.unwrap_or_default().to_owned(),
                });
            }
            return Ok(Some(existing.clone()));
        }
        let Some(requested) = requested else {
            return Ok(None);
        };
        self.get_active(requested)?;
        bindings.insert(session.to_owned(), requested.to_owned());
        Ok(Some(requested.to_owned()))
    }

    pub fn workspace_for_session(&self, session: &str) -> Option<String> {
        self.session_bindings.lock().unwrap().get(session).cloned()
    }

    pub fn clear_session(&self, session: &str) {
        self.session_bindings.lock().unwrap().remove(session);
    }
}

struct WorkspaceLease<'a> {
    manager: &'a WorkspaceManager,
    workspace_id: String,
    record: WorkspaceRecord,
}

pub struct WorkspaceOperationLease {
    manager: Arc<WorkspaceManager>,
    workspace_id: String,
}

fn finish_operation(manager: &WorkspaceManager, workspace_id: &str) {
    manager.in_flight.lock().unwrap().remove(workspace_id);
}

impl Drop for WorkspaceLease<'_> {
    fn drop(&mut self) {
        finish_operation(self.manager, &self.workspace_id);
    }
}

impl WorkspaceOperationLease {
    pub fn note_launch(&self, pid: u32) -> Result<(), WorkspaceError> {
        self.manager.note_launch(&self.workspace_id, pid)
    }

    pub async fn move_window(&self, target: &WindowTarget) -> Result<Value, WorkspaceError> {
        let record = self.manager.get_active(&self.workspace_id)?;
        if !record.capabilities.move_existing_window {
            return Err(WorkspaceError::MoveUnsupported {
                kind: record.kind.as_str(),
                relaunch_required: record.kind == WorkspaceKind::NestedCompositor,
            });
        }
        self.manager
            .backend
            .move_window(&self.workspace_id, target)
            .await
    }
}

impl Drop for WorkspaceOperationLease {
    fn drop(&mut self) {
        finish_operation(&self.manager, &self.workspace_id);
    }
}

tokio::task_local! {
    static CURRENT_MANAGER: Arc<WorkspaceManager>;
}

pub async fn with_manager<F>(manager: Arc<WorkspaceManager>, future: F) -> F::Output
where
    F: std::future::Future,
{
    CURRENT_MANAGER.scope(manager, future).await
}

pub fn current_manager() -> Option<Arc<WorkspaceManager>> {
    CURRENT_MANAGER.try_with(Arc::clone).ok()
}

pub fn resolve_workspace_id(args: &Value) -> Result<Option<String>, WorkspaceError> {
    let Some(manager) = current_manager() else {
        return if args.get("workspace_id").and_then(Value::as_str).is_some() {
            Err(WorkspaceError::Unavailable(
                "this host did not register a workspace backend".into(),
            ))
        } else {
            Ok(None)
        };
    };
    manager.resolve_workspace_id(args)
}

/// Resolve a workspace-bound window reference, verify that its native window
/// still belongs to the workspace, and hold the workspace open until dispatch
/// completes. Raw pid/window targeting is deliberately rejected for bound
/// sessions because a process can own windows in several workspaces.
pub async fn validate_operation_target(
    manager: Option<&Arc<WorkspaceManager>>,
    workspace_targeted: bool,
    tool_name: &str,
    args: &mut Value,
) -> Result<Option<WorkspaceOperationLease>, WorkspaceError> {
    if !workspace_targeted {
        return Ok(None);
    }
    let Some(manager) = manager else {
        return Ok(None);
    };
    let Some(workspace_id) = manager.resolve_workspace_id(args)? else {
        return Ok(None);
    };
    let reference = args
        .get("workspace_ref")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkspaceError::ReferenceRequired {
            tool: tool_name.to_owned(),
            workspace_id: workspace_id.clone(),
        })?;
    let target = manager
        .window_references
        .lock()
        .unwrap()
        .get(reference)
        .cloned()
        .ok_or_else(|| WorkspaceError::InvalidReference(reference.to_owned()))?;
    if target.workspace_id != workspace_id {
        return Err(WorkspaceError::ReferenceConflict {
            reference_workspace: target.workspace_id,
            requested: workspace_id,
        });
    }
    if args
        .get("window_id")
        .and_then(Value::as_i64)
        .is_some_and(|window_id| window_id != target.window_id)
        || args
            .get("pid")
            .and_then(Value::as_i64)
            .zip(target.pid)
            .is_some_and(|(pid, target_pid)| pid != target_pid)
    {
        return Err(WorkspaceError::Unsupported(
            "workspace_ref conflicts with the supplied native target".into(),
        ));
    }
    let lease = manager.lease_operation(&workspace_id)?;
    let state = manager.backend_state_while_leased(&workspace_id).await?;
    let belongs = state
        .get("windows")
        .and_then(Value::as_array)
        .is_some_and(|windows| {
            windows.iter().any(|window| {
                window.get("window_id").and_then(Value::as_i64) == Some(target.window_id)
            })
        });
    if !belongs {
        return Err(WorkspaceError::TargetMoved(workspace_id));
    }
    let object = args
        .as_object_mut()
        .ok_or_else(|| WorkspaceError::Unsupported("tool arguments must be an object".into()))?;
    object.insert("window_id".into(), Value::from(target.window_id));
    if let Some(pid) = target.pid {
        object.entry("pid").or_insert_with(|| Value::from(pid));
    }
    Ok(Some(lease))
}

pub fn configure_command_for_args(
    args: &Value,
    command: &mut std::process::Command,
) -> Result<Option<String>, WorkspaceError> {
    let Some(workspace_id) = resolve_workspace_id(args)? else {
        return Ok(None);
    };
    current_manager()
        .expect("workspace manager resolved")
        .configure_command(&workspace_id, command)?;
    Ok(Some(workspace_id))
}

pub fn bind_session(
    session: &str,
    requested: Option<&str>,
) -> Result<Option<String>, WorkspaceError> {
    match current_manager() {
        Some(manager) => manager.bind_session(session, requested),
        None if requested.is_some() => Err(WorkspaceError::Unavailable(
            "this platform did not register a workspace backend".into(),
        )),
        None => Ok(None),
    }
}

/// Validate a requested workspace before another session subsystem mutates its
/// own state. The real bind repeats this check to close races.
pub fn validate_workspace_id(requested: Option<&str>) -> Result<(), WorkspaceError> {
    let Some(requested) = requested else {
        return Ok(());
    };
    let manager = current_manager().ok_or_else(|| {
        WorkspaceError::Unavailable("this host did not register a workspace backend".into())
    })?;
    manager.get_active(requested).map(|_| ())
}

pub fn clear_session(session: &str) {
    if let Some(manager) = current_manager() {
        manager.clear_session(session);
    }
}

pub fn register_tools(
    registry: &mut ToolRegistry,
    backend: Arc<dyn WorkspaceBackend>,
) -> Arc<WorkspaceManager> {
    let manager = Arc::new(WorkspaceManager::new(backend));
    let host_namespace = registry.host_namespace().to_owned();
    registry.set_workspace_manager(manager.clone());
    let weak_manager = Arc::downgrade(&manager);
    crate::session::register_session_end_hook_for_namespace(host_namespace, move |session| {
        if let Some(manager) = weak_manager.upgrade() {
            manager.clear_session(session);
        }
    });
    registry.register(Box::new(ListBackendsTool(manager.clone())));
    registry.register(Box::new(CreateWorkspaceTool(manager.clone())));
    registry.register(Box::new(ListWorkspacesTool(manager.clone())));
    registry.register(Box::new(GetWorkspaceTool(manager.clone())));
    registry.register(Box::new(GetWorkspaceStateTool(manager.clone())));
    registry.register(Box::new(CloseWorkspaceTool(manager.clone())));
    registry.register(Box::new(MoveWindowTool(manager.clone())));
    manager
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn tool_error(error: WorkspaceError) -> ToolResult {
    let mut structured = json!({
        "code": error.code(),
        "message": error.to_string(),
    });
    if let WorkspaceError::MoveUnsupported {
        relaunch_required, ..
    } = &error
    {
        structured["relaunch_required"] = Value::Bool(*relaunch_required);
        structured["hint"] =
            Value::String("Relaunch the application with launch_app{workspace_id}.".into());
    }
    ToolResult::error(error.to_string()).with_structured(structured)
}

fn def(
    name: &str,
    description: &str,
    schema: Value,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
) -> ToolDef {
    ToolDef {
        name: name.into(),
        description: description.into(),
        input_schema: schema,
        read_only,
        destructive,
        idempotent,
        open_world: false,
    }
}

struct ListBackendsTool(Arc<WorkspaceManager>);
static LIST_BACKENDS_DEF: OnceLock<ToolDef> = OnceLock::new();
#[async_trait]
impl Tool for ListBackendsTool {
    fn def(&self) -> &ToolDef {
        LIST_BACKENDS_DEF.get_or_init(|| def("list_workspace_backends", "List workspace backends and their actual platform capabilities. Native spaces and nested compositors are visual/routing boundaries, not security sandboxes.", json!({"type":"object","additionalProperties":true}), true, false, true))
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        let backends = self.0.descriptors();
        ToolResult::text(format!("{} workspace backend(s) reported.", backends.len()))
            .with_structured(json!({"platform": self.0.backend.platform(), "backends": backends}))
    }
}

struct CreateWorkspaceTool(Arc<WorkspaceManager>);
static CREATE_DEF: OnceLock<ToolDef> = OnceLock::new();
#[async_trait]
impl Tool for CreateWorkspaceTool {
    fn def(&self) -> &ToolDef {
        CREATE_DEF.get_or_init(|| def("create_workspace", "Create or attach to an isolated visual workspace. Use list_workspace_backends first; unsupported lifecycle operations fail explicitly.", json!({
            "type":"object",
            "properties":{
                "backend":{"type":"string","enum":["auto","native_space","nested_compositor"],"default":"auto"},
                "name":{"type":"string","maxLength":128},
                "native_id":{"type":"string","description":"Existing platform-native Space/desktop id to attach to when creation is unsupported."},
                "options":{"type":"object","additionalProperties":true}
            },
            "additionalProperties":true
        }), false, false, false))
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let raw_kind = args
            .get("backend")
            .and_then(Value::as_str)
            .unwrap_or("auto");
        let Some(kind) = WorkspaceKind::parse(raw_kind) else {
            return tool_error(WorkspaceError::Unsupported(format!(
                "unknown workspace backend '{raw_kind}'"
            )));
        };
        let request = CreateWorkspaceRequest {
            kind,
            name: args.get("name").and_then(Value::as_str).map(str::to_owned),
            native_id: args
                .get("native_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            options: args.get("options").cloned().unwrap_or_else(|| json!({})),
        };
        match self.0.create(request).await {
            Ok(record) => {
                ToolResult::text(format!("Workspace '{}' is active.", record.workspace_id))
                    .with_structured(json!({"workspace": record}))
            }
            Err(error) => tool_error(error),
        }
    }
}

struct ListWorkspacesTool(Arc<WorkspaceManager>);
static LIST_DEF: OnceLock<ToolDef> = OnceLock::new();
#[async_trait]
impl Tool for ListWorkspacesTool {
    fn def(&self) -> &ToolDef {
        LIST_DEF.get_or_init(|| {
            def(
                "list_workspaces",
                "List workspaces managed by this driver process, including closed tombstones.",
                json!({"type":"object","additionalProperties":true}),
                true,
                false,
                true,
            )
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        let workspaces = self.0.list();
        ToolResult::text(format!("{} workspace record(s).", workspaces.len()))
            .with_structured(json!({"workspaces":workspaces}))
    }
}

struct GetWorkspaceTool(Arc<WorkspaceManager>);
static GET_DEF: OnceLock<ToolDef> = OnceLock::new();
#[async_trait]
impl Tool for GetWorkspaceTool {
    fn def(&self) -> &ToolDef {
        GET_DEF.get_or_init(|| {
            def(
                "get_workspace",
                "Read one workspace, its lifecycle status, and its capabilities.",
                workspace_id_schema(),
                true,
                false,
                true,
            )
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let Some(id) = args.get("workspace_id").and_then(Value::as_str) else {
            return ToolResult::error("get_workspace requires workspace_id");
        };
        match self.0.get(id) {
            Ok(record) => {
                ToolResult::text(format!("Workspace '{id}' status is '{:?}'.", record.status))
                    .with_structured(json!({"workspace":record}))
            }
            Err(error) => tool_error(error),
        }
    }
}

struct GetWorkspaceStateTool(Arc<WorkspaceManager>);
static GET_STATE_DEF: OnceLock<ToolDef> = OnceLock::new();
#[async_trait]
impl Tool for GetWorkspaceStateTool {
    fn def(&self) -> &ToolDef {
        GET_STATE_DEF.get_or_init(|| {
            def(
                "get_workspace_state",
                "Capture a bounded visual overview and list windows currently belonging to a workspace. Pass each returned workspace_ref to get_window_state and window-targeted input tools; a newer state call makes older refs stale.",
                json!({
                    "type":"object",
                    "properties":{
                        "workspace_id":{"type":"string"},
                        "session":{"type":"string","description":"Session whose workspace binding is used when workspace_id is omitted."}
                    },
                    "anyOf":[{"required":["workspace_id"]},{"required":["session"]}],
                    "additionalProperties":false
                }),
                true,
                false,
                true,
            )
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let workspace_id = match self.0.resolve_workspace_id(&args) {
            Ok(Some(workspace_id)) => workspace_id,
            Ok(None) => {
                return ToolResult::error(
                    "get_workspace_state requires workspace_id or a workspace-bound session",
                )
            }
            Err(error) => return tool_error(error),
        };
        let record = match self.0.get(&workspace_id) {
            Ok(record) => record,
            Err(error) => return tool_error(error),
        };
        match self.0.get_state(&workspace_id).await {
            Ok(mut state) => {
                let overview = state
                    .as_object_mut()
                    .and_then(|object| object.remove("overview_png"))
                    .and_then(|value| value.as_str().map(str::to_owned));
                if let Some(object) = state.as_object_mut() {
                    object.insert("overview_available".into(), Value::Bool(overview.is_some()));
                }
                let mut result =
                    ToolResult::text(format!("Workspace '{workspace_id}' state is available."));
                if let Some(overview) = overview {
                    result.content.push(Content::image_png(overview));
                }
                result.with_structured(json!({"workspace":record,"state":state}))
            }
            Err(error) => tool_error(error),
        }
    }
}

struct CloseWorkspaceTool(Arc<WorkspaceManager>);
static CLOSE_DEF: OnceLock<ToolDef> = OnceLock::new();
#[async_trait]
impl Tool for CloseWorkspaceTool {
    fn def(&self) -> &ToolDef {
        CLOSE_DEF.get_or_init(|| def("close_workspace", "Close a workspace and release backend-owned resources. A nested compositor also terminates verified child processes previously launched into it before removing its ephemeral profile; adopted native spaces/desktops and their apps are never destroyed. Refuses while sessions are bound unless force=true.", json!({
            "type":"object","required":["workspace_id"],"properties":{
                "workspace_id":{"type":"string"},"force":{"type":"boolean","default":false}
            },"additionalProperties":true
        }), false, true, true))
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let Some(id) = args.get("workspace_id").and_then(Value::as_str) else {
            return ToolResult::error("close_workspace requires workspace_id");
        };
        match self
            .0
            .close(
                id,
                args.get("force").and_then(Value::as_bool).unwrap_or(false),
            )
            .await
        {
            Ok(()) => ToolResult::text(format!("Workspace '{id}' closed."))
                .with_structured(json!({"workspace_id":id,"status":"closed"})),
            Err(error) => tool_error(error),
        }
    }
}

struct MoveWindowTool(Arc<WorkspaceManager>);
static MOVE_DEF: OnceLock<ToolDef> = OnceLock::new();
#[async_trait]
impl Tool for MoveWindowTool {
    fn def(&self) -> &ToolDef {
        MOVE_DEF.get_or_init(|| def("move_window_to_workspace", "Move an existing native window into a workspace when the backend supports adoption. Nested compositor clients generally must be relaunched instead.", json!({
            "type":"object","required":["workspace_id","window_id"],"properties":{
                "workspace_id":{"type":"string"},"window_id":{"type":"integer"},"pid":{"type":"integer"}
            },"additionalProperties":true
        }), false, false, false))
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let Some(id) = args.get("workspace_id").and_then(Value::as_str) else {
            return ToolResult::error("move_window_to_workspace requires workspace_id");
        };
        let Some(window_id) = args.get("window_id").and_then(Value::as_i64) else {
            return ToolResult::error("move_window_to_workspace requires integer window_id");
        };
        let target = WindowTarget {
            window_id,
            pid: args.get("pid").and_then(Value::as_i64),
        };
        match self.0.move_window(id, target).await {
            Ok(detail) => {
                ToolResult::text(format!("Window {window_id} moved to workspace '{id}'."))
                    .with_structured(
                        json!({"workspace_id":id,"window_id":window_id,"detail":detail}),
                    )
            }
            Err(error) => tool_error(error),
        }
    }
}

fn workspace_id_schema() -> Value {
    json!({"type":"object","required":["workspace_id"],"properties":{"workspace_id":{"type":"string"}},"additionalProperties":true})
}

/// Copy an environment overlay to a child without touching the process-global
/// environment. Useful to adapters that expose their connection parameters.
pub fn apply_environment<I, K, V>(command: &mut std::process::Command, entries: I)
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    for (key, value) in entries {
        command.env(key, value);
    }
}

/// Owned environment shape used by backend status/details and tests.
pub type WorkspaceEnvironment = Vec<(OsString, OsString)>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[derive(Default)]
    struct FakeBackend {
        live: Mutex<HashSet<String>>,
        closed_pids: Mutex<Vec<Vec<u32>>>,
    }

    fn fake_capabilities() -> WorkspaceCapabilities {
        WorkspaceCapabilities {
            create: true,
            attach: false,
            close: true,
            visual_separation: true,
            separate_display_server: true,
            host_focus_isolation: true,
            launch: true,
            move_existing_window: false,
            capture: true,
            input: true,
            filesystem_isolation: false,
            network_isolation: false,
            process_isolation: false,
            security_isolation: false,
        }
    }

    #[async_trait]
    impl WorkspaceBackend for FakeBackend {
        fn platform(&self) -> &'static str {
            "test"
        }

        fn descriptors(&self) -> Vec<WorkspaceBackendDescriptor> {
            vec![WorkspaceBackendDescriptor {
                kind: WorkspaceKind::NestedCompositor,
                available: true,
                experimental: false,
                capabilities: fake_capabilities(),
                detail: None,
            }]
        }

        async fn create(
            &self,
            workspace_id: &str,
            _request: &CreateWorkspaceRequest,
        ) -> Result<CreatedWorkspace, WorkspaceError> {
            self.live.lock().unwrap().insert(workspace_id.to_owned());
            Ok(CreatedWorkspace {
                kind: WorkspaceKind::NestedCompositor,
                adopted: false,
                native_id: None,
                capabilities: fake_capabilities(),
                detail: json!({"fake": true}),
            })
        }

        async fn close(
            &self,
            workspace_id: &str,
            launched_pids: &[u32],
        ) -> Result<(), WorkspaceError> {
            if self.live.lock().unwrap().remove(workspace_id) {
                self.closed_pids
                    .lock()
                    .unwrap()
                    .push(launched_pids.to_vec());
                Ok(())
            } else {
                Err(WorkspaceError::NotFound(workspace_id.to_owned()))
            }
        }

        async fn move_window(
            &self,
            _workspace_id: &str,
            _target: &WindowTarget,
        ) -> Result<Value, WorkspaceError> {
            unreachable!("manager rejects this backend's unsupported move first")
        }

        async fn get_state(&self, workspace_id: &str) -> Result<Value, WorkspaceError> {
            if !self.live.lock().unwrap().contains(workspace_id) {
                return Err(WorkspaceError::NotFound(workspace_id.to_owned()));
            }
            Ok(json!({"windows":[{"window_id":7,"pid":42}]}))
        }

        fn configure_command(
            &self,
            workspace_id: &str,
            command: &mut std::process::Command,
        ) -> Result<(), WorkspaceError> {
            if !self.live.lock().unwrap().contains(workspace_id) {
                return Err(WorkspaceError::NotFound(workspace_id.to_owned()));
            }
            command.env("CUA_FAKE_WORKSPACE", workspace_id);
            Ok(())
        }
    }

    #[test]
    fn kinds_are_strict_and_stable() {
        assert_eq!(
            WorkspaceKind::parse("native_space"),
            Some(WorkspaceKind::NativeSpace)
        );
        assert_eq!(
            WorkspaceKind::parse("nested_compositor"),
            Some(WorkspaceKind::NestedCompositor)
        );
        assert_eq!(WorkspaceKind::parse("space"), None);
    }

    #[test]
    fn security_isolation_is_explicit_not_inferred() {
        let caps = WorkspaceCapabilities {
            create: true,
            attach: false,
            close: true,
            visual_separation: true,
            separate_display_server: true,
            host_focus_isolation: true,
            launch: true,
            move_existing_window: false,
            capture: true,
            input: true,
            filesystem_isolation: false,
            network_isolation: false,
            process_isolation: false,
            security_isolation: false,
        };
        assert!(!caps.security_isolation);
        assert!(!caps.filesystem_isolation);
        assert!(!caps.network_isolation);
        assert!(!caps.process_isolation);
    }

    #[tokio::test]
    async fn manager_owns_lifecycle_and_refuses_bound_close() {
        let backend = Arc::new(FakeBackend::default());
        let manager = WorkspaceManager::new(backend.clone());
        let created = manager
            .create(CreateWorkspaceRequest {
                kind: WorkspaceKind::NestedCompositor,
                name: Some("test space".into()),
                native_id: None,
                options: json!({}),
            })
            .await
            .unwrap();
        assert_eq!(manager.list().len(), 1);
        assert_eq!(
            manager.get(&created.workspace_id).unwrap().name.as_deref(),
            Some("test space")
        );

        let lease = manager.begin_operation(&created.workspace_id).unwrap();
        assert!(matches!(
            manager.begin_operation(&created.workspace_id),
            Err(WorkspaceError::Busy(_))
        ));
        assert!(matches!(
            manager.close(&created.workspace_id, true).await,
            Err(WorkspaceError::Busy(_))
        ));
        drop(lease);

        let bound = manager
            .bind_session("session-a", Some(&created.workspace_id))
            .unwrap();
        assert_eq!(bound.as_deref(), Some(created.workspace_id.as_str()));
        let state = manager.get_state(&created.workspace_id).await.unwrap();
        assert_eq!(state["windows"][0]["window_id"], 7);
        assert_eq!(
            manager
                .resolve_workspace_id(&json!({"session":"session-a"}))
                .unwrap()
                .as_deref(),
            Some(created.workspace_id.as_str())
        );
        assert!(matches!(
            manager.close(&created.workspace_id, false).await,
            Err(WorkspaceError::InUse(_))
        ));

        let mut command = std::process::Command::new("ignored");
        manager
            .configure_command(&created.workspace_id, &mut command)
            .unwrap();
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "CUA_FAKE_WORKSPACE" && value.is_some()));

        manager.note_launch(&created.workspace_id, 4242).unwrap();
        manager.close(&created.workspace_id, true).await.unwrap();
        assert_eq!(*backend.closed_pids.lock().unwrap(), vec![vec![4242]]);
        assert_eq!(manager.list().len(), 1);
        assert_eq!(
            manager.get(&created.workspace_id).unwrap().status,
            WorkspaceStatus::Closed
        );
        assert_eq!(manager.workspace_for_session("session-a"), None);
    }

    #[tokio::test]
    async fn workspace_refs_are_scoped_exact_and_stale_on_refresh() {
        let manager = Arc::new(WorkspaceManager::new(Arc::new(FakeBackend::default())));
        let first = manager
            .create(CreateWorkspaceRequest {
                kind: WorkspaceKind::NestedCompositor,
                name: None,
                native_id: None,
                options: json!({}),
            })
            .await
            .unwrap();
        let second = manager
            .create(CreateWorkspaceRequest {
                kind: WorkspaceKind::NestedCompositor,
                name: None,
                native_id: None,
                options: json!({}),
            })
            .await
            .unwrap();
        manager
            .bind_session("workspace-ref-session", Some(&first.workspace_id))
            .unwrap();
        let state = manager.get_state(&first.workspace_id).await.unwrap();
        let reference = state["windows"][0]["workspace_ref"]
            .as_str()
            .unwrap()
            .to_owned();

        let mut args = json!({
            "session":"workspace-ref-session",
            "workspace_ref":reference,
        });
        let lease = validate_operation_target(Some(&manager), true, "click", &mut args)
            .await
            .unwrap();
        assert_eq!(args["window_id"], 7);
        assert_eq!(args["pid"], 42);
        drop(lease);

        let mut raw_pid = json!({"session":"workspace-ref-session","pid":42});
        assert!(
            validate_operation_target(Some(&manager), true, "type_text", &mut raw_pid)
                .await
                .is_err()
        );

        let mut wrong_workspace = json!({
            "workspace_id":second.workspace_id,
            "workspace_ref":reference,
        });
        assert!(matches!(
            validate_operation_target(Some(&manager), true, "click", &mut wrong_workspace).await,
            Err(WorkspaceError::ReferenceConflict { .. })
        ));

        manager.get_state(&first.workspace_id).await.unwrap();
        let mut stale = json!({
            "workspace_id":first.workspace_id,
            "workspace_ref":reference,
        });
        assert!(
            validate_operation_target(Some(&manager), true, "click", &mut stale)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn registries_own_independent_workspace_managers() {
        let mut first_registry = ToolRegistry::new();
        let first = register_tools(&mut first_registry, Arc::new(FakeBackend::default()));
        let mut second_registry = ToolRegistry::new();
        let second = register_tools(&mut second_registry, Arc::new(FakeBackend::default()));
        assert!(!Arc::ptr_eq(&first, &second));

        first
            .create(CreateWorkspaceRequest {
                kind: WorkspaceKind::NestedCompositor,
                name: None,
                native_id: None,
                options: json!({}),
            })
            .await
            .unwrap();
        assert_eq!(first.list().len(), 1);
        assert!(second.list().is_empty());
    }

    #[tokio::test]
    async fn registries_isolate_same_named_session_lifecycle() {
        let session = format!("shared-host-session-{}", uuid::Uuid::new_v4());
        let mut first_registry = ToolRegistry::new();
        let first = register_tools(&mut first_registry, Arc::new(FakeBackend::default()));
        first_registry.register_session_tools();
        let mut second_registry = ToolRegistry::new();
        let second = register_tools(&mut second_registry, Arc::new(FakeBackend::default()));
        second_registry.register_session_tools();
        let first_workspace = first
            .create(CreateWorkspaceRequest {
                kind: WorkspaceKind::NestedCompositor,
                name: None,
                native_id: None,
                options: json!({}),
            })
            .await
            .unwrap();
        let second_workspace = second
            .create(CreateWorkspaceRequest {
                kind: WorkspaceKind::NestedCompositor,
                name: None,
                native_id: None,
                options: json!({}),
            })
            .await
            .unwrap();

        for (registry, workspace_id) in [
            (&first_registry, &first_workspace.workspace_id),
            (&second_registry, &second_workspace.workspace_id),
        ] {
            let result = registry
                .invoke(
                    "start_session",
                    json!({"session":session,"workspace_id":workspace_id}),
                )
                .await;
            assert_ne!(result.is_error, Some(true));
        }

        assert!(first_registry.fire_session_end(&session).await);
        let second_state = second_registry
            .invoke("get_session_state", json!({"session":session}))
            .await;
        assert_ne!(second_state.is_error, Some(true));
        assert_eq!(
            second_state.structured_content.unwrap()["workspace_id"],
            second_workspace.workspace_id
        );
    }

    #[tokio::test]
    async fn unsupported_window_adoption_fails_before_backend_dispatch() {
        let manager = WorkspaceManager::new(Arc::new(FakeBackend::default()));
        let created = manager
            .create(CreateWorkspaceRequest {
                kind: WorkspaceKind::NestedCompositor,
                name: None,
                native_id: None,
                options: json!({}),
            })
            .await
            .unwrap();
        let error = manager
            .move_window(
                &created.workspace_id,
                WindowTarget {
                    window_id: 42,
                    pid: Some(7),
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            WorkspaceError::MoveUnsupported {
                relaunch_required: true,
                ..
            }
        ));
    }

    #[test]
    fn ended_session_cannot_resurrect_a_workspace_binding() {
        let manager = WorkspaceManager::new(Arc::new(FakeBackend::default()));
        let session = format!("workspace-ended-bind-{}", uuid::Uuid::new_v4());
        crate::session::fire_session_end(&session);
        let error = manager
            .bind_session(&session, Some("ws_missing"))
            .unwrap_err();
        assert!(matches!(error, WorkspaceError::SessionEnded(ref id) if id == &session));
    }

    #[tokio::test]
    async fn registration_exposes_the_complete_workspace_surface() {
        let mut registry = ToolRegistry::new();
        register_tools(&mut registry, Arc::new(FakeBackend::default()));
        let names: HashSet<_> = registry.tool_names().collect();
        for expected in [
            "list_workspace_backends",
            "create_workspace",
            "list_workspaces",
            "get_workspace",
            "get_workspace_state",
            "close_workspace",
            "move_window_to_workspace",
        ] {
            assert!(names.contains(expected), "missing tool {expected}");
        }
        let result = registry.invoke("list_workspace_backends", json!({})).await;
        assert_ne!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.as_ref().unwrap()["platform"],
            "test"
        );
    }
}
