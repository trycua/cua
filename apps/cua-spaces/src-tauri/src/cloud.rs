// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The user's own clouds (AWS, Google Cloud, Modal) for the "Your cloud"
//! tile and the "Connect a cloud" sheet. The sheet is the app core's
//! (`cloudConnect.*`, shared with the SwiftUI app); these commands run its
//! requests through the `cua daemon`'s Spaces tools. Credentials never pass
//! through here: each cloud uses its own CLI sign-in.

use cua_proto::daemon::v1 as dpb;
use serde_json::Value;

use crate::core::{AppCore, CmdResult};

/// The cloud tools the app may call: read, test (creates nothing) and
/// connect. Deleting and sweeping stay in `cua cloud`.
pub const TOOLS: &[&str] = &["cloud_status", "cloud_test", "cloud_connect"];

impl AppCore {
    /// Runs one of [`TOOLS`] in the daemon (started when needed).
    pub async fn cloud_tool(&self, tool: &str, args: Value) -> CmdResult<Value> {
        if !TOOLS.contains(&tool) {
            return Err(format!("{tool} is not available to the app"));
        }
        let daemon = self.daemon(true).await?;
        let r = daemon
            .spaces()
            .call_space_tool(dpb::CallSpaceToolRequest {
                name: tool.into(),
                arguments_json: args.to_string(),
            })
            .await
            .map_err(|e| e.message().to_string())?
            .into_inner();
        crate::persistent::tool_result(&r.content_json, r.is_error)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_reading_testing_and_connecting_are_offered() {
        assert_eq!(
            super::TOOLS,
            ["cloud_status", "cloud_test", "cloud_connect"]
        );
        assert!(!super::TOOLS.contains(&"cloud_sweep"));
    }
}
