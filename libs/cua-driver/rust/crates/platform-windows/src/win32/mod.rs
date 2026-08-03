//! Win32 API wrappers for window/process enumeration.

pub mod apps;
pub mod installed_apps;
pub mod windows;

pub use apps::{list_descendants, list_processes, related_processes, ProcessInfo};
pub use installed_apps::{list_installed_apps, InstalledApp};
pub(crate) use windows::{find_window_by_pid_and_handle, list_windows_via_win32};
pub use windows::{list_windows, resolve_uwp_host_window, WindowInfo};
