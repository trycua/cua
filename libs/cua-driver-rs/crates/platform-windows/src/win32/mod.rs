//! Win32 API wrappers for window/process enumeration.

pub mod apps;
pub mod windows;

pub use apps::{list_processes, ProcessInfo};
pub use windows::{list_windows, WindowInfo};
