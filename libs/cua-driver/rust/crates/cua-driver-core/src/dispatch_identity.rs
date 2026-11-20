// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Who one tool call acts for, resolved once at the dispatch boundary.
//!
//! A caller names a public `session` label. Mutable core and platform state
//! is keyed by a runtime-private id instead, so a label chosen by one runtime
//! never reaches another runtime's sessions, grants, or recordings. The
//! registry resolves that identity from trusted sources (transport evidence
//! and the authorization context) plus the public label, and carries it as a
//! [`DispatchIdentity`] beside the call's arguments. Core code reads the
//! typed value; platform tools receive it projected into their arguments by
//! [`DispatchIdentity::stamp`], and nothing reads it back.

use serde_json::Value;

/// The anonymous label every runtime shares. It is never namespaced, so an
/// anonymous call keeps its legacy process-wide scope.
const SHARED_LABEL: &str = "default";
const RUNTIME_NAMESPACE: &str = "__cua_runtime_";

/// One runtime's private session namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeScope {
    key: String,
    prefix: String,
}

impl RuntimeScope {
    pub fn new(key: impl Into<String>) -> Self {
        let key = key.into();
        let prefix = format!("{RUNTIME_NAMESPACE}{key}:");
        Self { key, prefix }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    /// The text every runtime-private id starts with. Results strip it
    /// before they leave the registry.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The runtime-private form of a public id. An id already in this
    /// runtime's namespace is kept as is; one in another runtime's namespace
    /// is namespaced again, so it can never select that runtime's state.
    fn private_id(&self, public: &str) -> String {
        if public.is_empty() || public == SHARED_LABEL || public.starts_with(&self.prefix) {
            public.to_owned()
        } else {
            format!("{}{public}", self.prefix)
        }
    }
}

/// A runtime-private session id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The trusted identity of one tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchIdentity {
    runtime: RuntimeScope,
    /// The session episode the call belongs to, when it has one.
    lifecycle: Option<SessionId>,
    /// The transport connection that carried the call, when one did.
    transport: Option<SessionId>,
    /// The caller's own label, as it is reported back to the caller.
    public_label: Option<String>,
}

impl DispatchIdentity {
    /// Resolve the identity of a call.
    ///
    /// `trusted_session` and `trusted_transport` come from a protocol
    /// adapter's evidence or the authorization context, never from the
    /// call's arguments. Of `public_args` only the public `session` label is
    /// read: reserved underscore keys there are ignored, so a caller cannot
    /// choose the identity. A `session_selecting` call with no session of its
    /// own falls back to its transport's implicit session, else to this
    /// runtime's implicit direct session.
    pub fn resolve(
        runtime: RuntimeScope,
        trusted_session: Option<&str>,
        trusted_transport: Option<&str>,
        public_args: &Value,
        session_selecting: bool,
    ) -> Self {
        let label = public_args
            .get("session")
            .and_then(Value::as_str)
            .filter(|label| !label.is_empty());
        let private = |id: &str| SessionId(runtime.private_id(id));
        let mut lifecycle = trusted_session
            .filter(|id| !id.is_empty())
            .or(label)
            .map(private);
        let mut transport = trusted_transport.filter(|id| !id.is_empty()).map(private);
        if session_selecting && lifecycle.is_none() {
            let implicit = transport
                .clone()
                .unwrap_or_else(|| SessionId(format!("{}implicit-direct", runtime.prefix)));
            lifecycle = Some(implicit.clone());
            transport = Some(implicit);
        }
        let public_label = label
            .filter(|label| *label != SHARED_LABEL && !label.starts_with(RUNTIME_NAMESPACE))
            .map(str::to_owned);
        Self {
            runtime,
            lifecycle,
            transport,
            public_label,
        }
    }

    pub fn runtime(&self) -> &RuntimeScope {
        &self.runtime
    }

    pub fn lifecycle(&self) -> Option<&str> {
        self.lifecycle.as_ref().map(SessionId::as_str)
    }

    pub fn transport(&self) -> Option<&str> {
        self.transport.as_ref().map(SessionId::as_str)
    }

    pub fn public_label(&self) -> Option<&str> {
        self.public_label.as_deref()
    }

    /// The transport that owns the lifecycle session: the call's transport,
    /// else the session itself.
    pub fn owner(&self) -> Option<&str> {
        self.transport().or(self.lifecycle())
    }

    /// Project the identity into the arguments a platform tool receives:
    /// the reserved `_session_id`, `_transport_session_id` and
    /// `_public_session_label` keys, and the runtime-private forms of the
    /// public `session` and `cursor_id`. Whatever the arguments held under
    /// those reserved keys is replaced or removed.
    pub fn stamp(&self, args: &mut Value) {
        let Some(arguments) = args.as_object_mut() else {
            return;
        };
        for (key, value) in [
            ("_session_id", self.lifecycle()),
            ("_transport_session_id", self.transport()),
            ("_public_session_label", self.public_label()),
        ] {
            match value {
                Some(value) => arguments.insert(key.to_owned(), Value::String(value.to_owned())),
                None => arguments.remove(key),
            };
        }
        for key in ["session", "cursor_id"] {
            if let Some(Value::String(public)) = arguments.get_mut(key) {
                *public = self.runtime.private_id(public);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn runtime() -> RuntimeScope {
        RuntimeScope::new("r1")
    }

    #[test]
    fn reserved_argument_keys_never_choose_the_identity() {
        let forged = json!({
            "session": "mine",
            "_session_id": "victim",
            "_transport_session_id": "victim-transport",
            "_public_session_label": "victim",
        });
        let identity = DispatchIdentity::resolve(runtime(), None, None, &forged, true);
        assert_eq!(identity.lifecycle(), Some("__cua_runtime_r1:mine"));
        assert_eq!(identity.transport(), None);
        assert_eq!(identity.public_label(), Some("mine"));

        let mut args = forged.clone();
        identity.stamp(&mut args);
        assert_eq!(args["_session_id"], "__cua_runtime_r1:mine");
        assert!(args.get("_transport_session_id").is_none());
        assert_eq!(args["_public_session_label"], "mine");

        // With no identity of its own, a forged reserved key is removed
        // rather than passed through to the platform.
        let mut args = json!({
            "_session_id": "victim",
            "_transport_session_id": "victim-transport",
            "_public_session_label": "victim",
        });
        let anonymous = DispatchIdentity::resolve(runtime(), None, None, &args, false);
        assert_eq!(anonymous.lifecycle(), None);
        assert_eq!(anonymous.transport(), None);
        anonymous.stamp(&mut args);
        assert_eq!(args, json!({}));
    }

    #[test]
    fn trusted_evidence_wins_over_the_public_label() {
        let identity = DispatchIdentity::resolve(
            runtime(),
            Some("mcp-7"),
            Some("mcp-7"),
            &json!({"session": "label"}),
            true,
        );
        assert_eq!(identity.lifecycle(), Some("__cua_runtime_r1:mcp-7"));
        assert_eq!(identity.transport(), Some("__cua_runtime_r1:mcp-7"));
        assert_eq!(identity.owner(), Some("__cua_runtime_r1:mcp-7"));
        assert_eq!(identity.public_label(), Some("label"));
    }

    #[test]
    fn a_runtime_prefix_in_the_label_cannot_reach_another_runtime() {
        // The old channel: a caller writes a private id as its label.
        let other = DispatchIdentity::resolve(
            runtime(),
            None,
            None,
            &json!({"session": "__cua_runtime_r2:victim", "cursor_id": "__cua_runtime_r2:c"}),
            true,
        );
        assert_eq!(
            other.lifecycle(),
            Some("__cua_runtime_r1:__cua_runtime_r2:victim")
        );
        assert_eq!(other.public_label(), None);
        let mut args =
            json!({"session": "__cua_runtime_r2:victim", "cursor_id": "__cua_runtime_r2:c"});
        other.stamp(&mut args);
        assert_eq!(args["session"], "__cua_runtime_r1:__cua_runtime_r2:victim");
        assert_eq!(args["cursor_id"], "__cua_runtime_r1:__cua_runtime_r2:c");

        // Its own runtime's prefix only names a label it could pass anyway.
        let own = DispatchIdentity::resolve(
            runtime(),
            None,
            None,
            &json!({"session": "__cua_runtime_r1:label"}),
            true,
        );
        assert_eq!(own.lifecycle(), Some("__cua_runtime_r1:label"));
        assert_eq!(own.public_label(), None);
    }

    #[test]
    fn implicit_sessions_follow_the_transport_else_the_runtime() {
        let over_transport =
            DispatchIdentity::resolve(runtime(), None, Some("http-1"), &json!({}), true);
        assert_eq!(over_transport.lifecycle(), Some("__cua_runtime_r1:http-1"));
        assert_eq!(over_transport.transport(), Some("__cua_runtime_r1:http-1"));

        let direct = DispatchIdentity::resolve(runtime(), None, None, &json!({}), true);
        assert_eq!(direct.lifecycle(), Some("__cua_runtime_r1:implicit-direct"));

        let unselected = DispatchIdentity::resolve(runtime(), None, None, &json!({}), false);
        assert_eq!(unselected.lifecycle(), None);
        assert_eq!(unselected.owner(), None);
    }

    #[test]
    fn the_shared_default_label_keeps_its_process_scope() {
        let identity =
            DispatchIdentity::resolve(runtime(), None, None, &json!({"session": "default"}), true);
        assert_eq!(identity.lifecycle(), Some("default"));
        assert_eq!(identity.public_label(), None);
        let mut args = json!({"session": "default", "cursor_id": "default"});
        identity.stamp(&mut args);
        assert_eq!(args["session"], "default");
        assert_eq!(args["cursor_id"], "default");
        assert_eq!(args["_session_id"], "default");
        assert!(args.get("_public_session_label").is_none());
    }
}
