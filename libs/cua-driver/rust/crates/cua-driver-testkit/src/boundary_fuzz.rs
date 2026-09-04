//! Fuzz target bodies for the transport-free MCP tool-call boundary.
//!
//! Each public `fn(&[u8])` here is one libFuzzer target (see `fuzz/`) and is
//! also driven by `tests/tool_boundary_fuzz_smoke.rs` over the seed corpus
//! plus a deterministic byte stream. A body must return normally on every
//! input; any panic is a finding. The bodies never reach a platform adapter,
//! display, or input device: every tool behind the boundary is a stub.
//!
//! The chain under test, all inside `cua-driver-core`:
//!
//! ```text
//! bytes -> protocol::Request -> Request::tool_call
//!       -> server::handle_request            (reserved-arg strip, session stamping)
//!       -> ToolRegistry::invoke_with_context  (alias, normalisation, authorization)
//!       -> tool_args::parse_typed_input       (serde into contract input types)
//! ```
//!
//! Environment-driven globals (`CUA_DRIVER_*` policy and permission-mode
//! variables) are deliberately not consulted by `tool_arguments` and
//! `registry_invoke`, which build their authorization context from explicit
//! constructors. `mcp_request` goes through the public `handle_request`
//! entry point, which does read the process defaults once; keep those
//! variables unset when fuzzing so runs stay reproducible.

use arbitrary::Unstructured;
use cua_driver_contract::{
    ClickInput, ClipboardReadInput, ClipboardWriteInput, DragInput, EndSessionInput,
    EscalateSessionInput, GetAgentCursorStateInput, GetCursorPositionInput, GetDesktopStateInput,
    GetScreenSizeInput, GetSessionInput, GetSessionStateInput, HotkeyInput, InvokeMenuInput,
    ListSessionsInput, MoveCursorInput, PressKeyInput, ScrollInput, SetAgentCursorEnabledInput,
    SetAgentCursorMotionInput, SetAgentCursorThemeInput, SetWindowFrameInput, StartSessionInput,
    ToolInput, TypeTextInput, VerifyStateInput,
};
use cua_driver_core::authorization::PermissionMode;
use cua_driver_core::protocol::{Request, Response, ToolResult};
use cua_driver_core::server::{
    handle_request, handle_request_with_transport_session, ToolProvider,
};
use cua_driver_core::session_authorization::{
    EffectiveAuthorizationContext, SessionAuthorizationRegistry, SessionModeCeiling,
};
use cua_driver_core::tool::{Tool, ToolDef, ToolRegistry};
use serde_json::{Map, Value};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Every target with its name, in the order the fuzz crate registers them.
pub const TARGETS: &[(&str, fn(&[u8]))] = &[
    ("mcp_request", mcp_request),
    ("tool_arguments", tool_arguments),
    ("typed_input_json", typed_input_json),
    ("registry_invoke", registry_invoke),
];

/// Reserved arguments the MCP ingress itself is allowed to stamp onto a call.
/// Anything else with a leading underscore reaching a provider was forged by
/// the caller and survived sanitisation.
const TRANSPORT_STAMPED_RESERVED: &[&str] = &[
    "_session_id",
    "_transport_session_id",
    cua_driver_core::browser::download::MCP_HOST_DOWNLOAD_APPROVAL_ARG,
];

const TRANSPORT_SESSION: &str = "fuzz-transport";

/// Argument names the boundary treats specially. Drawing keys from this pool
/// keeps random objects landing on real normalisation and policy branches.
const INTERESTING_KEYS: &[&str] = &[
    "session",
    "pid",
    "window_id",
    "element_index",
    "x",
    "y",
    "text",
    "key",
    "keys",
    "button",
    "target",
    "kind",
    "display_id",
    "scope",
    "delivery_mode",
    "dispatch",
    "count",
    "direction",
    "amount",
    "reason",
    "mode",
    "timeout_ms",
    "profile",
    "strategy",
    "dir",
    "delay_ms",
    "_session_id",
    "_transport_session_id",
    "_observation_only",
    "_protected_process_fingerprint",
    "_cua_browser_download_mcp_host_approved",
];

const INTERESTING_STRINGS: &[&str] = &[
    "",
    "window",
    "desktop",
    "foreground",
    "Foreground",
    "background",
    "left",
    "right",
    "middle",
    "up",
    "down",
    "main",
    "existing_profile",
    "isolated_new",
    "\u{0}",
    "\u{1F600}",
    "implicit-direct",
];

/// Runtime-only tool names that never appear in the published contract but do
/// hit dedicated boundary branches (aliases, host-approval stamping, replay).
const RUNTIME_ONLY_TOOLS: &[&str] = &[
    "type_text_chars",
    "browser_download",
    "browser_prepare",
    "replay_trajectory",
    "get_window_state",
    "list_windows",
    "kill_app",
    "set_value",
    "page",
    "start_recording",
    "revoke_all",
];

fn contract_tool_names() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        cua_driver_contract::manifest()
            .tools
            .into_iter()
            .map(|tool| tool.name)
            .collect()
    })
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("fuzz runtime")
    })
}

/// Unrestricted in-process context built from explicit trusted-host
/// constructors, mirroring the registry dispatch tests in `cua-driver-core`.
fn unrestricted_context() -> Arc<EffectiveAuthorizationContext> {
    static CONTEXT: OnceLock<Arc<EffectiveAuthorizationContext>> = OnceLock::new();
    CONTEXT
        .get_or_init(|| {
            let ceiling = SessionModeCeiling::for_trusted_sessions(
                [PermissionMode::Unrestricted],
                true,
                Duration::from_secs(3600),
                Duration::from_secs(1800),
            )
            .expect("fuzz ceiling");
            SessionAuthorizationRegistry::with_ceiling(ceiling)
                .compatibility_context(PermissionMode::Unrestricted, None)
                .expect("fuzz context")
        })
        .clone()
}

// ── Generators ───────────────────────────────────────────────────────────────

fn pick<'a>(u: &mut Unstructured<'_>, pool: &'a [&str]) -> &'a str {
    u.choose(pool).copied().unwrap_or("")
}

fn arbitrary_string(u: &mut Unstructured<'_>) -> String {
    if u.ratio(3, 4).unwrap_or(true) {
        pick(u, INTERESTING_STRINGS).to_owned()
    } else {
        u.arbitrary::<String>().unwrap_or_default()
    }
}

fn arbitrary_number(u: &mut Unstructured<'_>) -> Value {
    match u.int_in_range(0..=7u8).unwrap_or(0) {
        0 => Value::from(u.arbitrary::<i64>().unwrap_or(0)),
        1 => Value::from(u.arbitrary::<u64>().unwrap_or(0)),
        2 => Value::from(u.int_in_range(0..=64u8).unwrap_or(0)),
        3 => Value::from(u.arbitrary::<f64>().unwrap_or(0.0)),
        4 => Value::from(u32::MAX),
        5 => Value::from(u64::from(u32::MAX) + 1),
        6 => Value::from(-1),
        _ => Value::from(i64::from(std::process::id())),
    }
}

/// Bounded-depth JSON value biased toward the shapes the boundary inspects.
pub fn arbitrary_json(u: &mut Unstructured<'_>, depth: u8) -> Value {
    let choice = u.int_in_range(0..=9u8).unwrap_or(0);
    match choice {
        0 => Value::Null,
        1 => Value::Bool(u.arbitrary().unwrap_or(false)),
        2 | 3 => arbitrary_number(u),
        4 | 5 => Value::String(arbitrary_string(u)),
        6 if depth > 0 => {
            let len = u.int_in_range(0..=4usize).unwrap_or(0);
            Value::Array((0..len).map(|_| arbitrary_json(u, depth - 1)).collect())
        }
        _ if depth > 0 => arbitrary_object(u, depth - 1),
        _ => Value::String(arbitrary_string(u)),
    }
}

fn arbitrary_object(u: &mut Unstructured<'_>, depth: u8) -> Value {
    let len = u.int_in_range(0..=6usize).unwrap_or(0);
    let mut object = Map::new();
    for _ in 0..len {
        let key = if u.ratio(4, 5).unwrap_or(true) {
            pick(u, INTERESTING_KEYS).to_owned()
        } else {
            u.arbitrary::<String>().unwrap_or_default()
        };
        object.insert(key, arbitrary_json(u, depth));
    }
    Value::Object(object)
}

/// Tool arguments: usually an object, sometimes trailing raw JSON text from
/// the input so libFuzzer can splice in real payloads, occasionally a
/// non-object so the "arguments must be an object" branches run.
fn arbitrary_arguments(u: &mut Unstructured<'_>) -> Value {
    match u.int_in_range(0..=5u8).unwrap_or(0) {
        0 => {
            let rest = u.bytes(u.len()).unwrap_or(&[]);
            serde_json::from_slice(rest).unwrap_or_else(|_| Value::Object(Map::new()))
        }
        1 => arbitrary_json(u, 2),
        _ => arbitrary_object(u, 3),
    }
}

fn arbitrary_tool_name(u: &mut Unstructured<'_>) -> String {
    match u.int_in_range(0..=7u8).unwrap_or(0) {
        0 => pick(u, RUNTIME_ONLY_TOOLS).to_owned(),
        1 => u.arbitrary::<String>().unwrap_or_default(),
        _ => u.choose(contract_tool_names()).cloned().unwrap_or_default(),
    }
}

// ── mcp_request ──────────────────────────────────────────────────────────────

#[derive(Default)]
struct RecordingProvider {
    calls: Mutex<Vec<(String, Value)>>,
}

#[async_trait::async_trait]
impl ToolProvider for RecordingProvider {
    fn tools_list(&self) -> Value {
        serde_json::json!({"tools": []})
    }

    async fn invoke_tool(&self, name: &str, arguments: Value) -> Result<Value, String> {
        self.calls
            .lock()
            .unwrap()
            .push((name.to_owned(), arguments));
        Ok(serde_json::json!({"content": [], "isError": false}))
    }
}

fn check_stamped_arguments(
    caller_args: &Value,
    delivered: &Value,
    transport_session: Option<&str>,
) {
    let Some(delivered) = delivered.as_object() else {
        return;
    };
    for key in delivered.keys().filter(|key| key.starts_with('_')) {
        assert!(
            TRANSPORT_STAMPED_RESERVED.contains(&key.as_str()),
            "forged reserved argument {key:?} reached the provider"
        );
    }
    let public_session = caller_args
        .get("session")
        .and_then(Value::as_str)
        .filter(|session| !session.is_empty());
    let expected_session = public_session.or(transport_session);
    assert_eq!(
        delivered.get("_session_id").and_then(Value::as_str),
        expected_session,
        "_session_id must come from the public session or the transport"
    );
    let expected_owner = transport_session.or(public_session);
    assert_eq!(
        delivered
            .get("_transport_session_id")
            .and_then(Value::as_str),
        expected_owner,
        "_transport_session_id must come from the transport or the public session"
    );
}

fn caller_arguments(request: &Request) -> Value {
    request
        .params
        .as_ref()
        .and_then(|params| params.get("arguments"))
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()))
}

fn drive_request(request: Request, provider: &dyn ToolProvider, transport_session: Option<&str>) {
    let id = request.id.clone().unwrap_or(Value::Null);
    let response: Response = runtime().block_on(async {
        match transport_session {
            Some(session) => {
                handle_request_with_transport_session(request, id, provider, session).await
            }
            None => handle_request(request, id, provider).await,
        }
    });
    let encoded = serde_json::to_value(&response).expect("response serialises");
    assert_eq!(encoded["jsonrpc"], "2.0");
}

/// Ingress with a recording provider, so the arguments handed past the
/// transport boundary can be checked for forged reserved fields.
fn drive_recorded(data: &[u8], transport_session: Option<&str>) {
    let Ok(request) = serde_json::from_slice::<Request>(data) else {
        return;
    };
    let is_tool_call = request.method == "tools/call";
    let caller_args = caller_arguments(&request);
    let provider = RecordingProvider::default();
    drive_request(request, &provider, transport_session);
    if is_tool_call {
        for (_, delivered) in provider.calls.lock().unwrap().iter() {
            check_stamped_arguments(&caller_args, delivered, transport_session);
        }
    }
}

/// Raw bytes as a JSON-RPC request through both public ingress entry points,
/// first against a recording provider and then end to end into the stub
/// `ToolRegistry`, which is the path a real MCP client's call takes.
pub fn mcp_request(data: &[u8]) {
    let Ok(request) = serde_json::from_slice::<Request>(data) else {
        return;
    };
    let _ = request.is_notification();
    let _ = request.initialize_metadata();
    let _ = request.tool_call();
    drive_request(request, stub_registry().as_ref(), Some(TRANSPORT_SESSION));
    drive_recorded(data, None);
    drive_recorded(data, Some(TRANSPORT_SESSION));
}

// ── typed inputs ─────────────────────────────────────────────────────────────

fn sanitized(args: &Value) -> Value {
    let mut copy = args.clone();
    cua_driver_core::tool_args::sanitize_reserved_args(&mut copy);
    copy
}

fn input_schema_validator<T: ToolInput>() -> &'static jsonschema::Validator {
    // One validator per input type. A `static` inside a generic fn is shared
    // across every `T`, so key the cache by tool name instead of relying on
    // monomorphisation.
    static VALIDATORS: OnceLock<Mutex<Vec<(&'static str, &'static jsonschema::Validator)>>> =
        OnceLock::new();
    let validators = VALIDATORS.get_or_init(|| Mutex::new(Vec::new()));
    let mut guard = validators.lock().unwrap();
    if let Some((_, validator)) = guard.iter().find(|(name, _)| *name == T::TOOL_NAME) {
        return validator;
    }
    let validator = jsonschema::validator_for(&T::input_schema()).expect("valid input schema");
    let validator: &'static jsonschema::Validator = Box::leak(Box::new(validator));
    guard.push((T::TOOL_NAME, validator));
    validator
}

/// Numeric range and length keywords. The published schemas advertise these
/// (`minimum: 1` on pids, `minItems` on key lists, `maxLength` on labels) but
/// the contract types enforce only some of them; the rest are checked by the
/// platform runtimes after typed parsing. Until every bound is enforced in the
/// parser, treat them as advertised-but-runtime-enforced and keep the fuzz
/// invariant on structural agreement (types, required keys, enums, unknown
/// properties). See the design note for the list of affected fields.
fn is_value_bound_error(error: &jsonschema::ValidationError<'_>) -> bool {
    use jsonschema::error::ValidationErrorKind as Kind;
    matches!(
        error.kind(),
        Kind::Minimum { .. }
            | Kind::Maximum { .. }
            | Kind::ExclusiveMinimum { .. }
            | Kind::ExclusiveMaximum { .. }
            | Kind::MinLength { .. }
            | Kind::MaxLength { .. }
            | Kind::MinItems { .. }
            | Kind::MaxItems { .. }
    )
}

/// Offer `args` to one typed input. When the parser accepts the value, its
/// canonical re-serialised form must validate structurally against the
/// published schema and re-serialising must be idempotent, so SDK clients and
/// the live parser agree on the canonical form.
///
/// The raw input is deliberately not validated: serde is allowed to be more
/// lenient than the schema (an explicit `null` for an optional field, unknown
/// keys on types without `deny_unknown_fields`). What matters is that every
/// value the driver normalises to is one the contract advertises.
fn check_typed_input<T: ToolInput>(args: &Value) {
    let Ok(parsed) = cua_driver_core::tool_args::parse_typed_input::<T>(T::TOOL_NAME, args.clone())
    else {
        return;
    };
    let once = serde_json::to_value(&parsed).expect("typed input serialises");
    let shape_errors: Vec<String> = input_schema_validator::<T>()
        .iter_errors(&once)
        .filter(|error| !is_value_bound_error(error))
        .map(|error| error.to_string())
        .collect();
    assert!(
        shape_errors.is_empty(),
        "{}: parser accepted {} and canonicalised it to {once}, which the published input schema rejects: {shape_errors:?}",
        T::TOOL_NAME,
        sanitized(args)
    );
    let reparsed: T = serde_json::from_value(once.clone()).unwrap_or_else(|error| {
        panic!(
            "{}: serialised typed input {once} does not parse again: {error}",
            T::TOOL_NAME
        )
    });
    let twice = serde_json::to_value(&reparsed).expect("typed input serialises");
    assert_eq!(
        once,
        twice,
        "{}: typed input normalisation is not idempotent",
        T::TOOL_NAME
    );
    let _ = cua_driver_core::tool_args::parse_typed_projection::<T>(T::TOOL_NAME, args);
}

macro_rules! typed_inputs {
    ($($ty:ty),* $(,)?) => {
        fn check_typed_input_for_tool(tool: &str, args: &Value) {
            $(if tool == <$ty as ToolInput>::TOOL_NAME { check_typed_input::<$ty>(args); })*
        }

        fn check_every_typed_input(args: &Value) {
            $(check_typed_input::<$ty>(args);)*
        }
    };
}

typed_inputs!(
    StartSessionInput,
    EscalateSessionInput,
    GetSessionStateInput,
    GetSessionInput,
    ListSessionsInput,
    SetAgentCursorEnabledInput,
    SetAgentCursorMotionInput,
    SetAgentCursorThemeInput,
    GetAgentCursorStateInput,
    EndSessionInput,
    GetDesktopStateInput,
    GetScreenSizeInput,
    GetCursorPositionInput,
    SetWindowFrameInput,
    InvokeMenuInput,
    MoveCursorInput,
    ClickInput,
    DragInput,
    ScrollInput,
    TypeTextInput,
    ClipboardReadInput,
    ClipboardWriteInput,
    PressKeyInput,
    HotkeyInput,
    VerifyStateInput,
);

/// Raw bytes as JSON text offered to every typed contract input.
pub fn typed_input_json(data: &[u8]) {
    let Ok(value) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    check_every_typed_input(&value);
}

// ── tool_arguments ───────────────────────────────────────────────────────────

/// A tool name plus generated arguments through every pure pre-dispatch step.
pub fn tool_arguments(data: &[u8]) {
    let mut u = Unstructured::new(data);
    let tool = arbitrary_tool_name(&mut u);
    let args = arbitrary_arguments(&mut u);

    let clean = sanitized(&args);
    if let Some(object) = clean.as_object() {
        assert!(
            object.keys().all(|key| !key.starts_with('_')),
            "sanitize_reserved_args left a reserved key in {clean}"
        );
    }

    let mut normalised = clean.clone();
    if cua_driver_core::action_target::normalize_action_target(&tool, &mut normalised).is_ok() {
        if let Some(object) = normalised.as_object() {
            assert!(
                !object.contains_key("target"),
                "normalize_action_target accepted {clean} but left a target key"
            );
        }
    }
    let _ = cua_driver_core::action_target::supports_typed_target(&tool);

    let context = unrestricted_context();
    let _ = cua_driver_core::authorization::authorize_tool_call_with_context(
        &tool,
        &normalised,
        context.as_ref(),
    );
    let _ = cua_driver_core::authorization::classify_tool_call(&tool, &normalised);
    let _ = cua_driver_core::tool::advertised_capabilities_for(&tool, &Value::Null);

    check_typed_input_for_tool(&tool, &args);
}

// ── registry_invoke ──────────────────────────────────────────────────────────

struct StubTool {
    def: ToolDef,
}

#[async_trait::async_trait]
impl Tool for StubTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        ToolResult::text("stub")
    }
}

/// One registry of stubs named after every published contract tool. Session
/// lifecycle tools are intentionally absent so no input can suspend the shared
/// runtime scope and starve later inputs.
fn stub_registry() -> Arc<ToolRegistry> {
    static REGISTRY: OnceLock<Arc<ToolRegistry>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| {
            let mut registry = ToolRegistry::new();
            for contract in cua_driver_contract::manifest().tools {
                // Not `ToolDef::from_contract`: that bridge refuses portable
                // subset contracts, and stubs only need the advertised schema
                // so delivery-mode and capability logic sees realistic input.
                registry.register(Box::new(StubTool {
                    def: ToolDef {
                        name: contract.name.clone(),
                        description: contract.description.clone(),
                        input_schema: contract.input_schema.clone(),
                        read_only: contract.annotations.read_only,
                        destructive: contract.annotations.destructive,
                        idempotent: contract.annotations.idempotent,
                        open_world: contract.annotations.open_world,
                    },
                }));
            }
            let registry = Arc::new(registry);
            registry.init_self_weak();
            registry
        })
        .clone()
}

/// A tool name plus generated arguments through `ToolRegistry` dispatch.
pub fn registry_invoke(data: &[u8]) {
    let mut u = Unstructured::new(data);
    let tool = arbitrary_tool_name(&mut u);
    let args = arbitrary_arguments(&mut u);
    let registry = stub_registry();
    let known = registry.get_def(&tool).is_some() || tool == "type_text_chars";

    let result =
        runtime().block_on(registry.invoke_with_context(&tool, args, unrestricted_context()));
    let _ = serde_json::to_value(&result).expect("tool result serialises");
    if !known {
        assert_eq!(
            result.is_error,
            Some(true),
            "unregistered tool {tool:?} did not produce an error result"
        );
    }
}
