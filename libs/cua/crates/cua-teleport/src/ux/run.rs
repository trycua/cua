// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Progress and results of running an approved plan (the run itself needs a
//! Space: `cua_spaces::Space::run_app_teleport`).

use serde::{Deserialize, Serialize};

/// Where a step is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    /// The step began.
    Started,
    /// Progress within the step.
    Progress,
    /// The step finished.
    Finished,
    /// The step failed (the run stops).
    Failed,
    /// Every step finished.
    Done,
}

/// One progress event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEvent {
    /// Step index (0-based; `steps` for `done`).
    pub step: u32,
    /// Step count.
    pub steps: u32,
    /// Step name (`install`, `files`, `state`, `launch`, `done`).
    pub kind: String,
    /// Phase.
    pub phase: RunPhase,
    /// What is happening (an installer phase, a file, an error).
    pub detail: String,
    /// Bytes done in this step, when it moves bytes.
    pub done_bytes: u64,
    /// Bytes this step moves.
    pub total_bytes: u64,
}

/// What a run did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunReport {
    /// Catalog id.
    pub app_id: String,
    /// Installed (or already present) installable ids.
    pub installed: Vec<String>,
    /// Guest paths of sent files and folders.
    pub sent: Vec<String>,
    /// State items the Space imported.
    pub imported: Vec<String>,
    /// State items it skipped, with reasons.
    pub skipped: Vec<String>,
    /// The app was started (by the launch step or the state import).
    pub launched: bool,
}
