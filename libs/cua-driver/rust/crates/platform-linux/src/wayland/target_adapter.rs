//! Explicit compositor dispatch for trusted target-addressable Wayland helpers.
//!
//! This intentionally does not pretend that every Wayland compositor shares a
//! generic control protocol.  KDE uses the KWin UUID adapter, GNOME uses the
//! Shell stable-sequence helper, and wlroots/Sway remains on its native path.

use crate::x11::WindowInfo;

pub fn list_windows(filter_pid: Option<u32>) -> Option<Vec<WindowInfo>> {
    if super::kwin_adapter::is_kde_wayland_session() {
        match super::kwin_adapter::list_windows(filter_pid) {
            Ok(windows) => Some(windows),
            Err(error) => {
                tracing::debug!("trusted KWin enumeration unavailable: {error}");
                None
            }
        }
    } else {
        super::shell_helper::list_windows(filter_pid)
    }
}

pub fn trusted_window_for_id(pid: u32, window_id: u64) -> anyhow::Result<Option<WindowInfo>> {
    if super::kwin_adapter::is_kde_wayland_session() {
        super::kwin_adapter::trusted_window_for_id(pid, window_id)
    } else {
        Ok(super::shell_helper::trusted_window_for_id(pid, window_id))
    }
}

pub fn trusted_window_ids_for_pid(pid: u32) -> anyhow::Result<Option<Vec<u64>>> {
    if super::kwin_adapter::is_kde_wayland_session() {
        super::kwin_adapter::trusted_window_ids_for_pid(pid).map(Some)
    } else {
        Ok(super::shell_helper::trusted_window_ids_for_pid(pid))
    }
}

pub fn window_origin_for_pid(pid: u32) -> Option<(i32, i32)> {
    if super::kwin_adapter::is_kde_wayland_session() {
        super::kwin_adapter::window_origin_for_pid(pid)
    } else {
        super::shell_helper::window_origin_for_pid(pid)
    }
}

pub fn with_focused_window<T>(
    pid: u32,
    window_id: u64,
    body: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    if super::kwin_adapter::is_kde_wayland_session() {
        super::kwin_adapter::with_focused_window(pid, window_id, body)
    } else {
        super::shell_helper::with_focused_window(pid, window_id, body)
    }
}

pub fn activate_window(pid: u32, window_id: u64) -> anyhow::Result<()> {
    if super::kwin_adapter::is_kde_wayland_session() {
        return super::kwin_adapter::activate_window(pid, window_id);
    }
    super::shell_helper::trusted_window_for_id(pid, window_id).ok_or_else(|| {
        anyhow::anyhow!("foreground_unavailable: no exact GNOME Shell window owns the target")
    })?;
    if super::shell_helper::activate_window(window_id) {
        Ok(())
    } else {
        anyhow::bail!("foreground_unavailable: GNOME Shell did not verify exact activation")
    }
}
