//! Capability-honest access to Windows virtual desktops.
//!
//! Windows documents only three operations on `IVirtualDesktopManager`:
//! querying a window's desktop ID, checking whether a window is on the current
//! desktop, and moving a caller-owned window to a desktop identified by a GUID.
//! An explicit daemon-start opt-in enables `winvd`'s version-sensitive Windows
//! 11 shell interfaces for desktop creation and cross-process window moves.

use std::collections::HashMap;
use std::sync::Mutex;
use std::{fmt, str::FromStr};

use async_trait::async_trait;
use cua_driver_core::workspace::{
    CreateWorkspaceRequest, CreatedWorkspace, WindowTarget, WorkspaceBackend,
    WorkspaceBackendDescriptor, WorkspaceCapabilities as PublicCapabilities,
    WorkspaceError as PublicError, WorkspaceKind,
};
use serde_json::json;

pub const WINDOWS_INTERNAL_DESKTOPS_ENV: &str = "CUA_DRIVER_ENABLE_WINDOWS_INTERNAL_DESKTOPS";

/// The stable identifier Windows assigns to a virtual desktop.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DesktopId(u128);

impl DesktopId {
    /// Construct an ID from the UUID's numeric value.
    pub const fn from_u128(value: u128) -> Self {
        Self(value)
    }

    /// Return the UUID's numeric value.
    pub const fn as_u128(self) -> u128 {
        self.0
    }
}

impl fmt::Display for DesktopId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = self.0;
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            value >> 96,
            (value >> 80) & 0xffff,
            (value >> 64) & 0xffff,
            (value >> 48) & 0xffff,
            value & 0xffff_ffff_ffff
        )
    }
}

/// Why a desktop GUID could not be parsed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DesktopIdParseError {
    #[error("desktop ID must be a UUID in 8-4-4-4-12 form")]
    InvalidFormat,
    #[error("desktop ID contains a non-hexadecimal character")]
    InvalidHex,
}

impl FromStr for DesktopId {
    type Err = DesktopIdParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = match input.as_bytes() {
            [b'{', .., b'}'] => &input[1..input.len() - 1],
            _ => input,
        };
        let bytes = input.as_bytes();
        if bytes.len() != 36
            || bytes[8] != b'-'
            || bytes[13] != b'-'
            || bytes[18] != b'-'
            || bytes[23] != b'-'
        {
            return Err(DesktopIdParseError::InvalidFormat);
        }

        let mut value = 0u128;
        for (index, byte) in bytes.iter().copied().enumerate() {
            if matches!(index, 8 | 13 | 18 | 23) {
                continue;
            }
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err(DesktopIdParseError::InvalidHex),
            };
            value = (value << 4) | u128::from(digit);
        }
        Ok(Self(value))
    }
}

/// Operations this documented backend can actually perform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceCapabilities {
    pub attach_to_known_desktop: bool,
    pub query_window_desktop: bool,
    pub check_current_desktop: bool,
    pub move_window_to_known_desktop: bool,
    pub enumerate_desktops: bool,
    pub create_desktop: bool,
    pub remove_desktop: bool,
    pub switch_desktop: bool,
}

impl WorkspaceCapabilities {
    pub const DOCUMENTED_WINDOWS_API: Self = Self {
        attach_to_known_desktop: true,
        query_window_desktop: true,
        check_current_desktop: true,
        move_window_to_known_desktop: true,
        enumerate_desktops: false,
        create_desktop: false,
        remove_desktop: false,
        switch_desktop: false,
    };
}

/// A caller-supplied desktop reference.
///
/// Attaching is intentionally local: the documented API has no operation that
/// can validate whether a GUID currently names a desktop. A move to an unknown
/// ID is rejected by Windows and returned as [`WorkspaceError`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KnownDesktop {
    id: DesktopId,
}

impl KnownDesktop {
    pub const fn id(self) -> DesktopId {
        self.id
    }
}

/// Stable categories callers can use without interpreting HRESULT text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceErrorCategory {
    UnsupportedPlatform,
    InvalidWindowHandle,
    ComUnavailable,
    WindowDesktopQueryFailed,
    CurrentDesktopQueryFailed,
    MoveWindowFailed,
}

/// An error from the native virtual-desktop backend.
#[derive(Debug, thiserror::Error)]
#[error("{category:?}: {message}")]
pub struct WorkspaceError {
    pub category: WorkspaceErrorCategory,
    pub hresult: Option<i32>,
    message: String,
}

impl WorkspaceError {
    pub fn message(&self) -> &str {
        &self.message
    }

    fn new(category: WorkspaceErrorCategory, message: impl Into<String>) -> Self {
        Self {
            category,
            hresult: None,
            message: message.into(),
        }
    }

    #[cfg(target_os = "windows")]
    fn windows(
        category: WorkspaceErrorCategory,
        operation: &'static str,
        error: windows::core::Error,
    ) -> Self {
        let hresult = error.code().0;
        Self {
            category,
            hresult: Some(hresult),
            message: format!(
                "{operation} failed (HRESULT 0x{:08x}): {error}",
                hresult as u32
            ),
        }
    }
}

/// Low-level backend for the documented `IVirtualDesktopManager` contract.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeWorkspaceBackend;

impl NativeWorkspaceBackend {
    pub const fn new() -> Self {
        Self
    }

    pub const fn capabilities(&self) -> WorkspaceCapabilities {
        WorkspaceCapabilities::DOCUMENTED_WINDOWS_API
    }

    /// Bind a caller-discovered desktop GUID for use as a move destination.
    /// This does not (and, with the documented API, cannot) validate the ID.
    pub const fn attach_to_known_desktop(&self, id: DesktopId) -> KnownDesktop {
        KnownDesktop { id }
    }

    #[cfg(target_os = "windows")]
    pub fn get_window_desktop_id(&self, hwnd: isize) -> Result<DesktopId, WorkspaceError> {
        self.validate_window(hwnd)?;
        self.with_manager(|manager| {
            let guid = unsafe {
                manager
                    .GetWindowDesktopId(windows::Win32::Foundation::HWND(hwnd as *mut _))
                    .map_err(|error| {
                        WorkspaceError::windows(
                            WorkspaceErrorCategory::WindowDesktopQueryFailed,
                            "GetWindowDesktopId",
                            error,
                        )
                    })?
            };
            Ok(DesktopId::from_u128(guid.to_u128()))
        })
    }

    #[cfg(not(target_os = "windows"))]
    pub fn get_window_desktop_id(&self, _hwnd: isize) -> Result<DesktopId, WorkspaceError> {
        Err(Self::unsupported())
    }

    #[cfg(target_os = "windows")]
    pub fn is_window_on_current_desktop(&self, hwnd: isize) -> Result<bool, WorkspaceError> {
        self.validate_window(hwnd)?;
        self.with_manager(|manager| unsafe {
            manager
                .IsWindowOnCurrentVirtualDesktop(windows::Win32::Foundation::HWND(hwnd as *mut _))
                .map(|value| value.as_bool())
                .map_err(|error| {
                    WorkspaceError::windows(
                        WorkspaceErrorCategory::CurrentDesktopQueryFailed,
                        "IsWindowOnCurrentVirtualDesktop",
                        error,
                    )
                })
        })
    }

    #[cfg(not(target_os = "windows"))]
    pub fn is_window_on_current_desktop(&self, _hwnd: isize) -> Result<bool, WorkspaceError> {
        Err(Self::unsupported())
    }

    #[cfg(target_os = "windows")]
    pub fn move_window_to_desktop(
        &self,
        hwnd: isize,
        desktop: KnownDesktop,
    ) -> Result<(), WorkspaceError> {
        self.validate_window(hwnd)?;
        self.with_manager(|manager| {
            let guid = windows::core::GUID::from_u128(desktop.id.as_u128());
            unsafe {
                manager
                    .MoveWindowToDesktop(windows::Win32::Foundation::HWND(hwnd as *mut _), &guid)
                    .map_err(|error| {
                        WorkspaceError::windows(
                            WorkspaceErrorCategory::MoveWindowFailed,
                            "MoveWindowToDesktop",
                            error,
                        )
                    })
            }
        })
    }

    #[cfg(not(target_os = "windows"))]
    pub fn move_window_to_desktop(
        &self,
        _hwnd: isize,
        _desktop: KnownDesktop,
    ) -> Result<(), WorkspaceError> {
        Err(Self::unsupported())
    }

    #[cfg(target_os = "windows")]
    fn validate_window(&self, hwnd: isize) -> Result<(), WorkspaceError> {
        if hwnd == 0 {
            Err(WorkspaceError::new(
                WorkspaceErrorCategory::InvalidWindowHandle,
                "window handle must not be null",
            ))
        } else {
            Ok(())
        }
    }

    #[cfg(target_os = "windows")]
    fn with_manager<T>(
        &self,
        operation: impl FnOnce(
            &windows::Win32::UI::Shell::IVirtualDesktopManager,
        ) -> Result<T, WorkspaceError>,
    ) -> Result<T, WorkspaceError> {
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        };
        use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};

        struct ComInitialization(bool);

        impl Drop for ComInitialization {
            fn drop(&mut self) {
                if self.0 {
                    unsafe { CoUninitialize() };
                }
            }
        }

        // A successful call, including S_FALSE, must be balanced. If the thread
        // already uses a different apartment, COM is still initialized and
        // CoCreateInstance below is the authoritative availability check.
        let _com =
            ComInitialization(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() });
        let manager = unsafe {
            CoCreateInstance::<_, IVirtualDesktopManager>(
                &VirtualDesktopManager,
                None,
                CLSCTX_INPROC_SERVER,
            )
            .map_err(|error| {
                WorkspaceError::windows(
                    WorkspaceErrorCategory::ComUnavailable,
                    "CoCreateInstance(IVirtualDesktopManager)",
                    error,
                )
            })?
        };
        operation(&manager)
    }

    #[cfg(not(target_os = "windows"))]
    fn unsupported() -> WorkspaceError {
        WorkspaceError::new(
            WorkspaceErrorCategory::UnsupportedPlatform,
            "Windows virtual desktops are unavailable on this platform",
        )
    }
}

#[derive(Clone, Copy, Debug)]
struct WorkspaceDesktop {
    desktop: KnownDesktop,
    internal: bool,
    owned: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DesktopRemoval {
    AlreadyAbsent,
    RemoveWithFallback(DesktopId),
    OnlyDesktop,
}

fn desktop_removal(target: DesktopId, desktops: &[DesktopId]) -> DesktopRemoval {
    if !desktops.contains(&target) {
        return DesktopRemoval::AlreadyAbsent;
    }
    desktops
        .iter()
        .copied()
        .find(|candidate| *candidate != target)
        .map(DesktopRemoval::RemoveWithFallback)
        .unwrap_or(DesktopRemoval::OnlyDesktop)
}

/// cua-driver adapter for Windows virtual desktop GUIDs.
///
/// The default documented mode can only attach. The explicitly-enabled
/// internal mode can create/remove driver-owned desktops and move arbitrary
/// application views on supported Windows 11 builds.
#[derive(Default)]
pub struct WindowsWorkspaceBackend {
    native: NativeWorkspaceBackend,
    workspaces: Mutex<HashMap<String, WorkspaceDesktop>>,
}

impl WindowsWorkspaceBackend {
    pub fn new() -> Self {
        Self::default()
    }

    fn internal_enabled() -> bool {
        cfg!(target_os = "windows")
            && std::env::var_os(WINDOWS_INTERNAL_DESKTOPS_ENV).is_some_and(|value| value == "1")
    }

    fn public_capabilities(internal: bool) -> PublicCapabilities {
        PublicCapabilities {
            create: internal,
            attach: true,
            close: true,
            visual_separation: true,
            separate_display_server: false,
            host_focus_isolation: false,
            launch: internal,
            move_existing_window: internal,
            capture: true,
            input: true,
            filesystem_isolation: false,
            network_isolation: false,
            process_isolation: false,
            security_isolation: false,
        }
    }

    #[cfg(target_os = "windows")]
    fn open_internal(native_id: Option<&str>) -> Result<WorkspaceDesktop, PublicError> {
        let validate_supplied_guid =
            native_id.is_some_and(|value| !value.eq_ignore_ascii_case("current"));
        let (desktop, owned) = match native_id {
            None => (winvd::create_desktop(), true),
            Some(value) if value.eq_ignore_ascii_case("current") => {
                (winvd::get_current_desktop(), false)
            }
            Some(value) => {
                let id: DesktopId = value.parse().map_err(|error| {
                    PublicError::Backend(format!("invalid desktop GUID: {error}"))
                })?;
                let guid = windows::core::GUID::from_u128(id.as_u128());
                (Ok(winvd::get_desktop(guid)), false)
            }
        };
        let desktop = desktop.map_err(|error| {
            PublicError::Backend(format!(
                "Windows internal desktop operation failed: {error:?}"
            ))
        })?;
        // winvd::get_desktop(GUID) is deliberately a cheap handle wrapper and
        // does not check that the GUID is present in Explorer's desktop list.
        // Resolve its index before recording an adopted workspace so a typo or
        // stale GUID fails at creation rather than at the first window move.
        if validate_supplied_guid {
            desktop.get_index().map_err(|error| {
                PublicError::Backend(format!(
                    "Windows desktop GUID does not identify a live desktop: {error:?}"
                ))
            })?;
        }
        let guid = desktop.get_id().map_err(|error| {
            PublicError::Backend(format!("Windows desktop GUID query failed: {error:?}"))
        })?;
        Ok(WorkspaceDesktop {
            desktop: KnownDesktop {
                id: DesktopId::from_u128(guid.to_u128()),
            },
            internal: true,
            owned,
        })
    }

    #[cfg(target_os = "windows")]
    fn remove_internal_owned(desktop: WorkspaceDesktop) -> Result<(), PublicError> {
        let target = windows::core::GUID::from_u128(desktop.desktop.id().as_u128());
        let desktops = winvd::get_desktops().map_err(|error| {
            PublicError::Backend(format!("enumerate fallback desktops failed: {error:?}"))
        })?;
        let mut desktop_ids = Vec::with_capacity(desktops.len());
        for candidate in desktops {
            let id = candidate.get_id().map_err(|error| {
                PublicError::Backend(format!("query fallback desktop ID failed: {error:?}"))
            })?;
            desktop_ids.push(DesktopId::from_u128(id.to_u128()));
        }
        let fallback = match desktop_removal(desktop.desktop.id(), &desktop_ids) {
            // The user or Explorer may have removed/rebuilt the desktop before
            // close_workspace ran. Its lifecycle goal is already satisfied.
            DesktopRemoval::AlreadyAbsent => return Ok(()),
            DesktopRemoval::RemoveWithFallback(id) => windows::core::GUID::from_u128(id.as_u128()),
            DesktopRemoval::OnlyDesktop => {
                return Err(PublicError::Backend(
                    "cannot remove the only Windows virtual desktop".into(),
                ))
            }
        };
        match winvd::remove_desktop(target, fallback) {
            Ok(()) | Err(winvd::Error::DesktopNotFound) => Ok(()),
            Err(error) => Err(PublicError::Backend(format!(
                "remove driver-owned desktop failed: {error:?}"
            ))),
        }
    }
}

#[async_trait]
impl WorkspaceBackend for WindowsWorkspaceBackend {
    fn platform(&self) -> &'static str {
        "windows"
    }

    fn descriptors(&self) -> Vec<WorkspaceBackendDescriptor> {
        let internal = Self::internal_enabled();
        vec![WorkspaceBackendDescriptor {
            kind: WorkspaceKind::NativeSpace,
            available: cfg!(target_os = "windows"),
            experimental: true,
            capabilities: Self::public_capabilities(internal),
            detail: Some(if internal {
                "Experimental Windows 11 24H2+ shell integration is enabled: driver-owned desktops can be created/closed and arbitrary app windows can be moved with readback verification. This is not a security sandbox."
                    .into()
            } else {
                format!(
                    "Documented attach-only mode. Set {WINDOWS_INTERNAL_DESKTOPS_ENV}=1 at trusted daemon startup to opt into Windows 11 24H2+ desktop creation and cross-process moves."
                )
            }),
        }]
    }

    async fn create(
        &self,
        workspace_id: &str,
        request: &CreateWorkspaceRequest,
    ) -> Result<CreatedWorkspace, PublicError> {
        if !matches!(
            request.kind,
            WorkspaceKind::Auto | WorkspaceKind::NativeSpace
        ) {
            return Err(PublicError::Unsupported(
                "Windows currently implements native_space workspaces".into(),
            ));
        }
        let internal = Self::internal_enabled();
        let desktop = if internal {
            #[cfg(target_os = "windows")]
            {
                let native_id = request.native_id.clone();
                tokio::task::spawn_blocking(move || Self::open_internal(native_id.as_deref()))
                    .await
                    .map_err(|error| {
                        PublicError::Backend(format!("Windows desktop task failed: {error}"))
                    })??
            }
            #[cfg(not(target_os = "windows"))]
            unreachable!("internal mode is available only on Windows")
        } else {
            let native_id = request.native_id.as_deref().ok_or_else(|| {
                PublicError::Unsupported(
                    "Windows documented native_space mode requires a caller-known desktop GUID"
                        .into(),
                )
            })?;
            if native_id.eq_ignore_ascii_case("current") {
                return Err(PublicError::Unsupported(
                    "native_id='current' requires the explicitly-enabled Windows internal desktop backend"
                        .into(),
                ));
            }
            let desktop_id: DesktopId = native_id
                .parse()
                .map_err(|error| PublicError::Backend(format!("invalid desktop GUID: {error}")))?;
            WorkspaceDesktop {
                desktop: self.native.attach_to_known_desktop(desktop_id),
                internal: false,
                owned: false,
            }
        };
        let desktop_id = desktop.desktop.id();
        let mut workspaces = self.workspaces.lock().unwrap();
        if workspaces.contains_key(workspace_id) {
            return Err(PublicError::Backend(format!(
                "duplicate workspace id '{workspace_id}'"
            )));
        }
        workspaces.insert(workspace_id.to_owned(), desktop);
        Ok(CreatedWorkspace {
            kind: WorkspaceKind::NativeSpace,
            adopted: !desktop.owned,
            native_id: Some(desktop_id.to_string()),
            capabilities: Self::public_capabilities(internal),
            detail: json!({
                "attached": !desktop.owned,
                "desktop_created": desktop.owned,
                "desktop_enumeration_supported": internal,
                "internal_api": internal,
            }),
        })
    }

    async fn close(&self, workspace_id: &str, _launched_pids: &[u32]) -> Result<(), PublicError> {
        let Some(desktop) = self.workspaces.lock().unwrap().remove(workspace_id) else {
            return Err(PublicError::NotFound(workspace_id.to_owned()));
        };
        if desktop.internal && desktop.owned {
            #[cfg(target_os = "windows")]
            if let Err(error) =
                tokio::task::spawn_blocking(move || Self::remove_internal_owned(desktop))
                    .await
                    .map_err(|error| {
                        PublicError::Backend(format!("Windows desktop close task failed: {error}"))
                    })?
            {
                self.workspaces
                    .lock()
                    .unwrap()
                    .insert(workspace_id.to_owned(), desktop);
                return Err(error);
            }
        }
        Ok(())
    }

    async fn move_window(
        &self,
        workspace_id: &str,
        target: &WindowTarget,
    ) -> Result<serde_json::Value, PublicError> {
        let desktop = *self
            .workspaces
            .lock()
            .unwrap()
            .get(workspace_id)
            .ok_or_else(|| PublicError::NotFound(workspace_id.to_owned()))?;
        let hwnd = isize::try_from(target.window_id)
            .map_err(|_| PublicError::Backend("window_id is outside HWND range".into()))?;
        let native = self.native;
        let known = desktop.desktop;
        let observed = tokio::task::spawn_blocking(move || {
            #[cfg(target_os = "windows")]
            if desktop.internal {
                let guid = windows::core::GUID::from_u128(known.id().as_u128());
                let hwnd = windows::Win32::Foundation::HWND(hwnd as *mut _);
                winvd::move_window_to_desktop(guid, &hwnd).map_err(|error| {
                    WorkspaceError::new(
                        WorkspaceErrorCategory::MoveWindowFailed,
                        format!("internal MoveViewToDesktop failed: {error:?}"),
                    )
                })?;
            } else {
                native.move_window_to_desktop(hwnd, known)?;
            }
            #[cfg(not(target_os = "windows"))]
            native.move_window_to_desktop(hwnd, known)?;
            native.get_window_desktop_id(hwnd)
        })
        .await
        .map_err(|error| PublicError::Backend(format!("virtual desktop task failed: {error}")))?
        .map_err(|error| PublicError::Backend(format!("move/readback failed: {error}")))?;
        if observed != known.id() {
            return Err(PublicError::Backend(format!(
                "MoveWindowToDesktop returned success but readback was {observed}, expected {}",
                known.id()
            )));
        }
        Ok(json!({"desktop_id": observed.to_string(), "verified": true}))
    }

    async fn get_state(&self, workspace_id: &str) -> Result<serde_json::Value, PublicError> {
        #[cfg(not(target_os = "windows"))]
        {
            let _ = workspace_id;
            return Err(PublicError::Unavailable(
                "Windows workspace state is unavailable off Windows".into(),
            ));
        }
        #[cfg(target_os = "windows")]
        {
            let desktop = *self
                .workspaces
                .lock()
                .unwrap()
                .get(workspace_id)
                .ok_or_else(|| PublicError::NotFound(workspace_id.to_owned()))?;
            let native = self.native;
            let target = desktop.desktop.id();
            tokio::task::spawn_blocking(move || {
                let mut windows = Vec::new();
                for window in crate::win32::list_windows(None).into_iter().take(512) {
                    let Ok(hwnd) = isize::try_from(window.hwnd) else {
                        continue;
                    };
                    let Ok(observed) = native.get_window_desktop_id(hwnd) else {
                        continue;
                    };
                    if observed == target {
                        windows.push(json!({
                            "window_id":window.hwnd,
                            "pid":window.pid,
                            "title":window.title,
                            "bounds":{"x":window.x,"y":window.y,"width":window.width,"height":window.height},
                            "is_on_screen":window.is_on_screen,
                            "minimized":window.minimized,
                        }));
                    }
                }
                Ok(json!({"windows":windows,"desktop_id":target.to_string(),"bounded":true}))
            })
            .await
            .map_err(|error| PublicError::Backend(format!("workspace state task failed: {error}")))?
        }
    }

    fn configure_command(
        &self,
        workspace_id: &str,
        _command: &mut std::process::Command,
    ) -> Result<(), PublicError> {
        if !self.workspaces.lock().unwrap().contains_key(workspace_id) {
            return Err(PublicError::NotFound(workspace_id.to_owned()));
        }
        // Native virtual-desktop membership is a window property, so the
        // caller moves the first resolved top-level window after launch.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "a5cd92ff-29be-454c-8d04-d82879fb3f1b";

    #[test]
    fn desktop_id_round_trips_canonical_and_braced_forms() {
        let canonical: DesktopId = EXAMPLE.parse().unwrap();
        let braced: DesktopId = format!("{{{EXAMPLE}}}").parse().unwrap();

        assert_eq!(canonical, braced);
        assert_eq!(canonical.to_string(), EXAMPLE);
        assert_eq!(canonical.as_u128(), 0xa5cd92ff_29be_454c_8d04_d82879fb3f1b);
    }

    #[test]
    fn desktop_id_accepts_uppercase_and_formats_lowercase() {
        let id: DesktopId = "A5CD92FF-29BE-454C-8D04-D82879FB3F1B".parse().unwrap();
        assert_eq!(id.to_string(), EXAMPLE);
    }

    #[test]
    fn desktop_id_rejects_bad_shape_and_hex() {
        assert_eq!(
            "a5cd92ff29be454c8d04d82879fb3f1b".parse::<DesktopId>(),
            Err(DesktopIdParseError::InvalidFormat)
        );
        assert_eq!(
            "a5cd92ff-29be-454c-8d04-d82879fb3f1z".parse::<DesktopId>(),
            Err(DesktopIdParseError::InvalidHex)
        );
    }

    #[test]
    fn desktop_removal_is_idempotent_and_requires_a_distinct_fallback() {
        let target = DesktopId::from_u128(1);
        let fallback = DesktopId::from_u128(2);

        assert_eq!(
            desktop_removal(target, &[fallback]),
            DesktopRemoval::AlreadyAbsent
        );
        assert_eq!(
            desktop_removal(target, &[target]),
            DesktopRemoval::OnlyDesktop
        );
        assert_eq!(
            desktop_removal(target, &[target, fallback]),
            DesktopRemoval::RemoveWithFallback(fallback)
        );
    }

    #[test]
    fn capabilities_do_not_claim_undocumented_operations() {
        let capabilities = NativeWorkspaceBackend::new().capabilities();

        assert!(capabilities.attach_to_known_desktop);
        assert!(capabilities.query_window_desktop);
        assert!(capabilities.check_current_desktop);
        assert!(capabilities.move_window_to_known_desktop);
        assert!(!capabilities.enumerate_desktops);
        assert!(!capabilities.create_desktop);
        assert!(!capabilities.remove_desktop);
        assert!(!capabilities.switch_desktop);

        let public = WindowsWorkspaceBackend::public_capabilities(false);
        assert!(!public.launch);
        assert!(!public.move_existing_window);
        assert!(public.attach);

        let internal = WindowsWorkspaceBackend::public_capabilities(true);
        assert!(internal.create);
        assert!(internal.launch);
        assert!(internal.move_existing_window);
    }

    #[tokio::test]
    async fn current_desktop_alias_is_refused_without_a_reliable_owned_window_probe() {
        let backend = WindowsWorkspaceBackend::new();
        let error = backend
            .create(
                "ws_test",
                &CreateWorkspaceRequest {
                    kind: WorkspaceKind::NativeSpace,
                    name: None,
                    native_id: Some("current".into()),
                    options: serde_json::json!({}),
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(error, PublicError::Unsupported(_)));
    }

    #[test]
    fn attaching_preserves_the_caller_supplied_id() {
        let backend = NativeWorkspaceBackend::new();
        let id: DesktopId = EXAMPLE.parse().unwrap();
        assert_eq!(backend.attach_to_known_desktop(id).id(), id);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn native_calls_are_explicitly_unsupported_off_windows() {
        let error = NativeWorkspaceBackend::new()
            .get_window_desktop_id(1)
            .unwrap_err();
        assert_eq!(error.category, WorkspaceErrorCategory::UnsupportedPlatform);
        assert_eq!(error.hresult, None);
    }
}
