//! Read-only transport for the canonical, compiled-in Cua Driver skill pack.
//!
//! Implements SEP-2640 at d6b31a03504c15677d49b922b6b6ace0ef65728d.
//! Reading these resources neither activates a skill nor grants execution authority.

use std::sync::OnceLock;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::protocol::{Request, Response};

const SKILL_URI: &str = "skill://cua-driver/SKILL.md";
const URI_PREFIX: &str = "skill://cua-driver/";

// This is the entire resource namespace. Never resolve a client URI on disk.
const FILES: &[(&str, &str)] = &[
    (
        "BROWSER.md",
        include_str!("../../../Skills/cua-driver/BROWSER.md"),
    ),
    (
        "EMBEDDING.md",
        include_str!("../../../Skills/cua-driver/EMBEDDING.md"),
    ),
    (
        "LINUX.md",
        include_str!("../../../Skills/cua-driver/LINUX.md"),
    ),
    (
        "MACOS.md",
        include_str!("../../../Skills/cua-driver/MACOS.md"),
    ),
    (
        "README.md",
        include_str!("../../../Skills/cua-driver/README.md"),
    ),
    (
        "RECORDING.md",
        include_str!("../../../Skills/cua-driver/RECORDING.md"),
    ),
    (
        "SKILL.md",
        include_str!("../../../Skills/cua-driver/SKILL.md"),
    ),
    (
        "WINDOWS.md",
        include_str!("../../../Skills/cua-driver/WINDOWS.md"),
    ),
];

struct Pack {
    entry: Value,
    resources: Vec<Value>,
}

fn parse_frontmatter(source: &str) -> Result<Value, String> {
    let mut lines = source.lines();
    if lines.next() != Some("---") {
        return Err("skill must start with YAML frontmatter".into());
    }
    let mut yaml = String::new();
    for line in lines {
        if line == "---" {
            let value: Value = serde_yaml_ng::from_str(&yaml).map_err(|e| e.to_string())?;
            if !value.is_object()
                || value.get("name").and_then(Value::as_str) != Some("cua-driver")
                || value.get("description").and_then(Value::as_str).is_none()
            {
                return Err("skill frontmatter must describe cua-driver".into());
            }
            return Ok(value);
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    Err("skill frontmatter is not terminated".into())
}

fn pack() -> &'static Pack {
    static PACK: OnceLock<Pack> = OnceLock::new();
    PACK.get_or_init(|| {
        let frontmatter = parse_frontmatter(include_str!("../../../Skills/cua-driver/SKILL.md"))
            .expect("embedded canonical skill must have valid JSON-compatible frontmatter");
        let mut manifest = Vec::with_capacity(FILES.len());
        let mut resources = Vec::with_capacity(FILES.len());
        for (name, text) in FILES {
            let uri = format!("{URI_PREFIX}{name}");
            manifest.push(json!({
                "uri": uri,
                "digest": format!("sha256:{:x}", Sha256::digest(text.as_bytes())),
                "size": text.len(),
            }));
            let mut resource = json!({
                "uri": uri,
                "name": name,
                "mimeType": "text/markdown",
                "size": text.len(),
            });
            if *name == "SKILL.md" {
                resource["name"] = frontmatter["name"].clone();
                resource["description"] = frontmatter["description"].clone();
            }
            resources.push(resource);
        }
        Pack {
            entry: json!({"uri": SKILL_URI, "frontmatter": frontmatter, "resources": manifest}),
            resources,
        }
    })
}

/// Capabilities shared by every MCP transport serving this pack.
pub fn capabilities() -> Value {
    json!({
        "tools": {},
        "resources": {},
        "extensions": {"io.modelcontextprotocol/skills": {}},
    })
}

fn params(request: &Request) -> Result<Option<&Map<String, Value>>, &'static str> {
    match &request.params {
        None => Ok(None),
        Some(Value::Object(params)) => Ok(Some(params)),
        Some(_) => Err("params must be an object"),
    }
}

fn required_uri(params: Option<&Map<String, Value>>) -> Result<&str, &'static str> {
    params
        .and_then(|params| params.get("uri"))
        .and_then(Value::as_str)
        .ok_or("uri must be a string")
}

fn result(request: &Request) -> Result<Value, &'static str> {
    let params = params(request)?;
    // A complete, single-page catalog has no valid continuation cursor.
    if params.is_some_and(|params| params.contains_key("cursor")) {
        return Err("this resource catalog has no continuation cursor");
    }
    match request.method.as_str() {
        "skills/list" => Ok(json!({"skills": [&pack().entry]})),
        "skills/get" => {
            if required_uri(params)? != SKILL_URI {
                return Err("unknown skill URI");
            }
            Ok(json!({"skill": &pack().entry}))
        }
        "resources/list" => Ok(json!({"resources": &pack().resources})),
        "resources/read" => {
            let uri = required_uri(params)?;
            let name = uri.strip_prefix(URI_PREFIX).ok_or("unknown resource URI")?;
            let (_, text) = FILES
                .iter()
                .find(|(file, _)| *file == name)
                .ok_or("unknown resource URI")?;
            Ok(json!({"contents": [{"uri": uri, "mimeType": "text/markdown", "text": text}]}))
        }
        _ => unreachable!("handle filters methods"),
    }
}

/// Handle skill/resource methods; the transport adds version-specific envelopes.
pub fn handle(request: &Request, id: Value) -> Option<Response> {
    if !matches!(
        request.method.as_str(),
        "skills/list" | "skills/get" | "resources/list" | "resources/read"
    ) {
        return None;
    }
    Some(match result(request) {
        Ok(result) => Response::ok(id, result),
        Err(message) => Response::error(id, -32602, message),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ResponseBody;

    fn request(method: &str, params: Option<Value>) -> Request {
        Request {
            jsonrpc: "2.0".into(),
            id: Some(json!(42)),
            method: method.into(),
            params,
        }
    }

    fn call(method: &str, params: Option<Value>) -> Value {
        let response = handle(&request(method, params), json!(42)).unwrap();
        assert_eq!(response.id, json!(42));
        match response.body {
            ResponseBody::Result { result } => result,
            ResponseBody::Error { error } => panic!("unexpected error: {error:?}"),
        }
    }

    fn invalid(method: &str, params: Option<Value>) {
        match handle(&request(method, params), json!(42)).unwrap().body {
            ResponseBody::Error { error } => assert_eq!(error.code, -32602),
            ResponseBody::Result { result } => panic!("unexpected success: {result}"),
        }
    }

    #[test]
    fn manifest_matches_every_raw_embedded_resource() {
        let listed = call("skills/list", None);
        assert_eq!(listed["skills"].as_array().unwrap().len(), 1);
        assert!(listed.get("resultType").is_none());
        assert!(listed.get("ttlMs").is_none());
        assert!(listed.get("cacheScope").is_none());
        let entry = &listed["skills"][0];
        assert_eq!(entry["uri"], SKILL_URI);
        assert_eq!(
            call("skills/get", Some(json!({"uri": SKILL_URI})))["skill"],
            *entry
        );
        let manifest = entry["resources"].as_array().unwrap();
        let resources = call("resources/list", Some(json!({})));
        assert_eq!(manifest.len(), 8);
        assert_eq!(
            resources["resources"].as_array().unwrap().len(),
            manifest.len()
        );
        for (index, (name, text)) in FILES.iter().enumerate() {
            let uri = format!("{URI_PREFIX}{name}");
            assert_eq!(manifest[index]["uri"], uri);
            assert_eq!(manifest[index]["size"], text.len());
            assert_eq!(
                manifest[index]["digest"],
                format!("sha256:{:x}", Sha256::digest(text.as_bytes()))
            );
            assert_eq!(resources["resources"][index]["uri"], uri);
            let read = call("resources/read", Some(json!({"uri": uri})));
            assert_eq!(read["contents"].as_array().unwrap().len(), 1);
            assert_eq!(read["contents"][0]["mimeType"], "text/markdown");
            assert_eq!(
                read["contents"][0]["text"].as_str().unwrap().as_bytes(),
                text.as_bytes()
            );
        }
    }

    #[test]
    fn full_frontmatter_preserves_nested_metadata_and_scalar_types() {
        let source = include_str!("../../../Skills/cua-driver/SKILL.md");
        let yaml = source.splitn(3, "---").nth(1).unwrap();
        let expected: Value = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(pack().entry["frontmatter"], expected);
        assert!(expected["version"].is_string());
        assert_eq!(
            expected["metadata"]["openclaw"]["envVars"][0]["required"],
            false
        );
        let extended = "---\nname: cua-driver\ndescription: |\n  A multiline\n  description.\nversion: '1.2.3'\nfuture: {enabled: true, count: 7, values: [null, 1.5, text]}\n---\nbody";
        let parsed = parse_frontmatter(extended).unwrap();
        assert_eq!(parsed["description"], "A multiline\ndescription.\n");
        assert_eq!(
            parsed["future"],
            json!({"enabled": true, "count": 7, "values": [null, 1.5, "text"]})
        );
        assert!(parse_frontmatter("---\nname: cua-driver\n").is_err());
    }

    #[test]
    fn uri_allowlist_rejects_aliases_traversal_and_foreign_resources() {
        for uri in [
            "skill://cua-driver",
            "skill://cua-driver/",
            "skill://cua-driver/unknown.md",
            "skill://cua-driver/../SKILL.md",
            "skill://cua-driver/./SKILL.md",
            "skill://cua-driver/%53KILL.md",
            "skill://cua-driver/%2e%2e/SKILL.md",
            "skill://cua-driver/SKILL.md?query=1",
            "skill://cua-driver/SKILL.md#fragment",
            "skill://cua-driver/SKILL.md/",
            "skill://cua-driver//SKILL.md",
            "skill://cua-driver/skill.md",
            "skill://CUA-DRIVER/SKILL.md",
            "skill://foreign/SKILL.md",
            "file:///etc/passwd",
            "https://cua.ai/SKILL.md",
            "skill://cua-driver/..\\SKILL.md",
            "skill://cua-driver/SKILL.md\0",
        ] {
            invalid("resources/read", Some(json!({"uri": uri})));
            invalid("skills/get", Some(json!({"uri": uri})));
        }
        invalid(
            "skills/get",
            Some(json!({"uri": "skill://cua-driver/BROWSER.md"})),
        );
    }

    #[test]
    fn malformed_params_and_cursors_are_invalid() {
        for method in [
            "skills/list",
            "skills/get",
            "resources/list",
            "resources/read",
        ] {
            for params in [json!(null), json!(1), json!([]), json!("invalid")] {
                invalid(method, Some(params));
            }
            for cursor in [json!("next"), json!(""), json!(null), json!(5)] {
                invalid(method, Some(json!({"uri": SKILL_URI, "cursor": cursor})));
            }
        }
        for method in ["skills/get", "resources/read"] {
            invalid(method, None);
            invalid(method, Some(json!({})));
            invalid(method, Some(json!({"uri": 7})));
        }
    }

    #[test]
    fn only_supported_methods_and_capabilities_are_exposed() {
        assert_eq!(
            capabilities(),
            json!({"tools": {}, "resources": {}, "extensions": {"io.modelcontextprotocol/skills": {}}})
        );
        for method in [
            "tools/call",
            "resources/directory/read",
            "skills/execute",
            "resources/templates/list",
        ] {
            assert!(handle(&request(method, None), json!(42)).is_none());
        }
    }
}
