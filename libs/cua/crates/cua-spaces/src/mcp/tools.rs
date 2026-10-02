//! One handler per contract tool.

use super::ToolOutcome;
use crate::error::{Error, Result};
use crate::{Space, SpaceCreate, SpaceCreated, Spaces};
use cua_spaces_contract::inputs as i;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
#[cfg(feature = "spaces-files")]
use std::path::PathBuf;
use std::time::Duration;

/// The most Spaces one `create_space` call makes.
const MAX_CREATE_COUNT: u32 = 8;

pub(super) fn args<T: DeserializeOwned>(tool: &str, value: Value) -> Result<T> {
    let value = if value.is_null() { json!({}) } else { value };
    serde_json::from_value(value)
        .map_err(|e| Error::invalid(format!("{tool}: {e} (see the tool's inputSchema)")))
}

#[cfg(feature = "spaces-files")]
fn expand_host_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

/// Checks the contract's `requires` for `tool` against the Space. Wildcard
/// entries (`teleport.*`) and `stream_endpoint` (display vs window) are
/// checked by the primitive itself, which knows the concrete feature.
fn gate(tool: &str, space: &Space) -> Result<()> {
    if tool == "stream_endpoint" {
        return Ok(());
    }
    let Some(spec) = cua_spaces_contract::tool(tool) else {
        return Ok(());
    };
    for feature in spec.requires.iter().filter(|f| !f.ends_with(".*")) {
        space.require(feature)?;
    }
    Ok(())
}

tokio::task_local! {
    /// The Space [`call_on`] pinned for this call.
    static PINNED: Space;
}

async fn space_for(spaces: &Spaces, tool: &str, id: &str) -> Result<Space> {
    let space = match PINNED.try_with(Space::clone) {
        Ok(pinned) => pinned,
        Err(_) => spaces.space(id).await?,
    };
    gate(tool, &space)?;
    Ok(space)
}

#[allow(dead_code)] // used only when an optional module is compiled out
fn unavailable(feature: &str) -> Error {
    Error::host(
        "feature",
        format!("this build of cua-spaces has no `{feature}` module"),
    )
}

pub(super) async fn call(
    spaces: &Spaces,
    tool: &str,
    arguments: Value,
    broker: Option<&std::sync::Arc<dyn crate::teleport_broker::SessionBroker>>,
) -> ToolOutcome {
    // A former tool name (`drive_ls`) runs as the tool it now is.
    let tool = cua_spaces_contract::canonical(tool);
    match dispatch(spaces, tool, arguments, broker).await {
        Ok(outcome) => outcome,
        Err(e) => {
            tracing::debug!(tool, error = %e, "tool failed");
            ToolOutcome::error(&e)
        }
    }
}

pub(super) async fn call_on(space: &Space, tool: &str, mut arguments: Value) -> ToolOutcome {
    if !super::SPACE_SCOPED_TOOLS.contains(&tool) {
        return ToolOutcome::error(&Error::invalid(format!(
            "{tool} is not a Space-scoped tool"
        )));
    }
    if arguments.is_null() {
        arguments = json!({});
    }
    if let Some(o) = arguments.as_object_mut() {
        o.insert("space".into(), json!(space.id().to_string()));
    }
    // A registry-less runtime: pinned calls never read or write the registry.
    let scratch = Spaces::builder()
        .home(std::env::temp_dir().join("cua-spaces-call-on"))
        .build();
    PINNED
        .scope(space.clone(), call(&scratch, tool, arguments, None))
        .await
}

/// One `phase` vocabulary for every provider (the client's `SpaceState`):
/// `ready` once the Space answered its handshake (direct, relay), its claim
/// bound (Fleet) or its instance runs (local); `starting` while a claim is
/// pending or an instance is provisioning.
pub const PHASE_READY: &str = "ready";
/// See [`PHASE_READY`].
pub const PHASE_STARTING: &str = "starting";

async fn dispatch(
    spaces: &Spaces,
    tool: &str,
    a: Value,
    broker: Option<&std::sync::Arc<dyn crate::teleport_broker::SessionBroker>>,
) -> Result<ToolOutcome> {
    // Tools a registered extension serves (teleport, the Cua Volume,
    // persistent agents): see `crate::extension`.
    if let Some(ext) = spaces.extension_for(tool) {
        let pinned = PINNED.try_with(Space::clone).ok();
        return ext
            .call_tool(crate::extension::ToolCall {
                spaces,
                tool,
                args: if a.is_null() { json!({}) } else { a },
                broker,
                pinned: pinned.as_ref(),
            })
            .await;
    }
    if let Some(what) = extension_capability(tool) {
        return Err(Error::needs_cua_spaces(what));
    }
    match tool {
        // --- lifecycle ------------------------------------------------------
        "add_space" => {
            let a: i::AddSpace = args(tool, a)?;
            let info = spaces
                .add_with_service(&a.url, a.token, a.name, a.service)
                .await?;
            Ok(ToolOutcome::json(&with(
                &info,
                json!({"phase": PHASE_READY}),
            )))
        }
        "remove_space" => {
            let a: i::RemoveSpace = args(tool, a)?;
            let info = spaces.remove(&a.space).await?;
            Ok(ToolOutcome::json(&json!({"removed": info})))
        }
        "list_spaces" => {
            let _: i::ListSpaces = args(tool, a)?;
            // `phase` is the state at the last handshake: a registered Space
            // answered GetCapabilities when it was added.
            // Includes the signed-in account's relay machines (refreshed).
            // A host added by its direct address, and a Space on a host,
            // say how they are reached (`via`).
            let direct_hosts: Vec<String> = spaces
                .registry()
                .direct_hosts()
                .unwrap_or_default()
                .into_iter()
                .map(|h| h.space)
                .collect();
            let rows: Vec<Value> = spaces
                .list_all()
                .await?
                .iter()
                .map(|info| with(info, row_extra(info, &direct_hosts, PHASE_READY)))
                .collect();
            Ok(ToolOutcome::json(&rows))
        }
        "create_space" => {
            let a: i::CreateSpace = args(tool, a)?;
            let place = |e: cua_sandbox_core::placement::PlacementError| {
                Error::Sandbox(cua_sandbox_core::Error::InvalidPlacement(e))
            };
            // A plain word that names no location is one of the user's
            // machines ("mac-mini" is "host:mac-mini").
            let on =
                a.on.as_deref()
                    .map(str::trim)
                    .filter(|o| !o.is_empty())
                    .map(cua_sandbox_core::placement::On::parse_or_host)
                    .transpose()
                    .map_err(place)?;
            let kind = cua_sandbox_core::placement::Kind::parse(
                a.kind.map(|k| k.as_str()).unwrap_or("auto"),
            )
            .map_err(place)?;
            let runtime =
                cua_sandbox_core::placement::Runtime::parse(a.runtime.as_deref().unwrap_or(""))
                    .map_err(place)?;
            let count = a.count.unwrap_or(1);
            if !(1..=MAX_CREATE_COUNT).contains(&count) {
                return Err(Error::invalid(format!(
                    "count {count}: create 1 to {MAX_CREATE_COUNT} Spaces at a time"
                )));
            }
            let one = |name: Option<String>| SpaceCreate {
                image: a.image.clone(),
                on: on.clone(),
                kind,
                runtime: runtime.clone(),
                name,
                timeout: a.timeout.map(Duration::from_secs),
                wait: a.wait,
                reuse: a.reuse.unwrap_or(false),
                spacesd: a.workload.spacesd,
                command: a.workload.command.clone(),
                env: a.workload.env.clone().unwrap_or_default(),
                services: a.workload.services.clone().unwrap_or_default(),
                ..Default::default()
            };
            let row = |created: SpaceCreated| match created {
                SpaceCreated::Ready { info, reused } => {
                    let mut extra = row_extra(&info, &[], PHASE_READY);
                    extra["reused"] = json!(reused);
                    with(&info, extra)
                }
                SpaceCreated::Starting(p) => json!({
                    "id": p.id,
                    "provider": p.id.split_once(':').map_or("", |(l, _)| l),
                    "phase": PHASE_STARTING,
                }),
            };
            if count == 1 {
                return Ok(ToolOutcome::json(&row(spaces
                    .create(one(a.name.clone()))
                    .await?)));
            }
            // Several at once, concurrently: a host reserves capacity per
            // Space, so a limit fails only the ones past it.
            let names: Vec<Option<String>> = (1..=count)
                .map(|n| {
                    a.name.as_ref().map(|base| {
                        if n == 1 {
                            base.clone()
                        } else {
                            format!("{base}-{n}")
                        }
                    })
                })
                .collect();
            let results =
                futures_util::future::join_all(names.into_iter().map(|n| spaces.create(one(n))))
                    .await;
            let mut rows = Vec::new();
            let mut first_error = None;
            for r in results {
                match r {
                    Ok(c) => rows.push(row(c)),
                    Err(e) => {
                        rows.push(json!({"error": {"kind": e.tag(), "message": e.to_string()}}));
                        first_error.get_or_insert(e);
                    }
                }
            }
            if rows.iter().all(|r| r.get("error").is_some()) {
                return Err(first_error.expect("an error"));
            }
            Ok(ToolOutcome::json(&rows))
        }
        "delete_space" => {
            let a: i::SpaceOnly = args(tool, a)?;
            Ok(ToolOutcome::text(spaces.delete(&a.space).await?))
        }
        "stop_space" => {
            let a: i::SpaceOnly = args(tool, a)?;
            Ok(ToolOutcome::json(&spaces.stop(&a.space).await?))
        }
        "start_space" => {
            let a: i::SpaceOnly = args(tool, a)?;
            Ok(ToolOutcome::json(&spaces.start(&a.space).await?))
        }
        // --- your cloud (the sandbox layer's cloud manager) ---------------
        "cloud_status" => {
            let a: i::CloudStatus = args(tool, a)?;
            let c = spaces.sandboxes().clouds()?;
            Ok(json_of(&c.status(a.provider.as_deref()).await?))
        }
        "cloud_test" => {
            let a: i::CloudTest = args(tool, a)?;
            let c = spaces.sandboxes().clouds()?;
            Ok(json_of(&c.test(&cloud_target(a.target)).await?))
        }
        "cloud_connect" => {
            let a: i::CloudConnect = args(tool, a)?;
            let c = spaces.sandboxes().clouds()?;
            Ok(json_of(
                &c.connect(
                    &cloud_target(a.target),
                    a.make_default.unwrap_or(false),
                    a.ttl_hours,
                )
                .await?,
            ))
        }
        "cloud_disconnect" => {
            let a: i::CloudDisconnect = args(tool, a)?;
            let c = spaces.sandboxes().clouds()?;
            Ok(json_of(&c.disconnect(&a.provider).await?))
        }
        "cloud_sweep" => {
            let a: i::CloudSweep = args(tool, a)?;
            let c = spaces.sandboxes().clouds()?;
            Ok(json_of(
                &c.sweep(
                    a.provider.as_deref(),
                    a.dry_run.unwrap_or(true),
                    a.all.unwrap_or(false),
                )
                .await?,
            ))
        }
        // --- execution --------------------------------------------------------
        "space_bash" => {
            let a: i::SpaceBash = args(tool, a)?;
            let s = space_for(spaces, tool, &a.space).await?;
            let t = Duration::from_secs(a.timeout.unwrap_or(60).clamp(1, 24 * 3600));
            Ok(ToolOutcome::text(s.bash(&a.command, t).await?.render()))
        }
        "space_write" => {
            let a: i::SpaceWrite = args(tool, a)?;
            let s = space_for(spaces, tool, &a.space).await?;
            let r = s.write(&a.path, a.content.into_bytes()).await?;
            Ok(ToolOutcome::text(format!(
                "wrote {} ({} bytes, sha256 {} verified in Space)",
                r.path, r.bytes, r.sha256
            )))
        }
        #[cfg(feature = "spaces-files")]
        "upload" => {
            let a: i::Upload = args(tool, a)?;
            let s = space_for(spaces, tool, &a.space).await?;
            Ok(ToolOutcome::json(
                &s.upload(&expand_host_path(&a.path), a.dest.as_deref())
                    .await?,
            ))
        }
        #[cfg(feature = "spaces-files")]
        "send_file" => {
            let a: i::SendFile = args(tool, a)?;
            let s = space_for(spaces, tool, &a.space).await?;
            let needs_home = a
                .target_directory
                .as_deref()
                .is_some_and(|t| t.starts_with('/'));
            let home = if needs_home {
                s.home().await.ok()
            } else {
                None
            };
            let subdir =
                crate::files::downloads_subdir(a.target_directory.as_deref(), home.as_deref())?;
            let report = s
                .send_file(
                    &expand_host_path(&a.path),
                    crate::files::SendFileOptions {
                        subdir,
                        respect_ignore_files: a.respect_ignorefiles.unwrap_or(true),
                        ..Default::default()
                    },
                )
                .await?;
            Ok(ToolOutcome::json(&report))
        }
        #[cfg(feature = "spaces-files")]
        "download" => {
            let a: i::Download = args(tool, a)?;
            let s = space_for(spaces, tool, &a.space).await?;
            let dest = a
                .dest
                .map(|d| expand_host_path(&d))
                .unwrap_or_else(|| spaces.download_dir().to_path_buf());
            Ok(ToolOutcome::json(&s.download(&a.path, &dest).await?))
        }
        #[cfg(not(feature = "spaces-files"))]
        "upload" | "send_file" | "download" => Err(unavailable("spaces-files")),
        // --- streams ----------------------------------------------------------
        #[cfg(feature = "spaces-stream")]
        "stream_endpoint" => {
            use crate::stream::{StreamOptions, StreamTarget};
            let a: i::StreamEndpoint = args(tool, a)?;
            let s = space_for(spaces, tool, &a.space).await?;
            let target = match (a.window_id, a.app_name) {
                (Some(w), _) if !w.is_empty() => StreamTarget::Window(w),
                (_, Some(app)) if !app.is_empty() => {
                    StreamTarget::Window(s.find_window(&app).await?.window_id)
                }
                _ => StreamTarget::Display(a.display),
            };
            let ticket = s
                .open_stream(
                    target,
                    StreamOptions {
                        max_fps: a.max_fps.unwrap_or(0),
                        ticket_ttl: a.ticket_ttl.map(Duration::from_secs),
                        ..Default::default()
                    },
                )
                .await?;
            Ok(ToolOutcome::json(&ticket))
        }
        #[cfg(feature = "spaces-stream")]
        "list_space_windows" => {
            let a: i::ListSpaceWindows = args(tool, a)?;
            let s = space_for(spaces, tool, &a.space).await?;
            Ok(ToolOutcome::json(&s.windows(a.app_name.as_deref()).await?))
        }
        #[cfg(feature = "spaces-stream")]
        "stream_space_window" => {
            use crate::stream::{StreamOptions, StreamTarget};
            let a: i::StreamSpaceWindow = args(tool, a)?;
            let s = space_for(spaces, tool, &a.space).await?;
            let window = match (a.window_id.filter(|w| !w.is_empty()), a.app_name.as_deref()) {
                (Some(id), _) => s
                    .windows(None)
                    .await?
                    .into_iter()
                    .find(|w| w.window_id == id)
                    .ok_or_else(|| Error::NotFound(format!("window {id} in {}", s.id())))?,
                (None, Some(app)) if !app.is_empty() => s.find_window(app).await?,
                _ => {
                    return Err(Error::invalid(
                        "stream_space_window needs a window_id or an app_name",
                    ));
                }
            };
            let ticket = s
                .open_stream(
                    StreamTarget::Window(window.window_id.clone()),
                    StreamOptions {
                        policy: Some(cua_spacesd_client::pb::SessionPolicy::BackgroundOnly),
                        ticket_ttl: Some(Duration::from_secs(120)),
                        ..Default::default()
                    },
                )
                .await?;
            let label = if window.app_name.is_empty() {
                format!("window {}", window.window_id)
            } else {
                format!("{} window", window.app_name)
            };
            spaces
                .operator_display()
                .stream_window(crate::operator::WindowStreamRequest {
                    space_id: s.id().to_string(),
                    window_id: window.window_id.clone(),
                    app_name: window.app_name.clone(),
                    title: a.title.unwrap_or(window.title),
                    media_url: ticket.ws_url,
                    media_session_id: ticket.media_session_id,
                })
                .await?;
            Ok(ToolOutcome::text(format!(
                "Streaming {label} ({}) of {} to your desktop (bidirectional).",
                window.window_id,
                s.id()
            )))
        }
        #[cfg(not(feature = "spaces-stream"))]
        "stream_endpoint" | "list_space_windows" | "stream_space_window" => {
            Err(unavailable("spaces-stream"))
        }
        "show_space_pip" | "hide_space_pip" | "open_space_viewer" => {
            let a: i::SpaceOnly = args(tool, a)?;
            let id = spaces.resolve(&a.space)?.to_string();
            let display = spaces.operator_display();
            match tool {
                "show_space_pip" => {
                    space_for(spaces, tool, &id).await?;
                    display.pin_pip(&id).await?;
                    Ok(ToolOutcome::text(format!(
                        "Pinned {id} as picture-in-picture on your desktop."
                    )))
                }
                "hide_space_pip" => {
                    display.unpin_pip(&id).await?;
                    Ok(ToolOutcome::text(format!(
                        "Unpinned {id} from your desktop."
                    )))
                }
                _ => {
                    space_for(spaces, tool, &id).await?;
                    display.open_viewer(&id).await?;
                    Ok(ToolOutcome::text(format!(
                        "Opened the {id} viewer on your desktop."
                    )))
                }
            }
        }
        // --- services ---------------------------------------------------------
        "list_tools" => {
            let a: i::ListTools = args(tool, a)?;
            let s = space_for(spaces, tool, &a.space).await?;
            let route = s.resolve_service(a.service.as_deref())?;
            let (tools, contract) = s.list_tools(Some(route.name())).await?;
            let filter = a
                .name
                .as_deref()
                .map(str::to_lowercase)
                .filter(|f| !f.is_empty());
            let detailed: Vec<Value> = match &filter {
                Some(f) => tools
                    .iter()
                    .filter(|t| t.name.to_lowercase().contains(f))
                    .map(|t| json!({"name": t.name, "description": t.description, "inputSchema": t.input_schema}))
                    .collect(),
                None => tools
                    .iter()
                    .map(|t| {
                        let first: String = t.description.trim().lines().next().unwrap_or("").chars().take(140).collect();
                        json!({"name": t.name, "description": first})
                    })
                    .collect(),
            };
            let mut out = json!({
                "service": route.name(),
                "count": detailed.len(),
                "tools": detailed,
                "services": s.services(),
                "contract_version": contract,
            });
            if filter.is_none() {
                out["hint"] = json!(
                    "call list_tools again with name=<substring> for full input schemas of matching tools"
                );
            }
            Ok(ToolOutcome::json(&out))
        }
        "call_tool" => {
            let a: i::CallTool = args(tool, a)?;
            let s = space_for(spaces, tool, &a.space).await?;
            let r = s
                .call_tool(
                    a.service.as_deref(),
                    &a.tool,
                    a.arguments.unwrap_or_default(),
                    None,
                )
                .await?;
            Ok(ToolOutcome {
                content: r.content,
                structured: r.structured.filter(|v| v.is_object()),
                is_error: r.is_error,
                meta: r.meta,
            })
        }
        // --- agents -----------------------------------------------------------
        #[cfg(feature = "spaces-agents")]
        "agent_start" | "agent_message" | "agent_status" | "agent_events" | "agent_interrupt"
        | "agent_stop" | "agent_list" => agents(spaces, tool, a).await,
        #[cfg(feature = "spaces-agents")]
        "agent_capabilities" => {
            let _: i::AgentCapabilities = args(tool, a)?;
            Ok(ToolOutcome::json(&crate::agents::capabilities()))
        }
        #[cfg(not(feature = "spaces-agents"))]
        "agent_start" | "agent_message" | "agent_status" | "agent_events" | "agent_interrupt"
        | "agent_stop" | "agent_list" | "agent_capabilities" => Err(unavailable("spaces-agents")),
        // --- keyvault ---------------------------------------------------------
        "request_site_login" => {
            let a: i::RequestSiteLogin = args(tool, a)?;
            space_for(spaces, tool, &a.space).await?;
            Ok(ToolOutcome::json(
                &crate::site_login::request_site_login(spaces, a, None).await?,
            ))
        }
        // --- hotspot ----------------------------------------------------------
        #[cfg(feature = "spaces-hotspot")]
        "hotspot_start" => {
            let a: i::HotspotStart = args(tool, a)?;
            space_for(spaces, tool, &a.space).await?;
            let status = spaces
                .hotspot_start(
                    &a.space,
                    crate::hotspot::HotspotOptions {
                        set_system_proxy: a.set_system_proxy.unwrap_or(true),
                        bypass: a.bypass.unwrap_or_default(),
                        ..Default::default()
                    },
                )
                .await?;
            Ok(ToolOutcome::json(&status))
        }
        #[cfg(feature = "spaces-hotspot")]
        "hotspot_stop" => {
            let a: i::HotspotTarget = args(tool, a)?;
            let stopped = spaces.hotspot_stop(a.space.as_deref()).await?;
            Ok(ToolOutcome::json(&json!({"stopped": stopped})))
        }
        #[cfg(feature = "spaces-hotspot")]
        "hotspot_status" => {
            let a: i::HotspotTarget = args(tool, a)?;
            Ok(ToolOutcome::json(&json!({
                "hotspots": spaces.hotspot_statuses(a.space.as_deref()).await?
            })))
        }
        #[cfg(not(feature = "spaces-hotspot"))]
        "hotspot_start" | "hotspot_stop" | "hotspot_status" => Err(unavailable("spaces-hotspot")),
        // --- sharing ----------------------------------------------------------
        #[cfg(feature = "spaces-agents")]
        "share_space" => {
            let a: i::ShareSpace = args(tool, a)?;
            let role = crate::share::ShareRole::parse(a.role.as_deref().unwrap_or("viewer"))?;
            let shares = spaces.share_space(&a.space, &a.who, role).await?;
            Ok(ToolOutcome::json(&shares))
        }
        #[cfg(feature = "spaces-agents")]
        "unshare_space" => {
            let a: i::UnshareSpace = args(tool, a)?;
            let shares = spaces.unshare_space(&a.space, a.who.as_deref()).await?;
            Ok(ToolOutcome::json(&shares))
        }
        #[cfg(feature = "spaces-agents")]
        "space_shares" => {
            let a: i::SpaceShares = args(tool, a)?;
            let shares = spaces.space_shares(&a.space).await?;
            let mut out = serde_json::to_value(&shares)?;
            if let Some(n) = a.audit.filter(|n| *n > 0) {
                out["audit"] =
                    serde_json::to_value(spaces.share_audit(Some(&shares.space), n as usize)?)?;
            }
            Ok(ToolOutcome::json(&out))
        }
        #[cfg(feature = "spaces-agents")]
        "relay_register_space" => {
            let a: i::SpaceOnly = args(tool, a)?;
            Ok(ToolOutcome::json(&spaces.relay_register(&a.space).await?))
        }
        #[cfg(feature = "spaces-agents")]
        "relay_unregister_space" => {
            let a: i::SpaceOnly = args(tool, a)?;
            let unregistered = spaces.relay_unregister(&a.space).await?;
            let space = spaces.resolve(&a.space)?.to_string();
            Ok(ToolOutcome::json(
                &json!({"space": space, "unregistered": unregistered}),
            ))
        }
        #[cfg(not(feature = "spaces-agents"))]
        "share_space"
        | "unshare_space"
        | "space_shares"
        | "relay_register_space"
        | "relay_unregister_space" => Err(unavailable("spaces-agents")),
        other => Err(Error::NotFound(format!("tool {other}"))),
    }
}

/// `phase`, and for a host added by its direct address or a Space on a
/// host, `via`: `direct` (its Tailscale or LAN address, no relay) or
/// `relay`.
fn row_extra(info: &crate::SpaceInfo, direct_hosts: &[String], phase: &str) -> Value {
    let mut extra = json!({ "phase": phase });
    let via = if direct_hosts.contains(&info.id) || info.host.starts_with("direct:") {
        Some("direct")
    } else if !info.host.is_empty() {
        Some("relay")
    } else {
        None
    };
    if let Some(via) = via {
        extra["via"] = json!(via);
    }
    extra
}

fn with<T: serde::Serialize>(value: &T, extra: Value) -> Value {
    let mut v = serde_json::to_value(value).unwrap_or(Value::Null);
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in e {
            o.insert(k.clone(), val.clone());
        }
    }
    v
}

fn json_of<T: serde::Serialize>(v: &T) -> ToolOutcome {
    ToolOutcome::json(&serde_json::to_value(v).unwrap_or(Value::Null))
}

/// A `cloud_*` target, trimmed.
fn cloud_target(t: i::CloudTarget) -> cua_sandbox_core::byoc::CloudTarget {
    let some = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
    cua_sandbox_core::byoc::CloudTarget {
        provider: t.provider.trim().to_ascii_lowercase(),
        profile: some(t.profile),
        region: some(t.region),
        zone: some(t.zone),
        project: some(t.project),
        environment: some(t.environment),
    }
}

/// The capability a contract tool belongs to when an extension serves it
/// (so an MIT build names what is missing instead of "not found").
fn extension_capability(tool: &str) -> Option<&'static str> {
    use cua_spaces_contract::Category;
    match cua_spaces_contract::tool(tool)?.category {
        Category::Teleport => Some("teleport"),
        Category::Volume => Some("Cua Volume"),
        Category::PersistentAgents => Some("persistent agents"),
        _ => None,
    }
}

#[cfg(feature = "spaces-agents")]
async fn agents(spaces: &Spaces, tool: &str, a: Value) -> Result<ToolOutcome> {
    use crate::agents::{self as ag, McpServer, RunOptions};
    let space_arg = a
        .get("space")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid(format!("{tool}: `space` is required")))?
        .to_string();
    let s = space_for(spaces, tool, &space_arg).await?;
    let agents = s.agents().await?;
    // Events without their raw ACP payload (the default for a model).
    let compact = |e: &ag::AgentEvent, raw: bool| {
        let mut v = serde_json::to_value(e).unwrap_or(Value::Null);
        if !raw && let Some(o) = v.as_object_mut() {
            o.remove("raw");
        }
        v
    };
    let tail_of = |evs: &[ag::AgentEvent], n: usize| -> String {
        let lines: Vec<String> = evs.iter().filter_map(ag::AgentEvent::render).collect();
        lines[lines.len().saturating_sub(n)..].join("\n")
    };
    match tool {
        "agent_start" => {
            // `home`: a persistent agent's run (persistent agents ship with
            // Cua Spaces; `agent_start.home` is their extension's operation).
            if a.get("home").is_some_and(|h| !h.is_null()) {
                return match spaces.extension_for("agent_start.home") {
                    Some(ext) => {
                        ext.call_tool(crate::extension::ToolCall {
                            spaces,
                            tool: "agent_start.home",
                            args: a,
                            broker: None,
                            pinned: Some(&s),
                        })
                        .await
                    }
                    None => Err(Error::needs_cua_spaces("persistent agents")),
                };
            }
            let a: i::AgentStart = args(tool, a)?;
            let (host_env, missing) = ag::env_from_host(&a.env_from_host)?;
            let mut env = a.env.clone();
            env.extend(host_env);
            let endpoint = a.base_url.map(|base_url| ag::Endpoint {
                base_url,
                wire: None,
                model: a.model.clone(),
            });
            let started = agents
                .start(
                    &a.agent,
                    &a.prompt,
                    RunOptions {
                        cwd: a.cwd,
                        repo: a.repo,
                        branch: a.branch,
                        env,
                        mcp_servers: a
                            .mcp_servers
                            .into_iter()
                            .map(|m| McpServer {
                                name: m.name,
                                url: m.url,
                                command: m.command,
                                args: m.args,
                                ..Default::default()
                            })
                            .collect(),
                        endpoint,
                        model: a.model,
                        exit_when_idle: a.exit_when_idle.unwrap_or(false),
                        ..Default::default()
                    },
                )
                .await?;
            if a.show.unwrap_or(false) {
                show(&s, &started.run_dir, &started.run_id).await;
            }
            let mut notes = started.notes.clone();
            for m in missing {
                notes.push(format!(
                    "{m} is not set in this server's environment; not forwarded"
                ));
            }
            Ok(ToolOutcome::json(&json!({
                "run_id": started.run_id, "agent": started.harness, "space": s.id().to_string(),
                "process_tag": started.process_tag, "cwd": started.cwd, "notes": notes,
                "hint": "follow with agent_events(cursor) or agent_status; continue with agent_message; cancel a turn with agent_interrupt",
            })))
        }
        "agent_message" => {
            let a: i::AgentMessage = args(tool, a)?;
            if a.force.unwrap_or(false) {
                agents.interrupt(&a.run_id).await?;
            }
            let st = agents.send(&a.run_id, &a.text, vec![]).await?;
            Ok(ToolOutcome::json(&json!({
                "delivered": true, "run_id": a.run_id, "status": st.status, "phase": st.phase,
                "reason": if st.phase == "working" { "queued after the running turn" } else { "a new turn in the same session" },
            })))
        }
        "agent_events" => {
            let a: i::AgentEvents = args(tool, a)?;
            let raw = a.raw.unwrap_or(false);
            let page = agents
                .events(
                    &a.run_id,
                    a.cursor.unwrap_or(0),
                    a.max.unwrap_or(100).clamp(1, 1000) as usize,
                )
                .await?;
            let st = agents.status(&a.run_id).await?;
            Ok(ToolOutcome::json(&json!({
                "run_id": a.run_id, "status": st.status, "phase": st.phase,
                "events": page.events.iter().map(|e| compact(e, raw)).collect::<Vec<_>>(),
                "cursor": page.cursor, "caught_up": page.caught_up,
            })))
        }
        "agent_status" => {
            let a: i::AgentStatus = args(tool, a)?;
            let st = agents.status(&a.run_id).await?;
            let result = agents.result(&a.run_id).await?;
            let tail = agents.events(&a.run_id, 0, 100_000).await?;
            Ok(ToolOutcome::json(&json!({
                "run_id": st.run_id, "agent": st.harness, "status": st.status, "phase": st.phase,
                "reason": st.reason, "turn": st.turn, "alive": st.alive,
                "accepts_message": st.accepts_message,
                "summary": st.meta.as_ref().map(|m| m.prompt.chars().take(100).collect::<String>()),
                "created_at": st.meta.as_ref().map(|m| m.created_at),
                "result": result,
                "output_tail": tail_of(&tail.events, a.tail.unwrap_or(40) as usize),
                "space": s.id().to_string(),
            })))
        }
        "agent_interrupt" => {
            let a: i::AgentInterrupt = args(tool, a)?;
            let st = agents.interrupt(&a.run_id).await?;
            Ok(ToolOutcome::json(&json!({
                "run_id": a.run_id, "interrupted": st.alive == Some(true), "status": st.status,
            })))
        }
        "agent_stop" => {
            let a: i::AgentStop = args(tool, a)?;
            let st = agents.stop(&a.run_id).await?;
            Ok(ToolOutcome::json(&json!({
                "run_id": a.run_id, "stopped": st.alive == Some(false), "alive": st.alive,
                "reason": match st.alive {
                    Some(false) => "the run's process is gone",
                    Some(true) => "the run is STILL RUNNING after the stop attempt",
                    None => "could not confirm: the liveness probe did not run",
                },
            })))
        }
        _ => {
            let _: i::SpaceOnly = args(tool, a)?;
            let runs: Vec<Value> = agents.list().await?.iter().map(list_row).collect();
            Ok(ToolOutcome::json(
                &json!({"space": s.id().to_string(), "runs": runs}),
            ))
        }
    }
}

/// One `agent_list` row. It carries the same published `reason`, `alive`
/// and `accepts_message` as `agent_status`, so a roster built from one list
/// call never has to guess (or default) whether a follow-up would be taken.
#[cfg(feature = "spaces-agents")]
fn list_row(r: &crate::agents::RunInfo) -> Value {
    json!({"run_id": r.run_id, "agent": r.harness, "status": r.status,
        "phase": r.phase, "reason": r.reason, "turn": r.turn, "alive": r.alive,
        "accepts_message": r.accepts_message,
        "summary": r.meta.as_ref().map(|m| m.prompt.chars().take(100).collect::<String>()),
        "created_at": r.meta.as_ref().map(|m| m.created_at),
        "label": r.meta.as_ref().and_then(|m| m.label.clone())})
}

#[cfg(all(test, feature = "spaces-agents"))]
mod list_row_tests {
    use super::list_row;
    use crate::agents::{RunInfo, RunStatus};

    fn run(status: RunStatus, accepts_message: bool) -> RunInfo {
        RunInfo {
            run_id: "run-1".into(),
            harness: Some("goose".into()),
            status,
            phase: "waiting".into(),
            reason: "waiting for a follow-up".into(),
            turn: 1,
            session_id: None,
            stop_reason: None,
            error: None,
            alive: Some(true),
            accepts_message,
            meta: None,
        }
    }

    #[test]
    fn an_idle_row_publishes_that_it_accepts_a_message() {
        let row = list_row(&run(RunStatus::Idle, true));
        assert_eq!(row["status"], "idle");
        assert_eq!(row["accepts_message"], true);
        assert_eq!(row["reason"], "waiting for a follow-up");
        assert_eq!(row["alive"], true);
    }

    #[test]
    fn a_row_that_refuses_says_so_rather_than_omitting_it() {
        let row = list_row(&run(RunStatus::Failed, false));
        assert_eq!(row["accepts_message"], false);
    }
}

/// Best effort: a terminal on the Space's desktop following the run's
/// event log. Never fatal.
#[cfg(feature = "spaces-agents")]
async fn show(s: &crate::Space, run_dir: &str, run_id: &str) {
    use crate::agents::runs::TAG_PREFIX;
    let q = |x: &str| cua_agents::quote(x);
    let viewer = format!("tail -n +1 -f {}/events.jsonl", q(run_dir));
    let script = format!(
        "export DISPLAY=\"${{DISPLAY:-:1}}\"; command -v xterm >/dev/null 2>&1 && exec xterm -T {} -e sh -c {}",
        q(&format!("agent-{run_id}")),
        q(&viewer)
    );
    if let Ok(g) = s.spacesd()
        && let Ok(h) = g
            .spawn(
                cua_spacesd_client::Command::shell(script)
                    .tag(format!("{TAG_PREFIX}{run_id}/view")),
            )
            .await
    {
        h.detach();
    }
}
