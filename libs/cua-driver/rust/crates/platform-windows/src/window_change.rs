//! Bounded top-level window-set evidence for post-action outcome checks.
//!
//! This module deliberately reports only whether the target process's visible
//! top-level HWND set changed. A stable set is not proof that an action was a
//! no-op; a changed set is positive evidence that the application reacted.

use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowSetChange {
    pub added: Vec<u64>,
    pub removed: Vec<u64>,
}

impl WindowSetChange {
    pub(crate) fn changed(&self) -> bool {
        !self.added.is_empty() || !self.removed.is_empty()
    }
}

pub(crate) fn diff_window_ids(
    before: impl IntoIterator<Item = u64>,
    after: impl IntoIterator<Item = u64>,
) -> WindowSetChange {
    let before: BTreeSet<u64> = before.into_iter().collect();
    let after: BTreeSet<u64> = after.into_iter().collect();

    WindowSetChange {
        added: after.difference(&before).copied().collect(),
        removed: before.difference(&after).copied().collect(),
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn snapshot_pid_windows(pid: u32) -> Vec<u64> {
    crate::win32::list_windows_win32_first(pid)
        .into_iter()
        .map(|window| window.hwnd)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_set_is_not_positive_evidence() {
        let change = diff_window_ids([10, 20], [20, 10]);
        assert!(!change.changed());
        assert!(change.added.is_empty());
        assert!(change.removed.is_empty());
    }

    #[test]
    fn closed_modal_is_positive_window_change_evidence() {
        let change = diff_window_ids([10, 20], [10]);
        assert!(change.changed());
        assert_eq!(change.removed, vec![20]);
        assert!(change.added.is_empty());
    }

    #[test]
    fn opened_popup_is_positive_window_change_evidence() {
        let change = diff_window_ids([10], [10, 30]);
        assert!(change.changed());
        assert_eq!(change.added, vec![30]);
        assert!(change.removed.is_empty());
    }

    #[test]
    fn replacement_is_reported_without_claiming_why() {
        let change = diff_window_ids([10, 20], [10, 30]);
        assert!(change.changed());
        assert_eq!(change.removed, vec![20]);
        assert_eq!(change.added, vec![30]);
    }
}
