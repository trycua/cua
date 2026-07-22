//! Experimental macOS native-Space backend.
//!
//! Mission Control has no public API for arbitrary third-party window/Space
//! management. This adapter therefore requires an explicit opt-in, dynamically
//! resolves private SkyLight symbols, attaches only to a caller-known Space id,
//! and verifies every move by reading the window's Space membership back.

use std::collections::HashMap;
use std::ffi::{c_char, c_void};
use std::sync::{Mutex, OnceLock};

use async_trait::async_trait;
use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::TCFType;
use core_foundation::number::CFNumber;
use cua_driver_core::workspace::{
    CreateWorkspaceRequest, CreatedWorkspace, WindowTarget, WorkspaceBackend,
    WorkspaceBackendDescriptor, WorkspaceCapabilities, WorkspaceError, WorkspaceKind,
};
use serde_json::json;

pub const PRIVATE_SPACES_ENV: &str = "CUA_DRIVER_ENABLE_PRIVATE_SPACES";

type Connection = u32;
type SpaceId = u64;
type MainConnectionFn = unsafe extern "C" fn() -> Connection;
type GetActiveSpaceFn = unsafe extern "C" fn(Connection) -> SpaceId;
type CopySpacesForWindowsFn = unsafe extern "C" fn(Connection, u32, CFArrayRef) -> CFArrayRef;
type AddWindowsToSpacesFn = unsafe extern "C" fn(Connection, CFArrayRef, CFArrayRef);
type RemoveWindowsFromSpacesFn = unsafe extern "C" fn(Connection, CFArrayRef, CFArrayRef);

#[derive(Clone, Copy)]
struct PrivateSpaceApi {
    main_connection: MainConnectionFn,
    get_active_space: GetActiveSpaceFn,
    copy_spaces_for_windows: CopySpacesForWindowsFn,
    add_windows_to_spaces: AddWindowsToSpacesFn,
    remove_windows_from_spaces: RemoveWindowsFromSpacesFn,
}

impl PrivateSpaceApi {
    fn load() -> Option<Self> {
        static API: OnceLock<Option<PrivateSpaceApi>> = OnceLock::new();
        *API.get_or_init(|| unsafe {
            let path = b"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight\0";
            let handle = libc::dlopen(
                path.as_ptr().cast::<c_char>(),
                libc::RTLD_LAZY | libc::RTLD_LOCAL,
            );
            if handle.is_null() {
                return None;
            }
            Some(Self {
                main_connection: symbol(handle, b"CGSMainConnectionID\0")?,
                get_active_space: symbol(handle, b"CGSGetActiveSpace\0")?,
                copy_spaces_for_windows: symbol(handle, b"CGSCopySpacesForWindows\0")?,
                add_windows_to_spaces: symbol(handle, b"CGSAddWindowsToSpaces\0")?,
                remove_windows_from_spaces: symbol(handle, b"CGSRemoveWindowsFromSpaces\0")?,
            })
        })
    }

    fn active_space(self) -> SpaceId {
        unsafe { (self.get_active_space)((self.main_connection)()) }
    }

    fn spaces_for_window(self, window_id: u32) -> Result<Vec<SpaceId>, WorkspaceError> {
        let windows = CFArray::from_CFTypes(&[CFNumber::from(i64::from(window_id))]);
        let raw = unsafe {
            (self.copy_spaces_for_windows)(
                (self.main_connection)(),
                0x7,
                windows.as_concrete_TypeRef(),
            )
        };
        if raw.is_null() {
            return Err(WorkspaceError::Backend(format!(
                "CGSCopySpacesForWindows returned null for window {window_id}"
            )));
        }
        let spaces: CFArray<CFNumber> = unsafe { TCFType::wrap_under_create_rule(raw) };
        Ok(spaces
            .iter()
            .filter_map(|number| number.to_i64())
            .filter_map(|id| u64::try_from(id).ok())
            .collect())
    }

    fn move_window(self, window_id: u32, target: SpaceId) -> Result<(), WorkspaceError> {
        let current = self.spaces_for_window(window_id)?;
        if current.len() == 1 && current[0] == target {
            return Ok(());
        }
        let windows = CFArray::from_CFTypes(&[CFNumber::from(i64::from(window_id))]);
        let target_i64 = i64::try_from(target)
            .map_err(|_| WorkspaceError::Backend("Space id exceeds signed 64-bit range".into()))?;
        let targets = CFArray::from_CFTypes(&[CFNumber::from(target_i64)]);
        // Add first. Private SkyLight calls can silently no-op; removing the
        // original membership first could otherwise strand the window in no
        // visible Space.
        unsafe {
            (self.add_windows_to_spaces)(
                (self.main_connection)(),
                windows.as_concrete_TypeRef(),
                targets.as_concrete_TypeRef(),
            );
        }
        let after_add = self.spaces_for_window(window_id)?;
        if !after_add.contains(&target) {
            return Err(WorkspaceError::Backend(format!(
                "private Space add did not verify (observed {after_add:?}, expected membership in {target}); SIP or entitlements may have blocked it"
            )));
        }

        let old: Vec<_> = current.iter().copied().filter(|id| *id != target).collect();
        if !old.is_empty() {
            let old_numbers: Vec<CFNumber> = old
                .iter()
                .filter_map(|id| i64::try_from(*id).ok())
                .map(CFNumber::from)
                .collect();
            let old_spaces = CFArray::from_CFTypes(&old_numbers);
            unsafe {
                (self.remove_windows_from_spaces)(
                    (self.main_connection)(),
                    windows.as_concrete_TypeRef(),
                    old_spaces.as_concrete_TypeRef(),
                );
            }
        }

        let observed = self.spaces_for_window(window_id)?;
        if observed.len() != 1 || observed[0] != target {
            // Restore original membership before removing the newly-added
            // target. This ordering maintains at least one visible Space even
            // when either private call silently fails.
            if !current.is_empty() {
                let original_numbers: Vec<CFNumber> = current
                    .iter()
                    .filter_map(|id| i64::try_from(*id).ok())
                    .map(CFNumber::from)
                    .collect();
                let originals = CFArray::from_CFTypes(&original_numbers);
                unsafe {
                    (self.add_windows_to_spaces)(
                        (self.main_connection)(),
                        windows.as_concrete_TypeRef(),
                        originals.as_concrete_TypeRef(),
                    );
                }
            }
            let originals_restored = self
                .spaces_for_window(window_id)
                .is_ok_and(|spaces| original_membership_restored(&current, &spaces));
            if originals_restored && !current.contains(&target) {
                unsafe {
                    (self.remove_windows_from_spaces)(
                        (self.main_connection)(),
                        windows.as_concrete_TypeRef(),
                        targets.as_concrete_TypeRef(),
                    );
                }
            }
            let rollback_observed = self.spaces_for_window(window_id).unwrap_or_default();
            return Err(WorkspaceError::Backend(format!(
                "private Space move did not verify (observed {observed:?}, expected [{target}], rollback observed {rollback_observed:?}); SIP or entitlements may have blocked it"
            )));
        }
        Ok(())
    }
}

fn original_membership_restored(original: &[SpaceId], observed: &[SpaceId]) -> bool {
    !original.is_empty() && original.iter().all(|id| observed.contains(id))
}

unsafe fn symbol<T: Copy>(handle: *mut c_void, name: &[u8]) -> Option<T> {
    let pointer = unsafe { libc::dlsym(handle, name.as_ptr().cast::<c_char>()) };
    if pointer.is_null() {
        None
    } else {
        Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&pointer) })
    }
}

#[derive(Default)]
pub struct MacosWorkspaceBackend {
    workspaces: Mutex<HashMap<String, SpaceId>>,
}

impl MacosWorkspaceBackend {
    pub fn new() -> Self {
        Self::default()
    }

    fn enabled() -> bool {
        std::env::var_os(PRIVATE_SPACES_ENV).is_some_and(|value| value == "1")
    }

    fn capabilities() -> WorkspaceCapabilities {
        WorkspaceCapabilities {
            create: false,
            attach: true,
            close: true,
            visual_separation: true,
            separate_display_server: false,
            host_focus_isolation: false,
            launch: true,
            move_existing_window: true,
            capture: true,
            input: true,
            filesystem_isolation: false,
            network_isolation: false,
            process_isolation: false,
            security_isolation: false,
        }
    }
}

#[async_trait]
impl WorkspaceBackend for MacosWorkspaceBackend {
    fn platform(&self) -> &'static str {
        "macos"
    }

    fn descriptors(&self) -> Vec<WorkspaceBackendDescriptor> {
        let enabled = Self::enabled();
        let api = enabled.then(PrivateSpaceApi::load).flatten();
        vec![WorkspaceBackendDescriptor {
            kind: WorkspaceKind::NativeSpace,
            available: enabled && api.is_some(),
            experimental: true,
            capabilities: Self::capabilities(),
            detail: Some(match (enabled, api) {
                (false, _) => format!(
                    "Private SkyLight Space operations are disabled; set {PRIVATE_SPACES_ENV}=1 at trusted daemon startup to opt in."
                ),
                (true, None) => "Required SkyLight symbols are unavailable on this macOS version.".into(),
                (true, Some(_)) => "Private SkyLight operations enabled. Attach with native_id='current' or a caller-known Space id. Moves are always verified. This is not a security sandbox."
                    .into(),
            }),
        }]
    }

    async fn create(
        &self,
        workspace_id: &str,
        request: &CreateWorkspaceRequest,
    ) -> Result<CreatedWorkspace, WorkspaceError> {
        if !Self::enabled() {
            return Err(WorkspaceError::Unavailable(format!(
                "macOS private Space operations require {PRIVATE_SPACES_ENV}=1 at daemon startup"
            )));
        }
        let api = PrivateSpaceApi::load().ok_or_else(|| {
            WorkspaceError::Unavailable("required SkyLight Space symbols are unavailable".into())
        })?;
        if !matches!(
            request.kind,
            WorkspaceKind::Auto | WorkspaceKind::NativeSpace
        ) {
            return Err(WorkspaceError::Unsupported(
                "macOS currently implements native_space workspaces".into(),
            ));
        }
        let native_id = request.native_id.as_deref().ok_or_else(|| {
            WorkspaceError::Unsupported(
                "macOS native_space currently attaches to a caller-known Space id; private Space creation is intentionally not claimed"
                    .into(),
            )
        })?;
        let space_id: SpaceId = if native_id.eq_ignore_ascii_case("current") {
            api.active_space()
        } else {
            native_id.parse().map_err(|_| {
                WorkspaceError::Backend(
                    "native_id must be 'current' or an unsigned Space id".into(),
                )
            })?
        };
        let mut workspaces = self.workspaces.lock().unwrap();
        if workspaces.contains_key(workspace_id) {
            return Err(WorkspaceError::Backend(format!(
                "duplicate workspace id '{workspace_id}'"
            )));
        }
        workspaces.insert(workspace_id.to_owned(), space_id);
        Ok(CreatedWorkspace {
            kind: WorkspaceKind::NativeSpace,
            adopted: true,
            native_id: Some(space_id.to_string()),
            capabilities: Self::capabilities(),
            detail: json!({"attached": true, "space_created": false, "private_api": true}),
        })
    }

    async fn close(
        &self,
        workspace_id: &str,
        _launched_pids: &[u32],
    ) -> Result<(), WorkspaceError> {
        if self
            .workspaces
            .lock()
            .unwrap()
            .remove(workspace_id)
            .is_none()
        {
            return Err(WorkspaceError::NotFound(workspace_id.to_owned()));
        }
        Ok(())
    }

    async fn move_window(
        &self,
        workspace_id: &str,
        target: &WindowTarget,
    ) -> Result<serde_json::Value, WorkspaceError> {
        let space_id = *self
            .workspaces
            .lock()
            .unwrap()
            .get(workspace_id)
            .ok_or_else(|| WorkspaceError::NotFound(workspace_id.to_owned()))?;
        let window_id = u32::try_from(target.window_id)
            .map_err(|_| WorkspaceError::Backend("window_id is outside CGWindowID range".into()))?;
        let api = PrivateSpaceApi::load().ok_or_else(|| {
            WorkspaceError::Unavailable("required SkyLight Space symbols are unavailable".into())
        })?;
        api.move_window(window_id, space_id)?;
        Ok(json!({"space_id": space_id, "verified": true, "private_api": true}))
    }

    async fn get_state(&self, workspace_id: &str) -> Result<serde_json::Value, WorkspaceError> {
        let space_id = *self
            .workspaces
            .lock()
            .unwrap()
            .get(workspace_id)
            .ok_or_else(|| WorkspaceError::NotFound(workspace_id.to_owned()))?;
        let api = PrivateSpaceApi::load().ok_or_else(|| {
            WorkspaceError::Unavailable("required SkyLight Space symbols are unavailable".into())
        })?;
        let active_space = api.active_space();
        let windows: Vec<_> = crate::windows::all_windows()
            .into_iter()
            .filter_map(|mut window| {
                let spaces = api.spaces_for_window(window.window_id).ok()?;
                if !spaces.contains(&space_id) {
                    return None;
                }
                window.on_current_space = Some(space_id == active_space);
                window.space_ids = Some(spaces);
                Some(window)
            })
            .take(256)
            .collect();
        Ok(json!({"windows":windows,"space_id":space_id,"bounded":true}))
    }

    fn configure_command(
        &self,
        workspace_id: &str,
        _command: &mut std::process::Command,
    ) -> Result<(), WorkspaceError> {
        if !self.workspaces.lock().unwrap().contains_key(workspace_id) {
            return Err(WorkspaceError::NotFound(workspace_id.to_owned()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_space_never_claims_security_isolation_or_creation() {
        let capabilities = MacosWorkspaceBackend::capabilities();
        assert!(!capabilities.create);
        assert!(!capabilities.security_isolation);
        assert!(capabilities.attach);
    }

    #[test]
    fn rollback_removes_target_only_after_original_membership_is_visible() {
        assert!(original_membership_restored(&[1, 2], &[3, 2, 1]));
        assert!(!original_membership_restored(&[1, 2], &[2, 3]));
        assert!(!original_membership_restored(&[], &[3]));
    }

    #[test]
    fn macos_registry_exposes_workspace_tools() {
        let registry = crate::register_tools();
        let names: std::collections::HashSet<_> = registry.tool_names().collect();
        assert!(names.contains("list_workspace_backends"));
        assert!(names.contains("create_workspace"));
        assert!(names.contains("get_workspace_state"));
        assert!(names.contains("move_window_to_workspace"));
        assert!(names.contains("close_workspace"));
    }
}
