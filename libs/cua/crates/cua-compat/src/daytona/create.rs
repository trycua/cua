//! `POST /sandbox`: the create request, its validation, and making the
//! sandbox (a Space) it describes.
//!
//! The request is `CreateSandbox` (daytona_api_client 0.225.0
//! `models/create_sandbox.py`) as `Daytona.create` builds it
//! (`daytona/_sync/daytona.py` `_create`, which has already refused negative
//! intervals, `auto_stop_interval` with `auto_pause_interval`, ephemeral with
//! auto-pause and a non-positive queue timeout; the same rules are enforced
//! here for callers that are not the SDK).

use super::api::{self, New, sandbox_dto, start_sandbox, string_map};
use super::volumes::{self, Mount};
use super::{Daytona, backend_error, error, ok_json};
use crate::auth::KeyEntry;
use crate::backend::{BackendError, CreateExtras, Placement, Source};
use axum::body::Body;
use axum::extract::State;
use http::{HeaderMap, Response, StatusCode};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A validated `CreateSandbox`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct CreateReq {
    pub name: Option<String>,
    pub snapshot: Option<String>,
    pub user: Option<String>,
    pub env: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
    pub public: Option<bool>,
    pub target: Option<String>,
    pub cpu: Option<u32>,
    pub memory_gib: Option<u32>,
    pub disk_gib: Option<u32>,
    pub gpu: u32,
    pub spot: bool,
    pub kvm: bool,
    pub network_block_all: bool,
    pub network_allow_list: Option<String>,
    pub domain_allow_list: Option<String>,
    pub outbound_proxy_url: Option<String>,
    pub otel_endpoint_override: Option<String>,
    pub auto_stop: Option<i64>,
    pub auto_pause: Option<i64>,
    pub auto_archive: Option<i64>,
    pub auto_delete: Option<i64>,
    pub ttl_minutes: Option<i64>,
    pub queue_timeout: Option<i64>,
    pub volumes: Vec<Mount>,
    pub secrets: Vec<(String, String)>,
    pub dockerfile: Option<String>,
    pub context_hashes: Vec<String>,
    pub linked_sandbox: Option<String>,
}

/// POSIX-portable user names (they go into `useradd`).
pub(crate) fn valid_user(u: &str) -> bool {
    let mut c = u.chars();
    c.next().is_some_and(|f| f.is_ascii_lowercase() || f == '_')
        && u.len() <= 32
        && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn int(b: &Value, k: &str) -> Result<Option<i64>, String> {
    match b.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
            .map(Some)
            .ok_or_else(|| format!("{k} must be an integer number")),
    }
}

fn string(b: &Value, k: &str) -> Result<Option<String>, String> {
    match b.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{k} must be a string")),
    }
}

fn boolean(b: &Value, k: &str) -> Result<Option<bool>, String> {
    match b.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(v)) => Ok(Some(*v)),
        Some(_) => Err(format!("{k} must be a boolean value")),
    }
}

/// Comma-separated IPv4 CIDRs (`10.0.0.0/8,1.2.3.4/32`).
pub(crate) fn valid_cidr_list(s: &str) -> bool {
    !s.is_empty()
        && s.split(',').all(|c| {
            let c = c.trim();
            let Some((ip, bits)) = c.split_once('/') else {
                return false;
            };
            ip.parse::<std::net::Ipv4Addr>().is_ok() && bits.parse::<u8>().is_ok_and(|b| b <= 32)
        })
}

impl CreateReq {
    pub(crate) fn parse(b: &Value) -> Result<CreateReq, String> {
        if !b.is_object() {
            return Err("the request body must be a JSON object".into());
        }
        let positive = |k: &str, v: Option<i64>| -> Result<Option<u32>, String> {
            match v {
                Some(n) if n >= 1 => u32::try_from(n)
                    .map(Some)
                    .map_err(|_| format!("{k} is too large")),
                Some(_) => Err(format!("{k} must not be less than 1")),
                None => Ok(None),
            }
        };
        let mut r = CreateReq {
            name: string(b, "name")?,
            snapshot: string(b, "snapshot")?.filter(|s| !s.is_empty()),
            user: string(b, "user")?.filter(|s| !s.is_empty()),
            public: boolean(b, "public")?,
            target: string(b, "target")?.filter(|s| !s.is_empty()),
            cpu: positive("cpu", int(b, "cpu")?)?,
            memory_gib: positive("memory", int(b, "memory")?)?,
            disk_gib: positive("disk", int(b, "disk")?)?,
            gpu: int(b, "gpu")?.unwrap_or(0).max(0) as u32,
            spot: boolean(b, "spot")?.unwrap_or(false),
            kvm: boolean(b, "kvm")?.unwrap_or(false),
            network_block_all: boolean(b, "networkBlockAll")?.unwrap_or(false),
            network_allow_list: string(b, "networkAllowList")?.filter(|s| !s.is_empty()),
            domain_allow_list: string(b, "domainAllowList")?.filter(|s| !s.is_empty()),
            outbound_proxy_url: string(b, "outboundProxyUrl")?.filter(|s| !s.is_empty()),
            otel_endpoint_override: string(b, "otelEndpointOverride")?.filter(|s| !s.is_empty()),
            auto_stop: int(b, "autoStopInterval")?,
            auto_pause: int(b, "autoPauseInterval")?,
            auto_archive: int(b, "autoArchiveInterval")?,
            auto_delete: int(b, "autoDeleteInterval")?,
            ttl_minutes: int(b, "ttlMinutes")?,
            queue_timeout: int(b, "queueTimeout")?,
            env: string_map(b.get("env")),
            labels: string_map(b.get("labels")),
            linked_sandbox: string(b, "linkedSandbox")?.filter(|s| !s.is_empty()),
            ..Default::default()
        };
        if let Some(n) = &r.name
            && (n.is_empty() || n.len() > 255 || n.contains('/') || n.chars().any(char::is_control))
        {
            return Err("name must be 1-255 characters without '/' or control characters".into());
        }
        if let Some(u) = &r.user
            && !valid_user(u)
        {
            return Err(format!("user {u:?} is not a valid user name"));
        }
        for (k, v) in [
            ("autoStopInterval", r.auto_stop),
            ("autoPauseInterval", r.auto_pause),
            ("autoArchiveInterval", r.auto_archive),
            ("ttlMinutes", r.ttl_minutes),
        ] {
            if v.is_some_and(|n| n < 0) {
                return Err(format!("{k} must not be less than 0"));
            }
        }
        if r.auto_stop.unwrap_or(0) != 0 && r.auto_pause.unwrap_or(0) != 0 {
            return Err(
                "autoStopInterval and autoPauseInterval are mutually exclusive. Set at most one of them to a non-zero value".into(),
            );
        }
        if r.auto_pause.unwrap_or(0) != 0 && r.auto_delete == Some(0) {
            return Err("Ephemeral sandboxes cannot have auto-pause enabled".into());
        }
        if r.queue_timeout.is_some_and(|q| q < 1) {
            return Err("queueTimeout must be a positive integer".into());
        }
        if let Some(l) = &r.network_allow_list
            && !valid_cidr_list(l)
        {
            return Err(format!(
                "networkAllowList {l:?} must be comma-separated IPv4 CIDRs"
            ));
        }
        if let Some(l) = &r.domain_allow_list
            && !l.split(',').all(|h| super::secrets::valid_host(h.trim()))
        {
            return Err(format!(
                "domainAllowList {l:?} must be comma-separated domains"
            ));
        }
        if let Some(u) = &r.outbound_proxy_url {
            let ok = url::Url::parse(u)
                .map(|u| {
                    matches!(u.scheme(), "http" | "https" | "socks5" | "socks5h")
                        && u.host().is_some()
                })
                .unwrap_or(false);
            if !ok {
                return Err(format!(
                    "outboundProxyUrl {u:?} must be an http(s) or socks5 URL"
                ));
            }
        }
        r.volumes = volumes::parse_mounts(b.get("volumes"))?;
        if let Some(a) = b.get("secrets").and_then(Value::as_array) {
            for item in a {
                let o = item
                    .as_object()
                    .ok_or("each secret must be an {ENV: secret name} object")?;
                for (env, name) in o {
                    let name = name.as_str().ok_or("a secret name must be a string")?;
                    r.secrets.push((env.clone(), name.to_string()));
                }
            }
        }
        if let Some(bi) = b.get("buildInfo").filter(|v| !v.is_null()) {
            r.dockerfile = string(bi, "dockerfileContent")?.filter(|s| !s.trim().is_empty());
            r.context_hashes = bi
                .get("contextHashes")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|h| h.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if r.dockerfile.is_none() {
                return Err("buildInfo.dockerfileContent should not be empty".into());
            }
            if r.snapshot.is_some() {
                return Err("snapshot and buildInfo are mutually exclusive".into());
            }
        }
        Ok(r)
    }
}

/// Environment a sandbox gets from the create's network options: the
/// outbound proxy (`HTTP(S)_PROXY`, as Daytona applies it) and the OTel
/// endpoint override.
pub(crate) fn derived_env(req: &CreateReq) -> BTreeMap<String, String> {
    let mut e = BTreeMap::new();
    if let Some(p) = &req.outbound_proxy_url {
        for k in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            e.insert(k.to_string(), p.clone());
        }
    }
    if let Some(o) = &req.otel_endpoint_override {
        e.insert("OTEL_EXPORTER_OTLP_ENDPOINT".into(), o.clone());
    }
    e
}

/// `POST /api/sandbox`.
pub(super) async fn create(
    State(d): State<Arc<Daytona>>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match d.caller(&headers).await {
        Some(k) => k,
        None => return error(StatusCode::UNAUTHORIZED, "Invalid credentials"),
    };
    let b = match api::body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let req = match CreateReq::parse(&b) {
        Ok(r) => r,
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    // The creation runs to the end even when the client gives up (its
    // timeout): the sandbox is then there to be found, as with Daytona,
    // rather than a Space nobody has a record of.
    let (d2, key2) = (d.clone(), key.clone());
    match tokio::spawn(async move { make(&d2, &key2, req).await }).await {
        Ok(r) => r,
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("create failed: {e}"),
        ),
    }
}

async fn make(d: &Arc<Daytona>, key: &KeyEntry, req: CreateReq) -> Response<Body> {
    if let Some(name) = &req.name
        && d.find(key, name).await.is_some()
    {
        return error(
            StatusCode::CONFLICT,
            format!("Sandbox with name {name} already exists"),
        );
    }
    let mounts = match volumes::resolve_mounts(d, key, &req.volumes).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let secrets = match super::secrets::resolve(d, key, &req.secrets, &req.labels).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    // What it starts from.
    let snapshot = req.snapshot.clone().unwrap_or_default();
    let mut extras = CreateExtras {
        disk_gb: req.disk_gib,
        gpu: (req.gpu > 0).then(|| "auto".to_string()),
        ..Default::default()
    };
    let mut built = None;
    let source = if let Some(df) = &req.dockerfile {
        match super::build::resolve(d, df, &req.context_hashes).await {
            Ok(b) => {
                extras.build = b.build.clone();
                extras.command = b.command.clone();
                let s = Source::Image(Some(b.from.clone()));
                built = Some(b);
                s
            }
            Err(m) => return error(StatusCode::BAD_REQUEST, m),
        }
    } else {
        match api::resolve_snapshot(d, key, &snapshot).await {
            Ok((s, snap_extras)) => {
                if let Some(b) = snap_extras.build {
                    extras.build = Some(b);
                }
                if snap_extras.command.is_some() {
                    extras.command = snap_extras.command;
                }
                s
            }
            Err(r) => return r,
        }
    };
    // What the snapshot brings: its resources, and its warm pool.
    let mut req = req;
    if req.dockerfile.is_none()
        && !snapshot.is_empty()
        && let Some(sn) = super::snapshots::find(d, &key.id(), &snapshot).await
    {
        if req.cpu.is_none() && sn.cpu > 0 {
            req.cpu = Some(sn.cpu);
        }
        if req.memory_gib.is_none() && sn.memory > 0 {
            req.memory_gib = Some(sn.memory);
        }
        if req.disk_gib.is_none() && sn.disk > 0 {
            req.disk_gib = Some(sn.disk);
            extras.disk_gb = Some(sn.disk);
        }
        let kid = key.id();
        if let Some(w) = d
            .warm_pools
            .items
            .lock()
            .await
            .iter()
            .find(|w| w.key_id == kid && w.snapshot_id == sn.id)
        {
            extras.pool = Some(w.cua_pool.clone());
        }
        d.snapshots
            .lock()
            .await
            .iter_mut()
            .filter(|s| s.id == sn.id)
            .for_each(|s| s.last_used_ms = Some(crate::state::now_ms()));
    }
    // Placement: the labels `cua.on` / `cua.kind` (and a linked sandbox's
    // machine) over the gateway defaults.
    let place = Placement {
        on: req.labels.get("cua.on").cloned(),
        kind: req.labels.get("cua.kind").cloned(),
        cpus: req.cpu,
        memory_mb: req.memory_gib.map(|g| u64::from(g) * 1024),
        // Computer use is part of a Daytona sandbox: the desktop is on unless
        // `cua.desktop=false` trades it for a faster boot.
        desktop: req.labels.get("cua.desktop").is_none_or(|v| v != "false"),
    };
    if req.linked_sandbox.is_some() {
        // Daytona co-locates the two sandboxes on one runner network and
        // resolves the owner's name from the follower. Cua Spaces each have
        // a private guest network (QEMU user networking, containers, Lume),
        // so no name or address of one reaches another: refuse instead of
        // accepting a link that does nothing. Share data over Cua Volume or
        // a preview URL, or run both in one Space.
        return error(
            StatusCode::BAD_REQUEST,
            "linkedSandbox is not supported: Cua Spaces have separate guest networks, so a linked sandbox could not reach its owner. Share files through Cua Volume or preview URLs, or run both workloads in one Space",
        );
    }
    let mut env = req.env.clone();
    env.extend(derived_env(&req));
    env.extend(secrets.iter().cloned());
    if let Some(b) = &built {
        for (k, v) in &b.env {
            env.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    let mut extra = Map::new();
    for (k, v) in [
        ("name", req.name.clone().map(Value::from)),
        (
            "user",
            req.user
                .clone()
                .or_else(|| built.as_ref().and_then(|b| b.user.clone()))
                .map(Value::from),
        ),
        ("public", req.public.map(Value::from)),
        ("target", req.target.clone().map(Value::from)),
        ("autoStopInterval", req.auto_stop.map(Value::from)),
        ("autoPauseInterval", req.auto_pause.map(Value::from)),
        ("autoArchiveInterval", req.auto_archive.map(Value::from)),
        ("autoDeleteInterval", req.auto_delete.map(Value::from)),
        ("ttlMinutes", req.ttl_minutes.map(Value::from)),
        ("queueTimeout", req.queue_timeout.map(Value::from)),
        ("disk", req.disk_gib.map(Value::from)),
        ("gpu", (req.gpu > 0).then(|| Value::from(req.gpu))),
        ("kvm", req.kvm.then(|| Value::from(true))),
        ("spot", req.spot.then(|| Value::from(true))),
        (
            "networkAllowList",
            req.network_allow_list.clone().map(Value::from),
        ),
        (
            "domainAllowList",
            req.domain_allow_list.clone().map(Value::from),
        ),
        (
            "outboundProxyUrl",
            req.outbound_proxy_url.clone().map(Value::from),
        ),
        (
            "otelEndpointOverride",
            req.otel_endpoint_override.clone().map(Value::from),
        ),
        (
            "workDir",
            built
                .as_ref()
                .and_then(|b| b.workdir.clone())
                .map(Value::from),
        ),
        ("buildInfo", built.as_ref().map(|b| b.info.clone())),
    ] {
        if let Some(v) = v {
            extra.insert(k.into(), v);
        }
    }
    if !req.volumes.is_empty() {
        extra.insert(
            "volumes".into(),
            Value::Array(
                mounts
                    .iter()
                    .map(|(v, m)| {
                        let mut o = json!({"volumeId": v.id, "mountPath": m.path});
                        if let Some(s) = &m.subpath {
                            o["subpath"] = json!(s);
                        }
                        o
                    })
                    .collect(),
            ),
        );
    }
    if !req.secrets.is_empty() {
        extra.insert(
            "secrets".into(),
            Value::Array(req.secrets.iter().map(|(e, n)| json!({e: n})).collect()),
        );
    }
    extra.insert("labels".into(), json!(req.labels));
    let n = New {
        source,
        snapshot: if snapshot.is_empty() {
            built
                .as_ref()
                .map_or_else(|| "base".to_string(), |b| format!("build:{}", b.from))
        } else {
            snapshot
        },
        env,
        internet: !req.network_block_all,
        place,
        extra,
        held: None,
        extras,
    };
    let pending = n.extras.build.is_some();
    let r = match start_sandbox(d, key, n, pending).await {
        Ok(r) => r,
        Err(e) => return backend_error(e),
    };
    if pending {
        // The build runs on; the client waits for `started` (events or
        // polling) and reads the build log from `/build-logs`.
        return ok_json(sandbox_dto(d, &r));
    }
    match api::finish_create(d, key, r, &mounts, req.kvm).await {
        Ok(r) => ok_json(sandbox_dto(d, &r)),
        Err(resp) => resp,
    }
}

/// Maps a backend error of a create to the client's.
#[allow(dead_code)]
pub(crate) fn _backend_error(e: BackendError) -> Response<Body> {
    backend_error(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn p(v: Value) -> Result<CreateReq, String> {
        CreateReq::parse(&v)
    }

    #[test]
    fn the_sdk_rules_hold_on_the_server_too() {
        assert!(p(json!({"autoStopInterval": -1})).is_err());
        assert!(p(json!({"autoStopInterval": 5, "autoPauseInterval": 5})).is_err());
        assert!(p(json!({"autoPauseInterval": 5, "autoDeleteInterval": 0})).is_err());
        assert!(p(json!({"queueTimeout": 0})).is_err());
        assert!(p(json!({"cpu": 0})).is_err());
        assert!(p(json!({"user": "bob; rm -rf /"})).is_err());
        assert!(p(json!({"networkAllowList": "10.0.0.0/33"})).is_err());
        assert!(p(json!({"outboundProxyUrl": "ftp://x"})).is_err());
        assert!(p(json!({"snapshot": "a", "buildInfo": {"dockerfileContent": "FROM x"}})).is_err());
        let ok = p(json!({
            "name": "n", "user": "daytona", "env": {"A": "1"}, "labels": {"x": "y"},
            "cpu": 2, "memory": 4, "disk": 10, "autoStopInterval": 0, "autoDeleteInterval": 0,
            "networkAllowList": "10.0.0.0/8,1.2.3.4/32", "outboundProxyUrl": "http://proxy:3128",
            "secrets": [{"KEY": "prod"}],
            "volumes": [{"volumeId": "v", "mountPath": "/data"}],
            "buildInfo": {"dockerfileContent": "FROM python:3.12\n", "contextHashes": ["h"]},
        }))
        .unwrap();
        assert_eq!(ok.memory_gib, Some(4));
        assert_eq!(ok.secrets, vec![("KEY".to_string(), "prod".to_string())]);
        assert_eq!(derived_env(&ok)["HTTPS_PROXY"], "http://proxy:3128");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn arbitrary_json_never_panics(s in "\\PC{0,200}") {
            if let Ok(v) = serde_json::from_str::<Value>(&s) {
                let _ = CreateReq::parse(&v);
            }
        }

        #[test]
        fn intervals_are_never_accepted_negative(n in -50i64..50, k in 0usize..4) {
            let key = ["autoStopInterval", "autoPauseInterval", "autoArchiveInterval", "ttlMinutes"][k];
            let r = CreateReq::parse(&json!({ key: n }));
            prop_assert_eq!(r.is_err(), n < 0);
        }

        #[test]
        fn user_names_cannot_inject(u in "\\PC{0,40}") {
            if valid_user(&u) {
                prop_assert!(u.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'));
            }
        }
    }
}
