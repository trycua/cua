//! E2B's REST control plane (`X-API-KEY`), over the backend: create
//! (`/sandboxes`, `/v2/sandboxes`), get, list, kill, timeout, connect,
//! pause/resume, snapshots, fork and metrics. Platform features with no
//! Cua equivalent (template builds, volumes, secrets, logs, egress rules,
//! IAM, E2B's MCP gateway) answer with E2B's error shape and the reason.

use super::{DEFAULT_TIMEOUT_S, E2b, ENVD_VERSION, TemplateTarget};
use crate::auth::KeyEntry;
use crate::backend::{BackendError, CreateSpec, Source};
use crate::state::{OnTimeout, Record, now_ms};
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, RawQuery, State};
use axum::routing::{any, get, post, put};
use http::{HeaderMap, Response, StatusCode, header};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

pub fn routes(e2b: Arc<E2b>) -> Router {
    Router::new()
        .route("/sandboxes", post(create).get(list_v1))
        .route("/v2/sandboxes", post(create).get(list_v2))
        .route("/sandboxes/{id}", get(info).delete(kill))
        .route("/sandboxes/{id}/timeout", post(set_timeout))
        .route("/sandboxes/{id}/refreshes", post(refresh))
        .route("/sandboxes/{id}/connect", post(connect))
        .route("/v2/sandboxes/{id}/connect", post(connect))
        .route("/sandboxes/{id}/pause", post(pause))
        .route("/sandboxes/{id}/resume", post(resume))
        .route("/sandboxes/{id}/snapshots", post(snapshot))
        .route("/sandboxes/{id}/fork", post(fork))
        .route("/sandboxes/{id}/metrics", get(metrics))
        .route("/snapshots", get(list_snapshots))
        .route(
            "/templates/{id}",
            get(super::templates::get)
                .delete(delete_template)
                .patch(super::templates::update)
                .post(no_build),
        )
        .route("/v3/templates", post(super::templates::request_build))
        .route("/v2/templates", post(super::templates::request_build))
        .route("/templates", get(super::templates::list).post(no_build))
        .route(
            "/v2/templates/{id}/builds/{build}",
            post(super::templates::start_build),
        )
        .route(
            "/templates/{id}/builds/{build}/status",
            get(super::templates::build_status),
        )
        .route(
            "/templates/{id}/files/{hash}",
            get(super::templates::file_upload_link),
        )
        .route(
            "/templates/{id}/upload/{hash}",
            put(super::templates::upload_context),
        )
        .route("/templates/{id}/tags", get(super::templates::tags))
        .route(
            "/templates/tags",
            post(super::templates::assign_tags).delete(super::templates::remove_tags),
        )
        .route("/templates/aliases/{alias}", get(super::templates::alias))
        .route("/sandboxes/metrics", get(batch_metrics))
        .route("/sandboxes/{id}/network", put(update_network))
        .route("/sandboxes/{id}/logs", get(no_logs))
        .route("/v2/sandboxes/{id}/logs", get(no_logs))
        .route("/volumes", any(no_volumes))
        .route("/volumes/{*rest}", any(no_volumes))
        .route(
            "/secrets",
            post(super::secrets::create).get(super::secrets::list),
        )
        .route(
            "/secrets/{id}",
            get(super::secrets::get)
                .post(super::secrets::update)
                .delete(super::secrets::destroy),
        )
        .fallback(not_found_route)
        .with_state(e2b)
}

/// Any route the gateway does not serve: E2B's JSON 404 (the SDKs read the
/// body, so an HTML or empty answer would surface as a decoding error).
async fn not_found_route(uri: http::Uri) -> Response<Body> {
    api_error(
        StatusCode::NOT_FOUND,
        format!("route {} not found", uri.path()),
    )
}

/// E2B's API error body.
pub fn api_error(status: StatusCode, message: impl Into<String>) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"code": status.as_u16(), "message": message.into()}).to_string(),
        ))
        .expect("response")
}

fn json_response(status: StatusCode, v: Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(v.to_string()))
        .expect("response")
}

fn no_content() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .expect("response")
}

fn backend_error(e: BackendError) -> Response<Body> {
    let status = match e {
        BackendError::NotFound(_) => StatusCode::NOT_FOUND,
        BackendError::Invalid(_) | BackendError::Unsupported(_) => StatusCode::BAD_REQUEST,
        BackendError::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
        BackendError::RateLimited(_) => StatusCode::TOO_MANY_REQUESTS,
        BackendError::Other(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    api_error(status, e.to_string())
}

pub(super) async fn authed(e: &E2b, headers: &HeaderMap) -> Result<KeyEntry, Response<Body>> {
    if let Some(k) = e.caller(headers).await {
        return Ok(k);
    }
    // E2B's own wording (the SDKs print it after "401: Unauthorized, please
    // check your credentials. - ").
    let presented = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    Err(api_error(
        StatusCode::UNAUTHORIZED,
        if presented.starts_with("e2b_") {
            "Invalid API key, please visit https://docs.e2b.dev/api-key for more information.\nCannot get the team for the given API key"
        } else {
            "API key is malformed: expected the \"e2b_\" prefix, visit https://docs.e2b.dev/api-key for more information"
        },
    ))
}

/// Sandbox ids are alphanumerics (E2B's are, and so are this gateway's); a
/// path-like id is a 400, not a lookup.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The caller's record, or 404.
async fn owned(e: &E2b, key: &KeyEntry, id: &str) -> Result<Record, Response<Body>> {
    if !valid_id(id) {
        return Err(api_error(StatusCode::BAD_REQUEST, "Invalid sandbox ID"));
    }
    match e.store.get(id).await {
        Some(r) if r.key_id == key.id() => Ok(r),
        _ => Err(api_error(
            StatusCode::NOT_FOUND,
            format!("sandbox \"{id}\" doesn't exist or you don't have access to it"),
        )),
    }
}

pub(super) async fn body_json(body: Body) -> Result<Value, Response<Body>> {
    let bytes = http_body_util::BodyExt::collect(http_body_util::Limited::new(body, 1 << 20))
        .await
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, format!("reading body: {e}")))?
        .to_bytes();
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}")))
}

pub(super) fn rfc3339(ms: u64) -> String {
    let t = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// A new E2B-style sandbox id: 20 lowercase alphanumerics.
pub(super) fn new_id() -> String {
    use rand::Rng;
    const A: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    (0..20)
        .map(|_| A[rng.random_range(0..A.len())] as char)
        .collect()
}

/// The `Sandbox` body of create / connect / fork.
fn sandbox_json(e: &E2b, r: &Record) -> Value {
    let mut v = json!({
        "templateID": r.template_id,
        "sandboxID": r.id,
        "clientID": "cua",
        "envdVersion": ENVD_VERSION,
        "envdAccessToken": r.access_token,
        "domain": e.cfg.sandbox_domain,
    });
    if let Some(a) = &r.alias {
        v["alias"] = json!(a);
    }
    if let Some(t) = &r.traffic_token {
        v["trafficAccessToken"] = json!(t);
    }
    v
}

fn state_name(r: &Record) -> &'static str {
    if r.paused { "paused" } else { "running" }
}

fn end_at(r: &Record) -> u64 {
    r.end_at_ms.unwrap_or_else(now_ms)
}

/// `SandboxDetail` (`GET /sandboxes/{id}`).
fn detail_json(e: &E2b, r: &Record) -> Value {
    let mut v = json!({
        "templateID": r.template_id,
        "sandboxID": r.id,
        "clientID": "cua",
        "startedAt": rfc3339(r.started_at_ms),
        "endAt": rfc3339(end_at(r)),
        "envdVersion": ENVD_VERSION,
        "envdAccessToken": r.access_token,
        "domain": e.cfg.sandbox_domain,
        "cpuCount": r.cpus,
        "memoryMB": r.memory_mb,
        "diskSizeMB": r.disk_mb,
        "metadata": r.metadata,
        "state": state_name(r),
        "allowInternetAccess": if r.extra.get("internet_set").is_some() { json!(r.internet) } else { Value::Null },
        "lifecycle": {
            "autoResume": r.auto_resume,
            "onTimeout": match r.on_timeout { OnTimeout::Kill => "kill", OnTimeout::Pause => "pause" },
        },
    });
    if let Some(a) = &r.alias {
        v["alias"] = json!(a);
    }
    let mut net = r
        .extra
        .get("network")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if r.traffic_token.is_some() {
        net.insert("allowPublicTraffic".into(), json!(false));
    }
    // E2B shows a `network` object once the sandbox was given one, even an
    // empty one (`update_network` with nothing to filter).
    if !net.is_empty() || r.extra.contains_key("network") {
        v["network"] = Value::Object(net);
    }
    v
}

/// `ListedSandbox`.
fn listed_json(r: &Record) -> Value {
    let mut v = json!({
        "templateID": r.template_id,
        "sandboxID": r.id,
        "clientID": "cua",
        "startedAt": rfc3339(r.started_at_ms),
        "endAt": rfc3339(end_at(r)),
        "cpuCount": r.cpus,
        "memoryMB": r.memory_mb,
        "diskSizeMB": r.disk_mb,
        "metadata": r.metadata,
        "state": state_name(r),
        "envdVersion": ENVD_VERSION,
    });
    if let Some(a) = &r.alias {
        v["alias"] = json!(a);
    }
    v
}

fn string_map(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
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

fn timeout_ms(v: &Value, key: &str) -> u64 {
    v.get(key)
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT_S)
        .saturating_mul(1000)
}

/// E2B's request validation (its OpenAPI filter's wording), for the checks
/// the SDKs can trip over: a timeout of at least 1, string metadata and
/// environment values.
fn validate_create(b: &Value, schema: &str) -> Option<String> {
    let fail = |at: &str, what: &str| {
        Some(format!(
            "validation error: error in openapi3filter.RequestError: request body has an error: doesn't match schema #/components/schemas/{schema}: Error at \"{at}\": {what}"
        ))
    };
    if let Some(t) = b.get("timeout") {
        match t.as_f64() {
            // NewSandbox (v1) allows 0, NewSandboxV2 does not (recorded).
            Some(n) if n < 1.0 && (schema == "NewSandboxV2" || n < 0.0) => {
                return fail(
                    "/timeout",
                    if schema == "NewSandboxV2" {
                        "number must be at least 1"
                    } else {
                        "number must be at least 0"
                    },
                );
            }
            None if !t.is_null() => return fail("/timeout", "value must be an integer"),
            _ => {}
        }
    }
    for (field, path) in [("metadata", "/metadata"), ("envVars", "/envVars")] {
        if let Some(m) = b.get(field).and_then(Value::as_object) {
            for (k, v) in m {
                if !v.is_string() {
                    return fail(&format!("{path}/{k}"), "value must be a string");
                }
            }
        }
    }
    None
}

/// The outbound policy of a `network` object (`allowOut`, `denyOut`), and
/// the `network` JSON `get_info` shows for it.
fn network_policy(n: &Value, no_internet: bool) -> Result<(super::egress::Policy, Value), String> {
    let list = |k: &str| -> Vec<String> {
        n.get(k)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let (allow, deny) = (list("allowOut"), list("denyOut"));
    let policy = super::egress::Policy::parse(&allow, &deny, no_internet)?;
    let mut shown = serde_json::Map::new();
    if !policy.allow.is_empty() {
        shown.insert("allowOut".into(), json!(policy.allow));
    }
    if !policy.deny.is_empty() {
        shown.insert("denyOut".into(), json!(policy.deny));
    }
    Ok((policy, Value::Object(shown)))
}

/// Fields of a create request this gateway cannot honour, as the reason.
fn unsupported_create_fields(b: &Value) -> Option<String> {
    let set = |k: &str| match b.get(k) {
        None | Some(Value::Null) => false,
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(_) => true,
    };
    if set("mcp") {
        return Some("mcp: E2B's MCP gateway has no Cua equivalent; run MCP servers in the sandbox and reach them with get_host".into());
    }
    if set("iam") {
        return Some("iam: workload identity is not supported by this gateway".into());
    }
    if set("volumeMounts") {
        return Some(
            "volumeMounts: E2B volumes are not supported; use Cua Volume (mounted in every Space)"
                .into(),
        );
    }
    if let Some(n) = b.get("network").and_then(Value::as_object) {
        for k in ["rules", "egressProxy", "maskRequestHost", "httpsPorts"] {
            let v = n.get(k);
            let used = match v {
                None | Some(Value::Null) => false,
                Some(Value::Array(a)) => !a.is_empty(),
                Some(Value::Object(o)) => !o.is_empty(),
                Some(Value::String(s)) => !s.is_empty(),
                Some(_) => true,
            };
            if used {
                return Some(format!(
                    "network.{k}: rewriting outbound requests (headers, proxies, host masking, HTTPS interception) needs an inspecting egress proxy, which Cua Spaces do not have; allow_out and deny_out take addresses and CIDR blocks"
                ));
            }
        }
    }
    None
}

/// Resolves `templateID` to a backend source.
async fn resolve_template(
    e: &E2b,
    key: &KeyEntry,
    template: &str,
) -> Result<Source, Response<Body>> {
    if let Some(t) = e.cfg.templates.get(template) {
        return Ok(match t {
            TemplateTarget::Image(i) => Source::Image(Some(i.clone())),
            TemplateTarget::Checkpoint(c) => Source::Checkpoint(c.clone()),
        });
    }
    // `code-interpreter-v1` is the base image plus the code-interpreter
    // server, which `prepare_default_user` installs.
    if template.is_empty() || template == "base" || super::code_interpreter::wants(template) {
        return Ok(Source::Image(None));
    }
    // A template this gateway built (by name[:tag] or id).
    if let Some((_, b)) = e.templates.resolve(&key.id(), template).await {
        return Ok(Source::Checkpoint(b.checkpoint.unwrap_or_default()));
    }
    let who = crate::backend::Identity {
        daemon: key.daemon.clone(),
        token: key.token.clone(),
    };
    // `<namespace>/<name>` names a snapshot as `list_snapshots` shows it.
    let template = template
        .strip_prefix(&format!("{}/", namespace(key)))
        .unwrap_or(template);
    match e.backend.checkpoints(&who).await {
        Ok(cps) if cps.iter().any(|c| c.name == template) => {
            return Ok(Source::Checkpoint(template.to_string()));
        }
        Ok(_) => {}
        Err(err) => return Err(backend_error(err)),
    }
    if template.contains('/') {
        return Ok(Source::Image(Some(template.to_string())));
    }
    Err(api_error(
        StatusCode::NOT_FOUND,
        format!(
            "template '{}' not found",
            crate::e2b::templates::split_name(template).0
        ),
    ))
}

/// Makes E2B's default user (with passwordless sudo, as E2B's base
/// template has) when the image lacks it.
async fn prepare_default_user(e: &E2b, r: &Record) {
    let Ok(c) = e.spacesd(r).await else { return };
    let driver = e.drivers.of(&r.id, &c).await;
    ensure_default_user(e, &c, &driver, &e.cfg.default_user.clone()).await;
    if super::code_interpreter::wants(&r.template_id) {
        super::code_interpreter::provision(e, &c, &driver).await;
    }
}

/// [`prepare_default_user`] for a sandbox that has no record (template
/// builds).
pub(super) async fn ensure_default_user(
    e: &E2b,
    c: &cua_spacesd_client::SpacesdClient,
    driver: &str,
    user: &str,
) {
    // Login shells (every command runs in one) get umask 022, as E2B's
    // images do, instead of the guest agent's group-writable default.
    // E2B's base image has `python` (the Cua Linux image has python3 only):
    // `python -m http.server` and friends must work.
    let _ = crate::guest::run_as_idempotent(
        c,
        driver,
        "root",
        "printf 'umask 022\\n' > /etc/profile.d/10-cua-compat-umask.sh; \
         command -v python >/dev/null 2>&1 || { p=$(command -v python3) && ln -s \"$p\" /usr/local/bin/python; }; true",
        &[],
    )
    .await;
    if !e.cfg.create_default_user || user == "root" {
        return;
    }
    let script = format!(
        "id -u {user} >/dev/null 2>&1 && exit 0; \
         (useradd -m -s /bin/bash {user} || adduser -D -s /bin/sh {user}) >/dev/null 2>&1 || exit 1; \
         if [ -d /etc/sudoers.d ]; then echo '{user} ALL=(ALL:ALL) NOPASSWD: ALL' > /etc/sudoers.d/90-{user} && chmod 440 /etc/sudoers.d/90-{user}; fi"
    );
    match crate::guest::run_as_idempotent(c, driver, "root", &script, &[]).await {
        Ok(out) if out.success() => {}
        Ok(out) => tracing::warn!(
            "compat: creating user {user}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(err) => tracing::warn!("compat: creating user {user}: {err}"),
    }
}

struct NewSandbox {
    template_id: String,
    /// The built template's default working directory for commands.
    workdir: Option<String>,
    /// The built template's `USER` (the default user of its sandboxes).
    user: Option<String>,
    /// `false`: an automatic pause keeps the disk only.
    auto_pause_memory: Option<bool>,
    /// Outbound rules and the `network` object `get_info` shows.
    egress: Option<(super::egress::Policy, Value)>,
    alias: Option<String>,
    /// `allow_internet_access` was given (E2B reports null otherwise).
    internet_set: bool,
    source: Source,
    timeout_ms: u64,
    metadata: BTreeMap<String, String>,
    env_vars: BTreeMap<String, String>,
    internet: bool,
    on_timeout: OnTimeout,
    auto_resume: bool,
    public_traffic: bool,
    held_checkpoint: Option<String>,
    place: crate::backend::Placement,
}

/// Cua placement from E2B metadata (`cua.on`, `cua.kind`, `cua.cpus`,
/// `cua.memory_mb`, `cua.desktop`): a one-line E2B program can still choose any machine,
/// pool, BYOC cloud or kind Cua offers.
pub fn placement_from_metadata(m: &BTreeMap<String, String>) -> crate::backend::Placement {
    crate::backend::Placement {
        // `host:local` and this machine's own name are this machine.
        on: m.get("cua.on").map(|o| match o.as_str() {
            "host:local" => "local".to_string(),
            other => other.to_string(),
        }),
        kind: m.get("cua.kind").cloned(),
        cpus: m.get("cua.cpus").and_then(|v| v.parse().ok()),
        memory_mb: m.get("cua.memory_mb").and_then(|v| v.parse().ok()),
        desktop: m.get("cua.desktop").is_some_and(|v| v == "true"),
    }
}

/// [`start_sandbox`] run to its end even when the client's request is dropped
/// (its request timeout, a cancelled task): a create nobody waits for any
/// more is killed when it finishes, so an abandoned request cannot leave a
/// sandbox that lives for its whole E2B `timeout`. (A real E2B create takes
/// about a second; a create queued behind a burst can outlast a client's
/// timeout, and the SDK then retries.)
async fn start_detached(
    e: &Arc<E2b>,
    key: &KeyEntry,
    n: NewSandbox,
) -> Result<Record, BackendError> {
    let (mut tx, rx) = tokio::sync::oneshot::channel();
    let (e2, key2) = (e.clone(), key.clone());
    let id = new_id();
    tokio::spawn(async move {
        // The client leaving cancels the create in core (no host is bought
        // or kept booting for nobody); a create that still finishes is
        // killed below.
        let who = crate::backend::Identity {
            daemon: key2.daemon.clone(),
            token: key2.token.clone(),
        };
        let name = format!("e2b-{id}");
        let work = start_sandbox_as(&e2, &key2, n, id);
        tokio::pin!(work);
        let res = tokio::select! {
            r = &mut work => r,
            _ = tx.closed() => {
                tracing::info!("compat: the client of create {name} left: cancelling it");
                let _ = e2.backend.cancel_create(&who, &name).await;
                work.await
            }
        };
        if let Err(Ok(r)) = tx.send(res) {
            tracing::info!(
                "compat: the create of {} finished after its client gave up; killing it",
                r.id
            );
            crate::state::kill(&e2.store, e2.backend.as_ref(), &r.id).await;
        }
    });
    rx.await
        .unwrap_or_else(|_| Err(BackendError::Other("the create task stopped".into())))
}

async fn start_sandbox(e: &E2b, key: &KeyEntry, n: NewSandbox) -> Result<Record, BackendError> {
    start_sandbox_as(e, key, n, new_id()).await
}

async fn start_sandbox_as(
    e: &E2b,
    key: &KeyEntry,
    n: NewSandbox,
    id: String,
) -> Result<Record, BackendError> {
    let who = crate::backend::Identity {
        daemon: key.daemon.clone(),
        token: key.token.clone(),
    };
    let n_egress = n.egress.clone();
    let mut source = n.source.clone();
    // The default image (or the code-interpreter template, which is it
    // plus a server) forks from a booted, prepared base when this shape
    // has one: a second whatever the concurrency, instead of a boot each.
    let mut golden: Option<String> = None;
    if matches!(source, Source::Image(None)) && n.internet && !n.place.desktop {
        let variant = if super::code_interpreter::wants(&n.template_id) {
            super::golden::Variant::CodeInterpreter
        } else {
            super::golden::Variant::Base
        };
        if let Some(cp) = e.golden.checkpoint_for(e, &who, variant, &n.place).await {
            source = Source::Checkpoint(cp.clone());
            golden = Some(cp);
        }
    }
    let spec = |source: Source| CreateSpec {
        name: format!("e2b-{id}"),
        source,
        env: n.env_vars.clone().into_iter().collect(),
        internet: n.internet,
        place: n.place.clone(),
    };
    let created = match e.backend.create(&who, spec(source.clone())).await {
        Err(err) if golden.is_some() => {
            // The base could not be forked (gone, stale): boot cold, and
            // build a new base next time.
            tracing::warn!("compat: forking the base sandbox failed, creating cold: {err}");
            if let Some(cp) = golden.take() {
                e.golden.forget(e, &who, &cp).await;
            }
            source = n.source.clone();
            e.backend.create(&who, spec(source.clone())).await?
        }
        other => other?,
    };
    let from_image = golden.is_none() && matches!(source, Source::Image(_));
    let from_checkpoint = match &source {
        Source::Checkpoint(c) if golden.is_none() => Some(c.clone()),
        _ => None,
    };
    let now = now_ms();
    let r = Record {
        id: id.clone(),
        cua_name: created.name,
        key_id: key.id(),
        identity_daemon: key.daemon.clone(),
        identity_token: key.token.clone(),
        template_id: n.template_id,
        alias: n.alias,
        metadata: n.metadata,
        env_vars: n.env_vars,
        access_token: crate::auth::new_token(),
        traffic_token: (!n.public_traffic).then(crate::auth::new_token),
        started_at_ms: now,
        end_at_ms: Some(now + n.timeout_ms),
        on_timeout: n.on_timeout,
        delete_after_pause_ms: None,
        hard_end_at_ms: None,
        auto_resume: n.auto_resume,
        paused: false,
        internet: n.internet,
        cpus: created.cpus,
        memory_mb: created.memory_mb,
        disk_mb: created.disk_mb,
        held_checkpoint: n.held_checkpoint,
        extra: {
            let mut x = Map::new();
            if n.internet_set {
                x.insert("internet_set".into(), Value::Bool(true));
            }
            if let Some(w) = &n.workdir {
                x.insert("workdir".into(), Value::String(w.clone()));
            }
            if let Some(u) = &n.user {
                x.insert("user".into(), Value::String(u.clone()));
            }
            if n.auto_pause_memory == Some(false) {
                x.insert("auto_pause_memory".into(), Value::Bool(false));
            }
            if let Some((_, shown)) = &n.egress {
                x.insert("network".into(), shown.clone());
            }
            x
        },
    };
    e.store.insert(r.clone()).await;
    if from_image {
        prepare_default_user(e, &r).await;
    }
    if !n.internet {
        // Never hand out an open sandbox for `allow_internet_access=False`:
        // a host or daemon that ignored the restriction is refused as an
        // error, not discovered by the program.
        match e.backend.network_blocked(&r.identity(), &r.cua_name).await {
            Ok(true) => {}
            Ok(false) => {
                crate::state::kill(&e.store, e.backend.as_ref(), &r.id).await;
                return Err(BackendError::Unsupported(
                    "allow_internet_access=False: this host did not apply the network restriction (it needs a current cua daemon), so no sandbox was created".into(),
                ));
            }
            Err(err) => {
                crate::state::kill(&e.store, e.backend.as_ref(), &r.id).await;
                return Err(err);
            }
        }
    }
    if let Some(cp) = &from_checkpoint
        && e.store.is_fs_only(cp).await
    {
        // A snapshot taken without memory: start from its disk state.
        if let Err(err) = e.reboot_guest(&r).await {
            crate::state::kill(&e.store, e.backend.as_ref(), &r.id).await;
            return Err(err);
        }
    }
    if let Some((policy, _)) = &n_egress
        && !policy.is_empty()
        && let Err(err) = super::egress::apply(e, &r, policy).await
    {
        crate::state::kill(&e.store, e.backend.as_ref(), &r.id).await;
        return Err(err);
    }
    Ok(r)
}

/// `POST /sandboxes`, `POST /v2/sandboxes`.
async fn create(
    State(e): State<Arc<E2b>>,
    uri: http::Uri,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let schema = if uri.path().starts_with("/v2") {
        "NewSandboxV2"
    } else {
        "NewSandbox"
    };
    if let Some(reason) = validate_create(&b, schema) {
        return api_error(StatusCode::BAD_REQUEST, reason);
    }
    if let Some(reason) = unsupported_create_fields(&b) {
        return api_error(StatusCode::BAD_REQUEST, reason);
    }
    let template_id = b
        .get("templateID")
        .and_then(Value::as_str)
        .unwrap_or("base")
        .to_string();
    let source = match resolve_template(&e, &key, &template_id).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let auto_resume = match b.get("autoResume") {
        Some(Value::Bool(v)) => *v,
        Some(Value::Object(o)) => o.get("enabled").and_then(Value::as_bool).unwrap_or(false),
        _ => false,
    };
    let metadata = string_map(b.get("metadata").unwrap_or(&Value::Null));
    let source = match (source, metadata.get("cua.image")) {
        (Source::Image(_), Some(img)) => Source::Image(Some(img.clone())),
        (s, _) => s,
    };
    let mut template_id = template_id;
    let built = e.templates.resolve(&key.id(), &template_id).await;
    let alias = if let Some((t, _)) = &built {
        // A template this gateway built: the record carries its id and name.
        let name = t.names.first().cloned();
        template_id = t.id.clone();
        e.templates.bump_spawn(&t.id).await;
        name
    } else if template_id.is_empty() || template_id == "base" {
        Some("base".to_string())
    } else if e.cfg.templates.contains_key(&template_id)
        || super::code_interpreter::wants(&template_id)
    {
        Some(template_id.clone())
    } else {
        None
    };
    let egress = match b.get("network").filter(|n| n.is_object()) {
        Some(n) => match network_policy(n, false) {
            Ok((p, mut shown)) => {
                // The network object lists what was asked for.
                if let Some(o) = shown.as_object_mut() {
                    let public = n
                        .get("allowPublicTraffic")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    o.insert("allowPublicTraffic".into(), json!(public));
                }
                (shown.as_object().is_some_and(|o| !o.is_empty())).then_some((p, shown))
            }
            Err(msg) => {
                return api_error(StatusCode::BAD_REQUEST, format!("network: {msg}"));
            }
        },
        None => None,
    };
    let workdir = built.as_ref().and_then(|(_, b)| b.workdir.clone());
    let user = built
        .as_ref()
        .and_then(|(_, b)| b.user.clone())
        .filter(|u| *u != e.cfg.default_user);
    let n = NewSandbox {
        place: placement_from_metadata(&metadata),
        workdir,
        user,
        auto_pause_memory: b.get("autoPauseMemory").and_then(Value::as_bool),
        egress,
        alias,
        internet_set: b.get("allow_internet_access").is_some_and(|v| !v.is_null()),
        template_id,
        source,
        timeout_ms: timeout_ms(&b, "timeout"),
        metadata,
        env_vars: string_map(b.get("envVars").unwrap_or(&Value::Null)),
        internet: b
            .get("allow_internet_access")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        on_timeout: if b.get("autoPause").and_then(Value::as_bool).unwrap_or(false) {
            OnTimeout::Pause
        } else {
            OnTimeout::Kill
        },
        auto_resume,
        public_traffic: b
            .pointer("/network/allowPublicTraffic")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        held_checkpoint: None,
    };
    match start_detached(&e, &key, n).await {
        Ok(r) => json_response(StatusCode::CREATED, sandbox_json(&e, &r)),
        Err(err) => backend_error(err),
    }
}

/// `GET /sandboxes/{id}`.
async fn info(
    State(e): State<Arc<E2b>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    match owned(&e, &key, &id).await {
        Ok(r) => {
            if e.check_gone(&r).await {
                return api_error(StatusCode::NOT_FOUND, format!("sandbox \"{id}\" not found"));
            }
            json_response(StatusCode::OK, detail_json(&e, &r))
        }
        Err(resp) => resp,
    }
}

/// `DELETE /sandboxes/{id}`.
async fn kill(
    State(e): State<Arc<E2b>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    if let Err(resp) = owned(&e, &key, &id).await {
        return resp;
    }
    if e.kill(&id).await {
        no_content()
    } else {
        api_error(StatusCode::NOT_FOUND, format!("sandbox \"{id}\" not found"))
    }
}

/// `POST /sandboxes/{id}/timeout` `{timeout}`: the lifetime from now.
async fn set_timeout(
    State(e): State<Arc<E2b>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    if let Err(resp) = owned(&e, &key, &id).await {
        return resp;
    }
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let ms = timeout_ms(&b, "timeout");
    let at = now_ms() + ms;
    e.store.update(&id, |r| r.end_at_ms = Some(at)).await;
    // `timeout=0` ends the sandbox now: a killing sandbox is gone before
    // this answers, so `is_running()` right after is false (the reaper alone
    // left a window of a few hundred ms).
    if ms == 0
        && e.store
            .get(&id)
            .await
            .is_some_and(|r| r.on_timeout == OnTimeout::Kill)
    {
        e.kill(&id).await;
    }
    no_content()
}

/// `POST /sandboxes/{id}/refreshes` `{duration}`: extends the lifetime.
async fn refresh(
    State(e): State<Arc<E2b>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    if let Err(resp) = owned(&e, &key, &id).await {
        return resp;
    }
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let at = now_ms() + timeout_ms(&b, "duration");
    e.store
        .update(&id, |r| {
            r.end_at_ms = Some(r.end_at_ms.unwrap_or(0).max(at))
        })
        .await;
    no_content()
}

/// `POST /sandboxes/{id}/connect` and `/v2/...`: resume a paused sandbox
/// (201) or extend a running one's lifetime (200).
async fn connect(
    State(e): State<Arc<E2b>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&e, &key, &id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let at = now_ms() + timeout_ms(&b, "timeout");
    if r.paused {
        // `connect(on_resume="reboot")` (`memory: false`): come back from
        // the kept disk, not the kept memory.
        if b.get("memory").and_then(Value::as_bool) == Some(false) {
            e.store
                .update(&id, |r| {
                    r.extra.insert("reboot_on_resume".into(), Value::Bool(true));
                })
                .await;
        }
        let r = e.store.get(&id).await.unwrap_or(r);
        if let Err(err) = e.resume(&r).await {
            return backend_error(err);
        }
        e.store.update(&id, |r| r.end_at_ms = Some(at)).await;
        let r = e.store.get(&id).await.unwrap_or(r);
        return json_response(StatusCode::CREATED, sandbox_json(&e, &r));
    }
    if e.check_gone(&r).await {
        return api_error(StatusCode::NOT_FOUND, format!("sandbox \"{id}\" not found"));
    }
    e.store
        .update(&id, |r| {
            r.end_at_ms = Some(r.end_at_ms.unwrap_or(0).max(at))
        })
        .await;
    json_response(StatusCode::OK, sandbox_json(&e, &r))
}

/// `POST /sandboxes/{id}/pause` `{memory}`: suspend (RAM kept unless `memory`
/// is false); 409 when paused.
async fn pause(
    State(e): State<Arc<E2b>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&e, &key, &id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    // `pause(mode="filesystem")`: only the disk comes back.
    let keep_memory = b.get("memory").and_then(Value::as_bool) != Some(false);
    if r.paused {
        return api_error(
            StatusCode::CONFLICT,
            format!("sandbox \"{id}\" is already paused"),
        );
    }
    if let Err(err) = e.backend.suspend(&r.identity(), &r.cua_name).await {
        return backend_error(err);
    }
    e.forget(&id).await;
    e.store
        .update(&id, |r| {
            r.paused = true;
            r.end_at_ms = None;
            if keep_memory {
                r.extra.remove("reboot_on_resume");
            } else {
                r.extra.insert("reboot_on_resume".into(), Value::Bool(true));
            }
        })
        .await;
    no_content()
}

/// `POST /sandboxes/{id}/resume` `{timeout, autoPause}`.
async fn resume(
    State(e): State<Arc<E2b>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&e, &key, &id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    if !r.paused {
        return api_error(
            StatusCode::CONFLICT,
            format!("sandbox \"{id}\" is already running"),
        );
    }
    if let Err(err) = e.resume(&r).await {
        return backend_error(err);
    }
    let at = now_ms() + timeout_ms(&b, "timeout");
    let auto_pause = b.get("autoPause").and_then(Value::as_bool);
    e.store
        .update(&id, |r| {
            r.end_at_ms = Some(at);
            if let Some(p) = auto_pause {
                r.on_timeout = if p { OnTimeout::Pause } else { OnTimeout::Kill };
            }
        })
        .await;
    let r = e.store.get(&id).await.unwrap_or(r);
    json_response(StatusCode::CREATED, sandbox_json(&e, &r))
}

/// The namespace E2B puts before a snapshot's name (the team slug there).
fn namespace(key: &KeyEntry) -> String {
    let n = u16::from_str_radix(&key.id()[..4], 16).unwrap_or(0);
    format!("cua-{n}")
}

/// `POST /sandboxes/{id}/snapshots` `{name?}`: a checkpoint of the live
/// sandbox, usable as `create(template=<snapshotID>)`.
async fn snapshot(
    State(e): State<Arc<E2b>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&e, &key, &id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let given = b
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
        .map(|n| n.rsplit('/').next().unwrap_or(n).to_string());
    let name = given.clone().unwrap_or_else(new_id);
    if let Err(err) = e
        .backend
        .checkpoint(&r.identity(), &r.cua_name, &name)
        .await
    {
        return backend_error(err);
    }
    if b.get("memory").and_then(Value::as_bool) == Some(false) {
        e.store.mark_fs_only(&name).await;
    }
    let names = match &given {
        Some(n) => vec![format!("{}/{n}", namespace(&key))],
        None => {
            e.store.mark_unnamed(&name).await;
            vec![]
        }
    };
    json_response(
        StatusCode::CREATED,
        json!({"snapshotID": name, "names": names}),
    )
}

#[derive(serde::Deserialize, Default)]
struct SnapshotQuery {
    #[serde(rename = "sandboxID")]
    sandbox_id: Option<String>,
    limit: Option<usize>,
    #[serde(rename = "nextToken")]
    next_token: Option<String>,
}

/// `GET /snapshots?sandboxID=&limit=&nextToken=`.
async fn list_snapshots(
    State(e): State<Arc<E2b>>,
    headers: HeaderMap,
    Query(q): Query<SnapshotQuery>,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let who = crate::backend::Identity {
        daemon: key.daemon.clone(),
        token: key.token.clone(),
    };
    let cps = match e.backend.checkpoints(&who).await {
        Ok(c) => c,
        Err(err) => return backend_error(err),
    };
    let source = match &q.sandbox_id {
        Some(id) => match owned(&e, &key, id).await {
            Ok(r) => Some(r.cua_name),
            Err(resp) => return resp,
        },
        None => None,
    };
    let mut items = Vec::new();
    for cp in cps {
        if e.store.is_held(&cp.name).await || super::golden::is_golden(&cp.name) {
            continue;
        }
        if source.as_ref().is_some_and(|s| *s != cp.source) {
            continue;
        }
        let names = if e.store.is_unnamed(&cp.name).await {
            vec![]
        } else {
            vec![format!("{}/{}", namespace(&key), cp.name)]
        };
        items.push(json!({"snapshotID": cp.name, "names": names}));
    }
    paginate(items, q.limit, q.next_token.as_deref())
}

fn paginate(items: Vec<Value>, limit: Option<usize>, next: Option<&str>) -> Response<Body> {
    let start: usize = next.and_then(|t| t.parse().ok()).unwrap_or(0);
    let limit = limit.filter(|l| *l > 0).unwrap_or(100);
    let page: Vec<Value> = items.iter().skip(start).take(limit).cloned().collect();
    let mut resp = json_response(StatusCode::OK, Value::Array(page));
    if start + limit < items.len() {
        resp.headers_mut().insert(
            "x-next-token",
            (start + limit).to_string().parse().expect("header"),
        );
    }
    resp
}

/// `DELETE /templates/{id}`: deletes a template this gateway built, else a
/// snapshot (a checkpoint).
async fn delete_template(
    State(e): State<Arc<E2b>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    if let Some(resp) = super::templates::delete_template(&e, &key, &id).await {
        return resp;
    }
    if e.cfg.templates.contains_key(&id) {
        return api_error(
            StatusCode::BAD_REQUEST,
            format!("'{id}' is a template alias from templates.json; edit that file to remove it"),
        );
    }
    if e.store.is_held(&id).await {
        return api_error(
            StatusCode::CONFLICT,
            format!("'{id}' backs running forks and is deleted with the last of them"),
        );
    }
    let who = crate::backend::Identity {
        daemon: key.daemon.clone(),
        token: key.token.clone(),
    };
    let id = id.rsplit('/').next().unwrap_or(&id).to_string();
    match e.backend.delete_checkpoint(&who, &id).await {
        Ok(true) => {
            e.store.forget_snapshot(&id).await;
            no_content()
        }
        Ok(false) => api_error(StatusCode::NOT_FOUND, format!("snapshot '{id}' not found")),
        Err(err) => backend_error(err),
    }
}

/// `POST /sandboxes/{id}/fork` `{timeout, count}`: checkpoint the live
/// sandbox once, then start `count` sandboxes from it. The checkpoint is
/// held until the last fork is gone.
async fn fork(
    State(e): State<Arc<E2b>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&e, &key, &id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let count = b
        .get("count")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, 100) as usize;
    let cp = format!("fork-{id}-{}", &crate::auth::new_token()[..8]);
    if let Err(err) = e.backend.checkpoint(&r.identity(), &r.cua_name, &cp).await {
        return backend_error(err);
    }
    let timeout = timeout_ms(&b, "timeout");
    let tasks = (0..count).map(|_| {
        let n = NewSandbox {
            template_id: r.template_id.clone(),
            workdir: r
                .extra
                .get("workdir")
                .and_then(Value::as_str)
                .map(str::to_string),
            user: r
                .extra
                .get("user")
                .and_then(Value::as_str)
                .map(str::to_string),
            auto_pause_memory: r.extra.get("auto_pause_memory").and_then(Value::as_bool),
            // The rules live in the guest and travel with the memory
            // checkpoint; the record keeps what get_info shows.
            egress: r
                .extra
                .get("network")
                .map(|n| (super::egress::Policy::default(), n.clone())),
            alias: r.alias.clone(),
            internet_set: r.extra.get("internet_set").is_some(),
            source: Source::Checkpoint(cp.clone()),
            timeout_ms: timeout,
            metadata: r.metadata.clone(),
            env_vars: r.env_vars.clone(),
            internet: r.internet,
            on_timeout: r.on_timeout,
            auto_resume: r.auto_resume,
            public_traffic: r.traffic_token.is_none(),
            held_checkpoint: Some(cp.clone()),
            place: placement_from_metadata(&r.metadata),
        };
        let e = e.clone();
        let key = key.clone();
        async move { start_sandbox(&e, &key, n).await }
    });
    let results = futures_util::future::join_all(tasks).await;
    let mut out = Vec::new();
    let mut any = false;
    for res in results {
        match res {
            Ok(fr) => {
                any = true;
                out.push(json!({"sandbox": sandbox_json(&e, &fr)}));
            }
            Err(err) => {
                let status = match err {
                    BackendError::NotFound(_) => 404,
                    BackendError::Invalid(_) | BackendError::Unsupported(_) => 400,
                    _ => 500,
                };
                out.push(json!({"error": {"code": status, "message": err.to_string()}}));
            }
        }
    }
    if !any {
        let _ = e.backend.delete_checkpoint(&r.identity(), &cp).await;
    }
    json_response(StatusCode::CREATED, Value::Array(out))
}

/// `GET /sandboxes/{id}/metrics`: one sample from the guest.
async fn metrics(
    State(e): State<Arc<E2b>>,
    Path(id): Path<String>,
    Query(window): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    // E2B answers an empty list for a sandbox that is gone or not yours.
    let Ok(r) = owned(&e, &key, &id).await else {
        return json_response(StatusCode::OK, json!([]));
    };
    if r.paused {
        return json_response(StatusCode::OK, json!([]));
    }
    let c = match e.spacesd(&r).await {
        Ok(c) => c,
        Err(err) => return backend_error(err),
    };
    let m = match c.metrics().await {
        Ok(m) => m,
        Err(err) => return api_error(StatusCode::BAD_GATEWAY, err.to_string()),
    };
    // `start` / `end` (Unix seconds): the one sample this gateway has is the
    // current one, so it answers only a window that includes now.
    let now = now_ms() / 1000;
    let bound = |k: &str| window.get(k).and_then(|v| v.parse::<u64>().ok());
    if bound("start").is_some_and(|s| s > now) || bound("end").is_some_and(|t| t < now) {
        return json_response(StatusCode::OK, json!([]));
    }
    json_response(StatusCode::OK, json!([metric_json(&m)]))
}

/// One metrics sample, E2B's shape.
fn metric_json(m: &cua_spacesd_client::pb::MetricsResponse) -> Value {
    let ts_ms = m
        .sampled_at
        .as_ref()
        .map(|t| (t.seconds.max(0) as u64) * 1000 + (t.nanos.max(0) as u64) / 1_000_000)
        .unwrap_or_else(now_ms);
    json!({
        "timestamp": rfc3339(ts_ms),
        "timestampUnix": ts_ms / 1000,
        "cpuCount": m.cpu_count,
        "cpuUsedPct": m.cpu_percent,
        "memUsed": m.memory_used_bytes,
        "memTotal": m.memory_total_bytes,
        "memCache": 0,
        "diskUsed": m.disk_used_bytes,
        "diskTotal": m.disk_total_bytes,
    })
}

/// `GET /sandboxes/metrics?sandbox_ids=a,b`: the latest metric of each.
async fn batch_metrics(
    State(e): State<Arc<E2b>>,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let ids: Vec<String> = q
        .as_deref()
        .map(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .filter(|(k, _)| k == "sandbox_ids")
                .flat_map(|(_, v)| v.split(',').map(str::to_string).collect::<Vec<_>>())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "validation error: error in openapi3filter.RequestError: parameter \"sandbox_ids\" in query has an error: value is required but missing",
        );
    }
    let mut out = serde_json::Map::new();
    for id in ids {
        let Ok(r) = owned(&e, &key, &id).await else {
            continue;
        };
        if r.paused {
            continue;
        }
        let Ok(c) = e.spacesd(&r).await else { continue };
        if let Ok(m) = c.metrics().await {
            out.insert(id, metric_json(&m));
        }
    }
    json_response(StatusCode::OK, json!({"sandboxes": out}))
}

/// The caller's records matching `metadata` (`k=v&k2=v2`) and `state`.
async fn filtered(
    e: &E2b,
    key: &KeyEntry,
    raw: Option<&str>,
) -> (Vec<Record>, HashMap<String, String>) {
    let params: HashMap<String, String> = raw
        .map(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .into_owned()
                .collect()
        })
        .unwrap_or_default();
    let want: Vec<(String, String)> = params
        .get("metadata")
        .map(|m| {
            url::form_urlencoded::parse(m.as_bytes())
                .into_owned()
                .collect()
        })
        .unwrap_or_default();
    let states: Vec<String> = params
        .get("state")
        .map(|s| s.split(',').map(str::to_string).collect())
        .unwrap_or_default();
    let template = params.get("template").cloned();
    let started_after_ms = params.get("startedAfter").and_then(|t| {
        time::OffsetDateTime::parse(t, &time::format_description::well_known::Rfc3339)
            .ok()
            .map(|d| (d.unix_timestamp_nanos() / 1_000_000) as u64)
    });
    let records = e
        .store
        .all()
        .await
        .into_iter()
        .filter(|r| r.key_id == key.id())
        .filter(|r| template.as_ref().is_none_or(|t| *t == r.template_id))
        .filter(|r| started_after_ms.is_none_or(|t| r.started_at_ms >= t))
        .filter(|r| want.iter().all(|(k, v)| r.metadata.get(k) == Some(v)))
        .filter(|r| states.is_empty() || states.iter().any(|s| s == state_name(r)))
        .collect();
    (records, params)
}

/// `GET /sandboxes` (running ones; v1 shape).
async fn list_v1(
    State(e): State<Arc<E2b>>,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let (records, _) = filtered(&e, &key, q.as_deref()).await;
    json_response(
        StatusCode::OK,
        Value::Array(
            records
                .iter()
                .filter(|r| !r.paused)
                .map(listed_json)
                .collect(),
        ),
    )
}

/// `GET /v2/sandboxes?metadata=&state=&limit=&nextToken=`.
async fn list_v2(
    State(e): State<Arc<E2b>>,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let (mut records, params) = filtered(&e, &key, q.as_deref()).await;
    // Newest first unless `order=asc` (E2B's default).
    if params.get("order").map(String::as_str) != Some("asc") {
        records.reverse();
    }
    let items: Vec<Value> = records.iter().map(listed_json).collect();
    paginate(
        items,
        params.get("limit").and_then(|l| l.parse().ok()),
        params.get("nextToken").map(String::as_str),
    )
}

async fn no_build() -> Response<Body> {
    api_error(
        StatusCode::BAD_REQUEST,
        "this template API is the deprecated v1/v2 Dockerfile build; use the Template builder (e2b.Template, which builds through /v3/templates)",
    )
}

/// `PUT /sandboxes/{id}/network` `{allowOut, denyOut, allow_internet_access}`:
/// replaces the sandbox's outbound rules.
async fn update_network(
    State(e): State<Arc<E2b>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&e, &key, &id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    for k in ["rules", "egressProxy"] {
        if b.get(k)
            .is_some_and(|v| !v.is_null() && v.as_object().is_none_or(|o| !o.is_empty()))
        {
            return api_error(
                StatusCode::BAD_REQUEST,
                format!(
                    "network.{k}: rewriting outbound requests needs an inspecting egress proxy, which Cua Spaces do not have; allow_out and deny_out take addresses and CIDR blocks"
                ),
            );
        }
    }
    let internet = b.get("allow_internet_access").and_then(Value::as_bool);
    let (policy, shown) = match network_policy(&b, internet == Some(false)) {
        Ok(x) => x,
        Err(msg) => return api_error(StatusCode::BAD_REQUEST, format!("network: {msg}")),
    };
    if r.paused {
        return api_error(
            StatusCode::CONFLICT,
            format!("sandbox \"{id}\" is paused: resume it to change its network"),
        );
    }
    if let Err(err) = super::egress::apply(&e, &r, &policy).await {
        return backend_error(err);
    }
    e.store
        .update(&id, |r| {
            r.extra.insert("network".into(), shown);
            if let Some(i) = internet {
                r.internet = i;
                r.extra.insert("internet_set".into(), Value::Bool(true));
            }
        })
        .await;
    no_content()
}

async fn no_logs() -> Response<Body> {
    api_error(
        StatusCode::NOT_FOUND,
        "sandbox logs are not available from the Cua gateway; read process output with commands.run",
    )
}

async fn no_volumes(State(e): State<Arc<E2b>>, headers: HeaderMap) -> Response<Body> {
    if let Err(r) = authed(&e, &headers).await {
        return r;
    }
    // The answer the live service gives a team that has not enabled volumes.
    api_error(StatusCode::FORBIDDEN, "use of volumes is not enabled")
}
