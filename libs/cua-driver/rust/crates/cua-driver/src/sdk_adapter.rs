//! Transport-facing adapter over the public Cua Driver SDK contract.
//!
//! The daemon, MCP transports, and finite CLI inspection commands all consume
//! this adapter. Platform registries remain private to `cua-driver-sdk`.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use cua_driver_core::{server::ToolProvider, CaptureScope};
use cua_driver_sdk::{CuaDriver, CuaDriverSession, TrustedSessionOptions};
use serde_json::{json, Value};

#[derive(Default)]
struct PublicSessionState {
    scopes: HashMap<String, CaptureScope>,
    ended: HashMap<String, u64>,
    next_tombstone: u64,
}

impl PublicSessionState {
    fn mark_ended(&mut self, session: &str) -> u64 {
        self.next_tombstone = self.next_tombstone.wrapping_add(1).max(1);
        let marker = self.next_tombstone;
        self.ended.insert(session.to_owned(), marker);
        marker
    }

    fn rollback_ended(&mut self, session: &str, marker: u64, previous: Option<u64>) {
        if self.ended.get(session).copied() != Some(marker) {
            return;
        }
        match previous {
            Some(previous) => {
                self.ended.insert(session.to_owned(), previous);
            }
            None => {
                self.ended.remove(session);
            }
        }
    }
}

pub struct SdkAdapter {
    driver: Arc<CuaDriver>,
    tools_list: Value,
    // Runtime-adapter-local mirror used only for the legacy socket's early
    // resurrection refusal. Core owns the authoritative runtime-private
    // tombstone; keeping this mirror on the adapter preserves the loud
    // transport error without reintroducing process-global public session IDs.
    public_sessions: Arc<Mutex<PublicSessionState>>,
    runtime_prefix: String,
    runtime_scope: String,
    _session_end_hook: cua_driver_core::session::SessionEndHookRegistration,
    session_lifecycle: tokio::sync::Mutex<()>,
}

impl SdkAdapter {
    pub async fn load(driver: Arc<CuaDriver>) -> anyhow::Result<Arc<Self>> {
        let tools_json = driver
            .list_tools_json()
            .await
            .map_err(|error| anyhow::anyhow!("load SDK tool inventory: {error}"))?;
        let tools_list: Value = serde_json::from_str(&tools_json)
            .map_err(|error| anyhow::anyhow!("parse SDK tool inventory: {error}"))?;
        if !tools_list.get("tools").is_some_and(Value::is_array) {
            anyhow::bail!("SDK tool inventory omitted tools array");
        }
        let runtime_prefix = driver.runtime_scope_prefix().ok_or_else(|| {
            anyhow::anyhow!("SDK adapter requires a directly owned embedded runtime")
        })?;
        let runtime_scope = runtime_prefix
            .strip_prefix("__cua_runtime_")
            .and_then(|value| value.strip_suffix(':'))
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("SDK adapter received an invalid runtime scope"))?
            .to_owned();
        let public_sessions = Arc::new(Mutex::new(PublicSessionState::default()));
        let hook_sessions = public_sessions.clone();
        let hook_prefix = runtime_prefix.clone();
        let session_end_hook =
            cua_driver_core::session::register_scoped_session_end_hook(move |session| {
                let Some(public) = session.strip_prefix(&hook_prefix) else {
                    return;
                };
                if public.is_empty() || public == "default" {
                    return;
                }
                let mut sessions = hook_sessions.lock().unwrap();
                sessions.scopes.entry(public.to_owned()).or_default();
                sessions.mark_ended(public);
            });
        Ok(Arc::new(Self {
            driver,
            tools_list,
            public_sessions,
            runtime_prefix,
            runtime_scope,
            _session_end_hook: session_end_hook,
            session_lifecycle: tokio::sync::Mutex::new(()),
        }))
    }

    pub fn tools_list(&self) -> Value {
        self.tools_list.clone()
    }

    pub fn is_known_tool(&self, name: &str) -> bool {
        name == "type_text_chars"
            || self
                .tools_list
                .get("tools")
                .and_then(Value::as_array)
                .is_some_and(|tools| {
                    tools
                        .iter()
                        .any(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
                })
    }

    pub fn describe(&self, name: &str) -> Option<Value> {
        self.tools_list
            .get("tools")?
            .as_array()?
            .iter()
            .find(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
            .map(|tool| {
                json!({
                    "name": tool.get("name").cloned().unwrap_or(Value::Null),
                    "description": tool.get("description").cloned().unwrap_or(Value::String(String::new())),
                    "input_schema": tool.get("inputSchema").cloned().unwrap_or_else(|| json!({"type": "object"})),
                })
            })
    }

    /// Legacy daemon inventory shape consumed by released socket clients.
    pub fn daemon_tools_list(&self) -> Value {
        daemon_tools_list_from(&self.tools_list)
    }

    pub async fn invoke_raw(&self, name: &str, arguments: Value) -> Result<Value, String> {
        let _lifecycle = if matches!(name, "start_session" | "end_session") {
            Some(self.session_lifecycle.lock().await)
        } else {
            None
        };
        let public_session = arguments
            .get("session")
            .and_then(Value::as_str)
            .filter(|session| !session.is_empty() && *session != "default")
            .map(str::to_owned);
        let ending_session = (name == "end_session")
            .then_some(public_session.as_deref())
            .flatten();
        if let Some(session) = &public_session {
            self.public_sessions
                .lock()
                .unwrap()
                .scopes
                .entry(session.clone())
                .or_default();
        }
        let ending_tombstone = ending_session.map(|session| {
            // Mark before dispatch so a concurrently arriving legacy socket
            // call cannot slip through after teardown has begun.
            let mut sessions = self.public_sessions.lock().unwrap();
            let previous = sessions.ended.get(session).copied();
            let marker = sessions.mark_ended(session);
            (session, marker, previous)
        });

        let result = self
            .driver
            .call_tool_from_trusted_adapter(name, arguments)
            .await
            .map_err(|error| {
                if let Some((session, marker, previous)) = ending_tombstone {
                    self.public_sessions
                        .lock()
                        .unwrap()
                        .rollback_ended(session, marker, previous);
                }
                error.to_string()
            })?;
        let value: Value = serde_json::from_str(&result.raw_json).map_err(|error| {
            if let Some((session, marker, previous)) = ending_tombstone {
                self.public_sessions
                    .lock()
                    .unwrap()
                    .rollback_ended(session, marker, previous);
            }
            format!("{name} returned invalid SDK result JSON: {error}")
        })?;
        let failed = value.get("isError").and_then(Value::as_bool) == Some(true);
        if failed {
            if let Some((session, marker, previous)) = ending_tombstone {
                self.public_sessions
                    .lock()
                    .unwrap()
                    .rollback_ended(session, marker, previous);
            }
        } else if let Some(session) = public_session.as_deref() {
            let capture_scope = value
                .pointer("/structuredContent/capture_scope")
                .cloned()
                .and_then(|scope| serde_json::from_value(scope).ok());
            let mut sessions = self.public_sessions.lock().unwrap();
            if name == "start_session" {
                sessions.ended.remove(session);
            }
            if let Some(capture_scope) = capture_scope {
                sessions.scopes.insert(session.to_owned(), capture_scope);
            }
        }
        Ok(value)
    }

    pub fn is_session_ended(&self, session: &str) -> bool {
        self.public_sessions
            .lock()
            .unwrap()
            .ended
            .contains_key(session)
    }

    pub fn mark_all_sessions_ended(&self) {
        let mut sessions = self.public_sessions.lock().unwrap();
        let known = sessions.scopes.keys().cloned().collect::<Vec<_>>();
        for session in known {
            sessions.mark_ended(&session);
        }
    }

    pub async fn revoke_all_sessions(&self) -> usize {
        let _lifecycle = self.session_lifecycle.lock().await;
        cua_driver_core::session::suspend_runtime_scope(&self.runtime_scope);
        let count = cua_driver_core::session::revoke_sessions_with_prefix(&self.runtime_prefix);
        self.mark_all_sessions_ended();
        count
    }

    pub fn begin_tool_call(
        &self,
        name: &str,
        arguments: &Value,
        transport: cua_driver_core::session::SessionTransport,
        client_kind: cua_driver_core::session::SessionClientKind,
    ) -> Option<cua_driver_core::session::SessionToolContext> {
        let observed_state = arguments
            .get("session")
            .and_then(Value::as_str)
            .filter(|session| !session.is_empty() && *session != "default")
            .map(|session| self.public_session_observation_state(session));
        cua_driver_core::session::begin_tool_call_with_state(
            name,
            arguments,
            self.is_known_tool(name),
            transport,
            client_kind,
            observed_state,
        )
    }

    fn public_session_observation_state(
        &self,
        session: &str,
    ) -> cua_driver_core::session::SessionObservationState {
        let sessions = self.public_sessions.lock().unwrap();
        cua_driver_core::session::SessionObservationState {
            ended: sessions.ended.contains_key(session),
            capture_scope: sessions.scopes.get(session).copied().unwrap_or_default(),
        }
    }

    pub async fn end_session(&self, session: &str) -> Result<(), String> {
        self.invoke_raw("end_session", json!({"session": session}))
            .await
            .map(|_| ())
    }

    pub fn create_trusted_session(
        &self,
        options: TrustedSessionOptions,
    ) -> Result<Arc<CuaDriverSession>, String> {
        self.driver
            .create_trusted_session(options)
            .map_err(|error| error.to_string())
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        self.driver
            .shutdown()
            .await
            .map_err(|error| error.to_string())
    }
}

fn daemon_tools_list_from(tools_list: &Value) -> Value {
    let tools = tools_list
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|tool| {
                let annotations = tool.get("annotations").unwrap_or(&Value::Null);
                let mut daemon_tool = json!({
                    "name": tool.get("name").cloned().unwrap_or(Value::Null),
                    "description": tool.get("description").cloned().unwrap_or(Value::String(String::new())),
                    "input_schema": tool.get("inputSchema").cloned().unwrap_or_else(|| json!({"type": "object"})),
                    "read_only": annotations.get("readOnlyHint").cloned().unwrap_or(Value::Bool(false)),
                    "destructive": annotations.get("destructiveHint").cloned().unwrap_or(Value::Bool(false)),
                    "idempotent": annotations.get("idempotentHint").cloned().unwrap_or(Value::Bool(false)),
                    "open_world": annotations.get("openWorldHint").cloned().unwrap_or(Value::Bool(false)),
                    "capabilities": tool.get("capabilities").cloned().unwrap_or_else(|| json!([])),
                    "risk": tool.get("risk").cloned().unwrap_or(Value::Null),
                });
                if let Some(output_schema) = tool.get("outputSchema") {
                    daemon_tool
                        .as_object_mut()
                        .expect("daemon tool entry is an object")
                        .insert("output_schema".into(), output_schema.clone());
                }
                daemon_tool
            })
            .collect::<Vec<_>>();
    json!({
        "tools": tools,
        "capability_version": tools_list.get("capability_version").cloned().unwrap_or(Value::Null),
        "schema_version": tools_list.get("schema_version").cloned().unwrap_or(Value::Null),
        "enforcement_adapters": tools_list.get("enforcement_adapters").cloned().unwrap_or_else(|| json!([])),
        "tool_observation_owner": "daemon",
    })
}

#[async_trait::async_trait]
impl ToolProvider for SdkAdapter {
    fn tools_list(&self) -> Value {
        self.tools_list()
    }

    async fn invoke_tool(&self, name: &str, arguments: Value) -> Result<Value, String> {
        self.invoke_raw(name, arguments).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_driver() -> Arc<cua_driver_sdk::CuaDriver> {
        cua_driver_sdk::CuaDriver::create_for_host(cua_driver_sdk::DriverHostOptions {
            cursor: cursor_overlay::CursorConfig {
                enabled: false,
                ..cursor_overlay::CursorConfig::default()
            },
            host_owns_permission_ux: false,
            host_bundle_id: None,
            claude_code_compatibility: false,
            prepare_desktop_environment: false,
            register_host_tools: None,
            authorization_host: None,
            activity_observer: None,
        })
    }

    #[test]
    fn daemon_shape_is_derived_from_canonical_mcp_inventory() {
        let tools_list = json!({
            "tools": [{
                "name": "probe",
                "description": "Probe.",
                "inputSchema": {"type": "object"},
                "annotations": {
                    "readOnlyHint": true,
                    "destructiveHint": false,
                    "idempotentHint": true,
                    "openWorldHint": false
                },
                "capabilities": ["probe.read"],
                "risk": {"level": "low"},
                "outputSchema": {
                    "type": "object",
                    "required": ["value"],
                    "properties": {"value": {"type": "string"}}
                }
            }],
            "capability_version": "1",
            "schema_version": "1",
            "enforcement_adapters": [{
                "id": "browser_prepare.existing_profile",
                "state": "active"
            }]
        });
        let daemon = daemon_tools_list_from(&tools_list);
        assert_eq!(daemon["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(
            daemon["tools"][0]["output_schema"]["required"],
            json!(["value"])
        );
        assert_eq!(daemon["tools"][0]["read_only"], true);
        assert_eq!(
            daemon["enforcement_adapters"][0]["id"],
            "browser_prepare.existing_profile"
        );
        assert_eq!(daemon["tool_observation_owner"], "daemon");
    }

    #[tokio::test]
    async fn public_observation_mirror_tracks_scope_end_and_revival() {
        let _runtime_guard = crate::test_runtime_lock().lock().await;
        let sdk = SdkAdapter::load(host_driver()).await.expect("SDK adapter");
        let session = "adapter-observation-scope";

        let started = sdk
            .invoke_raw(
                "start_session",
                json!({"session": session, "capture_scope": "window"}),
            )
            .await
            .expect("start session");
        assert_ne!(started["isError"], true);
        assert_eq!(
            sdk.public_session_observation_state(session),
            cua_driver_core::session::SessionObservationState {
                ended: false,
                capture_scope: CaptureScope::Window,
            }
        );

        sdk.end_session(session).await.expect("end session");
        assert!(sdk.public_session_observation_state(session).ended);

        sdk.invoke_raw(
            "start_session",
            json!({"session": session, "capture_scope": "window"}),
        )
        .await
        .expect("revive session");
        assert!(!sdk.public_session_observation_state(session).ended);

        sdk.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn runtime_owned_end_hook_updates_only_its_adapter() {
        let _runtime_guard = crate::test_runtime_lock().lock().await;
        let first = SdkAdapter::load(host_driver())
            .await
            .expect("first SDK adapter");
        let second = SdkAdapter::load(host_driver())
            .await
            .expect("second SDK adapter");
        let session = "same-public-label";

        for sdk in [&first, &second] {
            sdk.invoke_raw(
                "start_session",
                json!({"session": session, "capture_scope": "auto"}),
            )
            .await
            .expect("start same-label session");
        }

        let evicted = cua_driver_core::session::evict_idle_with_prefix(
            std::time::Duration::ZERO,
            &first.runtime_prefix,
        );
        assert_eq!(evicted, vec![format!("{}{session}", first.runtime_prefix)]);
        assert!(first.public_session_observation_state(session).ended);
        assert!(
            !second.public_session_observation_state(session).ended,
            "a private end hook must not tombstone another runtime's same public label"
        );

        first
            .invoke_raw(
                "start_session",
                json!({"session": session, "capture_scope": "auto"}),
            )
            .await
            .expect("revive first session");
        assert_eq!(first.revoke_all_sessions().await, 1);
        assert!(first.public_session_observation_state(session).ended);
        assert!(
            !second.public_session_observation_state(session).ended,
            "bulk revoke must remain scoped to the owning runtime"
        );

        second
            .invoke_raw(
                "escalate_session",
                json!({"session": session, "reason": "no_window_target"}),
            )
            .await
            .expect("escalate second session");
        assert_eq!(
            second
                .public_session_observation_state(session)
                .capture_scope,
            CaptureScope::Auto,
            "capture policy remains auto while its effective scope escalates"
        );

        first.shutdown().await.expect("shutdown first");
        second.shutdown().await.expect("shutdown second");
    }

    #[tokio::test]
    async fn failed_end_preserves_an_existing_out_of_band_tombstone() {
        let _runtime_guard = crate::test_runtime_lock().lock().await;
        let sdk = SdkAdapter::load(host_driver()).await.expect("SDK adapter");
        let session = "failed-end-preserves-tombstone";
        sdk.invoke_raw(
            "start_session",
            json!({"session": session, "capture_scope": "auto"}),
        )
        .await
        .expect("start session");

        sdk.shutdown().await.expect("shutdown");
        assert!(sdk.public_session_observation_state(session).ended);
        assert!(sdk.end_session(session).await.is_err());
        assert!(
            sdk.public_session_observation_state(session).ended,
            "a failed in-band end must not erase an out-of-band tombstone"
        );
    }
}
