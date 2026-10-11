//! Extension points for the Spaces capabilities that ship with Cua Spaces
//! rather than in this crate: session and app teleport, the Cua Volume and
//! persistent agents (their implementations are source-available,
//! FSL-1.1-MIT, in `cua-spaces-ext`).
//!
//! This crate (MIT) defines the contract: the tool names and schemas in
//! [`cua_spaces_contract`], and [`SpacesExtension`], which serves some of
//! them. A host registers extensions on the builder
//! ([`crate::SpacesBuilder::extension`]); the Spaces MCP server routes a tool
//! it does not implement itself to the extension that serves it. With none
//! registered those tools fail with `host_capability_missing` and say where
//! the capability ships ([`crate::Error::needs_cua_spaces`]), so an MIT build
//! behaves like the Cua Spaces build minus those tools, never differently.
//!
//! Besides contract tools, an extension may serve internal operations (names
//! outside the contract, such as `teleport.send`): calls a host makes in
//! process ([`Spaces::call_extension`]) that no MCP client can reach.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::mcp::ToolOutcome;
use crate::teleport_broker::SessionBroker;
use crate::{Space, Spaces};

/// A boxed, `Send` future (the extension trait is object safe).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One tool call routed to an extension.
pub struct ToolCall<'a> {
    /// The runtime the call runs in.
    pub spaces: &'a Spaces,
    /// The contract tool (or internal operation) name.
    pub tool: &'a str,
    /// The tool's arguments, as the client sent them (`{}` when absent).
    pub args: Value,
    /// The Keyvault broker the host wired in, if any (session teleport
    /// delivers only through it).
    pub broker: Option<&'a Arc<dyn SessionBroker>>,
    /// The Space a Space-scoped call was pinned to, if any.
    pub pinned: Option<&'a Space>,
}

/// A capability served outside this crate. Implementations must be cheap to
/// share (`Arc`) and keep their own state (a drive handle, leases, host
/// providers).
pub trait SpacesExtension: Send + Sync + 'static {
    /// A short name for logs (`teleport`, `drive`, `persistent`).
    fn name(&self) -> &str;

    /// Whether this extension serves `tool` (a contract tool or one of its
    /// internal operations).
    fn serves(&self, tool: &str) -> bool;

    /// Runs `call`. Errors become tool errors with the error's kind.
    fn call_tool<'a>(&'a self, call: ToolCall<'a>) -> BoxFuture<'a, Result<ToolOutcome>>;

    /// A Space was just connected (created, or connected again): called on
    /// every fresh connection, from the caller's task, so it must return at
    /// once (spawn any work). For example, Cua Volume mounts the Space's
    /// view in its guest.
    fn space_connected(&self, spaces: &Spaces, space: &str) {
        let _ = (spaces, space);
    }

    /// A Space is going away (removed, deleted, released): what this
    /// extension holds for it ends here, while the Space still answers.
    fn space_dropped<'a>(&'a self, spaces: &'a Spaces, space: &'a str) -> BoxFuture<'a, ()> {
        let _ = (spaces, space);
        Box::pin(async {})
    }

    /// For [`Spaces::extension`] (downcasting to the concrete type).
    fn as_any(&self) -> &dyn Any;
}

impl Spaces {
    /// The registered extensions, in registration order.
    pub fn extensions(&self) -> &[Arc<dyn SpacesExtension>] {
        &self.inner.extensions
    }

    /// The first registered extension of type `T`.
    pub fn extension<T: SpacesExtension>(&self) -> Option<&T> {
        self.inner
            .extensions
            .iter()
            .find_map(|e| e.as_any().downcast_ref::<T>())
    }

    /// Tells every extension that `id` just connected.
    pub(crate) fn notify_connected(&self, id: &crate::SpaceId) {
        let key = id.to_string();
        for e in &self.inner.extensions {
            e.space_connected(self, &key);
        }
    }

    /// The extension that serves `tool`, if any.
    pub fn extension_for(&self, tool: &str) -> Option<&Arc<dyn SpacesExtension>> {
        self.inner.extensions.iter().find(|e| e.serves(tool))
    }

    /// Calls `op` on the extension that serves it and returns its JSON
    /// result. Without one: `host_capability_missing` naming `what`.
    pub async fn call_extension(&self, what: &str, op: &str, args: Value) -> Result<Value> {
        let ext = self
            .extension_for(op)
            .ok_or_else(|| Error::needs_cua_spaces(what))?;
        let out = ext
            .call_tool(ToolCall {
                spaces: self,
                tool: op,
                args,
                broker: None,
                pinned: None,
            })
            .await?;
        outcome_json(out)
    }
}

/// The JSON a [`ToolOutcome`] carries (its structured content, else its text
/// parsed as JSON); a tool error becomes an [`Error`] of its kind.
pub fn outcome_json(out: ToolOutcome) -> Result<Value> {
    if out.is_error {
        let (kind, message) = out
            .structured
            .as_ref()
            .and_then(|s| s.get("error"))
            .map(|e| {
                (
                    e.get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("env")
                        .to_string(),
                    e.get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .unwrap_or_else(|| ("env".into(), out.first_text().unwrap_or_default().into()));
        return Err(Error::extension(&kind, message));
    }
    if let Some(v) = out.structured {
        return Ok(v);
    }
    let text = out.first_text().unwrap_or_default();
    Ok(serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string())))
}
