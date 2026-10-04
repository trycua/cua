// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The overlay over a Space while a teleport uploads: indeterminate until
//! byte totals arrive, then determinate; an error offers Retry; `done`
//! clears it. Also the drop well's status line (files sent into a Space).

use serde::{Deserialize, Serialize};

/// Active or failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferPhase {
    /// Uploading.
    Active,
    /// Failed.
    Error,
}

/// The overlay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferOverlayState {
    /// Phase.
    pub phase: TransferPhase,
    /// App being teleported.
    pub app_name: String,
    /// Failure reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Bytes sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_bytes: Option<f64>,
    /// Total bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<f64>,
}

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferStatus {
    /// Began (or retried).
    Start,
    /// Bytes moved.
    Progress,
    /// Finished.
    Done,
    /// Failed.
    Error,
}

/// A lifecycle signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferSignal {
    /// Status.
    pub status: TransferStatus,
    /// App name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    /// Message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Bytes sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_bytes: Option<f64>,
    /// Total bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<f64>,
}

/// Advances the overlay; `None` hides it.
pub fn reduce(
    state: Option<&TransferOverlayState>,
    signal: &TransferSignal,
) -> Option<TransferOverlayState> {
    let app = || {
        signal
            .app_name
            .clone()
            .or_else(|| state.map(|s| s.app_name.clone()))
            .unwrap_or_default()
    };
    match signal.status {
        TransferStatus::Start => Some(TransferOverlayState {
            phase: TransferPhase::Active,
            app_name: app(),
            message: None,
            sent_bytes: None,
            total_bytes: None,
        }),
        TransferStatus::Progress => Some(TransferOverlayState {
            phase: TransferPhase::Active,
            app_name: app(),
            message: None,
            sent_bytes: signal.sent_bytes,
            total_bytes: signal.total_bytes,
        }),
        TransferStatus::Done => None,
        TransferStatus::Error => Some(TransferOverlayState {
            phase: TransferPhase::Error,
            app_name: app(),
            message: signal.message.clone(),
            sent_bytes: None,
            total_bytes: None,
        }),
    }
}

/// "Teleporting Slack..." or "Could not teleport Slack".
pub fn title(state: &TransferOverlayState) -> String {
    let app = if state.app_name.is_empty() {
        "app"
    } else {
        &state.app_name
    };
    match state.phase {
        TransferPhase::Error => format!("Could not teleport {app}"),
        TransferPhase::Active => format!("Teleporting {app}\u{2026}"),
    }
}

/// "12.3 MB".
pub fn format_megabytes(bytes: f64) -> String {
    format!("{} MB", crate::to_fixed(bytes / (1024.0 * 1024.0), 1))
}

/// The determinate fraction in `[0, 1]`, or none while totals are unknown.
pub fn progress(state: &TransferOverlayState) -> Option<f64> {
    match (state.sent_bytes, state.total_bytes) {
        (Some(s), Some(t)) if t > 0.0 => Some((s / t).clamp(0.0, 1.0)),
        _ => None,
    }
}

/// "{x} MB / {y} MB", or none while totals are unknown.
pub fn size_label(state: &TransferOverlayState) -> Option<String> {
    match (state.sent_bytes, state.total_bytes) {
        (Some(s), Some(t)) => Some(format!("{} / {}", format_megabytes(s), format_megabytes(t))),
        _ => None,
    }
}

/// A file a drop sent, verified in the Space.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SentFileInfo {
    /// File name.
    pub name: String,
    /// Where it landed in the Space.
    pub dest: String,
    /// Bytes.
    pub bytes: u64,
}

/// The drop well's line while files go: "Sending notes.txt…".
pub fn drop_sending_text(paths: &[String]) -> String {
    match paths {
        [one] => {
            let name = one.rsplit('/').find(|p| !p.is_empty()).unwrap_or(one);
            format!("Sending {name}\u{2026}")
        }
        many => format!("Sending {} files\u{2026}", many.len()),
    }
}

/// The drop well's line once they landed (verified by hash in the Space).
pub fn drop_sent_text(files: &[SentFileInfo]) -> String {
    use super::flow::format_bytes;
    match files {
        [] => String::new(),
        [f] => format!(
            "{} ({}) verified in {}",
            f.name,
            format_bytes(f.bytes),
            f.dest
        ),
        many => format!(
            "{} files ({}) verified in Downloads",
            many.len(),
            format_bytes(many.iter().map(|f| f.bytes).sum())
        ),
    }
}

#[cfg(test)]
mod drop_tests {
    use super::*;

    #[test]
    fn drop_lines_name_what_landed() {
        assert_eq!(
            drop_sending_text(&["/a/b.txt".into()]),
            "Sending b.txt\u{2026}"
        );
        assert_eq!(
            drop_sent_text(&[SentFileInfo {
                name: "b.txt".into(),
                dest: "/home/u/Downloads/b.txt".into(),
                bytes: 10,
            }]),
            "b.txt (10 B) verified in /home/u/Downloads/b.txt"
        );
        assert_eq!(drop_sent_text(&[]), "");
    }
}
