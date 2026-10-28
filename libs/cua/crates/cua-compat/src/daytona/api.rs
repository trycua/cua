//! Daytona's REST API over the backend: sandboxes (create, get, list,
//! delete, start/stop/pause/archive/recover, fork, snapshot, labels,
//! intervals, public, previews), snapshots and `/config`. Features with no
//! Cua equivalent (Dockerfile builds, volumes, secrets, SSH access, network
//! allow-lists, resize, telemetry) answer with Daytona's error shape and the
//! reason.

use super::{Daytona, backend_error, error, ok_json};
use crate::auth::KeyEntry;
use crate::backend::{BackendError, CreateExtras, CreateSpec, Placement, Source};
use crate::e2b::TemplateTarget;
use crate::state::{OnTimeout, Record, now_ms};
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, RawQuery, State};
use axum::routing::{any, get, post, put};
use http::{HeaderMap, Response, StatusCode};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

pub fn routes(d: Arc<Daytona>) -> Router {
    Router::new()
        .route("/api/config", get(config))
        .route("/api/sandbox", post(super::create::create).get(list))
        .route("/api/sandbox/{id}", get(info).delete(delete))
        .route("/api/sandbox/{id}/start", post(start))
        .route("/api/sandbox/{id}/recover", post(recover))
        .route("/api/sandbox/{id}/stop", post(stop))
        .route("/api/sandbox/{id}/pause", post(pause))
        .route("/api/sandbox/{id}/archive", post(archive))
        .route("/api/sandbox/{id}/fork", post(fork))
        .route(
            "/api/sandbox/{id}/snapshot",
            post(super::snapshots::sandbox_snapshot),
        )
        .route("/api/sandbox/{id}/labels", put(labels))
        .route("/api/sandbox/{id}/public/{public}", post(public))
        .route("/api/sandbox/{id}/autostop/{n}", post(interval_autostop))
        .route("/api/sandbox/{id}/autopause/{n}", post(interval_autopause))
        .route(
            "/api/sandbox/{id}/autoarchive/{n}",
            post(interval_autoarchive),
        )
        .route(
            "/api/sandbox/{id}/autodelete/{n}",
            post(interval_autodelete),
        )
        .route("/api/sandbox/{id}/ttl/{n}", post(ttl))
        .route("/api/sandbox/{id}/last-activity", post(last_activity))
        .route("/api/sandbox/{id}/toolbox-proxy-url", get(toolbox_url))
        .route("/api/sandbox/{id}/ports/{port}/preview-url", get(preview))
        .route(
            "/api/sandbox/{id}/ports/{port}/signed-preview-url",
            get(signed_preview),
        )
        .route(
            "/api/sandbox/{id}/ports/{port}/signed-preview-url/{token}/expire",
            post(expire_signed),
        )
        .route("/api/sandbox/{id}/forks", get(forks))
        .route("/api/sandbox/{id}/parent", get(parent))
        .route("/api/sandbox/{id}/ancestors", get(ancestors))
        .route("/api/sandbox/{id}/resize", post(resize))
        .route("/api/sandbox/{id}/network-settings", post(network_settings))
        .route(
            "/api/sandbox/{id}/secrets",
            axum::routing::put(update_secrets).get(no_secrets),
        )
        .route("/api/sandbox/{id}/signing-key", get(signing_key))
        .route(
            "/api/sandbox/{id}/signing-key/rotate",
            post(rotate_signing_key),
        )
        .route(
            "/api/sandbox/{id}/telemetry/metrics",
            get(telemetry_metrics),
        )
        .route("/api/sandbox/{id}/telemetry/{*rest}", any(no_telemetry))
        .route("/api/sandbox/{id}/build-logs", get(sandbox_build_logs))
        .route(
            "/api/sandbox/{id}/build-logs-url",
            get(sandbox_build_logs_url),
        )
        .with_state(d)
}

/// Minutes Daytona's defaults use when a create omits them.
const DEFAULT_AUTOSTOP_MIN: i64 = 15;
const DEFAULT_AUTOARCHIVE_MIN: i64 = 7 * 24 * 60;

pub(super) fn rfc3339(ms: u64) -> String {
    let t = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

pub(crate) fn uuid() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::rng().fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn ext_i64(r: &Record, k: &str, d: i64) -> i64 {
    r.extra.get(k).and_then(Value::as_i64).unwrap_or(d)
}

/// The Daytona state of a record: an explicit `dtState` (set while
/// something is in flight, or when it failed), else started/stopped/
/// paused/archived from the Space's lifecycle.
pub fn state_of(r: &Record) -> String {
    if let Some(s) = r.extra.get("dtState").and_then(Value::as_str) {
        return s.to_string();
    }
    if !r.paused {
        return "started".into();
    }
    let as_ = r
        .extra
        .get("pausedAs")
        .and_then(Value::as_str)
        .unwrap_or("stopped")
        .to_string();
    if as_ == "stopped" {
        let archive = ext_i64(r, "autoArchiveInterval", DEFAULT_AUTOARCHIVE_MIN);
        let since = r
            .extra
            .get("stoppedAt")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if archive > 0 && since > 0 && since + (archive as u64) * 60_000 <= now_ms() {
            return "archived".into();
        }
    }
    as_
}

/// When Daytona would destroy the sandbox regardless of what it does:
/// `ttlMinutes` after creation, or the auto-delete interval after it
/// stopped (`autoDestroyAt` of the DTO).
pub(crate) fn auto_destroy_ms(r: &Record) -> Option<u64> {
    let mut at = r.hard_end_at_ms;
    if r.paused && r.on_timeout == OnTimeout::Kill {
        at = Some(at.map_or(r.end_at_ms?, |h| h.min(r.end_at_ms.unwrap_or(h))));
    }
    at
}

fn dt_memory_gib(r: &Record) -> u64 {
    r.memory_mb.div_ceil(1024).max(1)
}

fn dt_disk_gib(r: &Record) -> u64 {
    (r.disk_mb / 1024).max(r.extra.get("disk").and_then(Value::as_u64).unwrap_or(0))
}

/// The `Sandbox` DTO (daytona_api_client 0.225.0 `models/sandbox.py`).
pub fn sandbox_dto(d: &Daytona, r: &Record) -> Value {
    let x = &r.extra;
    let get = |k: &str| x.get(k).cloned().unwrap_or(Value::Null);
    let state = state_of(r);
    let desired = match state.as_str() {
        "destroyed" | "destroying" => "destroyed",
        "stopped" | "stopping" => "stopped",
        "archived" | "archiving" => "archived",
        "paused" | "pausing" => "paused",
        "resizing" => "resized",
        _ => "started",
    };
    let mut dto = json!({
        "id": r.id,
        "organizationId": "cua",
        "name": x.get("name").cloned().unwrap_or(json!(r.id)),
        "snapshot": r.template_id,
        "user": d.user_of(r),
        "env": r.env_vars,
        "labels": x.get("labels").cloned().unwrap_or(json!({})),
        "public": x.get("public").and_then(Value::as_bool).unwrap_or(false),
        "networkBlockAll": !r.internet,
        "kvm": x.get("kvm").and_then(Value::as_bool).unwrap_or(false),
        "networkAllowList": get("networkAllowList"),
        "domainAllowList": get("domainAllowList"),
        "outboundProxyUrl": get("outboundProxyUrl"),
        "otelEndpointOverride": get("otelEndpointOverride"),
        "target": x.get("target").cloned().unwrap_or(json!("cua")),
        "cpu": r.cpus,
        "gpu": x.get("gpu").and_then(Value::as_u64).unwrap_or(0),
        "spot": x.get("spot").and_then(Value::as_bool).unwrap_or(false),
        "memory": dt_memory_gib(r),
        "disk": dt_disk_gib(r),
        "state": state,
        "desiredState": desired,
        "errorReason": get("errorReason"),
        "recoverable": x.get("recoverable").and_then(Value::as_bool).unwrap_or(false),
        "autoStopInterval": ext_i64(r, "autoStopInterval", DEFAULT_AUTOSTOP_MIN),
        "autoPauseInterval": ext_i64(r, "autoPauseInterval", 0),
        "autoArchiveInterval": ext_i64(r, "autoArchiveInterval", DEFAULT_AUTOARCHIVE_MIN),
        "autoDeleteInterval": ext_i64(r, "autoDeleteInterval", -1),
        "autoDestroyAt": auto_destroy_ms(r).map(rfc3339),
        "queueTimeout": get("queueTimeout"),
        "volumes": x.get("volumes").cloned().unwrap_or(json!([])),
        "createdAt": rfc3339(r.started_at_ms),
        "updatedAt": rfc3339(x.get("updatedAt").and_then(Value::as_u64).unwrap_or(r.started_at_ms)),
        "lastActivityAt": rfc3339(x.get("lastActivity").and_then(Value::as_u64).unwrap_or(r.started_at_ms)),
        "sandboxClass": "cua",
        "toolboxProxyUrl": d.cfg.toolbox_url,
        "daemonVersion": "cua-compat",
        "linkedSandboxId": get("linkedSandboxId"),
    });
    if let Some(b) = x.get("buildInfo").filter(|b| !b.is_null()) {
        dto["buildInfo"] = b.clone();
    }
    dto
}

/// Records activity: the idle deadline (auto-stop or auto-pause) restarts.
pub async fn touch(d: &Daytona, r: &Record) {
    let now = now_ms();
    let last = r
        .extra
        .get("lastActivity")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if now.saturating_sub(last) < 5_000 && r.end_at_ms.is_some() {
        return;
    }
    d.store.update(&r.id, |r| apply_policy(r, now)).await;
}

/// Sets the idle deadline and what follows it from the record's intervals.
fn apply_policy(r: &mut Record, now: u64) {
    r.extra.insert("lastActivity".into(), json!(now));
    if r.paused {
        return;
    }
    let pause = ext_i64(r, "autoPauseInterval", 0);
    let stop = ext_i64(r, "autoStopInterval", DEFAULT_AUTOSTOP_MIN);
    let del = ext_i64(r, "autoDeleteInterval", -1);
    let (minutes, as_) = if pause > 0 {
        (pause, "paused")
    } else {
        (stop, "stopped")
    };
    if minutes > 0 {
        let at = now.saturating_add((minutes as u64).saturating_mul(60_000));
        r.end_at_ms = Some(at);
        r.on_timeout = OnTimeout::Pause;
        r.extra.insert("pausedAs".into(), json!(as_));
        r.extra.insert("stoppedAt".into(), json!(at));
        r.delete_after_pause_ms = (del >= 0).then(|| (del as u64).saturating_mul(60_000));
    } else {
        r.end_at_ms = None;
    }
}

pub(super) async fn authed(d: &Daytona, headers: &HeaderMap) -> Result<KeyEntry, Response<Body>> {
    d.caller(headers)
        .await
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Invalid credentials"))
}

pub(super) async fn owned(d: &Daytona, key: &KeyEntry, id: &str) -> Result<Record, Response<Body>> {
    d.find(key, id).await.ok_or_else(|| {
        error(
            StatusCode::NOT_FOUND,
            format!("Sandbox with ID or name {id} not found"),
        )
    })
}

pub(super) async fn body_json(body: Body) -> Result<Value, Response<Body>> {
    let bytes = http_body_util::BodyExt::collect(http_body_util::Limited::new(body, 1 << 20))
        .await
        .map_err(|e| error(StatusCode::BAD_REQUEST, format!("reading body: {e}")))?
        .to_bytes();
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| error(StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}")))
}

pub(super) fn string_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| match v {
                    Value::String(s) => Some((k.clone(), s.clone())),
                    Value::Null => None,
                    other => Some((k.clone(), other.to_string())),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `GET /api/config`: what the SDK reads at start (it uses `analyticsApiUrl`
/// only; the rest is the required shape).
async fn config(State(d): State<Arc<Daytona>>) -> Response<Body> {
    ok_json(json!({
        "version": "cua-compat",
        "oidc": {"issuer": "", "clientId": "", "audience": "", "provider": "cua"},
        "linkedAccountsEnabled": false,
        "announcements": {},
        "proxyTemplateUrl": format!("https://{{{{PORT}}}}-{{{{sandboxId}}}}.{}", d.cfg.preview_domain),
        "proxyToolboxUrl": d.cfg.toolbox_url,
        "defaultSnapshot": "base",
        "dashboardUrl": "",
        "maxAutoArchiveInterval": 43200,
        "maintananceMode": false,
        "environment": "cua",
        "analyticsApiUrl": null,
    }))
}

/// Resolves a `snapshot` name: registered snapshots, Cua checkpoints, image
/// references; empty is the default image.
pub(super) async fn resolve_snapshot(
    d: &Daytona,
    key: &KeyEntry,
    snapshot: &str,
) -> Result<(Source, CreateExtras), Response<Body>> {
    if snapshot.is_empty() || snapshot == "base" {
        return Ok((Source::Image(None), CreateExtras::default()));
    }
    let found = d
        .snapshots
        .lock()
        .await
        .iter()
        .find(|s| s.name == snapshot || s.id == snapshot)
        .cloned();
    if let Some(s) = &found {
        if s.state != "active" {
            return Err(error(
                StatusCode::BAD_REQUEST,
                format!("Snapshot {snapshot} is {}", s.state),
            ));
        }
        let mut extras = CreateExtras::default();
        let mut source = match &s.target {
            TemplateTarget::Image(i) => Source::Image(Some(i.clone())),
            TemplateTarget::Checkpoint(c) => Source::Checkpoint(c.clone()),
        };
        if let Some(b) = &s.build {
            let built = super::build::resolve(d, &b.dockerfile, &b.context_hashes)
                .await
                .map_err(|m| error(StatusCode::BAD_REQUEST, format!("Snapshot {snapshot}: {m}")))?;
            extras.build = built.build;
            extras.command = built.command;
            source = Source::Image(Some(built.from));
        }
        return Ok((source, extras));
    }
    match d.backend.checkpoints(&Daytona::identity(key)).await {
        Ok(cps) if cps.iter().any(|c| c.name == snapshot) => {
            return Ok((Source::Checkpoint(snapshot.into()), CreateExtras::default()));
        }
        Ok(_) => {}
        Err(e) => return Err(backend_error(e)),
    }
    if snapshot.contains('/') || snapshot.contains(':') {
        return Ok((
            Source::Image(Some(snapshot.into())),
            CreateExtras::default(),
        ));
    }
    Err(error(
        StatusCode::NOT_FOUND,
        format!(
            "Snapshot {snapshot} not found. Register it with daytona.snapshot.create, name a Cua checkpoint, or pass an image reference"
        ),
    ))
}

pub(super) async fn prepare_user(d: &Daytona, r: &Record) {
    let user = d.user_of(r);
    if !d.cfg.create_default_user || user == "root" {
        return;
    }
    let Ok(c) = d.spacesd(r).await else { return };
    let script = format!(
        "id -u {user} >/dev/null 2>&1 && exit 0; \
         (useradd -m -s /bin/bash {user} || adduser -D -s /bin/sh {user}) >/dev/null 2>&1 || exit 1; \
         if [ -d /etc/sudoers.d ]; then echo '{user} ALL=(ALL:ALL) NOPASSWD: ALL' > /etc/sudoers.d/90-{user} && chmod 440 /etc/sudoers.d/90-{user}; fi"
    );
    let driver = d.drivers.of(&r.id, &c).await;
    if let Ok(out) = crate::guest::run_as(&c, &driver, "root", &script, &[]).await
        && !out.success()
    {
        tracing::warn!(
            "compat: creating user {user} in {}: {}",
            r.id,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
}

/// A sandbox to make.
pub(super) struct New {
    pub source: Source,
    pub snapshot: String,
    pub env: BTreeMap<String, String>,
    pub internet: bool,
    pub place: Placement,
    pub extra: Map<String, Value>,
    pub held: Option<String>,
    pub extras: CreateExtras,
}

/// Makes the record and the Space. With `pending` (an image build is
/// needed) the record is returned at once in state `pending_build` and the
/// build and boot continue in the background, ending in `started` (or
/// `build_failed` with the reason), announced on the event stream.
pub(super) async fn start_sandbox(
    d: &Arc<Daytona>,
    key: &KeyEntry,
    n: New,
    pending: bool,
) -> Result<Record, BackendError> {
    let id = uuid();
    let now = now_ms();
    let name = format!("daytona-{}", &id[..18]);
    let mut extra = n.extra;
    extra.entry("name".to_string()).or_insert(json!(id));
    let ttl = extra
        .get("ttlMinutes")
        .and_then(Value::as_u64)
        .filter(|m| *m > 0);
    let mut r = Record {
        id: id.clone(),
        cua_name: name.clone(),
        key_id: key.id(),
        identity_daemon: key.daemon.clone(),
        identity_token: key.token.clone(),
        template_id: n.snapshot,
        alias: None,
        metadata: BTreeMap::new(),
        env_vars: n.env.clone(),
        access_token: crate::auth::new_token(),
        traffic_token: Some(crate::auth::new_token()),
        started_at_ms: now,
        end_at_ms: None,
        on_timeout: OnTimeout::Pause,
        delete_after_pause_ms: None,
        hard_end_at_ms: ttl.map(|m| now + m * 60_000),
        auto_resume: false,
        paused: false,
        internet: n.internet,
        cpus: n.place.cpus.unwrap_or(2),
        memory_mb: n.place.memory_mb.unwrap_or(2048),
        disk_mb: n.extras.disk_gb.map_or(0, |g| u64::from(g) * 1024),
        held_checkpoint: n.held.clone(),
        extra,
    };
    let spec = CreateSpec {
        name,
        source: n.source.clone(),
        env: n.env.clone().into_iter().collect(),
        internet: n.internet,
        place: n.place.clone(),
    };
    let from_image = matches!(n.source, Source::Image(_));
    if pending {
        r.extra.insert("dtState".into(), json!("pending_build"));
        d.store.insert(r.clone()).await;
        let log = super::build::log_for(d, &r.id).await;
        log.line("Building the image layers ...").await;
        let _ = d.events.send((
            r.key_id.clone(),
            "sandbox.created".into(),
            json!({"sandbox": sandbox_dto(d, &r)}),
        ));
        let (d2, key2, id2) = (d.clone(), key.clone(), r.id.clone());
        tokio::spawn(async move {
            let res = d2
                .backend
                .create_with(&Daytona::identity(&key2), spec, n.extras)
                .await;
            let log = super::build::log_for(&d2, &id2).await;
            match res {
                Ok(created) => {
                    log.line("Image built; starting the sandbox").await;
                    d2.store
                        .update(&id2, |r| {
                            r.cua_name = created.name.clone();
                            r.cpus = created.cpus;
                            r.memory_mb = created.memory_mb;
                            if created.disk_mb > 0 {
                                r.disk_mb = created.disk_mb;
                            }
                        })
                        .await;
                    // The pending-build marker stays through the start: a
                    // failed start keeps the record (state `error`).
                    if let Some(r) = d2.store.get(&id2).await {
                        if from_image {
                            prepare_user(&d2, &r).await;
                        }
                        let mounts = mounts_of(&d2, &key2, &r).await;
                        let kvm = r.extra.get("kvm").and_then(Value::as_bool) == Some(true);
                        match finish_create(&d2, &key2, r.clone(), &mounts, kvm).await {
                            Ok(r) => {
                                d2.store
                                    .update(&r.id, |r| {
                                        r.extra.remove("dtState");
                                    })
                                    .await;
                                let r = d2.store.get(&r.id).await.unwrap_or(r);
                                log.line("Sandbox started").await;
                                apply_policy_now(&d2, &r).await;
                                d2.announce(&r, "pending_build").await;
                            }
                            Err(_) => {
                                // finish_create marked the failure.
                                if let Some(r) = d2.store.get(&id2).await {
                                    d2.announce(&r, "pending_build").await;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    log.line(&format!("Build failed: {e}")).await;
                    d2.store
                        .update(&id2, |r| {
                            r.extra.insert("dtState".into(), json!("build_failed"));
                            r.extra.insert("errorReason".into(), json!(e.to_string()));
                        })
                        .await;
                    if let Some(r) = d2.store.get(&id2).await {
                        d2.announce(&r, "pending_build").await;
                    }
                }
            }
            log.finish().await;
        });
        return Ok(r);
    }
    let created = d
        .backend
        .create_with(&Daytona::identity(key), spec, n.extras)
        .await?;
    r.cua_name = created.name;
    r.cpus = created.cpus;
    r.memory_mb = created.memory_mb;
    if created.disk_mb > 0 {
        r.disk_mb = created.disk_mb;
    }
    apply_policy(&mut r, now);
    d.store.insert(r.clone()).await;
    if from_image {
        prepare_user(d, &r).await;
    }
    Ok(r)
}

async fn apply_policy_now(d: &Daytona, r: &Record) {
    let now = now_ms();
    d.store.update(&r.id, |r| apply_policy(r, now)).await;
}

/// The volume mounts a record asked for, resolved again.
async fn mounts_of(
    d: &Daytona,
    key: &KeyEntry,
    r: &Record,
) -> Vec<(super::stores::VolumeEntry, super::volumes::Mount)> {
    // Stored form is `{volumeId, mountPath, subpath?}`.
    let wanted = super::volumes::parse_mounts(r.extra.get("volumes")).unwrap_or_default();
    super::volumes::resolve_mounts(d, key, &wanted)
        .await
        .unwrap_or_default()
}

/// What happens after the Space is up: volumes mounted, KVM checked. A
/// failure deletes the Space and the record (a create that fails leaves
/// nothing behind) and is returned as the client's error.
pub(super) async fn finish_create(
    d: &Arc<Daytona>,
    _key: &KeyEntry,
    r: Record,
    mounts: &[(super::stores::VolumeEntry, super::volumes::Mount)],
    kvm: bool,
) -> Result<Record, Response<Body>> {
    let fail = |msg: String, code: StatusCode| {
        let d = d.clone();
        let r = r.clone();
        async move {
            let was_build = r.extra.contains_key("dtState");
            if was_build {
                d.store
                    .update(&r.id, |r| {
                        r.extra.insert("dtState".into(), json!("error"));
                        r.extra.insert("errorReason".into(), json!(msg.clone()));
                    })
                    .await;
                d.backend.delete(&r.identity(), &r.cua_name).await.ok();
            } else {
                d.kill(&r.id).await;
            }
            error(code, msg)
        }
    };
    if kvm {
        match d.spacesd(&r).await {
            Ok(c) => {
                let driver = d.drivers.of(&r.id, &c).await;
                match crate::guest::run_as(&c, &driver, "root", "test -e /dev/kvm", &[]).await {
                    Ok(o) if o.success() => {}
                    _ => {
                        return Err(fail(
                            "kvm was requested but this Space has no /dev/kvm (nested virtualization is not available on its host)".into(),
                            StatusCode::BAD_REQUEST,
                        )
                        .await);
                    }
                }
            }
            Err(e) => return Err(fail(e.to_string(), StatusCode::BAD_GATEWAY).await),
        }
    }
    // The disk the guest really has (Daytona reports the disk GiB).
    if r.disk_mb == 0
        && let Ok(c) = d.spacesd(&r).await
        && let Ok(m) = c.metrics().await
        && m.disk_total_bytes > 0
    {
        let mb = m.disk_total_bytes / (1024 * 1024);
        d.store.update(&r.id, |r| r.disk_mb = mb).await;
    }
    if let Some(policy) = egress_of(&r) {
        match d.spacesd(&r).await {
            Ok(c) => {
                if let Err(e) = c.set_egress(&policy).await {
                    return Err(fail(
                        format!("Applying the network policy failed: {e}"),
                        StatusCode::BAD_REQUEST,
                    )
                    .await);
                }
            }
            Err(e) => return Err(fail(e.to_string(), StatusCode::BAD_GATEWAY).await),
        }
    }
    if let Err(resp) = super::volumes::mount_all(d, &r, mounts).await {
        let (status, msg) = response_parts(resp).await;
        return Err(fail(msg, status).await);
    }
    let r = d.store.get(&r.id).await.unwrap_or(r);
    let _ = d.events.send((
        r.key_id.clone(),
        "sandbox.created".into(),
        json!({"sandbox": sandbox_dto(d, &r)}),
    ));
    Ok(r)
}

/// The egress policy a record's network options ask for (`None`: none).
pub(crate) fn egress_of(r: &Record) -> Option<cua_spacesd_client::EgressPolicy> {
    let list = |k: &str| -> Vec<String> {
        r.extra
            .get(k)
            .and_then(Value::as_str)
            .map(|s| {
                s.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    let (cidrs, domains) = (list("networkAllowList"), list("domainAllowList"));
    if cidrs.is_empty() && domains.is_empty() {
        return None;
    }
    Some(cua_spacesd_client::EgressPolicy {
        block_all: false,
        allow_cidrs: cidrs,
        allow_domains: domains,
    })
}

async fn response_parts(r: Response<Body>) -> (StatusCode, String) {
    let status = r.status();
    let bytes = http_body_util::BodyExt::collect(r.into_body())
        .await
        .map(|c| c.to_bytes())
        .unwrap_or_default();
    let msg = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| String::from_utf8_lossy(&bytes).into_owned());
    (status, msg)
}

/// `GET /api/sandbox/{idOrName}`.
async fn info(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    match owned(&d, &key, &id).await {
        Ok(r) => {
            if d.check_gone(&r).await {
                return error(
                    StatusCode::NOT_FOUND,
                    format!("Sandbox with ID or name {id} not found"),
                );
            }
            ok_json(sandbox_dto(&d, &r))
        }
        Err(e) => e,
    }
}

/// A parsed `GET /sandbox` query (generated client `list_sandboxes`,
/// daytona_api_client 0.225.0: `sandbox_api.py`).
#[derive(Default, Debug, PartialEq)]
pub(crate) struct ListQuery {
    pub cursor: Option<String>,
    pub limit: Option<usize>,
    pub id: Option<String>,
    pub name: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub states: Vec<String>,
    pub snapshots: Vec<String>,
    pub regions: Vec<String>,
    pub min_cpu: Option<f64>,
    pub max_cpu: Option<f64>,
    pub min_mem: Option<f64>,
    pub max_mem: Option<f64>,
    pub min_disk: Option<f64>,
    pub max_disk: Option<f64>,
    pub min_gpu: Option<f64>,
    pub max_gpu: Option<f64>,
    pub is_public: Option<bool>,
    pub is_recoverable: Option<bool>,
    pub created_after: Option<u64>,
    pub created_before: Option<u64>,
    pub activity_after: Option<u64>,
    pub activity_before: Option<u64>,
    pub destroy_after: Option<u64>,
    pub destroy_before: Option<u64>,
    pub include_errored_deleted: bool,
    pub sort: Option<String>,
    pub order: Option<String>,
}

fn parse_time_ms(v: &str) -> Option<u64> {
    let t = time::OffsetDateTime::parse(v, &time::format_description::well_known::Rfc3339).ok()?;
    u64::try_from(t.unix_timestamp_nanos() / 1_000_000).ok()
}

impl ListQuery {
    /// Repeated keys and comma-separated lists both mean a list (the
    /// generated client sends `states=a&states=b` or `states=a,b`).
    pub(crate) fn parse(raw: &str) -> Result<ListQuery, String> {
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(raw.as_bytes())
            .into_owned()
            .collect();
        let one = |k: &str| pairs.iter().find(|(pk, _)| pk == k).map(|(_, v)| v.clone());
        let many = |k: &str| -> Vec<String> {
            pairs
                .iter()
                .filter(|(pk, _)| pk == k)
                .flat_map(|(_, v)| {
                    v.split(',')
                        .map(|x| x.trim().to_string())
                        .collect::<Vec<_>>()
                })
                .filter(|x| !x.is_empty())
                .collect()
        };
        let num = |k: &str| -> Result<Option<f64>, String> {
            one(k)
                .map(|v| {
                    v.parse::<f64>()
                        .map_err(|_| format!("{k} must be a number"))
                })
                .transpose()
        };
        let flag = |k: &str| -> Result<Option<bool>, String> {
            one(k)
                .map(|v| match v.as_str() {
                    "true" => Ok(true),
                    "false" => Ok(false),
                    _ => Err(format!("{k} must be a boolean value")),
                })
                .transpose()
        };
        let when = |k: &str| -> Result<Option<u64>, String> {
            one(k)
                .map(|v| parse_time_ms(&v).ok_or_else(|| format!("{k} must be an ISO 8601 date")))
                .transpose()
        };
        let limit = match one("limit") {
            Some(l) => {
                let n: i64 = l
                    .parse()
                    .map_err(|_| "limit must be an integer".to_string())?;
                if !(1..=200).contains(&n) {
                    return Err("limit must not be greater than 200".into());
                }
                Some(n as usize)
            }
            None => None,
        };
        let labels = match one("labels") {
            Some(l) if !l.is_empty() => {
                let v: Value = serde_json::from_str(&l)
                    .map_err(|_| "labels must be a JSON object".to_string())?;
                if !v.is_object() {
                    return Err("labels must be a JSON object".into());
                }
                string_map(Some(&v))
            }
            _ => BTreeMap::new(),
        };
        let sort = one("sort");
        if let Some(s) = &sort
            && ![
                "name",
                "cpu",
                "memoryGib",
                "diskGib",
                "lastActivityAt",
                "createdAt",
            ]
            .contains(&s.as_str())
        {
            return Err(format!(
                "sort must be one of: name, cpu, memoryGib, diskGib, lastActivityAt, createdAt (got {s})"
            ));
        }
        let order = one("order");
        if let Some(o) = &order
            && !["asc", "desc"].contains(&o.as_str())
        {
            return Err("order must be one of: asc, desc".into());
        }
        let states = many("states");
        const STATES: [&str; 22] = [
            "creating",
            "restoring",
            "destroyed",
            "destroying",
            "started",
            "stopped",
            "starting",
            "stopping",
            "error",
            "build_failed",
            "pending_build",
            "building_snapshot",
            "unknown",
            "pulling_snapshot",
            "archived",
            "archiving",
            "resizing",
            "snapshotting",
            "forking",
            "pausing",
            "paused",
            "resuming",
        ];
        if let Some(bad) = states.iter().find(|s| !STATES.contains(&s.as_str())) {
            return Err(format!(
                "each value in states must be one of the following values: {}  (got {bad})",
                STATES.join(", ")
            ));
        }
        Ok(ListQuery {
            cursor: one("cursor").filter(|c| !c.is_empty()),
            limit,
            id: one("id")
                .filter(|s| !s.is_empty())
                .map(|s| s.to_lowercase()),
            name: one("name")
                .filter(|s| !s.is_empty())
                .map(|s| s.to_lowercase()),
            labels,
            states,
            snapshots: many("snapshots"),
            regions: many("regionIds"),
            min_cpu: num("minCpu")?,
            max_cpu: num("maxCpu")?,
            min_mem: num("minMemoryGiB")?,
            max_mem: num("maxMemoryGiB")?,
            min_disk: num("minDiskGiB")?,
            max_disk: num("maxDiskGiB")?,
            min_gpu: num("minGpu")?,
            max_gpu: num("maxGpu")?,
            is_public: flag("isPublic")?,
            is_recoverable: flag("isRecoverable")?,
            created_after: when("createdAtAfter")?,
            created_before: when("createdAtBefore")?,
            activity_after: when("lastEventAfter")?,
            activity_before: when("lastEventBefore")?,
            destroy_after: when("autoDestroyAtAfter")?,
            destroy_before: when("autoDestroyAtBefore")?,
            include_errored_deleted: flag("includeErroredDeleted")?.unwrap_or(false),
            sort,
            order,
        })
    }
}

/// The facts of a sandbox a list query filters and sorts on.
pub(crate) struct Facts {
    pub id: String,
    pub name: String,
    pub labels: BTreeMap<String, String>,
    pub state: String,
    pub snapshot: String,
    pub target: String,
    pub cpu: u64,
    pub mem_gib: u64,
    pub disk_gib: u64,
    pub gpu: u64,
    pub public: bool,
    pub recoverable: bool,
    pub created_ms: u64,
    pub activity_ms: u64,
    pub destroy_ms: Option<u64>,
}

impl ListQuery {
    pub(crate) fn matches(&self, f: &Facts) -> bool {
        let within = |v: u64, lo: Option<f64>, hi: Option<f64>| {
            lo.is_none_or(|l| v as f64 >= l) && hi.is_none_or(|h| v as f64 <= h)
        };
        let ts = |v: u64, after: Option<u64>, before: Option<u64>| {
            after.is_none_or(|a| v > a) && before.is_none_or(|b| v < b)
        };
        self.labels.iter().all(|(k, v)| f.labels.get(k) == Some(v))
            && (self.states.is_empty() || self.states.contains(&f.state))
            && (self.snapshots.is_empty() || self.snapshots.contains(&f.snapshot))
            && (self.regions.is_empty() || self.regions.contains(&f.target))
            && self
                .id
                .as_ref()
                .is_none_or(|p| f.id.to_lowercase().starts_with(p.as_str()))
            && self
                .name
                .as_ref()
                .is_none_or(|p| f.name.to_lowercase().starts_with(p.as_str()))
            && within(f.cpu, self.min_cpu, self.max_cpu)
            && within(f.mem_gib, self.min_mem, self.max_mem)
            && within(f.disk_gib, self.min_disk, self.max_disk)
            && within(f.gpu, self.min_gpu, self.max_gpu)
            && self.is_public.is_none_or(|p| p == f.public)
            && self.is_recoverable.is_none_or(|p| p == f.recoverable)
            && ts(f.created_ms, self.created_after, self.created_before)
            && ts(f.activity_ms, self.activity_after, self.activity_before)
            && (self.destroy_after.is_none() && self.destroy_before.is_none()
                || f.destroy_ms
                    .is_some_and(|d| ts(d, self.destroy_after, self.destroy_before)))
            && (self.include_errored_deleted || f.state != "destroyed")
    }

    pub(crate) fn sort(&self, items: &mut [(Facts, Value)]) {
        let key = self.sort.as_deref().unwrap_or("createdAt");
        items.sort_by(|(a, _), (b, _)| {
            let o = match key {
                "name" => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                "cpu" => a.cpu.cmp(&b.cpu),
                "memoryGib" => a.mem_gib.cmp(&b.mem_gib),
                "diskGib" => a.disk_gib.cmp(&b.disk_gib),
                "lastActivityAt" => a.activity_ms.cmp(&b.activity_ms),
                _ => a.created_ms.cmp(&b.created_ms),
            }
            .then_with(|| a.id.cmp(&b.id));
            if self.order.as_deref() == Some("desc") {
                o.reverse()
            } else {
                o
            }
        });
    }
}

fn facts_of(d: &Daytona, r: &Record) -> Facts {
    let dto = sandbox_dto(d, r);
    let num = |k: &str| dto[k].as_u64().unwrap_or(0);
    Facts {
        id: r.id.clone(),
        name: dto["name"].as_str().unwrap_or("").to_string(),
        labels: string_map(r.extra.get("labels")),
        state: state_of(r),
        snapshot: r.template_id.clone(),
        target: dto["target"].as_str().unwrap_or("").to_string(),
        cpu: num("cpu"),
        mem_gib: num("memory"),
        disk_gib: num("disk"),
        gpu: num("gpu"),
        public: dto["public"].as_bool().unwrap_or(false),
        recoverable: dto["recoverable"].as_bool().unwrap_or(false),
        created_ms: r.started_at_ms,
        activity_ms: r
            .extra
            .get("lastActivity")
            .and_then(Value::as_u64)
            .unwrap_or(r.started_at_ms),
        destroy_ms: auto_destroy_ms(r),
    }
}

/// `GET /api/sandbox`: filters, sort and an opaque cursor
/// (`PaginatedSandboxes`/`ListSandboxesResponse`: `items`, `nextCursor`).
async fn list(
    State(d): State<Arc<Daytona>>,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let q = match ListQuery::parse(q.as_deref().unwrap_or("")) {
        Ok(q) => q,
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    let kid = key.id();
    let mut items: Vec<(Facts, Value)> = Vec::new();
    for r in d.store.all().await {
        if r.key_id != kid {
            continue;
        }
        let f = facts_of(&d, &r);
        if q.matches(&f) {
            items.push((f, sandbox_dto(&d, &r)));
        }
    }
    q.sort(&mut items);
    let start: usize = match &q.cursor {
        Some(c) => match decode_cursor(c) {
            Some(n) => n,
            None => return error(StatusCode::BAD_REQUEST, "Invalid cursor"),
        },
        None => 0,
    };
    let limit = q.limit.unwrap_or(100);
    let page: Vec<Value> = items
        .iter()
        .skip(start)
        .take(limit)
        .map(|(_, v)| v.clone())
        .collect();
    let next = (start + limit < items.len()).then(|| encode_cursor(start + limit));
    ok_json(json!({"items": page, "nextCursor": next}))
}

fn encode_cursor(n: usize) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("o:{n}"))
}

fn decode_cursor(c: &str) -> Option<usize> {
    use base64::Engine;
    let b = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(c)
        .ok()?;
    String::from_utf8(b).ok()?.strip_prefix("o:")?.parse().ok()
}

/// `DELETE /api/sandbox/{idOrName}`.
async fn delete(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&d, &key, &id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let old = state_of(&r);
    d.kill(&r.id).await;
    let mut dto = sandbox_dto(&d, &r);
    dto["state"] = json!("destroyed");
    dto["desiredState"] = json!("destroyed");
    let _ = d.events.send((
        r.key_id.clone(),
        "sandbox.state.updated".into(),
        json!({"sandbox": dto.clone(), "oldState": old, "newState": "destroyed"}),
    ));
    ok_json(dto)
}

/// A lifecycle request on a sandbox.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Op {
    Start,
    Stop,
    Pause,
    Archive,
    Recover,
}

/// States in which a lifecycle call is refused because something else is
/// in flight.
pub(crate) fn busy(state: &str) -> bool {
    matches!(
        state,
        "creating"
            | "pending_build"
            | "building_snapshot"
            | "pulling_snapshot"
            | "starting"
            | "stopping"
            | "archiving"
            | "resizing"
            | "snapshotting"
            | "forking"
            | "pausing"
            | "resuming"
            | "destroying"
            | "restoring"
    )
}

/// What an [`Op`] does to a sandbox in `state`: the state it ends in, or the
/// refusal. `Ok(None)` is a call with nothing to do (already there).
pub(crate) fn plan_op(
    op: Op,
    state: &str,
    recoverable: bool,
) -> Result<Option<&'static str>, String> {
    if state == "destroyed" || state == "destroying" {
        return Err("Sandbox is destroyed".into());
    }
    if busy(state) {
        return Err(format!(
            "Sandbox is {state}: wait for the state change to finish"
        ));
    }
    match (op, state) {
        (Op::Start, "started") => Ok(None),
        (Op::Start, "stopped" | "archived" | "paused") => Ok(Some("started")),
        (Op::Start, "error" | "build_failed") => {
            Err("Sandbox is in an error state: recover it, or delete it".into())
        }
        (Op::Stop, "stopped") => Ok(None),
        (Op::Stop, "started" | "paused") => Ok(Some("stopped")),
        (Op::Stop, "archived") => Ok(None),
        (Op::Pause, "paused") => Ok(None),
        (Op::Pause, "started") => Ok(Some("paused")),
        (Op::Pause, _) => Err(format!(
            "Sandbox must be started to be paused (state {state})"
        )),
        (Op::Archive, "archived") => Ok(None),
        (Op::Archive, "stopped") => Ok(Some("archived")),
        (Op::Archive, _) => Err(format!(
            "Sandbox must be stopped before archiving (state {state})"
        )),
        (Op::Recover, "error") if recoverable => Ok(Some("started")),
        (Op::Recover, _) => Err(format!(
            "Sandbox is not in a recoverable state (state {state})"
        )),
        (Op::Stop, other) | (Op::Start, other) => {
            Err(format!("Sandbox cannot do that in state {other}"))
        }
    }
}

/// Runs a lifecycle [`Op`]: suspends or resumes the Space and records how
/// the sandbox shows (`stopped`, `paused`, `archived`, `started`).
async fn transition(d: &Daytona, key: &KeyEntry, id: &str, op: Op, force: bool) -> Response<Body> {
    let r = match owned(d, key, id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let old = state_of(&r);
    let recoverable = r.extra.get("recoverable").and_then(Value::as_bool) == Some(true);
    let to = match plan_op(op, &old, recoverable) {
        Ok(None) => return ok_json(sandbox_dto(d, &r)),
        Ok(Some(to)) => to,
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    let _ = force; // a Cua suspend is already abrupt for the guest: SIGKILL vs SIGTERM does not apply
    if to == "started" {
        let res = if op == Op::Recover {
            d.backend.restart(&r.identity(), &r.cua_name).await
        } else if r.paused {
            d.backend.resume(&r.identity(), &r.cua_name).await
        } else {
            Ok(())
        };
        if let Err(e) = res {
            return backend_error(e);
        }
        d.forget(&r.id).await;
        let now = now_ms();
        d.store
            .update(&r.id, |r| {
                r.paused = false;
                r.on_timeout = OnTimeout::Pause;
                for k in ["stoppedAt", "dtState", "errorReason"] {
                    r.extra.remove(k);
                }
                r.extra.insert("updatedAt".into(), json!(now));
                apply_policy(r, now);
            })
            .await;
    } else {
        // "stopped" / "paused" suspend the Space; "archived" is a stopped
        // sandbox (already suspended) that has been told to stay so.
        if !r.paused {
            if let Err(e) = d.backend.suspend(&r.identity(), &r.cua_name).await {
                return backend_error(e);
            }
            d.forget(&r.id).await;
        }
        let now = now_ms();
        let del = ext_i64(&r, "autoDeleteInterval", -1);
        d.store
            .update(&r.id, |r| {
                r.paused = true;
                r.extra.insert("pausedAs".into(), json!(to));
                if to != "archived" || !r.extra.contains_key("stoppedAt") {
                    r.extra.insert("stoppedAt".into(), json!(now));
                }
                r.extra.insert("updatedAt".into(), json!(now));
                if del >= 0 && to == "stopped" {
                    r.end_at_ms = Some(now + del as u64 * 60_000);
                    r.on_timeout = OnTimeout::Kill;
                } else if to == "archived" {
                    // Archived sandboxes keep their auto-delete clock.
                } else {
                    r.end_at_ms = None;
                }
            })
            .await;
    }
    let r = d.store.get(&r.id).await.unwrap_or(r);
    d.announce(&r, &old).await;
    ok_json(sandbox_dto(d, &r))
}

#[derive(serde::Deserialize, Default)]
struct StopQuery {
    force: Option<bool>,
}

macro_rules! lifecycle {
    ($name:ident, $op:expr) => {
        async fn $name(
            State(d): State<Arc<Daytona>>,
            Path(id): Path<String>,
            headers: HeaderMap,
            Query(q): Query<StopQuery>,
        ) -> Response<Body> {
            let key = match authed(&d, &headers).await {
                Ok(k) => k,
                Err(r) => return r,
            };
            transition(&d, &key, &id, $op, q.force.unwrap_or(false)).await
        }
    };
}
lifecycle!(start, Op::Start);
lifecycle!(stop, Op::Stop);
lifecycle!(pause, Op::Pause);
lifecycle!(archive, Op::Archive);
lifecycle!(recover, Op::Recover);

/// `POST /api/sandbox/{id}/fork` `{name?}`: a new sandbox from the live
/// one (a checkpoint held until its last fork is gone).
async fn fork(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&d, &key, &id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let cp = format!("fork-{}-{}", &r.id[..8], &crate::auth::new_token()[..8]);
    if let Err(e) = d.backend.checkpoint(&r.identity(), &r.cua_name, &cp).await {
        return backend_error(e);
    }
    let mut extra = Map::new();
    for k in [
        "user",
        "public",
        "target",
        "labels",
        "autoStopInterval",
        "autoPauseInterval",
        "autoArchiveInterval",
        "autoDeleteInterval",
    ] {
        if let Some(v) = r.extra.get(k) {
            extra.insert(k.into(), v.clone());
        }
    }
    if let Some(n) = b.get("name").and_then(Value::as_str) {
        extra.insert("name".into(), json!(n));
    }
    extra.insert("parentId".into(), json!(r.id));
    let labels = string_map(r.extra.get("labels"));
    let n = New {
        source: Source::Checkpoint(cp.clone()),
        snapshot: r.template_id.clone(),
        env: r.env_vars.clone(),
        internet: r.internet,
        place: Placement {
            on: labels.get("cua.on").cloned(),
            kind: labels.get("cua.kind").cloned(),
            cpus: None,
            memory_mb: None,
            // Computer use is part of a Daytona sandbox: the desktop is on unless
            // `cua.desktop=false` trades it for a faster boot.
            desktop: labels.get("cua.desktop").is_none_or(|v| v != "false"),
        },
        extra,
        held: Some(cp.clone()),
        extras: CreateExtras::default(),
    };
    match start_sandbox(&d, &key, n, false).await {
        Ok(f) => ok_json(sandbox_dto(&d, &f)),
        Err(e) => {
            let _ = d.backend.delete_checkpoint(&r.identity(), &cp).await;
            backend_error(e)
        }
    }
}

/// `PUT /api/sandbox/{id}/labels` `{labels}`.
async fn labels(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&d, &key, &id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let labels = string_map(b.get("labels"));
    d.store
        .update(&r.id, |r| {
            r.extra.insert("labels".into(), json!(labels));
        })
        .await;
    ok_json(json!({"labels": labels}))
}

async fn set_field(
    d: &Daytona,
    headers: &HeaderMap,
    id: &str,
    f: impl FnOnce(&mut Record),
) -> Response<Body> {
    let key = match authed(d, headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(d, &key, id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let now = now_ms();
    d.store
        .update(&r.id, |r| {
            f(r);
            r.extra.insert("updatedAt".into(), json!(now));
            apply_policy(r, now);
        })
        .await;
    let r = d.store.get(&r.id).await.unwrap_or(r);
    ok_json(sandbox_dto(d, &r))
}

async fn public(
    State(d): State<Arc<Daytona>>,
    Path((id, public)): Path<(String, bool)>,
    headers: HeaderMap,
) -> Response<Body> {
    set_field(&d, &headers, &id, |r| {
        r.extra.insert("public".into(), json!(public));
    })
    .await
}

macro_rules! interval {
    ($name:ident, $field:expr) => {
        async fn $name(
            State(d): State<Arc<Daytona>>,
            Path((id, n)): Path<(String, i64)>,
            headers: HeaderMap,
        ) -> Response<Body> {
            set_field(&d, &headers, &id, |r| {
                r.extra.insert($field.into(), json!(n));
            })
            .await
        }
    };
}
interval!(interval_autostop, "autoStopInterval");
interval!(interval_autopause, "autoPauseInterval");
interval!(interval_autoarchive, "autoArchiveInterval");
interval!(interval_autodelete, "autoDeleteInterval");

async fn ttl(
    State(d): State<Arc<Daytona>>,
    Path((id, minutes)): Path<(String, u64)>,
    headers: HeaderMap,
) -> Response<Body> {
    set_field(&d, &headers, &id, |r| {
        r.extra.insert("ttlMinutes".into(), json!(minutes));
        r.hard_end_at_ms = (minutes > 0).then(|| now_ms() + minutes * 60_000);
    })
    .await
}

async fn last_activity(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    match owned(&d, &key, &id).await {
        Ok(r) => {
            d.store.update(&r.id, |r| apply_policy(r, now_ms())).await;
            Response::builder()
                .status(StatusCode::CREATED)
                .body(Body::empty())
                .expect("response")
        }
        Err(e) => e,
    }
}

async fn toolbox_url(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    match owned(&d, &key, &id).await {
        Ok(_) => ok_json(json!({"url": d.cfg.toolbox_url})),
        Err(e) => e,
    }
}

fn preview_url(d: &Daytona, label: &str, port: u16) -> String {
    format!("https://{port}-{label}.{}", d.cfg.preview_domain)
}

/// `GET .../ports/{port}/preview-url`: `{sandboxId, url, token}`; send the
/// token as `x-daytona-preview-token` unless the sandbox is public.
async fn preview(
    State(d): State<Arc<Daytona>>,
    Path((id, port)): Path<(String, u16)>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    match owned(&d, &key, &id).await {
        Ok(r) => ok_json(json!({
            "sandboxId": r.id,
            "url": preview_url(&d, &r.id, port),
            "token": r.traffic_token.clone().unwrap_or_default(),
        })),
        Err(e) => e,
    }
}

#[derive(serde::Deserialize, Default)]
struct SignedQuery {
    #[serde(rename = "expiresInSeconds")]
    expires_in_seconds: Option<u64>,
}

/// `GET .../signed-preview-url?expiresInSeconds=`: the token is in the
/// host name, so the URL needs no header.
async fn signed_preview(
    State(d): State<Arc<Daytona>>,
    Path((id, port)): Path<(String, u16)>,
    headers: HeaderMap,
    Query(q): Query<SignedQuery>,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&d, &key, &id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let token = crate::auth::new_token()[..32].to_string();
    let exp = now_ms() + q.expires_in_seconds.unwrap_or(60).max(1) * 1000;
    d.signed
        .lock()
        .await
        .insert(token.clone(), (r.id.clone(), port, exp));
    ok_json(json!({
        "sandboxId": r.id,
        "port": port,
        "token": token,
        "url": preview_url(&d, &token, port),
    }))
}

async fn expire_signed(
    State(d): State<Arc<Daytona>>,
    Path((id, _port, token)): Path<(String, u16, String)>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    if let Err(e) = owned(&d, &key, &id).await {
        return e;
    }
    d.signed.lock().await.remove(&token);
    Response::builder()
        .status(StatusCode::OK)
        .body(Body::empty())
        .expect("response")
}

async fn lineage(d: &Daytona, key: &KeyEntry, r: &Record) -> Vec<Record> {
    let mut out = Vec::new();
    let mut cur = r
        .extra
        .get("parentId")
        .and_then(Value::as_str)
        .map(str::to_string);
    while let Some(p) = cur {
        match d.find(key, &p).await {
            Some(pr) => {
                cur = pr
                    .extra
                    .get("parentId")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                out.push(pr);
            }
            None => break,
        }
    }
    out
}

async fn forks(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&d, &key, &id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let kid = key.id();
    let kids: Vec<Value> = d
        .store
        .all()
        .await
        .iter()
        .filter(|c| {
            c.key_id == kid && c.extra.get("parentId").and_then(Value::as_str) == Some(&r.id)
        })
        .map(|c| sandbox_dto(&d, c))
        .collect();
    ok_json(Value::Array(kids))
}

async fn parent(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&d, &key, &id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    match lineage(&d, &key, &r).await.first() {
        Some(p) => ok_json(sandbox_dto(&d, p)),
        None => error(StatusCode::NOT_FOUND, format!("Sandbox {id} has no parent")),
    }
}

async fn ancestors(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&d, &key, &id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    ok_json(Value::Array(
        lineage(&d, &key, &r)
            .await
            .iter()
            .map(|p| sandbox_dto(&d, p))
            .collect(),
    ))
}

/// `POST /sandbox/{id}/resize` `{cpu?, memory?, disk?}` (`ResizeSandbox`).
/// Hot resize accepts only increases of cpu and memory; disk grows only on
/// a stopped sandbox (the SDK's documented rules). The Space is restarted
/// with the new shape through the core (`Backend::resize`); a core without
/// that operation answers with the reason, and nothing changes.
async fn resize(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&d, &key, &id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let field = |k: &str| -> Result<Option<u32>, Response<Body>> {
        match b.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .filter(|n| *n >= 1)
                .and_then(|n| u32::try_from(n).ok())
                .map(Some)
                .ok_or_else(|| {
                    error(
                        StatusCode::BAD_REQUEST,
                        format!("{k} must be an integer of at least 1"),
                    )
                }),
        }
    };
    let (cpu, memory, disk) = match (field("cpu"), field("memory"), field("disk")) {
        (Ok(a), Ok(m), Ok(dk)) => (a, m, dk),
        (Err(e), ..) | (_, Err(e), _) | (.., Err(e)) => return e,
    };
    if cpu.is_none() && memory.is_none() && disk.is_none() {
        return error(
            StatusCode::BAD_REQUEST,
            "At least one of cpu, memory or disk must be set",
        );
    }
    let state = state_of(&r);
    if let Some(msg) = resize_refusal(
        &state,
        r.cpus,
        dt_memory_gib(&r),
        dt_disk_gib(&r),
        cpu,
        memory,
        disk,
    ) {
        return error(StatusCode::BAD_REQUEST, msg);
    }
    let shape = crate::backend::Shape {
        cpus: cpu,
        memory_mb: memory.map(|g| u64::from(g) * 1024),
        disk_gb: disk,
    };
    let old = state.clone();
    d.store
        .update(&r.id, |r| {
            r.extra.insert("dtState".into(), json!("resizing"));
        })
        .await;
    if let Some(cur) = d.store.get(&r.id).await {
        d.announce(&cur, &old).await;
    }
    // A restart takes longer than an HTTP call may: the work continues in the
    // background (the sandbox reports `resizing`; the SDK waits for it), and a
    // refusal, which comes at once, is still answered on the call.
    let work = {
        let (d, r) = (d.clone(), r.clone());
        tokio::spawn(async move {
            let mut res = d.backend.resize(&r.identity(), &r.cua_name, shape).await;
            // The core boots the resized VM again; a sandbox that was stopped
            // stays stopped (Daytona resizes a stopped sandbox without
            // starting it); a stopped sandbox here is a suspended Space.
            if res.is_ok() && r.paused {
                res = d.backend.suspend(&r.identity(), &r.cua_name).await;
            }
            d.forget(&r.id).await;
            let refused = matches!(res, Err(BackendError::Unsupported(_)));
            d.store
                .update(&r.id, |r| {
                    r.extra.remove("dtState");
                    match &res {
                        Ok(()) => {
                            if let Some(c) = cpu {
                                r.cpus = c;
                            }
                            if let Some(m) = shape.memory_mb {
                                r.memory_mb = m;
                            }
                            if let Some(g) = disk {
                                r.disk_mb = u64::from(g) * 1024;
                            }
                            r.extra.insert("updatedAt".into(), json!(now_ms()));
                        }
                        Err(_) if refused => {}
                        Err(e) => {
                            r.extra.insert("dtState".into(), json!("error"));
                            r.extra.insert("errorReason".into(), json!(e.to_string()));
                        }
                    }
                })
                .await;
            if let Some(cur) = d.store.get(&r.id).await {
                d.announce(&cur, "resizing").await;
            }
            res
        })
    };
    let mut work = work;
    match tokio::time::timeout(std::time::Duration::from_secs(5), &mut work).await {
        Ok(Ok(res)) => {
            let cur = d.store.get(&r.id).await.unwrap_or(r.clone());
            match res {
                Ok(()) => ok_json(sandbox_dto(&d, &cur)),
                Err(BackendError::Unsupported(m)) => error(
                    StatusCode::BAD_REQUEST,
                    format!("Resizing is not supported: {m}"),
                ),
                Err(e) => backend_error(e),
            }
        }
        Ok(Err(e)) => error(StatusCode::INTERNAL_SERVER_ERROR, format!("resize: {e}")),
        Err(_) => {
            let cur = d.store.get(&r.id).await.unwrap_or(r.clone());
            ok_json(sandbox_dto(&d, &cur))
        }
    }
}

/// Why a resize request is refused before it is tried (`None`: go ahead).
pub(crate) fn resize_refusal(
    state: &str,
    cur_cpu: u32,
    cur_mem_gib: u64,
    cur_disk_gib: u64,
    cpu: Option<u32>,
    memory: Option<u32>,
    disk: Option<u32>,
) -> Option<String> {
    if !matches!(state, "started" | "stopped") {
        return Some(format!(
            "Sandbox must be started or stopped to be resized (state {state})"
        ));
    }
    if disk.is_some_and(|d| u64::from(d) < cur_disk_gib) {
        return Some("Disk can only be increased".into());
    }
    if disk.is_some_and(|d| u64::from(d) != cur_disk_gib) && state != "stopped" {
        return Some("Disk resize requires a stopped sandbox".into());
    }
    if state == "started" {
        if cpu.is_some_and(|c| c < cur_cpu) {
            return Some("A running sandbox can only get more cpu; stop it to shrink".into());
        }
        if memory.is_some_and(|m| u64::from(m) < cur_mem_gib) {
            return Some("A running sandbox can only get more memory; stop it to shrink".into());
        }
    }
    None
}
/// `POST /sandbox/{id}/network-settings` `{networkBlockAll?, networkAllowList?,
/// domainAllowList?}` (`UpdateSandboxNetworkSettings`): block everything,
/// restore access (clearing the lists), or allow only some CIDRs and
/// domains. Enforced in the guest by an nftables policy
/// (`cua_spacesd_client::EgressPolicy`).
async fn network_settings(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&d, &key, &id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let block = b.get("networkBlockAll").and_then(Value::as_bool);
    let cidrs = b
        .get("networkAllowList")
        .and_then(Value::as_str)
        .map(str::to_string);
    let domains = b
        .get("domainAllowList")
        .and_then(Value::as_str)
        .map(str::to_string);
    if block.is_none() && cidrs.is_none() && domains.is_none() {
        return error(
            StatusCode::BAD_REQUEST,
            "At least one of networkBlockAll, networkAllowList or domainAllowList must be set",
        );
    }
    if let Some(l) = &cidrs
        && !l.is_empty()
        && !super::create::valid_cidr_list(l)
    {
        return error(
            StatusCode::BAD_REQUEST,
            format!("networkAllowList {l:?} must be comma-separated IPv4 CIDRs"),
        );
    }
    if let Some(l) = &domains
        && !l.is_empty()
        && !l.split(',').all(|h| super::secrets::valid_host(h.trim()))
    {
        return error(
            StatusCode::BAD_REQUEST,
            format!("domainAllowList {l:?} must be comma-separated domains"),
        );
    }
    if state_of(&r) != "started" {
        return error(
            StatusCode::BAD_REQUEST,
            format!(
                "Sandbox must be started to change its network settings (state {})",
                state_of(&r)
            ),
        );
    }
    // The new state: lists imply not blocking all; block=false clears them.
    let (nb, nc, nd) = match (
        block,
        cidrs.filter(|s| !s.is_empty()),
        domains.filter(|s| !s.is_empty()),
    ) {
        (Some(true), _, _) => (true, None, None),
        (Some(false), None, None) => (false, None, None),
        (_, c, dm) => (false, c, dm),
    };
    let policy = cua_spacesd_client::EgressPolicy {
        block_all: nb,
        allow_cidrs: nc
            .iter()
            .flat_map(|c| c.split(',').map(|x| x.trim().to_string()))
            .collect(),
        allow_domains: nd
            .iter()
            .flat_map(|c| c.split(',').map(|x| x.trim().to_string()))
            .collect(),
    };
    let c = match d.spacesd(&r).await {
        Ok(c) => c,
        Err(e) => return backend_error(e),
    };
    if let Err(e) = c.set_egress(&policy).await {
        return error(
            StatusCode::BAD_REQUEST,
            format!("Changing the network policy failed: {e}"),
        );
    }
    d.store
        .update(&r.id, |r| {
            r.internet = !nb;
            for (k, v) in [("networkAllowList", &nc), ("domainAllowList", &nd)] {
                match v {
                    Some(v) => {
                        r.extra.insert(k.into(), json!(v));
                    }
                    None => {
                        r.extra.remove(k);
                    }
                }
            }
            r.extra.insert("updatedAt".into(), json!(now_ms()));
        })
        .await;
    let r = d.store.get(&r.id).await.unwrap_or(r);
    ok_json(sandbox_dto(&d, &r))
}
/// `PUT /sandbox/{id}/secrets` `{secrets: [{ENV: secret name}]}`: replaces
/// the secrets mounted in the sandbox (see `secrets.rs` for how a mount
/// differs here: the value goes into the environment, with the label
/// `cua.secrets=plaintext`).
async fn update_secrets(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&d, &key, &id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let mut wanted: Vec<(String, String)> = Vec::new();
    for item in b
        .get("secrets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let Some(o) = item.as_object() else {
            return error(
                StatusCode::BAD_REQUEST,
                "each secret must be an {ENV: secret name} object",
            );
        };
        for (env, name) in o {
            let Some(name) = name.as_str() else {
                return error(StatusCode::BAD_REQUEST, "a secret name must be a string");
            };
            wanted.push((env.clone(), name.to_string()));
        }
    }
    let labels = string_map(r.extra.get("labels"));
    let values = match super::secrets::resolve(&d, &key, &wanted, &labels).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let old: Vec<String> = r
        .extra
        .get("secrets")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_object())
                .flat_map(|o| o.keys().cloned())
                .collect()
        })
        .unwrap_or_default();
    d.store
        .update(&r.id, |r| {
            for k in &old {
                r.env_vars.remove(k);
            }
            r.env_vars.extend(values.iter().cloned());
            if wanted.is_empty() {
                r.extra.remove("secrets");
            } else {
                r.extra.insert(
                    "secrets".into(),
                    Value::Array(wanted.iter().map(|(e, n)| json!({e: n})).collect()),
                );
            }
            r.extra.insert("updatedAt".into(), json!(now_ms()));
        })
        .await;
    let r = d.store.get(&r.id).await.unwrap_or(r);
    ok_json(sandbox_dto(&d, &r))
}

async fn no_secrets() -> Response<Body> {
    error(
        StatusCode::NOT_FOUND,
        "Resolving a sandbox's secrets is an internal call of the Daytona runner and is not served by this gateway",
    )
}
/// `GET .../signing-key`: the key `download_url`/`upload_url` sign with
/// (HMAC-SHA256 over `v1:files:<method>:<path>:<expires>`).
async fn signing_key(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    match owned(&d, &key, &id).await {
        Ok(r) => ok_json(json!(r.access_token)),
        Err(e) => e,
    }
}

/// `POST .../signing-key/rotate`: a new key; URLs signed with the old one
/// stop working.
async fn rotate_signing_key(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&d, &key, &id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let fresh = crate::auth::new_token();
    let t = fresh.clone();
    d.store.update(&r.id, |r| r.access_token = t).await;
    ok_json(json!(fresh))
}

/// `GET .../telemetry/metrics`: the guest's current reading, one point per
/// metric (Cua keeps no metric history).
async fn telemetry_metrics(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&d, &key, &id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    if r.paused {
        return ok_json(json!({"series": []}));
    }
    let c = match d.spacesd(&r).await {
        Ok(c) => c,
        Err(e) => return backend_error(e),
    };
    let m = match c.metrics().await {
        Ok(m) => m,
        Err(e) => return error(StatusCode::BAD_GATEWAY, e.to_string()),
    };
    let ts = rfc3339(now_ms());
    let point = |name: &str, v: f64| json!({"metricName": name, "dataPoints": [{"timestamp": ts, "value": v}]});
    ok_json(json!({"series": [
        point("daytona.sandbox.cpu.utilization", m.cpu_percent),
        point("daytona.sandbox.cpu.limit", f64::from(m.cpu_count)),
        point("daytona.sandbox.memory.usage", m.memory_used_bytes as f64),
        point("daytona.sandbox.memory.limit", m.memory_total_bytes as f64),
        point("daytona.sandbox.memory.cache", 0.0),
        point("daytona.sandbox.filesystem.usage", m.disk_used_bytes as f64),
        point("daytona.sandbox.filesystem.total", m.disk_total_bytes as f64),
    ]}))
}
async fn no_telemetry() -> Response<Body> {
    error(
        StatusCode::NOT_FOUND,
        "Sandbox telemetry is not available from the Cua gateway",
    )
}
/// `GET .../build-logs-url`: `{url}` of the log stream.
pub(super) async fn logs_url(d: &Daytona, path: String) -> Response<Body> {
    ok_json(json!({"url": format!("{}/api{path}", d.cfg.api_base())}))
}

async fn sandbox_build_logs_url(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    match owned(&d, &key, &id).await {
        Ok(r) => logs_url(&d, format!("/sandbox/{}/build-logs", r.id)).await,
        Err(e) => e,
    }
}

#[derive(serde::Deserialize, Default)]
pub(super) struct FollowQuery {
    pub follow: Option<bool>,
}

/// Streams a build log: what exists, then (with `follow=true`) new lines
/// until the build ends.
pub(super) async fn stream_log(log: Arc<super::build::LogBuf>, follow: bool) -> Response<Body> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(16);
    tokio::spawn(async move {
        let mut sent = 0usize;
        let mut done = log.watch();
        loop {
            let (text, finished) = log.snapshot().await;
            if text.len() > sent {
                if tx
                    .send(Ok(bytes::Bytes::from(text[sent..].to_string())))
                    .await
                    .is_err()
                {
                    return;
                }
                sent = text.len();
            }
            if finished || !follow {
                return;
            }
            tokio::select! {
                _ = done.changed() => {}
                _ = tokio::time::sleep(std::time::Duration::from_millis(300)) => {}
            }
        }
    });
    Response::builder()
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from_stream(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        ))
        .expect("response")
}

async fn sandbox_build_logs(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<FollowQuery>,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    match owned(&d, &key, &id).await {
        Ok(r) => {
            stream_log(
                super::build::log_for(&d, &r.id).await,
                q.follow.unwrap_or(false),
            )
            .await
        }
        Err(e) => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const STATES: [&str; 22] = [
        "creating",
        "restoring",
        "destroyed",
        "destroying",
        "started",
        "stopped",
        "starting",
        "stopping",
        "error",
        "build_failed",
        "pending_build",
        "building_snapshot",
        "unknown",
        "pulling_snapshot",
        "archived",
        "archiving",
        "resizing",
        "snapshotting",
        "forking",
        "pausing",
        "paused",
        "resuming",
    ];

    #[test]
    fn lifecycle_rules_follow_the_sdk_docs() {
        // sandbox.py: archive "must be stopped before archiving"; pause needs
        // a started sandbox; start of a started sandbox has nothing to do.
        assert_eq!(plan_op(Op::Start, "started", false), Ok(None));
        assert_eq!(plan_op(Op::Start, "stopped", false), Ok(Some("started")));
        assert_eq!(plan_op(Op::Start, "archived", false), Ok(Some("started")));
        assert_eq!(plan_op(Op::Stop, "started", false), Ok(Some("stopped")));
        assert_eq!(plan_op(Op::Stop, "stopped", false), Ok(None));
        assert_eq!(plan_op(Op::Archive, "stopped", false), Ok(Some("archived")));
        assert!(plan_op(Op::Archive, "started", false).is_err());
        assert_eq!(plan_op(Op::Pause, "started", false), Ok(Some("paused")));
        assert!(plan_op(Op::Pause, "stopped", false).is_err());
        assert_eq!(plan_op(Op::Recover, "error", true), Ok(Some("started")));
        assert!(plan_op(Op::Recover, "error", false).is_err());
        assert!(plan_op(Op::Recover, "started", true).is_err());
        assert!(plan_op(Op::Start, "pending_build", false).is_err());
        assert!(plan_op(Op::Stop, "destroyed", false).is_err());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn a_planned_state_is_never_busy_or_destroyed(op in 0usize..5, st in 0usize..22, rec in proptest::prelude::any::<bool>()) {
            let op = [Op::Start, Op::Stop, Op::Pause, Op::Archive, Op::Recover][op];
            if let Ok(Some(to)) = plan_op(op, STATES[st], rec) {
                prop_assert!(!busy(to) && to != "destroyed" && to != "error");
            }
            // Busy states refuse every operation.
            if busy(STATES[st]) {
                prop_assert!(plan_op(op, STATES[st], rec).is_err());
            }
        }

        #[test]
        fn list_queries_never_panic(raw in "\\PC{0,200}") {
            let _ = ListQuery::parse(&raw);
        }

        #[test]
        fn cursors_roundtrip(n in 0usize..1_000_000) {
            prop_assert_eq!(decode_cursor(&encode_cursor(n)), Some(n));
        }
    }

    #[test]
    fn resize_rules_follow_the_sdk_docs() {
        // sandbox.py resize: hot resize only increases cpu/memory; disk grows only when stopped.
        assert!(resize_refusal("started", 2, 4, 10, Some(4), Some(8), None).is_none());
        assert!(resize_refusal("started", 4, 4, 10, Some(2), None, None).is_some());
        assert!(resize_refusal("started", 2, 8, 10, None, Some(4), None).is_some());
        assert!(resize_refusal("started", 2, 4, 10, None, None, Some(20)).is_some());
        assert!(resize_refusal("stopped", 2, 4, 10, Some(1), Some(2), Some(20)).is_none());
        assert!(resize_refusal("stopped", 2, 4, 10, None, None, Some(5)).is_some());
        assert!(resize_refusal("archived", 2, 4, 10, Some(4), None, None).is_some());
    }

    #[test]
    fn list_query_filters_and_rejects() {
        let q = ListQuery::parse(
            "labels=%7B%22a%22%3A%22b%22%7D&states=started&states=stopped,paused&minCpu=2&isPublic=true&limit=5&sort=cpu&order=desc&createdAtAfter=2026-01-01T00%3A00%3A00Z",
        )
        .unwrap();
        assert_eq!(q.states, vec!["started", "stopped", "paused"]);
        assert_eq!(q.labels["a"], "b");
        assert_eq!(q.limit, Some(5));
        assert_eq!(q.min_cpu, Some(2.0));
        assert!(q.created_after.is_some());
        let f = |cpu, public: bool, state: &str, created| Facts {
            id: "id1".into(),
            name: "n".into(),
            labels: [("a".to_string(), "b".to_string())].into(),
            state: state.into(),
            snapshot: "s".into(),
            target: "t".into(),
            cpu,
            mem_gib: 1,
            disk_gib: 1,
            gpu: 0,
            public,
            recoverable: false,
            created_ms: created,
            activity_ms: 0,
            destroy_ms: None,
        };
        let after = q.created_after.unwrap();
        assert!(q.matches(&f(2, true, "started", after + 1)));
        assert!(!q.matches(&f(1, true, "started", after + 1)));
        assert!(!q.matches(&f(2, false, "started", after + 1)));
        assert!(!q.matches(&f(2, true, "archived", after + 1)));
        assert!(!q.matches(&f(2, true, "started", after - 1)));
        for bad in [
            "limit=0",
            "limit=201",
            "states=bogus",
            "sort=weird",
            "order=up",
            "minCpu=x",
            "isPublic=maybe",
            "labels=%5B%5D",
            "createdAtAfter=yesterday",
        ] {
            assert!(ListQuery::parse(bad).is_err(), "{bad}");
        }
    }
}
