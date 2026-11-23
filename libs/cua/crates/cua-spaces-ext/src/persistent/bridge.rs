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
//! | `thread_read`, `thread_post`, `thread_inbox`, `thread_claim`, `thread_reply` | its Space's thread, always as `agent:<name>` and only in its own Space |

use crate::SpacesDrive as _;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cua_spacesd_client::{DownloadOptions, SpacesdClient, UploadOptions, pb};
use serde_json::{Map, Value, json};

use super::members::PostArgs;
use super::{AgentRecord, Persistent};
use cua_spaces::extension::Caller;
use cua_spaces::mcp::{McpServer, ToolExtension, ToolOutcome};
use cua_spaces::{Error, Result, Spaces};

/// Largest unanswered line buffered per session.
const MAX_LINE: usize = 16 * 1024 * 1024;

/// Server instructions the agent sees.
const BASE_INSTRUCTIONS: &str = "Cua: tools for a persistent agent. Your Space has a thread where people and other agents talk: thread_read reads it, thread_post posts (an @name of another agent gives it a task), thread_inbox lists the tasks addressed to you, thread_claim takes one and thread_reply answers it and marks it done. notify_user tells your user something in the Cua app (use it for results they asked for, or when you need them). The Cua Volume keeps files beyond this Space: your home is agents/<you>/ (your memory lives there too), public/ is shared reference (read only), spaces/<this space>/ is this Space's folder; anything else needs volume_request_access, which the user approves. computer_list shows the user's own computers you were allowed to use; computer_list_tools and computer_call use them. Never put secrets in the drive: the user's Keyvault holds them.";

/// What the agent's MCP server tells it at `initialize`: its tools, then how
/// to do long work and how to share a Space with other agents (the texts the
/// join result and the bundled skill carry too).
pub fn instructions() -> &'static str {
    static TEXT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        format!(
            "{BASE_INSTRUCTIONS} {} {}",
            cua_spaces_contract::LONG_WORK_STEPS,
            cua_spaces_contract::COORDINATION_STEPS
        )
    });
    TEXT.as_str()
}

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
        .with_instructions(instructions())
        .with_extension(Arc::new(AgentTools {
            persistent,
            agent: rec.name.clone(),
            space: rec.space.clone(),
            run_id: run_id.to_string(),
            session: rec.k(),
            task: rec.task.as_ref().map(|t| t.seq),
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
    /// The member's session this run is.
    session: u32,
    /// The task the run's turn works on (an approval it asks belongs to
    /// it unless it says otherwise).
    task: Option<u64>,
}

fn schema(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

fn tool(name: &str, description: &str, input: Value, read_only: bool) -> Value {
    json!({"name": name, "description": description, "inputSchema": input,
           "annotations": {"readOnlyHint": read_only, "destructiveHint": !read_only,
                           "idempotentHint": read_only, "openWorldHint": true}})
}

/// `space_job` as this agent sees it: the contract's tool for its own Space
/// (no `space` argument).
fn space_job_tool() -> Value {
    let contract = cua_spaces_contract::tool("space_job").expect("space_job is a contract tool");
    let mut input = contract.input_schema.clone();
    if let Some(p) = input.get_mut("properties").and_then(Value::as_object_mut) {
        p.remove("space");
    }
    if let Some(r) = input.get_mut("required").and_then(Value::as_array_mut) {
        r.retain(|x| x != "space");
    }
    tool(
        "space_job",
        &format!("{} {}", contract.description, contract.instructions),
        input,
        false,
    )
}

fn str_arg<'a>(a: &'a Value, k: &str) -> Result<&'a str> {
    a.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid(format!("`{k}` is required")))
}

impl AgentTools {
    async fn call(&self, tool: &str, a: Value) -> Result<ToolOutcome> {
        match tool {
            "repo_push" => self.repo_push(&a).await,
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
                    crate::drive_runtime::agent_context(
                        &self.persistent.spaces,
                        &self.agent,
                        Some(&self.space),
                    ),
                    tool,
                    a,
                )
                .await
            }
            "volume_sync_status" => Ok(ToolOutcome::json(
                &crate::drive_tools::sync_status_for(
                    &self.persistent.spaces,
                    &crate::drive_runtime::agent_context(
                        &self.persistent.spaces,
                        &self.agent,
                        Some(&self.space),
                    ),
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
                    crate::drive_runtime::agent_context(
                        &self.persistent.spaces,
                        &self.agent,
                        Some(&self.space),
                    ),
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
            "thread_read" | "thread_post" | "thread_inbox" | "thread_claim" | "thread_reply" => {
                self.thread_tool(tool, a).await
            }
            "space_job" => self.space_job(a).await,
            "stream_space_window" | "list_space_windows" => self.child_windows(tool, a).await,
            "launch_space_app" => self.launch_in_child(a).await,
            other => Err(Error::NotFound(format!("tool {other}"))),
        }
    }

    /// `list_space_windows` and `stream_space_window` for one of this agent's own child Spaces. The window is shown
    /// in this agent's Space (`into` is fixed here, whatever the arguments say): its Stream list, where the person
    /// watches, streamed straight from the child. Any other Space is refused. These tools live here, on the host,
    /// so a home whose image carries an older `cua` has them too.
    async fn child_windows(&self, tool: &str, mut a: Value) -> Result<ToolOutcome> {
        let spaces = &self.persistent.spaces;
        let target = str_arg(&a, "space")?.to_string();
        let info = spaces.find(&target).await.map_err(|_| {
            Error::invalid(format!(
                "no Space {target:?} among yours: pass the name of a child Space you made"
            ))
        })?;
        let id = info.id.to_string();
        if !spaces.descendants(&self.space)?.contains(&id) {
            return Err(Error::invalid(format!(
                "{target:?} is not a child Space of yours: you can only show windows of Spaces you made"
            )));
        }
        if let Some(o) = a.as_object_mut() {
            o.insert("space".into(), json!(id));
            if tool == "stream_space_window" {
                o.insert("into".into(), json!(self.space));
            }
        }
        Ok(McpServer::new(spaces.clone()).call(tool, a).await)
    }

    /// `launch_space_app`: opens an app on the desktop of one of this agent's child Spaces (its computer tool
    /// `computer_launch`), so the agent does not have to find the right shell incantation for a graphical app.
    async fn launch_in_child(&self, a: Value) -> Result<ToolOutcome> {
        let spaces = &self.persistent.spaces;
        let target = str_arg(&a, "space")?.to_string();
        let app = str_arg(&a, "app")?.to_string();
        let info = spaces.find(&target).await.map_err(|_| {
            Error::invalid(format!(
                "no Space {target:?} among yours: pass the name of a child Space you made"
            ))
        })?;
        let id = info.id.to_string();
        if !spaces.descendants(&self.space)?.contains(&id) {
            return Err(Error::invalid(format!(
                "{target:?} is not a child Space of yours: you can only open apps in Spaces you made"
            )));
        }
        // The child's own driver tool (the same one `computer_launch` calls), on its desktop.
        let child = spaces.space(&id).await?;
        let mut arguments = Map::new();
        arguments.insert("name".into(), json!(app));
        if let Some(args) = a
            .get("args")
            .filter(|v| v.as_array().is_some_and(|l| !l.is_empty()))
        {
            arguments.insert("args".into(), args.clone());
        }
        let out = child
            .call_tool(
                None,
                "launch_app",
                arguments,
                Some(std::time::Duration::from_secs(30)),
            )
            .await?;
        let text = out
            .content
            .iter()
            .filter_map(|c| c["text"].as_str().map(str::to_string))
            .collect::<Vec<_>>()
            .join("\n");
        if out.is_error {
            return Err(Error::invalid(format!(
                "{app} did not open in {target}: {}",
                if text.is_empty() {
                    "the Space's desktop refused"
                } else {
                    &text
                }
            )));
        }
        Ok(ToolOutcome::json(
            &json!({"opened": app, "space": target, "detail": text}),
        ))
    }

    /// `space_job` in this agent's own Space (whatever `space` says). With
    /// `notify` the job's end is a task for this agent in the Space's thread.
    async fn space_job(&self, mut a: Value) -> Result<ToolOutcome> {
        if let Some(o) = a.as_object_mut() {
            o.insert("space".into(), json!(self.space));
        }
        let args: cua_spaces_contract::inputs::SpaceJob = serde_json::from_value(a)
            .map_err(|e| Error::invalid(format!("space_job: {e} (see the tool's inputSchema)")))?;
        let space = self.persistent.spaces.space(&self.space).await?;
        let mut out = space.job(&args).await?;
        if args.action == "start" && args.notify.unwrap_or(false) {
            let caller = Caller::Agent {
                name: self.agent.clone(),
                space: self.space.clone(),
            };
            out["notify"] = match self.persistent.watch_job(
                &caller,
                &self.space,
                out["job"].as_str().unwrap_or_default(),
                out["name"].as_str().unwrap_or_default(),
                args.wake.as_deref(),
            ) {
                Ok(_) => json!({"registered": true}),
                Err(e) => json!({"registered": false, "reason": e.to_string()}),
            };
        }
        Ok(ToolOutcome::json(&out))
    }

    /// The thread tools, as this agent in its own Space (the author and
    /// the Space are fixed here, whatever the arguments say).
    async fn thread_tool(&self, tool: &str, a: Value) -> Result<ToolOutcome> {
        let caller = Caller::Agent {
            name: self.agent.clone(),
            space: self.space.clone(),
        };
        let t = self
            .persistent
            .thread(&caller, &self.space)
            .await?
            .as_session(self.session);
        let u64_arg = |k: &str| a.get(k).and_then(Value::as_u64);
        let strings = |k: &str| -> Vec<String> {
            a.get(k)
                .and_then(Value::as_array)
                .map(|v| {
                    v.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        match tool {
            "thread_read" => {
                let after = u64_arg("after");
                if let (Some(after), Some(ms)) = (after, u64_arg("follow_ms").filter(|m| *m > 0)) {
                    t.store()
                        .wait_after(after, std::time::Duration::from_millis(ms))
                        .await?;
                }
                Ok(ToolOutcome::json(&t.read(
                    after,
                    u64_arg("before"),
                    u64_arg("limit").unwrap_or(50) as usize,
                    a.get("mark_read").and_then(Value::as_bool).unwrap_or(true),
                    None,
                )?))
            }
            "thread_post" => Ok(ToolOutcome::json(&json!({"entry":
                t.post(PostArgs {
                    text: str_arg(&a, "text")?.to_string(),
                    reply_to: u64_arg("reply_to").or(if a.get("approval").is_some() {
                        self.task
                    } else {
                        None
                    }),
                    to_session: u64_arg("to_session").map(|k| k as u32),
                    view: a
                        .get("view")
                        .and_then(|v| serde_json::from_value(v.clone()).ok()),
                    attachments: strings("attachments"),
                    approval: a.get("approval").and_then(|x| {
                        Some((
                            x.get("action")?.as_str()?.to_string(),
                            x.get("details")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                        ))
                    }),
                    answer: None,
                    client_id: None,
                })
                .await?}))),
            "thread_inbox" => Ok(ToolOutcome::json(&json!({"tasks": t.inbox(None)?}))),
            "thread_claim" => {
                let task = u64_arg("task").ok_or_else(|| Error::invalid("`task` is required"))?;
                Ok(ToolOutcome::json(&t.claim(task, None, None)?))
            }
            _ => {
                let task = u64_arg("task").ok_or_else(|| Error::invalid("`task` is required"))?;
                let (reply, update) = t
                    .reply(
                        task,
                        str_arg(&a, "text")?,
                        &strings("attachments"),
                        a.get("state").and_then(Value::as_str),
                    )
                    .await?;
                Ok(ToolOutcome::json(
                    &json!({"reply": reply, "update": update}),
                ))
            }
        }
    }
}

impl AgentTools {
    fn host_audit(&self) -> cua_volume::audit::AuditLog {
        cua_volume::audit::AuditLog::new(self.persistent.dir.join("host-tools-audit.jsonl"))
    }

    fn audit(&self, action: &str, what: &str, detail: &str) {
        if let Err(e) =
            self.host_audit()
                .append(&format!("agent:{}", self.agent), action, what, detail)
        {
            tracing::warn!("host tools audit: {e}");
        }
    }

    /// `repo_push`: the host pushes a repository of a Space to GitHub once
    /// the owner says yes. The Space never holds the connection.
    async fn repo_push(&self, a: &Value) -> Result<ToolOutcome> {
        use crate::persistent::host_tools as ht;
        let repo = str_arg(a, "repo")?;
        let branch = str_arg(a, "branch")?;
        let path = str_arg(a, "path")?;
        if !ht::valid_repo(repo) {
            return Err(Error::invalid("`repo` must look like owner/name"));
        }
        if !ht::valid_branch(branch) {
            return Err(Error::invalid("`branch` is not a valid branch name"));
        }
        if path.contains('\n') || path.contains('\0') {
            return Err(Error::invalid("`path` is not a valid path"));
        }
        let host = a
            .get("host")
            .and_then(Value::as_str)
            .unwrap_or(cua_spaces::agents::github_auth::DEFAULT_HOST);
        if !cua_spaces::agents::github_auth::valid_host(host) {
            return Err(Error::invalid("`host` is not a valid GitHub host name"));
        }
        let mut req = ht::PushRequest {
            repo: repo.into(),
            host: host.into(),
            ssh_remote: false,
            branch: branch.into(),
            title: a
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(branch)
                .into(),
            body: a.get("body").and_then(Value::as_str).unwrap_or("").into(),
            open_pr: a.get("open_pr").and_then(Value::as_bool).unwrap_or(true),
            base: a.get("base").and_then(Value::as_str).map(String::from),
        };
        let conn = {
            let h = host.to_string();
            // Asking `gh` who is signed in runs a program: off the async threads.
            tokio::task::spawn_blocking(move || ht::GithubConnection::from_host(&h))
                .await
                .map_err(|e| Error::invalid(e.to_string()))?
        };
        // A developer override of where pushes go, in a build that does not
        // allow one: nothing is measured or asked.
        if let Some(e) = conn.blocked() {
            self.audit(
                "denied",
                ht::REPO_PUSH,
                "pushes are redirected by the environment",
            );
            return Err(e);
        }
        // Switched off by the person: nothing is measured or asked.
        if conn.is_off() {
            self.audit(
                "denied",
                ht::REPO_PUSH,
                "GitHub is switched off for this host",
            );
            return Err(conn.refusal());
        }
        let from = a
            .get("from_space")
            .and_then(Value::as_str)
            .unwrap_or(&self.space);
        let space = self.persistent.spaces.space(from).await?;
        let bundle = format!("/tmp/cua-push-{}.bundle", self.run_id);
        let m = ht::measure_in_space(&space, path, &bundle).await?;
        req.ssh_remote = m.ssh_remote;
        // What the card is about, bound to the content: the full commit and
        // the numbers git can recompute from it. After the answer the host
        // checks the bundle against exactly this before pushing.
        let binding = ht::Binding::from(&m);
        let limits = ht::PushLimits::from_env();
        if !conn.ready(m.ssh_remote) {
            let _ = space
                .bash(
                    &format!("rm -f {}", ht::sh_quote(&bundle)),
                    std::time::Duration::from_secs(20),
                )
                .await;
            self.audit("denied", ht::REPO_PUSH, "no GitHub sign-in");
            return Err(conn.refusal());
        }
        let account = conn.account(m.ssh_remote);
        let (text, details) = ht::card(
            &self.agent,
            &req,
            &m,
            from,
            &account,
            conn.can_open_pr(m.ssh_remote),
        );
        let store = self.persistent.thread_store(&self.space);
        self.audit("ask", &format!("{repo}#{branch}"), &details);
        // What the apps draw: the diffstat per file, the commits, the branches.
        let opens = req.open_pr && conn.can_open_pr(m.ssh_remote);
        let card = crate::persistent::push_card::for_push(
            &space,
            path,
            &self.agent,
            repo,
            branch,
            req.base.as_deref(),
            &m.head,
            &m.base,
            opens.then_some((req.title.as_str(), req.body.as_str())),
            req.open_pr && !opens,
            Some(account.as_str()),
            from,
        )
        .await;
        let (seq, decision) = match ht::ask_owner_card(
            &store,
            ht::REPO_PUSH,
            &text,
            &details,
            card,
            ht::ASK_TIMEOUT,
        )
        .await
        {
            Ok(d) => d,
            Err(e) => {
                self.audit("timeout", &format!("{repo}#{branch}"), "no answer");
                return Err(e);
            }
        };
        let fetch = || ht::fetch_bundle(&space, &bundle, &limits);
        match ht::release(decision, &conn, &req, &binding, &limits, fetch).await {
            Ok(ht::Released::Declined(note)) => {
                self.audit("declined", &format!("{repo}#{branch}"), &note);
                Ok(ToolOutcome::json(&json!({
                    "pushed": false, "approval": seq,
                    "reason": "the owner said not now", "note": note})))
            }
            Ok(ht::Released::Stopped { card, detail }) => {
                // The owner said yes to code that is not what is there now
                // (or it could not be read): nothing was pushed, and the
                // thread says so under the card.
                self.audit("stopped", &format!("{repo}#{branch}"), &detail);
                let mut note = crate::thread::Draft::note("host", None, ht::sentence(&card));
                note.reply_to = Some(seq);
                if let Err(e) = store.append(note) {
                    tracing::warn!("push stopped note: {e}");
                }
                Err(Error::invalid(if card == detail {
                    card
                } else {
                    format!("{card} ({detail})")
                }))
            }
            Ok(ht::Released::Pushed(done)) => {
                self.audit(
                    "pushed",
                    &format!("{repo}#{branch}"),
                    &format!("{} as {account}", done.commit),
                );
                Ok(ToolOutcome::json(&json!({
                    "pushed": true, "approval": seq, "repo": repo,
                    "branch": branch, "commit": done.commit, "account": account,
                    "pull_request": done.pull_request,
                    "open_pull_request_at": done.compare_url})))
            }
            Err(e) => {
                self.audit("failed", &format!("{repo}#{branch}"), &e.to_string());
                Err(e)
            }
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
    "thread_read",
    "thread_post",
    "thread_inbox",
    "thread_claim",
    "thread_reply",
    "space_job",
    "stream_space_window",
    "list_space_windows",
    "launch_space_app",
    "repo_push",
];

#[async_trait::async_trait]
impl ToolExtension for AgentTools {
    fn tools(&self) -> Vec<Value> {
        let tools = vec![
            tool(
                "repo_push",
                "Push a committed repository from a Space to GitHub and open a pull request. The host does it after the user approves a card showing what changes; you hold no GitHub access and the user can say no. Commit your work first. Returns the pull request URL.",
                schema(
                    json!({"path": {"type": "string", "description": "The repository folder in the Space."},
                           "repo": {"type": "string", "description": "owner/name on GitHub."},
                           "branch": {"type": "string", "description": "The branch to create."},
                           "title": {"type": "string", "description": "Pull request title."},
                           "body": {"type": "string", "description": "Pull request description."},
                           "open_pr": {"type": "boolean", "description": "Open a pull request (default true)."},
                           "base": {"type": "string", "description": "Target branch (default: the repository's)."},
                           "host": {"type": "string", "description": "The GitHub host (default github.com; a GitHub Enterprise host name otherwise)."},
                           "from_space": {"type": "string", "description": "The Space holding the repository (default: yours)."}}),
                    &["path", "repo", "branch"],
                ),
                false,
            ),
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
                "Sync state of the Cua Volume as this computer sees it: `feed` (`live`, `off` for storage on this computer, `offline` with `last_error`), each device's last sync, your files still uploading (`pending`), `conflicts` (each `path` and the kept `conflict_path`) and the volume mounted in this Space. Check it before relying on a file another device just wrote.",
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
            tool(
                "thread_read",
                "Read your Space's thread: what people and other agents said, and the open tasks. Without `after`, the newest entries; pass the returned cursor as `after` to read on (follow_ms waits for the next entry, up to 25000), or `before` to page back.",
                schema(
                    json!({"after": {"type": "integer", "description": "Entries after this seq."},
                              "before": {"type": "integer", "description": "The newest entries before this seq."},
                              "limit": {"type": "integer", "description": "Default 50."},
                              "mark_read": {"type": "boolean", "description": "Move your read cursor. Default true."},
                              "follow_ms": {"type": "integer"}}),
                    &[],
                ),
                true,
            ),
            tool(
                "thread_post",
                "Post to your Space's thread, as yourself. An explicit @name of another agent member gives it a task, within limits: a chain stops after 3 agent to agent hops from a person's message and a Space allows 20 such wakes a minute. Never post secrets.",
                schema(
                    json!({"text": {"type": "string"},
                              "reply_to": {"type": "integer", "description": "The seq this answers."},
                              "attachments": {"type": "array", "items": {"type": "string"}, "description": "Cua Volume paths you can read (your home, this Space's folder)."},
                              "approval": {"type": "object", "properties": {"action": {"type": "string"}, "details": {"type": "string"}}, "required": ["action"], "description": "Ask the people to approve something; their answer is a later entry."},
                              "to_session": {"type": "integer", "description": "With an @name: which of that agent's sessions takes the task (default: any free one)."}}),
                    &["text"],
                ),
                false,
            ),
            space_job_tool(),
            tool(
                "launch_space_app",
                "Open an app on the desktop of one of your child Spaces, by its name (for example Firefox). Use this to open a browser or any graphical app there; it is not a shell command. Then call stream_space_window to show it.",
                schema(
                    json!({"space": {"type": "string", "description": "The child Space's name."},
                              "app": {"type": "string", "description": "The app, for example Firefox."},
                              "args": {"type": "array", "items": {"type": "string"}, "description": "Arguments, for example a URL."}}),
                    &["space", "app"],
                ),
                false,
            ),
            tool(
                "list_space_windows",
                "List the windows open in one of your child Spaces (app, title, handle). Use it to see whether an app's window has opened.",
                schema(
                    json!({"space": {"type": "string", "description": "The child Space's name."},
                              "app_name": {"type": "string", "description": "Only this app's windows (substring)."}}),
                    &["space"],
                ),
                true,
            ),
            tool(
                "stream_space_window",
                "Show the person a window of one of your child Spaces, live. It appears in this Space's Stream list and opens by itself, streamed straight from the child. This is how to show an app you opened in a child: launch the app there, then call this with the app's name (it waits up to 10 s for the window to appear). Then say in one short line what they are looking at and leave the child running; do not send a screenshot instead, and do not delete the child they asked to see.",
                schema(
                    json!({"space": {"type": "string", "description": "The child Space's name."},
                              "app_name": {"type": "string", "description": "The app whose largest window to show, for example Firefox."},
                              "window_id": {"type": "string", "description": "An exact handle from list_space_windows (overrides app_name)."},
                              "title": {"type": "string", "description": "Label for the row (default: the window's title)."}}),
                    &["space"],
                ),
                false,
            ),
            tool(
                "thread_inbox",
                "The open tasks addressed to you, oldest first.",
                schema(json!({}), &[]),
                true,
            ),
            tool(
                "thread_claim",
                "Claim a queued task of yours for this session before you work on it. A task another session holds is refused (task_claimed); leave it alone.",
                schema(json!({"task": {"type": "integer"}}), &["task"]),
                false,
            ),
            tool(
                "thread_reply",
                "Reply under a task of yours and set its state (done by default; working when more is coming).",
                schema(
                    json!({"task": {"type": "integer"}, "text": {"type": "string"},
                              "attachments": {"type": "array", "items": {"type": "string"}},
                              "state": {"type": "string", "enum": ["done", "failed", "working"]}}),
                    &["task", "text"],
                ),
                false,
            ),
        ];
        tools
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
