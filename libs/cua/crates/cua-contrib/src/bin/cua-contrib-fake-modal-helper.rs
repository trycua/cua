//! A stand-in for `cua-modal-helper` in hermetic tests: the same JSON
//! protocol, with sandboxes kept in a state file instead of on Modal. Every
//! tunnel is `CUA_FAKE_MODAL_TUNNEL` (a mock cua-spacesd). Requests are
//! recorded in the state file (never the token values, only whether both
//! were passed). Test-only (feature `testing`).

use serde_json::{Value, json};
use std::io::Read;

fn main() {
    let path = std::env::var("CUA_FAKE_MODAL_STATE").expect("CUA_FAKE_MODAL_STATE");
    let tunnel = std::env::var("CUA_FAKE_MODAL_TUNNEL").unwrap_or_default();
    let mut input = String::new();
    std::io::stdin()
        .take(1 << 20)
        .read_to_string(&mut input)
        .unwrap();
    let req: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let mut state: Value = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({"next": 0, "sandboxes": {}, "requests": []}));
    let tokens = std::env::var("MODAL_TOKEN_ID").is_ok_and(|v| v == "tok-id")
        && std::env::var("MODAL_TOKEN_SECRET").is_ok_and(|v| v == "tok-secret");
    let mut recorded = req.clone();
    recorded["tokens"] = json!(tokens);
    state["requests"].as_array_mut().unwrap().push(recorded);
    let reply = if !tokens {
        json!({"error": {"kind": "auth", "message": "Invalid token id or secret"}})
    } else {
        match req["op"].as_str().unwrap_or_default() {
            "create" => {
                let n = state["next"].as_u64().unwrap_or(0) + 1;
                state["next"] = json!(n);
                let id = format!("sb-{n:06}");
                let tunnels: serde_json::Map<String, Value> = req["ports"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|p| {
                        let port = p.as_u64().unwrap_or_default();
                        let url = if port == 3211 {
                            tunnel.clone()
                        } else {
                            format!("https://{id}-{port}.w.modal.host")
                        };
                        (port.to_string(), json!(url))
                    })
                    .collect();
                let sb =
                    json!({"id": id, "status": "running", "tags": req["tags"], "tunnels": tunnels});
                state["sandboxes"][&id] = sb.clone();
                json!({"sandbox": sb})
            }
            "get" => match state["sandboxes"].get(req["id"].as_str().unwrap_or_default()) {
                Some(sb) => json!({"sandbox": sb}),
                None => json!({"error": {"kind": "not_found", "message": "Sandbox not found"}}),
            },
            "list" => {
                let all: Vec<Value> = state["sandboxes"]
                    .as_object()
                    .map(|m| m.values().cloned().collect())
                    .unwrap_or_default();
                json!({"sandboxes": all})
            }
            "delete" => {
                let id = req["id"].as_str().unwrap_or_default().to_string();
                match state["sandboxes"]
                    .as_object_mut()
                    .and_then(|m| m.remove(&id))
                {
                    Some(_) => json!({}),
                    None => json!({"error": {"kind": "not_found", "message": "Sandbox not found"}}),
                }
            }
            other => {
                json!({"error": {"kind": "invalid", "message": format!("unknown op {other}")}})
            }
        }
    };
    std::fs::write(&path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    println!("{reply}");
    std::process::exit(if reply.get("error").is_some() { 2 } else { 0 });
}
