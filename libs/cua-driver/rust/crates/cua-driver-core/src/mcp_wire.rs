//! Shared legacy and per-request MCP semantics for the stdio adapters.

use serde_json::{json, Value};

use crate::protocol::{initialize_result, Request, Response, ResponseBody};

pub const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";
pub const LEGACY_PROTOCOL_VERSION: &str = "2025-06-18";
pub const PROTOCOL_VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
pub const CLIENT_CAPABILITIES_KEY: &str = "io.modelcontextprotocol/clientCapabilities";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolEra {
    Legacy,
    Modern,
}

/// Only a legacy initialize permits subsequent metadata-free stdio requests.
/// Modern capabilities are validated on each request and are never retained.
#[derive(Debug, Default)]
pub struct ProtocolSession {
    legacy_initialized: bool,
}

impl ProtocolSession {
    pub fn validate(&mut self, req: &Request) -> Result<ProtocolEra, Response> {
        let era = classify_request(req)?;
        if era == ProtocolEra::Legacy {
            if req.method == "initialize" {
                // Preserve the historical permissive initialize params contract.
                self.legacy_initialized = true;
            } else if !self.legacy_initialized {
                return Err(invalid_params(
                    req,
                    "Missing required per-request MCP metadata",
                ));
            }
        }
        Ok(era)
    }
}

fn invalid_params(req: &Request, message: &str) -> Response {
    Response::error(req.id.clone().unwrap_or(Value::Null), -32602, message)
}

fn valid_request_id(req: &Request) -> bool {
    req.id
        .as_ref()
        .is_some_and(|id| id.is_string() || id.as_i64().is_some() || id.as_u64().is_some())
}

/// Classify an individual request, retaining legacy compatibility for embedders.
/// Stdio adapters additionally use `ProtocolSession` to reject absent metadata.
pub fn classify_request(req: &Request) -> Result<ProtocolEra, Response> {
    let meta = req.params.as_ref().and_then(|params| params.get("_meta"));
    let modern_keys_present = meta.is_some_and(|meta| {
        meta.get(PROTOCOL_VERSION_KEY).is_some() || meta.get(CLIENT_CAPABILITIES_KEY).is_some()
    });
    if !modern_keys_present && req.method != "server/discover" {
        if meta.is_some_and(|meta| !meta.is_object()) {
            return Err(invalid_params(req, "MCP _meta must be an object"));
        }
        return Ok(ProtocolEra::Legacy);
    }
    if req.jsonrpc != "2.0" || !valid_request_id(req) {
        return Err(Response::error(
            Value::Null,
            -32600,
            "Invalid JSON-RPC request",
        ));
    }
    if !req.params.as_ref().is_some_and(Value::is_object) {
        return Err(invalid_params(req, "MCP params must be an object"));
    }
    let version = meta
        .and_then(|meta| meta.get(PROTOCOL_VERSION_KEY))
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_params(req, "Missing or invalid MCP protocolVersion metadata"))?;
    if version != MODERN_PROTOCOL_VERSION {
        return Err(Response::error_with_data(
            req.id.clone().unwrap_or(Value::Null),
            -32022,
            "Unsupported protocol version",
            Some(json!({"supported": [MODERN_PROTOCOL_VERSION], "requested": version})),
        ));
    }
    if !meta
        .and_then(|meta| meta.get(CLIENT_CAPABILITIES_KEY))
        .is_some_and(Value::is_object)
    {
        return Err(invalid_params(
            req,
            "Missing or invalid MCP clientCapabilities metadata",
        ));
    }
    Ok(ProtocolEra::Modern)
}

pub fn server_capabilities() -> Value {
    let mut capabilities = crate::mcp_skills::capabilities();
    capabilities["tools"] = json!({});
    capabilities
}

/// Methods that do not require platform tool execution, shared by both adapters.
pub fn handle_metadata_request(req: &Request, id: Value) -> Option<Response> {
    match req.method.as_str() {
        "server/discover" => Some(Response::ok(
            id,
            json!({
                "supportedVersions": [MODERN_PROTOCOL_VERSION],
                "capabilities": server_capabilities(),
                "instructions": initialize_result()["instructions"],
            }),
        )),
        "ping" => Some(Response::ok(id, json!({}))),
        _ => crate::mcp_skills::handle(req, id),
    }
}

/// Add only protocol envelope fields; tool structured content remains untouched.
pub fn finish_response(era: ProtocolEra, method: &str, mut response: Response) -> Response {
    if era == ProtocolEra::Modern {
        if let ResponseBody::Result { result } = &mut response.body {
            if let Some(result) = result.as_object_mut() {
                result.insert("resultType".into(), json!("complete"));
                let meta = result.entry("_meta").or_insert_with(|| json!({}));
                if !meta.is_object() {
                    *meta = json!({});
                }
                meta["io.modelcontextprotocol/serverInfo"] =
                    json!({"name": "cua-driver", "version": env!("CARGO_PKG_VERSION")});
                if method == "server/discover"
                    || method.ends_with("/list")
                    || method.ends_with("/read")
                {
                    result.insert("ttlMs".into(), json!(0));
                    result.insert("cacheScope".into(), json!("private"));
                }
            }
        }
    }
    response
}

/// Reject malformed calls and modern unknown tools before any tool invocation.
pub fn validate_tool_call(
    req: &Request,
    id: Value,
    era: ProtocolEra,
    inventory: &Value,
) -> Result<(), Response> {
    if req.method != "tools/call" {
        return Ok(());
    }
    let call = req
        .tool_call()
        .map_err(|error| Response::error(id.clone(), -32602, format!("Invalid params: {error}")))?;
    if !call.args.is_object() {
        return Err(Response::error(
            id,
            -32602,
            "Tool arguments must be an object",
        ));
    }
    if era == ProtocolEra::Modern
        && !inventory["tools"].as_array().is_some_and(|tools| {
            tools
                .iter()
                .any(|tool| tool["name"].as_str() == Some(call.name.as_str()))
        })
    {
        return Err(Response::error(
            id,
            -32602,
            format!("Unknown tool: {}", call.name),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, meta: Value) -> Request {
        serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": {"_meta": meta}
        }))
        .unwrap()
    }

    fn modern_meta() -> Value {
        json!({PROTOCOL_VERSION_KEY: MODERN_PROTOCOL_VERSION, CLIENT_CAPABILITIES_KEY: {}})
    }

    #[test]
    fn modern_versions_and_capabilities_are_validated_per_request() {
        let mut session = ProtocolSession::default();
        assert_eq!(
            session.validate(&request("ping", modern_meta())).unwrap(),
            ProtocolEra::Modern
        );
        for meta in [
            json!({}),
            json!({PROTOCOL_VERSION_KEY: MODERN_PROTOCOL_VERSION}),
            json!({PROTOCOL_VERSION_KEY: MODERN_PROTOCOL_VERSION, CLIENT_CAPABILITIES_KEY: []}),
        ] {
            let error = serde_json::to_value(session.validate(&request("ping", meta)).unwrap_err())
                .unwrap();
            assert_eq!(error["error"]["code"], -32602);
        }
        let error = serde_json::to_value(
            session
                .validate(&request(
                    "ping",
                    json!({
                        PROTOCOL_VERSION_KEY: "unknown", CLIENT_CAPABILITIES_KEY: {}
                    }),
                ))
                .unwrap_err(),
        )
        .unwrap();
        assert_eq!(error["error"]["code"], -32022);
        assert_eq!(
            error["error"]["data"],
            json!({"supported": [MODERN_PROTOCOL_VERSION], "requested": "unknown"})
        );
    }

    #[test]
    fn only_legacy_initialize_enables_metadata_free_requests() {
        let mut session = ProtocolSession::default();
        let legacy = request("ping", json!({}));
        assert!(session.validate(&legacy).is_err());
        assert_eq!(
            session.validate(&request("initialize", json!({}))).unwrap(),
            ProtocolEra::Legacy
        );
        assert_eq!(session.validate(&legacy).unwrap(), ProtocolEra::Legacy);
        assert!(session
            .validate(&request(
                "ping",
                json!({PROTOCOL_VERSION_KEY: MODERN_PROTOCOL_VERSION})
            ))
            .is_err());
    }

    #[test]
    fn modern_discovery_and_tool_envelopes_follow_canonical_shapes() {
        let req = request("server/discover", modern_meta());
        let response = finish_response(
            ProtocolEra::Modern,
            &req.method,
            handle_metadata_request(&req, json!(1)).unwrap(),
        );
        let result = serde_json::to_value(response).unwrap()["result"].clone();
        assert_eq!(result["resultType"], "complete");
        assert_eq!(
            result["supportedVersions"],
            json!([MODERN_PROTOCOL_VERSION])
        );
        assert_eq!(result["ttlMs"], 0);
        assert_eq!(result["cacheScope"], "private");
        assert_eq!(
            result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "cua-driver"
        );
        assert!(result.get("serverInfo").is_none());
        let structured = json!({"resultType": "domain-value", "private": [1, 2]});
        let response = finish_response(
            ProtocolEra::Modern,
            "tools/call",
            Response::ok(
                json!(1),
                json!({"content": [], "structuredContent": structured}),
            ),
        );
        let result = serde_json::to_value(response).unwrap()["result"].clone();
        assert_eq!(result["structuredContent"], structured);
        assert_eq!(result["resultType"], "complete");
        assert!(result.get("ttlMs").is_none());
    }

    #[test]
    fn legacy_errors_omit_optional_data() {
        let error = serde_json::to_value(Response::error(json!(1), -32602, "invalid")).unwrap();
        assert!(error["error"].get("data").is_none());
    }

    #[test]
    fn modern_invalid_jsonrpc_envelopes_are_rejected() {
        for (jsonrpc, id) in [
            ("1.0", json!(1)),
            ("2.0", json!(true)),
            ("2.0", json!(1.5)),
            ("2.0", json!({})),
        ] {
            let req = serde_json::from_value(json!({
                "jsonrpc": jsonrpc, "id": id, "method": "ping",
                "params": {"_meta": modern_meta()}
            }))
            .unwrap();
            let response = serde_json::to_value(classify_request(&req).unwrap_err()).unwrap();
            assert_eq!(response["error"]["code"], -32600);
            assert!(response["id"].is_null());
        }
        let req = serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": []
        }))
        .unwrap();
        let response = serde_json::to_value(classify_request(&req).unwrap_err()).unwrap();
        assert_eq!(response["error"]["code"], -32602);
    }
}
