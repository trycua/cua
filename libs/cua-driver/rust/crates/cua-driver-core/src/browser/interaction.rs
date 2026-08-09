//! Focus, control-key, native select, and direct scrolling browser actions.
//!
//! These tools share the browser engine's exact-or-refused binding, mutation
//! serialization, origin authorization, and frame revalidation. They use only
//! fixed CDP commands and fixed JavaScript functions; model-provided values are
//! always passed as structured arguments.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::protocol::ToolResult;
use crate::tool::{ProtectedResourceOwnership, Tool, ToolDef};
use crate::tool_args::ArgsExt;

use super::cdp_ws::CdpConnection;
use super::engine::{BrowserEngine, ValidatedTab};
use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::store::{BrowserActionKind, RefEntry};
use super::tools::{browser_protected_resource_scope, browser_resource_ownership};
use super::types::BrowserProduct;

const ACTIVE_ELEMENT_CHECK: &str = "function() { const root=this.getRootNode(); return ('activeElement' in root) && root.activeElement === this; }";

const SELECT_OPTION_BY_LABEL: &str = "function(label) { \
    const view=this.ownerDocument && this.ownerDocument.defaultView; \
    if (!view || !(this instanceof view.HTMLSelectElement)) return {kind:'wrong_role'}; \
    if (this.disabled) return {kind:'disabled'}; \
    const wanted=String(label).trim(); \
    const matches=Array.from(this.options).filter(option => \
        String(option.textContent || '').trim() === wanted); \
    if (matches.length !== 1) return {kind:'bad_option',matches:matches.length}; \
    const option=matches[0]; \
    if (option.disabled || (option.parentElement && option.parentElement.disabled)) \
        return {kind:'disabled_option'}; \
    const setter=Object.getOwnPropertyDescriptor(view.HTMLSelectElement.prototype,'value').set; \
    if (!setter) return {kind:'unsupported'}; \
    setter.call(this,option.value); \
    this.dispatchEvent(new view.Event('input',{bubbles:true,composed:true})); \
    this.dispatchEvent(new view.Event('change',{bubbles:true})); \
    return this.selectedIndex === option.index && option.selected \
        ? {kind:'ok',label:String(option.textContent || '').trim()} \
        : {kind:'postcondition_failed'}; \
}";

fn schema_ids(extra: Value, required: &[&str]) -> Value {
    let mut properties = json!({
        "target_id": {"type":"string","description":"Opaque browser target id minted by get_browser_state."},
        "tab_id": {"type":"string","description":"Opaque tab id minted by get_browser_state."},
        "session": {"type":"string","description":"Explicit caller session owning the browser capabilities."}
    });
    if let (Some(base), Some(additions)) = (properties.as_object_mut(), extra.as_object()) {
        base.extend(additions.clone());
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn explicit_session(args: &Value) -> Result<String, ToolResult> {
    let session = args
        .opt_str("session")
        .or_else(|| args.opt_str("_session_id"))
        .unwrap_or_else(|| "default".into());
    if session.is_empty() || session == "default" {
        return Err(ToolResult::error(
            "Browser targets and page refs are session-scoped capabilities - declare an explicit session (start_session) and pass its id on this call.",
        ));
    }
    Ok(session)
}

async fn authorized_tab(
    engine: &BrowserEngine,
    session: &str,
    target_id: &str,
    tab_id: &str,
) -> Result<ValidatedTab, ToolResult> {
    engine
        .revalidate_for_mutation(session, target_id, Some(tab_id))
        .await
        .map_err(|refusal| refusal.to_tool_result())
}

async fn exact_ref(
    engine: &BrowserEngine,
    session: &str,
    target_id: &str,
    tab_id: &str,
    validated: &ValidatedTab,
    external_ref: &str,
) -> Result<(RefEntry, String), ToolResult> {
    let entry = engine
        .store
        .resolve_ref(session, target_id, tab_id, external_ref)
        .map_err(|refusal| refusal.to_tool_result())?;
    let cdp_session = engine
        .frame_session_for_mutation(session, target_id, tab_id, validated, &entry.frame)
        .await
        .map_err(|refusal| refusal.to_tool_result())?;
    Ok((entry, cdp_session))
}

async fn scroll_and_focus(
    conn: &CdpConnection,
    cdp_session: &str,
    backend_node_id: i64,
) -> Result<(), ToolResult> {
    conn.call(
        Some(cdp_session),
        "DOM.scrollIntoViewIfNeeded",
        json!({"backendNodeId": backend_node_id}),
    )
    .await
    .map_err(|_| stale("the ref's node no longer has live page layout"))?;
    conn.call(
        Some(cdp_session),
        "DOM.focus",
        json!({"backendNodeId": backend_node_id}),
    )
    .await
    .map_err(|_| stale("the ref's node can no longer be focused"))?;
    Ok(())
}

async fn resolve_object(
    conn: &CdpConnection,
    cdp_session: &str,
    backend_node_id: i64,
) -> Result<String, ToolResult> {
    conn.call(
        Some(cdp_session),
        "DOM.resolveNode",
        json!({"backendNodeId": backend_node_id}),
    )
    .await
    .ok()
    .and_then(|value| {
        value
            .pointer("/object/objectId")
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
    .ok_or_else(|| stale("the ref's node no longer resolves in the live page"))
}

fn stale(message: &str) -> ToolResult {
    BrowserRefusal::new(BrowserRefusalCode::BrowserRefStale, message).to_tool_result()
}

fn unavailable(message: impl Into<String>, reason: &str) -> ToolResult {
    BrowserRefusal::new(BrowserRefusalCode::BrowserActionUnavailable, message)
        .with_detail(json!({"reason": reason}))
        .to_tool_result()
}

macro_rules! protected_browser_input {
    ($tool:ty, $name:literal) => {
        #[async_trait]
        impl Tool for $tool {
            fn def(&self) -> &ToolDef {
                &self.def
            }

            async fn protected_resource_ownership(
                &self,
                adapter_id: &str,
                args: &Value,
            ) -> ProtectedResourceOwnership {
                if adapter_id == "browser_bound_input" {
                    browser_resource_ownership(&self.engine, args)
                } else {
                    ProtectedResourceOwnership::UserOwned
                }
            }

            async fn protected_resource_scope(
                &self,
                adapter_id: &str,
                args: &Value,
            ) -> Result<Option<Value>, String> {
                if adapter_id == "browser_bound_input" {
                    browser_protected_resource_scope(&self.engine, args, $name).await
                } else {
                    Ok(None)
                }
            }

            async fn invoke(&self, args: Value) -> ToolResult {
                self.invoke_inner(args).await
            }
        }
    };
}

pub(crate) struct BrowserFocusTool {
    def: ToolDef,
    engine: Arc<BrowserEngine>,
}

impl BrowserFocusTool {
    pub(crate) fn new(engine: Arc<BrowserEngine>) -> Self {
        Self {
            def: ToolDef {
                name: "browser_focus".into(),
                description: "Scroll a current page ref into view, focus it through DOM.focus, and verify it is the active element in its exact document. Accepts action and content refs; refuses stale or unfocusable refs.".into(),
                input_schema: schema_ids(
                    json!({"ref":{"type":"string","description":"Current page ref to focus."}}),
                    &["target_id", "tab_id", "session", "ref"],
                ),
                read_only: false,
                destructive: false,
                idempotent: true,
                open_world: true,
            },
            engine,
        }
    }

    async fn invoke_inner(&self, args: Value) -> ToolResult {
        let (target_id, tab_id, external_ref) = match (
            args.require_str("target_id"),
            args.require_str("tab_id"),
            args.require_str("ref"),
        ) {
            (Ok(target), Ok(tab), Ok(reference)) => (target, tab, reference),
            (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => return error,
        };
        let session = match explicit_session(&args) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let _mutation = match self
            .engine
            .lock_mutation(&session, &target_id, &tab_id)
            .await
        {
            Ok(guard) => guard,
            Err(refusal) => return refusal.to_tool_result(),
        };
        let validated = match authorized_tab(&self.engine, &session, &target_id, &tab_id).await {
            Ok(value) => value,
            Err(error) => return error,
        };
        let (entry, cdp_session) = match exact_ref(
            &self.engine,
            &session,
            &target_id,
            &tab_id,
            &validated,
            &external_ref,
        )
        .await
        {
            Ok(value) => value,
            Err(error) => return error,
        };
        if let Err(error) =
            scroll_and_focus(&validated.conn, &cdp_session, entry.backend_node_id).await
        {
            return error;
        }
        let object_id =
            match resolve_object(&validated.conn, &cdp_session, entry.backend_node_id).await {
                Ok(value) => value,
                Err(error) => return error,
            };
        let active = validated
            .conn
            .call(
                Some(&cdp_session),
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": ACTIVE_ELEMENT_CHECK,
                    "returnByValue": true
                }),
            )
            .await;
        if !matches!(active, Ok(ref value) if value.pointer("/result/value").and_then(Value::as_bool) == Some(true))
        {
            return unavailable(
                "the ref did not become the active element in its exact document",
                "focus_postcondition_failed",
            );
        }
        ToolResult::text(format!("focused {external_ref} in {tab_id}")).with_structured(json!({
            "status":"ok", "target_id":target_id, "tab_id":tab_id,
            "ref":external_ref, "frame":entry.frame.kind.as_str(), "route":"dom_focus"
        }))
    }
}
protected_browser_input!(BrowserFocusTool, "browser_focus");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeySpec {
    name: &'static str,
    key: &'static str,
    code: &'static str,
    virtual_key: u16,
    location: Option<u8>,
    text: Option<&'static str>,
}

impl KeySpec {
    fn parse(name: &str) -> Option<Self> {
        let (key, code, virtual_key, location, text) = match name {
            "Enter" => ("Enter", "Enter", 13, None, None),
            "NumpadEnter" => ("Enter", "NumpadEnter", 13, Some(3), None),
            "Space" => (" ", "Space", 32, None, Some(" ")),
            "Backspace" => ("Backspace", "Backspace", 8, None, None),
            "Delete" => ("Delete", "Delete", 46, None, None),
            "Tab" => ("Tab", "Tab", 9, None, None),
            "Escape" => ("Escape", "Escape", 27, None, None),
            "ArrowUp" => ("ArrowUp", "ArrowUp", 38, None, None),
            "ArrowDown" => ("ArrowDown", "ArrowDown", 40, None, None),
            "ArrowLeft" => ("ArrowLeft", "ArrowLeft", 37, None, None),
            "ArrowRight" => ("ArrowRight", "ArrowRight", 39, None, None),
            "Home" => ("Home", "Home", 36, None, None),
            "End" => ("End", "End", 35, None, None),
            "PageUp" => ("PageUp", "PageUp", 33, None, None),
            "PageDown" => ("PageDown", "PageDown", 34, None, None),
            _ => return None,
        };
        Some(Self {
            name: match name {
                "Enter" => "Enter",
                "NumpadEnter" => "NumpadEnter",
                "Space" => "Space",
                "Backspace" => "Backspace",
                "Delete" => "Delete",
                "Tab" => "Tab",
                "Escape" => "Escape",
                "ArrowUp" => "ArrowUp",
                "ArrowDown" => "ArrowDown",
                "ArrowLeft" => "ArrowLeft",
                "ArrowRight" => "ArrowRight",
                "Home" => "Home",
                "End" => "End",
                "PageUp" => "PageUp",
                "PageDown" => "PageDown",
                _ => unreachable!(),
            },
            key,
            code,
            virtual_key,
            location,
            text,
        })
    }

    fn params(self, event_type: &str) -> Value {
        let mut params = json!({
            "type":event_type, "key":self.key, "code":self.code,
            "windowsVirtualKeyCode":self.virtual_key,
            "nativeVirtualKeyCode":self.virtual_key
        });
        if let Some(location) = self.location {
            params["location"] = json!(location);
            params["isKeypad"] = json!(true);
        }
        if event_type == "keyDown" {
            if let Some(text) = self.text {
                params["text"] = json!(text);
                params["unmodifiedText"] = json!(text);
            }
        }
        params
    }
}

pub(crate) struct BrowserPressKeyTool {
    def: ToolDef,
    engine: Arc<BrowserEngine>,
}

impl BrowserPressKeyTool {
    pub(crate) fn new(engine: Arc<BrowserEngine>) -> Self {
        Self {
            def: ToolDef {
                name: "browser_press_key".into(),
                description: "Focus a current page ref and dispatch one closed, trusted control or navigation key through Chromium Input. Modifiers, chords, and arbitrary strings are intentionally unsupported.".into(),
                input_schema: schema_ids(json!({
                    "ref":{"type":"string","description":"Current page ref that receives the key."},
                    "key":{"type":"string","enum":["Enter","NumpadEnter","Space","Backspace","Delete","Tab","Escape","ArrowUp","ArrowDown","ArrowLeft","ArrowRight","Home","End","PageUp","PageDown"]}
                }), &["target_id", "tab_id", "session", "ref", "key"]),
                read_only: false,
                destructive: false,
                idempotent: false,
                open_world: true,
            },
            engine,
        }
    }

    async fn invoke_inner(&self, args: Value) -> ToolResult {
        let (target_id, tab_id, external_ref, key_name) = match (
            args.require_str("target_id"),
            args.require_str("tab_id"),
            args.require_str("ref"),
            args.require_str("key"),
        ) {
            (Ok(target), Ok(tab), Ok(reference), Ok(key)) => (target, tab, reference, key),
            (Err(error), _, _, _)
            | (_, Err(error), _, _)
            | (_, _, Err(error), _)
            | (_, _, _, Err(error)) => return error,
        };
        let key = match KeySpec::parse(&key_name) {
            Some(value) => value,
            None => return ToolResult::error(format!("unsupported browser key {key_name:?}")),
        };
        let session = match explicit_session(&args) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let _mutation = match self
            .engine
            .lock_mutation(&session, &target_id, &tab_id)
            .await
        {
            Ok(guard) => guard,
            Err(refusal) => return refusal.to_tool_result(),
        };
        let validated = match authorized_tab(&self.engine, &session, &target_id, &tab_id).await {
            Ok(value) => value,
            Err(error) => return error,
        };
        let (entry, cdp_session) = match exact_ref(
            &self.engine,
            &session,
            &target_id,
            &tab_id,
            &validated,
            &external_ref,
        )
        .await
        {
            Ok(value) => value,
            Err(error) => return error,
        };
        // Enter focus emulation before focusing the exact node. Enabling it
        // after DOM.focus can move Chromium's effective key target back to the
        // document in a background WebContentsView, producing an acknowledged
        // key command that the intended control never receives.
        if let Err(error) = validated
            .conn
            .call(
                Some(&cdp_session),
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled":true}),
            )
            .await
        {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserInputTrustUnavailable,
                format!("the tab could not enter trusted focus emulation: {error}"),
            )
            .to_tool_result();
        }
        if let Err(error) =
            scroll_and_focus(&validated.conn, &cdp_session, entry.backend_node_id).await
        {
            let _ = validated
                .conn
                .call(
                    Some(&cdp_session),
                    "Emulation.setFocusEmulationEnabled",
                    json!({"enabled":false}),
                )
                .await;
            return error;
        }
        let mut delivery = Ok(json!({}));
        for event_type in ["keyDown", "keyUp"] {
            if delivery.is_ok() {
                delivery = validated
                    .conn
                    .call(
                        Some(&cdp_session),
                        "Input.dispatchKeyEvent",
                        key.params(event_type),
                    )
                    .await;
            }
        }
        let cleanup = validated
            .conn
            .call(
                Some(&cdp_session),
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled":false}),
            )
            .await;
        if let Err(error) = delivery {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserInputTrustUnavailable,
                format!("trusted key delivery failed: {error}"),
            )
            .to_tool_result();
        }
        if let Err(error) = cleanup {
            return BrowserRefusal::new(BrowserRefusalCode::BrowserInputTrustUnavailable, format!("the key was acknowledged but focus emulation could not be restored ({error}); delivery is unknown and must not be retried automatically"))
                .with_detail(json!({"delivery":"unknown","retryable":false})).to_tool_result();
        }
        ToolResult::text(format!("pressed {} on {external_ref}", key.name)).with_structured(json!({
            "status":"ok", "target_id":target_id, "tab_id":tab_id, "ref":external_ref,
            "frame":entry.frame.kind.as_str(), "key":key.name, "route":"trusted"
        }))
    }
}
protected_browser_input!(BrowserPressKeyTool, "browser_press_key");

pub(crate) struct BrowserSelectTool {
    def: ToolDef,
    engine: Arc<BrowserEngine>,
}

impl BrowserSelectTool {
    pub(crate) fn new(engine: Arc<BrowserEngine>) -> Self {
        Self {
            def: ToolDef {
                name: "browser_select".into(),
                description: "Choose exactly one enabled option in a native HTML select by its trimmed visible label, then emit input/change and verify the selected option. Custom combobox widgets are refused rather than guessed.".into(),
                input_schema: schema_ids(json!({
                    "ref":{"type":"string","description":"Current semantic ref for a native HTML select."},
                    "option":{"type":"string","description":"Exact trimmed visible option label."}
                }), &["target_id", "tab_id", "session", "ref", "option"]),
                read_only: false,
                destructive: false,
                idempotent: true,
                open_world: true,
            },
            engine,
        }
    }

    async fn invoke_inner(&self, args: Value) -> ToolResult {
        let (target_id, tab_id, external_ref, option) = match (
            args.require_str("target_id"),
            args.require_str("tab_id"),
            args.require_str("ref"),
            args.require_str("option"),
        ) {
            (Ok(target), Ok(tab), Ok(reference), Ok(option)) => (target, tab, reference, option),
            (Err(error), _, _, _)
            | (_, Err(error), _, _)
            | (_, _, Err(error), _)
            | (_, _, _, Err(error)) => return error,
        };
        let session = match explicit_session(&args) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let _mutation = match self
            .engine
            .lock_mutation(&session, &target_id, &tab_id)
            .await
        {
            Ok(guard) => guard,
            Err(refusal) => return refusal.to_tool_result(),
        };
        let validated = match authorized_tab(&self.engine, &session, &target_id, &tab_id).await {
            Ok(value) => value,
            Err(error) => return error,
        };
        let (entry, cdp_session) = match exact_ref(
            &self.engine,
            &session,
            &target_id,
            &tab_id,
            &validated,
            &external_ref,
        )
        .await
        {
            Ok(value) => value,
            Err(error) => return error,
        };
        if entry.semantic && !entry.actions.contains(&BrowserActionKind::Select) {
            return unavailable(
                format!("semantic ref {external_ref} is not a native HTML select"),
                "wrong_role",
            );
        }
        if let Err(error) =
            scroll_and_focus(&validated.conn, &cdp_session, entry.backend_node_id).await
        {
            return error;
        }
        let object_id =
            match resolve_object(&validated.conn, &cdp_session, entry.backend_node_id).await {
                Ok(value) => value,
                Err(error) => return error,
            };
        let selected = match validated
            .conn
            .call(
                Some(&cdp_session),
                "Runtime.callFunctionOn",
                json!({
                    "objectId":object_id, "functionDeclaration":SELECT_OPTION_BY_LABEL,
                    "arguments":[{"value":option}], "returnByValue":true
                }),
            )
            .await
        {
            Ok(value) => value,
            Err(error) => {
                return ToolResult::error(format!("native select delivery failed: {error}"))
            }
        };
        let result = selected.pointer("/result/value").unwrap_or(&Value::Null);
        match result.get("kind").and_then(Value::as_str) {
            Some("ok") => ToolResult::text(format!("selected an option in {external_ref}")).with_structured(json!({
                "status":"ok", "target_id":target_id, "tab_id":tab_id, "ref":external_ref,
                "frame":entry.frame.kind.as_str(), "option":result.get("label").and_then(Value::as_str),
                "route":"dom_native_select", "verified":true
            })),
            Some("bad_option") => BrowserRefusal::new(BrowserRefusalCode::BrowserActionUnavailable, "no unique enabled option matched the exact trimmed visible label")
                .with_detail(json!({"reason":"bad_option","matches":result.get("matches").and_then(Value::as_u64)})).to_tool_result(),
            Some("wrong_role") => unavailable("the live ref is not a native HTML select", "wrong_role"),
            Some("disabled") => unavailable("the native select is disabled", "disabled"),
            Some("disabled_option") => unavailable("the matching option is disabled", "disabled_option"),
            Some("postcondition_failed") => unavailable("the selected option postcondition was not observed", "postcondition_failed"),
            _ => unavailable("the page does not expose a supported native select setter", "unsupported"),
        }
    }
}
protected_browser_input!(BrowserSelectTool, "browser_select");

pub(crate) struct BrowserScrollTool {
    def: ToolDef,
    engine: Arc<BrowserEngine>,
}

impl BrowserScrollTool {
    pub(crate) fn new(engine: Arc<BrowserEngine>) -> Self {
        Self {
            def: ToolDef {
                name: "browser_scroll".into(),
                description: "Either scroll a current action or content ref into view, or dispatch one viewport-centered trusted page scroll up/down by about 80% of the visible height. Exactly one of ref or direction is required.".into(),
                input_schema: {
                    let mut schema = schema_ids(json!({
                        "ref":{"type":"string","description":"Current page ref to reveal."},
                        "direction":{"type":"string","enum":["up","down"],"description":"Viewport page-scroll direction."}
                    }), &["target_id", "tab_id", "session"]);
                    schema["oneOf"] = json!([
                        {"required":["ref"],"not":{"required":["direction"]}},
                        {"required":["direction"],"not":{"required":["ref"]}}
                    ]);
                    schema
                },
                read_only: false,
                destructive: false,
                idempotent: false,
                open_world: true,
            },
            engine,
        }
    }

    async fn invoke_inner(&self, args: Value) -> ToolResult {
        let (target_id, tab_id) = match (args.require_str("target_id"), args.require_str("tab_id"))
        {
            (Ok(target), Ok(tab)) => (target, tab),
            (Err(error), _) | (_, Err(error)) => return error,
        };
        let external_ref = args.opt_str("ref");
        let direction = args.opt_str("direction");
        if external_ref.is_some() == direction.is_some() {
            return ToolResult::error("browser_scroll requires exactly one of ref or direction");
        }
        if direction
            .as_deref()
            .is_some_and(|value| !matches!(value, "up" | "down"))
        {
            return ToolResult::error("direction must be \"up\" or \"down\"");
        }
        let session = match explicit_session(&args) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let _mutation = match self
            .engine
            .lock_mutation(&session, &target_id, &tab_id)
            .await
        {
            Ok(guard) => guard,
            Err(refusal) => return refusal.to_tool_result(),
        };
        let validated = match authorized_tab(&self.engine, &session, &target_id, &tab_id).await {
            Ok(value) => value,
            Err(error) => return error,
        };
        if let Some(external_ref) = external_ref {
            let (entry, cdp_session) = match exact_ref(
                &self.engine,
                &session,
                &target_id,
                &tab_id,
                &validated,
                &external_ref,
            )
            .await
            {
                Ok(value) => value,
                Err(error) => return error,
            };
            if validated
                .conn
                .call(
                    Some(&cdp_session),
                    "DOM.scrollIntoViewIfNeeded",
                    json!({"backendNodeId":entry.backend_node_id}),
                )
                .await
                .is_err()
            {
                return stale("the ref's node no longer has live page layout");
            }
            return ToolResult::text(format!("scrolled {external_ref} into view")).with_structured(
                json!({
                    "status":"ok", "target_id":target_id, "tab_id":tab_id, "ref":external_ref,
                    "frame":entry.frame.kind.as_str(), "route":"dom_scroll_into_view"
                }),
            );
        }

        if validated.record.cdp_window_id.is_some()
            && validated.record.product_kind != BrowserProduct::Electron
        {
            if let Some(limitation) = self
                .engine
                .platform
                .standalone_trusted_input_background_limitation()
            {
                return BrowserRefusal::new(
                    BrowserRefusalCode::BrowserInputTrustUnavailable,
                    limitation,
                )
                .to_tool_result();
            }
        }
        let metrics = match validated
            .conn
            .call(
                Some(&validated.cdp_session),
                "Page.getLayoutMetrics",
                json!({}),
            )
            .await
        {
            Ok(value) => value,
            Err(error) => {
                return ToolResult::error(format!("Page.getLayoutMetrics failed: {error}"))
            }
        };
        let viewport = metrics
            .get("cssVisualViewport")
            .or_else(|| metrics.get("visualViewport"));
        let dimensions = viewport
            .and_then(|value| {
                Some((
                    value.get("clientWidth")?.as_f64()?,
                    value.get("clientHeight")?.as_f64()?,
                ))
            })
            .filter(|(width, height)| {
                width.is_finite() && *width > 0.0 && height.is_finite() && *height > 0.0
            });
        let (width, height) = match dimensions {
            Some(value) => value,
            None => {
                return unavailable(
                    "Page.getLayoutMetrics returned no usable visual viewport",
                    "viewport_unavailable",
                )
            }
        };
        let direction = direction.expect("validated exactly one scroll target");
        let delta_y = height * 0.8 * if direction == "up" { -1.0 } else { 1.0 };
        if let Err(error) = validated
            .conn
            .call(
                Some(&validated.cdp_session),
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled":true}),
            )
            .await
        {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserInputTrustUnavailable,
                format!("the tab could not enter trusted focus emulation: {error}"),
            )
            .to_tool_result();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        let dispatch = validated.conn.call(
            Some(&validated.cdp_session),
            "Input.dispatchMouseEvent",
            json!({
                "type":"mouseWheel", "x":width / 2.0, "y":height / 2.0,
                "deltaX":0.0, "deltaY":delta_y
            }),
        );
        let delivery = tokio::time::timeout(Duration::from_secs(2), dispatch).await;
        let cleanup = tokio::time::timeout(
            Duration::from_secs(2),
            validated.conn.call(
                Some(&validated.cdp_session),
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled":false}),
            ),
        )
        .await;
        match (delivery, cleanup) {
            (Ok(Ok(_)), Ok(Ok(_))) => ToolResult::text(format!("scrolled page {direction}"))
                .with_structured(json!({
                    "status":"ok", "target_id":target_id, "tab_id":tab_id,
                    "direction":direction, "delta_y":delta_y, "route":"trusted"
                })),
            (Ok(Err(error)), _) => BrowserRefusal::new(
                BrowserRefusalCode::BrowserInputTrustUnavailable,
                format!("trusted page scroll failed: {error}"),
            )
            .to_tool_result(),
            (Err(_), _) => BrowserRefusal::new(BrowserRefusalCode::BrowserInputTrustUnavailable, "trusted page scroll timed out; delivery is unknown and must not be retried automatically")
                .with_detail(json!({"delivery":"unknown","retryable":false})).to_tool_result(),
            (Ok(Ok(_)), Ok(Err(error))) => BrowserRefusal::new(BrowserRefusalCode::BrowserInputTrustUnavailable, format!("trusted page scroll was acknowledged but focus emulation could not be restored ({error}); delivery is unknown and must not be retried automatically"))
                .with_detail(json!({"delivery":"unknown","retryable":false})).to_tool_result(),
            (Ok(Ok(_)), Err(_)) => BrowserRefusal::new(BrowserRefusalCode::BrowserInputTrustUnavailable, "trusted page scroll was acknowledged but focus-emulation cleanup timed out; delivery is unknown and must not be retried automatically")
                .with_detail(json!({"delivery":"unknown","retryable":false})).to_tool_result(),
        }
    }
}
protected_browser_input!(BrowserScrollTool, "browser_scroll");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_vocabulary_is_closed_and_carries_exact_cdp_metadata() {
        let names = [
            "Enter",
            "NumpadEnter",
            "Space",
            "Backspace",
            "Delete",
            "Tab",
            "Escape",
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
            "Home",
            "End",
            "PageUp",
            "PageDown",
        ];
        for name in names {
            let spec = KeySpec::parse(name).expect("declared key");
            assert_eq!(spec.name, name);
            assert_eq!(spec.params("keyDown")["type"], "keyDown");
            assert_eq!(spec.params("keyUp")["type"], "keyUp");
        }
        assert!(KeySpec::parse("Meta+A").is_none());
        assert!(KeySpec::parse("synthetic text").is_none());
        assert_eq!(
            KeySpec::parse("NumpadEnter").unwrap().params("keyDown")["isKeypad"],
            true
        );
        assert_eq!(
            KeySpec::parse("Space").unwrap().params("keyDown")["text"],
            " "
        );
    }

    #[test]
    fn schemas_reject_open_ended_key_and_scroll_shapes() {
        let schema = schema_ids(
            json!({"key":{"type":"string"}}),
            &["target_id", "tab_id", "session", "key"],
        );
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"].as_array().unwrap().len(), 4);
    }
}
