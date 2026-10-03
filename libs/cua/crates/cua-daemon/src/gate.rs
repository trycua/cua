//! The approval gate at the daemon: what a caller that is not the user may
//! do (see [`crate::caller`] for the trust model).
//!
//! The classification and the policy are the MCP server's
//! (`cua_spaces::mcp::gate`), so a call means the same through `cua mcp`,
//! `/mcp`, the socket and loopback. The daemon has no screen of its own: a
//! gated action is put to the approver the build registered (Touch ID in
//! Cua Spaces), and fails closed with `ApprovalDenied` when nothing can ask.

use crate::{Error, caller::Caller};
use serde_json::Value;

/// The caller of a gRPC request (set by the listener; fails closed to an
/// agent when absent).
pub(crate) fn caller_of<T>(req: &tonic::Request<T>) -> Caller {
    req.extensions()
        .get::<Caller>()
        .cloned()
        .unwrap_or_else(|| Caller::Agent("unknown".into()))
}

/// The caller of an HTTP request.
pub(crate) fn caller_of_http<B>(req: &http::Request<B>) -> Caller {
    req.extensions()
        .get::<Caller>()
        .cloned()
        .unwrap_or_else(|| Caller::Agent("unknown".into()))
}

/// What the daemon enforces for callers that are not the user.
#[derive(Clone)]
pub(crate) struct Gate {
    /// Whether callers are told apart at all (off where the OS gives no peer
    /// identity, and in fixtures).
    pub enforce: bool,
    /// The agent surface over this daemon's Spaces: classifies calls and
    /// holds the guard (policy home and approver).
    #[cfg(feature = "spaces")]
    pub mcp: cua_spaces::mcp::McpServer,
}

impl Gate {
    /// Lets `caller` do what the tool `tool` with `args` does, or says why
    /// not. The user always passes.
    #[cfg(feature = "spaces")]
    pub async fn check(&self, caller: &Caller, tool: &str, args: Value) -> Result<(), Error> {
        let Caller::Agent(name) = caller else {
            return Ok(());
        };
        if !self.enforce {
            return Ok(());
        }
        self.mcp.check(name, tool, &args).await.map_err(|o| {
            let message = o
                .structured
                .as_ref()
                .and_then(|s| s["error"]["message"].as_str().map(str::to_string))
                .or_else(|| o.first_text().map(str::to_string))
                .unwrap_or_else(|| "not approved".into());
            Error::ApprovalDenied(message)
        })
    }

    /// Without Spaces there is no policy to apply.
    #[cfg(not(feature = "spaces"))]
    pub async fn check(&self, _caller: &Caller, _tool: &str, _args: Value) -> Result<(), Error> {
        Ok(())
    }

    /// Whether `caller` must not be shown the user-tier token or reach the
    /// daemon's own controls.
    pub fn restricts(&self, caller: &Caller) -> bool {
        self.enforce && caller.is_agent()
    }
}
