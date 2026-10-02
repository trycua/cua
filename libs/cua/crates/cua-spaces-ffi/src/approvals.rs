// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Settings → Agent approvals: what an agent must ask the user for.
//!
//! The setting is `<cua home>/approvals.json`, the file the MCP server
//! enforces on every tool call ([`cua_spaces::approvals`]). The pane reads
//! it and changes one row at a time through [`Policy::set`], which writes
//! nothing until the user confirms with Touch ID or the login password, so
//! there is no way to loosen (or tighten) a row without the fingerprint and
//! no master switch. The SwiftUI view only renders [`ApprovalsView`].
//!
//! This lives beside the other native-only commands rather than in
//! `cua-spaces-app-core`, which stays free of the heavy `cua-spaces`
//! dependency so it can build for wasm.

use std::path::PathBuf;
use std::sync::Arc;

use cua_sdk::CuaError;
use cua_spaces::approvals::{self, Approver, Cap, Policy};

/// Shown above the rows.
const INTRO: &str = "Choose what an agent must ask you for. Changes need Touch ID.";
/// The locked section's title.
const LOCKED_TITLE: &str = "Always asks";

/// One capability row.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ApprovalRow {
    /// Stable id (`cloud`, `machines`, ...).
    pub id: String,
    /// The row title.
    pub title: String,
    /// One line under the title.
    pub detail: String,
    /// On: the agent needs the user's approval before it does this.
    pub require: bool,
}

/// A gate that is not a setting: no toggle, always asks.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ApprovalLocked {
    /// The row title.
    pub title: String,
    /// One line under the title.
    pub detail: String,
}

/// The whole pane.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ApprovalsView {
    /// The line at the top.
    pub intro: String,
    /// The settings, one toggle each.
    pub rows: Vec<ApprovalRow>,
    /// The locked section's title.
    pub locked_title: String,
    /// The gates that cannot be turned off (no toggle).
    pub locked: Vec<ApprovalLocked>,
}

fn view_of(policy: &Policy) -> ApprovalsView {
    ApprovalsView {
        intro: INTRO.into(),
        rows: policy
            .rows()
            .into_iter()
            .map(|r| ApprovalRow {
                id: r.id.into(),
                title: r.title.into(),
                detail: r.detail.into(),
                require: r.require,
            })
            .collect(),
        locked_title: LOCKED_TITLE.into(),
        locked: approvals::always()
            .into_iter()
            .map(|a| ApprovalLocked {
                title: a.title.into(),
                detail: a.detail.into(),
            })
            .collect(),
    }
}

/// Touch ID, an Apple Watch or the login password (the daemon's own gate).
struct OsApprover;

impl Approver for OsApprover {
    fn confirm(&self, reason: &str) -> Result<(), String> {
        cua_teleport::biometric::authorize_sensitive_export(reason).map_err(|e| e.to_string())
    }
}

/// Fixtures and tests: answers yes or no without asking anyone.
struct FixedApprover(bool);

impl Approver for FixedApprover {
    fn confirm(&self, _reason: &str) -> Result<(), String> {
        if self.0 {
            Ok(())
        } else {
            Err("Approval was cancelled".into())
        }
    }
}

/// The Agent approvals pane's model.
#[derive(uniffi::Object)]
pub struct ApprovalsPane {
    home: PathBuf,
    approver: Arc<dyn Approver>,
}

impl ApprovalsPane {
    fn change(&self, id: &str, require: bool) -> Result<ApprovalsView, CuaError> {
        let cap = Cap::from_id(id)
            .ok_or_else(|| CuaError::InvalidArgument(format!("unknown approval \"{id}\"")))?;
        Policy::set(&self.home, cap, require, self.approver.as_ref())
            .map(|p| view_of(&p))
            .map_err(CuaError::PermissionDenied)
    }
}

#[uniffi::export]
impl ApprovalsPane {
    /// The pane for the Cua home (`None`: `$CUA_HOME`, else `~/.cua`), which
    /// asks the user with Touch ID or the login password.
    #[uniffi::constructor]
    pub fn new(cua_home: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            home: cua_home
                .map(PathBuf::from)
                .unwrap_or_else(cua_home::cua_home),
            approver: Arc::new(OsApprover),
        })
    }

    /// A pane over `home` (a throwaway directory) whose approval is always
    /// `accept` (fixtures and tests; never asks anyone).
    #[uniffi::constructor]
    pub fn fixture(home: String, accept: bool) -> Arc<Self> {
        Arc::new(Self {
            home: PathBuf::from(home),
            approver: Arc::new(FixedApprover(accept)),
        })
    }

    /// The current settings (the defaults when there is no file).
    pub fn view(&self) -> ApprovalsView {
        view_of(&Policy::load(&self.home))
    }

    /// Sets one row after the user confirms; returns the refreshed pane, or
    /// an error (declined, or the file could not be written) that leaves the
    /// file unchanged.
    pub async fn set(&self, id: String, require: bool) -> Result<ApprovalsView, CuaError> {
        let me = ApprovalsPane {
            home: self.home.clone(),
            approver: self.approver.clone(),
        };
        // The prompt blocks until the user answers, so it runs off the
        // runtime's workers (and on the SDK runtime, which a Swift task is
        // not on).
        cua_sdk::support::run(async move {
            tokio::task::spawn_blocking(move || me.change(&id, require))
                .await
                .map_err(|e| CuaError::Runtime(e.to_string()))?
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(accept: bool) -> (tempfile::TempDir, Arc<ApprovalsPane>) {
        let dir = tempfile::tempdir().unwrap();
        let p = ApprovalsPane::fixture(dir.path().display().to_string(), accept);
        (dir, p)
    }

    #[test]
    fn defaults_without_a_file() {
        let (dir, p) = pane(true);
        let v = p.view();
        assert_eq!(v.rows.len(), Cap::ALL.len());
        assert!(v.rows.iter().all(|r| r.require == (r.id != "display")));
        assert!(!approvals::path_in(dir.path()).exists(), "reading writes nothing");
        assert_eq!(v.intro, INTRO);
    }

    #[test]
    fn locked_rows_have_no_toggle_and_cannot_be_set() {
        let (_d, p) = pane(true);
        let v = p.view();
        assert_eq!(v.locked.len(), approvals::always().len());
        assert!(v.rows.iter().all(|r| !v.locked.iter().any(|l| l.title == r.title)));
        assert!(p.change("keyvault", false).is_err());
    }

    #[test]
    fn declined_leaves_the_file_unchanged() {
        let (dir, p) = pane(false);
        let e = p.change("cloud", false).unwrap_err();
        assert!(matches!(e, CuaError::PermissionDenied(_)), "{e:?}");
        assert!(!approvals::path_in(dir.path()).exists());
        assert!(p.view().rows[0].require);
    }

    #[test]
    fn accepted_changes_the_file_the_server_reads() {
        let (dir, p) = pane(true);
        let v = p.change("cloud", false).unwrap();
        assert!(!v.rows.iter().find(|r| r.id == "cloud").unwrap().require);
        let on_disk = Policy::load(dir.path());
        assert!(!on_disk.requires(Cap::Cloud));
        assert!(on_disk.requires(Cap::Machines));
        let v = p.change("cloud", true).unwrap();
        assert!(v.rows.iter().find(|r| r.id == "cloud").unwrap().require);
    }
}
