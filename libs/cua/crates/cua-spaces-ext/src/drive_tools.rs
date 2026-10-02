// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Cua Volume tools (`drive_*`), for any caller context.
//!
//! [`drive_tool`] runs one tool as a given [`Context`]: the Spaces MCP
//! server calls it as the user (or, with `as_agent`, as that agent sees
//! the drive), and the in-Space agent bridge calls it as the agent the run
//! belongs to. Access is checked by [`cua_volume::Session`], never here: this
//! module only shapes arguments and results.
//!
//! An agent context can never pick another identity (`as_agent` must be
//! absent or name itself) and can never grant, approve, deny, revoke or read
//! the grant list or the audit log.

use crate::DriveResult as _;
use crate::SpacesDrive as _;
use base64::Engine as _;
use cua_spaces_contract::inputs as i;
use cua_volume::{Condition, Context, Drive, Mode, Principal};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::ToolOutcome;
use cua_spaces::error::{Error, Result};

/// Largest file read or written through a tool.
pub const MAX_TOOL_BYTES: usize = 8 * 1024 * 1024;

/// Every drive tool name.
pub const DRIVE_TOOLS: &[&str] = &[
    "volume_ls",
    "volume_read",
    "volume_write",
    "volume_delete",
    "volume_history",
    "volume_restore",
    "volume_grant",
    "volume_revoke",
    "volume_grants",
    "volume_request_access",
    "volume_requests",
    "volume_approve",
    "volume_deny",
    "volume_audit",
];

/// The drive's runtime tools (user only): storage, mount, sync, cache.
pub const SERVICE_TOOLS: &[&str] = &[
    "volume_storage",
    "volume_storage_set",
    "volume_mount_status",
    "volume_mount",
    "volume_unmount",
    "volume_sync_status",
    "volume_sync_events",
    "volume_sync_resolve",
    "volume_cache_stats",
    "volume_cache_set",
    "volume_cache_clear",
];

/// Runs drive runtime tool `tool` as `ctx` (the user only).
pub async fn service_tool(
    spaces: &cua_spaces::Spaces,
    ctx: Context,
    tool: &str,
    a: Value,
) -> Result<ToolOutcome> {
    check_identity(&ctx, &a)?;
    if tool == "volume_sync_status" {
        return Ok(ToolOutcome::json(&sync_status_for(spaces, &ctx).await?));
    }
    user_only(&ctx, tool)?;
    let svc = spaces.drive_service().await?;
    match tool {
        "volume_storage" => Ok(ToolOutcome::json(&svc.storage().drive()?)),
        "volume_storage_set" => {
            let x: i::DriveStorageSet = args(tool, a)?;
            let s3 = x.s3.map(|s| cua_volume::config::S3Settings {
                endpoint: s.endpoint.filter(|e| !e.is_empty()),
                region: s.region.unwrap_or_else(|| "us-east-1".into()),
                bucket: s.bucket,
                root: s.root.unwrap_or_default(),
                path_style: s.path_style.unwrap_or(false),
            });
            let check = svc
                .set_storage(cua_volume::service::StorageUpdate {
                    backend: x.backend,
                    s3,
                    access_key_id: x.access_key_id,
                    secret_access_key: x.secret_access_key,
                    dry_run: x.dry_run.unwrap_or(false),
                })
                .await
                .drive()?;
            Ok(ToolOutcome::json(&check))
        }
        "volume_mount_status" => Ok(ToolOutcome::json(&svc.mount_status().await)),
        "volume_mount" => Ok(ToolOutcome::json(&svc.mount().await.drive()?)),
        "volume_unmount" => Ok(ToolOutcome::json(&svc.unmount().await.drive()?)),
        "volume_sync_events" => {
            let x: i::DriveSyncEvents = args(tool, a)?;
            let wait = std::time::Duration::from_millis(x.wait_ms.unwrap_or(0).min(30_000) as u64);
            let (events, next) = svc.sync_events(x.since_seq.unwrap_or(0), wait).await;
            Ok(ToolOutcome::json(
                &json!({"events": events, "next_seq": next}),
            ))
        }
        "volume_sync_resolve" => {
            let x: i::DriveSyncResolve = args(tool, a)?;
            svc.sync_resolve(&x.path).drive()?;
            Ok(ToolOutcome::json(&json!({"resolved": x.path})))
        }
        "volume_cache_stats" => Ok(ToolOutcome::json(&svc.cache_stats())),
        "volume_cache_set" => {
            let x: i::DriveCacheSet = args(tool, a)?;
            Ok(ToolOutcome::json(&svc.cache_set(x.capacity_bytes).drive()?))
        }
        "volume_cache_clear" => Ok(ToolOutcome::json(&svc.cache_clear())),
        other => Err(Error::NotFound(format!("tool {other}"))),
    }
}

/// The sync status as `ctx` may see it, with the volumes mounted in
/// Spaces. The user sees everything. An agent (or a Space) sees the
/// pending uploads and conflicts of files it can read, its own Space's
/// volume, and no host paths. An agent inside a Space gets this host's
/// view: its tools are answered here.
pub async fn sync_status_for(spaces: &cua_spaces::Spaces, ctx: &Context) -> Result<Value> {
    let svc = spaces.drive_service().await?;
    let mut s = svc.sync_status().await;
    let mut volumes = spaces.volumes().await;
    if ctx.principal != Principal::User {
        let session = spaces.drive().session(ctx.clone());
        let readable = |p: &str| session.mode(p).ok().flatten().is_some();
        s.conflicts
            .retain(|c| readable(&c.path) || readable(&c.conflict_path));
        s.pending.retain(|p| readable(&p.path));
        s.pending_uploads = s.pending.len() as u32;
        s.pending_bytes = s.pending.iter().map(|p| p.bytes).sum();
        if let Some(c) = s.cache.as_mut() {
            c.dir.clear();
        }
        volumes.retain(|v| ctx.space.as_deref() == Some(v.space.as_str()));
    }
    let mut unavailable = spaces
        .drive_extension()
        .map(|e| e.runtime.unavailable())
        .unwrap_or_default();
    if ctx.principal != Principal::User {
        unavailable.retain(|(space, _)| ctx.space.as_deref() == Some(space.as_str()));
    }
    let mut v = serde_json::to_value(&s).map_err(|e| Error::invalid(e.to_string()))?;
    v["volumes"] = serde_json::to_value(&volumes).map_err(|e| Error::invalid(e.to_string()))?;
    // Spaces that connected without a volume, and why.
    v["volume_errors"] = unavailable
        .into_iter()
        .map(|(space, error)| json!({"space": space, "error": error}))
        .collect();
    Ok(v)
}

fn args<T: DeserializeOwned>(tool: &str, value: Value) -> Result<T> {
    let value = if value.is_null() { json!({}) } else { value };
    serde_json::from_value(value)
        .map_err(|e| Error::invalid(format!("{tool}: {e} (see the tool's inputSchema)")))
}

fn str_arg<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// The context a host MCP caller acts in: the user, or the agent named by
/// `as_agent` (in `in_space`).
pub fn host_context(arguments: &Value) -> Result<Context> {
    match str_arg(arguments, "as_agent") {
        None => Ok(Context::user()),
        Some(a) => {
            if !cua_volume::path::valid_agent_name(a) {
                return Err(Error::invalid(format!(
                    "as_agent {a:?} is not an agent name"
                )));
            }
            Ok(Context::agent(a, str_arg(arguments, "in_space")))
        }
    }
}

/// Refuses an agent context that names another identity.
fn check_identity(ctx: &Context, arguments: &Value) -> Result<()> {
    if ctx.principal == Principal::User {
        return Ok(());
    }
    let named = str_arg(arguments, "as_agent");
    if let Some(n) = named
        && Principal::Agent(n.to_string()) != ctx.principal
    {
        return Err(crate::drive_err(cua_volume::Error::Forbidden(format!(
            "{} may not act as agent:{n}",
            ctx.principal
        ))));
    }
    if let (Some(s), Some(mine)) = (str_arg(arguments, "in_space"), ctx.space.as_deref())
        && s != mine
    {
        return Err(crate::drive_err(cua_volume::Error::Forbidden(format!(
            "{} is in {mine}, not {s}",
            ctx.principal
        ))));
    }
    Ok(())
}

fn user_only(ctx: &Context, tool: &str) -> Result<()> {
    if ctx.principal == Principal::User {
        Ok(())
    } else {
        Err(crate::drive_err(cua_volume::Error::Forbidden(format!(
            "{tool} is for the user only; {} can ask with volume_request_access",
            ctx.principal
        ))))
    }
}

fn mode(s: &str) -> Result<Mode> {
    Mode::parse(s).drive()
}

fn expires(secs: Option<u64>) -> Option<u64> {
    secs.map(|s| cua_volume::now_ms() + s.saturating_mul(1000))
}

/// Runs a blocking drive call (a presence prompt, a locked JSON file) off
/// the async runtime.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> cua_volume::Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| crate::drive_err(cua_volume::Error::Backend(format!("drive task: {e}"))))?
        .drive()
}

/// Runs drive tool `tool` as `ctx`.
pub async fn drive_tool(drive: &Drive, ctx: Context, tool: &str, a: Value) -> Result<ToolOutcome> {
    drive_tool_synced(drive, None, ctx, tool, a).await
}

/// [`drive_tool`], with each file's sync state from `feed` where it is
/// cheap to know (`sync` on `volume_ls` entries that have something to say,
/// and on `volume_read`).
pub async fn drive_tool_synced(
    drive: &Drive,
    feed: Option<&cua_volume::feed::Feed>,
    ctx: Context,
    tool: &str,
    a: Value,
) -> Result<ToolOutcome> {
    check_identity(&ctx, &a)?;
    let session = drive.session(ctx.clone());
    match tool {
        "volume_ls" => {
            let x: i::DriveLs = args(tool, a)?;
            let path = x.path.unwrap_or_default();
            let entries = session.ls(&path).await.drive()?;
            let entries: Vec<Value> = entries
                .into_iter()
                .map(|e| {
                    let sync = (!e.folder)
                        .then(|| feed.map(|f| f.file_sync(&e.path)))
                        .flatten()
                        .filter(|s| !s.is_plain());
                    let mut v = serde_json::to_value(&e).unwrap_or(Value::Null);
                    if let Some(s) = sync {
                        v["sync"] = serde_json::to_value(s).unwrap_or(Value::Null);
                    }
                    v
                })
                .collect();
            Ok(ToolOutcome::json(&json!({
                "path": cua_volume::path::folder(&path).drive()?,
                "principal": ctx.principal.id(),
                "entries": entries,
            })))
        }
        "volume_read" => {
            let x: i::DriveRead = args(tool, a)?;
            let (bytes, meta) = session.read(&x.path, x.version.as_deref()).await.drive()?;
            if bytes.len() > MAX_TOOL_BYTES {
                return Err(Error::invalid(format!(
                    "{} is {} bytes; volume_read returns at most {MAX_TOOL_BYTES}",
                    x.path,
                    bytes.len()
                )));
            }
            let (encoding, content) = match String::from_utf8(bytes) {
                Ok(text) => ("utf8", text),
                Err(e) => (
                    "base64",
                    base64::engine::general_purpose::STANDARD.encode(e.into_bytes()),
                ),
            };
            let sync = feed.map(|f| f.file_sync(&meta.key));
            let mut out = json!({
                "path": meta.key, "size": meta.size, "etag": meta.etag,
                "version": meta.version, "modified_ms": meta.modified_ms,
                "encoding": encoding, "content": content,
            });
            if let Some(s) = sync {
                out["sync"] = serde_json::to_value(s).unwrap_or(Value::Null);
            }
            Ok(ToolOutcome::json(&out))
        }
        "volume_write" => {
            let x: i::DriveWrite = args(tool, a)?;
            let bytes = match x.encoding.as_deref().unwrap_or("utf8") {
                "utf8" | "text" => x.content.into_bytes(),
                "base64" => base64::engine::general_purpose::STANDARD
                    .decode(x.content.trim())
                    .map_err(|e| Error::invalid(format!("content is not base64: {e}")))?,
                other => {
                    return Err(Error::invalid(format!(
                        "encoding {other:?}: use utf8 or base64"
                    )));
                }
            };
            if bytes.len() > MAX_TOOL_BYTES {
                return Err(Error::invalid(format!(
                    "{} bytes; volume_write takes at most {MAX_TOOL_BYTES}",
                    bytes.len()
                )));
            }
            let cond = match (x.if_etag, x.create_only.unwrap_or(false)) {
                (Some(_), true) => {
                    return Err(Error::invalid("pass if_etag or create_only, not both"));
                }
                (Some(e), false) => Condition::IfMatch(e),
                (None, true) => Condition::IfNoneMatch,
                (None, false) => Condition::None,
            };
            let meta = session.write(&x.path, bytes, cond).await.drive()?;
            Ok(ToolOutcome::json(&meta))
        }
        "volume_delete" => {
            let x: i::DriveDelete = args(tool, a)?;
            let cond = x.if_etag.map(Condition::IfMatch).unwrap_or_default();
            session.delete(&x.path, cond).await.drive()?;
            Ok(ToolOutcome::json(&json!({"deleted": x.path})))
        }
        "volume_history" => {
            let x: i::DriveHistory = args(tool, a)?;
            let versions = session.history(&x.path).await.drive()?;
            Ok(ToolOutcome::json(
                &json!({"path": x.path, "versions": versions}),
            ))
        }
        "volume_restore" => {
            let x: i::DriveRestore = args(tool, a)?;
            Ok(ToolOutcome::json(
                &session.restore(&x.path, &x.version).await.drive()?,
            ))
        }
        "volume_grant" => {
            user_only(&ctx, tool)?;
            let x: i::DriveGrant = args(tool, a)?;
            let m = mode(&x.mode)?;
            let d = drive.clone();
            let g = blocking(move || {
                d.grant(
                    &x.principal,
                    &x.prefix,
                    m,
                    expires(x.expires_in_secs),
                    x.note.as_deref().unwrap_or(""),
                )
            })
            .await?;
            Ok(ToolOutcome::json(&g))
        }
        "volume_revoke" => {
            user_only(&ctx, tool)?;
            let x: i::DriveRevoke = args(tool, a)?;
            let d = drive.clone();
            Ok(ToolOutcome::json(
                &blocking(move || d.revoke(&x.grant_id)).await?,
            ))
        }
        "volume_grants" => {
            user_only(&ctx, tool)?;
            let x: i::DriveGrants = args(tool, a)?;
            let now = cua_volume::now_ms();
            let d = drive.clone();
            let mut grants = blocking(move || d.grants()).await?;
            if !x.all.unwrap_or(false) {
                grants.retain(|g| g.is_live(now));
            }
            Ok(ToolOutcome::json(&json!({"grants": grants})))
        }
        "volume_request_access" => {
            let x: i::DriveRequestAccess = args(tool, a)?;
            if ctx.principal == Principal::User {
                return Err(Error::invalid(
                    "the user already has the whole drive; pass as_agent to ask as an agent",
                ));
            }
            let m = mode(&x.mode)?;
            let reason = x.reason.unwrap_or_default();
            let d = drive.clone();
            let c = ctx.clone();
            let req = blocking(move || d.request_access(&c, &x.prefix, m, &reason)).await?;
            Ok(ToolOutcome::json(&req))
        }
        "volume_requests" => {
            user_only(&ctx, tool)?;
            let d = drive.clone();
            Ok(ToolOutcome::json(
                &json!({"requests": blocking(move || d.requests()).await?}),
            ))
        }
        "volume_approve" => {
            user_only(&ctx, tool)?;
            let x: i::DriveApprove = args(tool, a)?;
            let d = drive.clone();
            let g = blocking(move || d.approve(&x.request_id, expires(x.expires_in_secs))).await?;
            Ok(ToolOutcome::json(&g))
        }
        "volume_deny" => {
            user_only(&ctx, tool)?;
            let x: i::DriveDeny = args(tool, a)?;
            let d = drive.clone();
            let id = x.request_id.clone();
            blocking(move || d.deny(&id)).await?;
            Ok(ToolOutcome::json(&json!({"denied": x.request_id})))
        }
        "volume_audit" => {
            user_only(&ctx, tool)?;
            let x: i::DriveAudit = args(tool, a)?;
            let limit = x.limit.unwrap_or(50).clamp(1, 1000) as usize;
            let log = drive.audit().clone();
            let (events, verdict) = blocking(move || log.tail(limit)).await?;
            let mut out = json!({"events": events, "verified": verdict.is_ok()});
            if let Err(e) = verdict {
                out["error"] = json!(e);
            }
            Ok(ToolOutcome::json(&out))
        }
        other => Err(Error::NotFound(format!("tool {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Gate(AtomicBool);
    impl cua_volume::Presence for Gate {
        fn confirm(&self, _: &str) -> std::result::Result<(), String> {
            if self.0.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err("declined".into())
            }
        }
    }

    fn json_of(o: &ToolOutcome) -> Value {
        serde_json::from_str(o.content[0]["text"].as_str().unwrap()).unwrap()
    }

    async fn call(d: &Drive, ctx: Context, tool: &str, a: Value) -> Result<Value> {
        drive_tool(d, ctx, tool, a).await.map(|o| json_of(&o))
    }

    #[tokio::test]
    async fn the_runtime_tools_answer_the_user_and_refuse_agents() {
        let home = tempfile::tempdir().unwrap();
        let spaces = crate::register(
            cua_spaces::Spaces::builder().home(home.path()),
            cua_volume::Drive::open_local(home.path()),
            None,
        )
        .build();
        let server = cua_spaces::mcp::McpServer::new(spaces.clone());
        let call = |name: &'static str, a: Value| {
            let server = &server;
            async move {
                let o = Box::pin(server.call(name, a)).await;
                let text = o.content[0]["text"].as_str().unwrap_or("").to_string();
                let v = serde_json::from_str(&text).unwrap_or(Value::String(text));
                (o.is_error, v)
            }
        };
        let (err, v) = call("volume_storage", json!({})).await;
        assert!(!err, "{v}");
        assert_eq!(v["backend"], "fs");
        // The former name still answers for one release, and is not listed.
        let (err, old) = call("drive_storage", json!({})).await;
        assert!(!err && old == v, "{old}");
        let listed = server.tools_list().to_string();
        assert!(listed.contains("\"volume_storage\""));
        assert!(!listed.contains("\"drive_"), "aliases stay hidden");
        assert_eq!(v["cloud_available"], false);
        let (err, v) = call(
            "volume_storage_set",
            json!({"backend": "fs", "dry_run": true}),
        )
        .await;
        assert!(!err && v["ok"] == true && v["applied"] == false, "{v}");
        let (err, _) = call("volume_storage_set", json!({"backend": "cloud"})).await;
        assert!(err);
        let (err, v) = call("volume_mount_status", json!({})).await;
        assert!(
            !err && v["enabled"] == false && v["volume_name"] == "Cua Volume",
            "{v}"
        );
        let (err, v) = call("volume_sync_status", json!({})).await;
        assert!(!err && v["feed"] == "off", "{v}");
        assert_eq!(v["backend"], "fs");
        assert!(
            ["off", "unsupported"].contains(&v["mount"].as_str().unwrap_or("")),
            "{v}"
        );
        assert_eq!(v["volumes"], json!([]));
        spaces
            .drive()
            .session(Context::user())
            .write("public/a.md", b"x".to_vec(), Condition::None)
            .await
            .unwrap();
        // Files still uploading: the user sees them all, an agent only
        // those it can read, and each file says so.
        let feed = spaces.drive_feed().expect("the services run");
        for k in ["agents/ada/notes.md", "agents/bob/secret.md", "public/a.md"] {
            feed.set_pending(k, Some(1));
        }
        let (_, v) = call("volume_sync_status", json!({})).await;
        assert_eq!(v["pending_uploads"], 3, "{v}");
        let ada = Context::agent("ada", Some("local:lab"));
        let v = sync_status_for(&spaces, &ada).await.unwrap();
        let paths: Vec<&str> = v["pending"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, ["agents/ada/notes.md", "public/a.md"]);
        assert_eq!(v["pending_uploads"], 2);
        let read = drive_tool_synced(
            spaces.drive(),
            Some(&feed),
            ada.clone(),
            "volume_read",
            json!({"path": "public/a.md"}),
        )
        .await
        .unwrap();
        assert_eq!(json_of(&read)["sync"]["state"], "pending_upload");
        let ls = drive_tool_synced(
            spaces.drive(),
            Some(&feed),
            ada,
            "volume_ls",
            json!({"path": "public"}),
        )
        .await
        .unwrap();
        assert_eq!(
            json_of(&ls)["entries"][0]["sync"]["state"],
            "pending_upload"
        );
        for k in ["agents/ada/notes.md", "agents/bob/secret.md", "public/a.md"] {
            feed.set_pending(k, None);
        }
        let (err, v) = call(
            "volume_sync_events",
            json!({"since_seq": 0, "wait_ms": 100}),
        )
        .await;
        assert!(!err && v["next_seq"].is_u64(), "{v}");
        let (err, v) = call("volume_cache_set", json!({"capacity_bytes": 300u64 << 20})).await;
        assert!(!err && v["capacity_bytes"] == 300u64 << 20, "{v}");
        let (err, v) = call("volume_cache_stats", json!({})).await;
        assert!(!err && v["block_bytes"] == 1 << 20, "{v}");
        let (err, _) = call("volume_cache_clear", json!({})).await;
        assert!(!err);
        let (err, _) = call("volume_sync_resolve", json!({"path": "nothing"})).await;
        assert!(err);
        // An agent never reaches these.
        let (err, v) = call("volume_storage", json!({"as_agent": "ada"})).await;
        assert!(err, "{v}");
        let d = spaces.drive_service().await.unwrap();
        d.shutdown().await;
    }

    #[tokio::test]
    async fn permissions_hold_through_the_tools() {
        let home = tempfile::tempdir().unwrap();
        let gate = Arc::new(Gate(AtomicBool::new(false)));
        let d = Drive::open_local(home.path()).with_presence(gate.clone());
        let user = Context::user();
        call(
            &d,
            user.clone(),
            "volume_write",
            json!({"path": "agents/writer/out.md", "content": "draft"}),
        )
        .await
        .unwrap();
        let rs = Context::agent("researcher", Some("local:lab"));
        let e = call(
            &d,
            rs.clone(),
            "volume_read",
            json!({"path": "agents/writer/out.md"}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.tag(), "forbidden");
        // The host view as that agent is the same refusal.
        let host_as =
            host_context(&json!({"as_agent": "researcher", "in_space": "local:lab"})).unwrap();
        assert_eq!(host_as, rs);
        // An agent cannot name another identity or use admin tools.
        let e = call(
            &d,
            rs.clone(),
            "volume_read",
            json!({"path": "agents/writer/out.md", "as_agent": "writer"}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.tag(), "forbidden");
        for t in [
            "volume_grant",
            "volume_grants",
            "volume_approve",
            "volume_audit",
            "volume_requests",
        ] {
            let e = call(&d, rs.clone(), t, json!({"principal": "agent:researcher", "prefix": "agents/writer/", "mode": "r", "request_id": "x"}))
                .await
                .unwrap_err();
            assert_eq!(e.tag(), "forbidden", "{t}");
        }
        // Asking files a request; approval needs presence.
        let req = call(
            &d,
            rs.clone(),
            "volume_request_access",
            json!({"prefix": "agents/writer/", "mode": "r", "reason": "cite"}),
        )
        .await
        .unwrap();
        let id = req["id"].as_str().unwrap().to_string();
        let e = call(
            &d,
            user.clone(),
            "volume_approve",
            json!({"request_id": id}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.tag(), "not_confirmed");
        gate.0.store(true, Ordering::SeqCst);
        call(
            &d,
            user.clone(),
            "volume_approve",
            json!({"request_id": id}),
        )
        .await
        .unwrap();
        let r = call(
            &d,
            rs.clone(),
            "volume_read",
            json!({"path": "agents/writer/out.md"}),
        )
        .await
        .unwrap();
        assert_eq!(r["content"], "draft");
        assert_eq!(r["encoding"], "utf8");
        // Secrets never land in an agent home.
        let leak = format!("key {}{}", "AKIA", "ABCDEFGHIJKLMNOP");
        let e = call(
            &d,
            rs.clone(),
            "volume_write",
            json!({"path": "agents/researcher/m.md", "content": leak}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.tag(), "secret_detected");
        // Binary round trip and compare-and-swap.
        let w = call(&d, rs.clone(), "volume_write", json!({"path": "agents/researcher/b.bin", "content": "AAEC/w==", "encoding": "base64", "create_only": true}))
            .await
            .unwrap();
        let r = call(
            &d,
            rs.clone(),
            "volume_read",
            json!({"path": "agents/researcher/b.bin"}),
        )
        .await
        .unwrap();
        assert_eq!(
            (r["encoding"].as_str(), r["content"].as_str()),
            (Some("base64"), Some("AAEC/w=="))
        );
        let e = call(
            &d,
            rs.clone(),
            "volume_write",
            json!({"path": "agents/researcher/b.bin", "content": "x", "if_etag": "stale"}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.tag(), "precondition_failed");
        call(
            &d,
            rs.clone(),
            "volume_write",
            json!({"path": "agents/researcher/b.bin", "content": "x", "if_etag": w["etag"]}),
        )
        .await
        .unwrap();
        let audit = call(&d, user, "volume_audit", json!({"limit": 100}))
            .await
            .unwrap();
        assert_eq!(audit["verified"], true);
        let actions: Vec<&str> = audit["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["action"].as_str())
            .collect();
        for want in ["denied", "request", "grant", "secret_blocked"] {
            assert!(actions.contains(&want), "{want} in {actions:?}");
        }
    }
}
