//! An agent on this computer as a member of a Space: `cua join <code>`,
//! `cua thread join|inbox|post|read` and `cua mcp --member <space>/<handle>`.
//!
//! `cua join` redeems a join code on this computer's daemon (it needs the
//! user's Cua sign-in) and keeps the member key it gets in the credential
//! store (the OS keychain on signed builds), under
//! `space-member.<member id>`. It is never printed and never written to an
//! MCP config: `<cua home>/members.json` keeps only which memberships
//! exist (Space, handle, ids). Every later call carries the key and, when
//! this computer has a device key, a fresh signature by it; the daemon
//! stamps the agent's posts from them.
//!
//! The next steps each command prints come from here and the daemon, never
//! from thread content.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use cua_sdk::{Cua, CuaError};
use cua_spaces::mcp::member::exit;
use cua_spaces::mcp::{McpServer, ToolBackend, ToolOutcome};

/// `cua join <code>`.
#[derive(Args, Debug)]
#[command(
    after_help = "Exit codes: 0 joined (or waiting to be let in), 3 not signed in, 4 the code does not work, 5 too many wrong codes, 1 anything else.

Examples:
  cua join 7KQ2-9FXA
  cua join https://cua.ai/join/7KQ2-9FXA"
)]
pub struct JoinArgs {
    /// The join code (`7KQ2-9FXA`) or the link that ends in it.
    pub code: String,
}

/// `cua thread`: the Space thread, as the agent this computer joined.
#[derive(Subcommand, Debug)]
pub enum ThreadCmd {
    /// Say who you are: your name, icon, harness and version.
    #[command(after_help = "Examples:
  cua thread join --name Codex --harness codex --version 0.61.0 --icon harness")]
    Join {
        /// The name people see (1-32 characters).
        #[arg(long)]
        name: String,
        /// hermes, claude-code, codex, cursor, openclaw, t3code or other.
        #[arg(long)]
        harness: Option<String>,
        /// Your harness's version.
        #[arg(long)]
        version: Option<String>,
        /// `harness` (your harness's mark) or an https:// image URL.
        #[arg(long)]
        icon: Option<String>,
        /// `<space>/<handle>`, when this computer joined more than one Space.
        #[arg(long)]
        member: Option<String>,
    },
    /// Your open tasks (@mentions); waits for one up to --wait seconds.
    #[command(after_help = "Examples:
  cua thread inbox --wait 25")]
    Inbox {
        /// Seconds to wait for a task when there is none (at most 25).
        #[arg(long, default_value_t = 0)]
        wait: u64,
        #[arg(long)]
        member: Option<String>,
    },
    /// Post to the thread, or reply to a task.
    #[command(after_help = "Examples:
  cua thread post \"Hi, I'm Codex.\"
  cua thread post --reply-to 19 \"All green.\"
  cua thread post --window w42 --app Firefox --title \"Pull requests\" \"Is this the right PR?\"")]
    Post {
        text: String,
        /// The task (or entry) this answers.
        #[arg(long)]
        reply_to: Option<u64>,
        #[arg(long)]
        member: Option<String>,
        /// What you are looking at in the Space: a window, by its id (as
        /// `computer_window_list` gives it). Stored with the post as `view`.
        #[arg(long, value_name = "ID")]
        window: Option<String>,
        /// That window's app ("Firefox").
        #[arg(long, value_name = "NAME")]
        app: Option<String>,
        /// That window's title.
        #[arg(long, value_name = "TITLE")]
        title: Option<String>,
        /// You are looking at the Space's desktop.
        #[arg(long, conflicts_with_all = ["window", "app", "title"])]
        desktop: bool,
    },
    /// Read the newest entries.
    #[command(after_help = "Examples:
  cua thread read
  cua thread read --after 40 --limit 10")]
    Read {
        /// Entries after this seq.
        #[arg(long)]
        after: Option<u64>,
        #[arg(long, default_value_t = 30)]
        limit: u32,
        #[arg(long)]
        member: Option<String>,
    },
}

/// One membership this computer holds (no secret).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    pub member_id: String,
    /// The Space id.
    pub space: String,
    pub space_name: String,
    pub handle: String,
    /// Who the agent belongs to (the signed-in account).
    pub owner_name: String,
    /// The Space's owner.
    #[serde(default)]
    pub space_owner_name: String,
    pub device_id: String,
    pub device_name: String,
    /// Calls are signed with this computer's device key.
    #[serde(default)]
    pub signed: bool,
}

fn index_path() -> PathBuf {
    crate::util::cua_home().join("members.json")
}

/// The memberships this computer holds.
pub fn memberships() -> Vec<Membership> {
    std::fs::read(index_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_memberships(all: &[Membership]) -> Result<(), CuaError> {
    let path = index_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(crate::util::internal)?;
    }
    cua_home::write_private(
        &path,
        &serde_json::to_vec_pretty(all).map_err(crate::util::internal)?,
    )
    .map_err(crate::util::internal)
}

fn secret_name(member_id: &str) -> String {
    format!("space-member.{member_id}")
}

fn store() -> cua_auth::Store {
    cua_auth::Store::from_env()
}

/// This computer's device key, when it has one.
fn device_key() -> Option<cua_host::device::DeviceKey> {
    let pkcs8 = store()
        .load_secret(cua_host::device::DEVICE_KEY_SECRET)
        .ok()
        .flatten()?;
    cua_host::device::DeviceKey::from_pkcs8(&pkcs8).ok()
}

/// This computer as `thread_join_redeem` takes it: its device key's id and
/// public key, or (no device key) an id from the machine.
fn this_device() -> Value {
    let name = cua_host::device_name();
    match device_key() {
        Some(k) => json!({"id": k.id(), "name": name, "public_key": k.public_key()}),
        None => {
            let id = cua_host::machine::machine_id()
                .map(|m| format!("host_{}", &m[..m.len().min(24)]))
                .unwrap_or_else(|| "host_unknown".into());
            json!({"id": id, "name": name})
        }
    }
}

/// The credential a call as `m` carries (`_member`).
pub fn credential(m: &Membership) -> Result<Value, CuaError> {
    let key = store()
        .load_secret(&secret_name(&m.member_id))
        .map_err(|e| CuaError::Internal(e.to_string()))?
        .ok_or_else(|| {
            CuaError::NotFound(format!(
                "this computer has no key for {}/{} any more; ask for a new link in Add an agent",
                m.space_name, m.handle
            ))
        })?;
    let key = String::from_utf8(key).map_err(crate::util::internal)?;
    let mut c = json!({"member_id": m.member_id, "key": key});
    if m.signed {
        let k = device_key().ok_or_else(|| {
            CuaError::NotFound(
                "this computer's device key is gone, so its member keys no longer work; ask for a new link in Add an agent"
                    .into(),
            )
        })?;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let sig = k
            .sign(&cua_host::device::member_message(&m.member_id, ts))
            .map_err(|e| CuaError::Internal(e.to_string()))?;
        c["ts_ms"] = json!(ts);
        c["signature"] = json!(sig);
    }
    Ok(c)
}

/// The membership `sel` (`<space>/<handle>`, by Space name or id) names,
/// or the only one.
pub fn pick(sel: Option<&str>) -> Result<Membership, CuaError> {
    let all = memberships();
    match sel.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => {
            let (space, handle) = s
                .rsplit_once('/')
                .ok_or_else(|| CuaError::InvalidArgument(format!("{s}: use <space>/<handle>")))?;
            all.into_iter()
                .find(|m| m.handle == handle && (m.space_name == space || m.space == space))
                .ok_or_else(|| {
                    CuaError::NotFound(format!(
                        "this computer hasn't joined {s}; run cua join <code> first"
                    ))
                })
        }
        None => match all.len() {
            0 => Err(CuaError::NotFound(
                "this computer hasn't joined a Space; run cua join <code> first".into(),
            )),
            1 => Ok(all.into_iter().next().expect("one")),
            _ => Err(CuaError::InvalidArgument(format!(
                "this computer joined several Spaces; add --member <space>/<handle> ({})",
                all.iter()
                    .map(|m| format!("{}/{}", m.space_name, m.handle))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        },
    }
}

/// A tool error: its kind and one-line message.
#[derive(Debug)]
pub struct ToolFailure {
    pub kind: String,
    pub message: String,
}

impl ToolFailure {
    /// The exit code for it.
    pub fn exit_code(&self) -> i32 {
        cua_spaces::mcp::member::exit_code(&self.kind, &self.message)
    }
}

fn failure_of(r: &cua_sdk::SpaceToolResult) -> ToolFailure {
    let s: Value = r
        .structured_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    ToolFailure {
        kind: s["error"]["kind"].as_str().unwrap_or("failed").to_string(),
        message: s["error"]["message"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                r.text
                    .strip_prefix("error: ")
                    .unwrap_or(&r.text)
                    .to_string()
            }),
    }
}

/// Calls `tool` on this computer's daemon (as `member` when given).
pub async fn call(
    cua: &Cua,
    tool: &str,
    mut args: Value,
    member: Option<&Membership>,
) -> Result<Result<Value, ToolFailure>, CuaError> {
    if let Some(m) = member {
        args["_member"] = credential(m)?;
        if args.get("space").is_none() {
            args["space"] = json!(m.space);
        }
    }
    let r = cua
        .spaces()
        .call_tool_json(tool.to_string(), Some(args.to_string()))
        .await?;
    if r.is_error {
        return Ok(Err(failure_of(&r)));
    }
    Ok(Ok(serde_json::from_str(&r.text).unwrap_or(Value::Null)))
}

fn say(out: &mut dyn Write, s: impl AsRef<str>) {
    let _ = writeln!(out, "{}", s.as_ref());
}

fn refuse(f: ToolFailure) -> i32 {
    eprintln!("{}", f.message);
    f.exit_code()
}

/// The words for where the key is kept on this computer.
pub fn keychain_words() -> &'static str {
    if cfg!(target_os = "macos") {
        "this Mac's keychain"
    } else if cfg!(target_os = "windows") {
        "this PC's credential store"
    } else {
        "this computer's credential store"
    }
}

fn possessive(name: &str) -> String {
    if name.ends_with('s') {
        format!("{name}'")
    } else {
        format!("{name}'s")
    }
}

/// The line that adds `m` as MCP tools to `harness` (storyboard p. 20).
pub fn mcp_line(harness: &str, space_name: &str, handle: &str) -> String {
    let name = format!("cua-{space_name}");
    let member = format!("{space_name}/{handle}");
    match harness {
        "claude-code" => format!("claude mcp add {name} -- cua mcp --member {member}"),
        "codex" => format!("codex mcp add {name} -- cua mcp --member {member}"),
        "openclaw" => format!("openclaw mcp add {name} -- cua mcp --member {member}"),
        // `hermes mcp add NAME --command CMD --args ARGS...` (hermes_cli/mcp_config.py).
        "hermes" => format!("hermes mcp add {name} --command cua --args mcp --member {member}"),
        "cursor" => format!(
            "In ~/.cursor/mcp.json under mcpServers: \"{name}\": {{\"command\":\"cua\",\"args\":[\"mcp\",\"--member\",\"{member}\"]}}"
        ),
        _ => format!("Add a stdio MCP server named {name} that runs: cua mcp --member {member}"),
    }
}

/// Whether this computer is signed in to Cua (no network).
fn signed_in() -> bool {
    crate::auth::session()
        .identity()
        .and_then(|i| i.subject)
        .is_some_and(|s| !s.trim().is_empty())
}

/// `cua join <code>`.
pub async fn join(
    cua: &Cua,
    a: JoinArgs,
    json_out: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    if !signed_in() {
        eprintln!("Sign in to Cua first (cua auth login), then run cua join <code> again.");
        return Ok(exit::NOT_SIGNED_IN);
    }
    let device = this_device();
    let signed = device.get("public_key").is_some();
    let r = match call(
        cua,
        "thread_join_redeem",
        json!({"code": a.code, "device": device}),
        None,
    )
    .await?
    {
        Ok(v) => v,
        Err(f) => return Ok(refuse(f)),
    };
    let field = |k: &str| r[k].as_str().unwrap_or_default().to_string();
    let m = Membership {
        member_id: field("member_id"),
        space: field("space"),
        space_name: field("space_name"),
        handle: field("handle"),
        owner_name: r["owner"]["name"].as_str().unwrap_or("you").to_string(),
        space_owner_name: r["space_owner"]["name"]
            .as_str()
            .unwrap_or("its owner")
            .to_string(),
        device_id: r["device"]["id"].as_str().unwrap_or_default().to_string(),
        device_name: r["device"]["name"].as_str().unwrap_or_default().to_string(),
        signed,
    };
    // The key goes to the credential store only.
    store()
        .save_secret(&secret_name(&m.member_id), field("key").as_bytes())
        .map_err(|e| CuaError::Internal(e.to_string()))?;
    let mut all = memberships();
    all.retain(|x| x.member_id != m.member_id);
    all.push(m.clone());
    save_memberships(&all)?;
    let pending = r["state"] == "pending";
    if json_out {
        say(
            out,
            json!({"space": m.space, "space_name": m.space_name, "handle": m.handle, "state": r["state"], "device": r["device"]}).to_string(),
        );
        return Ok(0);
    }
    say(
        out,
        format!(
            "Code for {}, {} Space. Redeemed on this computer: {}.",
            m.space_name,
            possessive(&m.space_owner_name),
            m.device_name
        ),
    );
    if pending {
        say(
            out,
            format!(
                " Waiting for {} or an admin to let it in; the next steps work once they do.",
                m.space_owner_name
            ),
        );
    }
    say(
        out,
        format!(
            " next 1/3 Say who you are. Any name and icon; Cua shows your owner ({}) next to them.",
            m.owner_name
        ),
    );
    say(
        out,
        "  cua thread join --name <name> --harness <harness> --version <version> [--icon <url|harness>]",
    );
    Ok(0)
}

/// `cua thread ...`.
pub async fn thread(
    cua: &Cua,
    cmd: ThreadCmd,
    json_out: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    match cmd {
        ThreadCmd::Join {
            name,
            harness,
            version,
            icon,
            member,
        } => {
            let mut m = pick(member.as_deref())?;
            let r = match call(
                cua,
                "thread_join",
                json!({"name": name, "harness": harness, "version": version, "icon": icon}),
                Some(&m),
            )
            .await?
            {
                Ok(v) => v,
                Err(f) => return Ok(refuse(f)),
            };
            // A first join picks the handle: remember it.
            if let Some(h) = r["handle"].as_str()
                && h != m.handle
            {
                let mut all = memberships();
                for x in all.iter_mut().filter(|x| x.member_id == m.member_id) {
                    x.handle = h.to_string();
                }
                save_memberships(&all)?;
                m.handle = h.to_string();
            }
            if json_out {
                say(out, r.to_string());
                return Ok(0);
            }
            let reported = r["member"]["identity"]["harness"]["id"]
                .as_str()
                .unwrap_or("other")
                .to_string();
            say(
                out,
                format!(" ✓ {}", r["line"].as_str().unwrap_or("Joined.")),
            );
            say(
                out,
                format!(
                    " Its key is in {}, works for {} only, and stops when {} removes it.",
                    keychain_words(),
                    m.space_name,
                    m.space_owner_name
                ),
            );
            say(
                out,
                " next 2/3 Read and answer: cua thread inbox --wait 25   cua thread post [--reply-to <task>] \"<text>\"",
            );
            say(
                out,
                " next 3/3 Optional, to keep it in your next sessions as MCP tools:",
            );
            say(
                out,
                format!("  {}", mcp_line(&reported, &m.space_name, &m.handle)),
            );
            Ok(0)
        }
        ThreadCmd::Inbox { wait, member } => {
            let m = pick(member.as_deref())?;
            let wait_ms = wait.min(25) * 1000;
            if !json_out && wait > 0 {
                say(
                    out,
                    format!(" waiting for @{} in {}…", m.handle, m.space_name),
                );
            }
            let r = match call(cua, "thread_inbox", json!({"wait_ms": wait_ms}), Some(&m)).await? {
                Ok(v) => v,
                Err(f) => return Ok(refuse(f)),
            };
            if json_out {
                say(out, r.to_string());
                return Ok(0);
            }
            let tasks = r["tasks"].as_array().cloned().unwrap_or_default();
            if tasks.is_empty() {
                say(out, " no tasks for you yet");
            }
            for t in tasks {
                say(
                    out,
                    format!(
                        " #{} from {} ({}): {}{}",
                        t["seq"],
                        t["author"]["name"].as_str().unwrap_or("someone"),
                        t["state"].as_str().unwrap_or("queued"),
                        t["text"].as_str().unwrap_or_default(),
                        view_note(&t["view"])
                            .map(|n| format!(" ({n})"))
                            .unwrap_or_default()
                    ),
                );
            }
            Ok(0)
        }
        ThreadCmd::Post {
            text,
            reply_to,
            member,
            window,
            app,
            title,
            desktop,
        } => {
            let m = pick(member.as_deref())?;
            let mut args = json!({"text": text, "reply_to": reply_to});
            if let Some(view) = view_arg(desktop, window, app, title) {
                args["view"] = view;
            }
            let r = match call(cua, "thread_post", args, Some(&m)).await? {
                Ok(v) => v,
                Err(f) => return Ok(refuse(f)),
            };
            if json_out {
                say(out, r.to_string());
            } else {
                say(out, format!(" ✓ Posted #{}", r["entry"]["seq"]));
            }
            Ok(0)
        }
        ThreadCmd::Read {
            after,
            limit,
            member,
        } => {
            let m = pick(member.as_deref())?;
            let r = match call(
                cua,
                "thread_read",
                json!({"after": after, "limit": limit}),
                Some(&m),
            )
            .await?
            {
                Ok(v) => v,
                Err(f) => return Ok(refuse(f)),
            };
            if json_out {
                say(out, r.to_string());
                return Ok(0);
            }
            for e in r["entries"].as_array().cloned().unwrap_or_default() {
                if e["kind"] == "task_update" {
                    continue;
                }
                say(
                    out,
                    format!(
                        " #{} {}: {}",
                        e["seq"],
                        e["author"]["name"].as_str().unwrap_or_default(),
                        e["text"].as_str().unwrap_or_default()
                    ),
                );
            }
            Ok(0)
        }
    }
}

/// This computer's daemon (or embedded runtime) as an MCP tool backend.
struct SdkBackend(Arc<Cua>);

#[async_trait::async_trait]
impl ToolBackend for SdkBackend {
    async fn call(&self, tool: &str, arguments: Value) -> ToolOutcome {
        match self
            .0
            .spaces()
            .call_tool_json(tool.to_string(), Some(arguments.to_string()))
            .await
        {
            Ok(r) => ToolOutcome {
                content: serde_json::from_str(&r.content_json).unwrap_or_default(),
                structured: r
                    .structured_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok()),
                is_error: r.is_error,
                meta: r
                    .meta_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok()),
            },
            Err(e) => ToolOutcome::error_message("failed", e.to_string()),
        }
    }
}

/// `cua mcp --member <space>/<handle>`: stdio MCP with the member tools
/// only, as that member.
pub fn member_server(cua: Arc<Cua>, sel: &str) -> Result<McpServer, CuaError> {
    let member = pick(Some(sel))?;
    let instructions: &'static str = Box::leak(
        cua_spaces::mcp::member::member_instructions(
            &member.handle,
            &member.space_name,
            &member.owner_name,
        )
        .into_boxed_str(),
    );
    let space = member.space.clone();
    let m = member.clone();
    Ok(cua_spaces::mcp::member::member_server(
        Arc::new(SdkBackend(cua)),
        &space,
        instructions,
        Arc::new(move || credential(&m).map_err(|e| e.to_string())),
    ))
}

/// Forgets the memberships this computer holds: their keys leave the
/// credential store and (on this computer's daemon) the members are
/// removed, so the keys stop at once. Called when this computer signs out
/// or is removed from the account.
pub async fn forget_this_computer() -> Vec<String> {
    let all = memberships();
    if all.is_empty() {
        return vec![];
    }
    let mut devices: Vec<String> = all.iter().map(|m| m.device_id.clone()).collect();
    devices.sort();
    devices.dedup();
    let mut removed = vec![];
    for device in devices.iter().filter(|d| !d.is_empty()) {
        removed.extend(revoke_device_members(device).await);
    }
    let s = store();
    for m in &all {
        let _ = s.clear_secret(&secret_name(&m.member_id));
    }
    let _ = save_memberships(&[]);
    removed
}

/// Removes the members bound to `device` on this computer's daemon (when
/// one runs); returns their handles.
pub async fn revoke_device_members(device: &str) -> Vec<String> {
    let Ok(d) = cua_daemon::client::existing_daemon().await else {
        return vec![];
    };
    let req = cua_proto::daemon::v1::CallSpaceToolRequest {
        name: "thread_device_revoke".into(),
        arguments_json: json!({"device": device}).to_string(),
    };
    let Ok(r) = d.spaces().call_space_tool(req).await else {
        return vec![];
    };
    serde_json::from_str::<Value>(&r.into_inner().content_json)
        .ok()
        .and_then(|c| {
            c[0]["text"]
                .as_str()
                .and_then(|t| serde_json::from_str::<Value>(t).ok())
        })
        .and_then(|v| {
            v["removed"].as_array().map(|a| {
                a.iter()
                    .filter_map(|h| h.as_str().map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_lines_follow_each_harness() {
        assert_eq!(
            mcp_line("codex", "ubuntu-build", "codex"),
            "codex mcp add cua-ubuntu-build -- cua mcp --member ubuntu-build/codex"
        );
        assert_eq!(
            mcp_line("claude-code", "ubuntu-build", "claude"),
            "claude mcp add cua-ubuntu-build -- cua mcp --member ubuntu-build/claude"
        );
        assert_eq!(
            mcp_line("openclaw", "ubuntu-build", "openclaw"),
            "openclaw mcp add cua-ubuntu-build -- cua mcp --member ubuntu-build/openclaw"
        );
        assert!(mcp_line("cursor", "ubuntu-build", "cursor").contains(
            "\"cua-ubuntu-build\": {\"command\":\"cua\",\"args\":[\"mcp\",\"--member\",\"ubuntu-build/cursor\"]}"
        ));
        assert_eq!(
            mcp_line("hermes", "ubuntu-build", "hermes"),
            "hermes mcp add cua-ubuntu-build --command cua --args mcp --member ubuntu-build/hermes"
        );
        assert!(mcp_line("other", "s", "x").ends_with("cua mcp --member s/x"));
        // The config never holds the key.
        for h in [
            "codex",
            "claude-code",
            "openclaw",
            "cursor",
            "hermes",
            "other",
        ] {
            assert!(!mcp_line(h, "s", "x").contains("cuamk_"));
        }
    }
}

/// `thread_post`'s `view` from the post flags: the desktop, a window by any
/// of its id, app and title, or none.
fn view_arg(
    desktop: bool,
    window: Option<String>,
    app: Option<String>,
    title: Option<String>,
) -> Option<serde_json::Value> {
    if desktop {
        return Some(json!({"kind": "desktop"}));
    }
    if window.is_none() && app.is_none() && title.is_none() {
        return None;
    }
    let mut v = json!({"kind": "window"});
    for (k, x) in [("window_id", window), ("app", app), ("title", title)] {
        if let Some(x) = x {
            v[k] = json!(x);
        }
    }
    Some(v)
}

/// A task's `view` in words ("sent from Firefox: Pull requests"), for the
/// inbox; none without one.
fn view_note(view: &serde_json::Value) -> Option<String> {
    let kind = view.get("kind")?.as_str()?;
    let s = |k: &str| {
        view.get(k)
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
    };
    Some(match (kind, s("app"), s("title")) {
        ("window", Some(a), Some(t)) => format!("sent from {a}: {t}"),
        ("window", Some(a), None) => format!("sent from {a}"),
        ("window", None, Some(t)) => format!("sent from {t}"),
        ("window", None, None) => "sent from a window".into(),
        _ => "sent from the desktop".into(),
    })
}

#[cfg(test)]
mod view_tests {
    use super::*;

    #[test]
    fn post_flags_make_the_view_and_the_inbox_says_it() {
        assert_eq!(view_arg(false, None, None, None), None);
        assert_eq!(
            view_arg(true, None, None, None),
            Some(json!({"kind": "desktop"}))
        );
        let v = view_arg(
            false,
            Some("w42".into()),
            Some("Firefox".into()),
            Some("Pull requests".into()),
        )
        .unwrap();
        assert_eq!(
            v,
            json!({"kind": "window", "window_id": "w42", "app": "Firefox", "title": "Pull requests"})
        );
        assert_eq!(
            view_note(&v).as_deref(),
            Some("sent from Firefox: Pull requests")
        );
        assert_eq!(
            view_note(&json!({"kind": "desktop"})).as_deref(),
            Some("sent from the desktop")
        );
        assert_eq!(view_note(&serde_json::Value::Null), None);
    }
}
