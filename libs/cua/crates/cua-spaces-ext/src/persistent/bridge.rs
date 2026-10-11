// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The host side of a persistent agent's bridge.
//!
//! A run started with the bridge gets the stdio MCP server `cua`: a byte
//! pipe (`runner/bridge.mjs`) that appends what the harness writes to
//! `<run>/bridge/<id>.in` and copies `<run>/bridge/<id>.out` back. The
//! supervisor reads new complete lines of each `.in` over the Space's
//! spacesd channel, hands them to an MCP server that acts as this agent,
//! and appends each answer to `.out`. So the agent reaches the host without
//! a token or a network path into it, and every call is attributed to the
//! agent that owns the run.
//!
//! The agent's tools:
//!
//! | Tool | What |
//! |---|---|
//! | `notify_user` | a notification in the Cua app ("Your research is ready") |
//! | `volume_ls`, `volume_read`, `volume_write`, `volume_request_access`, `volume_sync_status` | the Cua Volume, as `agent:<name>` in its Space (sync as this host sees it) |
//! | `computer_list`, `computer_list_tools`, `computer_call` | the user's computers this agent was granted by name |
//! | `request_site_login` | sign in to a site with a password from the user's Keyvault, after the user approves; the agent never sees it |

use crate::SpacesDrive as _;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cua_spacesd_client::{DownloadOptions, SpacesdClient, UploadOptions, pb};
use serde_json::{Map, Value, json};

use super::{AgentRecord, Persistent};
use cua_spaces::mcp::{McpServer, ToolExtension, ToolOutcome};
use cua_spaces::{Error, Result, Spaces};

/// Largest unanswered line buffered per session.
const MAX_LINE: usize = 16 * 1024 * 1024;

/// Server instructions the agent sees.
pub const INSTRUCTIONS: &str = "Cua: tools for a persistent agent. notify_user tells your user something in the Cua app (use it for results they asked for, or when you need them). The Cua Volume keeps files beyond this Space: your home is agents/<you>/ (your memory lives there too), public/ is shared reference (read only), spaces/<this space>/ is this Space's folder; anything else needs volume_request_access, which the user approves. computer_list shows the user's own computers you were allowed to use; computer_list_tools and computer_call use them. Never put secrets in the drive: the user's Keyvault holds them.";

/// Per-run pipe state kept by the supervisor.
#[derive(Default)]
pub struct BridgeState {
    offsets: HashMap<String, u64>,
    partial: HashMap<String, Vec<u8>>,
    writers: HashMap<String, Arc<tokio::sync::Mutex<()>>>,
    inflight: HashMap<String, Arc<AtomicUsize>>,
}

/// The MCP server that answers `rec`'s bridge.
pub fn server(
    spaces: &Spaces,
    persistent: Persistent,
    rec: &AgentRecord,
    run_id: &str,
) -> McpServer {
    McpServer::new(spaces.clone())
        .with_filter(Arc::new(|_| false))
        .with_name("cua-agent")
        .with_instructions(INSTRUCTIONS)
        .with_extension(Arc::new(AgentTools {
            persistent,
            agent: rec.name.clone(),
            space: rec.space.clone(),
            run_id: run_id.to_string(),
        }))
}

/// Reads every session's new requests and answers them (each answer is
/// written by its own task, so a slow call never holds up the others or
/// the supervisor). Returns the number of requests dispatched.
pub async fn pump(
    guest: &SpacesdClient,
    run_dir: &str,
    state: &mut BridgeState,
    server: &McpServer,
) -> Result<usize> {
    let dir = format!("{run_dir}/{}", cua_spaces::agents::runs::BRIDGE_DIR);
    let entries = match guest.list_dir(&dir, 1).await {
        Ok(e) => e,
        Err(cua_spacesd_client::Error::PathNotFound(_)) => return Ok(0),
        Err(e) => return Err(Error::Env(e)),
    };
    let names: std::collections::HashSet<String> = entries.iter().map(|e| e.name.clone()).collect();
    let mut dispatched = 0;
    for e in &entries {
        let Some(id) = e.name.strip_suffix(".in") else {
            continue;
        };
        let offset = *state.offsets.get(id).unwrap_or(&0);
        if e.size > offset {
            let mut buf = Vec::new();
            guest
                .download_with(
                    &e.path,
                    DownloadOptions {
                        offset,
                        length: e.size - offset,
                        ..Default::default()
                    },
                    &mut buf,
                )
                .await
                .map_err(Error::Env)?;
            state
                .offsets
                .insert(id.to_string(), offset + buf.len() as u64);
            let pending = state.partial.entry(id.to_string()).or_default();
            pending.extend_from_slice(&buf);
            let Some(last_nl) = pending.iter().rposition(|b| *b == b'\n') else {
                if pending.len() > MAX_LINE {
                    pending.clear();
                }
                continue;
            };
            let complete: Vec<u8> = pending.drain(..=last_nl).collect();
            let out_path = format!("{dir}/{id}.out");
            let writer = state
                .writers
                .entry(id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone();
            let inflight = state
                .inflight
                .entry(id.to_string())
                .or_insert_with(|| Arc::new(AtomicUsize::new(0)))
                .clone();
            for line in complete.split(|b| *b == b'\n') {
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                dispatched += 1;
                let message: Value = match serde_json::from_slice(line) {
                    Ok(v) => v,
                    Err(e) => json!({"jsonrpc": "2.0", "id": null, "__parse_error": e.to_string()}),
                };
                let (server, guest, out_path, writer, inflight) = (
                    server.clone(),
                    guest.clone(),
                    out_path.clone(),
                    writer.clone(),
                    inflight.clone(),
                );
                inflight.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let reply = match message.get("__parse_error").and_then(Value::as_str) {
                        Some(e) => Some(json!({"jsonrpc": "2.0", "id": null,
                            "error": {"code": cua_spaces::mcp::codes::PARSE_ERROR, "message": e}})),
                        None => server.handle(message).await,
                    };
                    if let Some(reply) = reply {
                        let mut bytes = serde_json::to_vec(&reply).unwrap_or_default();
                        bytes.push(b'\n');
                        let _w = writer.lock().await;
                        if let Err(e) = guest
                            .upload(
                                &out_path,
                                bytes,
                                UploadOptions {
                                    mode: pb::WriteMode::Append,
                                    permissions: 0o600,
                                    ..Default::default()
                                },
                            )
                            .await
                        {
                            tracing::warn!("bridge: could not answer {out_path}: {e}");
                        }
                    }
                    inflight.fetch_sub(1, Ordering::SeqCst);
                });
            }
        }
        // A session whose pipe closed and whose answers are all written is
        // cleaned up.
        let closed = names.contains(&format!("{id}.closed"));
        let idle = state
            .inflight
            .get(id)
            .is_none_or(|n| n.load(Ordering::SeqCst) == 0);
        if closed && idle && state.offsets.get(id).copied().unwrap_or(0) >= e.size {
            for ext in ["in", "out", "closed"] {
                let _ = guest.remove(&format!("{dir}/{id}.{ext}"), false).await;
            }
            state.offsets.remove(id);
            state.partial.remove(id);
            state.writers.remove(id);
            state.inflight.remove(id);
        }
    }
    Ok(dispatched)
}

/// The tools a persistent agent gets through its bridge.
struct AgentTools {
    persistent: Persistent,
    agent: String,
    space: String,
    run_id: String,
}

fn schema(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

fn tool(name: &str, description: &str, input: Value, read_only: bool) -> Value {
    json!({"name": name, "description": description, "inputSchema": input,
           "annotations": {"readOnlyHint": read_only, "destructiveHint": !read_only,
                           "idempotentHint": read_only, "openWorldHint": true}})
}

fn str_arg<'a>(a: &'a Value, k: &str) -> Result<&'a str> {
    a.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid(format!("`{k}` is required")))
}

impl AgentTools {
    async fn call(&self, tool: &str, a: Value) -> Result<ToolOutcome> {
        match tool {
            "notify_user" => {
                let title = str_arg(&a, "title")?;
                let body = a.get("body").and_then(Value::as_str).unwrap_or("");
                let n = self.persistent.feed().post(
                    Some(&self.agent),
                    "message",
                    title,
                    body,
                    Some(&self.run_id),
                    Some(&self.space),
                )?;
                Ok(ToolOutcome::json(&json!({"notified": true, "id": n.id})))
            }
            "volume_ls" | "volume_read" | "volume_write" => {
                let feed = self.persistent.spaces.drive_feed();
                crate::drive_tools::drive_tool_synced(
                    &self.persistent.drive,
                    feed.as_deref(),
                    cua_volume::Context::agent(&self.agent, Some(&self.space)),
                    tool,
                    a,
                )
                .await
            }
            "volume_sync_status" => Ok(ToolOutcome::json(
                &crate::drive_tools::sync_status_for(
                    &self.persistent.spaces,
                    &cua_volume::Context::agent(&self.agent, Some(&self.space)),
                )
                .await?,
            )),
            "volume_request_access" => {
                let prefix = a
                    .get("prefix")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let reason = a
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let write = a.get("mode").and_then(Value::as_str) == Some("rw");
                let out = crate::drive_tools::drive_tool(
                    &self.persistent.drive,
                    cua_volume::Context::agent(&self.agent, Some(&self.space)),
                    tool,
                    a,
                )
                .await?;
                if !out.is_error {
                    self.persistent.feed().post(
                        Some(&self.agent),
                        "approval",
                        &format!(
                            "{} asks to {} {prefix}",
                            self.agent,
                            if write { "write" } else { "read" }
                        ),
                        &reason,
                        Some(&self.run_id),
                        Some(&self.space),
                    )?;
                }
                Ok(out)
            }
            "computer_list" => {
                let now = cua_volume::now_ms();
                let machines: Vec<String> = self
                    .persistent
                    .access()
                    .grants()?
                    .into_iter()
                    .filter(|g| g.agent == self.agent && g.is_live(now))
                    .map(|g| g.machine)
                    .collect();
                Ok(ToolOutcome::json(&json!({"machines": machines})))
            }
            "computer_list_tools" => {
                let machine = str_arg(&a, "machine")?;
                self.persistent
                    .access()
                    .check(&self.agent, machine, "list_tools")?;
                let s = self.persistent.spaces.space(machine).await?;
                let (tools, service) = s
                    .list_tools(a.get("service").and_then(Value::as_str))
                    .await?;
                let filter = a.get("name").and_then(Value::as_str);
                let tools: Vec<_> = tools
                    .into_iter()
                    .filter(|t| filter.is_none_or(|f| t.name.contains(f)))
                    .collect();
                Ok(ToolOutcome::json(
                    &json!({"machine": machine, "service": service, "tools": tools}),
                ))
            }
            "computer_call" => {
                let machine = str_arg(&a, "machine")?;
                let name = str_arg(&a, "tool")?;
                self.persistent.access().check(
                    &self.agent,
                    machine,
                    &format!("call_tool {name}"),
                )?;
                let s = self.persistent.spaces.space(machine).await?;
                let args: Map<String, Value> = a
                    .get("arguments")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let r = s
                    .call_tool(a.get("service").and_then(Value::as_str), name, args, None)
                    .await?;
                Ok(ToolOutcome {
                    content: r.content,
                    structured: r.structured.filter(Value::is_object),
                    is_error: r.is_error,
                    meta: r.meta,
                })
            }
            "request_site_login" => {
                let mut a = a;
                if let Some(o) = a.as_object_mut() {
                    // The agent's own Space unless it names another; always
                    // as this agent (it cannot sign in as another one).
                    o.entry("space").or_insert(json!(self.space));
                    o.remove("agent");
                }
                let args: cua_spaces_contract::inputs::RequestSiteLogin = serde_json::from_value(a)
                    .map_err(|e| Error::invalid(format!("request_site_login: {e}")))?;
                let v = cua_spaces::site_login::request_site_login(
                    &self.persistent.spaces,
                    args,
                    Some(self.agent.clone()),
                )
                .await?;
                if v.get("status").and_then(Value::as_str) == Some("pending") {
                    self.persistent.feed().post(
                        Some(&self.agent),
                        "approval",
                        &format!("{} asks to sign in", self.agent),
                        "Approve or deny it in Cua.",
                        Some(&self.run_id),
                        Some(&self.space),
                    )?;
                }
                Ok(ToolOutcome::json(&v))
            }
            other => Err(Error::NotFound(format!("tool {other}"))),
        }
    }
}

const NAMES: &[&str] = &[
    "notify_user",
    "volume_ls",
    "volume_read",
    "volume_write",
    "volume_request_access",
    "volume_sync_status",
    "computer_list",
    "computer_list_tools",
    "computer_call",
    "request_site_login",
];

#[async_trait::async_trait]
impl ToolExtension for AgentTools {
    fn tools(&self) -> Vec<Value> {
        vec![
            tool(
                "notify_user",
                "Tell your user something in the Cua app, as a notification (for example \"Your research is ready\"). Use it for results they asked for or when you need them; not for progress chatter.",
                schema(
                    json!({"title": {"type": "string", "description": "One short line."},
                              "body": {"type": "string", "description": "The details (optional)."}}),
                    &["title"],
                ),
                false,
            ),
            tool(
                "volume_ls",
                "List a folder of the Cua Volume (what you may see only). Your home is agents/<you>/, public/ is shared reference, spaces/<this space>/ is this Space's folder.",
                schema(
                    json!({"path": {"type": "string", "description": "Folder. Default: the root."}}),
                    &[],
                ),
                true,
            ),
            tool(
                "volume_read",
                "Read a file from the Cua Volume. Text comes back as-is, anything else as base64 (`encoding`).",
                schema(
                    json!({"path": {"type": "string"}, "version": {"type": "string", "description": "An older version (drive history)."}}),
                    &["path"],
                ),
                true,
            ),
            tool(
                "volume_sync_status",
                "Sync state of the Cua Volume as this machine sees it: `feed` (`live`, `off` for storage on this machine, `offline` with `last_error`), each device's last sync, your files still uploading (`pending`), `conflicts` (each `path` and the kept `conflict_path`) and the volume mounted in this Space. Check it before relying on a file another device just wrote.",
                schema(json!({}), &[]),
                true,
            ),
            tool(
                "volume_write",
                "Write a file to the Cua Volume (your home, or this Space's folder). Every write is a new version. Secrets are refused: they belong in the user's Keyvault.",
                schema(
                    json!({"path": {"type": "string"}, "content": {"type": "string"},
                              "encoding": {"type": "string", "enum": ["utf8", "base64"], "description": "Default utf8."},
                              "if_etag": {"type": "string", "description": "Only if the file still has this etag."},
                              "create_only": {"type": "boolean", "description": "Only if the file does not exist."}}),
                    &["path", "content"],
                ),
                false,
            ),
            tool(
                "volume_request_access",
                "Ask the user for access to a drive folder you cannot reach (another agent's outputs, write access to public/). The user approves or declines in the Cua app.",
                schema(
                    json!({"prefix": {"type": "string"}, "mode": {"type": "string", "enum": ["r", "rw"], "description": "Default r."},
                              "reason": {"type": "string", "description": "Why, in one line: the user reads it."}}),
                    &["prefix", "reason"],
                ),
                false,
            ),
            tool(
                "computer_list",
                "The user's own computers you were allowed to use (Space ids).",
                schema(json!({}), &[]),
                true,
            ),
            tool(
                "computer_list_tools",
                "List the computer-use tools of one of the user's computers you were allowed to use.",
                schema(
                    json!({"machine": {"type": "string"}, "service": {"type": "string"},
                              "name": {"type": "string", "description": "Only tools whose name contains this."}}),
                    &["machine"],
                ),
                true,
            ),
            tool(
                "computer_call",
                "Call a computer-use tool on one of the user's computers you were allowed to use. Every call is recorded for the user.",
                schema(
                    json!({"machine": {"type": "string"}, "tool": {"type": "string"},
                              "arguments": {"type": "object"}, "service": {"type": "string"}}),
                    &["machine", "tool"],
                ),
                false,
            ),
        ]
    }

    async fn call(&self, tool: &str, arguments: Value) -> Option<ToolOutcome> {
        // A former tool name (`drive_ls`) runs as the tool it now is.
        let tool = cua_spaces_contract::canonical(tool);
        if !NAMES.contains(&tool) {
            return None;
        }
        Some(match AgentTools::call(self, tool, arguments).await {
            Ok(o) => o,
            Err(e) => ToolOutcome::error(&e),
        })
    }
}
