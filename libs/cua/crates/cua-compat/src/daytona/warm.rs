//! Daytona warm pools (`/warm-pools`) on Cua pools.
//!
//! A Daytona warm pool keeps N started sandboxes of a snapshot ready so a
//! create from that snapshot is instant. Cua has the same thing: a pool of
//! ready sandboxes of an image (`Sandboxes.create_pool`) that creates draw
//! from and the daemon refills. The warm pool's snapshot must name an image
//! (or a Dockerfile base): a checkpoint snapshot is a copy-on-write fork
//! source that is already instant, so a pool of it is refused. Cua pools run
//! on this machine.
//!
//! Shapes: `daytona_api_client` 0.225.0 `warm_pools_api.py`, `WarmPool`,
//! `CreateWarmPool`, `UpdateWarmPool`.

use super::api::{authed, rfc3339};
use super::stores::WarmPoolEntry;
use super::{Daytona, backend_error, error, ok_json};
use crate::backend::Placement;
use crate::e2b::TemplateTarget;
use crate::state::now_ms;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::routing::{get, patch};
use http::{HeaderMap, Response, StatusCode};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn routes(d: Arc<Daytona>) -> Router {
    Router::new()
        .route("/api/warm-pools", get(list).post(create))
        .route("/api/warm-pools/{id}", patch(update).delete(remove))
        .with_state(d)
}

async fn dto(d: &Daytona, w: &WarmPoolEntry) -> Value {
    let ready = d
        .backend
        .pool_ready(&w.identity(), &w.cua_pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(0);
    json!({
        "id": w.id,
        "organizationId": "cua",
        "snapshot": w.snapshot,
        "target": w.target,
        "pool": w.pool,
        "currentSize": ready,
        "cpu": w.cpu,
        "mem": w.mem,
        "disk": w.disk,
        "osUser": w.os_user,
        "env": {},
        "errorReason": w.error_reason,
        "createdAt": rfc3339(w.created_ms),
        "updatedAt": rfc3339(w.updated_ms),
    })
}

async fn list(State(d): State<Arc<Daytona>>, h: HeaderMap) -> Response<Body> {
    let key = match authed(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let kid = key.id();
    let items = d.warm_pools.items.lock().await.clone();
    let mut out = Vec::new();
    for w in items.iter().filter(|w| w.key_id == kid) {
        out.push(dto(&d, w).await);
    }
    ok_json(Value::Array(out))
}

fn pool_size(b: &Value) -> Result<u32, String> {
    let n = b
        .get("pool")
        .and_then(|v| {
            v.as_u64().or_else(|| {
                v.as_f64()
                    .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                    .map(|f| f as u64)
            })
        })
        .ok_or("pool must be a number")?;
    u32::try_from(n).map_err(|_| "pool is too large".to_string())
}

async fn create(State(d): State<Arc<Daytona>>, h: HeaderMap, body: Body) -> Response<Body> {
    let key = match authed(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let b = match super::api::body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let Some(snapshot) = b
        .get("snapshot")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return error(StatusCode::BAD_REQUEST, "snapshot should not be empty");
    };
    let pool = match pool_size(&b) {
        Ok(n) if n >= 1 => n,
        Ok(_) => return error(StatusCode::BAD_REQUEST, "pool must not be less than 1"),
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    let kid = key.id();
    let Some(snap) = super::snapshots::find(&d, &kid, snapshot).await else {
        return error(
            StatusCode::NOT_FOUND,
            format!("Snapshot {snapshot} not found"),
        );
    };
    if snap.state != "active" {
        return error(
            StatusCode::BAD_REQUEST,
            format!("Snapshot {} is {}", snap.name, snap.state),
        );
    }
    let TemplateTarget::Image(base) = &snap.target else {
        return error(
            StatusCode::BAD_REQUEST,
            format!(
                "Snapshot {} is a checkpoint of a sandbox: creates from it are already copy-on-write forks, so a pool would add nothing",
                snap.name
            ),
        );
    };
    let image = snap.image_name.clone().unwrap_or_else(|| base.clone());
    let target = b
        .get("target")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("cua")
        .to_string();
    let mut items = d.warm_pools.items.lock().await;
    if items
        .iter()
        .any(|w| w.key_id == kid && w.snapshot_id == snap.id && w.target == target)
    {
        return error(
            StatusCode::CONFLICT,
            format!(
                "A warm pool for snapshot {} in {target} already exists",
                snap.name
            ),
        );
    }
    let id = super::api_uuid();
    let now = now_ms();
    let w = WarmPoolEntry {
        cua_pool: format!("daytona-wp-{}", &id[..12]),
        id,
        key_id: kid,
        identity_daemon: key.daemon.clone(),
        identity_token: key.token.clone(),
        snapshot: snap.name.clone(),
        snapshot_id: snap.id.clone(),
        target,
        pool,
        cpu: snap.cpu.max(1),
        mem: snap.memory.max(1),
        disk: snap.disk,
        os_user: d.cfg.default_user.clone(),
        error_reason: None,
        created_ms: now,
        updated_ms: now,
    };
    let place = Placement {
        cpus: Some(w.cpu),
        memory_mb: Some(u64::from(w.mem) * 1024),
        ..Default::default()
    };
    if let Err(e) = d
        .backend
        .pool_create(&w.identity(), &w.cua_pool, &image, pool, &place)
        .await
    {
        return backend_error(e);
    }
    items.push(w.clone());
    drop(items);
    d.warm_pools.save().await;
    Response::builder()
        .status(StatusCode::CREATED)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(dto(&d, &w).await.to_string()))
        .expect("response")
}

async fn update(
    State(d): State<Arc<Daytona>>,
    h: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response<Body> {
    let key = match authed(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let b = match super::api::body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let size = match pool_size(&b) {
        Ok(n) => n,
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    let kid = key.id();
    let found = d
        .warm_pools
        .items
        .lock()
        .await
        .iter()
        .find(|w| w.id == id && w.key_id == kid)
        .cloned();
    let Some(w) = found else {
        return error(StatusCode::NOT_FOUND, format!("Warm pool {id} not found"));
    };
    // The core pool is re-created with the new size (an existing name
    // updates it); 0 drains it.
    let snap = super::snapshots::find(&d, &kid, &w.snapshot_id).await;
    let image = snap.as_ref().and_then(|s| match &s.target {
        TemplateTarget::Image(i) => Some(s.image_name.clone().unwrap_or_else(|| i.clone())),
        _ => None,
    });
    let res = if size == 0 {
        d.backend
            .pool_delete(&w.identity(), &w.cua_pool)
            .await
            .map(|_| ())
    } else if let Some(image) = image {
        let place = Placement {
            cpus: Some(w.cpu),
            memory_mb: Some(u64::from(w.mem) * 1024),
            ..Default::default()
        };
        d.backend
            .pool_create(&w.identity(), &w.cua_pool, &image, size, &place)
            .await
    } else {
        return error(
            StatusCode::BAD_REQUEST,
            format!("Snapshot {} no longer exists", w.snapshot),
        );
    };
    if let Err(e) = res {
        return backend_error(e);
    }
    let mut items = d.warm_pools.items.lock().await;
    let Some(x) = items.iter_mut().find(|x| x.id == w.id) else {
        return error(StatusCode::NOT_FOUND, format!("Warm pool {id} not found"));
    };
    x.pool = size;
    x.updated_ms = now_ms();
    let out = x.clone();
    drop(items);
    d.warm_pools.save().await;
    ok_json(dto(&d, &out).await)
}

async fn remove(
    State(d): State<Arc<Daytona>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    let key = match authed(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let kid = key.id();
    let found = d
        .warm_pools
        .items
        .lock()
        .await
        .iter()
        .find(|w| w.id == id && w.key_id == kid)
        .cloned();
    let Some(w) = found else {
        return error(StatusCode::NOT_FOUND, format!("Warm pool {id} not found"));
    };
    if let Err(e) = d.backend.pool_delete(&w.identity(), &w.cua_pool).await {
        return backend_error(e);
    }
    d.warm_pools.items.lock().await.retain(|x| x.id != w.id);
    d.warm_pools.save().await;
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .expect("response")
}
