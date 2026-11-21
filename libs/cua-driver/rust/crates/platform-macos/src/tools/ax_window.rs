// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Exact-window AX operations; own-process work never leaves the main thread.
use crate::{ax::bindings::*, windows::WindowOwner};
use core_foundation::base::{CFType, TCFType};
use cua_driver_core::native_operation::NativeOperation;
use std::{sync::Arc, time::Duration};

/// Bound on waiting for the main queue to start one of this process's own
/// window writes.
const MAIN_QUEUE_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct Window {
    pub pid: i32,
    pub id: u32,
    pub operation: Arc<NativeOperation>,
}

impl Window {
    /// Resolve the exact AX window and run `work` on it with the thread
    /// affinity `ax::main_thread` requires. Nothing runs once the call's
    /// operation was cancelled before its first native write.
    pub fn with<T: Send + 'static>(
        &self,
        work: impl FnOnce(AXUIElementRef) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let (pid, id) = (self.pid, self.id);
        let operation = self.operation.clone();
        crate::ax::main_thread::run_ax_mutation(pid, MAIN_QUEUE_TIMEOUT, move || {
            if !operation.begin() {
                return Err("native window operation cancelled before start".into());
            }
            unsafe { with_window(pid, id, work) }
        })
        .unwrap_or_else(|error| {
            Err(format!(
                "AppKit main queue did not complete the window operation: {error}"
            ))
        })
    }
}

unsafe fn with_window<T>(
    pid: i32,
    window_id: u32,
    work: impl FnOnce(AXUIElementRef) -> Result<T, String>,
) -> Result<T, String> {
    require_owner(
        crate::windows::resolve_window_owner(pid, window_id),
        pid,
        window_id,
    )?;
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return Err(format!(
            "could not create an accessibility element for pid {pid}"
        ));
    }
    AXUIElementSetMessagingTimeout(app, 2.0);
    let app = CFType::wrap_under_create_rule(app.cast());
    let windows = copy_ax_windows_including(app.as_CFTypeRef() as AXUIElementRef, pid, window_id)
        .into_iter()
        .map(|window| CFType::wrap_under_create_rule(window.cast()))
        .collect::<Vec<_>>();
    windows.iter().map(|window| window.as_CFTypeRef() as AXUIElementRef).find(|window| ax_get_window_id(*window) == Some(window_id))
        .ok_or_else(|| format!("window_id {window_id} belongs to pid {pid} in WindowServer but has no matching AXWindow"))
        .and_then(|target| { AXUIElementSetMessagingTimeout(target, 2.0); work(target) })
}

fn require_owner(owner: WindowOwner, pid: i32, window_id: u32) -> Result<(), String> {
    match owner {
        WindowOwner::SamePid => Ok(()),
        WindowOwner::ForeignPid {
            owner_pid,
            owner_app_name,
        } => Err(format!(
            "window_id {window_id} belongs to pid {owner_pid} ({owner_app_name}), not pid {pid}"
        )),
        WindowOwner::Unknown => Err(format!(
            "window_id {window_id} is closed, stale, or unknown to WindowServer"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_or_unknown_window_fails_the_ownership_gate() {
        let owner = crate::windows::resolve_window_owner_in(&[], 17, 23);
        assert_eq!(
            require_owner(owner, 17, 23).unwrap_err(),
            "window_id 23 is closed, stale, or unknown to WindowServer"
        );
    }

    #[test]
    fn wrong_owner_fails_with_the_actual_owner_identity() {
        assert_eq!(
            require_owner(
                WindowOwner::ForeignPid {
                    owner_pid: 41,
                    owner_app_name: "Other application".into(),
                },
                17,
                23
            )
            .unwrap_err(),
            "window_id 23 belongs to pid 41 (Other application), not pid 17"
        );
        assert_eq!(require_owner(WindowOwner::SamePid, 17, 23), Ok(()));
    }
}
