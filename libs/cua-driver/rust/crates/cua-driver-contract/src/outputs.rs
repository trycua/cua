// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use serde::{Deserialize, Serialize};

/// Successful structured result shared by session state and escalation tools.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionStateOutput {
    pub session: String,
    pub capture_scope: String,
    pub effective_scope: String,
    pub desktop_unlocked: bool,
    pub escalation_reason: Option<String>,
    pub escalation_detail: Option<String>,
}

/// Successful structured result returned by `start_session`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartSessionOutput {
    #[serde(flatten)]
    pub state: SessionStateOutput,
    pub active: bool,
    pub revived: bool,
}

/// Successful structured result returned by `end_session`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EndSessionOutput {
    pub session: String,
    pub active: bool,
}
