//! Win32 API wrappers for window/process enumeration.

pub mod apps;
pub mod installed_apps;
pub mod windows;

pub use apps::{list_descendants, list_processes, related_processes, ProcessInfo};
pub use installed_apps::{list_installed_apps, InstalledApp};
pub(crate) use windows::{
    capture_foreground_target, find_window_by_pid_and_handle,
    foreground_matches_target_or_owned_window, list_windows_via_win32, window_owner_pid,
};
pub use windows::{list_windows, resolve_uwp_host_window, WindowInfo};
