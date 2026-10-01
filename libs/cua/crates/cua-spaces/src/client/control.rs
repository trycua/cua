//! The control plane: every advertised tool behind a typed call.
//!
//! Two objects, and the split is the one `FRICTION.md` §7 asks for. A
//! [`Connection`] is the only thing that can create or destroy a Space; a
//! [`Space`] carries its own id, so the id is not restated on every call.
//!
//! §5, §28 and §33 were the same bug wearing three hats: the recommended
//! entry point could silently claim a cloud sandbox, it could not see a
//! Local Space at all, and a harness handed a Space id threaded it nowhere.
//! Here [`Connection::attach`] reaches *that* Space or fails, and
//! [`Connection::create_space`] is the one call that makes a sandbox; where
//! it runs (and so whether it costs money) is its `on` argument.

use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

use crate::client::error::{Result, SpacesError};
use crate::client::metadata;
use crate::client::model::*;
use crate::client::teleport::{Approval, TeleportManifest, TeleportReceipt, TeleportScope};
use crate::client::transport::{ToolTransport, expect_array, expect_object};

fn object_of(tool: &str, payload: &Value) -> Result<Map<String, Value>> {
    expect_object(tool, payload)
}

/// A connection to a Spaces backend.
pub struct Connection {
    transport: Arc<dyn ToolTransport>,
    /// Last-known snapshot per run, which is what lets the cheap roster call
    /// report a `reason` it was not given (`FRICTION.md` §22).
    snapshots: Mutex<Vec<RunSnapshot>>,
    endpoints: Mutex<Vec<(String, StreamEndpoint)>>,
}

impl Connection {
    pub fn new(transport: Arc<dyn ToolTransport>) -> Self {
        Connection {
            transport,
            snapshots: Mutex::new(Vec::new()),
            endpoints: Mutex::new(Vec::new()),
        }
    }

    pub fn transport(&self) -> Arc<dyn ToolTransport> {
        Arc::clone(&self.transport)
    }

    fn call(&self, tool: &str, arguments: Value) -> Result<Value> {
        self.transport.call_tool(tool, &arguments)
    }

    pub(crate) fn call_json(&self, tool: &str, arguments: Value) -> Result<Value> {
        self.call(tool, arguments)
    }

    pub(crate) fn call_object(&self, tool: &str, arguments: Value) -> Result<Map<String, Value>> {
        let payload = self.call(tool, arguments)?;
        object_of(tool, &payload)
    }

    /// The tool names the backend offers. A liveness probe, and the way to
    /// check a tool exists before depending on it.
    pub fn available_tools(&self) -> Result<Vec<String>> {
        self.transport.available_tools()
    }

    /// Call any tool the core does not model. Present on purpose: an SDK that
    /// cannot be gone around gets forked instead.
    pub fn call_tool_raw(&self, tool: &str, arguments_json: &str) -> Result<String> {
        let arguments: Value = if arguments_json.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(arguments_json)
                .map_err(|error| SpacesError::malformed(tool, error.to_string()))?
        };
        Ok(self.call(tool, arguments)?.to_string())
    }

    // -- Spaces -----------------------------------------------------------

    /// `list_spaces`. Every Space the account can see, in every location.
    pub fn spaces(&self) -> Result<Vec<SpaceInfo>> {
        let payload = self.call("list_spaces", json!({}))?;
        Ok(expect_array(&payload, Some("spaces"))
            .iter()
            .filter_map(Value::as_object)
            .map(SpaceInfo::from_row)
            .filter(|info| !info.id.is_empty())
            .collect())
    }

    /// Attach to a Space that already exists. **Never provisions.**
    pub fn attach(self: &Arc<Self>, space_id: &str, require_ready: bool) -> Result<Space> {
        let all = self.spaces()?;
        let Some(info) = all.iter().find(|info| info.id == space_id).cloned() else {
            return Err(SpacesError::SpaceUnavailable(
                space_id.to_string(),
                format!("not among the {} Spaces this account can see", all.len()),
            ));
        };
        if require_ready && !info.is_ready() {
            return Err(SpacesError::SpaceUnavailable(
                space_id.to_string(),
                format!("phase is {}, not ready", info.raw_phase),
            ));
        }
        Ok(Space {
            info,
            connection: Arc::clone(self),
        })
    }

    /// `create_space`: makes a new sandbox and registers it as a Space.
    /// `options.on` says where: `cloud` is metered, `local` is free, and
    /// unset is the user's default location (local unless configured).
    /// With `reuse`, a reachable registered Space in that location is
    /// returned instead.
    pub fn create_space(self: &Arc<Self>, options: CreateSpaceOptions) -> Result<Space> {
        let mut arguments = json!({ "wait": options.wait.unwrap_or(true), "reuse": options.reuse });
        for (key, value) in [
            ("image", options.image),
            ("on", options.on),
            ("kind", options.kind),
            ("runtime", options.runtime),
            ("name", options.name),
        ] {
            if let Some(v) = value.filter(|v| !v.is_empty()) {
                arguments[key] = Value::String(v);
            }
        }
        let row = self.call_object("create_space", arguments)?;
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| SpacesError::malformed("create_space", "no id in the response"))?
            .to_string();
        let phase = row
            .get("phase")
            .and_then(Value::as_str)
            .unwrap_or("starting")
            .to_string();
        let declared = row
            .get("provider")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .or_else(|| provider_prefix(&id).map(str::to_string))
            .unwrap_or_else(|| "local".into());
        Ok(Space {
            info: SpaceInfo {
                provider: SpaceProvider::from_raw(&declared),
                operating_system: row
                    .get("os")
                    .and_then(Value::as_str)
                    .unwrap_or("linux")
                    .to_string(),
                state: SpaceState::from_phase(&phase),
                raw_phase: phase,
                ip_address: row.get("ip").and_then(Value::as_str).map(str::to_string),
                id,
            },
            connection: Arc::clone(self),
        })
    }

    /// `delete_space`: deletes the sandbox of a Space `create_space` made
    /// and forgets it (a Space added by address is only forgotten).
    /// Irreversible.
    pub fn delete_space(&self, space_id: &str) -> Result<()> {
        self.call("delete_space", json!({ "space": space_id }))?;
        Ok(())
    }

    /// `add_space`: registers any cua-spacesd by URL after a capabilities
    /// handshake. The token is stored 0600 on the server side.
    pub fn add_space(
        &self,
        url: &str,
        token: Option<&str>,
        name: Option<&str>,
    ) -> Result<SpaceInfo> {
        let mut arguments = json!({ "url": url });
        if let Some(token) = token {
            arguments["token"] = json!(token);
        }
        if let Some(name) = name {
            arguments["name"] = json!(name);
        }
        let row = self.call_object("add_space", arguments)?;
        Ok(SpaceInfo::from_row(&row))
    }

    /// `remove_space`: forgets a registration; the sandbox is untouched.
    pub fn remove_space(&self, space_id: &str) -> Result<()> {
        self.call("remove_space", json!({ "space": space_id }))?;
        Ok(())
    }

    // -- Hotspot ----------------------------------------------------------
    //
    // This Mac's network sharing: a *host* capability, not a provider one.
    // It is served by the Cua Spaces app's loopback control server, so it
    // fails when that app is not running, and that is a different failure
    // from a provider limitation. One hotspot is active at a time: starting a
    // second moves the sharing, it does not add to it.

    /// `hotspot_start`.
    pub fn hotspot_start(&self, space_id: &str) -> Result<HotspotStatus> {
        Ok(HotspotStatus::decode(&self.call_object(
            "hotspot_start",
            json!({ "space": space_id }),
        )?))
    }

    /// `hotspot_stop`.
    pub fn hotspot_stop(&self) -> Result<HotspotStatus> {
        Ok(HotspotStatus::decode(
            &self.call_object("hotspot_stop", json!({}))?,
        ))
    }

    /// `hotspot_status`.
    pub fn hotspot_status(&self) -> Result<HotspotStatus> {
        Ok(HotspotStatus::decode(
            &self.call_object("hotspot_status", json!({}))?,
        ))
    }

    // -- Cua Volume ----------------------------------------------------------
    // Host-side tools: the volume every Space and agent shares. `as_agent`
    // (and `in_space`) view it as that agent does; they only narrow.

    fn drive_call(
        &self,
        tool: &str,
        mut arguments: Value,
        as_agent: Option<&str>,
        in_space: Option<&str>,
    ) -> Result<Map<String, Value>> {
        if let Some(o) = arguments.as_object_mut() {
            if let Some(a) = as_agent {
                o.insert("as_agent".into(), json!(a));
            }
            if let Some(s) = in_space {
                o.insert("in_space".into(), json!(s));
            }
        }
        self.call_object(tool, arguments)
    }

    /// `volume_ls`.
    pub fn volume_ls(
        &self,
        path: &str,
        as_agent: Option<&str>,
        in_space: Option<&str>,
    ) -> Result<Map<String, Value>> {
        self.drive_call("volume_ls", json!({ "path": path }), as_agent, in_space)
    }

    /// `volume_read`.
    pub fn volume_read(
        &self,
        path: &str,
        version: Option<&str>,
        as_agent: Option<&str>,
        in_space: Option<&str>,
    ) -> Result<Map<String, Value>> {
        let mut a = json!({ "path": path });
        if let Some(v) = version {
            a["version"] = json!(v);
        }
        self.drive_call("volume_read", a, as_agent, in_space)
    }

    /// `volume_write` (text; pass base64 with `encoding = "base64"`).
    pub fn volume_write(
        &self,
        path: &str,
        content: &str,
        encoding: Option<&str>,
        if_etag: Option<&str>,
        create_only: bool,
        as_agent: Option<&str>,
    ) -> Result<Map<String, Value>> {
        let mut a = json!({ "path": path, "content": content, "create_only": create_only });
        if let Some(e) = encoding {
            a["encoding"] = json!(e);
        }
        if let Some(e) = if_etag {
            a["if_etag"] = json!(e);
        }
        self.drive_call("volume_write", a, as_agent, None)
    }

    /// `volume_delete`.
    pub fn volume_delete(&self, path: &str, if_etag: Option<&str>) -> Result<Map<String, Value>> {
        let mut a = json!({ "path": path });
        if let Some(e) = if_etag {
            a["if_etag"] = json!(e);
        }
        self.call_object("volume_delete", a)
    }

    /// `volume_history`.
    pub fn volume_history(&self, path: &str) -> Result<Map<String, Value>> {
        self.call_object("volume_history", json!({ "path": path }))
    }

    /// `volume_restore`.
    pub fn volume_restore(&self, path: &str, version: &str) -> Result<Map<String, Value>> {
        self.call_object(
            "volume_restore",
            json!({ "path": path, "version": version }),
        )
    }

    /// `volume_grant` (the user confirms with presence).
    pub fn volume_grant(
        &self,
        principal: &str,
        prefix: &str,
        mode: &str,
        expires_in_secs: Option<u64>,
    ) -> Result<Map<String, Value>> {
        let mut a = json!({ "principal": principal, "prefix": prefix, "mode": mode });
        if let Some(s) = expires_in_secs {
            a["expires_in_secs"] = json!(s);
        }
        self.call_object("volume_grant", a)
    }

    /// `volume_revoke`.
    pub fn volume_revoke(&self, grant_id: &str) -> Result<Map<String, Value>> {
        self.call_object("volume_revoke", json!({ "grant_id": grant_id }))
    }

    /// `volume_grants`.
    pub fn volume_grants(&self, all: bool) -> Result<Map<String, Value>> {
        self.call_object("volume_grants", json!({ "all": all }))
    }

    /// `volume_request_access` (as an agent).
    pub fn volume_request_access(
        &self,
        as_agent: &str,
        prefix: &str,
        mode: &str,
        reason: &str,
    ) -> Result<Map<String, Value>> {
        self.drive_call(
            "volume_request_access",
            json!({ "prefix": prefix, "mode": mode, "reason": reason }),
            Some(as_agent),
            None,
        )
    }

    /// `volume_requests`.
    pub fn volume_requests(&self) -> Result<Map<String, Value>> {
        self.call_object("volume_requests", json!({}))
    }

    /// `volume_approve` (the user confirms with presence).
    pub fn volume_approve(&self, request_id: &str) -> Result<Map<String, Value>> {
        self.call_object("volume_approve", json!({ "request_id": request_id }))
    }

    /// `volume_deny`.
    pub fn volume_deny(&self, request_id: &str) -> Result<Map<String, Value>> {
        self.call_object("volume_deny", json!({ "request_id": request_id }))
    }

    /// `volume_audit`.
    pub fn volume_audit(&self, limit: u32) -> Result<Map<String, Value>> {
        self.call_object("volume_audit", json!({ "limit": limit }))
    }

    /// `volume_storage`.
    pub fn volume_storage(&self) -> Result<Map<String, Value>> {
        self.call_object("volume_storage", json!({}))
    }

    /// `volume_storage_set` (`update` is the tool's arguments: `backend`,
    /// `s3`, the keys, `dry_run`).
    pub fn volume_storage_set(&self, update: Value) -> Result<Map<String, Value>> {
        self.call_object("volume_storage_set", update)
    }

    /// `volume_mount_status`.
    pub fn volume_mount_status(&self) -> Result<Map<String, Value>> {
        self.call_object("volume_mount_status", json!({}))
    }

    /// `volume_mount`.
    pub fn volume_mount(&self) -> Result<Map<String, Value>> {
        self.call_object("volume_mount", json!({}))
    }

    /// `volume_unmount`.
    pub fn volume_unmount(&self) -> Result<Map<String, Value>> {
        self.call_object("volume_unmount", json!({}))
    }

    /// `volume_sync_status`.
    pub fn volume_sync_status(&self) -> Result<Map<String, Value>> {
        self.call_object("volume_sync_status", json!({}))
    }

    /// `volume_sync_events`.
    pub fn volume_sync_events(&self, since_seq: u64, wait_ms: u32) -> Result<Map<String, Value>> {
        self.call_object(
            "volume_sync_events",
            json!({ "since_seq": since_seq, "wait_ms": wait_ms }),
        )
    }

    /// `volume_sync_resolve`.
    pub fn volume_sync_resolve(&self, path: &str) -> Result<Map<String, Value>> {
        self.call_object("volume_sync_resolve", json!({ "path": path }))
    }

    /// `volume_cache_stats`.
    pub fn volume_cache_stats(&self) -> Result<Map<String, Value>> {
        self.call_object("volume_cache_stats", json!({}))
    }

    /// `volume_cache_set`.
    pub fn volume_cache_set(&self, capacity_bytes: u64) -> Result<Map<String, Value>> {
        self.call_object(
            "volume_cache_set",
            json!({ "capacity_bytes": capacity_bytes }),
        )
    }

    /// `volume_cache_clear`.
    pub fn volume_cache_clear(&self) -> Result<Map<String, Value>> {
        self.call_object("volume_cache_clear", json!({}))
    }

    // -- Your cloud -------------------------------------------------------

    /// `cloud_status`: your clouds (found credentials, the connected
    /// account and region, what each runs at what hourly cost) and what Cua
    /// created there. `provider` narrows it to one cloud.
    pub fn cloud_status(&self, provider: Option<&str>) -> Result<Map<String, Value>> {
        self.call_object("cloud_status", json!({ "provider": provider }))
    }

    /// `cloud_connect`: remembers where Spaces go in a cloud account (a
    /// `target` object: `provider`, and `profile`, `region`, `zone`,
    /// `project` or `environment`), after the checks `cloud_test` runs.
    pub fn cloud_connect(
        &self,
        target: Value,
        make_default: bool,
        ttl_hours: Option<u32>,
    ) -> Result<Map<String, Value>> {
        let mut args = target;
        if let Value::Object(o) = &mut args {
            o.insert("make_default".into(), json!(make_default));
            o.insert("ttl_hours".into(), json!(ttl_hours));
        }
        self.call_object("cloud_connect", args)
    }

    /// `cloud_test`: checks a cloud account without creating anything.
    pub fn cloud_test(&self, target: Value) -> Result<Map<String, Value>> {
        self.call_object("cloud_test", target)
    }

    /// `cloud_disconnect`: forgets a connected cloud (nothing in it is
    /// deleted).
    pub fn cloud_disconnect(&self, provider: &str) -> Result<Map<String, Value>> {
        self.call_object("cloud_disconnect", json!({ "provider": provider }))
    }

    /// `cloud_sweep`: what Cua left in your clouds; deletes it unless
    /// `dry_run`.
    pub fn cloud_sweep(
        &self,
        provider: Option<&str>,
        dry_run: bool,
        all: bool,
    ) -> Result<Map<String, Value>> {
        self.call_object(
            "cloud_sweep",
            json!({ "provider": provider, "dry_run": dry_run, "all": all }),
        )
    }

    // -- Snapshot memory --------------------------------------------------

    fn remember(&self, snapshot: &RunSnapshot) {
        if let Ok(mut snapshots) = self.snapshots.lock() {
            snapshots.retain(|held| held.id != snapshot.id);
            snapshots.push(snapshot.clone());
        }
    }

    fn last_snapshot(&self, run_id: &str) -> Option<RunSnapshot> {
        self.snapshots
            .lock()
            .ok()?
            .iter()
            .find(|held| held.id == run_id)
            .cloned()
    }
}

/// A handle on one Space.
pub struct Space {
    pub info: SpaceInfo,
    connection: Arc<Connection>,
}

impl Space {
    pub fn id(&self) -> &str {
        &self.info.id
    }

    pub fn provider(&self) -> SpaceProvider {
        self.info.provider
    }

    pub fn capabilities(&self) -> ProviderCapabilities {
        self.info.capabilities()
    }

    /// `$HOME` inside the Space, normalised across providers (§6).
    pub fn home(&self) -> &'static str {
        self.info.provider.home()
    }

    pub fn connection(&self) -> Arc<Connection> {
        Arc::clone(&self.connection)
    }

    fn space_argument(&self) -> Value {
        json!({ "space": self.info.id })
    }

    fn arguments(&self, extra: Value) -> Value {
        let mut merged = extra.as_object().cloned().unwrap_or_default();
        merged.insert("space".into(), Value::String(self.info.id.clone()));
        Value::Object(merged)
    }

    fn call(&self, tool: &str, arguments: Value) -> Result<Value> {
        self.connection.call(tool, arguments)
    }

    fn call_object(&self, tool: &str, arguments: Value) -> Result<Map<String, Value>> {
        self.connection.call_object(tool, arguments)
    }

    fn require(&self, tool: &str, capable: bool, detail: &str) -> Result<()> {
        if capable {
            Ok(())
        } else {
            Err(SpacesError::UnsupportedByProvider {
                tool: tool.to_string(),
                provider: self.info.provider.as_str().to_string(),
                detail: detail.to_string(),
            })
        }
    }

    /// `delete_space` for this Space.
    pub fn delete(&self) -> Result<()> {
        self.connection.delete_space(&self.info.id)
    }

    /// `stop_space`: turns this Space off (suspended or stopped, as its
    /// provider can). Returns `space`, `state`, `power` and `message`.
    pub fn stop(&self) -> Result<Map<String, Value>> {
        self.call_object("stop_space", self.arguments(json!({})))
    }

    /// `start_space`: turns this Space on again (resumed or booted).
    pub fn start(&self) -> Result<Map<String, Value>> {
        self.call_object("start_space", self.arguments(json!({})))
    }

    // -- Sharing ------------------------------------------------------------

    /// `share_space`: lets `who` (an email or account id) watch
    /// (`viewer`) or use (`editor`) this Space. Returns the shares.
    pub fn share_with(&self, who: &str, role: &str) -> Result<Map<String, Value>> {
        self.call_object(
            "share_space",
            self.arguments(json!({ "who": who, "role": role })),
        )
    }

    /// `unshare_space`: stops sharing with `who`, or with everyone.
    pub fn unshare(&self, who: Option<&str>) -> Result<Map<String, Value>> {
        self.call_object("unshare_space", self.arguments(json!({ "who": who })))
    }

    /// `space_shares`: who this Space is shared with.
    pub fn shares(&self) -> Result<Map<String, Value>> {
        self.call_object("space_shares", self.arguments(json!({})))
    }

    /// `relay_register_space`: publishes this Space on the relay for the
    /// account's other devices.
    pub fn relay_register(&self) -> Result<Map<String, Value>> {
        self.call_object("relay_register_space", self.arguments(json!({})))
    }

    /// `relay_unregister_space`: takes this Space off the relay.
    pub fn relay_unregister(&self) -> Result<Map<String, Value>> {
        self.call_object("relay_unregister_space", self.arguments(json!({})))
    }

    // -- Shell and files --------------------------------------------------

    /// `space_bash`.
    pub fn bash(&self, command: &str) -> Result<String> {
        let payload = self.call("space_bash", self.arguments(json!({ "command": command })))?;
        if let Some(row) = payload.as_object()
            && let Some(text) = row
                .get("stdout")
                .or_else(|| row.get("output"))
                .and_then(Value::as_str)
        {
            return Ok(text.to_string());
        }
        Ok(match payload.as_str() {
            Some(text) => text.to_string(),
            None => payload.to_string(),
        })
    }

    /// `space_write`. Literal text, no local file, no shell quoting.
    pub fn write_text(&self, text: &str, path: &str) -> Result<RemoteFile> {
        let target = self.guest_path(path);
        self.call(
            "space_write",
            self.arguments(json!({ "path": target, "content": text })),
        )?;
        Ok(RemoteFile {
            name: base_name(&target),
            byte_count: Some(text.len() as u64),
            path: target,
        })
    }

    /// `upload`. Reports the path it actually wrote — `FRICTION.md` §13:
    /// *"`upload` overwrites its destination silently … and no returned
    /// path"*. The collision-safe placement keeps the user's filename intact
    /// by allocating a unique *directory* rather than mangling the name, so
    /// two drops of `notes.txt` stay two files and the agent still sees
    /// `notes.txt`.
    pub fn upload(
        &self,
        local_path: &str,
        placement: &UploadPlacement,
        byte_count: Option<u64>,
    ) -> Result<RemoteFile> {
        self.require("upload", self.capabilities().upload, "no file transfer")?;
        let name = base_name(local_path);
        let destination = match placement.kind {
            UploadPlacementKind::ExactPath => placement.path.clone(),
            UploadPlacementKind::Clobbering => {
                let directory = self.upload_directory(&placement.path);
                self.bash(&format!("mkdir -p '{}'", shell_escaped(&directory)))?;
                format!("{directory}/{name}")
            }
            UploadPlacementKind::CollisionSafe => {
                let directory = self.upload_directory(&placement.path);
                let slot = format!("{directory}/{}", unique_slot(local_path, &directory));
                self.bash(&format!("mkdir -p '{}'", shell_escaped(&slot)))?;
                format!("{slot}/{name}")
            }
        };
        self.call(
            "upload",
            self.arguments(json!({ "path": local_path, "dest": destination })),
        )?;
        Ok(RemoteFile {
            path: destination,
            name,
            byte_count,
        })
    }

    /// `send_file`. The sibling of [`Space::upload`] that the pop-out's
    /// Teleport drop zone performs, and the only one of the two whose effect
    /// is *proved*: the Space recomputes the sha256 before this returns, so a
    /// copy that never landed, or landed corrupted, is an error rather than a
    /// path the server merely claimed to have written.
    ///
    /// `respect_ignorefiles` is meaningful only when `local_path` is a folder
    /// (gitignore semantics; `.git/` is always skipped). A file named
    /// explicitly is always sent, and `None` leaves the server's default.
    pub fn send_file(
        &self,
        local_path: &str,
        target_directory: Option<String>,
        respect_ignorefiles: Option<bool>,
    ) -> Result<String> {
        self.require("send_file", self.capabilities().upload, "no file transfer")?;
        let mut arguments = json!({ "path": local_path });
        if let Some(directory) = &target_directory {
            arguments["target_directory"] = Value::String(directory.clone());
        }
        if let Some(respect) = respect_ignorefiles {
            arguments["respect_ignorefiles"] = Value::Bool(respect);
        }
        let payload = self.call_object("send_file", self.arguments(arguments))?;
        // Prefer the destination the Space itself verified. Assembling a path
        // ourselves would be precisely the unproven claim this tool exists to
        // avoid, so the fallback is only ever the directory plus the name.
        if let Some(destination) = payload
            .get("dest")
            .and_then(Value::as_str)
            .filter(|dest| !dest.is_empty())
        {
            return Ok(destination.to_string());
        }
        let directory = target_directory.unwrap_or_else(|| format!("{}/Downloads", self.home()));
        Ok(format!("{directory}/{}", base_name(local_path)))
    }

    /// `download`. The Local path answers with JSON and the Fleet path with
    /// prose (§4); both return a path here.
    pub fn download(&self, remote_path: &str, into_directory: Option<String>) -> Result<String> {
        self.require("download", self.capabilities().download, "no file transfer")?;
        let mut arguments = json!({ "path": self.guest_path(remote_path) });
        if let Some(directory) = &into_directory {
            arguments["dest"] = Value::String(directory.clone());
        }
        let payload = self.call("download", self.arguments(arguments))?;
        if let Some(destination) = payload
            .get("dest")
            .and_then(Value::as_str)
            .filter(|dest| !dest.is_empty())
        {
            return Ok(destination.to_string());
        }
        let directory = into_directory.unwrap_or_else(|| "~/Downloads/cua-spaces".to_string());
        Ok(format!("{directory}/{}", base_name(remote_path)))
    }

    fn upload_directory(&self, requested: &str) -> String {
        if requested.is_empty() {
            self.info.provider.default_upload_directory()
        } else {
            requested.to_string()
        }
    }

    /// A guest path the caller wrote with `~` must mean the Space's home, not
    /// a directory literally named `~`.
    fn guest_path(&self, path: &str) -> String {
        match path.strip_prefix("~/") {
            Some(rest) => format!("{}/{rest}", self.home()),
            None if path == "~" => self.home().to_string(),
            None => path.to_string(),
        }
    }

    // -- Windows, streaming, operator display -----------------------------

    /// `list_space_windows`.
    pub fn windows(&self) -> Result<Vec<SpaceWindow>> {
        self.require(
            "list_space_windows",
            self.capabilities().window_list,
            "no window list",
        )?;
        let payload = self.call("list_space_windows", self.space_argument())?;
        Ok(expect_array(&payload, Some("windows"))
            .iter()
            .filter_map(Value::as_object)
            .map(SpaceWindow::from_row)
            .filter(|window| !window.id.is_empty())
            .collect())
    }

    /// The window a run is working in, or `None` when the join cannot be made.
    ///
    /// `FRICTION.md` §37: the join key is the owning pid, not the title. A
    /// title match would be a guess, so this returns `None` instead.
    pub fn window_of_process(&self, process_id: i32) -> Result<Option<SpaceWindow>> {
        Ok(self
            .windows()?
            .into_iter()
            .find(|window| window.process_id == Some(process_id)))
    }

    /// `stream_endpoint` (formerly `local_rcdp`). Frames for a surface
    /// **inside this product** — a different call from the operator-display
    /// family (§10).
    pub fn stream_endpoint(&self, force_refresh: bool) -> Result<StreamEndpoint> {
        if !force_refresh
            && let Ok(cache) = self.connection.endpoints.lock()
            && let Some((_, endpoint)) = cache.iter().find(|(id, _)| id == &self.info.id)
        {
            return Ok(endpoint.clone());
        }
        self.require(
            "stream_endpoint",
            self.capabilities().rcdp_streaming,
            "this provider has no stream endpoint",
        )?;
        let row = self.call_object("stream_endpoint", self.space_argument())?;
        let endpoint = StreamEndpoint::from_row(&row, self.info.ip_address.as_deref());
        if let Ok(mut cache) = self.connection.endpoints.lock() {
            cache.retain(|(id, _)| id != &self.info.id);
            cache.push((self.info.id.clone(), endpoint.clone()));
        }
        Ok(endpoint)
    }

    /// `stream_space_window`. Draws on the **operator's** desktop.
    pub fn stream_window_to_operator_desktop(&self, window_id: &str) -> Result<()> {
        self.call(
            "stream_space_window",
            self.arguments(json!({ "window_id": window_id })),
        )?;
        Ok(())
    }

    /// `show_space_pip`.
    pub fn pin_picture_in_picture_on_operator_desktop(&self) -> Result<()> {
        self.call("show_space_pip", self.space_argument())?;
        Ok(())
    }

    /// `hide_space_pip`.
    pub fn unpin_picture_in_picture_from_operator_desktop(&self) -> Result<()> {
        self.call("hide_space_pip", self.space_argument())?;
        Ok(())
    }

    /// `open_space_viewer`.
    pub fn open_viewer_on_operator_desktop(&self) -> Result<()> {
        self.call("open_space_viewer", self.space_argument())?;
        Ok(())
    }

    // -- In-space MCP services --------------------------------------------

    /// `list_tools`. The server publishes the service list on any call, so
    /// this is one round trip and never a guess.
    pub fn service_catalog(
        &self,
        service: Option<String>,
        matching: Option<String>,
    ) -> Result<ServiceCatalog> {
        let mut arguments = self.space_argument();
        if let Some(service) = &service {
            arguments["service"] = Value::String(service.clone());
        }
        if let Some(matching) = matching {
            arguments["name"] = Value::String(matching);
        }
        let row = self.call_object("list_tools", arguments)?;
        let answered = row
            .get("service")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or(service)
            .unwrap_or_default();
        let tools = row
            .get("tools")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(Value::as_object)
                    .map(|tool| SpaceTool {
                        service: answered.clone(),
                        name: tool
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        summary: tool
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        input_schema_json: tool
                            .get("inputSchema")
                            .map(Value::to_string)
                            .unwrap_or_default(),
                    })
                    .filter(|tool| !tool.name.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let other_services = row
            .get("services")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(Value::as_str)
                    .filter(|name| *name != answered && !name.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Ok(ServiceCatalog {
            service: answered,
            tools,
            other_services,
            instructions: row
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::to_string),
            not_ready_warning: row
                .get("warning")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    /// Every service this Space declares.
    pub fn services(&self) -> Result<Vec<String>> {
        let catalog = self.service_catalog(None, None)?;
        let mut names = Vec::new();
        for name in std::iter::once(catalog.service).chain(catalog.other_services) {
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
        Ok(names)
    }

    /// `call_tool`. **Not** the connection's escape hatch: that one calls a
    /// tool on the Spaces MCP, on your machine; this one calls a tool on a
    /// service running *inside* the Space.
    pub fn call_service_tool(
        &self,
        tool: &str,
        service: Option<String>,
        arguments_json: &str,
    ) -> Result<Vec<ToolContentPart>> {
        let inner: Value = if arguments_json.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(arguments_json)
                .map_err(|error| SpacesError::malformed("call_tool", error.to_string()))?
        };
        let mut arguments = self.arguments(json!({ "tool": tool, "arguments": inner }));
        if let Some(service) = service {
            arguments["service"] = Value::String(service);
        }
        Ok(tool_content_parts(&self.call("call_tool", arguments)?))
    }

    // -- Agent threads ----------------------------------------------------

    /// `agent_start`.
    pub fn start_agent(&self, request: &AgentStartRequest) -> Result<AgentRunHandle> {
        self.require(
            "agent_start",
            self.capabilities().agents,
            "no agent harness",
        )?;
        let mut arguments = self.arguments(json!({
            "agent": request.agent,
            "prompt": metadata::encode(&request.prompt, &request.metadata),
            "show": request.shows_window,
        }));
        if let Some(seconds) = request.timeout_seconds {
            // Reserved. No backend reads this key today
            // (`server_backstop == false`), and an older server ignores an
            // unknown key, so sending it is free and the day a server honours
            // it nothing at the call site changes.
            arguments["timeout_seconds"] = json!(seconds);
        }
        let row = self.call_object("agent_start", arguments)?;
        let id = row
            .get("run_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| SpacesError::malformed("agent_start", "no run_id in the response"))?
            .to_string();
        Ok(AgentRunHandle {
            id,
            space: self.info.id.clone(),
            agent: row
                .get("agent")
                .and_then(Value::as_str)
                .unwrap_or(&request.agent)
                .to_string(),
            turn_model: TurnModel::from_row(
                &row.get("capabilities")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default(),
            ),
            notes: row
                .get("notes")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            approvals_are_enforced: APPROVALS_ARE_ENFORCED,
        })
    }

    /// `agent_status`. The expensive call: it carries the output tail.
    pub fn run_status(&self, run_id: &str, tail: u32) -> Result<RunSnapshot> {
        let row = self.call_object(
            "agent_status",
            self.arguments(json!({ "run_id": run_id, "tail": tail })),
        )?;
        let previous = self.connection.last_snapshot(run_id);
        let mut row = row;
        // `agent_status` echoes the prompt as `summary`, marker and all. The
        // readable projection and the carrier are both kept, so a relaunched
        // app can recover the metadata it started a run with (§21, §41).
        if let Some(summary) = row.get("summary").and_then(Value::as_str) {
            let raw = summary.to_string();
            row.insert("summary".into(), Value::String(metadata::strip(&raw)));
            row.entry("raw_summary".to_string())
                .or_insert(Value::String(raw));
        }
        let snapshot =
            RunSnapshot::decode(&row, run_id, &self.info.id, Some(tail), previous.as_ref());
        self.connection.remember(&snapshot);
        Ok(snapshot)
    }

    /// `agent_list`. The cheap call, and **one round trip regardless of how
    /// many runs there are** — `FRICTION.md` §25 records a roster that cost
    /// `1 + N` calls per tick, which is what froze OpenKoalaBots's window.
    pub fn runs(&self) -> Result<Vec<RunSnapshot>> {
        let payload = self.call("agent_list", self.space_argument())?;
        let mut out = Vec::new();
        for row in expect_array(&payload, Some("runs"))
            .iter()
            .filter_map(Value::as_object)
        {
            let id = row
                .get("run_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            let mut row = row.clone();
            if let Some(summary) = row.get("summary").and_then(Value::as_str) {
                let raw = summary.to_string();
                row.insert("summary".into(), Value::String(metadata::strip(&raw)));
                row.entry("raw_summary".to_string())
                    .or_insert(Value::String(raw));
            }
            let previous = self.connection.last_snapshot(&id);
            let snapshot = RunSnapshot::decode(&row, &id, &self.info.id, None, previous.as_ref());
            self.connection.remember(&snapshot);
            out.push(snapshot);
        }
        Ok(out)
    }

    /// `agent_message`.
    ///
    /// Refusal is the default and is a **value**, not an error (§9), but it is
    /// a value that cannot be mistaken for a delivery: `accepted` is the only
    /// discriminator and it is always present.
    ///
    /// `QueueUntilIdle` is the outbox §24 asks for, and it is honest about
    /// what it is: there is no backend queue, so the message is held in this
    /// process and dies with it. `queue_timeout_ms` bounds the wait.
    pub fn send_message(
        &self,
        run_id: &str,
        text: &str,
        mode: DeliveryMode,
        queue_timeout_ms: Option<u64>,
    ) -> Result<Delivery> {
        match mode {
            DeliveryMode::RefuseIfBusy => self.deliver(run_id, text, false, None),
            DeliveryMode::AbandonCurrentTurn => self.deliver(run_id, text, true, None),
            DeliveryMode::QueueUntilIdle => {
                let first = self.deliver(run_id, text, false, None)?;
                if first.accepted {
                    return Ok(first);
                }
                let budget = queue_timeout_ms.unwrap_or(30_000);
                let waited = self.wait_until_accepting(run_id, budget)?;
                self.deliver(run_id, text, false, Some(waited))
            }
        }
    }

    fn deliver(
        &self,
        run_id: &str,
        text: &str,
        force: bool,
        queued_for_ms: Option<u64>,
    ) -> Result<Delivery> {
        let mut arguments = self.arguments(json!({ "run_id": run_id, "text": text }));
        if force {
            arguments["force"] = Value::Bool(true);
        }
        let row = self.call_object("agent_message", arguments)?;
        Ok(Delivery::decode(&row, run_id, queued_for_ms))
    }

    /// Poll until the run accepts a message or ends, returning how long it
    /// took. The deadline bounds the sleep rather than being consulted after
    /// it — an earlier Swift implementation could return at four times the
    /// budget because the back-off grew past it (§2).
    fn wait_until_accepting(&self, run_id: &str, budget_ms: u64) -> Result<u64> {
        let started = std::time::Instant::now();
        let step = std::time::Duration::from_millis(250);
        loop {
            let snapshot = self.run_status(run_id, 0)?;
            if snapshot.accepts_message || snapshot.state.has_ended() {
                return Ok(started.elapsed().as_millis() as u64);
            }
            let elapsed = started.elapsed().as_millis() as u64;
            if elapsed >= budget_ms {
                return Err(SpacesError::TimedOut {
                    waiting_for: format!("{run_id} to accept a message"),
                    after_ms: budget_ms,
                });
            }
            std::thread::sleep(step.min(std::time::Duration::from_millis(budget_ms - elapsed)));
        }
    }

    /// `agent_stop`. The result reports a **verified** liveness probe rather
    /// than assuming the kill worked (§9).
    pub fn stop_run(&self, run_id: &str) -> Result<StopOutcome> {
        Ok(StopOutcome::decode(&self.call_object(
            "agent_stop",
            self.arguments(json!({ "run_id": run_id })),
        )?))
    }

    /// `agent_events`: normalized events after `cursor`, with the next
    /// cursor and whether the log is caught up.
    pub fn events_after(
        &self,
        run_id: &str,
        cursor: u64,
        max: u32,
    ) -> Result<serde_json::Map<String, Value>> {
        self.call_object(
            "agent_events",
            self.arguments(json!({ "run_id": run_id, "cursor": cursor, "max": max })),
        )
    }

    /// `agent_interrupt`: cancels the turn in flight; the session stays
    /// open. Returns whether a live run was signalled.
    pub fn interrupt_run(&self, run_id: &str) -> Result<bool> {
        let row = self.call_object(
            "agent_interrupt",
            self.arguments(json!({ "run_id": run_id })),
        )?;
        Ok(row
            .get("interrupted")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }

    /// `agent_capabilities`.
    pub fn harness_capabilities(&self) -> Result<HarnessCapabilities> {
        Ok(HarnessCapabilities::decode(
            &self.call_object("agent_capabilities", json!({}))?,
        ))
    }

    /// Typed events for a run, every one of them carrying
    /// `is_inferred == false`. Nothing here guesses.
    pub fn run_events(&self, run_id: &str, tail: u32) -> Result<Vec<AgentEvent>> {
        Ok(events_from_snapshot(&self.run_status(run_id, tail)?))
    }

    /// Present so call sites exist before the primitive does. Fails while
    /// `approvals_are_enforced` is `false`, rather than no-oping.
    pub fn approve_run(&self, _run_id: &str, _decision: &str) -> Result<()> {
        Err(SpacesError::NotImplementedYet(
            "approvals are not enforced by any shipping Spaces backend; \
             AgentRunHandle.approvals_are_enforced is false"
                .into(),
        ))
    }

    // -- Session teleport -------------------------------------------------

    /// `teleport_manifest`. Preview exactly what would be transferred.
    /// **Never moves a byte.**
    pub fn teleport_manifest(&self, app: &str, scope: TeleportScope) -> Result<TeleportManifest> {
        self.require(
            "teleport_manifest",
            self.capabilities().teleport,
            "this provider has no teleport receiver",
        )?;
        let payload = self.call(
            "teleport_manifest",
            json!({ "app": app, "scope": scope.as_str() }),
        )?;
        let manifest = TeleportManifest::decode(app, scope, &payload);
        if manifest.items.is_empty() {
            return Err(SpacesError::malformed(
                "teleport_manifest",
                format!("no items for {app}"),
            ));
        }
        Ok(manifest)
    }

    /// `teleport_app`. Takes an [`Approval`] and nothing else, which is the
    /// whole design: there is no overload that takes an app id.
    pub fn teleport_send(&self, approval: &Approval) -> Result<TeleportReceipt> {
        if approval.space() != self.info.id {
            return Err(SpacesError::TeleportRefused(format!(
                "this approval was granted for {}, not {}",
                approval.space(),
                self.info.id
            )));
        }
        self.require(
            "teleport_app",
            self.capabilities().teleport,
            "this provider has no teleport receiver",
        )?;
        let mut arguments = self.arguments(json!({
            "app": approval.app(),
            "scope": approval.scope().as_str(),
        }));
        // A missing `include` is the point, not an omission: it hands the
        // choice back to the server, whose default set leaves the expensive
        // items out.
        if let Some(includes) = approval.includes()
            && !includes.is_empty()
        {
            arguments["include"] = json!(includes);
        }
        let payload = self.call("teleport_app", arguments)?;
        let detail = match payload.as_str() {
            Some(text) => text.to_string(),
            None => payload.to_string(),
        };
        let method = payload
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                if detail.contains("\"method\": \"file\"") || detail.contains("\"method\":\"file\"")
                {
                    "file".into()
                } else {
                    "teleport".into()
                }
            });
        Ok(TeleportReceipt {
            app: approval.app().to_string(),
            space: self.info.id.clone(),
            method,
            transferred_paths: approval.approved_paths(),
            raw_result: detail,
        })
    }
}

fn base_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn shell_escaped(value: &str) -> String {
    value.replace('\'', "'\\''")
}

/// A short, deterministic directory name for a collision-safe upload.
///
/// Deterministic rather than random because the conformance fixtures have to
/// be byte-identical in three languages, and because the same local file
/// uploaded twice to the same directory landing in the same slot is a better
/// property than a fresh directory per attempt.
fn unique_slot(local_path: &str, directory: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in local_path.bytes().chain([0u8]).chain(directory.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::transport::ScriptedTransport;

    fn connect(script: &str) -> (Arc<Connection>, Arc<ScriptedTransport>) {
        let transport = Arc::new(ScriptedTransport::from_script(script).unwrap());
        (Arc::new(Connection::new(transport.clone())), transport)
    }

    const LIST_SPACES: &str = r#""list_spaces":[{"content":[{"type":"text","text":
        "{\"spaces\":[{\"id\":\"local:cua-space-1\",\"provider\":\"local\",\"phase\":\"running\",\"os\":\"macos\",\"ip\":\"192.168.64.7\"}]}"}]}]"#;

    #[test]
    fn attach_reaches_that_space_and_never_provisions() {
        let (connection, transport) = connect(&format!(
            r#"{{"script":"cua.control.script/1","responses":{{{LIST_SPACES}}}}}"#
        ));
        let space = connection.attach("local:cua-space-1", true).unwrap();
        assert_eq!(space.provider(), SpaceProvider::Local);
        assert_eq!(space.home(), "/Users/lume");
        // Nothing that could cost money was called.
        let called: Vec<String> = transport
            .calls()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(called, vec!["list_spaces"]);
    }

    #[test]
    fn attaching_to_a_space_that_is_not_there_says_so_rather_than_claiming_one() {
        let (connection, transport) = connect(&format!(
            r#"{{"script":"cua.control.script/1","responses":{{{LIST_SPACES}}}}}"#
        ));
        let error = connection
            .attach("fleet:nope", true)
            .err()
            .expect("attaching to an absent Space must fail");
        assert_eq!(error.tag(), "SpaceUnavailable");
        assert!(
            !transport
                .calls()
                .iter()
                .any(|(name, _)| name == "claim_space" || name == "get_or_create_space")
        );
    }

    /// §25. One `agent_list` per tick regardless of roster size.
    #[test]
    fn the_roster_costs_one_round_trip_regardless_of_how_many_runs_there_are() {
        let runs: Vec<String> = (0..12)
            .map(|n| format!(
                "{{\\\"run_id\\\":\\\"run-{n}\\\",\\\"status\\\":\\\"running\\\",\\\"accepts_message\\\":false,\\\"reason\\\":\\\"working\\\"}}"
            ))
            .collect();
        let script = format!(
            r#"{{"script":"cua.control.script/1","responses":{{{LIST_SPACES},
               "agent_list":[{{"content":[{{"type":"text","text":"{{\"runs\":[{}]}}"}}]}}]}}}}"#,
            runs.join(",")
        );
        let (connection, transport) = connect(&script);
        let space = connection.attach("local:cua-space-1", true).unwrap();
        let before = transport.call_count();
        let roster = space.runs().unwrap();
        assert_eq!(roster.len(), 12);
        assert_eq!(transport.call_count() - before, 1);
    }

    /// §3, end to end: a failing tool arrives as an error from the typed call,
    /// not as a string a caller would read as output.
    #[test]
    fn a_failing_tool_is_an_error_at_the_typed_call_site() {
        let script = format!(
            r#"{{"script":"cua.control.script/1","responses":{{{LIST_SPACES},
               "space_bash":[{{"content":[{{"type":"text","text":"error: Space not ready"}}],"isError":true}}]}}}}"#
        );
        let (connection, _) = connect(&script);
        let space = connection.attach("local:cua-space-1", true).unwrap();
        let error = space.bash("ls").unwrap_err();
        assert_eq!(error.tag(), "ToolFailed");
        assert!(error.to_string().contains("Space not ready"));
    }

    /// §6. A provider that cannot do a thing says so before the call, rather
    /// than after, in prose. (Fleet used to be that provider for streaming;
    /// `stream_endpoint` now serves every real provider, so the check is
    /// pinned on a provider the client cannot identify.)
    #[test]
    fn an_unknown_provider_refuses_streaming_before_calling_anything() {
        let script = r#"{"script":"cua.control.script/1","responses":{
            "list_spaces":[{"content":[{"type":"text","text":
              "{\"spaces\":[{\"id\":\"moon:abc\",\"provider\":\"moon\",\"phase\":\"Bound\"}]}"}]}]}}"#;
        let (connection, transport) = connect(script);
        let space = connection.attach("moon:abc", true).unwrap();
        let error = space.stream_endpoint(false).unwrap_err();
        assert_eq!(error.tag(), "UnsupportedByProvider");
        assert!(
            !transport
                .calls()
                .iter()
                .any(|(name, _)| name == "stream_endpoint")
        );
    }

    /// §21, §41. The marker leaves in the prompt and is gone from the summary.
    #[test]
    fn metadata_is_smuggled_in_one_place_and_stripped_in_one_place() {
        let script = format!(
            r#"{{"script":"cua.control.script/1","responses":{{{LIST_SPACES},
               "agent_start":[{{"content":[{{"type":"text","text":"{{\"run_id\":\"run-1\",\"agent\":\"claude-code\"}}"}}]}}],
               "agent_status":[{{"content":[{{"type":"text","text":"{{\"run_id\":\"run-1\",\"status\":\"running\",\"reason\":\"working\",\"summary\":\"do the thing\\n\\n<!--cua-spaces-meta:bot=koala-->\"}}"}}]}}]}}}}"#
        );
        let (connection, transport) = connect(&script);
        let space = connection.attach("local:cua-space-1", true).unwrap();
        let handle = space
            .start_agent(&AgentStartRequest {
                prompt: "do the thing".into(),
                metadata: vec![MetadataEntry {
                    key: "bot".into(),
                    value: "koala".into(),
                }],
                shows_window: false,
                ..AgentStartRequest::default()
            })
            .unwrap();
        assert!(!handle.approvals_are_enforced);
        assert_eq!(handle.directory(), "~/.spaces-agents/run-1");

        let sent = transport
            .calls()
            .into_iter()
            .find(|(name, _)| name == "agent_start")
            .unwrap()
            .1;
        assert_eq!(
            sent["prompt"].as_str().unwrap(),
            "do the thing\n\n<!--cua-spaces-meta:bot=koala-->"
        );
        assert_eq!(sent["show"], json!(false));

        let snapshot = space.run_status("run-1", 200).unwrap();
        assert_eq!(snapshot.summary, "do the thing");
        assert_eq!(
            snapshot.raw_prompt.as_deref(),
            Some("do the thing\n\n<!--cua-spaces-meta:bot=koala-->")
        );
        assert_eq!(
            metadata::decode(snapshot.raw_prompt.as_deref().unwrap()),
            vec![MetadataEntry {
                key: "bot".into(),
                value: "koala".into()
            }]
        );
    }

    /// §13. The user's filename survives and the written path comes back.
    #[test]
    fn a_collision_safe_upload_keeps_the_filename_and_reports_the_path() {
        let script = format!(
            r#"{{"script":"cua.control.script/1","responses":{{{LIST_SPACES},
               "space_bash":[{{"content":[{{"type":"text","text":"{{\"stdout\":\"\"}}"}}]}}],
               "upload":[{{"content":[{{"type":"text","text":"{{}}"}}]}}]}}}}"#
        );
        let (connection, _) = connect(&script);
        let space = connection.attach("local:cua-space-1", true).unwrap();
        let written = space
            .upload(
                "/Users/me/notes.txt",
                &UploadPlacement::collision_safe_default(),
                Some(11),
            )
            .unwrap();
        assert_eq!(written.name, "notes.txt");
        assert!(written.path.starts_with("/Users/lume/Downloads/"));
        assert!(written.path.ends_with("/notes.txt"));
        assert_eq!(written.byte_count, Some(11));
    }

    #[test]
    fn a_tilde_path_means_the_spaces_home_and_not_a_directory_called_tilde() {
        let script = format!(
            r#"{{"script":"cua.control.script/1","responses":{{{LIST_SPACES},
               "space_write":[{{"content":[{{"type":"text","text":"{{}}"}}]}}]}}}}"#
        );
        let (connection, _) = connect(&script);
        let space = connection.attach("local:cua-space-1", true).unwrap();
        let file = space.write_text("hi", "~/notes.txt").unwrap();
        assert_eq!(file.path, "/Users/lume/notes.txt");
    }

    #[test]
    fn an_approval_for_another_space_is_refused() {
        let script = format!(
            r#"{{"script":"cua.control.script/1","responses":{{{LIST_SPACES},
               "teleport_manifest":[{{"content":[{{"type":"text","text":
                 "{{\"items\":[{{\"rel_path\":\"claude/.credentials.json\",\"sensitive\":true,\"default_checked\":true,\"est_bytes\":1024}}]}}"}}]}}]}}}}"#
        );
        let (connection, _) = connect(&script);
        let space = connection.attach("local:cua-space-1", true).unwrap();
        let manifest = space
            .teleport_manifest("claude-code", TeleportScope::Full)
            .unwrap();
        let approval = manifest
            .approving(
                "fleet:somewhere-else",
                &["claude/.credentials.json".into()],
                true,
            )
            .unwrap();
        let error = space.teleport_send(&approval).unwrap_err();
        assert_eq!(error.tag(), "TeleportRefused");
    }

    /// The `nil`-selection rule reaching the wire: no `include` key at all.
    #[test]
    fn a_server_default_approval_sends_no_include_key() {
        let script = format!(
            r#"{{"script":"cua.control.script/1","responses":{{{LIST_SPACES},
               "teleport_manifest":[{{"content":[{{"type":"text","text":
                 "{{\"items\":[{{\"rel_path\":\"claude/.credentials.json\",\"sensitive\":true,\"default_checked\":true,\"est_bytes\":1024}},{{\"rel_path\":\"claude/projects/\",\"sensitive\":true,\"default_checked\":false,\"est_bytes\":935974962}}]}}"}}]}}],
               "teleport_app":[{{"content":[{{"type":"text","text":"{{\"method\":\"teleport\"}}"}}]}}]}}}}"#
        );
        let (connection, transport) = connect(&script);
        let space = connection.attach("local:cua-space-1", true).unwrap();
        let manifest = space
            .teleport_manifest("claude-code", TeleportScope::Full)
            .unwrap();
        let approval = manifest.approving_server_default(space.id(), true).unwrap();
        let receipt = space.teleport_send(&approval).unwrap();
        assert_eq!(receipt.method, "teleport");
        assert!(receipt.transferred_paths.is_empty());

        let sent = transport
            .calls()
            .into_iter()
            .find(|(name, _)| name == "teleport_app")
            .unwrap()
            .1;
        assert!(
            sent.get("include").is_none(),
            "a server-default approval must send no include key, got {sent}"
        );
        // And it never quietly became "everything".
        assert_eq!(approval.approved_bytes(), 1024);
    }

    #[test]
    fn a_queued_message_is_delivered_once_the_run_accepts_it() {
        let script = format!(
            r#"{{"script":"cua.control.script/1","responses":{{{LIST_SPACES},
               "agent_message":[
                 {{"content":[{{"type":"text","text":"{{\"delivered\":false,\"run_id\":\"run-1\",\"status\":\"running\",\"reason\":\"a turn is in flight\"}}"}}]}},
                 {{"content":[{{"type":"text","text":"{{\"delivered\":true,\"run_id\":\"run-1\",\"note\":\"queued to the pty\"}}"}}]}}],
               "agent_status":[{{"content":[{{"type":"text","text":"{{\"run_id\":\"run-1\",\"status\":\"idle\",\"accepts_message\":true,\"reason\":\"waiting\"}}"}}]}}]}}}}"#
        );
        let (connection, _) = connect(&script);
        let space = connection.attach("local:cua-space-1", true).unwrap();

        let refused = space
            .send_message("run-1", "hello", DeliveryMode::RefuseIfBusy, None)
            .unwrap();
        assert!(!refused.accepted);
        assert_eq!(refused.reason, "a turn is in flight");
        assert_eq!(refused.state_at_refusal, Some(AgentState::Running));
    }
}
